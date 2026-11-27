mod analyze;
mod analyze_flat;
mod analyze_more;
mod analyze_seams;
mod analyze_terms;
mod analyze_weight;
mod gh;
mod landing;
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
        /// For a change built on another open pull request (`Depends: #n`):
        /// a git ref of that pull request's head, for example its branch or
        /// `pull/<n>/head` after fetching it. The gates are then evaluated
        /// against it, as the gatekeeper does, instead of against main.
        #[arg(long, conflicts_with = "base")]
        depends_ref: Option<String>,
        /// Print the files that claimed ideas reserve (read with `gh`) and
        /// warn if this change touches any of them.
        #[arg(long)]
        reservations: bool,
        /// Judge the score with this minimum drop instead of the configured
        /// one (the verify workflow passes it when a leader waived the minimum).
        #[arg(long)]
        min_delta: Option<u64>,
        /// Git tree main will have when the stack and then this change land
        /// as squashed commits, or `conflict` (set by the verify workflow).
        #[arg(long)]
        landing_tree: Option<String>,
    },
    /// Print the open pull requests a pull request is built on (its
    /// `Depends` chain), the first to merge first, separated by spaces. The
    /// verify workflow applies them before the change when it is given no stack.
    Stack {
        #[arg(long)]
        pr: u64,
        #[arg(long, default_value = "hillclimb.toml")]
        config: PathBuf,
    },
    /// Print the git tree main will have once the stack and then the change
    /// have landed as squashed commits, or `conflict` (used by the verify
    /// workflow; a stacked result is merged from only if this is the tree
    /// that was built).
    Landing {
        /// A clone that has every commit named.
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        /// Commit of main the verification starts from.
        #[arg(long)]
        base: String,
        /// Head commits of the changes applied first, in order (repeatable).
        #[arg(long)]
        stack: Vec<String>,
        /// The git tree the verification applied the change to (main with the
        /// stack applied as real merges). The squashed stack must give it.
        #[arg(long)]
        stacked_tree: String,
        /// Head commit of the change.
        #[arg(long)]
        head: String,
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
    /// Print one role's instructions (contributor, reviewer, ideator, leader
    /// or coordinator), as they read under the arena's settings.
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
    /// checkout has no `.lake/build` and warns when lake would rebuild what
    /// the timed files import (lake is asked, and it goes by content, not by
    /// file times). A file that does not compile cannot be timed and makes
    /// the exit code non-zero; being over the limit does not (timings are
    /// noisy, and no gate enforces the limit).
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
        /// For a change built on another open pull request (`Depends: #n`):
        /// a git ref of that pull request's head; everything is then checked
        /// against it instead of against main, as for `hc eval`.
        #[arg(long, conflicts_with = "base")]
        depends_ref: Option<String>,
    },
}

#[derive(Subcommand)]
enum AnalyzeTool {
    /// Declarations of the project that the final theorems do not reach, with a token estimate.
    Reach(AnalyzeArgs),
    /// Unused structure fields, conjuncts of statements and hypotheses, and binders.
    Unused(AnalyzeArgs),
    /// Reached theorems and definitions with equal or nearly equal statements (or bodies).
    Twins(TwinsArgs),
    /// Steps of tactic blocks after which one closing tactic proves all remaining goals.
    /// Hits are upper bounds: about half of such tokens were realized in the first arena, some
    /// hits fail once the text is replaced, and compile time is not measured (use `hc time`).
    Tails(TailsArgs),
    /// Single-caller lemmas and their trees, arguments that are the same at every call, and
    /// facts proved in several declarations: boundaries a flatter proof could remove.
    Flat(FlatArgs),
    /// Dominator weights: the source tokens each declaration alone holds up (what would become
    /// unreachable without it), and an outline of the proof by weight.
    Weight(WeightArgs),
    /// Kernel term sizes of the project's declarations, per declaration, module and project,
    /// beside their source tokens; `--compare` an earlier `--out` file.
    Terms(TermsArgs),
    /// What lemma boundaries hide: parts of a conclusion the one caller does not read, a
    /// conclusion restated or undone at its call, hypotheses only passed on, repeated case
    /// splits and repeated `have`s, lemmas that only dispatch, and repeated facts.
    Seams(SeamsArgs),
}

