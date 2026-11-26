//! `hc analyze flat`: lemma boundaries a flatter proof could remove (`tools/lean/Flat.lean`).

use crate::analyze::{default_imports, module_path, print_summary, repo_root};
use crate::analyze_more::run_lean;
use anyhow::{bail, Result};
use hc_core::config::Config;
use hc_core::lexer;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const FLAT_LEAN: &str = include_str!("../../../tools/lean/Flat.lean");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Single {
    pub module: String,
    pub name: String,
    pub first: usize,
    pub last: usize,
    pub caller: String,
    pub caller_module: String,
    pub sites: usize,
    pub escapes: bool,
}

pub fn parse_singles(out: &str) -> Vec<Single> {
    out.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            match f.as_slice() {
                ["single", m, n, a, b, c, cm, k, e, ..] => Some(Single {
                    module: m.to_string(),
                    name: n.to_string(),
                    first: a.parse().ok()?,
                    last: b.parse().ok()?,
                    caller: c.to_string(),
                    caller_module: cm.to_string(),
                    sites: k.parse().ok()?,
                    escapes: *e == "1",
                }),
                _ => None,
            }
        })
        .collect()
}

/// Token counts of a declaration's text: (statement, conclusion, proof).
/// The statement runs from the declaration keyword to the first `:=`, `where` or `|` at
/// bracket depth zero; the conclusion is what follows its last `:` at depth zero.
pub fn split_tokens(text: &str) -> (usize, usize, usize) {
    let toks = lexer::tokenize(text);
    let kws = ["theorem", "lemma", "def", "abbrev", "instance"];
    let Some(kw) = toks.iter().position(|t| kws.contains(&t.text.as_str())) else {
        return (0, 0, toks.len());
    };
    let mut depth: i64 = 0;
    let mut split = toks.len();
    let mut colon = None;
    for (i, t) in toks.iter().enumerate().skip(kw + 1) {
        match t.text.as_str() {
            "(" | "[" | "{" | "⟨" | "⦃" => depth += 1,
            ")" | "]" | "}" | "⟩" | "⦄" => depth -= 1,
            ":=" | "where" | "|" if depth == 0 => {
                split = i;
                break;
            }
            ":" if depth == 0 => colon = Some(i),
            _ => {}
        }
    }
    let stmt = split - kw;
    let concl = colon.map(|c| split - c - 1).unwrap_or(0);
    (stmt, concl, toks.len() - split)
}

/// Upper bound on what inlining saves: the statement, minus a `have h : <conclusion> :=` at
/// each call site (conclusion tokens plus 3), minus a copy of the proof for each further site.
pub fn inline_saving(stmt: usize, concl: usize, proof: usize, sites: usize) -> usize {
    let k = sites.max(1);
    stmt.saturating_sub(k * (concl + 3)).saturating_sub((k - 1) * proof)
}

fn lines(src: &str, a: usize, b: usize) -> String {
    src.lines().skip(a.saturating_sub(1)).take(b + 1 - a.max(1)).collect::<Vec<_>>().join("\n")
}

pub struct FlatOptions {
    pub root: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub imports: Vec<String>,
    pub theorems: Vec<String>,
    pub out: Option<PathBuf>,
    pub top: usize,
    pub min_size: usize,
    pub min_calls: usize,
}

pub struct Measured {
    pub s: Single,
    pub total: usize,
    pub stmt: usize,
    pub concl: usize,
    pub proof: usize,
    pub saving: usize,
}

pub fn measure(project: &Path, singles: Vec<Single>) -> Vec<Measured> {
    let mut cache: BTreeMap<String, String> = BTreeMap::new();
    singles
        .into_iter()
        .map(|s| {
            let src = cache
                .entry(s.module.clone())
                .or_insert_with(|| std::fs::read_to_string(project.join(module_path(&s.module))).unwrap_or_default());
            let text = lines(src, s.first, s.last);
            let (stmt, concl, proof) = split_tokens(&text);
            let saving = inline_saving(stmt, concl, proof, s.sites);
            Measured { total: stmt + proof, stmt, concl, proof, saving, s }
        })
        .collect()
}

/// Trees of single-caller lemmas: each lemma hangs under its caller; a tree's root is a
/// caller that is not itself a single-caller lemma. Returns roots with (lemmas, tokens, saving).
pub fn trees(m: &[Measured]) -> Vec<(String, usize, usize, usize)> {
    let idx: BTreeMap<&str, usize> = m.iter().enumerate().map(|(i, x)| (x.s.name.as_str(), i)).collect();
    let mut agg: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
    for x in m {
        // Walk up to the root, guarding against cycles.
        let mut cur = x.s.caller.as_str();
        let mut steps = 0;
        while let Some(&j) = idx.get(cur) {
            cur = m[j].s.caller.as_str();
            steps += 1;
            if steps > m.len() {
                break;
            }
        }
        let e = agg.entry(cur.to_string()).or_default();
        e.0 += 1;
        e.1 += x.total;
        e.2 += x.saving;
    }
    let mut v: Vec<_> = agg.into_iter().map(|(r, (n, t, s))| (r, n, t, s)).collect();
    v.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
    v
}

