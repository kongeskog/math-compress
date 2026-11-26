//! `hc analyze twins` and `hc analyze tails`: the Lean programs `Twins.lean`
//! and `Tails.lean` in `tools/lean`, run like the others in `analyze.rs`.

use crate::analyze::{default_imports, module_path, print_summary, repo_root};
use anyhow::{bail, Context, Result};
use hc_core::config::Config;
use hc_core::lexer;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

pub const TWINS_LEAN: &str = include_str!("../../../tools/lean/Twins.lean");
pub const TAILS_LEAN: &str = include_str!("../../../tools/lean/Tails.lean");

/// Tactics that prove one kind of goal. They are candidates even where an
/// arena does not allow search tactics such as `grind`.
pub const SPECIFIC_TACTICS: &[&str] = &["omega", "simp", "linarith", "positivity", "norm_num", "ring"];

/// Compile `source` to a native program with the project's toolchain, once per source and
/// toolchain, cached under `$XDG_CACHE_HOME/hc` (or `~/.cache/hc`). `None` when that fails.
pub fn native_program(name: &str, source: &str, project: &Path) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    let prefix = Command::new("lake").args(["env", "lean", "--print-prefix"]).current_dir(project).output().ok()?;
    if !prefix.status.success() {
        return None;
    }
    let prefix = PathBuf::from(String::from_utf8_lossy(&prefix.stdout).trim());
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (source, prefix.display().to_string()).hash(&mut h);
    let stem = name.trim_end_matches(".lean");
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?
        .join("hc")
        .join(format!("{stem}-{:016x}", h.finish()));
    let exe = cache.join(stem.to_lowercase());
    if exe.is_file() {
        return Some(exe);
    }
    // Build in a directory of this process, then move it into place, so that two runs at the
    // same time never use a half-written program.
    let work = cache.with_extension(format!("tmp{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).ok()?;
    std::fs::write(work.join(name), source).ok()?;
    let c = work.join(format!("{stem}.c"));
    let built = work.join(stem.to_lowercase());
    let ok = Command::new(prefix.join("bin/lean"))
        .arg(format!("--root={}", work.display()))
        .arg("-c")
        .arg(&c)
        .arg(work.join(name))
        .stdout(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
        && Command::new(prefix.join("bin/leanc"))
            .args(["-O2", "-rdynamic", "-o"])
            .arg(&built)
            .arg(&c)
            .stdout(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
    if ok && built.is_file() && (std::fs::rename(&work, &cache).is_ok() || exe.is_file()) {
        let _ = std::fs::remove_dir_all(&work);
        return exe.is_file().then_some(exe);
    }
    let _ = std::fs::remove_dir_all(&work);
    None
}

/// Run the Lean program `source` (named `name`) inside `project` with `args`. By default it is
/// compiled natively first (see `native_program`), which is many times faster; when that fails,
/// or when `HC_LEAN_NATIVE=0` is set, it runs under `lake env lean --run`.
pub fn run_lean(name: &str, source: &str, project: &Path, args: &[String]) -> Result<(String, f64)> {
    let native = std::env::var("HC_LEAN_NATIVE").map(|v| v != "0").unwrap_or(true);
    if native {
        match native_program(name, source, project) {
            Some(exe) => {
                let start = Instant::now();
                let output = Command::new("lake")
                    .arg("env")
                    .arg(&exe)
                    .args(args)
                    .current_dir(project)
                    .stderr(Stdio::inherit())
                    .output()
                    .with_context(|| format!("running {} in {}", exe.display(), project.display()))?;
                if !output.status.success() {
                    bail!("{name} failed ({}); is the project built (`lake build`)?", output.status);
                }
                return Ok((String::from_utf8_lossy(&output.stdout).into_owned(), start.elapsed().as_secs_f64()));
            }
            None => eprintln!("Could not compile {name} natively; running it under `lean --run` (slower)."),
        }
    }
    let dir = std::env::temp_dir().join(format!("hc-analyze-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let file = dir.join(name);
    std::fs::write(&file, source)?;
    let start = Instant::now();
    let mut a = vec!["env".to_string(), "lean".to_string(), "--run".to_string(), file.display().to_string()];
    a.extend(args.iter().cloned());
    let output = Command::new("lake")
        .args(&a)
        .current_dir(project)
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| format!("running `lake env lean --run` in {}", project.display()));
    let _ = std::fs::remove_dir_all(&dir);
    let output = output?;
    if !output.status.success() {
        bail!("{name} failed ({}); is the project built (`lake build`)?", output.status);
    }
    Ok((String::from_utf8_lossy(&output.stdout).into_owned(), start.elapsed().as_secs_f64()))
}

pub fn tokens_of_lines(cache: &mut BTreeMap<String, Vec<lexer::Token>>, project: &Path, module: &str, a: usize, b: usize) -> usize {
    let toks = cache.entry(module.to_string()).or_insert_with(|| {
        std::fs::read_to_string(project.join(module_path(module))).map(|s| lexer::tokenize(&s)).unwrap_or_default()
    });
    toks.iter().filter(|t| t.line >= a && t.line <= b).count()
}

// ---------------------------------------------------------------- twins

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Twin {
    pub group: usize,
    pub how: String,
    pub module: String,
    pub name: String,
    pub kind: String,
    pub first: usize,
    pub last: usize,
    pub difference: String,
}

/// Parse `twin` records of `Twins.lean`.
pub fn parse_twins(out: &str) -> Result<Vec<Twin>> {
    let mut v = Vec::new();
    for (i, line) in out.lines().enumerate() {
        let f: Vec<&str> = line.split('\t').collect();
        if let ["twin", group, how, module, name, kind, first, last, difference, ..] = f.as_slice() {
            let num = |s: &str| s.parse::<usize>().with_context(|| format!("line {}: bad number `{s}`", i + 1));
            v.push(Twin {
                group: num(group)?,
                how: how.to_string(),
                module: module.to_string(),
                name: name.to_string(),
                kind: kind.to_string(),
                first: num(first)?,
                last: num(last)?,
                difference: difference.to_string(),
            });
        }
    }
    Ok(v)
}

/// Estimated saving of a group: all members but the largest (an upper bound).
pub fn group_saving(tokens: &[usize]) -> usize {
    tokens.iter().sum::<usize>() - tokens.iter().copied().max().unwrap_or(0)
}

pub struct TwinsOptions {
    pub root: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub imports: Vec<String>,
    pub theorems: Vec<String>,
    pub out: Option<PathBuf>,
    pub top: usize,
    pub max_diff: usize,
    pub min_size: usize,
    pub everywhere: bool,
}

pub fn twins_args(o: &TwinsOptions, imports: &[String], roots: &[String]) -> Vec<String> {
    let mut a = vec!["--max-diff".to_string(), o.max_diff.to_string(), "--min-size".to_string(), o.min_size.to_string()];
    if o.everywhere {
        a.push("--everywhere".into());
    }
    a.extend(imports.iter().cloned());
    a.push("--".into());
    a.extend(roots.iter().cloned());
    a
}

pub fn twins(o: TwinsOptions) -> Result<()> {
    let root = repo_root(o.root.clone());
    let cfg = Config::load(&o.config.clone().unwrap_or_else(|| root.join("hillclimb.toml")))?;
    let project = root.join(&cfg.project.dir);
    let roots = if o.theorems.is_empty() { cfg.project.theorems.clone() } else { o.theorems.clone() };
    if roots.is_empty() {
        bail!("no root theorems: set `project.theorems` in hillclimb.toml or pass --theorem");
    }
    let imports = default_imports(&roots, &o.imports);
    println!("Running Twins.lean in {} (imports: {})", project.display(), imports.join(" "));
    let (stdout, secs) = run_lean("Twins.lean", TWINS_LEAN, &project, &twins_args(&o, &imports, &roots))?;
    let twins = parse_twins(&stdout)?;
    let mut cache = BTreeMap::new();
    let tokens: Vec<usize> =
        twins.iter().map(|t| tokens_of_lines(&mut cache, &project, &t.module, t.first, t.last)).collect();
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, t) in twins.iter().enumerate() {
        groups.entry(t.group).or_default().push(i);
    }
    let mut order: Vec<(usize, usize)> = groups
        .iter()
        .map(|(g, m)| (*g, group_saving(&m.iter().map(|&i| tokens[i]).collect::<Vec<_>>())))
        .collect();
    order.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    println!();
    print_summary(&stdout);
    let exact: usize =
        order.iter().filter(|(g, _)| twins[groups[g][0]].how == "exact").map(|(_, s)| s).sum();
    let near: usize = order.iter().filter(|(g, _)| twins[groups[g][0]].how == "near").map(|(_, s)| s).sum();
    println!("\nEstimated saving (all members of a group but the largest): exact {exact} tokens, near {near} tokens.");
    println!("Near pairs overlap; their saving is the most a parametrized statement could save.");
    for (g, saving) in order.iter().take(o.top) {
        let m = &groups[g];
        println!("\n{} group {g}: about {saving} tokens", twins[m[0]].how);
        for &i in m {
            let t = &twins[i];
            let diff = if t.difference == "-" { String::new() } else { format!("  differs: {}", t.difference) };
            println!("{:>8}  {}:{}-{}  {} {}{diff}", tokens[i], module_path(&t.module), t.first, t.last, t.kind, t.name);
        }
    }
    if order.len() > o.top {
        println!("\n    ... {} more groups (--out for the full list)", order.len() - o.top);
    }
    println!("\n({secs:.1} s)");
    if let Some(path) = &o.out {
        let mut s = String::new();
        let mut k = 0;
        for line in stdout.lines() {
            s.push_str(line);
            if line.starts_with("twin\t") {
                s.push_str(&format!("\t{}", tokens[k]));
                k += 1;
            }
            s.push('\n');
        }
        std::fs::write(path, s)?;
        println!("Full list written to {} (twin records with their tokens added)", path.display());
    }
    Ok(())
}

// ---------------------------------------------------------------- tails

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub file: String,
    pub tactic: String,
    /// Start of the step and end of the block: (line, column), lines 1-based, columns in characters.
    pub start: (usize, usize),
    pub end: (usize, usize),
    pub goals: usize,
    pub runs: usize,
}

pub fn parse_hits(out: &str) -> Result<Vec<Hit>> {
    let mut v = Vec::new();
    for (i, line) in out.lines().enumerate() {
        let f: Vec<&str> = line.split('\t').collect();
        if let ["hit", file, _, _, tactic, l1, c1, l2, c2, goals, runs, ..] = f.as_slice() {
            let num = |s: &str| s.parse::<usize>().with_context(|| format!("line {}: bad number `{s}`", i + 1));
            v.push(Hit {
                file: file.to_string(),
                tactic: tactic.to_string(),
                start: (num(l1)?, num(c1)?),
                end: (num(l2)?, num(c2)?),
                goals: num(goals)?,
                runs: num(runs)?,
            });
        }
    }
    Ok(v)
}

/// The text between two (line, column) positions; columns count characters.
pub fn slice(src: &str, start: (usize, usize), end: (usize, usize)) -> String {
    let lines: Vec<&str> = src.split('\n').collect();
    let mut s = String::new();
    for l in start.0..=end.0 {
        let Some(text) = lines.get(l.wrapping_sub(1)) else { break };
        let chars: Vec<char> = text.chars().collect();
        let a = if l == start.0 { start.1.min(chars.len()) } else { 0 };
        let b = if l == end.0 { end.1.min(chars.len()) } else { chars.len() };
        s.extend(&chars[a..b.max(a)]);
        if l != end.0 {
            s.push('\n');
        }
    }
    s
}

/// Tokens saved by a hit: those of the replaced range minus those of the tactic.
pub fn saving(src: &str, h: &Hit) -> usize {
    let n = lexer::tokenize(&slice(src, h.start, h.end)).len();
    n.saturating_sub(lexer::tokenize(&h.tactic).len())
}

/// Hits of one file that do not overlap, largest saving first.
pub fn disjoint(hits: &[(Hit, usize)]) -> Vec<(Hit, usize)> {
    let mut sorted: Vec<&(Hit, usize)> = hits.iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.start.cmp(&b.0.start)));
    let mut kept: Vec<(Hit, usize)> = Vec::new();
    for (h, s) in sorted {
        let overlaps = kept.iter().any(|(k, _)| k.file == h.file && h.start < k.end && k.start < h.end);
        if !overlaps {
            kept.push((h.clone(), *s));
        }
    }
    kept
}