#[derive(clap::Args)]
struct SeamsArgs {
    #[command(flatten)]
    common: AnalyzeArgs,
    /// Skip subproofs with fewer expression nodes when comparing case-split skeletons.
    #[arg(long, default_value_t = 40)]
    min_size: usize,
    /// Most constants in which statements with the same skeleton may differ.
    #[arg(long, default_value_t = 2)]
    max_diff: usize,
}

#[derive(clap::Args)]
struct TermsArgs {
    #[command(flatten)]
    common: AnalyzeArgs,
    /// An earlier `--out` file to compare with.
    #[arg(long)]
    compare: Option<PathBuf>,
}

#[derive(clap::Args)]
struct WeightArgs {
    #[command(flatten)]
    common: AnalyzeArgs,
    /// Also recompute reachability without this declaration and compare with what it dominates
    /// (repeatable).
    #[arg(long = "check")]
    checks: Vec<String>,
}

#[derive(clap::Args)]
struct FlatArgs {
    #[command(flatten)]
    common: AnalyzeArgs,
    /// Skip facts whose statement or subproof has fewer expression nodes.
    #[arg(long, default_value_t = 12)]
    min_size: usize,
    /// Report constant arguments only for theorems applied at least this often.
    #[arg(long, default_value_t = 2)]
    min_calls: usize,
}

#[derive(clap::Args)]
struct TwinsArgs {
    #[command(flatten)]
    common: AnalyzeArgs,
    /// Most distinct pairs of constants or literals in which near twins may differ.
    #[arg(long, default_value_t = 2)]
    max_diff: usize,
    /// Skip statements smaller than this many expression nodes.
    #[arg(long, default_value_t = 15)]
    min_size: usize,
    /// Compare near twins across modules and namespaces too.
    #[arg(long)]
    everywhere: bool,
}

