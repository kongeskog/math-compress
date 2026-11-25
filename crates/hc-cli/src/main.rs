mod analyze;
mod gh;
mod ops;
mod precheck;
mod render;
mod timing;

use anyhow::{bail, Context, Result};
use chrono::Utc;
use clap::{Parser, Subcommand, ValueEnum};
use hc_core::config::Config;
use hc_core::eval;
use hc_core::gate::Gatekeeper;
use hc_core::model::*;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Parser)]
#[command(name = "hc", version, about = "Score and gate changes for a hillclimb project")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print the metrics of a checkout and its largest files.
    Score {
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        config: Option<PathBuf>,
        /// How many of the largest files to list.
        #[arg(long, default_value_t = 20)]
        top: usize,
        #[arg(long)]
        json: bool,
    },
    /// List leads for contributors: declarations that are exact copies of each other.
    Leads {
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        config: Option<PathBuf>,
        /// Ignore declarations shorter than this many tokens.
        #[arg(long, default_value_t = 20)]
        min_tokens: usize,
        #[arg(long, default_value_t = 40)]
        top: usize,
        #[arg(long)]
        json: bool,
    },
    /// Compare a checkout with its base and evaluate the static gates.
    Eval {
        /// The changed checkout.
        #[arg(long, default_value = ".")]
        head: PathBuf,
        /// A checkout of the base. If omitted, `--base-ref` is exported from git.
        #[arg(long)]
        base: Option<PathBuf>,
        /// Defaults to `upstream/main` when that remote exists (a fork's clone), else `origin/main`.
        #[arg(long)]
        base_ref: Option<String>,
        #[arg(long, default_value_t = 0)]
        pr: u64,
        #[arg(long, default_value = "")]
        head_sha: String,
        #[arg(long, default_value = "")]
        base_sha: String,
        /// Configuration to judge by. Defaults to the base checkout's own; the
        /// verify workflow passes main's, so that nothing in a stack can relax a rule.
        #[arg(long)]
        config: Option<PathBuf>,
        /// Git tree the change was applied to.
        #[arg(long)]
        base_tree: Option<String>,
        /// Git tree after applying the change.
        #[arg(long)]
        merged_tree: Option<String>,
        /// `pr@head_sha` of changes applied before this one (repeatable).
        #[arg(long)]
        stack: Vec<String>,
        /// Login of the pull request's author, for the open-change limit.
        #[arg(long, default_value = "")]
        author: String,
        /// How many open pull requests that author has, including this one.
        #[arg(long, default_value_t = 0)]
        author_open_prs: usize,
        /// Write the result as JSON.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Add the build outcome to a static result (used by the verify workflow).
    Finalize {
        #[arg(long = "static")]
        static_result: PathBuf,
        #[arg(long, value_enum)]
        compiles: Outcome,
        #[arg(long, value_enum)]
        same_theorems: Outcome,
        #[arg(long)]
        build_seconds: Option<u64>,
        #[arg(long)]
        out: PathBuf,
    },
    /// Reconcile every open pull request and idea, merge what is ready, and
    /// write the dashboard data.
    Gate {
        /// A checkout of the main branch.
        #[arg(long)]
        root: PathBuf,
        /// A checkout of the data branch.
        #[arg(long)]
        data: PathBuf,
        /// Read everything, change nothing on GitHub.
        #[arg(long)]
        dry_run: bool,
    },
    /// Write `ops.json`, the pipeline timings behind the dashboard's operations page.
    Ops {
        /// A checkout of the main branch.
        #[arg(long)]
        root: PathBuf,
        /// A checkout of the data branch.
        #[arg(long)]
        data: PathBuf,
    },
    /// Print one role's instructions (contributor, reviewer, ideator or leader).
    Prompt {
        role: String,
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long, default_value = "")]
        site: String,
    },
    /// Run a Lean analysis of the built project: what the final theorems do not reach,
    /// or what parts of reached declarations nothing uses (candidates, to be checked by building).
    Analyze {
        #[command(subcommand)]
        tool: AnalyzeTool,
    },
    /// Write the site's project files: the invitation, the instructions, rules and config.
    Render {
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        out: PathBuf,
        /// Public URL of the dashboard, used inside the prompts.
        #[arg(long)]
        site: String,
    },
    /// Time the compilation of changed files: in a built checkout of the base and in this one.
    ///
    /// Run from the changed checkout's repository root. For each file the
    /// command runs once in each checkout's project directory untimed, then
    /// `--runs` times in each, alternating (base, change, base, change, ...).
    /// The report gives every run, both medians, the percentage change and
    /// the limit that applies (`conduct.max_slowdown_pct`, or
    /// `conduct.search_tactics.max_slowdown_pct` where the change adds a call
    /// to a search tactic and the arena allows them), then the same as a
    /// Markdown table for the pull request description.
    ///
    /// Nothing is built. Each file is compiled against the other files of its
    /// own checkout as they were last built, so build both checkouts first
    /// (`lake build` in the project directory); the command stops when a
    /// checkout has no `.lake/build` and warns when modules are older than
    /// their sources. A file that does not compile cannot be timed and makes
    /// the exit code non-zero; being over the limit does not (timings are
    /// noisy; a reviewer judges).
    Time {
        /// Files to time, relative to the repository root. Defaults to the
        /// counted files that differ from the base ref; files that exist on
        /// one side only are listed, not timed.
        files: Vec<PathBuf>,
        /// A built checkout of the base (its repository root).
        #[arg(long)]
        base: PathBuf,
        /// Timed runs of each side, after one warm-up run of each.
        #[arg(long, default_value_t = 3)]
        runs: usize,
        /// Run in each project directory; `{file}` is the path relative to it.
        #[arg(long, default_value = "lake env lean {file}")]
        command: String,
        /// Only 1: two compilations at once disturb each other's timings.
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u64).range(1..=1))]
        jobs: u64,
        /// Print only the Markdown table.
        #[arg(long)]
        markdown: bool,
        /// The changed checkout.
        #[arg(long, default_value = ".")]
        head: PathBuf,
        /// Base ref for finding the changed files; defaults as for `hc eval`.
        #[arg(long)]
        base_ref: Option<String>,
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Everything to check before opening a pull request: `hc eval`, deleted
    /// names still mentioned, added lines, open pull requests on the same
    /// files, and the title and description.
    Precheck {
        /// The changed checkout.
        #[arg(long, default_value = ".")]
        head: PathBuf,
        /// A checkout of the base. If omitted, `--base-ref` is exported from git.
        #[arg(long)]
        base: Option<PathBuf>,
        /// Defaults as for `hc eval`.
        #[arg(long)]
        base_ref: Option<String>,
        #[arg(long)]
        config: Option<PathBuf>,
        /// Do not ask GitHub for open pull requests.
        #[arg(long)]
        offline: bool,
        /// The pull request's title, checked with `--body-file` like the format gate.
        #[arg(long)]
        title: Option<String>,
        /// A file holding the pull request's description.
        #[arg(long)]
        body_file: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum AnalyzeTool {
    /// Declarations of the project that the final theorems do not reach, with a token estimate.
    Reach(AnalyzeArgs),
    /// Unused structure fields, conjuncts of statements and hypotheses, and binders.
    Unused(AnalyzeArgs),
}

#[derive(clap::Args)]
struct AnalyzeArgs {
    /// The repository root (with `hillclimb.toml`). Defaults to the enclosing git work tree.
    #[arg(long)]
    root: Option<PathBuf>,
    #[arg(long)]
    config: Option<PathBuf>,
    /// Module to import (repeatable). Defaults to the first components of the theorem names.
    #[arg(long = "import")]
    imports: Vec<String>,
    /// Root constant (repeatable). Defaults to `project.theorems`.
    #[arg(long = "theorem")]
    theorems: Vec<String>,
    /// Write the full tab-separated list here (for `reach` with token columns added).
    #[arg(long)]
    out: Option<PathBuf>,
    /// How many entries of each list to print.
    #[arg(long, default_value_t = 40)]
    top: usize,
}

#[derive(Clone, Copy, ValueEnum)]
enum Outcome {
    Pass,
    Fail,
    Skip,
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Score { root, config, top, json } => score(&root, config, top, json),
        Cmd::Leads { root, config, min_tokens, top, json } => {
            let cfg = Config::load(&config.unwrap_or_else(|| root.join("hillclimb.toml")))?;
            let groups = hc_core::leads::duplicates(&root, &cfg, min_tokens)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&groups)?);
                return Ok(());
            }
            let total: usize = groups.iter().map(|g| g.saving).sum();
            println!("{} groups of identical declarations; removing the copies would save about {total} tokens.\n", groups.len());
            for g in groups.iter().take(top) {
                println!("{} tokens x {} copies{}", g.tokens, g.occurrences.len(), if g.same_name { "" } else { " (different names)" });
                for o in &g.occurrences {
                    println!("    {}:{}  {}", o.path, o.line, o.name);
                }
            }
            Ok(())
        }
        Cmd::Eval { head, base, base_ref, config, pr, head_sha, base_sha, base_tree, merged_tree, stack, author, author_open_prs, out } => {
            eval_cmd(&head, base, base_ref, config, pr, head_sha, base_sha, (base_tree, merged_tree, stack), &author, author_open_prs, out)
        }
        Cmd::Finalize { static_result, compiles, same_theorems, build_seconds, out } => {
            finalize(&static_result, compiles, same_theorems, build_seconds, &out)
        }
        Cmd::Gate { root, data, dry_run } => gate(&root, &data, dry_run),
        Cmd::Ops { root, data } => ops::write(&root, &data, Utc::now()),
        Cmd::Prompt { role, root, site } => {
            let cfg = Config::load(&root.join("hillclimb.toml"))?;
            // Without `--site`, the dashboard is assumed to be the repository's GitHub Pages site.
            let site = match (site.is_empty(), cfg.project.repo.split_once('/')) {
                (true, Some((owner, name))) => format!("https://{}.github.io/{name}/", owner.to_lowercase()),
                _ => site,
            };
            print!("{}", render::role_prompt(&role, &cfg, &site)?);
            Ok(())
        }
        Cmd::Analyze { tool } => {
            let (tool, a) = match tool {
                AnalyzeTool::Reach(a) => (analyze::Tool::Reach, a),
                AnalyzeTool::Unused(a) => (analyze::Tool::Unused, a),
            };
            let o = analyze::Options {
                root: a.root,
                config: a.config,
                imports: a.imports,
                theorems: a.theorems,
                out: a.out,
                top: a.top,
            };
            analyze::analyze(tool, o)
        }
        Cmd::Render { root, out, site } => {
            let cfg = Config::load(&root.join("hillclimb.toml"))?;
            render::site_files(&cfg, &out, &site)
        }
        Cmd::Time { files, base, runs, command, jobs: _, markdown, head, base_ref, config } => {
            let c = compare(&head, None, base_ref, config)?;
            timing::run(&head, &c, &timing::Options { files, base, runs, command, markdown })
        }
        Cmd::Precheck { head, base, base_ref, config, offline, title, body_file } => {
            let c = compare(&head, base, base_ref, config)?;
            let ok = precheck::run(&head, &c, &precheck::Options { offline, title, body_file })?;
            drop(c);
            if !ok {
                std::process::exit(1);
            }
            Ok(())
        }
    }
}