/// The candidates: `--tactic` if given; else the arena's search tactics when it
/// allows them, followed by the specific ones; else only the specific ones.
pub fn candidates(given: &[String], enabled: bool, arena: &[String]) -> Vec<String> {
    if !given.is_empty() {
        return given.to_vec();
    }
    let mut v: Vec<String> = if enabled { arena.to_vec() } else { vec![] };
    for t in SPECIFIC_TACTICS {
        if !v.iter().any(|x| x == t) {
            v.push(t.to_string());
        }
    }
    v
}

pub fn tails_args(tactics: &[String], heartbeats: u64, min_chars: u64, file: &str) -> Vec<String> {
    let mut a = Vec::new();
    for t in tactics {
        a.push("--tactic".to_string());
        a.push(t.clone());
    }
    a.extend(["--heartbeats".to_string(), heartbeats.to_string(), "--min-chars".to_string(), min_chars.to_string()]);
    a.push(file.to_string());
    a
}

/// Files already finished in an output written by an earlier run.
pub fn finished_files(out: &str) -> BTreeSet<String> {
    out.lines().filter_map(|l| l.strip_prefix("file\t")).filter_map(|r| r.split('\t').next()).map(str::to_string).collect()
}

pub struct TailsOptions {
    pub root: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub files: Vec<String>,
    pub all: bool,
    pub tactics: Vec<String>,
    pub heartbeats: u64,
    pub min_saving: Option<u64>,
    pub out: Option<PathBuf>,
    pub resume: bool,
}

