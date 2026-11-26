//! `hc analyze seams`: what lemma boundaries hide (`tools/lean/Seams.lean`, together with
//! `Flat.lean` for the single-caller lemmas and repeated facts), plus checks on the source text
//! that terms cannot show: a conclusion restated as a type at its one call, and the same `have`
//! proved twice in one declaration.

use crate::analyze::{default_imports, module_path, repo_root};
use crate::analyze_flat::{have_lines, parse_singles, FLAT_LEAN};
use crate::analyze_more::run_lean;
use anyhow::{bail, Result};
use hc_core::config::Config;
use hc_core::lexer;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const SEAMS_LEAN: &str = include_str!("../../../tools/lean/Seams.lean");

fn indent(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ').count()
}

/// The lines of a `have` block starting at line `i` (0-based): the line itself and the
/// following lines indented more deeply (blank lines included while the block goes on).
pub fn block_end(lines: &[&str], i: usize) -> usize {
    let base = indent(lines[i]);
    let mut j = i + 1;
    while j < lines.len() && (lines[j].trim().is_empty() || indent(lines[j]) > base) {
        j += 1;
    }
    while j > i + 1 && lines[j - 1].trim().is_empty() {
        j -= 1;
    }
    j
}

pub fn tokens(text: &str) -> usize {
    lexer::tokenize(text).len()
}

/// A `have` in the text: line (1-based), name, stated type (whitespace normalized), the
/// expression after `:=` on the same line, the block's tokens and its end line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Have {
    pub line: usize,
    pub name: String,
    pub ty: String,
    pub value: String,
    pub tokens: usize,
    pub end: usize,
}

/// `have`s with a stated type, from the text of a declaration given as lines.
pub fn haves(lines: &[&str], first: usize) -> Vec<Have> {
    let mut v = Vec::new();
    for i in 0..lines.len() {
        let t = lines[i].trim_start();
        let t = t.strip_prefix("· ").unwrap_or(t);
        if !t.starts_with("have ") {
            continue;
        }
        // The statement may run over a few lines, up to the `:=` at depth zero.
        let end = block_end(lines, i);
        let text: String = lines[i..end].join("\n");
        let rest_all = &text[text.find("have ").unwrap_or(0) + 5..];
        let mut depth = 0i32;
        let mut colon = None;
        let mut assign = None;
        let chars: Vec<char> = rest_all.chars().collect();
        let mut k = 0;
        while k < chars.len() {
            match chars[k] {
                '(' | '[' | '{' | '⟨' => depth += 1,
                ')' | ']' | '}' | '⟩' => depth -= 1,
                ':' if depth == 0 && k + 1 < chars.len() && chars[k + 1] == '=' => {
                    assign = Some(k);
                    break;
                }
                ':' if depth == 0 && colon.is_none() => colon = Some(k),
                _ => {}
            }
            k += 1;
        }
        let (Some(c), Some(a)) = (colon, assign) else { continue };
        let name: String = chars[..c].iter().collect::<String>().trim().to_string();
        if name.contains(' ') && !name.starts_with('⟨') {
            continue;
        }
        let ty: String = chars[c + 1..a].iter().collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ");
        let after: String = chars[a + 2..].iter().collect();
        let value: String =
            after.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_string();
        v.push(Have {
            line: first + i,
            name: if name.is_empty() { "this".into() } else { name },
            ty,
            value,
            tokens: tokens(&text),
            end: first + end - 1,
        });
    }
    v
}

/// Declarations of a file: (first line, last line), from lines starting with a declaration
/// keyword (after modifiers) at column zero to the line before the next one.
pub fn declarations(lines: &[&str]) -> Vec<(usize, usize)> {
    let kws = ["theorem ", "lemma ", "def ", "private theorem ", "private lemma ", "private def ", "noncomputable def "];
    let starts: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| {
            let l = l.trim_start_matches(|c| c == '@').trim_start();
            let l = if l.starts_with('[') { l.split_once("] ").map(|x| x.1).unwrap_or(l) } else { l };
            kws.iter().any(|k| l.starts_with(k)) && !lines.is_empty()
        })
        .map(|(i, _)| i)
        .filter(|&i| indent(lines[i]) == 0)
        .collect();
    starts.iter().enumerate().map(|(k, &s)| (s + 1, starts.get(k + 1).map(|&n| n).unwrap_or(lines.len()))).collect()
}