fn print_tree(m: &[Measured], root: &str, depth: usize, max_depth: usize) {
    let mut kids: Vec<&Measured> = m.iter().filter(|x| x.s.caller == root).collect();
    kids.sort_by(|a, b| b.total.cmp(&a.total));
    for k in kids {
        let dup = if k.s.sites > 1 { format!(", {} sites", k.s.sites) } else { String::new() };
        println!(
            "{}{} ({} tokens, saves at most {}{dup})  {}:{}",
            "  ".repeat(depth + 2),
            k.s.name,
            k.total,
            k.saving,
            module_path(&k.s.module),
            k.s.first
        );
        if depth + 1 < max_depth {
            print_tree(m, &k.s.name, depth + 1, max_depth);
        }
    }
}

pub fn flat(o: FlatOptions) -> Result<()> {
    let root = repo_root(o.root.clone());
    let cfg = Config::load(&o.config.clone().unwrap_or_else(|| root.join("hillclimb.toml")))?;
    let project = root.join(&cfg.project.dir);
    let roots = if o.theorems.is_empty() { cfg.project.theorems.clone() } else { o.theorems.clone() };
    if roots.is_empty() {
        bail!("no root theorems: set `project.theorems` in hillclimb.toml or pass --theorem");
    }
    let imports = default_imports(&roots, &o.imports);
    let mut args = vec!["--min-size".to_string(), o.min_size.to_string(), "--min-calls".into(), o.min_calls.to_string()];
    args.extend(imports.iter().cloned());
    args.push("--".into());
    args.extend(roots.iter().cloned());
    println!("Running Flat.lean in {} (imports: {})", project.display(), imports.join(" "));
    let (stdout, secs) = run_lean("Flat.lean", FLAT_LEAN, &project, &args)?;
    let m = measure(&project, parse_singles(&stdout));
    println!();
    print_summary(&stdout);
    let total: usize = m.iter().map(|x| x.total).sum();
    let saving: usize = m.iter().map(|x| x.saving).sum();
    let several = m.iter().filter(|x| x.s.sites > 1).count();
    println!(
        "\nSingle-caller theorems: {} with {total} tokens; inlining each saves at most {saving} tokens in all \
         ({several} are used at several sites of their caller).",
        m.len()
    );
    println!(
        "Estimate per lemma, for inlining as is (an upper bound for that, not for a change that also \
         simplifies across the boundary): statement tokens (keyword to `:=`) minus, per call site, a \
         `have h : <conclusion> :=` (conclusion tokens + 3), minus a copy of the proof for each further site."
    );
    let mut folders: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
    for x in &m {
        let f = x.s.module.split('.').next().unwrap_or("").to_string();
        let e = folders.entry(f).or_default();
        e.0 += 1;
        e.1 += x.total;
        e.2 += x.saving;
    }
    println!("\nBy top-level folder: lemmas, their tokens, saving at most");
    for (f, (n, t, s)) in &folders {
        println!("{n:>6} {t:>9} {s:>8}  {f}");
    }
    let tr = trees(&m);
    println!("\nLargest trees of single-caller lemmas (root: lemmas, tokens, saving at most):");
    for (r, n, t, s) in tr.iter().take(o.top.min(15)) {
        println!("  {r}: {n} lemmas, {t} tokens, {s}");
        print_tree(&m, r, 0, 3);
    }
    let mut by_saving: Vec<&Measured> = m.iter().collect();
    by_saving.sort_by(|a, b| b.saving.cmp(&a.saving));
    println!("\nLargest single estimates:");
    for x in by_saving.iter().take(o.top) {
        println!(
            "{:>6}  {}:{}-{}  {}  (statement {}, proof {}, used by {}, {} site{})",
            x.saving,
            module_path(&x.s.module),
            x.s.first,
            x.s.last,
            x.s.name,
            x.stmt,
            x.proof,
            x.s.caller,
            x.s.sites,
            if x.s.sites == 1 { "" } else { "s" }
        );
    }
    let section = |tag: &str, title: &str| {
        let ls: Vec<&str> = stdout.lines().filter(|l| l.starts_with(tag)).collect();
        println!("\n{title} ({}):", ls.len());
        for l in ls.iter().take(o.top) {
            println!("    {}", l.split('\t').skip(1).collect::<Vec<_>>().join("  "));
        }
    };
    section("constarg\t", "Arguments that are the same at every call");
    print_facts(&project, &stdout, o.top);
    section("factpair\t", "Caller and callee proving the same fact");
    println!("\n({secs:.1} s)");
    if let Some(p) = &o.out {
        let mut k = 0;
        let mut s = String::new();
        for line in stdout.lines() {
            s.push_str(line);
            if line.starts_with("single\t") {
                let x = &m[k];
                s.push_str(&format!("\t{}\t{}\t{}\t{}\t{}", x.total, x.stmt, x.concl, x.proof, x.saving));
                k += 1;
            }
            s.push('\n');
        }
        std::fs::write(p, s)?;
        println!("Full list written to {} (single records with tokens, statement, conclusion, proof, saving added)", p.display());
    }
    Ok(())
}