/// A path given on the command line, relative to the project directory.
fn project_relative(project: &Path, root: &Path, f: &str) -> String {
    let p = Path::new(f);
    let cands = [std::env::current_dir().unwrap_or_default().join(p), root.join(p), project.join(p)];
    for c in cands {
        if let (Ok(c), Ok(pr)) = (c.canonicalize(), project.canonicalize()) {
            if let Ok(rel) = c.strip_prefix(&pr) {
                return rel.to_string_lossy().into_owned();
            }
        }
    }
    f.to_string()
}

pub fn tails(o: TailsOptions) -> Result<()> {
    let root = repo_root(o.root.clone());
    let cfg = Config::load(&o.config.clone().unwrap_or_else(|| root.join("hillclimb.toml")))?;
    let project = root.join(&cfg.project.dir);
    let st = &cfg.conduct.search_tactics;
    let min_saving = o.min_saving.unwrap_or(st.min_saving);
    if !st.enabled {
        println!("Note: this arena does not allow search tactics (`conduct.search_tactics.enabled = false`).");
        if o.tactics.is_empty() {
            println!("Only tactics for one kind of goal are tried ({}); pass --tactic to choose.", SPECIFIC_TACTICS.join(", "));
        }
    }
    let tactics = candidates(&o.tactics, st.enabled, &st.tactics);
    let mut files: Vec<String> = o.files.iter().map(|f| project_relative(&project, &root, f)).collect();
    if o.all {
        let counted = cfg.counted_set()?;
        let frozen = cfg.frozen_set()?;
        let prefix = cfg.dir_prefix();
        for path in hc_core::eval::tree(&root)?.keys() {
            if let Some(rel) = path.strip_prefix(&prefix) {
                if counted.is_match(rel) && !frozen.is_match(path) && rel.ends_with(".lean") {
                    files.push(rel.to_string());
                }
            }
        }
    }
    if files.is_empty() {
        bail!("no files: name them, or pass --all");
    }
    let done = match (&o.out, o.resume) {
        (Some(p), true) => finished_files(&std::fs::read_to_string(p).unwrap_or_default()),
        _ => BTreeSet::new(),
    };
    if let (Some(p), false) = (&o.out, o.resume) {
        std::fs::write(p, "")?;
    }
    println!("Candidates: {}; at least {min_saving} tokens saved per place; {} heartbeats (thousands) per try.", tactics.join(", "), o.heartbeats);
    let mut all: Vec<(Hit, usize)> = Vec::new();
    let todo: Vec<&String> = files.iter().filter(|f| !done.contains(*f)).collect();
    for (i, f) in todo.iter().enumerate() {
        let src = std::fs::read_to_string(project.join(f)).with_context(|| format!("reading {f}"))?;
        let (stdout, secs) =
            match run_lean("Tails.lean", TAILS_LEAN, &project, &tails_args(&tactics, o.heartbeats, min_saving, f)) {
                Ok(r) => r,
                Err(e) => {
                    println!("[{}/{}] {f}: {e}", i + 1, todo.len());
                    continue;
                }
            };
        let hits = parse_hits(&stdout)?;
        let mut kept = Vec::new();
        let mut lines = String::new();
        for h in hits {
            let s = saving(&src, &h);
            if (s as u64) < min_saving {
                continue;
            }
            lines.push_str(&format!(
                "hit\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                h.file, h.start.0, h.start.1, h.tactic, h.end.0, h.end.1, h.goals, h.runs, s, secs.round()
            ));
            kept.push((h, s));
        }
        lines.push_str(&format!("file\t{f}\t{}\t{:.1}\n", kept.len(), secs));
        if let Some(p) = &o.out {
            std::fs::OpenOptions::new().create(true).append(true).open(p)?.write_all(lines.as_bytes())?;
        }
        let dj = disjoint(&kept);
        println!(
            "[{}/{}] {f}: {} hits, about {} tokens without overlaps ({secs:.0} s)",
            i + 1,
            todo.len(),
            kept.len(),
            dj.iter().map(|x| x.1).sum::<usize>()
        );
        for (h, s) in &kept {
            let several = if h.goals > 1 || h.runs > 1 { format!("  ({} goals, {} runs)", h.goals, h.runs) } else { String::new() };
            println!("    {}:{}  {}  saves {s}  (to {}:{}){several}", h.start.0, h.start.1, h.tactic, h.end.0, h.end.1);
        }
        all.extend(dj);
    }
    let mut by_folder: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for (h, s) in &all {
        let folder = Path::new(&h.file).parent().map(|p| p.display().to_string()).unwrap_or_default();
        let e = by_folder.entry(if folder.is_empty() { ".".into() } else { folder }).or_default();
        e.0 += 1;
        e.1 += s;
    }
    println!("\nBy folder (hits without overlaps, tokens saved at most):");
    for (folder, (n, s)) in &by_folder {
        println!("{s:>8} tokens {n:>5} hits  {folder}");
    }
    println!(
        "\nThese are upper bounds: about half of such tokens were realized in the first arena; a hit can fail once \
         the text is replaced; compile time is not measured (use `hc time`)."
    );
    if let Some(p) = &o.out {
        println!("Records appended to {}", p.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn programs_are_embedded_with_short_lines() {
        for src in [TWINS_LEAN, TAILS_LEAN] {
            assert!(src.contains("def main"));
            assert!(src.lines().all(|l| l.chars().count() <= 100));
        }
    }

    #[test]
    fn parses_twins_and_estimates_saving() {
        let out = "twin\t1\texact\tA.B\tA.B.f\ttheorem\t3\t5\t-\n\
                   twin\t1\texact\tA.C\tA.C.g\ttheorem\t7\t7\t-\n\
                   twin\t2\tnear\tA.B\tA.B.p\ttheorem\t9\t9\tNat.succ ↔ Nat.pred\n\
                   # 3 reached\n";
        let t = parse_twins(out).unwrap();
        assert_eq!(t.len(), 3);
        assert_eq!(t[2].difference, "Nat.succ ↔ Nat.pred");
        assert_eq!(t[1].first, 7);
        assert_eq!(group_saving(&[30, 12, 40]), 42);
        assert_eq!(group_saving(&[]), 0);
    }

    #[test]
    fn twins_arguments() {
        let o = TwinsOptions {
            root: None,
            config: None,
            imports: vec![],
            theorems: vec![],
            out: None,
            top: 1,
            max_diff: 3,
            min_size: 15,
            everywhere: true,
        };
        assert_eq!(
            twins_args(&o, &["Solution".into()], &["Solution.a".into()]),
            ["--max-diff", "3", "--min-size", "15", "--everywhere", "Solution", "--", "Solution.a"]
        );
    }

    #[test]
    fn hits_slices_and_savings() {
        let src = "theorem t : 1 = 1 := by\n  have h : 2 = 2 := rfl\n  simp [h]\n  rfl\n";
        let out = "hit\tA.lean\t2\t2\tgrind\t2\t2\t4\t5\t1\t1\nfile\tA.lean\t1\t1\t1\t100\n";
        let h = parse_hits(out).unwrap();
        assert_eq!(h.len(), 1);
        assert_eq!(slice(src, h[0].start, h[0].end), "have h : 2 = 2 := rfl\n  simp [h]\n  rfl");
        // 8 + 4 + 1 tokens replaced by one.
        assert_eq!(saving(src, &h[0]), 12);
        assert_eq!(finished_files(out).into_iter().collect::<Vec<_>>(), ["A.lean"]);
    }

    #[test]
    fn overlapping_hits_count_once() {
        let h = |a: (usize, usize), b: (usize, usize)| Hit {
            file: "A.lean".into(),
            tactic: "grind".into(),
            start: a,
            end: b,
            goals: 1,
            runs: 1,
        };
        let v = vec![(h((2, 2), (9, 0)), 40), (h((5, 4), (9, 0)), 20), (h((12, 2), (14, 0)), 30)];
        let d = disjoint(&v);
        assert_eq!(d.iter().map(|x| x.1).collect::<Vec<_>>(), [40, 30]);
    }

    #[test]
    fn candidate_lists() {
        let arena = vec!["grind".to_string(), "simp_all".to_string()];
        assert_eq!(candidates(&[], true, &arena)[..3], ["grind", "simp_all", "omega"]);
        assert_eq!(candidates(&[], false, &arena), SPECIFIC_TACTICS);
        assert_eq!(candidates(&["aesop".into()], false, &arena), ["aesop"]);
        let a = tails_args(&["grind".into()], 20000, 25, "Nc/A.lean");
        assert_eq!(a, ["--tactic", "grind", "--heartbeats", "20000", "--min-chars", "25", "Nc/A.lean"]);
    }
}