/// A trivial statement: a sign or non-vanishing fact (`0 < e`, `0 ≤ e`, `e ≠ 0`, `1 ≤ e`), a
/// comparison of a variable (or a cast of one) with a numeral, or at most five tokens.
pub fn trivial_statement(s: &str) -> bool {
    let t = s.trim();
    let sign = ["0 < ", "0 ≤ ", "1 ≤ ", "0 ≠ "];
    if sign.iter().any(|p| t.starts_with(p)) && !t.contains('∧') && !t.contains('∀') {
        return true;
    }
    if (t.ends_with("≠ 0") || t.ends_with("> 0") || t.ends_with("≥ 0")) && !t.contains('∧') && !t.contains('∀') {
        return true;
    }
    let s = s.replace('·', "x");
    let toks: Vec<String> = lexer::tokenize(&s).into_iter().map(|t| t.text).collect();
    if toks.len() <= 5 {
        return true;
    }
    let cmp = ["<", "≤", "≠", ">", "≥"];
    let numeral = |t: &str| t.chars().all(|c| c.is_ascii_digit());
    let atomish = |part: &[String]| part.iter().filter(|t| !matches!(t.as_str(), "↑" | "(" | ")" | "x" | ":")).count() <= 2;
    if let Some(p) = toks.iter().position(|t| cmp.contains(&t.as_str())) {
        let (l, r) = (&toks[..p], &toks[p + 1..]);
        return (l.iter().any(|t| numeral(t)) && atomish(r) && l.len() <= 2)
            || (r.iter().any(|t| numeral(t)) && atomish(l) && r.len() <= 2);
    }
    false
}

/// A trivial proof: one tactic (`by positivity`), or one term without nested applications.
pub fn trivial_proof(value: &str, block_tokens_after_type: usize) -> bool {
    let v = value.trim();
    if let Some(t) = v.strip_prefix("by ") {
        return !t.contains(';') && !t.contains("<;>") && tokens(t) <= 3 && block_tokens_after_type <= 6;
    }
    block_tokens_after_type <= 6
}

pub struct SeamsOptions {
    pub root: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub imports: Vec<String>,
    pub theorems: Vec<String>,
    pub out: Option<PathBuf>,
    pub top: usize,
    pub min_size: usize,
    pub max_diff: usize,
}

struct Src {
    cache: BTreeMap<String, String>,
    project: PathBuf,
}

impl Src {
    fn get(&mut self, module: &str) -> &str {
        let p = self.project.join(module_path(module));
        self.cache.entry(module.to_string()).or_insert_with(|| std::fs::read_to_string(p).unwrap_or_default())
    }
}