fn score(root: &Path, config: Option<PathBuf>, top: usize, json: bool) -> Result<()> {
    let cfg = Config::load(&config.unwrap_or_else(|| root.join("hillclimb.toml")))?;
    let m = eval::measure(root, &cfg)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&m)?);
        return Ok(());
    }
    print_metrics(&m.metrics, None);
    println!("\nLargest files:");
    for f in m.files.iter().take(top) {
        println!("{:>8} tokens {:>6} lines  {}", f.tokens, f.loc, f.path);
    }
    Ok(())
}

fn print_metrics(m: &Metrics, base: Option<&Metrics>) {
    for key in ["tokens", "loc", "decls", "files", "bytes"] {
        let v = m.get(key).unwrap_or(0);
        match base.and_then(|b| b.get(key)) {
            Some(b) => println!("{key:>8} {b:>10} -> {v:>10}  ({:+})", v as i64 - b as i64),
            None => println!("{key:>8} {v:>10}"),
        }
    }
}

/// Export a git ref into a temporary directory without touching the work tree.
fn export_ref(repo: &Path, reference: &str) -> Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("hc-base-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let status = Command::new("sh")
        .arg("-c")
        .arg("git -C \"$1\" archive \"$2\" | tar -x -C \"$3\"")
        .arg("sh")
        .arg(repo)
        .arg(reference)
        .arg(&dir)
        .status()
        .context("running git archive")?;
    if !status.success() {
        bail!("could not export `{reference}`; run `git fetch origin` or pass --base");
    }
    Ok(dir)
}

