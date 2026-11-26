//! `hc time`: compile changed files in a built checkout of the base and in
//! this one, alternating, and compare the medians with the conduct limit.

use crate::Compared;
use anyhow::{bail, Result};
use hc_core::config::Conduct;
use hc_core::lexer;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

/// Lines of a failing run's output shown in the report.
const ERROR_LINES: usize = 12;

pub struct Options {
    pub files: Vec<PathBuf>,
    pub base: PathBuf,
    pub runs: usize,
    pub command: String,
    pub markdown: bool,
}

/// The limit that applies to one file.
#[derive(Debug, Clone, PartialEq)]
pub struct Limit {
    /// Percent; `None` when the arena states no limit.
    pub pct: Option<u32>,
    /// Which rule: "general" or "search tactic".
    pub rule: &'static str,
    /// Set when the change adds a search tactic although the arena does not allow it.
    pub note: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FileTiming {
    pub path: String,
    pub base: Vec<f64>,
    pub change: Vec<f64>,
    pub limit: Limit,
}

impl FileTiming {
    pub fn base_median(&self) -> f64 {
        median(&self.base)
    }
    pub fn change_median(&self) -> f64 {
        median(&self.change)
    }
    pub fn pct(&self) -> f64 {
        pct_change(self.base_median(), self.change_median())
    }
    pub fn over(&self) -> bool {
        self.limit.pct.is_some_and(|l| self.pct() > l as f64)
    }
    fn verdict(&self) -> &'static str {
        match self.limit.pct {
            None => "no limit",
            Some(_) if self.over() => "over",
            Some(_) => "pass",
        }
    }
}