#[derive(clap::Args)]
struct TailsArgs {
    /// Files to analyze (relative to the project directory, the repository or here).
    files: Vec<String>,
    /// Analyze every counted file.
    #[arg(long)]
    all: bool,
    #[arg(long)]
    root: Option<PathBuf>,
    #[arg(long)]
    config: Option<PathBuf>,
    /// Candidate tactic (repeatable); replaces the default list.
    #[arg(long = "tactic")]
    tactics: Vec<String>,
    /// Heartbeats per try, in thousands (as `maxHeartbeats`).
    #[arg(long, default_value_t = 20000)]
    heartbeats: u64,
    /// Minimum tokens saved per place. Defaults to `conduct.search_tactics.min_saving`.
    #[arg(long)]
    min_saving: Option<u64>,
    /// Append the records of each file here as it finishes.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Skip files already recorded in `--out`.
    #[arg(long, requires = "out")]
    resume: bool,
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
        Cmd::Eval {
            head, base, base_ref, config, pr, head_sha, base_sha, base_tree, merged_tree, stack, author,
            author_open_prs, out, depends_ref, reservations, min_delta, landing_tree,
        } => {
            let extra = EvalExtra { depends_ref, reservations, min_delta, landing_tree };
            eval_cmd(&head, base, base_ref, config, pr, head_sha, base_sha, (base_tree, merged_tree, stack), &author, author_open_prs, out, extra)
        }
        Cmd::Landing { repo, base, stack, stacked_tree, head } => {
            println!("{}", landing::landing_tree(&repo, &base, &stack, &stacked_tree, &head)?);
            Ok(())
        }
        Cmd::Stack { pr, config } => {
            let cfg = Config::load(&config)?;
            let forge = gh::GhForge::new(&cfg.project.repo, &cfg.project.branch, &cfg.build.workflow, true);
            let bodies = forge.open_bodies()?;
            let open: BTreeMap<u64, &str> = bodies.iter().map(|(n, b)| (*n, b.as_str())).collect();
            // Nothing is known here about what has merged: a line that names
            // a pull request which is not open gives no stack, and the
            // gatekeeper decides what that means.
            let dep = match open.get(&pr) {
                Some(body) => hc_core::gate::dependency(pr, body, &open, &Default::default(), &cfg.project.branch),
                None => Default::default(),
            };
            if let Some(problem) = &dep.problem {
                eprintln!("{problem}");
            }
            println!("{}", dep.chain.iter().map(u64::to_string).collect::<Vec<_>>().join(" "));
            Ok(())
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
                AnalyzeTool::Twins(t) => {
                    let a = t.common;
                    return analyze_more::twins(analyze_more::TwinsOptions {
                        root: a.root,
                        config: a.config,
                        imports: a.imports,
                        theorems: a.theorems,
                        out: a.out,
                        top: a.top,
                        max_diff: t.max_diff,
                        min_size: t.min_size,
                        everywhere: t.everywhere,
                    });
                }
                AnalyzeTool::Seams(t) => {
                    let a = t.common;
                    return analyze_seams::seams(analyze_seams::SeamsOptions {
                        root: a.root,
                        config: a.config,
                        imports: a.imports,
                        theorems: a.theorems,
                        out: a.out,
                        top: a.top,
                        min_size: t.min_size,
                        max_diff: t.max_diff,
                    });
                }
                AnalyzeTool::Terms(t) => {
                    let a = t.common;
                    return analyze_terms::terms(analyze_terms::TermsOptions {
                        root: a.root,
                        config: a.config,
                        imports: a.imports,
                        theorems: a.theorems,
                        out: a.out,
                        top: a.top,
                        compare: t.compare,
                    });
                }
                AnalyzeTool::Weight(t) => {
                    let a = t.common;
                    return analyze_weight::weight(analyze_weight::WeightOptions {
                        root: a.root,
                        config: a.config,
                        imports: a.imports,
                        theorems: a.theorems,
                        out: a.out,
                        top: a.top,
                        checks: t.checks,
                    });
                }
                AnalyzeTool::Flat(t) => {
                    let a = t.common;
                    return analyze_flat::flat(analyze_flat::FlatOptions {
                        root: a.root,
                        config: a.config,
                        imports: a.imports,
                        theorems: a.theorems,
                        out: a.out,
                        top: a.top,
                        min_size: t.min_size,
                        min_calls: t.min_calls,
                    });
                }
                AnalyzeTool::Tails(t) => {
                    return analyze_more::tails(analyze_more::TailsOptions {
                        root: t.root,
                        config: t.config,
                        files: t.files,
                        all: t.all,
                        tactics: t.tactics,
                        heartbeats: t.heartbeats,
                        min_saving: t.min_saving,
                        out: t.out,
                        resume: t.resume,
                    });
                }
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
            let c = compare(&head, None, base_ref, config, None, None)?;
            timing::run(&head, &c, &timing::Options { files, base, runs, command, markdown })
        }
        Cmd::Precheck { head, base, base_ref, config, offline, title, body_file, depends_ref } => {
            let on_dependency = depends_ref.is_some();
            let c = compare(&head, base, base_ref, config, depends_ref, None)?;
            let ok = precheck::run(&head, &c, &precheck::Options { offline, title, body_file, on_dependency })?;
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

/// Options of `hc eval` beyond the comparison itself.
struct EvalExtra {
    depends_ref: Option<String>,
    reservations: bool,
    min_delta: Option<u64>,
    landing_tree: Option<String>,
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
    extra: EvalExtra,
) -> Result<()> {
    let c = compare(head, base, base_ref, config, extra.depends_ref, extra.min_delta)?;
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
    let changed_paths = eval::changed_paths(b, h);
    let changed = changed_paths.len();
    let ok = report(&c, &gates);

    if extra.reservations {
        // Read from the arena's issues: an idea that is claimed is assigned
        // to its claimant, and lists what it reserves in its description.
        let forge = gh::GhForge::new(&cfg.project.repo, &cfg.project.branch, &cfg.build.workflow, true);
        match forge.reservations(&cfg.ideas.label) {
            Ok(list) if list.is_empty() => println!("\nNo files are reserved by a claimed idea."),
            Ok(list) => {
                println!("\nFiles reserved by claimed ideas:");
                for r in &list {
                    println!("  idea #{} (claimed by @{}): {}", r.idea, r.claimant, r.patterns.join(", "));
                }
                for r in &list {
                    let hits = hc_core::gate::reserved_hits(&cfg.dir_prefix(), &r.patterns, &changed_paths);
                    if !hits.is_empty() {
                        println!(
                            "\nWarning: this change {}. (Not a gate; ignore it if this is your own change for that idea.)",
                            hc_core::gate::reservation_warning(r, &hits)
                        );
                    }
                }
            }
            Err(e) => eprintln!("\nCould not read the reservations: {e:#}"),
        }
    }

    if let Some(out) = out {
        let result = VerifyResult {
            pr,
            head_sha,
            base_sha,
            base_tree: trees.0,
            merged_tree: trees.1,
            stack: trees.2,
            landing_tree: extra.landing_tree,
            min_delta: Some(cfg.score.min_delta),
            rules: STATIC_RULES,
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
///
/// `depends_ref` is for a change built on another open one (`Depends: #n`):
/// a ref of that one's head, which is then the base, as it is for the
/// gatekeeper. `min_delta` replaces the configured minimum step.
fn compare(
    head: &Path,
    base: Option<PathBuf>,
    base_ref: Option<String>,
    config: Option<PathBuf>,
    depends_ref: Option<String>,
    min_delta: Option<u64>,
) -> Result<Compared> {
    let (base_dir, temp) = match base {
        Some(b) => (b, false),
        None => {
            // A change built on another open one is measured against that
            // one, as the gatekeeper measures it: its own difference only.
            if let Some(dep) = &depends_ref {
                println!("This change is built on another one ({dep}); the gates are evaluated against that, not against main.");
            }
            let reference = depends_ref.or(base_ref).unwrap_or_else(|| arena_ref(head));
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
    let mut cfg = Config::load(&config.unwrap_or_else(|| base_dir.join("hillclimb.toml")))?;
    if let Some(min) = min_delta {
        cfg.score.min_delta = min;
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn command_line_is_well_formed() {
        Cli::command().debug_assert();
        // What the verify workflow passes.
        let cli = Cli::try_parse_from([
            "hc", "eval", "--head", "head", "--base", "stacked", "--config", "base/hillclimb.toml", "--pr", "12",
            "--head-sha", "abc", "--base-sha", "def", "--base-tree", "t1", "--merged-tree", "t2",
            "--landing-tree", "conflict", "--stack", "7@aaa", "--min-delta", "1", "--author", "x",
            "--author-open-prs", "2", "--out", "static.json",
        ])
        .unwrap();
        match cli.cmd {
            Cmd::Eval { landing_tree, min_delta, stack, .. } => {
                assert_eq!(landing_tree.as_deref(), Some("conflict"));
                assert_eq!(min_delta, Some(1));
                assert_eq!(stack, ["7@aaa"]);
            }
            _ => panic!("not eval"),
        }
        assert!(Cli::try_parse_from(["hc", "eval", "--depends-ref", "origin/first", "--reservations"]).is_ok());
        assert!(Cli::try_parse_from(["hc", "eval", "--base", "dir", "--depends-ref", "origin/first"]).is_err());
        assert!(Cli::try_parse_from(["hc", "stack", "--pr", "12", "--config", "base/hillclimb.toml"]).is_ok());
        assert!(Cli::try_parse_from([
            "hc", "landing", "--repo", "base", "--base", "m", "--stack", "a", "--stack", "b", "--stacked-tree", "t", "--head", "h"
        ])
        .is_ok());
    }

    #[test]
    fn a_data_branch_tip_the_gatekeeper_did_not_write_is_seen() {
        let bot = "github-actions[bot]";
        assert!(data_tip_is_ours(bot, bot, bot));
        // A pull request merged into the data branch through the forge: the
        // author is the pull request's, the committer the forge.
        assert!(!data_tip_is_ours("mallory", "GitHub", bot));
        assert!(!data_tip_is_ours(bot, "GitHub", bot));
        assert!(!data_tip_is_ours("mallory", bot, bot));

        let dir = std::env::temp_dir().join(format!("hc-data-tip-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // No checkout, or one without a commit: nothing to compare.
        assert_eq!(foreign_data_tip(&dir, bot), None);
        let git = |args: &[&str]| {
            let ok = Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap().status.success();
            assert!(ok, "git {args:?}");
        };
        git(&["init", "--quiet"]);
        assert_eq!(foreign_data_tip(&dir, bot), None);
        let commit = |name: &str, file: &str| {
            std::fs::write(dir.join(file), name).unwrap();
            git(&["add", "-A"]);
            git(&["-c", &format!("user.name={name}"), "-c", "user.email=x@example.invalid", "commit", "--quiet", "-m", "x"]);
        };
        commit(bot, "state.json");
        assert_eq!(foreign_data_tip(&dir, bot), None);
        commit("mallory", "roles.json");
        let why = foreign_data_tip(&dir, bot).unwrap();
        assert!(why.contains("written by `mallory`") && why.contains("not by the gatekeeper"), "{why}");
        // Under another identity the gatekeeper names itself.
        assert_eq!(foreign_data_tip(&dir, "mallory"), None);

        // `hc gate` itself stops there, before it reads or changes anything:
        // no roles, no ledger, no forge. (With the check taken out of
        // `gate`, this pass would go on to ask the forge about `o/r`.)
        let root = dir.join("root");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("hillclimb.toml"), "[project]\nslug = \"t\"\ntitle = \"T\"\nrepo = \"o/r\"\ndir = \"proj\"\n").unwrap();
        let before = std::fs::read_dir(&dir).unwrap().count();
        let err = gate(&root, &dir, true).unwrap_err().to_string();
        assert!(err.contains("the data branch is at commit") && err.contains("nothing was done"), "{err}");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), before, "nothing was written beside the data");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The workflow that runs `hc gate` must fail when it fails, and must
    /// not publish on top of a commit that is not its own. Both were wrong
    /// once: the step piped into `tee` without `pipefail`.
    #[test]
    fn the_workflows_do_not_swallow_a_failed_step() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut seen = 0;
        for dir in [".github/workflows", "arena/.github/workflows"] {
            for entry in std::fs::read_dir(root.join(dir)).unwrap() {
                let path = entry.unwrap().path();
                let text = std::fs::read_to_string(&path).unwrap();
                // Each `run:` script, up to the next step.
                for script in text.split("\n      - ").filter(|s| s.contains("run:")) {
                    let body = &script[script.find("run:").unwrap()..];
                    if body.contains("| tee") {
                        seen += 1;
                        // Either the shell fails the pipeline, or the step reads the first command's status itself.
                        assert!(
                            body.contains("pipefail") || body.contains("PIPESTATUS[0]"),
                            "{}: a step pipes into tee and does not see the first command fail:\n{body}",
                            path.display()
                        );
                    }
                }
            }
        }
        assert!(seen >= 4, "the steps that pipe into tee were found ({seen})");
        let gatekeeper = std::fs::read_to_string(root.join(".github/workflows/gatekeeper.yml")).unwrap();
        let publish = &gatekeeper[gatekeeper.find("Publish dashboard data").unwrap()..];
        let (check, commit) = (publish.find("Nothing is published").unwrap(), publish.find("commit --quiet").unwrap());
        assert!(check < commit, "the tip is checked before anything is committed");
        // It commits under the one name `hc gate` expects when nothing else is set.
        assert!(publish.contains("BOT: github-actions[bot]") && publish.contains("user.name=\"$BOT\""));
    }

    /// The result an evaluation writes records the minimum it was judged
    /// with and the tree as it will land, and has the gate for counts.
    #[test]
    fn a_result_records_the_minimum_it_was_judged_with() {
        let dir = std::env::temp_dir().join(format!("hc-main-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let config = "[project]\nslug = \"t\"\ntitle = \"T\"\nrepo = \"o/r\"\ndir = \"proj\"\n[score]\nprimary = \"tokens\"\nmin_delta = 300\nguards = []\nmax_line_length = 100\nmax_build_ratio = 1.3\n";
        for (side, source) in [("base", "theorem a : True := by\n  trivial\n"), ("head", "theorem a : True := trivial\n")] {
            std::fs::create_dir_all(dir.join(side).join("proj")).unwrap();
            std::fs::write(dir.join(side).join("proj/A.lean"), source).unwrap();
            std::fs::write(dir.join(side).join("hillclimb.toml"), config).unwrap();
        }
        let run = |min_delta: Option<u64>| {
            let out = dir.join("static.json");
            let extra = EvalExtra { depends_ref: None, reservations: false, min_delta, landing_tree: Some("t2".into()) };
            eval_cmd(
                &dir.join("head"), Some(dir.join("base")), None, None, 1, "h".into(), "b".into(),
                (Some("t1".into()), Some("t2".into()), vec![]), "", 0, Some(out.clone()), extra,
            )
            .unwrap();
            serde_json::from_str::<VerifyResult>(&std::fs::read_to_string(out).unwrap()).unwrap()
        };
        let strict = run(None);
        assert_eq!(strict.min_delta, Some(300));
        assert_eq!(strict.gate("improves").unwrap().status, GateStatus::Fail);
        assert_eq!(strict.landing_tree.as_deref(), Some("t2"));
        assert!(strict.gate("no_increase").is_some());
        assert_eq!(strict.rules, STATIC_RULES);
        for key in STATIC_GATES_2 {
            assert_eq!(strict.gate(key).unwrap().status, GateStatus::Pass, "{key}");
        }
        let waived = run(Some(1));
        assert_eq!(waived.min_delta, Some(1));
        assert_eq!(waived.gate("improves").unwrap().status, GateStatus::Pass);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

fn read_jsonl<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    let Ok(text) = std::fs::read_to_string(path) else { return Ok(Vec::new()) };
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).with_context(|| format!("parsing a line of {}", path.display())))
        .collect()
}

/// The author and committer of the commit a data checkout is at, or `None`
/// if it is not a git checkout or has no commit yet.
fn data_tip(data: &Path) -> Option<(String, String, String)> {
    if !data.join(".git").exists() {
        return None;
    }
    let out = Command::new("git").arg("-C").arg(data).args(["log", "-1", "--format=%H%x00%an%x00%cn"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).trim_end().to_string();
    let mut parts = text.split('\0').map(str::to_string);
    Some((parts.next()?, parts.next()?, parts.next()?))
}

/// Whether the commit at the tip of the data branch is one the gatekeeper's
/// own publishing step wrote: authored and committed under the gatekeeper's
/// identity. A commit that reached the branch another way (a pull request
/// pointed at the data branch and merged, a push by hand) has another
/// author or another committer: a merge made through the forge is committed
/// by the forge, whoever asked for it.
fn data_tip_is_ours(author: &str, committer: &str, bot: &str) -> bool {
    author.eq_ignore_ascii_case(bot) && committer.eq_ignore_ascii_case(bot)
}

/// What is wrong with the tip of the data branch, if it is not ours.
fn foreign_data_tip(data: &Path, bot: &str) -> Option<String> {
    let (sha, author, committer) = data_tip(data)?;
    (!data_tip_is_ours(&author, &committer, bot)).then(|| {
        format!(
            "the data branch is at commit {sha}, written by `{author}` and committed by `{committer}`, not by the gatekeeper (`{bot}`)"
        )
    })
}

fn gate(root: &Path, data: &Path, dry_run: bool) -> Result<()> {
    let cfg = Config::load(&root.join("hillclimb.toml"))?;
    let now = Utc::now();
    std::fs::create_dir_all(data)?;
    if cfg.review_unstated {
        println!(
            "Notice: hillclimb.toml has no [review] section, so approvals are required as before (required = {}, required_large = {}). To merge on the gates alone, set `[review] required = 0` and `required_large = 0`.",
            cfg.review.required, cfg.review.required_large
        );
    }
    // The data branch holds the roles, the ledger and the record of what was
    // decided: everything the gatekeeper believes from one pass to the next.
    // If its tip is a commit the gatekeeper did not write, nothing in it is
    // believed: no pass is made, nothing merges and nothing is published on
    // top of it, until a leader has put the branch back.
    if let Some(why) = foreign_data_tip(data, &gh::bot_login()) {
        let title = "Alert: the data branch holds a commit the gatekeeper did not write";
        if !dry_run {
            let mut forge = gh::GhForge::new(&cfg.project.repo, &cfg.project.branch, &cfg.build.workflow, dry_run);
            let raised = hc_core::gate::Forge::issues(&mut forge)
                .map(|all| all.iter().any(|i| i.open && i.title == title))
                .unwrap_or(false);
            if !raised {
                let _ = hc_core::gate::Forge::create_issue(
                    &mut forge,
                    title,
                    &format!("Before a pass, {why}. The gatekeeper keeps its roles, its ledger and its record on that branch, so it has stopped: no pass is made and nothing merges until the branch is put back. Leaders: look at that commit and at how it got there (a pull request pointed at the data branch, a push by hand), reset the branch to the last commit by `{}` (`git log --format='%H %an %cn'`), and close this issue. If the gatekeeper runs outside the reusable workflow, under another login, `HC_BOT_LOGIN` names it, and its data must be committed under that name.", gh::bot_login()),
                    hc_core::gate::LABEL_ALERT,
                );
            }
        }
        bail!("{why}; nothing was done");
    }
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
