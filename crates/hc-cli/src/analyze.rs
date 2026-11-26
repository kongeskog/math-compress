//! `hc analyze`: run the Lean analysis programs in `tools/lean` on the project.
//!
//! Both programs are embedded in the binary, written to a temporary file and
//! run as `lake env lean --run <file> <module>… -- <root>…` inside the project
//! directory, which must be built. Their output is tab-separated records plus
//! summary lines starting with `# ` (see the header of each `.lean` file).

use anyhow::{bail, Context, Result};
use hc_core::config::Config;
use hc_core::lexer;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const REACH_LEAN: &str = include_str!("../../../tools/lean/Reach.lean");
pub const UNUSED_LEAN: &str = include_str!("../../../tools/lean/Unused.lean");

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Reach,
    Unused,
}

impl Tool {
    fn file_name(self) -> &'static str {
        match self {
            Tool::Reach => "Reach.lean",
            Tool::Unused => "Unused.lean",
        }
    }

    fn source(self) -> &'static str {
        match self {
            Tool::Reach => REACH_LEAN,
            Tool::Unused => UNUSED_LEAN,
        }
    }
}

/// The modules to import: the given ones, else the distinct first components
/// of the root names (`Solution.mainTheorem` -> `Solution`), in order.
pub fn default_imports(roots: &[String], given: &[String]) -> Vec<String> {
    if !given.is_empty() {
        return given.to_vec();
    }
    let mut out: Vec<String> = Vec::new();
    for r in roots {
        let first = r.split('.').next().unwrap_or(r).to_string();
        if !first.is_empty() && !out.contains(&first) {
            out.push(first);
        }
    }
    out
}

/// Arguments after `lake`: `env lean --run <file> <module>… -- <root>…`.
pub fn lake_args(file: &Path, imports: &[String], roots: &[String]) -> Vec<String> {
    let mut a = vec!["env".to_string(), "lean".to_string(), "--run".to_string(), file.display().to_string()];
    a.extend(imports.iter().cloned());
    a.push("--".to_string());
    a.extend(roots.iter().cloned());
    a
}