pub fn seams(o: SeamsOptions) -> Result<()> {
    let root = repo_root(o.root.clone());
    let cfg = Config::load(&o.config.clone().unwrap_or_else(|| root.join("hillclimb.toml")))?;
    let project = root.join(&cfg.project.dir);
    let roots = if o.theorems.is_empty() { cfg.project.theorems.clone() } else { o.theorems.clone() };
    if roots.is_empty() {
        bail!("no root theorems: set `project.theorems` in hillclimb.toml or pass --theorem");
    }
    let imports = default_imports(&roots, &o.imports);
    let tail = |mut a: Vec<String>| {
        a.extend(imports.iter().cloned());
        a.push("--".into());
        a.extend(roots.iter().cloned());
        a
    };
    println!("Running Seams.lean and Flat.lean in {} (imports: {})", project.display(), imports.join(" "));
    let (seams, s1) = run_lean(
        "Seams.lean",
        SEAMS_LEAN,
        &project,
        &tail(vec!["--min-size".into(), o.min_size.to_string(), "--max-diff".into(), o.max_diff.to_string()]),
    )?;
    let (flat, s2) = run_lean("Flat.lean", FLAT_LEAN, &project, &tail(vec![]))?;
    let mut src = Src { cache: BTreeMap::new(), project: project.clone() };
    let mut out_lines: Vec<String> = seams.lines().filter(|l| !l.starts_with("# ")).map(str::to_string).collect();
    let rec = |tag: &str| -> Vec<Vec<String>> {
        seams.lines().filter(|l| l.starts_with(tag)).map(|l| l.split('\t').map(str::to_string).collect()).collect()
    };

    // (a) Pieces a single caller reads.
    let parts = rec("part\t");
    println!("\n(a) Single-caller lemmas whose caller reads only part of the conclusion ({}):", parts.len());
    let mut ps: Vec<(usize, &Vec<String>)> = parts
        .iter()
        .map(|p| {
            let (a, b): (usize, usize) = (p[3].parse().unwrap_or(0), p[4].parse().unwrap_or(0));
            let text = src.get(&p[1]).lines().skip(a.saturating_sub(1)).take(b + 1 - a.max(1)).collect::<Vec<_>>().join("\n");
            (tokens(&text), p)
        })
        .collect();
    ps.sort_by(|x, y| y.0.cmp(&x.0));
    for (t, p) in ps.iter().take(o.top) {
        println!("  {:>6}  {}:{}  {}  (caller {}; reads {}; unread proof nodes {})", t, module_path(&p[1]), p[3], p[2], p[5], p[6], p[7]);
        println!("          unread: {}", p.get(8).map(String::as_str).unwrap_or(""));
    }

    // (b) Conclusion restated as a type at the one call (text).
    let singles = parse_singles(&flat);
    let mut restated: Vec<(usize, String)> = Vec::new();
    for s in &singles {
        let short = s.name.rsplit('.').next().unwrap_or(&s.name).to_string();
        let text = src.get(&s.caller_module).to_string();
        let lines: Vec<&str> = text.lines().collect();
        for h in haves(&lines, 1) {
            // The value starts with the lemma: `L args`, `(L args)`, `x.L args` or `(h n).L args`.
            let v = h.value.trim_start_matches('(');
            let first: Vec<&str> = v.split(' ').take(2).collect();
            let names_it = first.iter().any(|w| {
                let w = w.trim_start_matches('(');
                let last = w.rsplit(['.', ')']).next().unwrap_or(w);
                last == short
            });
            if !names_it {
                continue;
            }
            let ty_tokens = tokens(&h.ty);
            if ty_tokens < 12 {
                continue;
            }
            // Rewritten or taken apart right after?
            let after: Vec<&str> = lines.iter().skip(h.end).take(3).copied().collect();
            let undone = after.iter().any(|l| {
                (l.contains(&format!("at {}", h.name)) && (l.contains("rw ") || l.contains("simp only") || l.contains("push_cast")))
                    || l.contains(&format!("using {}", h.name))
                    || l.contains(&format!(".1 ({}", h.name))
                    || l.contains(&format!(".2 ({}", h.name))
            });
            restated.push((
                ty_tokens,
                format!(
                    "seam-b\t{}\t{}\t{}\t{}\t{}\t{}\t{ty_tokens}",
                    s.caller_module,
                    s.caller,
                    h.line,
                    s.name,
                    h.name,
                    if undone { "rewritten-after" } else { "-" }
                ),
            ));
        }
    }
    restated.sort_by(|a, b| b.0.cmp(&a.0));
    println!("\n(b) A single-caller lemma's conclusion restated as a `have` type at its call ({}):", restated.len());
    for (_, l) in restated.iter().take(o.top) {
        let f: Vec<&str> = l.split('\t').collect();
        println!("  {:>6}  {}:{}  `have {} : …` := {}  (in {}{})", f[7], module_path(f[1]), f[3], f[5], f[4], f[2], if f[6] == "-" { "" } else { "; rewritten right after" });
    }
    out_lines.extend(restated.iter().map(|x| x.1.clone()));

    // (c) Introductions undone across a boundary.
    let undo = rec("undo\t");
    println!("\n(c) An introduction undone across a boundary ({}):", undo.len());
    for u in undo.iter().take(o.top) {
        println!("  {}:{}  {}  (caller {}; {} through {})", module_path(&u[1]), u[3], u[2], u[5], u[6], u.get(7).map(String::as_str).unwrap_or(""));
    }

    // (d) Threads: a hypothesis carried through several declarations to its one use.
    let threads = rec("thread\t");
    let used: Vec<&Vec<String>> = threads.iter().filter(|t| t.get(7).map(String::as_str) == Some("used")).collect();
    let dead: Vec<&Vec<String>> = threads.iter().filter(|t| t.get(7).map(String::as_str) == Some("dead")).collect();
    println!(
        "\n(d) Hypotheses carried through several declarations to one use ({}), and passed on but never used ({}):",
        used.len(),
        dead.len()
    );
    for t in used.iter().take(o.top) {
        let names: Vec<&str> = t.get(9).map(|x| x.split(", ").collect()).unwrap_or_default();
        let shown: Vec<&str> = names.iter().take(8).copied().collect();
        println!(
            "  `{}` of {} ({}:{}): carried by {}: {}{}",
            t[6],
            t[2],
            module_path(&t[1]),
            t[3],
            t[8],
            shown.join(", "),
            if names.len() > 8 { ", …" } else { "" }
        );
    }
    for t in dead.iter().take(o.top) {
        println!("  `{}` of {} ({}:{}): passed to {}, used nowhere", t[6], t[2], module_path(&t[1]), t[3], t.get(9).map(String::as_str).unwrap_or(""));
    }

    // (e) Same skeletons.
    let skel = rec("skel\t");
    let mut by_group: BTreeMap<String, Vec<&Vec<String>>> = BTreeMap::new();
    let mut order = Vec::new();
    for s in &skel {
        if !by_group.contains_key(&s[1]) {
            order.push(s[1].clone());
        }
        by_group.entry(s[1].clone()).or_default().push(s);
    }
    println!("\n(e) Subproofs with the same case-split skeleton and close statements ({} pairs):", order.len());
    for g in order.iter().take(o.top) {
        let m = &by_group[g];
        println!("  {}  [{}]", g, m[0].get(8).map(String::as_str).unwrap_or(""));
        for s in m {
            let src_text = src.get(&s[3]).to_string();
            let (a, b): (usize, usize) = (s[5].parse().unwrap_or(0), s[6].parse().unwrap_or(0));
            let at = if s[7] == "-" { vec![] } else { have_lines(&src_text, a, b, &s[7]) };
            let place = if at.is_empty() { format!("{}:{}", module_path(&s[3]), a) } else { format!("{}:{}", module_path(&s[3]), at[0]) };
            println!("        {}  {} {}  {}", place, s[4], if s[7] == "-" { "(whole proof)".to_string() } else { format!("`{}`", s[7]) }, s[9]);
        }
    }

    // (f) The same `have` stated twice in one declaration (text).
    let mut dups: Vec<(usize, String)> = Vec::new();
    let mut modules: Vec<String> = Vec::new();
    for e in walk_lean(&project) {
        let m = e.trim_end_matches(".lean").replace('/', ".");
        modules.push(m);
    }
    modules.sort();
    modules.dedup();
    for m in &modules {
        let text = src.get(m).to_string();
        let lines: Vec<&str> = text.lines().collect();
        for (a, b) in declarations(&lines) {
            let hs = haves(&lines[a - 1..b.min(lines.len())], a);
            for i in 0..hs.len() {
                for j in i + 1..hs.len() {
                    let (x, y) = (&hs[i], &hs[j]);
                    if x.ty != y.ty || y.line <= x.end {
                        continue;
                    }
                    if trivial_statement(&x.ty) || trivial_proof(&x.value, x.tokens.saturating_sub(tokens(&x.ty) + 4)) {
                        continue;
                    }
                    let cost = x.tokens.min(y.tokens);
                    dups.push((
                        cost,
                        format!("seam-f\t{m}\t{}\t{}\t{}\t{}\t{}\t{cost}", x.line, x.name, y.line, y.name, x.ty),
                    ));
                }
            }
        }
    }
    dups.sort_by(|a, b| b.0.cmp(&a.0));
    println!("\n(f) The same `have` statement proved twice in one declaration ({}; tokens of the smaller block):", dups.len());
    for (c, l) in dups.iter().take(o.top) {
        let f: Vec<&str> = l.split('\t').collect();
        let ty: String = f[6].chars().take(90).collect();
        println!("  {c:>6}  {}:{} `{}` and :{} `{}`  : {ty}", module_path(f[1]), f[2], f[3], f[4], f[5]);
    }
    out_lines.extend(dups.iter().map(|x| x.1.clone()));

    // (g) Dispatch.
    let dispatch = rec("dispatch\t");
    println!("\n(g) Lemmas that only choose between other lemmas ({}):", dispatch.len());
    for d in dispatch.iter().take(o.top) {
        println!("  {}:{}  {}  → {}  (caller {})", module_path(&d[1]), d[3], d[2], d[6], d[5]);
    }

    // (h) Repeated facts across declarations, without trivia, ranked by source tokens.
    let mut facts: BTreeMap<String, Vec<Vec<String>>> = BTreeMap::new();
    for l in flat.lines().filter(|l| l.starts_with("fact\t")) {
        let f: Vec<String> = l.split('\t').map(str::to_string).collect();
        facts.entry(f[1].clone()).or_default().push(f);
    }
    let mut ranked: Vec<(usize, String, Vec<String>)> = Vec::new();
    for (g, members) in &facts {
        let stmt = members[0].get(8).cloned().unwrap_or_default();
        if trivial_statement(&stmt) {
            continue;
        }
        let mut sizes = Vec::new();
        let mut shown = Vec::new();
        for f in members {
            let text = src.get(&f[3]).to_string();
            let lines: Vec<&str> = text.lines().collect();
            let (a, b): (usize, usize) = (f[5].parse().unwrap_or(0), f[6].parse().unwrap_or(0));
            let at = have_lines(&text, a, b, &f[7]);
            let Some(&ln) = at.first() else { continue };
            let end = block_end(&lines, ln - 1);
            let block = lines[ln - 1..end].join("\n");
            let hs = haves(&lines[ln - 1..end], ln);
            let trivial = hs.first().is_some_and(|h| trivial_proof(&h.value, h.tokens.saturating_sub(tokens(&h.ty) + 4)));
            if trivial {
                continue;
            }
            sizes.push(tokens(&block));
            shown.push(format!("{}:{}-{}  {} (`{}`, {} tokens)", module_path(&f[3]), ln, end, f[4], f[7], tokens(&block)));
        }
        if sizes.len() >= 2 {
            let saving = sizes.iter().sum::<usize>() - sizes.iter().max().unwrap();
            ranked.push((saving, format!("{g}. {stmt}"), shown));
        }
    }
    ranked.sort_by(|a, b| b.0.cmp(&a.0));
    println!(
        "\n(h) Facts proved in several declarations, trivial ones dropped, by source tokens of the copies ({}):",
        ranked.len()
    );
    for (s, head, shown) in ranked.iter().take(o.top) {
        println!("  {s:>6}  {head}");
        for x in shown {
            println!("          {x}");
        }
    }
    for (s, head, shown) in &ranked {
        out_lines.push(format!("seam-h\t{s}\t{head}\t{}", shown.join(" | ")));
    }

    for l in seams.lines().filter(|l| l.starts_with("# ")) {
        println!("{}", &l[2..]);
    }
    println!("\n({:.1} s)", s1 + s2);
    if let Some(p) = &o.out {
        std::fs::write(p, out_lines.join("\n") + "\n")?;
        println!("Full list written to {}", p.display());
    }
    Ok(())
}