pub fn median(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    let mut v = xs.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// Percent change from `base` to `change`.
pub fn pct_change(base: f64, change: f64) -> f64 {
    if base <= 0.0 {
        return 0.0;
    }
    (change - base) / base * 100.0
}

/// How often one of `tactics` is called in the code of `src` (comments excluded).
/// A call is the tactic's name as a whole token, also with `!` or `?` after it.
fn tactic_calls(src: &str, tactics: &[String]) -> usize {
    lexer::tokenize(src)
        .iter()
        .filter(|t| {
            let name = t.text.trim_end_matches(['!', '?']);
            tactics.iter().any(|tac| tac == name)
        })
        .count()
}

/// The limit for a file whose base text is `base_src` and changed text is
/// `head_src`: the search-tactic limit if the change adds a call to one of the
/// search tactics (more calls than before) and that rule is enabled; else the
/// general one.
pub fn limit_for(conduct: &Conduct, base_src: &str, head_src: &str) -> Limit {
    let pct = |p: u32| if p == 0 { None } else { Some(p) };
    let st = &conduct.search_tactics;
    let adds = !st.tactics.is_empty() && tactic_calls(head_src, &st.tactics) > tactic_calls(base_src, &st.tactics);
    if adds && st.enabled {
        return Limit { pct: pct(st.max_slowdown_pct), rule: "search tactic", note: None };
    }
    Limit {
        pct: pct(conduct.max_slowdown_pct),
        rule: "general",
        note: adds.then(|| "adds a search tactic, which this arena does not allow".to_string()),
    }
}

/// The command for one file: `{file}` replaced by the shell-quoted path.
pub fn command_for(template: &str, file: &str) -> String {
    let quoted = format!("'{}'", file.replace('\'', r"'\''"));
    template.replace("{file}", &quoted)
}

pub fn check_options(o: &Options) -> Result<()> {
    if o.runs == 0 {
        bail!("--runs must be at least 1");
    }
    if !o.command.contains("{file}") {
        bail!("--command must contain `{{file}}`, where the file's path (relative to the project directory) goes");
    }
    Ok(())
}

fn secs(x: f64) -> String {
    format!("{x:.2}")
}

fn limit_text(l: &Limit) -> String {
    match l.pct {
        Some(p) => format!("{p}% ({})", l.rule),
        None => "none".into(),
    }
}

/// The plain-text report of one timed file.
pub fn render_text(t: &FileTiming) -> String {
    let mut s = format!("{}\n", t.path);
    s.push_str(&format!("  {:>6} {:>10} {:>10}\n", "run", "base (s)", "change (s)"));
    for i in 0..t.base.len().max(t.change.len()) {
        let cell = |v: &[f64]| v.get(i).map(|x| secs(*x)).unwrap_or_default();
        s.push_str(&format!("  {:>6} {:>10} {:>10}\n", i + 1, cell(&t.base), cell(&t.change)));
    }
    s.push_str(&format!(
        "  {:>6} {:>10} {:>10}   {:+.1}%   limit {}   {}\n",
        "median",
        secs(t.base_median()),
        secs(t.change_median()),
        t.pct(),
        limit_text(&t.limit),
        t.verdict()
    ));
    if let Some(n) = &t.limit.note {
        s.push_str(&format!("  note: {n}\n"));
    }
    s
}

/// A Markdown table of all timed files, for a pull request description.
pub fn render_markdown(ts: &[FileTiming], runs: usize, command: &str) -> String {
    let mut s = format!(
        "Compile time, {runs} alternating runs of each after one warm-up run (`{command}`), wall-clock seconds:\n\n"
    );
    s.push_str("| File | Base runs | Change runs | Base median | Change median | Change | Limit | Result |\n");
    s.push_str("|---|---|---|---|---|---|---|---|\n");
    for t in ts {
        let list = |v: &[f64]| v.iter().map(|x| secs(*x)).collect::<Vec<_>>().join(", ");
        s.push_str(&format!(
            "| `{}` | {} | {} | {} | {} | {:+.1}% | {} | {} |\n",
            t.path,
            list(&t.base),
            list(&t.change),
            secs(t.base_median()),
            secs(t.change_median()),
            t.pct(),
            limit_text(&t.limit),
            t.verdict()
        ));
    }
    s
}

/// The repository-relative form of a path given on the command line.
fn normalize(head: &Path, prefix: &str, p: &Path) -> Result<String> {
    let rel = if p.is_absolute() {
        let root = head.canonicalize()?;
        let abs = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        match abs.strip_prefix(&root) {
            Ok(r) => r.to_path_buf(),
            Err(_) => bail!("{} is not inside {}", p.display(), head.display()),
        }
    } else {
        p.to_path_buf()
    };
    let s = rel.to_string_lossy().replace('\\', "/");
    let s = s.trim_start_matches("./").to_string();
    if !head.join(&s).exists() && head.join(format!("{prefix}{s}")).exists() {
        return Ok(format!("{prefix}{s}"));
    }
    Ok(s)
}

/// Run `command` once in `dir`. Ok(seconds), or Err(the first lines of its output).
fn run_once(dir: &Path, command: &str) -> std::result::Result<f64, String> {
    let start = Instant::now();
    let out = Command::new("sh").arg("-c").arg(command).current_dir(dir).output();
    let elapsed = start.elapsed().as_secs_f64();
    match out {
        Err(e) => Err(format!("could not run `{command}`: {e}")),
        Ok(o) if o.status.success() => Ok(elapsed),
        Ok(o) => {
            let text = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
            let lines: Vec<&str> = text.lines().take(ERROR_LINES).collect();
            Err(format!("`{command}` in {} exited with {}:\n    {}", dir.display(), o.status, lines.join("\n    ")))
        }
    }
}

/// Whether what the timed files import is built, as the build tool sees it.
#[derive(Debug, Clone, PartialEq)]
pub enum Imports {
    UpToDate,
    /// These modules would be built again.
    Stale(Vec<String>),
    /// The build tool could not say (its last words).
    Unknown(String),
}

/// What `lake build --no-build <file>:deps …` said: it exits 0 when every
/// import of the files is up to date, and 3 after naming the modules that
/// are not. Lake decides by the hashes in its trace files, so a build
/// directory that was copied, or sources that were checked out again, are
/// up to date although every file time differs. (Comparing the times of
/// source and compiled files, as this command did, warned about every
/// module then.)
pub fn imports_state(exit: Option<i32>, output: &str) -> Imports {
    match exit {
        Some(0) => Imports::UpToDate,
        Some(3) => {
            let mut stale: Vec<String> =
                output.lines().filter_map(|l| l.trim().strip_prefix("- ")).map(str::to_string).collect();
            stale.sort();
            stale.dedup();
            Imports::Stale(stale)
        }
        _ => Imports::Unknown(output.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("no output").trim().to_string()),
    }
}

/// Ask lake about the imports of `files` (paths relative to `project`).
/// Nothing is built.
fn ask_lake(project: &Path, files: &BTreeSet<String>) -> Imports {
    let targets: Vec<String> = files.iter().filter(|f| project.join(f).is_file()).map(|f| format!("{f}:deps")).collect();
    if targets.is_empty() {
        return Imports::UpToDate;
    }
    match Command::new("lake").args(["build", "--no-build"]).args(&targets).current_dir(project).output() {
        Ok(o) => {
            let text = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
            imports_state(o.status.code(), &text)
        }
        Err(e) => Imports::Unknown(format!("`lake` could not be run: {e}")),
    }
}

/// What to tell the user about a checkout's build, if anything.
pub fn build_warning(label: &str, project: &Path, state: &Imports) -> Option<String> {
    match state {
        Imports::UpToDate => None,
        Imports::Stale(modules) => Some(format!(
            "warning: {label}: lake would build {} module(s) in {} again that the timed files import{}. A timed file would fail or be timed against the wrong thing; run `lake build` there first.",
            modules.len().max(1),
            project.display(),
            if modules.is_empty() {
                String::new()
            } else {
                format!(" (for example {})", modules.iter().take(3).cloned().collect::<Vec<_>>().join(", "))
            }
        )),
        Imports::Unknown(why) => Some(format!(
            "warning: {label}: could not ask lake whether {} is built ({why}); if a file fails to compile below, run `lake build` there first.",
            project.display()
        )),
    }
}

fn check_build(label: &str, project: &Path, timed: &BTreeSet<String>) -> Result<()> {
    if !project.join(".lake/build").is_dir() {
        bail!(
            "{label}: {} has no `.lake/build`. Build that checkout first (in its project directory: `lake exe cache get && lake build`); `hc time` does not build anything.",
            project.display()
        );
    }
    if let Some(warning) = build_warning(label, project, &ask_lake(project, timed)) {
        eprintln!("{warning}");
    }
    Ok(())
}

pub fn run(head: &Path, c: &Compared, o: &Options) -> Result<()> {
    check_options(o)?;
    let cfg = &c.cfg;
    let prefix = cfg.dir_prefix();
    let head_files: BTreeSet<&str> = c.head.files.iter().map(|f| f.path.as_str()).collect();
    let base_files: BTreeSet<&str> = c.base.files.iter().map(|f| f.path.as_str()).collect();

    let mut timed = Vec::new();
    let mut added = Vec::new();
    let mut deleted = Vec::new();
    if o.files.is_empty() {
        for p in hc_core::eval::changed_paths(&c.base, &c.head) {
            match (base_files.contains(p.as_str()), head_files.contains(p.as_str())) {
                (true, true) => timed.push(p),
                (false, true) => added.push(p),
                (true, false) => deleted.push(p),
                _ => {}
            }
        }
    } else {
        for f in &o.files {
            let p = normalize(head, &prefix, f)?;
            if !p.starts_with(&prefix) {
                bail!("{p} is not under the project directory `{}`", cfg.project.dir);
            }
            match (o.base.join(&p).is_file(), head.join(&p).is_file()) {
                (true, true) => timed.push(p),
                (false, true) => added.push(p),
                (true, false) => deleted.push(p),
                _ => bail!("{p} exists in neither checkout"),
            }
        }
    }
    let quiet = o.markdown;
    for p in &added {
        if !quiet {
            println!("added (not timed): {p}");
        }
    }
    for p in &deleted {
        if !quiet {
            println!("deleted (not timed): {p}");
        }
    }
    if timed.is_empty() {
        if !quiet {
            println!("No file to time.");
        }
        return Ok(());
    }

    let base_project = o.base.join(&cfg.project.dir);
    let head_project = head.join(&cfg.project.dir);
    let in_project: BTreeSet<String> = timed.iter().map(|p| p[prefix.len()..].to_string()).collect();
    check_build("base", &base_project, &in_project)?;
    check_build("change", &head_project, &in_project)?;

    let mut results = Vec::new();
    let mut failed = 0;
    for p in &timed {
        let base_src = std::fs::read_to_string(o.base.join(p)).unwrap_or_default();
        let head_src = std::fs::read_to_string(head.join(p)).unwrap_or_default();
        let ref_src = std::fs::read_to_string(c.base_dir.join(p)).ok();
        if !quiet && ref_src.as_deref().is_some_and(|r| r != base_src) {
            println!("note: {p} in {} differs from the base ref; timing against the checkout", o.base.display());
        }
        let limit = limit_for(&cfg.conduct, ref_src.as_deref().unwrap_or(&base_src), &head_src);
        let cmd = command_for(&o.command, &p[prefix.len()..]);
        if !quiet {
            eprintln!("timing {p} ({} runs each) ...", o.runs);
        }
        let mut t = FileTiming { path: p.clone(), base: Vec::new(), change: Vec::new(), limit };
        let mut error = None;
        // One warm-up run of each, then base and change in turn.
        'runs: for i in 0..=o.runs {
            for (side, dir) in [("base", &base_project), ("change", &head_project)] {
                match run_once(dir, &cmd) {
                    Ok(s) if i > 0 => {
                        if side == "base" {
                            t.base.push(s)
                        } else {
                            t.change.push(s)
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        error = Some(format!("{side}: {e}"));
                        break 'runs;
                    }
                }
            }
        }
        match error {
            Some(e) => {
                failed += 1;
                eprintln!("error: {p} could not be timed (a file that does not compile cannot be timed)\n  {e}");
            }
            None => {
                if !quiet {
                    print!("{}", render_text(&t));
                }
                results.push(t);
            }
        }
    }

    if !results.is_empty() {
        if !quiet {
            println!();
        }
        print!("{}", render_markdown(&results, o.runs, &o.command));
        if !quiet {
            let over = results.iter().filter(|t| t.over()).count();
            println!(
                "\n{} of {} file(s) over the limit. Timings are noisy: being over is reported, not enforced by any gate.",
                over,
                results.len()
            );
        }
    }
    if failed > 0 {
        bail!("{failed} file(s) could not be timed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn medians_and_percentages() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), 2.5);
        assert_eq!(median(&[]), 0.0);
        assert!((pct_change(10.0, 11.0) - 10.0).abs() < 1e-9);
        assert!((pct_change(10.0, 9.0) + 10.0).abs() < 1e-9);
        assert_eq!(pct_change(0.0, 1.0), 0.0);
    }

    fn timing(base: &[f64], change: &[f64], pct: Option<u32>) -> FileTiming {
        FileTiming {
            path: "p/A.lean".into(),
            base: base.to_vec(),
            change: change.to_vec(),
            limit: Limit { pct, rule: "general", note: None },
        }
    }

    #[test]
    fn verdicts() {
        assert_eq!(timing(&[10.0], &[10.5], Some(10)).verdict(), "pass");
        assert_eq!(timing(&[10.0], &[11.5], Some(10)).verdict(), "over");
        assert_eq!(timing(&[10.0], &[30.0], None).verdict(), "no limit");
    }

    #[test]
    fn tables() {
        let t = timing(&[1.0, 2.0, 3.0], &[2.0, 2.0, 2.4], Some(10));
        let text = render_text(&t);
        assert!(text.contains("median       2.00       2.00   +0.0%   limit 10% (general)   pass"), "{text}");
        assert_eq!(text.lines().count(), 6);
        let md = render_markdown(&[t], 3, "lake env lean {file}");
        assert!(md.contains("| `p/A.lean` | 1.00, 2.00, 3.00 | 2.00, 2.00, 2.40 | 2.00 | 2.00 | +0.0% | 10% (general) | pass |"), "{md}");
        assert!(md.starts_with("Compile time, 3 alternating runs"));
    }

    #[test]
    fn choice_of_limit() {
        let mut c = Conduct::default();
        let base = "theorem a : True := by\n  trivial\n";
        let with = "theorem a : True := by\n  grind\n";
        let in_comment = "theorem a : True := by\n  trivial -- grind would do\n";
        // Off by default: the general limit, with a note.
        let l = limit_for(&c, base, with);
        assert_eq!((l.pct, l.rule), (Some(10), "general"));
        assert!(l.note.is_some());
        c.search_tactics.enabled = true;
        assert_eq!(limit_for(&c, base, with).pct, Some(20));
        assert_eq!(limit_for(&c, base, with).rule, "search tactic");
        // A call already there, one in a comment, or `simp_all?` replaced by `simp_all`: no new call.
        assert_eq!(limit_for(&c, with, with).pct, Some(10));
        assert_eq!(limit_for(&c, base, in_comment).pct, Some(10));
        assert_eq!(limit_for(&c, "by simp_all?", "by simp_all").pct, Some(10));
        assert_eq!(limit_for(&c, base, "by simp_all!").pct, Some(20));
        c.max_slowdown_pct = 0;
        assert_eq!(limit_for(&c, base, base).pct, None);
    }

    /// What lake 5 prints, taken from runs in a copy of a built project: the
    /// copy has new file times throughout and is up to date all the same.
    #[test]
    fn staleness_is_what_lake_says_not_what_file_times_say() {
        assert_eq!(imports_state(Some(0), "All targets up-to-date (8774 jobs).\n"), Imports::UpToDate);
        let out = "✖ [8772/8774] Building Paper.Lengths\nerror: target is out-of-date and needs to be rebuilt\nSome required targets logged failures:\n- Paper.Lengths\n";
        assert_eq!(imports_state(Some(3), out), Imports::Stale(vec!["Paper.Lengths".into()]));
        assert_eq!(imports_state(Some(1), "error: unknown module `No.Such`\n"), Imports::Unknown("error: unknown module `No.Such`".into()));
        assert_eq!(imports_state(None, ""), Imports::Unknown("no output".into()));

        let project = Path::new("/x/proj");
        assert_eq!(build_warning("base", project, &Imports::UpToDate), None);
        let w = build_warning("base", project, &Imports::Stale(vec!["Paper.Lengths".into()])).unwrap();
        assert!(w.contains("lake would build 1 module(s) in /x/proj again") && w.contains("for example Paper.Lengths"), "{w}");
        assert!(build_warning("change", project, &Imports::Unknown("x".into())).unwrap().contains("could not ask lake"));
        // No timed file exists on this side: nothing to ask, and nothing is run.
        assert_eq!(ask_lake(Path::new("/nonexistent"), &["A.lean".to_string()].into()), Imports::UpToDate);
    }

    #[test]
    fn commands_and_arguments() {
        assert_eq!(command_for("lake env lean {file}", "A/B.lean"), "lake env lean 'A/B.lean'");
        assert_eq!(command_for("lean {file}", "it's.lean"), r"lean 'it'\''s.lean'");
        let opts = |runs, command: &str| Options {
            files: vec![],
            base: PathBuf::from("b"),
            runs,
            command: command.into(),
            markdown: false,
        };
        assert!(check_options(&opts(3, "lake env lean {file}")).is_ok());
        assert!(check_options(&opts(0, "lake env lean {file}")).is_err());
        assert!(check_options(&opts(3, "lake build")).is_err());

        let parse = |args: &[&str]| crate::Cli::try_parse_from(args).map(|c| c.cmd);
        match parse(&["hc", "time", "--base", "../b", "p/A.lean", "p/B.lean"]).unwrap() {
            crate::Cmd::Time { files, base, runs, command, jobs, markdown, .. } => {
                assert_eq!(files.len(), 2);
                assert_eq!(base, PathBuf::from("../b"));
                assert_eq!((runs, jobs, markdown), (3, 1, false));
                assert_eq!(command, "lake env lean {file}");
            }
            _ => panic!("not time"),
        }
        assert!(parse(&["hc", "time"]).is_err(), "--base is required");
        assert!(parse(&["hc", "time", "--base", "b", "--jobs", "2"]).is_err());
        assert!(parse(&["hc", "time", "--base", "b", "--jobs", "1", "--runs", "5", "--markdown"]).is_ok());
    }

    #[test]
    fn paths_are_made_repository_relative() {
        let dir = std::env::temp_dir().join(format!("hc-time-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("proj/A")).unwrap();
        std::fs::write(dir.join("proj/A/B.lean"), "").unwrap();
        assert_eq!(normalize(&dir, "proj/", Path::new("proj/A/B.lean")).unwrap(), "proj/A/B.lean");
        assert_eq!(normalize(&dir, "proj/", Path::new("./proj/A/B.lean")).unwrap(), "proj/A/B.lean");
        assert_eq!(normalize(&dir, "proj/", Path::new("A/B.lean")).unwrap(), "proj/A/B.lean");
        assert_eq!(normalize(&dir, "proj/", &dir.join("proj/A/B.lean")).unwrap(), "proj/A/B.lean");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