/// One unreached declaration reported by `Reach.lean`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decl {
    pub module: String,
    pub name: String,
    pub kind: String,
    pub first: usize,
    pub last: usize,
    pub flags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileTotal {
    pub module: String,
    pub path: String,
    pub unreached: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReachReport {
    pub decls: Vec<Decl>,
    pub files: Vec<FileTotal>,
    pub whole: Vec<String>,
    pub unimported: Vec<String>,
    pub summary: Vec<String>,
}

/// Parse the output of `Reach.lean`. Unknown records are ignored.
pub fn parse_reach(out: &str) -> Result<ReachReport> {
    let mut r = ReachReport::default();
    for (i, line) in out.lines().enumerate() {
        if let Some(s) = line.strip_prefix("# ") {
            r.summary.push(s.to_string());
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        let num = |s: &str| s.parse::<usize>().with_context(|| format!("line {}: bad number `{s}`", i + 1));
        match f.as_slice() {
            ["decl", module, name, kind, first, last, flags, ..] => r.decls.push(Decl {
                module: module.to_string(),
                name: name.to_string(),
                kind: kind.to_string(),
                first: num(first)?,
                last: num(last)?,
                flags: if *flags == "-" { vec![] } else { flags.split(',').map(str::to_string).collect() },
            }),
            ["file", module, path, unreached, total, ..] => r.files.push(FileTotal {
                module: module.to_string(),
                path: path.to_string(),
                unreached: num(unreached)?,
                total: num(total)?,
            }),
            ["whole", module, ..] => r.whole.push(module.to_string()),
            ["unimported", path, ..] => r.unimported.push(path.to_string()),
            _ => {}
        }
    }
    Ok(r)
}

/// Source file of a module, relative to the project directory.
pub fn module_path(module: &str) -> String {
    format!("{}.lean", module.replace('.', "/"))
}

/// Token estimates: per declaration (same order as `decls`), and per module
/// (tokens of its unreached declarations, tokens of the whole file).
pub struct Estimate {
    pub decl_tokens: Vec<usize>,
    pub module_tokens: BTreeMap<String, (usize, usize)>,
}

/// Count the tokens of each reported line range with the scoring tokenizer.
/// A file that cannot be read counts zero.
pub fn estimate(project: &Path, report: &ReachReport) -> Estimate {
    let mut cache: BTreeMap<String, Vec<lexer::Token>> = BTreeMap::new();
    let mut decl_tokens = Vec::new();
    let mut module_tokens: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for d in &report.decls {
        let toks = cache.entry(d.module.clone()).or_insert_with(|| {
            std::fs::read_to_string(project.join(module_path(&d.module))).map(|s| lexer::tokenize(&s)).unwrap_or_default()
        });
        let n = toks.iter().filter(|t| t.line >= d.first && t.line <= d.last).count();
        let whole = toks.len();
        decl_tokens.push(n);
        let e = module_tokens.entry(d.module.clone()).or_insert((0, whole));
        e.0 += n;
    }
    Estimate { decl_tokens, module_tokens }
}

/// The full list with token columns added: `decl` records get the tokens of
/// their lines, `file` records the tokens of the unreached declarations and of
/// the whole file.
pub fn annotate_reach(out: &str, est: &Estimate) -> String {
    let mut next = 0;
    let mut s = String::new();
    for line in out.lines() {
        let tag = line.split('\t').next().unwrap_or("");
        s.push_str(line);
        match tag {
            "decl" => {
                s.push_str(&format!("\t{}", est.decl_tokens.get(next).copied().unwrap_or(0)));
                next += 1;
            }
            "file" => {
                let module = line.split('\t').nth(1).unwrap_or("");
                let (u, w) = est.module_tokens.get(module).copied().unwrap_or((0, 0));
                s.push_str(&format!("\t{u}\t{w}"));
            }
            _ => {}
        }
        s.push('\n');
    }
    s
}

/// The repository root: `--root` if given, else the top of the git work tree
/// around the current directory, else the current directory.
pub fn repo_root(root: Option<PathBuf>) -> PathBuf {
    if let Some(r) = root {
        return r;
    }
    Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub struct Run {
    pub stdout: String,
    pub seconds: f64,
}

/// Write the tool to a temporary file and run it inside the project directory.
pub fn run_tool(tool: Tool, project: &Path, imports: &[String], roots: &[String]) -> Result<Run> {
    if roots.is_empty() {
        bail!("no root theorems: set `project.theorems` in hillclimb.toml or pass --theorem");
    }
    if imports.is_empty() {
        bail!("no modules to import; pass --import");
    }
    // The program's own arguments: what follows `env lean --run <file>`.
    let args = lake_args(Path::new(tool.file_name()), imports, roots).split_off(4);
    let (stdout, seconds) = crate::analyze_more::run_lean(tool.file_name(), tool.source(), project, &args)?;
    Ok(Run { stdout, seconds })
}

pub struct Options {
    pub root: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub imports: Vec<String>,
    pub theorems: Vec<String>,
    pub out: Option<PathBuf>,
    pub top: usize,
}

pub fn analyze(tool: Tool, o: Options) -> Result<()> {
    let root = repo_root(o.root);
    let cfg = Config::load(&o.config.unwrap_or_else(|| root.join("hillclimb.toml")))?;
    let project = root.join(&cfg.project.dir);
    let roots = if o.theorems.is_empty() { cfg.project.theorems.clone() } else { o.theorems };
    let imports = default_imports(&roots, &o.imports);
    println!("Running {} in {} (imports: {})", tool.file_name(), project.display(), imports.join(" "));
    let run = run_tool(tool, &project, &imports, &roots)?;
    match tool {
        Tool::Reach => print_reach(&project, &run, o.out.as_deref(), o.top),
        Tool::Unused => print_unused(&run, o.out.as_deref(), o.top),
    }
}

pub fn print_summary(stdout: &str) {
    for line in stdout.lines() {
        if let Some(s) = line.strip_prefix("# ") {
            println!("{s}");
        }
    }
}

fn print_reach(project: &Path, run: &Run, out: Option<&Path>, top: usize) -> Result<()> {
    let report = parse_reach(&run.stdout)?;
    let est = estimate(project, &report);
    println!();
    print_summary(&run.stdout);
    let total: usize = est.decl_tokens.iter().sum();
    let flagged: usize =
        report.decls.iter().zip(&est.decl_tokens).filter(|(d, _)| !d.flags.is_empty()).map(|(_, n)| n).sum();
    println!("\nAbout {total} tokens in unreached declarations ({flagged} of them in flagged ones).");
    if !report.whole.is_empty() {
        println!("\nWhole files (tokens of the file):");
        for m in &report.whole {
            let (_, w) = est.module_tokens.get(m).copied().unwrap_or((0, 0));
            println!("{w:>8}  {}", module_path(m));
        }
    }
    let mut mods: Vec<(&String, &(usize, usize))> = est.module_tokens.iter().collect();
    mods.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then(a.0.cmp(b.0)));
    if !mods.is_empty() {
        println!("\nUnreached tokens per module:");
        for (m, (u, w)) in mods.iter().take(top) {
            println!("{u:>8} of {w:>7}  {}", module_path(m));
        }
    }
    let mut order: Vec<usize> = (0..report.decls.len()).collect();
    order.sort_by(|&a, &b| est.decl_tokens[b].cmp(&est.decl_tokens[a]).then(a.cmp(&b)));
    if !order.is_empty() {
        println!("\nLargest unreached declarations:");
        for &i in order.iter().take(top) {
            let d = &report.decls[i];
            let flags = if d.flags.is_empty() { String::new() } else { format!("  [{}]", d.flags.join(",")) };
            println!(
                "{:>8}  {}:{}-{}  {} {}{flags}",
                est.decl_tokens[i],
                module_path(&d.module),
                d.first,
                d.last,
                d.kind,
                d.name
            );
        }
        if order.len() > top {
            println!("    ... {} more (--out for the full list)", order.len() - top);
        }
    }
    println!("\n({:.1} s)", run.seconds);
    if let Some(path) = out {
        std::fs::write(path, annotate_reach(&run.stdout, &est))?;
        println!("Full list written to {}", path.display());
    }
    Ok(())
}

fn print_unused(run: &Run, out: Option<&Path>, top: usize) -> Result<()> {
    println!();
    print_summary(&run.stdout);
    let mut by_kind: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for line in run.stdout.lines() {
        if line.starts_with("# ") {
            continue;
        }
        let tag = line.split('\t').next().unwrap_or("");
        if !tag.is_empty() {
            by_kind.entry(tag).or_default().push(line);
        }
    }
    for (kind, lines) in &by_kind {
        println!("\n{kind} ({}):", lines.len());
        for l in lines.iter().take(top) {
            println!("    {}", l.split('\t').skip(1).collect::<Vec<_>>().join("  "));
        }
        if lines.len() > top {
            println!("    ... {} more (--out for the full list)", lines.len() - top);
        }
    }
    println!("\n({:.1} s)", run.seconds);
    if let Some(path) = out {
        std::fs::write(path, &run.stdout)?;
        println!("Full list written to {}", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_default_to_first_components() {
        let roots: Vec<String> = ["Solution.mainTheorem", "Solution.separation", "Paper.thm_x", "Top"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(default_imports(&roots, &[]), ["Solution", "Paper", "Top"]);
        assert_eq!(default_imports(&roots, &["A.B".to_string()]), ["A.B"]);
    }

    #[test]
    fn lake_arguments() {
        let a = lake_args(Path::new("/t/Reach.lean"), &["Solution".into()], &["Solution.a".into(), "Solution.b".into()]);
        assert_eq!(a, ["env", "lean", "--run", "/t/Reach.lean", "Solution", "--", "Solution.a", "Solution.b"]);
    }

    #[test]
    fn embedded_programs_are_present() {
        assert!(REACH_LEAN.contains("def main"));
        assert!(UNUSED_LEAN.contains("def main"));
        for src in [REACH_LEAN, UNUSED_LEAN] {
            assert!(src.lines().all(|l| l.chars().count() <= 100), "Lean lines are at most 100 characters");
        }
    }

    const OUT: &str = "decl\tA.B\tA.B.foo\ttheorem\t3\t4\tsimp,rfl\n\
                       decl\tA.C\tA.C.bar\tdef\t1\t1\t-\n\
                       file\tA.B\tA/B.lean\t1\t5\n\
                       file\tA.C\tA/C.lean\t1\t1\n\
                       whole\tA.C\tA/C.lean\n\
                       unimported\tScratch.lean\n\
                       # 10 project constants reached from 1 roots\n\
                       something else\n";

    #[test]
    fn parses_reach_output() {
        let r = parse_reach(OUT).unwrap();
        assert_eq!(r.decls.len(), 2);
        assert_eq!(
            r.decls[0],
            Decl {
                module: "A.B".into(),
                name: "A.B.foo".into(),
                kind: "theorem".into(),
                first: 3,
                last: 4,
                flags: vec!["simp".into(), "rfl".into()],
            }
        );
        assert!(r.decls[1].flags.is_empty());
        assert_eq!(r.files[0], FileTotal { module: "A.B".into(), path: "A/B.lean".into(), unreached: 1, total: 5 });
        assert_eq!(r.whole, ["A.C"]);
        assert_eq!(r.unimported, ["Scratch.lean"]);
        assert_eq!(r.summary, ["10 project constants reached from 1 roots"]);
        assert!(parse_reach("decl\tA\tx\tdef\tone\t2\t-\n").is_err());
    }

    #[test]
    fn module_paths() {
        assert_eq!(module_path("Paper.Ryser"), "Paper/Ryser.lean");
        assert_eq!(module_path("Solution"), "Solution.lean");
    }

    #[test]
    fn token_estimate_on_a_fixture() {
        let dir = std::env::temp_dir().join(format!("hc-analyze-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("A")).unwrap();
        let b = "import Mathlib\n\n/-- doc -/\n@[simp] theorem foo : 1 = 1 :=\n  rfl\n-- tail\ndef keep := 2\n";
        std::fs::write(dir.join("A/B.lean"), b).unwrap();
        std::fs::write(dir.join("A/C.lean"), "def bar := 1\n").unwrap();
        let r = parse_reach(OUT).unwrap();
        let est = estimate(&dir, &r);
        // Lines 3-4 of A/B.lean: the doc comment is not counted, `@ [ simp ] theorem foo : 1 = 1 :=` is.
        assert_eq!(est.decl_tokens, [11, 4]);
        assert_eq!(lexer::tokenize(b).iter().filter(|t| (3..=5).contains(&t.line)).count(), 12);
        assert_eq!(est.module_tokens["A.B"], (11, 18));
        assert_eq!(est.module_tokens["A.C"], (4, 4));
        let annotated = annotate_reach(OUT, &est);
        assert!(annotated.contains("decl\tA.B\tA.B.foo\ttheorem\t3\t4\tsimp,rfl\t11\n"));
        assert!(annotated.contains("file\tA.B\tA/B.lean\t1\t5\t11\t18\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