#[allow(clippy::too_many_arguments)]
fn eval_cmd(
    head: &Path,
    base: Option<PathBuf>,
    base_ref: Option<String>,
    config: Option<PathBuf>,
    pr: u64,
    head_sha: String,
    base_sha: String,
    trees: (Option<String>, Option<String>, Vec<String>),
    author: &str,
    author_open_prs: usize,
    out: Option<PathBuf>,
) -> Result<()> {
    let c = compare(head, base, base_ref, config)?;
    let (cfg, b, h) = (&c.cfg, &c.base, &c.head);
    let mut gates = eval::static_gates(cfg, b, h)?;
    if !author.is_empty() {
        let exempt = cfg.roles.leaders.iter().any(|l| l.eq_ignore_ascii_case(author));
        let limit = cfg.limits.max_open_prs;
        gates.insert(
            0,
            if exempt || author_open_prs <= limit {
                Gate::new("quota", GateStatus::Pass, "")
            } else {
                Gate::new(
                    "quota",
                    GateStatus::Fail,
                    format!("{author} has {author_open_prs} open pull requests; the limit is {limit}. Close or finish one, then comment `/retry` here."),
                )
            },
        );
    }
    let changed = eval::changed_paths(b, h).len();
    let ok = report(&c, &gates);

    if let Some(out) = out {
        let result = VerifyResult {
            pr,
            head_sha,
            base_sha,
            base_tree: trees.0,
            merged_tree: trees.1,
            stack: trees.2,
            metrics: h.metrics,
            base_metrics: b.metrics,
            gates,
            changed_files: changed,
            top_files: h.files.iter().take(30).cloned().collect(),
            run_url: None,
        };
        std::fs::write(&out, serde_json::to_string_pretty(&result)?)?;
    } else if !ok {
        std::process::exit(1);
    }
    Ok(())
}