/// Lines among `a..=b` of `src` that introduce the name `name` with `have`, `let`,
/// `obtain`, `suffices` or `show … from`. Unnamed facts (`this`) match `have :` too.
pub fn have_lines(src: &str, a: usize, b: usize, name: &str) -> Vec<usize> {
    let mut v = Vec::new();
    for (i, line) in src.lines().enumerate().skip(a.saturating_sub(1)).take(b + 1 - a.max(1)) {
        let toks: Vec<String> = lexer::tokenize(line).into_iter().map(|t| t.text).collect();
        for w in toks.windows(2) {
            let intro = matches!(w[0].as_str(), "have" | "let" | "suffices" | "haveI" | "letI");
            if intro && (w[1] == name || (name == "this" && w[1] == ":")) {
                v.push(i + 1);
                break;
            }
        }
    }
    v
}

fn print_facts(project: &Path, stdout: &str, top: usize) {
    let mut cache: BTreeMap<String, String> = BTreeMap::new();
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for l in stdout.lines() {
        let f: Vec<&str> = l.split('\t').collect();
        if let ["fact", g, nodes, module, decl, a, b, name, stmt, ..] = f.as_slice() {
            let (a, b) = (a.parse().unwrap_or(0), b.parse().unwrap_or(0));
            let src = cache
                .entry(module.to_string())
                .or_insert_with(|| std::fs::read_to_string(project.join(module_path(module))).unwrap_or_default());
            let at = have_lines(src, a, b, name);
            let place = if at.is_empty() {
                format!("{}:{a}-{b}", module_path(module))
            } else {
                format!("{}:{}", module_path(module), at.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(","))
            };
            let member = format!("{nodes:>8}  {place}  {decl} (`{name}`)");
            match groups.last_mut() {
                Some((gg, ms)) if gg == g => ms.push(member),
                _ => groups.push((g.to_string(), vec![format!("{stmt}"), member])),
            }
        }
    }
    println!("\nFacts proved in several declarations ({} groups; subproof nodes, where, `have` name):", groups.len());
    for (g, ms) in groups.iter().take(top) {
        println!("  {g}. {}", ms[0]);
        for m in &ms[1..] {
            println!("    {m}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_statement_and_proof() {
        let t = "/-- doc -/\n@[simp] theorem foo {n : ℕ} (h : n = 1 := by rfl) : n + 0 = 1 := by\n  simp [h]";
        let (stmt, concl, proof) = split_tokens(t);
        // theorem foo { n : ℕ } ( h : n = 1 := by rfl ) : n + 0 = 1
        assert_eq!(stmt, 23);
        assert_eq!(concl, 5);
        // := by simp [ h ]
        assert_eq!(proof, 6);
    }

    #[test]
    fn finds_have_lines() {
        let src = "theorem t : True := by\n  have h1 : 1 = 1 := rfl\n  have : 2 = 2 := rfl\n  trivial";
        assert_eq!(have_lines(src, 1, 4, "h1"), [2]);
        assert_eq!(have_lines(src, 1, 4, "this"), [3]);
        assert!(have_lines(src, 1, 4, "h9").is_empty());
    }

    #[test]
    fn inline_estimate() {
        assert_eq!(inline_saving(40, 10, 30, 1), 27);
        assert_eq!(inline_saving(40, 10, 30, 2), 0);
        assert_eq!(inline_saving(5, 10, 30, 1), 0);
    }

    #[test]
    fn parses_and_builds_trees() {
        let out = "single\tA\tA.a\t1\t2\tA.b\tA\t1\t0\n\
                   single\tA\tA.b\t3\t4\tA.top\tA\t2\t0\n\
                   single\tA\tA.c\t5\t6\tA.top\tA\t1\t1\n";
        let s = parse_singles(out);
        assert_eq!(s.len(), 3);
        assert!(s[2].escapes);
        let m: Vec<Measured> = s
            .into_iter()
            .map(|s| Measured { total: 10, stmt: 4, concl: 1, proof: 6, saving: 1, s })
            .collect();
        assert_eq!(trees(&m), [("A.top".to_string(), 3, 30, 3)]);
    }
}