fn walk_lean(project: &Path) -> Vec<String> {
    let mut v = Vec::new();
    let mut stack = vec![project.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
            } else if name.ends_with(".lean") && name != "lakefile.lean" {
                if let Ok(rel) = p.strip_prefix(project) {
                    v.push(rel.to_string_lossy().to_string());
                }
            }
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "theorem t (n : ℕ) : True := by
  have h3 : ∑ i in s, f i = ∑ i in s, g i + c := by
    rw [foo]
    simp
  · have h3 : ∑ i in s, f i = ∑ i in s, g i + c := by
      rw [foo]
      simp
    trivial
  have hp : 0 < n := by positivity
  trivial
theorem u : True := trivial
";

    #[test]
    fn finds_haves_and_their_blocks() {
        let lines: Vec<&str> = SRC.lines().collect();
        let hs = haves(&lines, 1);
        assert_eq!(hs.len(), 3);
        assert_eq!(hs[0].name, "h3");
        assert_eq!(hs[0].line, 2);
        assert_eq!(hs[0].end, 4);
        assert_eq!(hs[0].ty, hs[1].ty);
        assert_eq!(hs[1].line, 5);
        assert_eq!(declarations(&lines), [(1, 10), (11, 11)]);
    }

    #[test]
    fn trivia() {
        assert!(trivial_statement("0 < · 0 ℕ"));
        assert!(trivial_statement("1 ≤ ↑x"));
        assert!(trivial_statement("x ≠ 0"));
        assert!(trivial_statement("0 ≤ Nc.Tow.kbar (· 7 ·) (· 8 ℕ)"));
        assert!(!trivial_statement("∑ i in s, f i = ∑ i in s, g i + c"));
        assert!(trivial_proof("by positivity", 2));
        assert!(!trivial_proof("by", 12));
    }
}