/// A changed checkout and its base, both measured with the base's configuration.
/// An exported base is removed when this is dropped.
struct Compared {
    cfg: Config,
    base: eval::Measurement,
    head: eval::Measurement,
    /// The base's files: the `--base` checkout, or the export of the base ref.
    base_dir: PathBuf,
    temp: bool,
}

impl Drop for Compared {
    fn drop(&mut self) {
        if self.temp {
            let _ = std::fs::remove_dir_all(&self.base_dir);
        }
    }
}

/// Print the metrics and each gate as `hc eval` does; true if no gate failed.
fn report(c: &Compared, gates: &[Gate]) -> bool {
    print_metrics(&c.head.metrics, Some(&c.base.metrics));
    println!();
    let mut ok = true;
    for g in gates {
        let mark = match g.status {
            GateStatus::Pass => "pass",
            GateStatus::Fail => {
                ok = false;
                "FAIL"
            }
            _ => "....",
        };
        println!("[{mark}] {} {}", g.label, g.detail.as_deref().map(|d| format!("- {d}")).unwrap_or_default());
    }
    println!(
        "\n{}",
        if ok {
            "Static gates pass. The build and the proof check still have to pass in verification."
        } else {
            "Not acceptable as it stands."
        }
    );
    ok
}

/// Find the base (a checkout, or the point this branch left the base ref)
/// and measure both sides.
fn compare(head: &Path, base: Option<PathBuf>, base_ref: Option<String>, config: Option<PathBuf>) -> Result<Compared> {
    let (base_dir, temp) = match base {
        Some(b) => (b, false),
        None => {
            let reference = base_ref.unwrap_or_else(|| arena_ref(head));
            // Compare with the point this work branched from, so that commits
            // which landed on main since then are not mistaken for this change.
            let git = |args: &[&str]| {
                Command::new("git").arg("-C").arg(head).args(args).output().ok().filter(|o| o.status.success()).map(
                    |o| String::from_utf8_lossy(&o.stdout).trim().to_string(),
                )
            };
            let tip = git(&["rev-parse", &reference]);
            let fork = git(&["merge-base", "HEAD", &reference]);
            match (&tip, &fork) {
                (Some(tip), Some(fork)) if tip != fork => {
                    println!("Comparing with where this branch left {reference} ({}).", &fork[..fork.len().min(10)]);
                    println!("{reference} has moved since; verification applies the change on top of the latest one.\n");
                }
                _ => println!("Comparing with {reference}\n"),
            }
            (export_ref(head, fork.as_deref().unwrap_or(&reference))?, true)
        }
    };
    // The configuration always comes from the base: a change cannot relax its own rules.
    let cfg = Config::load(&config.unwrap_or_else(|| base_dir.join("hillclimb.toml")))?;
    let b = eval::measure(&base_dir, &cfg)?;
    let mut h = eval::measure(head, &cfg)?;
    if temp {
        // A local work tree holds untracked build output the export does not;
        // compare only what git tracks plus new files inside the project.
        let prefix = cfg.dir_prefix();
        let tracked = tracked_files(head);
        h.tree.retain(|p, _| b.tree.contains_key(p) || p.starts_with(&prefix) || tracked.contains(p));
    }
    Ok(Compared { cfg, base: b, head: h, base_dir, temp })
}

/// The arena's main branch as seen from this clone: the remote whose URL is
/// the repository named in the checkout's own configuration (in a fork's
/// clone that is `upstream`, not `origin`). The rules themselves are still
/// read from that base, never from the working tree.
fn arena_ref(head: &Path) -> String {
    let fallback = "origin/main".to_string();
    let Ok(cfg) = Config::load(&head.join("hillclimb.toml")) else { return fallback };
    let Ok(out) = Command::new("git").arg("-C").arg(head).args(["remote", "-v"]).output() else {
        return fallback;
    };
    let wanted = cfg.project.repo.to_lowercase();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut parts = line.split_whitespace();
        let (Some(name), Some(url)) = (parts.next(), parts.next()) else { continue };
        let url = url.to_lowercase();
        let url = url.trim_end_matches('/').trim_end_matches(".git");
        if url.ends_with(&format!("/{wanted}")) || url.ends_with(&format!(":{wanted}")) {
            return format!("{name}/{}", cfg.project.branch);
        }
    }
    fallback
}

fn tracked_files(repo: &Path) -> std::collections::BTreeSet<String> {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["ls-files", "-z"])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split('\0')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn finalize(
    static_result: &Path,
    compiles: Outcome,
    same_theorems: Outcome,
    build_seconds: Option<u64>,
    out: &Path,
) -> Result<()> {
    let mut r: VerifyResult = serde_json::from_str(&std::fs::read_to_string(static_result)?)?;
    let static_ok = r.gates.iter().all(|g| g.status == GateStatus::Pass);
    let gate = |key: &str, o: Outcome, fail: &str| match o {
        Outcome::Pass => Gate::new(key, GateStatus::Pass, ""),
        Outcome::Fail => Gate::new(key, GateStatus::Fail, fail),
        Outcome::Skip => Gate::new(
            key,
            GateStatus::Skip,
            if static_ok { "Not run because the build failed" } else { "Not run because a static gate failed" },
        ),
    };
    // A build that passed without a checker verdict is not a pass.
    let same_theorems = match (compiles, same_theorems) {
        (Outcome::Pass, Outcome::Skip) => Outcome::Fail,
        (_, s) => s,
    };
    r.gates.retain(|g| g.key != "compiles" && g.key != "same_theorems");
    r.gates.push(gate("compiles", compiles, "The build failed or left an unfinished proof; see the verification run"));
    r.gates.push(gate("same_theorems", same_theorems, "The independent checker did not accept the proof; see the verification run"));
    r.metrics.build_seconds = build_seconds;
    std::fs::write(out, serde_json::to_string_pretty(&r)?)?;
    Ok(())
}

fn read_jsonl<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    let Ok(text) = std::fs::read_to_string(path) else { return Ok(Vec::new()) };
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).with_context(|| format!("parsing a line of {}", path.display())))
        .collect()
}

fn gate(root: &Path, data: &Path, dry_run: bool) -> Result<()> {
    let cfg = Config::load(&root.join("hillclimb.toml"))?;
    let now = Utc::now();
    std::fs::create_dir_all(data)?;
    let state_path = data.join("state.json");
    let ledger_path = data.join("ledger.json");
    let attempts_path = data.join("attempts.jsonl");

    let previous: Option<State> = std::fs::read_to_string(&state_path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok());
    let mut ledger: Ledger = std::fs::read_to_string(&ledger_path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let mut attempts: Vec<Attempt> = read_jsonl(&attempts_path)?;
    let roles_path = data.join("roles.json");
    let mut roles: RolesState = std::fs::read_to_string(&roles_path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();

    let measured = eval::measure(root, &cfg)?;
    let mut forge = gh::GhForge::new(&cfg.project.repo, &cfg.project.branch, &cfg.build.workflow, dry_run);
    let head_before = hc_core::gate::Forge::main_head(&mut forge)?;

    let out = Gatekeeper { cfg: &cfg, forge: &mut forge, now }.run(&mut ledger, &mut roles, &attempts)?;
    for line in &out.log {
        println!("{line}");
    }
    attempts.extend(out.decided.iter().cloned());
    if out.current.is_some() {
        // Rebuild main so later pull requests build incrementally against it.
        if let Err(e) = hc_core::gate::Forge::dispatch_verify(&mut forge, 0, &[]) {
            eprintln!("could not dispatch a build of main: {e:#}");
        }
        // Refresh the site so its link-preview image shows the new score.
        if !dry_run {
            let _ = Command::new("gh")
                .args(["workflow", "run", &cfg.build.pages_workflow, "-R", &cfg.project.repo, "--ref", &cfg.project.branch])
                .status();
        }
    }

    // Where main is now. Without a merge this is what we just measured.
    let last_build = previous.as_ref().and_then(|p| p.current.metrics.build_seconds);
    let current = match out.current {
        Some(p) => p,
        None => {
            let mut metrics = measured.metrics;
            metrics.build_seconds = last_build;
            let at = previous
                .as_ref()
                .filter(|p| p.current.sha == head_before)
                .map(|p| p.current.at)
                .unwrap_or(now);
            Point { sha: head_before, at, metrics }
        }
    };
    let baseline = previous.as_ref().map(|p| p.baseline.clone()).unwrap_or_else(|| current.clone());

    let mut board: BTreeMap<String, Leader> = BTreeMap::new();
    for a in attempts.iter().chain(out.open.iter()) {
        let l = board.entry(a.author.clone()).or_insert_with(|| Leader {
            login: a.author.clone(),
            merged: 0,
            attempts: 0,
            saved: 0,
        });
        l.attempts += 1;
        if a.status == AttemptStatus::Merged {
            l.merged += 1;
            l.saved += a.delta.map(|d| (-d).max(0) as u64).unwrap_or(0);
        }
    }
    let mut leaderboard: Vec<Leader> = board.into_values().collect();
    leaderboard.sort_by(|a, b| b.saved.cmp(&a.saved).then(b.merged.cmp(&a.merged)).then(a.login.cmp(&b.login)));

    let state = State {
        project: ProjectInfo {
            slug: cfg.project.slug.clone(),
            title: cfg.project.title.clone(),
            description: cfg.project.description.clone(),
            repo: cfg.project.repo.clone(),
            dir: cfg.project.dir.clone(),
            upstream: cfg.project.upstream.clone(),
            primary: cfg.score.primary.clone(),
            metrics: metric_defs(),
        },
        updated_at: now,
        baseline,
        current,
        counts: Counts {
            merged: attempts.iter().filter(|a| a.status == AttemptStatus::Merged).count() as u64,
            rejected: attempts.iter().filter(|a| a.status == AttemptStatus::Rejected).count() as u64,
            open: out.open.len() as u64,
            contributors: leaderboard.len() as u64,
            ideas_open: out.ideas.iter().filter(|i| i.status == IdeaStatus::Open).count() as u64,
        },
        leaderboard,
        ideas: out.ideas,
        open: out.open,
        top_files: measured.files.iter().take(30).cloned().collect(),
    };

    if dry_run {
        println!("{}", serde_json::to_string_pretty(&state)?);
        return Ok(());
    }
    std::fs::write(&state_path, serde_json::to_string_pretty(&state)?)?;
    std::fs::write(&ledger_path, serde_json::to_string_pretty(&ledger)?)?;
    roles.members.sort_by(|a, b| a.role.cmp(&b.role).then(a.since.cmp(&b.since)));
    std::fs::write(&roles_path, serde_json::to_string_pretty(&roles)?)?;
    let lines: Vec<String> = attempts.iter().map(serde_json::to_string).collect::<Result<_, _>>()?;
    std::fs::write(&attempts_path, lines.join("\n") + if lines.is_empty() { "" } else { "\n" })?;
    Ok(())
}
