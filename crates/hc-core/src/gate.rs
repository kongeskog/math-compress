//! The gatekeeper: one reconcile pass over every open pull request, idea,
//! application and command.
//!
//! It is written against the [`Forge`] trait so the whole state machine can be
//! tested without a network.
//!
//! Trust model. Anyone may open a pull request. Two things are earned:
//! reviewing (an approval counts only from a vetted reviewer or a leader) and
//! putting ideas straight into the queue (ideas from others wait for triage).
//! Roles are granted by leaders, either directly or by endorsing an
//! application that meets the experience threshold.

use crate::config::Config;
use crate::model::*;
use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use globset::{Glob, GlobSetBuilder};
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};

pub const COMMENT_MARKER: &str = "<!-- hillclimb-gate -->";
pub const STATUS_CONTEXT: &str = "hillclimb/gate";
pub const LABEL_REJECTED: &str = "hillclimb:rejected";
/// A gate failed that the author can fix; nothing was rejected.
pub const LABEL_NEEDS_FIX: &str = "hillclimb:needs-fix";
pub const LABEL_READY: &str = "hillclimb:needs-review";
/// The drop a change must still make when a leader waived the minimum step.
pub const WAIVED_MIN: u64 = 1;
pub const LABEL_PROPOSED: &str = "idea:proposed";
pub const LABEL_APPLICATION: &str = "application";
pub const LABEL_ALERT: &str = "hillclimb:alert";
/// How many approved changes may be verified stacked on one another.
pub const MAX_STACK: usize = 10;

/// What a change would be verified on: main and the approved changes ahead.
struct Line<'a> {
    main: &'a str,
    main_tree: &'a str,
    ahead: &'a [u64],
    /// Changes with a verification queued or running.
    active: &'a BTreeSet<u64>,
    /// `pr@head` of merged changes -> the merge commit.
    merged: &'a BTreeMap<String, String>,
}
/// How long after its dispatch a verification is assumed to be running even
/// if no run is seen: longer than a build takes, shorter than a hung one.
pub const RUN_GRACE_MINUTES: i64 = 40;
/// Gates the verify workflow must report as passed.
pub const REQUIRED_RESULT_GATES: &[&str] = &["frozen", "scope", "deny", "improves", "compiles", "same_theorems"];
/// Words an idea's description must contain (its section headings).
pub const IDEA_SECTIONS: &[&str] = &["target", "technique", "check", "estimate"];

#[derive(Debug, Clone)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub body: String,
    pub head_sha: String,
    pub base_ref: String,
    pub created_at: DateTime<Utc>,
    pub url: String,
    pub draft: bool,
    /// `Some(false)` when GitHub reports a merge conflict.
    pub mergeable: Option<bool>,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ReviewInfo {
    pub user: String,
    /// `APPROVED`, `CHANGES_REQUESTED`, `COMMENTED` or `DISMISSED`.
    pub state: String,
    pub commit_id: String,
    pub submitted_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default)]
pub struct IssueInfo {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub author: String,
    pub open: bool,
    pub labels: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub url: String,
}

#[derive(Debug, Clone)]
pub struct CommentInfo {
    pub issue: u64,
    pub user: String,
    pub body: String,
    pub created_at: DateTime<Utc>,
    /// Whether the comment was changed after it was posted. People with
    /// write access can edit other people's comments, so an edited comment
    /// is not counted as an approval or a waiver.
    pub edited: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitState {
    Success,
    Pending,
    Failure,
}

pub trait Forge {
    fn main_head(&mut self) -> Result<String>;
    /// Git tree of the main branch's head commit.
    fn main_tree(&mut self) -> Result<String>;
    /// Whether the latest finished build of main itself, at this commit,
    /// verified. `None` when there is no such build yet.
    fn main_verifies(&mut self, main: &str) -> Result<MainBuild>;
    fn open_prs(&mut self) -> Result<Vec<PullRequest>>;
    /// Recently merged pull requests with their merge commit and time.
    fn merged_prs(&mut self) -> Result<Vec<(PullRequest, String, DateTime<Utc>)>>;
    /// Results for this head produced by the trusted verify workflow, newest first.
    fn verify_results(&mut self, pr: u64, head_sha: &str) -> Result<Vec<VerifyResult>>;
    /// Pull requests that have a verify run queued or in progress.
    fn active_verifies(&mut self) -> Result<BTreeSet<u64>>;
    /// The subset of those whose run started after main last changed, so its
    /// result will be about today's main. A forge that cannot tell says all.
    fn current_verifies(&mut self) -> Result<BTreeSet<u64>> {
        self.active_verifies()
    }
    /// Verify `pr` on top of main with the pull requests in `stack` applied
    /// first, in order. `pr == 0` builds main itself.
    fn dispatch_verify(&mut self, pr: u64, stack: &[u64]) -> Result<()>;
    /// As `dispatch_verify`, but the `improves` gate is judged with this
    /// minimum step instead of the configured one (a leader waived it for
    /// the commit). The result records the minimum it was judged with.
    fn dispatch_verify_min(&mut self, pr: u64, stack: &[u64], min_delta: u64) -> Result<()>;
    /// Paths a pull request changes, relative to the repository root.
    fn changed_files(&mut self, pr: u64) -> Result<Vec<String>>;
    /// Paths that differ between two commits, `head` compared with the point
    /// where it left `base`. For a change built on another one this is its
    /// own difference, without the other's files.
    fn changed_between(&mut self, base: &str, head: &str) -> Result<Vec<String>>;
    /// Whether commit `ancestor` is `head` itself or one of its ancestors.
    fn contains(&mut self, head: &str, ancestor: &str) -> Result<bool>;
    /// Stop the verifications of these changes that are queued or running.
    fn cancel_verifies(&mut self, prs: &BTreeSet<u64>) -> Result<()>;
    fn reviews(&mut self, pr: u64) -> Result<Vec<ReviewInfo>>;
    /// Every comment on a pull request written by a person.
    fn pr_comments(&mut self, pr: u64) -> Result<Vec<CommentInfo>>;
    fn account_created(&mut self, login: &str) -> Result<DateTime<Utc>>;
    fn set_status(&mut self, sha: &str, state: CommitState, description: &str) -> Result<()>;
    /// Create or update the single gate comment on a pull request.
    fn upsert_comment(&mut self, pr: u64, body: &str) -> Result<DateTime<Utc>>;
    fn set_label(&mut self, issue: u64, label: &str, present: bool) -> Result<()>;
    /// Squash-merge; returns the new commit on the main branch.
    fn merge(&mut self, pr: &PullRequest, message: &str) -> Result<String>;
    /// Every issue (not pull request), open or closed.
    fn issues(&mut self) -> Result<Vec<IssueInfo>>;
    /// Comments by people on any issue or pull request since a time.
    fn comments_since(&mut self, since: Option<DateTime<Utc>>) -> Result<Vec<CommentInfo>>;
    fn assign(&mut self, issue: u64, login: &str, assigned: bool) -> Result<()>;
    fn comment(&mut self, issue: u64, body: &str) -> Result<()>;
    /// Returns the new issue's number and URL.
    fn create_issue(&mut self, title: &str, body: &str, label: &str) -> Result<(u64, String)>;
    fn close_issue(&mut self, issue: u64) -> Result<()>;
}

/// What the builds of main at its current commit say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MainBuild {
    /// A build and the independent check passed.
    Passed,
    /// Main's own content failed to build or to pass the check.
    Failed,
    /// No build has finished yet.
    Pending,
    /// This many builds ended without judging main (a download, the runner,
    /// the checker's setup), and none has judged it.
    Errored(u32),
}

/// How many builds of main may end without a verdict before an alert is raised.
pub const MAIN_BUILD_ATTEMPTS: u32 = 3;

/// What a reconcile pass produced; the caller persists it.
#[derive(Debug, Clone, Default)]
pub struct Outcome {
    /// Newly decided attempts (merged or rejected) to append to the log.
    pub decided: Vec<Attempt>,
    pub open: Vec<Attempt>,
    pub ideas: Vec<Idea>,
    /// Set when at least one merge happened: the new main head and metrics.
    pub current: Option<Point>,
    pub log: Vec<String>,
}

pub struct Gatekeeper<'a, F: Forge> {
    pub cfg: &'a Config,
    pub forge: &'a mut F,
    pub now: DateTime<Utc>,
}

/// The idea a pull request implements: `Idea: #12` (or `Closes #12`).
pub fn idea_ref(body: &str) -> Option<u64> {
    let re = Regex::new(r"(?im)^\s*idea:\s*#(\d+)|\b(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?)\s+#(\d+)")
        .expect("static regex");
    let c = re.captures(body)?;
    c.get(1).or(c.get(2))?.as_str().parse().ok()
}

/// Whether the pull request says it finishes its idea: `Idea: #12 (complete)`
/// (or `Closes #12`). A plain `Idea: #12` is one part of the idea.
pub fn idea_complete(body: &str) -> bool {
    Regex::new(r"(?im)^\s*idea:\s*#\d+\s*\(?\s*completes?\b|\b(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?)\s+#\d+")
        .expect("static regex")
        .is_match(body)
}

fn names_an_idea(body: &str) -> bool {
    Regex::new(r"(?im)^\s*idea:\s*(#\d+|none)\s*$").expect("static regex").is_match(body)
        || idea_ref(body).is_some()
}

/// The format gate: the title follows the standard and the description names
/// an idea. `depends` is what is wrong with the description's `Depends` line,
/// if anything: the gatekeeper resolves the line against the open pull
/// requests; without them only its form can be checked (see [`depends_ref`]).
/// Also returns the parsed title (technique, scope, summary).
pub fn format_check(
    format: &crate::config::Format,
    title: &str,
    body: &str,
    depends: Option<&str>,
) -> (Gate, Option<(String, String, String)>) {
    let parsed = format.parse_title(title);
    let mut problems = Vec::new();
    if parsed.is_none() {
        problems.push(format!("The title must be {}", format.title_help()));
    }
    if !names_an_idea(body) {
        problems.push(
            "The description must contain a line `Idea: #<number>` (one part of that idea), `Idea: #<number> (complete)` (this finishes it) or `Idea: none`"
                .to_string(),
        );
    }
    let editable = !problems.is_empty();
    if let Some(problem) = depends {
        problems.push(problem.to_string());
    }
    let gate = if problems.is_empty() {
        Gate::new("format", GateStatus::Pass, "")
    } else {
        Gate::new(
            "format",
            GateStatus::Fail,
            format!(
                "{}.{}",
                problems.join(". "),
                match (editable, depends.is_some()) {
                    (true, false) => " Edit the pull request; no new push is needed.",
                    (true, true) => " The title and the idea line are fixed by editing the pull request.",
                    (false, _) => "",
                }
            ),
        )
    };
    (gate, parsed)
}

fn key(pr: u64, sha: &str) -> String {
    format!("{pr}@{sha}")
}

/// The pull request numbers of a result's stack, in order.
fn stack_numbers(r: &VerifyResult) -> Vec<u64> {
    r.stack.iter().filter_map(|k| k.split('@').next()?.parse().ok()).collect()
}

/// Whether merging from this result lands the tree it verified. A result
/// made on main alone was computed by the same merge that lands it. A
/// stacked result was built with the changes ahead applied as real merges,
/// while they land as squashed commits; when the change's branch contains
/// commits of a change ahead of it, the two can give different trees, so the
/// workflow also computes the tree as it will land and the two must agree.
/// Results written before that tree was recorded keep the rule they were
/// made under.
fn lands_as_verified(r: &VerifyResult) -> bool {
    match &r.landing_tree {
        Some(landing) => r.merged_tree.as_ref() == Some(landing),
        None => r.stack.is_empty() || r.min_delta.is_none(),
    }
}

/// Whether `named`, the commit an approval or a waiver names, is the head
/// commit: the full hash, 40 hexadecimal characters, in either case. A
/// prefix is not enough (another commit could share it), and neither is
/// equality alone: an empty head would equal a missing argument.
fn names_commit(named: &str, head: &str) -> bool {
    named.len() == 40 && named.bytes().all(|b| b.is_ascii_hexdigit()) && named.eq_ignore_ascii_case(head)
}

fn refs(numbers: &[u64]) -> String {
    numbers.iter().map(|n| format!("#{n}")).collect::<Vec<_>>().join(", ")
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(10)]
}

/// The pull request this one is built on: a line `Depends: #12` in its
/// description (`Depends: none` is the same as no line). `Err` says what is
/// wrong with the line.
pub fn depends_ref(body: &str) -> std::result::Result<Option<u64>, String> {
    // What is inside an HTML comment is not shown on the pull request, so it
    // is not part of the description: a line hidden there says nothing. A
    // comment that is never closed hides everything after it.
    let visible = Regex::new(r"(?s)<!--.*?(?:-->|\z)").expect("static regex").replace_all(body, "\n");
    let re = Regex::new(r"(?im)^[ \t]*depends(?:[ \t]+on)?[ \t]*:(.*)$").expect("static regex");
    let mut lines = re.captures_iter(&visible);
    let Some(first) = lines.next() else { return Ok(None) };
    let usage = "The `Depends` line must be `Depends: #<number>`, naming the one open pull request this change is built on";
    if lines.next().is_some() {
        return Err(format!("{usage}; there may be only one such line"));
    }
    let value = first[1].trim();
    if value.eq_ignore_ascii_case("none") {
        return Ok(None);
    }
    match value.strip_prefix('#').and_then(|n| n.parse::<u64>().ok()) {
        Some(n) => Ok(Some(n)),
        None => Err(usage.to_string()),
    }
}

/// What a pull request is built on, worked out from its `Depends` line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dependency {
    /// The pull request the line names.
    pub direct: Option<u64>,
    /// The open pull requests this one is built on, the first to merge
    /// first: the one its line names last, before it the one that one names,
    /// and so on. Empty when there is no line or what it names has merged.
    pub chain: Vec<u64>,
    /// Why the line cannot be honoured; the format gate reports it.
    pub problem: Option<String>,
}

/// Resolve the `Depends` line of pull request `pr`. `open` holds the
/// description of every open pull request against the arena's branch;
/// `landed` the numbers of those that have merged.
pub fn dependency(
    pr: u64,
    body: &str,
    open: &BTreeMap<u64, &str>,
    landed: &BTreeSet<u64>,
    branch: &str,
) -> Dependency {
    let direct = match depends_ref(body) {
        Ok(None) => return Dependency::default(),
        Ok(Some(n)) => n,
        Err(e) => return Dependency { problem: Some(e), ..Default::default() },
    };
    let problem = |text: String| Dependency { direct: Some(direct), chain: Vec::new(), problem: Some(text) };
    let mut chain = Vec::new();
    let mut seen = BTreeSet::from([pr]);
    let mut cur = direct;
    loop {
        if !seen.insert(cur) {
            return problem(format!(
                "`Depends: #{direct}` leads back to a change already in its own chain (#{pr} → {} → #{cur}). A change cannot be built on itself; remove one of the `Depends` lines",
                refs(&chain).replace(", ", " → ")
            ));
        }
        if landed.contains(&cur) {
            break;
        }
        let Some(next_body) = open.get(&cur) else {
            if cur == direct {
                return problem(format!(
                    "`Depends: #{direct}`: #{direct} is not an open pull request against `{branch}` of this repository (it was closed without merging, is a draft or an issue, or does not exist). This change cannot merge while it names it. To submit it on its own, rebase it onto `{branch}`, remove the line and push"
                ));
            }
            // A change further down names something that is not open; its
            // own format gate says so, and it holds everything built on it.
            break;
        };
        chain.push(cur);
        match depends_ref(next_body) {
            Ok(Some(next)) => cur = next,
            _ => break,
        }
    }
    if chain.len() > MAX_STACK {
        return problem(format!(
            "`Depends: #{direct}` is the end of a chain of {} open changes; at most {MAX_STACK} can be verified on top of one another",
            chain.len()
        ));
    }
    chain.reverse();
    Dependency { direct: Some(direct), chain, problem: None }
}

/// Pull requests in order of number, except that one built on another open
/// one comes after it, so that what a change depends on is always judged
/// first. A circle of `Depends` lines is left in order of number; each of
/// its members fails the format gate.
fn dependency_order(mut prs: Vec<PullRequest>) -> Vec<PullRequest> {
    prs.sort_by_key(|p| p.number);
    let mut out = Vec::with_capacity(prs.len());
    while !prs.is_empty() {
        let waiting: BTreeSet<u64> = prs.iter().map(|p| p.number).collect();
        let (now, later): (Vec<PullRequest>, Vec<PullRequest>) = prs.into_iter().partition(|p| {
            !matches!(depends_ref(&p.body), Ok(Some(n)) if n != p.number && waiting.contains(&n))
        });
        if now.is_empty() {
            out.extend(later);
            break;
        }
        out.extend(now);
        prs = later;
    }
    out
}

/// The text under one heading of a description. A heading is a line that
/// starts with `#` and a space, or with a phrase in bold (`**Trial**`, with
/// or without a colon); what follows the bold phrase on its line belongs to
/// the section. The section ends at the next heading. `name` is matched
/// against the start of the heading, ignoring case.
pub fn section(body: &str, name: &str) -> Option<String> {
    let mut found: Option<String> = None;
    for line in body.lines() {
        let t = line.trim();
        let hashes = t.trim_start_matches('#');
        let heading: Option<(String, &str)> = if t.starts_with('#') && hashes.starts_with(' ') {
            Some((hashes.trim().trim_matches(|c| c == '*' || c == ':').trim().to_lowercase(), ""))
        } else if let Some(rest) = t.strip_prefix("**") {
            rest.split_once("**").map(|(h, after)| {
                (h.trim().trim_end_matches(':').trim().to_lowercase(), after.trim_start_matches(':').trim())
            })
        } else {
            None
        };
        match heading {
            Some(_) if found.is_some() => break,
            Some((h, after)) if h.starts_with(name) => found = Some(after.to_string()),
            Some(_) => {}
            None => {
                if let Some(s) = found.as_mut() {
                    s.push('\n');
                    s.push_str(line);
                }
            }
        }
    }
    found
}

/// The largest number in a text, for reading an estimate. A number is a run
/// of digits, optionally grouped in threes by commas (`1,200`), optionally
/// with a decimal part, optionally followed by `k` for thousands (`1.5k` is
/// 1500). A number written right after `#` names an issue and is skipped.
/// Without `k` the decimal part is dropped.
pub fn largest_number(text: &str) -> u64 {
    // `[0-9]`, not `\d`: the latter also matches digits of other scripts,
    // which do not parse as a number.
    let re = Regex::new(r"(#\s*)?([0-9]{1,3}(?:,[0-9]{3})+|[0-9]+)(?:\.([0-9]+))?\s?([kK]\b)?").expect("static regex");
    let mut max = 0u64;
    for c in re.captures_iter(text) {
        if c.get(1).is_some() {
            continue;
        }
        let whole: u64 = c[2].replace(',', "").parse().unwrap_or(u64::MAX / 2000);
        let value = if c.get(4).is_some() {
            let frac: f64 = c.get(3).and_then(|f| format!("0.{}", f.as_str()).parse().ok()).unwrap_or(0.0);
            whole.saturating_mul(1000).saturating_add((frac * 1000.0).round() as u64)
        } else {
            whole
        };
        max = max.max(value);
    }
    max
}

/// The paths or globs an idea reserves: its **Reserves** section. Every
/// string in backticks there is one; if nothing is in backticks, every word
/// that contains a `/`, a `*` or a `.`. Words are separated by spaces,
/// commas or semicolons.
pub fn reserves(body: &str) -> Vec<String> {
    let Some(text) = section(body, "reserve") else { return Vec::new() };
    let quoted: Vec<String> = Regex::new(r"`([^`\n]+)`")
        .expect("static regex")
        .captures_iter(&text)
        .map(|c| c[1].trim().to_string())
        .collect();
    let words = if quoted.is_empty() {
        text.split(|c: char| c.is_whitespace() || c == ',' || c == ';')
            .map(|w| w.trim_matches(|c: char| matches!(c, '.' | ':' | '(' | ')' | '"' | '\'')))
            .filter(|w| w.contains('/') || w.contains('*') || w.contains('.'))
            .map(str::to_string)
            .collect()
    } else {
        quoted
    };
    let mut out: Vec<String> = Vec::new();
    for w in words {
        let w = w.trim_start_matches("./").trim_end_matches('/').to_string();
        if !w.is_empty() && Glob::new(&w).is_ok() && !out.contains(&w) && out.len() < 100 {
            out.push(w);
        }
    }
    out
}

/// Files an idea reserves while it is claimed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reservation {
    pub idea: u64,
    pub claimant: String,
    pub patterns: Vec<String>,
}

/// The paths among `paths` that a reservation covers. A pattern is a glob
/// against the path from the repository root or from the project directory;
/// a pattern without `*` also covers everything under it as a directory and
/// the file of that name with any extension (`Paper/Tree` covers
/// `Paper/Tree.lean` and `Paper/Tree/…`).
pub fn reserved_hits(dir_prefix: &str, patterns: &[String], paths: &[String]) -> Vec<String> {
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        let mut forms = vec![p.clone()];
        if !p.contains('*') {
            forms.push(format!("{p}/**"));
            forms.push(format!("{p}.*"));
        }
        for f in forms {
            if let Ok(g) = Glob::new(&f) {
                b.add(g);
            }
        }
    }
    let Ok(set) = b.build() else { return Vec::new() };
    paths
        .iter()
        .filter(|p| set.is_match(p.as_str()) || p.strip_prefix(dir_prefix).is_some_and(|rel| set.is_match(rel)))
        .cloned()
        .collect()
}

/// The warning a change gets for touching reserved files. It never fails a
/// gate and never blocks a merge.
pub fn reservation_warning(r: &Reservation, hits: &[String]) -> String {
    let mut shown = hits.iter().take(5).map(|p| format!("`{p}`")).collect::<Vec<_>>().join(", ");
    if hits.len() > 5 {
        shown.push_str(&format!(" and {} more", hits.len() - 5));
    }
    format!(
        "touches files reserved by idea #{} (claimed by @{}): {shown}; coordinate there or expect to merge main after it lands",
        r.idea, r.claimant
    )
}

/// The first line of a comment that is a command, split into words.
fn command(body: &str) -> Vec<String> {
    body.lines()
        .map(str::trim)
        .find(|l| l.starts_with('/'))
        .map(|l| l.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

/// Merged changes and tokens saved by one author.
pub fn experience(login: &str, attempts: &[Attempt]) -> (u64, u64) {
    let mut merged = 0;
    let mut saved = 0;
    for a in attempts {
        if a.status == AttemptStatus::Merged && a.author.eq_ignore_ascii_case(login) {
            merged += 1;
            saved += a.delta.map(|d| (-d).max(0) as u64).unwrap_or(0);
        }
    }
    (merged, saved)
}

impl<'a, F: Forge> Gatekeeper<'a, F> {
    pub fn run(&mut self, ledger: &mut Ledger, roles: &mut RolesState, past: &[Attempt]) -> Result<Outcome> {
        let mut out = Outcome::default();
        roles.bootstrap = self.cfg.roles.bootstrap;
        for l in &self.cfg.roles.leaders {
            roles.grant(l, Role::Leader, Vec::new(), self.now);
        }

        let decided_keys: BTreeSet<String> = past.iter().map(|a| key(a.pr, &a.head_sha)).collect();
        let mut main = self.forge.main_head()?;
        let mut main_tree = self.forge.main_tree()?;
        // Never merge onto a main that is known not to verify.
        let mut paused: Option<&str> = None;
        let main_build = self.forge.main_verifies(&main)?;
        if let MainBuild::Errored(n) = main_build {
            // Nothing was learned about main. Build it again, a bounded number
            // of times; merging waits, because main is not known to verify.
            paused = Some("Merging is paused: the build of main is being repeated after a setup error");
            if n >= MAIN_BUILD_ATTEMPTS {
                if ledger.alerts.insert(format!("main-unbuilt@{main}")) {
                    self.forge.create_issue(
                        &format!("Alert: main could not be built at {}", &main[..main.len().min(10)]),
                        "Several builds of main ended before judging it: a download, the runner or the setup of the checker failed, not the proof. Merging is paused until a build of main passes. Leaders: read the latest `verify #0` runs, fix what they could not fetch or start, and run `verify #0` again. Nothing needs to be reverted.",
                        LABEL_ALERT,
                    )?;
                }
                out.log.push(format!("main could not be built in {n} attempts; merging paused"));
            } else if !self.forge.active_verifies()?.contains(&0) {
                self.forge.dispatch_verify(0, &[])?;
                out.log.push(format!("build of main ended without a verdict (attempt {n}); building it again"));
            }
        }
        if main_build == MainBuild::Failed {
            paused = Some("Merging is paused: main itself does not verify");
            if ledger.alerts.insert(format!("main-broken@{main}")) {
                self.forge.create_issue(
                    &format!("Alert: main does not verify at {}", &main[..main.len().min(10)]),
                    "The latest build of main failed its own verification. Merging is paused until a build of main passes. Leaders: inspect the latest `verify #0` run and repair or revert.",
                    LABEL_ALERT,
                )?;
            }
            out.log.push("main does not verify; merging paused".into());
        }
        // Approved, passing changes that wait for re-verification, in order.
        let mut ahead: Vec<u64> = Vec::new();
        let active = self.forge.active_verifies()?;
        let current = self.forge.current_verifies()?;
        // Changes whose verification in the line was dispatched in this pass.
        let mut queued_now: BTreeSet<u64> = BTreeSet::new();
        // `pr@head` of merged changes -> the merge commit.
        let mut merged_heads: BTreeMap<String, String> = past
            .iter()
            .filter(|a| a.status == AttemptStatus::Merged)
            .filter_map(|a| a.merge_sha.clone().map(|m| (key(a.pr, &a.head_sha), m)))
            .collect();
        let mut prs = self.forge.open_prs()?;
        prs.retain(|p| p.base_ref == self.cfg.project.branch && !p.draft);
        // In order of number, except that a change comes after the one it is built on.
        let prs = dependency_order(prs);
        let mut issues = self.forge.issues()?;

        // A pass can be interrupted between merging and saving. Any merged
        // pull request missing from the log is recorded now, so the history
        // never loses a merge.
        for (pr, merge_sha, merged_at) in self.forge.merged_prs()? {
            if pr.base_ref != self.cfg.project.branch
                || past.iter().any(|a| a.pr == pr.number && a.status == AttemptStatus::Merged)
            {
                continue;
            }
            let results = self.forge.verify_results(pr.number, &pr.head_sha)?;
            let result = results.iter().find(|r| r.gates.iter().all(|g| g.status == GateStatus::Pass));
            let mut attempt = self.attempt(&pr, result);
            if let Some((technique, scope, _)) = self.cfg.format.parse_title(&pr.title) {
                attempt.technique = Some(technique);
                attempt.scope = Some(scope);
            }
            attempt.gates = result.map(|r| r.gates.clone()).unwrap_or_default();
            attempt.status = AttemptStatus::Merged;
            attempt.decided_at = Some(merged_at);
            merged_heads.insert(key(pr.number, &pr.head_sha), merge_sha.clone());
            attempt.merge_sha = Some(merge_sha);
            out.log.push(format!("#{}: merge recovered into the log", pr.number));
            out.decided.push(attempt);
        }

        // Commands first, so a role granted in this pass already counts below.
        self.commands(ledger, roles, &prs, &mut issues, past, &active, &mut out.log)?;

        let open_bodies: BTreeMap<u64, &str> = prs.iter().map(|p| (p.number, p.body.as_str())).collect();
        let open_heads: BTreeMap<u64, &str> = prs.iter().map(|p| (p.number, p.head_sha.as_str())).collect();
        // Numbers of the pull requests that have merged, including in this pass.
        let mut landed: BTreeSet<u64> = past
            .iter()
            .chain(out.decided.iter())
            .filter(|a| a.status == AttemptStatus::Merged)
            .map(|a| a.pr)
            .collect();
        // Open changes that cannot merge as they stand, and why; what is
        // built on one of them waits for it.
        let mut blocked: BTreeMap<u64, String> = BTreeMap::new();
        let reservations = self.reservations(ledger, &issues);
        // `pr@head` of open changes that passed in a run about them alone.
        let mut sound: BTreeSet<String> = BTreeSet::new();
        // Things to tell people on pull requests, posted after the loop, when
        // the bot's own comments of this pass are written: the pull request,
        // the key under which it counts as told, whom, and the text.
        let mut notices: Vec<(u64, String, String, String)> = Vec::new();

        for pr in &prs {
            let id = key(pr.number, &pr.head_sha);
            // What this change is built on (`Depends: #n`). While any of that
            // is open the change is held: it is judged on top of it, can be
            // reviewed, and never merges.
            let mut dep = dependency(pr.number, &pr.body, &open_bodies, &landed, &self.cfg.project.branch);
            let dep_keys: Vec<String> =
                dep.chain.iter().map(|n| key(*n, open_heads.get(n).copied().unwrap_or_default())).collect();

            // The record of this commit: when it was first judged, and on top
            // of which commits of which changes. An approval counts only if
            // it was given after that moment, so whoever gave it had the
            // bot's statement of what the commit is built on before them, and
            // that statement cannot change afterwards without the approvals
            // being asked for again:
            //  - no record: the commit is judged for the first time now;
            //  - what it was judged on has merged, at exactly those commits:
            //    nothing changed;
            //  - something it was judged on merged as another commit: the
            //    commit cannot merge any more (a new push is needed);
            //  - the `Depends` line was removed or names another change while
            //    what it named is unmerged: the same;
            //  - what it is built on was pushed to, or a line was added: the
            //    statement changes, and approvals given before do not count.
            let key_number = |k: &String| k.split('@').next().and_then(|n| n.parse::<u64>().ok());
            let key_sha = |k: &String| k.split_once('@').map(|(_, s)| s.to_string()).unwrap_or_default();
            // A new or changed record has no time yet: it gets the forge's
            // time of the comment that states it, when that is written, and
            // until then (for the rest of this pass) no approval counts.
            let names = dep.direct;
            let fresh_record =
                || Judged { at: None, names, on: dep_keys.clone(), contains: None, has_merged: None };
            let record = match ledger.judged.get(&id).cloned() {
                None => fresh_record(),
                Some(was) => {
                    // What it was judged on that has not merged as it was then.
                    let pending: Vec<String> =
                        was.on.iter().filter(|k| !merged_heads.contains_key(*k)).cloned().collect();
                    let superseded = pending.iter().find(|k| key_number(k).is_some_and(|n| landed.contains(&n)));
                    if pending == dep_keys {
                        // Judged on the same commits. If the `Depends` line
                        // itself differs (added, removed or changed where
                        // that changes nothing above, as when what it names
                        // has merged), what the bot says of the commit
                        // changes all the same, and approvals start over.
                        if dep.problem.is_none() && was.names != names {
                            fresh_record()
                        } else {
                            was
                        }
                    } else if let Some(k) = superseded {
                        let n = key_number(k).unwrap_or(0);
                        let prefix = format!("{n}@");
                        let merged = merged_heads.keys().find_map(|m| m.strip_prefix(&prefix)).unwrap_or_default();
                        dep.chain.clear();
                        dep.problem = Some(format!(
                            "The change this was built on (#{n}) was revised before it merged: this commit was judged on top of `{}`, and `{}` is what merged. Merge `{}` into your branch and push; the new commit is judged against `{}` and needs a new approval",
                            short(&key_sha(k)),
                            short(merged),
                            self.cfg.project.branch,
                            self.cfg.project.branch
                        ));
                        was
                    } else if dep.problem.is_some() {
                        was
                    } else if pending.is_empty() || pending.last().and_then(key_number) == dep.chain.last().copied() {
                        fresh_record()
                    } else {
                        let n = pending.last().and_then(key_number).unwrap_or(0);
                        dep.chain.clear();
                        dep.problem = Some(format!(
                            "This commit was judged as a change on top of #{n}, so its `Depends: #{n}` line cannot be removed or changed without a new push (a review is for the commit as it was presented). Restore the line, or push a new commit"
                        ));
                        was
                    }
                }
            };
            let chain = dep.chain.clone();
            let dep_keys: Vec<String> = if chain.is_empty() { Vec::new() } else { dep_keys };
            let held = !chain.is_empty();
            // The commit must contain the head of what it is built on: then
            // what it adds to that change is fixed by the commits themselves,
            // and is what a reviewer of the difference read. If that change
            // was pushed to and this branch does not have the new commit, a
            // new commit is needed here, and with it a new approval.
            let mut record = record;
            if record.contains.is_none() {
                record.contains = match record.on.last() {
                    None => Some(true),
                    Some(k) => self.forge.contains(&pr.head_sha, &key_sha(k)).ok(),
                };
            }
            if record.contains == Some(false) && dep.problem.is_none() {
                let k = record.on.last().cloned().unwrap_or_default();
                let n = key_number(&k).unwrap_or(0);
                let branch = &self.cfg.project.branch;
                dep.problem = Some(if held {
                    format!(
                        "#{n}, which this is built on, is at `{}`, and this branch does not contain that commit. Merge #{n}'s branch into yours and push; the new commit needs a new approval",
                        short(&key_sha(&k))
                    )
                } else {
                    format!(
                        "#{n}, which this was built on, merged as `{}`, and this branch does not contain that commit. Merge `{branch}` into your branch and push; the new commit needs a new approval",
                        short(&key_sha(&k))
                    )
                });
            }
            // A commit whose `Depends` line names a change that has merged
            // must contain the commit of that change that merged. This does
            // not rest on the record: it holds for a commit seen for the
            // first time after the merge too, and for one whose record was
            // lost. Without it such a commit would count as an ordinary
            // change on main while carrying an older version of the change
            // it names, and what a revision of that change dropped would
            // come back with it under a description that says "depends".
            if let Some(n) = dep.direct.filter(|n| landed.contains(n) && dep.problem.is_none()) {
                let prefix = format!("{n}@");
                let merged = merged_heads.keys().find_map(|m| m.strip_prefix(&prefix)).map(str::to_string);
                if record.has_merged.is_none() {
                    record.has_merged = merged.as_ref().and_then(|sha| self.forge.contains(&pr.head_sha, sha).ok());
                }
                if record.has_merged == Some(false) {
                    let branch = &self.cfg.project.branch;
                    dep.problem = Some(format!(
                        "`Depends: #{n}`: #{n} merged as `{}`, and this branch does not contain that commit. Merge `{branch}` into your branch and push, or remove the line (the change is then judged against `{branch}` as a whole, with everything of #{n} that is in this branch and not in `{branch}`); either way approvals are given anew",
                        short(merged.as_deref().unwrap_or_default())
                    ));
                }
            } else {
                record.has_merged = None;
            }
            // Could not be checked (the forge did not answer): nothing merges
            // from this commit until it could.
            let unchecked = record.contains.is_none()
                || (dep.direct.is_some_and(|n| landed.contains(&n)) && dep.problem.is_none() && record.has_merged.is_none());
            // Approvals are measured against the forge's time of the comment
            // that states this record. There is none yet for a record made
            // or changed in this pass.
            let judged_at = record.at;
            let declared: Vec<u64> = record.on.iter().filter_map(key_number).collect();
            // Whether everything it was judged on is still open or has merged
            // as it was then. If not, a result that had it applied is about a
            // tree that will never exist and decides nothing.
            let declared_ok = record.on.iter().all(|k| dep_keys.contains(k) || merged_heads.contains_key(k));
            ledger.judged.insert(id.clone(), record);
            // A failure of this change's build decides something only if
            // everything it is built on has passed on its own at the commit
            // that was applied; until then the failure may be theirs.
            let chain_sound = dep_keys.iter().all(|k| sound.contains(k));
            // Whether everything it is built on is approved and in the line
            // (trivially so for a change built on nothing).
            let in_line = chain.iter().all(|d| ahead.contains(d));
            let dep_blocked: Option<String> =
                chain.iter().find_map(|d| blocked.get(d).map(|why| format!("#{d}, which {why}")));
            let (waiver, waiver_edited) = self.waiver(pr, roles)?;
            let min = waiver.as_ref().map(|_| WAIVED_MIN);

            // A result is valid for exactly the tree it was applied to. Prefer the
            // one for today's main; otherwise the newest one made on main alone
            // (for a change built on others: on main and exactly those) says
            // whether the change is sound in itself; a result stacked on
            // other changes says nothing until those have merged.
            let mut results = self.forge.verify_results(pr.number, &pr.head_sha)?;
            if waiver.is_some() {
                // The minimum is waived for this commit: a result that failed
                // for being judged with the configured minimum is not about
                // the commit as it now stands.
                results.retain(|r| {
                    r.min_delta == Some(WAIVED_MIN) || r.gate("improves").is_some_and(|g| g.status == GateStatus::Pass)
                });
            }
            if held {
                // A result that did not have what this change is built on
                // applied first measured that change's diff as part of this one.
                results.retain(|r| {
                    let applied = stack_numbers(r);
                    chain.iter().all(|d| applied.contains(d))
                });
            }
            // A result is about this change alone if nothing was applied
            // before it but what it is built on: all of that chain, or what
            // was left of it when the run was made.
            let own = |r: &VerifyResult| {
                let applied = stack_numbers(r);
                if applied.is_empty() {
                    !held
                } else {
                    declared_ok && declared.ends_with(&applied) && applied.ends_with(&chain)
                }
            };
            let is_fresh = |r: &VerifyResult| match &r.base_tree {
                Some(t) => *t == main_tree,
                None => r.base_sha == main,
            };
            // A commit that was judged on top of another change has commits of
            // that change in its branch, so a stacked result for it must say
            // which tree lands; one from before that was recorded does not count.
            let lands = |r: &VerifyResult| {
                lands_as_verified(r) && (declared.is_empty() || r.stack.is_empty() || r.landing_tree.is_some())
            };
            // Held changes have no result to merge from, whatever its tree; nor
            // has a commit of which it is not known whether it contains what
            // it was built on.
            let fresh_result =
                if held || unchecked { None } else { results.iter().find(|r| is_fresh(r) && lands(r)) };
            let fresh = fresh_result.is_some();
            // Verified on exactly today's main, but this branch would land as another tree.
            let diverged = !held && !unchecked && !fresh && results.iter().any(|r| is_fresh(r));
            let result = fresh_result.or(results.iter().find(|r| own(r))).or(results.first()).cloned();
            // A held change's result that does not pass, while something it is
            // built on has not passed on its own at the commit that was
            // applied: the failure may be that change's, so nothing is decided.
            let undecided = held
                && result.as_ref().is_some_and(|r| {
                    !(chain_sound && dep_keys.iter().all(|k| r.stack.contains(k)))
                        && r.gates.iter().any(|g| g.status != GateStatus::Pass)
                });
            let speculative_only = !fresh
                && !results.is_empty()
                && (dep_blocked.is_some() || undecided || !results.iter().any(|r| own(r)));
            let changed = result.as_ref().map(|r| r.changed_files).unwrap_or(0);
            let (review, early, edited) = self.review_gate(pr, roles, changed, judged_at)?;
            // An approval given before the bot's comment said what this
            // commit is judged against does not count. Its reviewer is told
            // once, so that nobody waits; the notice says what the basis is,
            // and is posted at the end of the pass, after that comment.
            let basis = if dep_keys.is_empty() {
                let branch = &self.cfg.project.branch;
                format!("It is judged against `{branch}` as a whole: everything in the pull request's difference from `{branch}`")
            } else {
                format!(
                    "It is judged as the difference from {}; this branch contains that, and what it adds to it is what an approval is of",
                    dep_keys
                        .iter()
                        .map(|k| format!("#{} at `{}`", key_number(k).unwrap_or(0), key_sha(k)))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            for user in &early {
                notices.push((
                    pr.number,
                    format!("{id}@early@{}", user.to_lowercase()),
                    user.clone(),
                    format!(
                        "@{user} your approval of `{}` does not count: it was given before the bot's comment on this commit said what the commit is judged against, as it stands now (before the commit was first checked, or before a change it is built on was pushed to). {basis}. If your approval holds for that, give it again: `/approve {}`.",
                        short(&pr.head_sha),
                        pr.head_sha
                    ),
                ));
            }
            for (command, users) in [("/approve", &edited), ("/waive-min", &waiver_edited)] {
                for user in users.iter() {
                    notices.push((
                        pr.number,
                        format!("{id}@edited@{command}@{}", user.to_lowercase()),
                        user.clone(),
                        format!(
                            "@{user} your `{command}` comment was edited after it was posted, so it does not count: anyone with write access can change a comment, and the gatekeeper cannot tell who did. Post it again as a new comment, naming the commit in full: `{command} {}`.",
                            pr.head_sha
                        ),
                    ));
                }
            }
            let (format, parsed) = self.format_gate(pr, dep.problem.as_deref());
            let format_ok = format.status == GateStatus::Pass;
            let mut attempt = self.attempt(pr, result.as_ref());
            if let Some((technique, scope, _)) = parsed {
                attempt.technique = Some(technique);
                attempt.scope = Some(scope);
            }
            attempt.depends = chain.clone();
            attempt.judged_on = dep_keys.clone();
            attempt.waiver = waiver.clone();
            attempt.warnings = self.reservation_warnings(pr, &reservations, &chain, &prs, roles.bootstrap);

            if pr.mergeable == Some(false) {
                blocked.insert(pr.number, "conflicts with main".into());
                attempt.gates = vec![format];
                attempt.gates.extend(result.as_ref().map(|r| r.gates.clone()).unwrap_or_default());
                attempt.gates.push(Gate::new(
                    "fresh",
                    GateStatus::Fail,
                    "Conflicts with main; merge main into your branch and push",
                ));
                attempt.gates.push(review);
                attempt.status = AttemptStatus::Conflict;
                self.publish(pr, &attempt, CommitState::Failure, "Conflicts with main", ledger)?;
                out.open.push(attempt);
                continue;
            }

            let Some(result) = result else {
                // Not verified yet. Re-dispatch if the automatic trigger was lost.
                attempt.gates = pending_gates(&format, &review);
                if held {
                    if let Some(g) = attempt.gates.iter_mut().find(|g| g.key == "fresh") {
                        g.detail = Some(match &dep_blocked {
                            Some(why) => format!(
                                "Built on {why}. It is verified once that is repaired, and cannot merge before it"
                            ),
                            None => format!(
                                "Built on {}: verified with it applied first, and merges only after it has merged",
                                refs(&chain)
                            ),
                        });
                    }
                }
                let k = id.clone();
                // A run that judges with the waived minimum has to be started
                // once for this commit, whatever else is running.
                let waived_run = min.is_some() && !ledger.dispatched.contains_key(&format!("{id}@waived"));
                // Approved before its first result: it joins the line now and is
                // verified on top of the changes ahead of it, so that the line
                // merges as one train. Its own run on main alone keeps going and
                // is what decides if the change fails. Nothing more is needed
                // when that run is already about today's main and nothing is
                // ahead but what the change is built on. A change built on
                // one that is not in the line stays out of it.
                let mut joined = false;
                if review.status == GateStatus::Pass && format_ok && in_line && dep_blocked.is_none() {
                    let alone_on_main = current.contains(&pr.number) && ahead == chain && !waived_run;
                    if !alone_on_main {
                        let line = Line { main: &main, main_tree: &main_tree, ahead: &ahead, active: &active, merged: &merged_heads };
                        joined = self.dispatch_stacked(pr, &prs, &line, &[], min, ledger, &mut out.log)?;
                        if joined {
                            queued_now.insert(pr.number);
                        }
                    }
                    ahead.push(pr.number);
                }
                let waited = self.now - pr.created_at > Duration::minutes(20);
                let recently = ledger
                    .dispatched
                    .get(&k)
                    .is_some_and(|t| self.now - *t < Duration::hours(3));
                let lost = !active.contains(&pr.number) && waited && !recently;
                if !joined
                    && dep_blocked.is_none()
                    && (waived_run || lost)
                    && self.start(pr, &chain, min, ledger, &mut out.log)?
                {
                    ledger.dispatched.insert(k, self.now);
                    out.log.push(format!("#{}: dispatched verification", pr.number));
                }
                let text = match (&dep_blocked, held) {
                    (Some(_), _) => "Waiting for the change it is built on".to_string(),
                    (None, true) => format!("Verification in progress on top of {}", refs(&chain)),
                    (None, false) => "Verification in progress".to_string(),
                };
                self.publish_open(pr, attempt, CommitState::Pending, &text, &mut out, &mut blocked, ledger)?;
                continue;
            };

            attempt.measured_on = result.stack.clone();
            let mut gates = vec![format];
            gates.extend(result.gates.iter().cloned());
            // A result from before long lines and tolerated constructs had a
            // gate of their own carries those checks inside `deny`.
            if result.min_delta.is_none() && !gates.iter().any(|g| g.key == "no_increase") {
                if let Some(at) = gates.iter().position(|g| g.key == "deny") {
                    let status = if gates[at].status == GateStatus::Pass { GateStatus::Pass } else { GateStatus::Skip };
                    gates.insert(at + 1, Gate::new("no_increase", status, "Checked together with the banned constructs"));
                }
            }
            // A pass of `improves` counts only if it was judged with the
            // minimum in force for this commit: the configured one, or the
            // waived one while a leader's waiver for this commit stands.
            let required_min = min.unwrap_or(self.cfg.score.min_delta);
            if let Some(judged) = result.min_delta {
                let enough = attempt.delta.is_some_and(|d| -d >= required_min as i64);
                if judged < required_min && !enough {
                    if let Some(g) = gates.iter_mut().find(|g| g.key == "improves" && g.status == GateStatus::Pass) {
                        g.status = GateStatus::Fail;
                        g.detail = Some(format!(
                            "Judged with a minimum of {judged} {}; the minimum is {required_min} and no waiver is in force for this commit",
                            self.cfg.score.primary
                        ));
                    }
                }
            }
            // Anything short of an explicit pass on every required gate is a
            // failure: a skipped or missing check never lets a change through.
            let required_gates = REQUIRED_RESULT_GATES.iter().copied().chain(result.min_delta.map(|_| "no_increase"));
            for required in required_gates.filter(|_| !speculative_only) {
                match gates.iter_mut().find(|g| g.key == required) {
                    Some(g) if g.status == GateStatus::Pending => {
                        g.status = GateStatus::Fail;
                        g.detail = Some("The check did not finish".into());
                    }
                    Some(_) => {}
                    None => gates.push(Gate::new(required, GateStatus::Fail, "The check did not run")),
                }
            }
            if speculative_only {
                // Only stacked results exist; their verdict is not about this
                // change alone, so nothing is decided from them.
                for g in gates.iter_mut().skip(1) {
                    g.status = GateStatus::Pending;
                    g.detail = Some(match &dep_blocked {
                        // Its build had that change applied first, so a
                        // failure may be that change's and decides nothing.
                        Some(why) => format!("Not decided while it is built on {why}"),
                        None if undecided => format!(
                            "Not decided: the run with {} applied first did not pass, and that has not passed on its own at the commit that was applied, so the failure may not be this change's",
                            refs(&chain)
                        ),
                        None => "Verified only together with changes queued ahead of it".into(),
                    });
                }
            }
            let result_failed = gates.iter().skip(1).any(|g| g.status == GateStatus::Fail);
            for g in gates.iter_mut().skip(1).filter(|g| g.status == GateStatus::Skip) {
                if !result_failed {
                    g.status = GateStatus::Fail;
                    g.detail = Some("The check did not run".into());
                }
            }
            let failed: Vec<&Gate> = gates.iter().skip(1).filter(|g| g.status == GateStatus::Fail).collect();

            if !failed.is_empty() {
                let reason = failed
                    .iter()
                    .map(|g| match &g.detail {
                        Some(d) => format!("{}: {}", g.label, d),
                        None => g.label.clone(),
                    })
                    .collect::<Vec<_>>()
                    .join(" · ");
                // What the failure means is a property of the gate (the table
                // in `model.rs`). If every failed gate is one the author can
                // fix, nothing is rejected and nothing is logged: the change
                // stays open and says what to fix.
                let verdict = failed.iter().any(|g| gate_on_fail(&g.key) == OnFail::Reject);
                gates.push(Gate::new("fresh", GateStatus::Skip, ""));
                if !verdict {
                    gates.push(review);
                    attempt.gates = gates;
                    if pr.labels.iter().any(|l| l == LABEL_REJECTED) {
                        self.forge.set_label(pr.number, LABEL_REJECTED, false)?;
                    }
                    self.forge.set_label(pr.number, LABEL_READY, false)?;
                    self.publish_open(pr, attempt, CommitState::Failure, "Needs a fix", &mut out, &mut blocked, ledger)?;
                    continue;
                }
                gates.push(Gate::new("review", GateStatus::Skip, ""));
                attempt.gates = gates;
                attempt.status = AttemptStatus::Rejected;
                attempt.reason = Some(reason);
                attempt.decided_at = Some(self.now);
                blocked.insert(pr.number, "was rejected at its current commit".into());
                self.publish(pr, &attempt, CommitState::Failure, "Rejected: a gate failed", ledger)?;
                self.forge.set_label(pr.number, LABEL_REJECTED, true)?;
                self.forge.set_label(pr.number, LABEL_READY, false)?;
                if pr.labels.iter().any(|l| l == LABEL_NEEDS_FIX) {
                    self.forge.set_label(pr.number, LABEL_NEEDS_FIX, false)?;
                }
                // A rejected head lives in the decided log, not among the open
                // attempts; a new push starts a new attempt.
                if !decided_keys.contains(&key(pr.number, &pr.head_sha)) {
                    out.log.push(format!("#{}: rejected", pr.number));
                    out.decided.push(attempt);
                }
                continue;
            }

            if pr.labels.iter().any(|l| l == LABEL_REJECTED) {
                self.forge.set_label(pr.number, LABEL_REJECTED, false)?;
            }
            let approved = review.status == GateStatus::Pass;
            if !speculative_only {
                // This commit passed in a run that was about it alone.
                sound.insert(id.clone());
            }

            if !fresh {
                let ready = approved && format_ok;
                // The first change this one is built on whose head is no
                // longer the one the result shown was verified on top of.
                let revised = chain.iter().zip(&dep_keys).find(|(_, k)| !result.stack.contains(k)).map(|(d, _)| *d);
                let mut status = AttemptStatus::Stale;
                let mut text = "Waiting for re-verification on current main".to_string();
                let detail = if let Some(why) = &dep_blocked {
                    text = "Waiting for the change it is built on".into();
                    format!("Built on {why}. Nothing more happens here until that is repaired; this change cannot merge before it")
                } else if undecided {
                    text = format!("Waiting for {} to pass on its own", refs(&chain));
                    format!(
                        "Nothing is decided yet: the run with {} applied first did not pass, and that has not passed on its own. This change is judged when it has",
                        refs(&chain)
                    )
                } else if unchecked {
                    text = "Waiting: could not check what this branch contains".into();
                    "Could not check that this branch contains the commit of the change it was built on; nothing merges from it until that is known. It is tried again on the next pass".to_string()
                } else if let Some(d) = revised {
                    // The branch contains the new head (otherwise the format
                    // gate has already said to merge it in), so only the
                    // verification and the approvals are out of date.
                    let prefix = format!("{d}@");
                    let then = result.stack.iter().find_map(|k| k.strip_prefix(&prefix)).unwrap_or_default();
                    let now = open_heads.get(&d).copied().unwrap_or_default();
                    text = format!("#{d}, which it is built on, was revised");
                    format!(
                        "Out of date: #{d} was pushed to after this was verified on top of it (`{}` then, `{}` now, which this branch contains). Approvals given before that push do not count and must be given again. {}",
                        short(then),
                        short(now),
                        if ready {
                            "It is being verified again on the new commit"
                        } else {
                            "It is verified again on the new commit once it is approved"
                        }
                    )
                } else if held {
                    if speculative_only {
                        text = format!("Waiting for verification on top of {}", refs(&chain));
                    } else if approved {
                        text = format!("Waiting for {} to merge first", refs(&chain));
                    } else {
                        // Verified on top of what it is built on as that stands:
                        // it can be reviewed while that is still open.
                        status = AttemptStatus::AwaitingReview;
                        text = format!("Checks passed on top of {}; waiting for independent review", refs(&chain));
                    }
                    format!(
                        "Verified on top of {}; merges only after {} merged{}",
                        refs(&chain),
                        if chain.len() == 1 { "it has" } else { "they have" },
                        if in_line { "" } else { ", and that is not approved yet" }
                    )
                } else if diverged {
                    format!(
                        "Verified on top of a change that has merged since, but this branch contains commits of that change and no longer merges to the tree that was verified. {}; if it conflicts, merge main into your branch and push",
                        if ready { "It is being verified again on main" } else { "It is verified again on main once approved" }
                    )
                } else if ready {
                    "Main moved; re-verifying on the new main".to_string()
                } else {
                    "Main moved; will be re-verified once approved".to_string()
                };
                gates.push(Gate::new("fresh", GateStatus::Pending, detail));
                gates.push(review);
                attempt.gates = gates;
                attempt.status = status;
                if ready && in_line && dep_blocked.is_none() && !unchecked {
                    // Verify on top of the approved changes ahead of this one,
                    // so the whole line can merge without a rebuild per merge.
                    // It is dispatched at once, in order, even while an older run
                    // of this change is still going: that run is about a main
                    // that no longer exists, and waiting for it would let the
                    // line form in the order results happen to arrive.
                    // A change built on others joins only when all of them are
                    // in the line, and needs no new run if the line is exactly
                    // what its own run was already verified on.
                    let line_keys: Vec<String> =
                        ahead.iter().map(|n| key(*n, open_heads.get(n).copied().unwrap_or_default())).collect();
                    let built = held && results.iter().any(|r| r.base_sha == main && r.stack == line_keys);
                    let line = Line { main: &main, main_tree: &main_tree, ahead: &ahead, active: &active, merged: &merged_heads };
                    if !built && self.dispatch_stacked(pr, &prs, &line, &results, min, ledger, &mut out.log)? {
                        queued_now.insert(pr.number);
                    }
                    ahead.push(pr.number);
                } else if held
                    && dep_blocked.is_none()
                    && !undecided
                    && (speculative_only || (revised.is_some() && ready))
                {
                    // Out of the line: verified on main and exactly what it is
                    // built on, once per commit of each.
                    let k = format!("{id}@on@{}", dep_keys.join("+"));
                    if !active.contains(&pr.number)
                        && !ledger.dispatched.contains_key(&k)
                        && self.start(pr, &chain, min, ledger, &mut out.log)?
                    {
                        ledger.dispatched.insert(k, self.now);
                        out.log.push(format!("#{}: verifying on top of {:?}", pr.number, chain));
                    }
                }
                self.forge.set_label(pr.number, LABEL_READY, !approved && format_ok && dep_blocked.is_none())?;
                self.publish_open(pr, attempt, CommitState::Pending, &text, &mut out, &mut blocked, ledger)?;
                continue;
            }

            gates.push(Gate::new("fresh", GateStatus::Pass, ""));
            gates.push(review);
            attempt.gates = gates;

            if !approved || !format_ok || paused.is_some() {
                attempt.status = AttemptStatus::AwaitingReview;
                self.forge.set_label(pr.number, LABEL_READY, !approved && format_ok)?;
                let text = if !format_ok {
                    "Checks passed; the title or description needs fixing"
                } else if !approved {
                    "All checks passed; waiting for independent review"
                } else {
                    paused.unwrap_or("Merging is paused")
                };
                self.publish_open(pr, attempt, CommitState::Pending, text, &mut out, &mut blocked, ledger)?;
                continue;
            }

            // A line of approved changes ahead of this one is being verified on
            // today's main. Merging this one first would move main under all of
            // them, so it takes its place at the end of the line instead.
            if ahead.iter().any(|a| active.contains(a) || queued_now.contains(a)) {
                let line = Line { main: &main, main_tree: &main_tree, ahead: &ahead, active: &active, merged: &merged_heads };
                if self.dispatch_stacked(pr, &prs, &line, &results, min, ledger, &mut out.log)? {
                    queued_now.insert(pr.number);
                }
                if let Some(g) = attempt.gates.iter_mut().find(|g| g.key == "fresh") {
                    g.status = GateStatus::Pending;
                    g.detail = Some(format!(
                        "Passed on today's main; merges after the changes queued ahead of it ({})",
                        ahead.iter().map(|n| format!("#{n}")).collect::<Vec<_>>().join(", ")
                    ));
                }
                ahead.push(pr.number);
                attempt.status = AttemptStatus::Stale;
                self.forge.set_label(pr.number, LABEL_READY, false)?;
                self.publish_open(pr, attempt, CommitState::Pending, "Queued behind the changes ahead of it", &mut out, &mut blocked, ledger)?;
                continue;
            }

            // Everything is green: merge. Reaching this point means: the
            // result is for exactly today's main tree and lands as the tree
            // it verified; every required gate passed in it; the approved
            // commit is the head; and nothing this change is built on is
            // still open (a held change never has a result to merge from).
            if pr.labels.iter().any(|l| l == LABEL_NEEDS_FIX) {
                self.forge.set_label(pr.number, LABEL_NEEDS_FIX, false)?;
            }
            let delta = attempt.delta.unwrap_or(0);
            let message = format!(
                "{} (#{})\n\n{}: {} -> {} ({})",
                pr.title,
                pr.number,
                self.cfg.score.primary,
                result.base_metrics.get(&self.cfg.score.primary).unwrap_or(0),
                result.metrics.get(&self.cfg.score.primary).unwrap_or(0),
                delta
            );
            self.forge.set_status(&pr.head_sha, CommitState::Success, "All gates passed")?;
            match self.forge.merge(pr, &message) {
                Ok(merge_sha) => {
                    attempt.status = AttemptStatus::Merged;
                    attempt.decided_at = Some(self.now);
                    attempt.merge_sha = Some(merge_sha.clone());
                    self.forge.set_label(pr.number, LABEL_READY, false)?;
                    self.forge.upsert_comment(pr.number, &self.comment_text(pr, &attempt, ledger))?;
                    main = merge_sha.clone();
                    main_tree = self.forge.main_tree()?;
                    landed.insert(pr.number);
                    merged_heads.insert(key(pr.number, &pr.head_sha), merge_sha.clone());
                    out.current = Some(Point { sha: merge_sha.clone(), at: self.now, metrics: result.metrics });
                    out.log.push(format!("#{}: merged ({delta})", pr.number));
                    if let Some(idea) = attempt.idea {
                        if idea_complete(&pr.body) {
                            // The author says this finishes the idea.
                            let _ = self.forge.comment(
                                idea,
                                &format!(
                                    "#{} merged and completes this idea ({delta} {}). Closing; reopen it if something is left.",
                                    pr.number, self.cfg.score.primary
                                ),
                            );
                            let _ = self.forge.close_issue(idea);
                            out.log.push(format!("idea #{idea}: closed, completed by #{}", pr.number));
                        } else {
                            // An idea can take several changes, so a part does not close it.
                            let _ = self.forge.comment(
                                idea,
                                &format!(
                                    "#{} merged for one part of this idea ({delta} {}). It stays open for the rest; the change that finishes it says `Idea: #{idea} (complete)`, or an ideator or leader closes it.",
                                    pr.number, self.cfg.score.primary
                                ),
                            );
                        }
                    }
                    out.decided.push(attempt);
                    // What landed must be exactly what was verified.
                    if result.merged_tree.as_ref().is_some_and(|t| *t != main_tree) {
                        paused = Some("Merging is paused: a merge produced a tree that differs from the verified one");
                        out.log.push(format!("#{}: MERGED TREE DIFFERS FROM THE VERIFIED TREE", pr.number));
                        if ledger.alerts.insert(format!("tree-mismatch@{merge_sha}")) {
                            self.forge.create_issue(
                                &format!("Alert: merge of #{} differs from what was verified", pr.number),
                                &format!("Commit {merge_sha} on main has a different tree than the one verification built for #{}. Merging is paused for this pass. Leaders: compare the two trees and rebuild main (`verify #0`).", pr.number),
                                LABEL_ALERT,
                            )?;
                        }
                    }
                }
                Err(e) => {
                    out.log.push(format!("#{}: merge failed: {e:#}", pr.number));
                    attempt.status = AttemptStatus::AwaitingReview;
                    out.open.push(attempt);
                }
            }
        }

        // Tell people what they need to know, each thing once. A notice that
        // cannot be posted is tried again in the next pass; it never ends
        // this one.
        for (number, told, user, text) in notices {
            if ledger.told.contains(&told) {
                continue;
            }
            match self.forge.comment(number, &text) {
                Ok(()) => {
                    ledger.told.insert(told);
                }
                Err(e) => out.log.push(format!("#{number}: could not tell {user} why a comment or review does not count: {e:#}")),
            }
        }

        // Forget dispatch records for pull requests that are gone.
        let open_numbers: BTreeSet<String> = prs.iter().map(|p| format!("{}@", p.number)).collect();
        ledger.dispatched.retain(|k, _| open_numbers.iter().any(|p| k.starts_with(p)));
        ledger.lines.retain(|k, _| open_numbers.iter().any(|p| k.starts_with(p)));
        ledger.line_bases.retain(|k, _| open_numbers.iter().any(|p| k.starts_with(p)));
        ledger.told.retain(|k| open_numbers.iter().any(|p| k.starts_with(p)));
        // The record of a judged commit is not dropped because its pull
        // request is missing from a pass: a draft, a closed or a retargeted
        // pull request can come back with the same commit, and what that
        // commit was judged on must still hold then. It goes when the pull
        // request has merged. Of the records of pull requests that are not
        // open in this pass (drafts, closed ones), at most `JUDGED_KEPT` are
        // kept, the oldest going first; records of open pull requests are
        // not counted, so piling up commits on an open pull request pushes
        // nothing out. No rule rests on a record being there: a commit whose
        // record is gone is judged for the first time when it is seen again,
        // approvals from before that do not count, and what its `Depends`
        // line requires of the branch is checked without the record.
        let number_of = |k: &String| k.split('@').next().and_then(|n| n.parse::<u64>().ok());
        ledger.judged.retain(|k, _| !number_of(k).is_some_and(|n| landed.contains(&n)));
        let is_open = |k: &String| open_numbers.iter().any(|p| k.starts_with(p));
        let mut closed: Vec<(Option<DateTime<Utc>>, String)> =
            ledger.judged.iter().filter(|(k, _)| !is_open(k)).map(|(k, j)| (j.at, k.clone())).collect();
        if closed.len() > JUDGED_KEPT {
            closed.sort();
            let excess = closed.len() - JUDGED_KEPT;
            for (_, k) in closed.into_iter().take(excess) {
                ledger.judged.remove(&k);
            }
        }

        // A merged change needs no more verification: stop what is still
        // running for it, so the runners go to the changes that wait.
        let done: BTreeSet<u64> = past
            .iter()
            .chain(out.decided.iter())
            .filter(|a| a.status == AttemptStatus::Merged && active.contains(&a.pr))
            .map(|a| a.pr)
            .filter(|n| prs.iter().all(|p| p.number != *n) || out.decided.iter().any(|a| a.pr == *n))
            .collect();
        if !done.is_empty() {
            match self.forge.cancel_verifies(&done) {
                Ok(()) => out.log.push(format!("stopped verification of merged changes {done:?}")),
                Err(e) => out.log.push(format!("could not stop verification of {done:?}: {e:#}")),
            }
        }

        let mut all: Vec<Attempt> = past.to_vec();
        all.extend(out.decided.iter().cloned());
        out.ideas = self.ideas(ledger, roles, &prs, &mut issues, &all, &mut out.log)?;
        Ok(out)
    }

    fn attempt(&self, pr: &PullRequest, result: Option<&VerifyResult>) -> Attempt {
        let primary = &self.cfg.score.primary;
        let delta = result.and_then(|r| {
            Some(r.metrics.get(primary)? as i64 - r.base_metrics.get(primary)? as i64)
        });
        Attempt {
            pr: pr.number,
            title: pr.title.clone(),
            author: pr.author.clone(),
            head_sha: pr.head_sha.clone(),
            base_sha: result.map(|r| r.base_sha.clone()),
            status: AttemptStatus::Verifying,
            created_at: pr.created_at,
            decided_at: None,
            metrics: result.map(|r| r.metrics),
            base_metrics: result.map(|r| r.base_metrics),
            delta,
            gates: Vec::new(),
            idea: idea_ref(&pr.body),
            url: pr.url.clone(),
            run_url: result.and_then(|r| r.run_url.clone()),
            merge_sha: None,
            reason: None,
            technique: None,
            scope: None,
            depends: Vec::new(),
            judged_on: Vec::new(),
            measured_on: Vec::new(),
            warnings: Vec::new(),
            waiver: None,
        }
    }

    /// Set the commit status and the bot's comment. The comment is where the
    /// gatekeeper states what a commit is judged against, so the time the
    /// forge gives for it is what approvals are measured against: the first
    /// comment made for a commit after its record was made or changed gives
    /// the record its time.
    fn publish(
        &mut self,
        pr: &PullRequest,
        attempt: &Attempt,
        state: CommitState,
        text: &str,
        ledger: &mut Ledger,
    ) -> Result<()> {
        self.forge.set_status(&pr.head_sha, state, text)?;
        let stated = self.forge.upsert_comment(pr.number, &self.comment_text(pr, attempt, ledger))?;
        if let Some(record) = ledger.judged.get_mut(&key(pr.number, &pr.head_sha)) {
            record.at.get_or_insert(stated);
        }
        Ok(())
    }

    /// The bot's comment as it is written: the rendered attempt, with the
    /// commit and what it is judged on in the first line, after the marker:
    /// `<!-- hillclimb-gate --> <!-- <head>; on <n@head+… or the branch>[; depends #n] -->`.
    ///
    /// The forge keeps a comment's time when the same text is written again,
    /// and that time is what approvals are measured against. With the commit
    /// and the basis in the text, the same text can only be a statement about
    /// the same commit on the same basis; after a push, or when the basis
    /// changes, the comment is written anew and gets a new time, whatever the
    /// rest of it says.
    fn comment_text(&self, pr: &PullRequest, attempt: &Attempt, ledger: &Ledger) -> String {
        let record = ledger.judged.get(&key(pr.number, &pr.head_sha));
        let on = match record {
            Some(r) if !r.on.is_empty() => r.on.join("+"),
            _ => self.cfg.project.branch.clone(),
        };
        let names = record.and_then(|r| r.names).map(|n| format!("; depends #{n}")).unwrap_or_default();
        render_comment(self.cfg, attempt).replacen(
            COMMENT_MARKER,
            &format!("{COMMENT_MARKER} <!-- {}; on {on}{names} -->", pr.head_sha),
            1,
        )
    }

    /// Publish an attempt that stays open. If a gate failed that its author
    /// can fix, the attempt is in the state "needs a fix" whatever else is
    /// going on: the commit status fails with those words, the label is set,
    /// and the comment lists what to fix. It is never logged as rejected.
    fn publish_open(
        &mut self,
        pr: &PullRequest,
        mut attempt: Attempt,
        state: CommitState,
        text: &str,
        out: &mut Outcome,
        blocked: &mut BTreeMap<u64, String>,
        ledger: &mut Ledger,
    ) -> Result<()> {
        let fix: Vec<(String, String)> = attempt
            .gates
            .iter()
            .filter(|g| g.status == GateStatus::Fail && gate_on_fail(&g.key) == OnFail::Fix)
            .map(|g| (g.label.clone(), g.detail.clone().unwrap_or_default()))
            .collect();
        let needs_fix = !fix.is_empty() && attempt.status != AttemptStatus::Conflict;
        let mut text = text.to_string();
        let mut state = state;
        if needs_fix {
            attempt.status = AttemptStatus::NeedsFix;
            attempt.reason = Some(
                fix.iter()
                    .map(|(label, detail)| if detail.is_empty() { label.clone() } else { format!("{label}: {detail}") })
                    .collect::<Vec<_>>()
                    .join(" · "),
            );
            blocked.insert(pr.number, "needs a fix by its author".into());
            state = CommitState::Failure;
            text = format!("Needs a fix: {}", fix.iter().map(|f| f.0.to_lowercase()).collect::<Vec<_>>().join("; "));
            // A commit status description is limited to 140 characters.
            if text.chars().count() > 140 {
                text = text.chars().take(139).chain(std::iter::once('…')).collect();
            }
        }
        if needs_fix || pr.labels.iter().any(|l| l == LABEL_NEEDS_FIX) {
            self.forge.set_label(pr.number, LABEL_NEEDS_FIX, needs_fix)?;
        }
        self.publish(pr, &attempt, state, &text, ledger)?;
        out.open.push(attempt);
        Ok(())
    }

    /// Start a verification of `pr` on main with `stack` applied first. With
    /// a waived minimum the run judges `improves` with it; a workflow that
    /// does not take that input yet refuses the run, which is logged and
    /// never ends the pass. Returns whether a run was started.
    fn start(
        &mut self,
        pr: &PullRequest,
        stack: &[u64],
        min: Option<u64>,
        ledger: &mut Ledger,
        log: &mut Vec<String>,
    ) -> Result<bool> {
        let Some(min) = min else {
            self.forge.dispatch_verify(pr.number, stack)?;
            return Ok(true);
        };
        match self.forge.dispatch_verify_min(pr.number, stack, min) {
            Ok(()) => {
                ledger.dispatched.insert(format!("{}@waived", key(pr.number, &pr.head_sha)), self.now);
                Ok(true)
            }
            Err(e) => {
                log.push(format!(
                    "#{}: could not start a verification with the waived minimum (does the verify workflow take `min_delta`?): {e:#}",
                    pr.number
                ));
                Ok(false)
            }
        }
    }

    /// A leader's waiver of the minimum step for exactly this head commit: a
    /// comment whose command line is `/waive-min <commit> <reason>`. Like an
    /// approval it names the commit, so a later push needs a new one; the
    /// reason is mandatory. A leader cannot waive for a change from their
    /// own account, except in the bootstrap phase, as with reviews. The
    /// latest valid waiver is the one shown. A comment that was edited after
    /// it was posted is not a waiver; the leaders whose otherwise valid
    /// waiver was in such a comment are returned so that they can be told.
    fn waiver(&mut self, pr: &PullRequest, roles: &RolesState) -> Result<(Option<Waiver>, Vec<String>)> {
        let mut comments = self.forge.pr_comments(pr.number)?;
        comments.sort_by_key(|c| c.created_at);
        let mut found = None;
        let mut edited: Vec<String> = Vec::new();
        for c in comments {
            let cmd = command(&c.body);
            if cmd.first().map(String::as_str) != Some("/waive-min") {
                continue;
            }
            let own = c.user.eq_ignore_ascii_case(&pr.author);
            if !roles.is_leader(&c.user) || (own && !roles.bootstrap) {
                continue;
            }
            let sha = cmd.get(1).map(String::as_str).unwrap_or("");
            // The commit in full, as for an approval: a prefix could be
            // inherited by another commit pushed later.
            if !names_commit(sha, &pr.head_sha) || cmd.len() < 3 {
                continue;
            }
            if c.edited {
                if !edited.iter().any(|e| e.eq_ignore_ascii_case(&c.user)) {
                    edited.push(c.user);
                }
                continue;
            }
            found = Some(Waiver { by: c.user, reason: cmd[2..].join(" ") });
        }
        // A leader who has posted a valid waiver since needs no telling.
        if let Some(w) = &found {
            edited.retain(|e| !e.eq_ignore_ascii_case(&w.by));
        }
        Ok((found, edited))
    }

    /// Reservations in force: ideas in the queue that are claimed and have a
    /// **Reserves** section.
    fn reservations(&self, ledger: &Ledger, issues: &[IssueInfo]) -> Vec<Reservation> {
        issues
            .iter()
            .filter(|i| i.open && i.labels.iter().any(|l| *l == self.cfg.ideas.label))
            .filter_map(|i| {
                let claim = ledger.claims.get(&i.number)?;
                let patterns = reserves(&i.body);
                (!patterns.is_empty()).then(|| Reservation { idea: i.number, claimant: claim.login.clone(), patterns })
            })
            .collect()
    }

    /// Warnings for a pull request that touches files an idea reserves. The
    /// claimant's own pull request for that idea is exempt, and so is any
    /// change by the claimant, except in the bootstrap phase, where every
    /// agent uses one account and only the pull request for the idea itself
    /// is exempt. A change built on the claimant's pull request for the idea
    /// is exempt too. Nothing here can fail: a warning never blocks a merge.
    fn reservation_warnings(
        &mut self,
        pr: &PullRequest,
        reservations: &[Reservation],
        chain: &[u64],
        prs: &[PullRequest],
        bootstrap: bool,
    ) -> Vec<String> {
        let mut out = Vec::new();
        let mut files: Option<(Vec<String>, bool)> = None;
        for r in reservations {
            let implements = |p: &PullRequest| {
                p.author.eq_ignore_ascii_case(&r.claimant) && idea_ref(&p.body) == Some(r.idea)
            };
            let by_claimant = pr.author.eq_ignore_ascii_case(&r.claimant);
            if implements(pr) || (by_claimant && !bootstrap) {
                continue;
            }
            if chain.iter().any(|d| prs.iter().any(|p| p.number == *d && implements(p))) {
                continue;
            }
            // The pull request's file list is its difference from main. For a
            // change built on another one that includes the other's files, so
            // its own difference is asked for: from the head of what it is
            // built on to its own head. If that cannot be had, the full list
            // is used and the warning says so.
            if files.is_none() {
                let on = chain.last().and_then(|d| prs.iter().find(|p| p.number == *d));
                let between = on.map(|p| self.forge.changed_between(&p.head_sha, &pr.head_sha));
                let own = !matches!(between, Some(Err(_)));
                let list = match between {
                    Some(Ok(list)) => Ok(list),
                    _ => self.forge.changed_files(pr.number),
                };
                match list {
                    Ok(list) => files = Some((list, own)),
                    Err(_) => {
                        // The files could not be read. Say so instead of
                        // staying silent: no warning would look like no overlap.
                        out.push(format!(
                            "the check for files reserved by claimed ideas ({}) could not be made, because the files this change touches could not be read; it is tried again on the next pass",
                            refs(&reservations.iter().map(|r| r.idea).collect::<Vec<_>>())
                        ));
                        break;
                    }
                }
            }
            let Some((files, own)) = files.as_ref() else { break };
            let hits = reserved_hits(&self.cfg.dir_prefix(), &r.patterns, files);
            if !hits.is_empty() {
                let mut warning = reservation_warning(r, &hits);
                if !*own {
                    warning.push_str(&format!(
                        " (the list may include files changed only by {}, which this change is built on)",
                        refs(chain)
                    ));
                }
                out.push(warning);
            }
        }
        out
    }

    /// Title and description follow the standard. Editing them needs no new push.
    fn format_gate(&self, pr: &PullRequest, depends: Option<&str>) -> (Gate, Option<(String, String, String)>) {
        format_check(&self.cfg.format, &pr.title, &pr.body, depends)
    }

    /// Verify `pr` on today's main with the approved changes ahead of it
    /// applied first. Dispatched once per main, head and line ahead; a longer
    /// line than `MAX_STACK` waits for the front to merge. Returns whether a
    /// run was dispatched now.
    fn dispatch_stacked(
        &mut self,
        pr: &PullRequest,
        prs: &[PullRequest],
        line: &Line,
        results: &[VerifyResult],
        min: Option<u64>,
        ledger: &mut Ledger,
        log: &mut Vec<String>,
    ) -> Result<bool> {
        if line.ahead.len() > MAX_STACK {
            return Ok(false);
        }
        let heads: Vec<String> = line
            .ahead
            .iter()
            .map(|n| prs.iter().find(|p| p.number == *n).map(|p| key(*n, &p.head_sha)).unwrap_or_default())
            .collect();
        let id = key(pr.number, &pr.head_sha);
        // A run with a waived minimum is another run: one made before the
        // waiver judged the commit with the configured minimum.
        let k = format!(
            "{}@{}@{}{}",
            id,
            line.main_tree,
            heads.join("+"),
            if min.is_some() { "@waived" } else { "" }
        );
        if ledger.dispatched.contains_key(&k) {
            return Ok(false);
        }
        // The latest run was stacked on a longer line whose front has merged
        // since, and nothing else has: it is building, or has built, exactly
        // today's main plus the line that is left, so it is not replaced. A
        // run that ended counts only if it left a result for that line from
        // the main it was dispatched on.
        let base = ledger.line_bases.get(&id);
        // If that result would land as another tree than it verified (the
        // branch contains commits of a change that was ahead), the run is of
        // no use for merging and is replaced at once.
        let of_line = |r: &&VerifyResult, was: &Vec<String>| r.stack == *was && Some(&r.base_sha) == base;
        let built = |was: &Vec<String>| results.iter().any(|r| of_line(&r, was) && lands_as_verified(r));
        let useless = |was: &Vec<String>| results.iter().any(|r| of_line(&r, was) && !lands_as_verified(r));
        // A run can be missed for a moment between two of its jobs, and
        // replacing it then throws away a build that is about to report. So a
        // run dispatched recently is taken to be going even when it is not seen.
        let prefix = format!("{id}@");
        let recent = ledger
            .dispatched
            .iter()
            .any(|(k, t)| k.starts_with(&prefix) && self.now - *t < Duration::minutes(RUN_GRACE_MINUTES));
        if let Some(was) = ledger.lines.get(&id) {
            if (line.active.contains(&pr.number) || recent || built(was)) && !useless(was) {
                if was.len() > heads.len() && was.ends_with(&heads) {
                    let gone = &was[..was.len() - heads.len()];
                    let in_order = gone.iter().all(|h| line.merged.contains_key(h));
                    let last = gone.last().and_then(|h| line.merged.get(h));
                    if in_order && last.map(String::as_str) == Some(line.main) {
                        return Ok(false);
                    }
                }
            }
        }
        if !self.start(pr, line.ahead, min, ledger, log)? {
            return Ok(false);
        }
        ledger.dispatched.insert(k, self.now);
        ledger.line_bases.insert(id.clone(), line.main.to_string());
        ledger.lines.insert(id, heads);
        log.push(if line.ahead.is_empty() {
            format!("#{}: verifying on today's main", pr.number)
        } else {
            format!("#{}: verifying on top of {:?}", pr.number, line.ahead)
        });
        Ok(true)
    }

    /// Approvals count only from vetted reviewers and leaders. A verdict is
    /// either a GitHub review or a comment `/approve <commit>` or
    /// `/request-changes`; the latest verdict of each reviewer wins.
    ///
    /// An approval names the commit in full: a prefix could also be the
    /// prefix of another commit pushed later. And it counts only if it was
    /// given after `since`, the moment the gatekeeper first judged the commit
    /// as it stands; the reviewers whose approval of the head came earlier
    /// are returned so that they can be told.
    fn review_gate(
        &mut self,
        pr: &PullRequest,
        roles: &RolesState,
        changed_files: usize,
        since: Option<DateTime<Utc>>,
    ) -> Result<(Gate, Vec<String>, Vec<String>)> {
        let r = &self.cfg.review;
        let needed = if changed_files > r.large_change_files { r.required_large } else { r.required };
        if needed == 0 {
            return Ok((Gate::new("review", GateStatus::Pass, "No review required"), Vec::new(), Vec::new()));
        }
        let mut early: Vec<String> = Vec::new();
        // Reviewers whose approval of the head is in a comment that was
        // edited after it was posted. Such a comment is not an approval:
        // people with write access can edit other people's comments, and
        // only the fact of an edit is visible, not who made it.
        let mut edited: Vec<String> = Vec::new();
        let mut verdicts = self.forge.reviews(pr.number)?;
        for c in self.forge.pr_comments(pr.number)? {
            let cmd = command(&c.body);
            match cmd.first().map(String::as_str) {
                Some("/approve") => {
                    let sha = cmd.get(1).map(String::as_str).unwrap_or("");
                    let current = names_commit(sha, &pr.head_sha);
                    if c.edited {
                        let own = c.user.eq_ignore_ascii_case(&pr.author);
                        let counts = roles.can_review(&c.user) && (!own || (roles.bootstrap && roles.is_leader(&c.user)));
                        if current && counts && !edited.iter().any(|e| e.eq_ignore_ascii_case(&c.user)) {
                            edited.push(c.user);
                        }
                        continue;
                    }
                    verdicts.push(ReviewInfo {
                        user: c.user,
                        state: "APPROVED".into(),
                        commit_id: if current { pr.head_sha.clone() } else { format!("other:{sha}") },
                        submitted_at: c.created_at,
                    });
                }
                Some("/request-changes") => verdicts.push(ReviewInfo {
                    user: c.user,
                    state: "CHANGES_REQUESTED".into(),
                    commit_id: pr.head_sha.clone(),
                    submitted_at: c.created_at,
                }),
                _ => {}
            }
        }
        verdicts.sort_by_key(|v| v.submitted_at);

        let mut latest: BTreeMap<String, &ReviewInfo> = BTreeMap::new();
        let mut unvetted = BTreeSet::new();
        for (i, v) in verdicts.iter().enumerate() {
            let own = v.user.eq_ignore_ascii_case(&pr.author);
            // While bootstrapping, the leaders run every agent from their own
            // account, so a leader's verdict counts on that account's changes.
            let bootstrap_own = own && roles.bootstrap && roles.is_leader(&v.user);
            let eligible = roles.can_review(&v.user) && (!own || bootstrap_own);
            let user = v.user.to_lowercase();
            if v.state == "APPROVED" && v.commit_id == pr.head_sha && since.is_none_or(|s| v.submitted_at <= s) {
                if eligible && !early.iter().any(|e| e.eq_ignore_ascii_case(&v.user)) {
                    early.push(v.user.clone());
                }
                continue;
            }
            match v.state.as_str() {
                // In that phase separate reviews by separate agents arrive
                // from the one account, so each approval is its own verdict;
                // a request for changes withdraws them all and blocks until
                // a later review approves.
                "APPROVED" if bootstrap_own => {
                    latest.remove(&user);
                    latest.insert(format!("{user}#{i}"), v);
                }
                "CHANGES_REQUESTED" if bootstrap_own => {
                    latest.retain(|k, _| !k.starts_with(&format!("{user}#")));
                    latest.insert(user, v);
                }
                "APPROVED" | "CHANGES_REQUESTED" if eligible => {
                    latest.insert(user, v);
                }
                "APPROVED" if !own => {
                    unvetted.insert(v.user.clone());
                }
                "DISMISSED" => {
                    latest.remove(&user);
                }
                _ => {}
            }
        }
        let mut approvals = Vec::new();
        let mut blocking = Vec::new();
        let mut outdated = 0;
        for v in latest.values() {
            if v.state == "CHANGES_REQUESTED" {
                blocking.push(v.user.clone());
            } else if v.commit_id != pr.head_sha {
                outdated += 1;
            } else {
                approvals.push(v.user.clone());
            }
        }
        // A reviewer who has approved again since is not told anything.
        early.retain(|e| !approvals.iter().any(|a| a.eq_ignore_ascii_case(e)));
        edited.retain(|e| !approvals.iter().any(|a| a.eq_ignore_ascii_case(e)));
        if !blocking.is_empty() {
            return Ok((
                Gate::new("review", GateStatus::Fail, format!("Changes requested by {}", blocking.join(", "))),
                early,
                edited,
            ));
        }
        if approvals.len() as u32 >= needed {
            let own = approvals.iter().any(|a| a.eq_ignore_ascii_case(&pr.author));
            let count = approvals.len();
            approvals.dedup();
            return Ok((Gate::new(
                "review",
                GateStatus::Pass,
                format!(
                    "Approved by {}{}",
                    approvals.join(", "),
                    if own {
                        format!(" (bootstrap phase: {count} separate review(s) by agents of the same account)")
                    } else {
                        String::new()
                    }
                ),
            ), early, edited));
        }
        let mut detail = format!("{} of {} approvals from vetted reviewers", approvals.len(), needed);
        if outdated > 0 {
            detail.push_str(&format!("; {outdated} approval(s) are for an older commit"));
        }
        if !early.is_empty() {
            detail.push_str(&format!(
                "; the approval(s) by {} were given before the bot's comment on this commit said what it is judged against (see above) and must be given again: `/approve {}`",
                early.join(", "),
                pr.head_sha
            ));
        }
        if !edited.is_empty() {
            detail.push_str(&format!(
                "; the `/approve` comment by {} was edited after it was posted and does not count; it has to be posted again as a new comment",
                edited.join(", ")
            ));
        }
        if !unvetted.is_empty() {
            detail.push_str(&format!(
                "; approvals from {} do not count because they are not vetted reviewers yet",
                unvetted.into_iter().collect::<Vec<_>>().join(", ")
            ));
        }
        Ok((Gate::new("review", GateStatus::Pending, detail), early, edited))
    }

    /// Slash commands in new comments: claims, triage, applications, roles, retries.
    #[allow(clippy::too_many_arguments)]
    fn commands(
        &mut self,
        ledger: &mut Ledger,
        roles: &mut RolesState,
        prs: &[PullRequest],
        issues: &mut [IssueInfo],
        attempts: &[Attempt],
        active: &BTreeSet<u64>,
        log: &mut Vec<String>,
    ) -> Result<()> {
        let lease = Duration::hours(self.cfg.ideas.lease_hours);
        let idea_label = self.cfg.ideas.label.clone();
        let comments = self.forge.comments_since(ledger.comments_seen_until)?;
        for c in &comments {
            if ledger.comments_seen_until.is_some_and(|t| c.created_at <= t) {
                continue;
            }
            let cmd = command(&c.body);
            let Some(name) = cmd.first().map(String::as_str) else { continue };
            let arg = |i: usize| cmd.get(i).map(|s| s.trim_start_matches('@').to_string()).unwrap_or_default();
            let n = c.issue;
            let issue = issues.iter().position(|i| i.number == n);
            let has = |label: &str| {
                issue.is_some_and(|i| issues[i].open && issues[i].labels.iter().any(|l| l == label))
            };
            let (queued, proposed) = (has(&idea_label), has(LABEL_PROPOSED));
            let application = roles
                .applications
                .iter()
                .position(|a| a.issue == n && a.status == ApplicationStatus::Open);
            let leader = roles.is_leader(&c.user);

            match name {
                "/claim" if queued => match ledger.claims.get(&n).map(|cl| cl.login.clone()) {
                    None => {
                        ledger.claims.insert(n, Claim { login: c.user.clone(), at: c.created_at });
                        let _ = self.forge.assign(n, &c.user, true);
                        self.forge.comment(
                            n,
                            &format!(
                                "Claimed by @{} until {}. Open a pull request whose description has the line `Idea: #{n}` before then, or the claim is released. Comment `/unclaim` to release it early.",
                                c.user,
                                (c.created_at + lease).format("%Y-%m-%d %H:%M UTC"),
                            ),
                        )?;
                        log.push(format!("idea #{n}: claimed by {}", c.user));
                    }
                    Some(h) if h.eq_ignore_ascii_case(&c.user) => {}
                    Some(h) => {
                        self.forge.comment(
                            n,
                            &format!("@{} this idea is already claimed by {h}. Pick another open idea.", c.user),
                        )?;
                    }
                },
                "/claim" if proposed => {
                    self.forge.comment(
                        n,
                        &format!("@{} this idea has not been accepted into the queue yet.", c.user),
                    )?;
                }
                "/unclaim" | "/release" => {
                    if ledger.claims.get(&n).is_some_and(|cl| cl.login.eq_ignore_ascii_case(&c.user)) {
                        ledger.claims.remove(&n);
                        let _ = self.forge.assign(n, &c.user, false);
                        self.forge.comment(n, "Claim released. This idea is open again.")?;
                        log.push(format!("idea #{n}: released"));
                    }
                }
                "/accept" if proposed => {
                    let i = issue.expect("a proposed idea is an issue");
                    let own = issues[i].author.eq_ignore_ascii_case(&c.user);
                    if roles.can_triage(&c.user) && (!own || leader) {
                        self.forge.set_label(n, LABEL_PROPOSED, false)?;
                        self.forge.set_label(n, &idea_label, true)?;
                        issues[i].labels.retain(|l| l != LABEL_PROPOSED);
                        issues[i].labels.push(idea_label.clone());
                        self.forge.comment(
                            n,
                            &format!("Accepted into the ideas queue by {}. Comment `/claim` to take it.", c.user),
                        )?;
                        log.push(format!("idea #{n}: accepted by {}", c.user));
                    }
                }
                "/decline" => {
                    if let (Some(a), true) = (application, leader) {
                        roles.applications[a].status = ApplicationStatus::Declined;
                        self.forge.comment(n, &format!("Declined by {}.", c.user))?;
                        self.forge.close_issue(n)?;
                        log.push(format!("application #{n}: declined"));
                    } else if proposed && roles.can_triage(&c.user) {
                        self.forge.comment(
                            n,
                            &format!("Declined by {}; see the comment above for the reason.", c.user),
                        )?;
                        self.forge.close_issue(n)?;
                        if let Some(i) = issue {
                            issues[i].open = false;
                        }
                        log.push(format!("idea #{n}: declined"));
                    }
                }
                "/apply" => {
                    let Some(role) = Role::parse(&arg(1)) else {
                        self.forge.comment(
                            n,
                            &format!(
                                "@{} say which role: `/apply reviewer`, `/apply ideator` or `/apply leader`.",
                                c.user
                            ),
                        )?;
                        continue;
                    };
                    if roles.has(&c.user, role)
                        || roles.applications.iter().any(|a| {
                            a.status == ApplicationStatus::Open
                                && a.role == role
                                && a.login.eq_ignore_ascii_case(&c.user)
                        })
                    {
                        continue;
                    }
                    let (merged, saved) = experience(&c.user, attempts);
                    let rc = &self.cfg.roles;
                    let min = match role {
                        Role::Reviewer => rc.reviewer_min_merged,
                        Role::Ideator => rc.ideator_min_merged,
                        Role::Leader => rc.leader_min_merged,
                    };
                    let age = self.now - self.forge.account_created(&c.user)?;
                    if merged < min || age < Duration::days(rc.min_account_age_days) {
                        self.forge.comment(
                            n,
                            &format!(
                                "@{} not yet: applying for {} needs {min} merged change(s) (you have {merged}) and an account at least {} days old. A leader can still grant the role directly.",
                                c.user,
                                role.name(),
                                rc.min_account_age_days
                            ),
                        )?;
                        continue;
                    }
                    let leaders = roles.leaders().max(1) as u32;
                    let needed = match role {
                        Role::Leader => rc.leader_endorsements,
                        _ => rc.endorsements,
                    }
                    .clamp(1, leaders);
                    let body = format!(
                        "@{} applies for the **{}** role.\n\n| | |\n|---|--:|\n| Merged changes | {merged} |\n| Tokens saved | {saved} |\n\nLeaders: look at this person's merged changes and reviews, then comment `/endorse` or `/decline`. {needed} endorsement(s) grant the role.",
                        c.user,
                        role.name()
                    );
                    let (number, url) = self.forge.create_issue(
                        &format!("Application: {} @{}", role.name(), c.user),
                        &body,
                        LABEL_APPLICATION,
                    )?;
                    roles.applications.push(Application {
                        issue: number,
                        login: c.user.clone(),
                        role,
                        opened_at: self.now,
                        status: ApplicationStatus::Open,
                        endorsements: Vec::new(),
                        needed,
                        merged,
                        saved,
                        url,
                    });
                    log.push(format!("{} applied for {}", c.user, role.name()));
                }
                "/endorse" => {
                    let (Some(a), true) = (application, leader) else { continue };
                    let app = &mut roles.applications[a];
                    if app.login.eq_ignore_ascii_case(&c.user)
                        || app.endorsements.iter().any(|e| e.eq_ignore_ascii_case(&c.user))
                    {
                        continue;
                    }
                    app.endorsements.push(c.user.clone());
                    if app.endorsements.len() as u32 >= app.needed {
                        app.status = ApplicationStatus::Granted;
                        let (login, role, by) = (app.login.clone(), app.role, app.endorsements.clone());
                        roles.grant(&login, role, by, self.now);
                        self.forge.comment(n, &format!("@{login} is now a {}.", role.name()))?;
                        self.forge.close_issue(n)?;
                        log.push(format!("{login} became {}", role.name()));
                    }
                }
                "/grant" | "/revoke" if leader => {
                    let (login, Some(role)) = (arg(1), Role::parse(&arg(2))) else {
                        self.forge.comment(
                            n,
                            "Usage: `/grant @login reviewer|ideator` or `/revoke @login reviewer|ideator|leader`.",
                        )?;
                        continue;
                    };
                    if login.is_empty() {
                        continue;
                    }
                    if name == "/grant" {
                        if role == Role::Leader {
                            self.forge.comment(
                                n,
                                "Leaders are added through `/apply leader` and endorsements, not granted directly.",
                            )?;
                        } else if !roles.has(&login, role) {
                            roles.grant(&login, role, vec![c.user.clone()], self.now);
                            self.forge.comment(
                                n,
                                &format!("@{login} is now a {}, granted by {}.", role.name(), c.user),
                            )?;
                            log.push(format!("{login} granted {}", role.name()));
                        }
                    } else {
                        let founding = self.cfg.roles.leaders.iter().any(|l| l.eq_ignore_ascii_case(&login));
                        if role == Role::Leader && founding {
                            self.forge.comment(
                                n,
                                "Founding leaders are set in the configuration and cannot be revoked by command.",
                            )?;
                        } else if roles.revoke(&login, role) {
                            self.forge.comment(
                                n,
                                &format!("{login} is no longer a {}, revoked by {}.", role.name(), c.user),
                            )?;
                            log.push(format!("{login} revoked {}", role.name()));
                        }
                    }
                }
                "/retry" => {
                    let Some(pr) = prs.iter().find(|p| p.number == n) else { continue };
                    if !(leader || pr.author.eq_ignore_ascii_case(&c.user)) || active.contains(&n) {
                        continue;
                    }
                    let k = format!("{}@retry", key(n, &pr.head_sha));
                    if ledger.dispatched.get(&k).is_some_and(|t| self.now - *t < Duration::minutes(30)) {
                        continue;
                    }
                    // A waiver for this commit holds for the retry too. The
                    // workflow itself applies what the change is built on.
                    let min = self.waiver(pr, roles)?.0.map(|_| WAIVED_MIN);
                    if self.start(pr, &[], min, ledger, log)? {
                        ledger.dispatched.insert(k, self.now);
                        log.push(format!("#{n}: retry requested by {}", c.user));
                    }
                }
                "/approve" => {
                    // The approval itself is read in the review gate. This
                    // only answers a reviewer who named the head commit by a
                    // short hash, which does not count, with the line to use.
                    let Some(pr) = prs.iter().find(|p| p.number == n) else { continue };
                    let sha = cmd.get(1).map(String::as_str).unwrap_or("");
                    let short_form = sha.len() >= 4
                        && sha.len() < pr.head_sha.len()
                        && pr.head_sha.to_lowercase().starts_with(&sha.to_lowercase());
                    if short_form && roles.can_review(&c.user) {
                        self.forge.comment(
                            n,
                            &format!(
                                "@{} that approval does not count: it names the commit by a short hash, which another commit could share. Name it in full: `/approve {}`.",
                                c.user, pr.head_sha
                            ),
                        )?;
                    }
                }
                "/waive-min" if leader => {
                    // The waiver itself is read from the pull request's
                    // comments in every pass, as approvals are; this only
                    // answers the comment.
                    let Some(pr) = prs.iter().find(|p| p.number == n) else { continue };
                    let sha = cmd.get(1).map(String::as_str).unwrap_or("");
                    let own = pr.author.eq_ignore_ascii_case(&c.user);
                    let head = pr.head_sha.as_str();
                    let primary = &self.cfg.score.primary;
                    if own && !roles.bootstrap {
                        self.forge.comment(
                            n,
                            &format!("@{} a leader cannot waive the minimum for a change from their own account.", c.user),
                        )?;
                    } else if !names_commit(sha, head) || cmd.len() < 3 {
                        self.forge.comment(
                            n,
                            &format!(
                                "@{} no waiver was recorded. Usage: `/waive-min {head} <reason>`: the head commit the waiver is for, written in full (a short hash is not accepted), then why this change is worth a review and a build below the minimum of {} {primary}. Like an approval, a waiver is for one commit.",
                                c.user, self.cfg.score.min_delta
                            ),
                        )?;
                    } else {
                        self.forge.comment(
                            n,
                            &format!(
                                "The minimum of {} {primary} is waived for commit {head} by {}: {}. The change must still lower {primary} by at least {WAIVED_MIN} and pass every other gate. A later push needs a new waiver.",
                                self.cfg.score.min_delta,
                                c.user,
                                cmd[2..].join(" ")
                            ),
                        )?;
                        log.push(format!("#{n}: minimum waived by {}", c.user));
                    }
                }
                _ => {}
            }
        }
        if let Some(last) = comments.iter().map(|c| c.created_at).max() {
            ledger.comments_seen_until = Some(ledger.comments_seen_until.map_or(last, |t| t.max(last)));
        }
        Ok(())
    }

    /// Triage newly filed ideas, expire claims, and list the queue.
    fn ideas(
        &mut self,
        ledger: &mut Ledger,
        roles: &RolesState,
        prs: &[PullRequest],
        issues: &mut [IssueInfo],
        attempts: &[Attempt],
        log: &mut Vec<String>,
    ) -> Result<Vec<Idea>> {
        let lease = Duration::hours(self.cfg.ideas.lease_hours);
        let idea_label = self.cfg.ideas.label.clone();
        let is = |i: &IssueInfo, l: &str| i.labels.iter().any(|x| x == l);

        // New issues titled "Idea: ...": check the format, then queue or hold for triage.
        for i in issues.iter_mut() {
            if !i.open
                || is(i, &idea_label)
                || is(i, LABEL_PROPOSED)
                || !i.title.trim_start().to_lowercase().starts_with("idea:")
            {
                continue;
            }
            let body = i.body.to_lowercase();
            let missing: Vec<&str> = IDEA_SECTIONS.iter().copied().filter(|s| !body.contains(s)).collect();
            // A large estimate has to be backed by a trial. The estimate is
            // the largest number in the Estimated saving section.
            let limit = self.cfg.ideas.trial_above;
            let estimate = largest_number(&section(&i.body, "estimat").unwrap_or_else(|| {
                // No heading: the paragraph in which the estimate is mentioned.
                i.body
                    .lines()
                    .skip_while(|l| !l.to_lowercase().contains("estimat"))
                    .take_while(|l| !l.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join("\n")
            }));
            let trial_missing = limit > 0
                && estimate > limit
                && section(&i.body, "trial").map_or(true, |s| s.trim().is_empty());
            if self.cfg.format.parse_idea_title(&i.title).is_none() || !missing.is_empty() || trial_missing {
                if ledger.format_notified.insert(i.number) {
                    let mut msg = format!(
                        "This is not in the ideas queue yet. The title must be `Idea: ` followed by {}.",
                        self.cfg.format.title_help()
                    );
                    if !missing.is_empty() {
                        msg.push_str(&format!(
                            " The description needs these sections: Targets, Technique, How to check, Estimated saving (missing: {}).",
                            missing.join(", ")
                        ));
                    }
                    if trial_missing {
                        msg.push_str(&format!(
                            " The estimated saving ({estimate}) is above {limit}, so the description also needs a **Trial** section: what was ported or measured on one file or case, and the saving measured there."
                        ));
                    }
                    msg.push_str(" Edit the issue and it will be picked up.");
                    self.forge.comment(i.number, &msg)?;
                }
                continue;
            }
            ledger.format_notified.remove(&i.number);
            if roles.can_triage(&i.author) {
                self.forge.set_label(i.number, &idea_label, true)?;
                i.labels.push(idea_label.clone());
                log.push(format!("idea #{}: queued", i.number));
            } else {
                self.forge.set_label(i.number, LABEL_PROPOSED, true)?;
                i.labels.push(LABEL_PROPOSED.to_string());
                self.forge.comment(
                    i.number,
                    "Thanks. This idea is proposed; a vetted ideator or a leader will check it and comment `/accept` to put it in the queue, or `/decline` with a reason.",
                )?;
                log.push(format!("idea #{}: proposed", i.number));
            }
        }

        let queued: BTreeSet<u64> =
            issues.iter().filter(|i| i.open && is(i, &idea_label)).map(|i| i.number).collect();

        // Expiry: a claim lapses unless its holder has an open pull request for it.
        let expired: Vec<(u64, String)> = ledger
            .claims
            .iter()
            .filter(|(n, cl)| {
                !queued.contains(n)
                    || (self.now - cl.at > lease
                        && !prs.iter().any(|p| {
                            idea_ref(&p.body) == Some(**n) && p.author.eq_ignore_ascii_case(&cl.login)
                        }))
            })
            .map(|(n, cl)| (*n, cl.login.clone()))
            .collect();
        for (n, login) in expired {
            ledger.claims.remove(&n);
            if queued.contains(&n) {
                let _ = self.forge.assign(n, &login, false);
                self.forge.comment(n, "The claim expired without a pull request. This idea is open again.")?;
                log.push(format!("idea #{n}: claim expired"));
            }
        }

        let mut ideas = Vec::new();
        for i in issues.iter().filter(|i| is(i, &idea_label) || is(i, LABEL_PROPOSED)) {
            let merged_all: Vec<&Attempt> = attempts
                .iter()
                .filter(|a| a.idea == Some(i.number) && a.status == AttemptStatus::Merged)
                .collect();
            let merged = merged_all.last().copied();
            let saved: i64 = merged_all.iter().filter_map(|a| a.delta).sum();
            let open_pr = prs.iter().find(|p| {
                idea_ref(&p.body) == Some(i.number)
                    && !attempts.iter().any(|a| a.pr == p.number && a.status == AttemptStatus::Merged)
            });
            let claim = ledger.claims.get(&i.number);
            // An idea is done when it is closed and something merged for it;
            // while it is open there may be more to do.
            let status = if merged.is_some() && !i.open {
                IdeaStatus::Done
            } else if !i.open {
                IdeaStatus::Closed
            } else if is(i, LABEL_PROPOSED) && !is(i, &idea_label) {
                IdeaStatus::Proposed
            } else if claim.is_some() || open_pr.is_some() {
                IdeaStatus::Claimed
            } else {
                IdeaStatus::Open
            };
            let parsed = self.cfg.format.parse_idea_title(&i.title);
            ideas.push(Idea {
                number: i.number,
                title: i.title.clone(),
                status,
                author: i.author.clone(),
                claimed_by: claim.map(|c| c.login.clone()).or(open_pr.map(|p| p.author.clone())),
                lease_expires: claim.map(|c| c.at + lease),
                pr: merged.map(|a| a.pr).or(open_pr.map(|p| p.number)),
                result_delta: merged.map(|_| saved),
                created_at: i.created_at,
                url: i.url.clone(),
                technique: parsed.as_ref().map(|p| p.0.clone()),
                scope: parsed.as_ref().map(|p| p.1.clone()),
            });
        }
        ideas.sort_by_key(|i| std::cmp::Reverse(i.number));
        Ok(ideas)
    }
}

fn pending_gates(format: &Gate, review: &Gate) -> Vec<Gate> {
    let mut gates = vec![format.clone()];
    gates.extend(
        GATES
            .iter()
            .filter(|g| !matches!(g.0, "review" | "format" | "quota"))
            .map(|g| Gate::new(g.0, GateStatus::Pending, "")),
    );
    gates.push(review.clone());
    gates
}

/// The single comment the gatekeeper keeps up to date on a pull request.
pub fn render_comment(cfg: &Config, a: &Attempt) -> String {
    let icon = |s: GateStatus| match s {
        GateStatus::Pass => "✅",
        GateStatus::Fail => "❌",
        GateStatus::Pending => "⏳",
        GateStatus::Skip => "➖",
    };
    let format_failed = a.gates.iter().any(|g| g.key == "format" && g.status == GateStatus::Fail);
    // In full: an approval or a waiver that names the commit by a prefix does not count.
    let head = a.head_sha.as_str();
    let on_top = refs(&a.depends);
    let after = " An approval counts only if it is given after this comment first appeared for the commit; an earlier one has to be given again.";
    let headline = match a.status {
        AttemptStatus::Verifying if !a.depends.is_empty() => {
            format!("Verification on top of {on_top} is pending or running.")
        }
        AttemptStatus::Verifying => "Verification is running.".to_string(),
        AttemptStatus::AwaitingReview if format_failed => {
            "The checks passed, but the title or description does not follow the standard. Edit the pull request; no new push is needed.".to_string()
        }
        AttemptStatus::AwaitingReview if !a.depends.is_empty() => format!(
            "All automatic gates passed on top of {on_top}. Waiting for a vetted reviewer, who reads this change as the difference from {on_top} and approves with a review or the comment `/approve {head}`. It merges only after {on_top} merged.{after}"
        ),
        AttemptStatus::AwaitingReview => format!(
            "All automatic gates passed. Waiting for a vetted reviewer, who approves with a review or the comment `/approve {head}`.{after}"
        ),
        AttemptStatus::Stale if !a.depends.is_empty() => {
            format!("Waiting for {on_top}, which this change is built on.")
        }
        AttemptStatus::Stale => "Main moved since this was verified.".to_string(),
        AttemptStatus::Conflict => "This branch conflicts with main.".to_string(),
        AttemptStatus::Merged => "Merged. Thank you.".to_string(),
        AttemptStatus::Rejected => "Not mergeable yet: a gate failed. Push a fix to try again.".to_string(),
        AttemptStatus::NeedsFix => {
            // Exactly what to fix, gate by gate. Nothing was rejected.
            let mut text = String::from(
                "Needs a fix. Nothing is rejected and nothing is recorded against this change; repair what is listed and the checks run again:\n",
            );
            let failed = a
                .gates
                .iter()
                .filter(|g| g.status == GateStatus::Fail && gate_on_fail(&g.key) == OnFail::Fix);
            let mut improves = false;
            for g in failed {
                improves |= g.key == "improves";
                text.push_str(&format!("\n- **{}**: {}", g.label, g.detail.as_deref().unwrap_or("failed").replace('\n', " ")));
            }
            text.push_str("\n\nThe title and description are fixed by editing the pull request; everything else by a push.");
            if improves && a.waiver.is_none() {
                text.push_str(&format!(
                    " A saving below the minimum of {} can be accepted only by a leader, with the comment `/waive-min {head} <reason>`.",
                    cfg.score.min_delta
                ));
            }
            text
        }
    };
    let mut s = format!("{COMMENT_MARKER}\n### Hillclimb gate\n\n{headline}\n\n");
    if !a.depends.is_empty() {
        s.push_str(&format!(
            "Depends on {}. It is judged as a change on top of {}: verified with {} applied first, reviewable now, and merged only after.\n\n",
            a.depends.iter().rev().map(|n| format!("#{n}")).collect::<Vec<_>>().join(", which depends on "),
            if a.depends.len() == 1 { "it" } else { "them" },
            if a.depends.len() == 1 { "it" } else { "them" },
        ));
        if !a.judged_on.is_empty() {
            let at: Vec<String> = a
                .judged_on
                .iter()
                .map(|k| match k.split_once('@') {
                    Some((n, sha)) => format!("#{n} at `{sha}`"),
                    None => k.clone(),
                })
                .collect();
            s.push_str(&format!(
                "It is judged on top of {}, which this branch must contain. If that is pushed to, this branch has to take the new commit in, and approvals given before do not count.\n\n",
                at.join(", ")
            ));
        }
    }
    if let Some(w) = &a.waiver {
        s.push_str(&format!(
            "The minimum of {} {} is waived for this commit by @{}: {}. It must still drop by at least {WAIVED_MIN}.\n\n",
            cfg.score.min_delta, cfg.score.primary, w.by, w.reason.replace('\n', " ")
        ));
    }
    for w in &a.warnings {
        s.push_str(&format!("Warning (does not block): {w}.\n\n"));
    }
    // What the numbers were measured against: main, or main with the changes
    // this one was verified on top of.
    let applied: Vec<String> = a
        .measured_on
        .iter()
        .map(|k| match k.split_once('@') {
            Some((n, sha)) => format!("#{n} (`{}`)", &sha[..sha.len().min(7)]),
            None => k.clone(),
        })
        .collect();
    if let (Some(m), Some(b)) = (a.metrics, a.base_metrics) {
        if applied.is_empty() {
            s.push_str("| Metric | Main | This change | Δ |\n|---|--:|--:|--:|\n");
        } else {
            s.push_str(&format!(
                "Metrics and static gates are measured against main with {} applied, not against main alone.\n\n",
                applied.join(", ")
            ));
            s.push_str(&format!(
                "| Metric | Main + {} | This change | Δ |\n|---|--:|--:|--:|\n",
                refs(&a.measured_on.iter().filter_map(|k| k.split('@').next()?.parse().ok()).collect::<Vec<u64>>())
            ));
        }
        for key in ["tokens", "loc", "decls", "files", "bytes"] {
            let (bv, mv) = (b.get(key).unwrap_or(0) as i64, m.get(key).unwrap_or(0) as i64);
            let name = if key == cfg.score.primary { format!("**{key}**") } else { key.to_string() };
            s.push_str(&format!("| {name} | {bv} | {mv} | {:+} |\n", mv - bv));
        }
        s.push('\n');
    }
    s.push_str("| | Gate | Detail |\n|---|---|---|\n");
    for g in &a.gates {
        s.push_str(&format!(
            "| {} | {} | {} |\n",
            icon(g.status),
            g.label,
            g.detail.as_deref().unwrap_or("").replace('|', "\\|").replace('\n', " ")
        ));
    }
    if let Some(url) = &a.run_url {
        s.push_str(&format!("\n[Verification run]({url})\n"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        toml::from_str(
            r#"
            [project]
            slug = "t"
            title = "T"
            repo = "o/r"
            dir = "proj"
            [roles]
            leaders = ["lead"]
            reviewer_min_merged = 1
            "#,
        )
        .unwrap()
    }

    fn t(h: i64) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-01-10T00:00:00Z").unwrap().with_timezone(&Utc) + Duration::hours(h)
    }

    #[derive(Default)]
    struct Mock {
        main: String,
        prs: Vec<PullRequest>,
        results: BTreeMap<String, VerifyResult>,
        extra_results: Vec<VerifyResult>,
        already_merged: Vec<(PullRequest, String, DateTime<Utc>)>,
        main_ok: Option<bool>,
        main_errors: u32,
        stacks: Vec<Vec<u64>>,
        reviews: BTreeMap<u64, Vec<ReviewInfo>>,
        pr_comments: BTreeMap<u64, Vec<CommentInfo>>,
        young: BTreeSet<String>,
        active: BTreeSet<u64>,
        /// Active runs that started before main last changed.
        obsolete: BTreeSet<u64>,
        issues: Vec<IssueInfo>,
        comments: Vec<CommentInfo>,
        merged: Vec<u64>,
        cancelled: BTreeSet<u64>,
        dispatched: Vec<u64>,
        statuses: BTreeMap<String, CommitState>,
        posted: Vec<(u64, String)>,
        labels: Vec<(u64, String, bool)>,
        closed: Vec<u64>,
        created: Vec<String>,
        /// Paths each pull request changes.
        files: BTreeMap<u64, Vec<String>>,
        /// Paths that differ between two commits, where that is known.
        between: BTreeMap<(String, String), Vec<String>>,
        /// Verifications dispatched with a waived minimum: (pull request, minimum).
        waived: Vec<(u64, u64)>,
        /// Text of the latest commit status of each head.
        status_text: BTreeMap<String, String>,
        /// (head, commit) pairs where the head does not contain the commit.
        /// A branch contains whatever is not listed here.
        lacks: BTreeSet<(String, String)>,
        /// Names of forge calls that fail: `dispatch_verify_min`,
        /// `changed_files`, `changed_between`, `contains`, `comment`.
        failing: BTreeSet<&'static str>,
        /// The forge's clock when the current pass started; the helpers that
        /// run a pass set it. The gate comment of a pass is written, and so
        /// stamped by the forge, `COMMENT_DELAY` later.
        clock: Option<DateTime<Utc>>,
        /// The gate comment on each pull request as it stands, with the
        /// forge's time for it. As on GitHub, writing the same text again
        /// changes nothing: the comment keeps the time it was last written.
        gate_comments: BTreeMap<u64, (String, DateTime<Utc>)>,
    }

    /// How long after the start of a pass the mock forge stamps the gate
    /// comment: the pass reads pull requests, results and reviews first.
    const COMMENT_DELAY: i64 = 30;

    impl Mock {
        fn fail(&self, call: &'static str) -> Result<()> {
            if self.failing.contains(call) {
                anyhow::bail!("{call}: the forge did not answer");
            }
            Ok(())
        }
    }

    impl Forge for Mock {
        fn main_head(&mut self) -> Result<String> {
            Ok(self.main.clone())
        }
        fn open_prs(&mut self) -> Result<Vec<PullRequest>> {
            Ok(self.prs.clone())
        }
        fn main_tree(&mut self) -> Result<String> {
            Ok(format!("tree:{}", self.main))
        }
        fn main_verifies(&mut self, _: &str) -> Result<MainBuild> {
            Ok(match self.main_ok {
                Some(true) => MainBuild::Passed,
                Some(false) => MainBuild::Failed,
                None if self.main_errors > 0 => MainBuild::Errored(self.main_errors),
                None => MainBuild::Pending,
            })
        }
        fn merged_prs(&mut self) -> Result<Vec<(PullRequest, String, DateTime<Utc>)>> {
            Ok(self.already_merged.clone())
        }
        fn verify_results(&mut self, pr: u64, head: &str) -> Result<Vec<VerifyResult>> {
            let mut all: Vec<VerifyResult> =
                self.extra_results.iter().rev().filter(|r| r.pr == pr && r.head_sha == head).cloned().collect();
            all.extend(self.results.get(&key(pr, head)).cloned());
            Ok(all)
        }
        fn active_verifies(&mut self) -> Result<BTreeSet<u64>> {
            Ok(self.active.clone())
        }
        fn current_verifies(&mut self) -> Result<BTreeSet<u64>> {
            Ok(self.active.difference(&self.obsolete).copied().collect())
        }
        fn dispatch_verify(&mut self, pr: u64, stack: &[u64]) -> Result<()> {
            self.dispatched.push(pr);
            self.stacks.push(stack.to_vec());
            Ok(())
        }
        fn dispatch_verify_min(&mut self, pr: u64, stack: &[u64], min_delta: u64) -> Result<()> {
            self.fail("dispatch_verify_min")?;
            self.waived.push((pr, min_delta));
            self.dispatch_verify(pr, stack)
        }
        fn changed_files(&mut self, pr: u64) -> Result<Vec<String>> {
            self.fail("changed_files")?;
            Ok(self.files.get(&pr).cloned().unwrap_or_default())
        }
        fn changed_between(&mut self, base: &str, head: &str) -> Result<Vec<String>> {
            self.fail("changed_between")?;
            self.between.get(&(base.to_string(), head.to_string())).cloned().ok_or_else(|| anyhow::anyhow!("not known"))
        }
        fn contains(&mut self, head: &str, ancestor: &str) -> Result<bool> {
            self.fail("contains")?;
            Ok(!self.lacks.contains(&(head.to_string(), ancestor.to_string())))
        }
        fn cancel_verifies(&mut self, prs: &BTreeSet<u64>) -> Result<()> {
            self.cancelled.extend(prs);
            Ok(())
        }
        fn reviews(&mut self, pr: u64) -> Result<Vec<ReviewInfo>> {
            Ok(self.reviews.get(&pr).cloned().unwrap_or_default())
        }
        fn pr_comments(&mut self, pr: u64) -> Result<Vec<CommentInfo>> {
            Ok(self.pr_comments.get(&pr).cloned().unwrap_or_default())
        }
        fn account_created(&mut self, login: &str) -> Result<DateTime<Utc>> {
            Ok(if self.young.contains(login) { t(-24) } else { t(-24 * 400) })
        }
        fn set_status(&mut self, sha: &str, state: CommitState, text: &str) -> Result<()> {
            self.statuses.insert(sha.into(), state);
            self.status_text.insert(sha.into(), text.into());
            Ok(())
        }
        fn upsert_comment(&mut self, pr: u64, body: &str) -> Result<DateTime<Utc>> {
            if let Some((was, at)) = self.gate_comments.get(&pr) {
                if was.trim() == body.trim() {
                    return Ok(*at);
                }
            }
            let at = self.clock.unwrap_or(t(0)) + Duration::seconds(COMMENT_DELAY);
            self.gate_comments.insert(pr, (body.to_string(), at));
            Ok(at)
        }
        fn set_label(&mut self, issue: u64, label: &str, present: bool) -> Result<()> {
            self.labels.push((issue, label.into(), present));
            Ok(())
        }
        fn merge(&mut self, pr: &PullRequest, _: &str) -> Result<String> {
            self.merged.push(pr.number);
            // Main's commit, and with it its tree, depends on what merged:
            // the pull request and the commit that was its head.
            self.main = landed_at(pr.number, &pr.head_sha);
            Ok(self.main.clone())
        }
        fn issues(&mut self) -> Result<Vec<IssueInfo>> {
            Ok(self.issues.clone())
        }
        fn comments_since(&mut self, _: Option<DateTime<Utc>>) -> Result<Vec<CommentInfo>> {
            Ok(self.comments.clone())
        }
        fn assign(&mut self, _: u64, _: &str, _: bool) -> Result<()> {
            Ok(())
        }
        fn comment(&mut self, issue: u64, body: &str) -> Result<()> {
            self.fail("comment")?;
            self.posted.push((issue, body.into()));
            Ok(())
        }
        fn create_issue(&mut self, title: &str, _: &str, _: &str) -> Result<(u64, String)> {
            self.created.push(title.into());
            Ok((900 + self.created.len() as u64, "url".into()))
        }
        fn close_issue(&mut self, issue: u64) -> Result<()> {
            self.closed.push(issue);
            Ok(())
        }
    }

    fn pr(n: u64, author: &str) -> PullRequest {
        PullRequest {
            number: n,
            title: format!("shorten(Paper/Tree): a shorter proof number {n}"),
            author: author.into(),
            body: "Shorter.\n\nIdea: none".into(),
            head_sha: head(n),
            base_ref: "main".into(),
            created_at: t(-1),
            url: format!("https://x/pull/{n}"),
            draft: false,
            mergeable: Some(true),
            labels: vec![],
        }
    }

    /// The head commit of pull request `n` in these tests: a full hash, 40
    /// hexadecimal characters, that starts `<n>abcdef` (for #1:
    /// `1abcdef111…`). Other commits in the tests are written out in the
    /// same form, with another start.
    fn head(n: u64) -> String {
        let d = (n % 10).to_string();
        format!("{d}abcdef{}", d.repeat(33))
    }

    /// Main's commit after pull request `n` merged with head `sha`.
    fn landed_at(n: u64, sha: &str) -> String {
        format!("merge{n}@{sha}")
    }

    /// Main's commit after pull request `n` merged with its usual head.
    fn landed(n: u64) -> String {
        landed_at(n, &head(n))
    }

    /// Main's tree after pull request `n` merged with its usual head.
    fn after(n: u64) -> String {
        format!("tree:{}", landed(n))
    }

    /// The same result for another head commit of the pull request: what
    /// it leaves on main is another tree.
    fn at_head(mut r: VerifyResult, sha: &str) -> VerifyResult {
        let tree = Some(format!("tree:{}", landed_at(r.pr, sha)));
        if r.landing_tree == r.merged_tree {
            r.landing_tree = tree.clone();
        }
        if r.merged_tree.is_some() {
            r.merged_tree = tree;
        }
        r.head_sha = sha.into();
        r
    }

    fn result(n: u64, base: &str, tokens: u64, pass: bool) -> VerifyResult {
        let gate = |k: &str, ok: bool| Gate::new(k, if ok { GateStatus::Pass } else { GateStatus::Fail }, "");
        VerifyResult {
            pr: n,
            head_sha: head(n),
            base_sha: base.into(),
            base_tree: None,
            merged_tree: None,
            stack: vec![],
            landing_tree: None,
            min_delta: None,
            metrics: Metrics { tokens, ..Default::default() },
            base_metrics: Metrics { tokens: 1000, ..Default::default() },
            gates: vec![
                gate("frozen", true),
                gate("scope", true),
                gate("deny", true),
                gate("improves", pass),
                gate("compiles", true),
                gate("same_theorems", true),
            ],
            changed_files: 1,
            top_files: vec![],
            run_url: None,
        }
    }

    /// When reviews and comments are made in these tests unless a test says
    /// otherwise: a minute after the pass at `t(0)`, so after the gatekeeper
    /// has first judged a commit it sees in that pass. (An approval given
    /// before that does not count; `approve_at` makes one.)
    fn soon(h: i64) -> DateTime<Utc> {
        t(h) + Duration::minutes(1)
    }

    fn approve(user: &str, commit: &str) -> ReviewInfo {
        ReviewInfo { user: user.into(), state: "APPROVED".into(), commit_id: commit.into(), submitted_at: soon(0) }
    }

    fn approve_at(user: &str, commit: &str, at: DateTime<Utc>) -> ReviewInfo {
        ReviewInfo { submitted_at: at, ..approve(user, commit) }
    }

    /// Roles as they stand in most tests: `lead` leads, `bob` reviews.
    fn roles() -> RolesState {
        let mut r = RolesState::default();
        r.grant("bob", Role::Reviewer, vec!["lead".into()], t(-100));
        r
    }

    /// One pass at `now`, and nothing else: a commit the ledger has no
    /// record of is judged for the first time in it.
    fn pass(m: &mut Mock, ledger: &mut Ledger, past: &[Attempt], now: DateTime<Utc>) -> Outcome {
        m.clock = Some(now);
        Gatekeeper { cfg: &cfg(), forge: m, now }.run(ledger, &mut roles(), past).unwrap()
    }

    /// What an earlier pass leaves in the ledger for every commit that is
    /// open now and has no record: judged an hour before `t(0)`, on top of
    /// what its description names. Reviews and comments in these tests are
    /// stamped later than that, as they are in reality: a reviewer acts
    /// after the pass that first judged the commit and wrote the bot's
    /// comment, and the approval is counted in a later pass. Tests about the
    /// first judgement itself use `pass`, which does not do this.
    fn seen_before(m: &Mock, ledger: &mut Ledger, past: &[Attempt]) {
        let open: Vec<&PullRequest> = m.prs.iter().filter(|p| p.base_ref == "main" && !p.draft).collect();
        let bodies: BTreeMap<u64, &str> = open.iter().map(|p| (p.number, p.body.as_str())).collect();
        let landed: BTreeSet<u64> =
            past.iter().filter(|a| a.status == AttemptStatus::Merged).map(|a| a.pr).collect();
        for p in &open {
            let id = key(p.number, &p.head_sha);
            if ledger.judged.contains_key(&id) {
                continue;
            }
            let dep = dependency(p.number, &p.body, &bodies, &landed, "main");
            let on = dep
                .chain
                .iter()
                .filter_map(|n| open.iter().find(|o| o.number == *n).map(|o| key(*n, &o.head_sha)))
                .collect();
            ledger.judged.insert(id, Judged { at: Some(t(-1)), names: dep.direct, on, contains: None, has_merged: None });
        }
    }

    fn run_with(m: &mut Mock, c: &Config, ledger: &mut Ledger, roles: &mut RolesState, past: &[Attempt]) -> Outcome {
        seen_before(m, ledger, past);
        m.clock = Some(t(0));
        Gatekeeper { cfg: c, forge: m, now: t(0) }.run(ledger, roles, past).unwrap()
    }

    fn run(m: &mut Mock, ledger: &mut Ledger, past: &[Attempt]) -> Outcome {
        run_with(m, &cfg(), ledger, &mut roles(), past)
    }

    /// As `run`, at another time than `t(0)`.
    fn run_at(m: &mut Mock, ledger: &mut Ledger, past: &[Attempt], now: DateTime<Utc>) -> Outcome {
        seen_before(m, ledger, past);
        pass(m, ledger, past, now)
    }

    fn base() -> Mock {
        Mock { main: "m0".into(), ..Default::default() }
    }

    /// One green pull request by alice, verified on current main.
    fn green() -> Mock {
        let mut m = base();
        m.prs.push(pr(1, "alice"));
        m.results.insert(key(1, &head(1)), result(1, "m0", 900, true));
        m
    }

    fn says(issue: u64, user: &str, body: &str, h: i64) -> CommentInfo {
        CommentInfo { issue, user: user.into(), body: body.into(), created_at: soon(h), edited: false }
    }

    fn merged_attempt(author: &str) -> Attempt {
        let mut m = green();
        m.prs[0].author = author.into();
        m.reviews.insert(1, vec![approve("bob", &head(1))]);
        run(&mut m, &mut Ledger::default(), &[]).decided.remove(0)
    }

    #[test]
    fn green_and_reviewed_merges() {
        let mut m = green();
        m.reviews.insert(1, vec![approve("bob", &head(1))]);
        let out = run(&mut m, &mut Ledger::default(), &[]);
        assert_eq!(m.merged, [1]);
        assert_eq!(out.decided[0].status, AttemptStatus::Merged);
        assert_eq!(out.decided[0].delta, Some(-100));
        assert_eq!(out.decided[0].technique.as_deref(), Some("shorten"));
        assert_eq!(out.decided[0].scope.as_deref(), Some("Paper/Tree"));
        assert_eq!(out.current.unwrap().metrics.tokens, 900);
        assert_eq!(m.statuses[&head(1)], CommitState::Success);
    }

    #[test]
    fn green_without_review_waits() {
        let mut m = green();
        let out = run(&mut m, &mut Ledger::default(), &[]);
        assert!(m.merged.is_empty());
        assert_eq!(out.open[0].status, AttemptStatus::AwaitingReview);
    }

    #[test]
    fn only_vetted_reviewers_count() {
        // Self-approval, an approval of an older commit, and an approval by
        // someone who is not a reviewer: none of them merges.
        let mut m = green();
        m.reviews.insert(1, vec![approve("Alice", &head(1)), approve("bob", "older"), approve("mallory", &head(1))]);
        let out = run(&mut m, &mut Ledger::default(), &[]);
        assert!(m.merged.is_empty());
        let review = out.open[0].gates.iter().find(|g| g.key == "review").unwrap();
        assert!(review.detail.as_deref().unwrap().contains("mallory"));
    }

    #[test]
    fn a_reviewer_cannot_approve_their_own_change() {
        let mut m = green();
        m.prs[0].author = "bob".into();
        m.reviews.insert(1, vec![approve("bob", &head(1))]);
        m.pr_comments.insert(1, vec![says(1, "bob", "/approve 1abcdef111111111111111111111111111111111", 0)]);
        run(&mut m, &mut Ledger::default(), &[]);
        assert!(m.merged.is_empty());
    }

    #[test]
    fn comment_approval_needs_the_current_commit() {
        let mut m = green();
        m.pr_comments.insert(1, vec![says(1, "bob", "Looks right.\n/approve 0123456", 0)]);
        run(&mut m, &mut Ledger::default(), &[]);
        assert!(m.merged.is_empty());

        // A prefix of the head commit is not the commit: the author could
        // push another commit with the same prefix and inherit the approval.
        // The reviewer is told the line to use.
        for short in ["1abcdef", "1abcdef111"] {
            let approval = says(1, "bob", &format!("Looks right.\n/approve {short}"), 0);
            m.pr_comments.insert(1, vec![approval.clone()]);
            m.comments = vec![approval];
            m.posted.clear();
            run(&mut m, &mut Ledger::default(), &[]);
            assert!(m.merged.is_empty(), "{short}");
            assert!(m.posted.iter().any(|(n, b)| *n == 1 && b.contains("Name it in full: `/approve 1abcdef111111111111111111111111111111111`")), "{short}");
        }
        // Someone whose approval would not count anyway gets no answer.
        m.comments = vec![says(1, "mallory", "/approve 1abcdef", 0)];
        m.posted.clear();
        run(&mut m, &mut Ledger::default(), &[]);
        assert!(m.posted.is_empty());

        m.pr_comments.insert(1, vec![says(1, "bob", "Looks right.\n/approve 1abcdef111111111111111111111111111111111", 0)]);
        m.comments.clear();
        run(&mut m, &mut Ledger::default(), &[]);
        assert_eq!(m.merged, [1]);
        assert!(m.posted.is_empty());
    }

    #[test]
    fn a_commit_that_shares_a_prefix_does_not_inherit_an_approval_or_a_waiver() {
        // Approved and waived by prefix for one commit; the author pushes
        // another commit whose hash starts the same way.
        let mut m = small();
        m.prs[0].head_sha = "1abcdef999999999999999999999999999999999".into();
        m.extra_results.push(at_head(waived_result(&head(1)), "1abcdef999999999999999999999999999999999"));
        m.pr_comments.insert(
            1,
            vec![says(1, "bob", "/approve 1abcdef", 0), says(1, "lead", "/waive-min 1abcdef small but right", 0)],
        );
        let out = run(&mut m, &mut Ledger::default(), &[]);
        assert!(m.merged.is_empty());
        assert!(out.open[0].waiver.is_none());
        assert_eq!(gate_of(&out.open[0], "review").status, GateStatus::Pending);
    }

    #[test]
    fn bootstrap_lets_a_leader_review_changes_from_the_leaders_account() {
        let mut m = green();
        m.prs[0].author = "lead".into();
        m.pr_comments.insert(1, vec![says(1, "lead", "/approve 1abcdef111111111111111111111111111111111", 0)]);
        run(&mut m, &mut Ledger::default(), &[]);
        assert!(m.merged.is_empty(), "not allowed outside bootstrap");

        let mut c = cfg();
        c.roles.bootstrap = true;
        let out = run_with(&mut m, &c, &mut Ledger::default(), &mut roles(), &[]);
        assert_eq!(m.merged, [1]);
        let review = out.decided[0].gates.iter().find(|g| g.key == "review").unwrap();
        assert!(review.detail.as_deref().unwrap().contains("bootstrap"));
    }

    #[test]
    fn large_changes_need_two_reviews_also_while_bootstrapping() {
        let mut c = cfg();
        c.roles.bootstrap = true;
        let mut m = green();
        m.prs[0].author = "lead".into();
        m.results.get_mut(&key(1, &head(1))).unwrap().changed_files = 30;
        m.pr_comments.insert(1, vec![says(1, "lead", "/approve 1abcdef111111111111111111111111111111111", 0)]);
        run_with(&mut m, &c, &mut Ledger::default(), &mut roles(), &[]);
        assert!(m.merged.is_empty(), "one review is not enough for a large change");

        // A second, separate review arrives; a request for changes in between would reset the count.
        m.pr_comments.insert(
            1,
            vec![
                says(1, "lead", "/approve 1abcdef111111111111111111111111111111111", 0),
                says(1, "lead", "/request-changes", 1),
                says(1, "lead", "/approve 1abcdef111111111111111111111111111111111", 2),
            ],
        );
        run_with(&mut m, &c, &mut Ledger::default(), &mut roles(), &[]);
        assert!(m.merged.is_empty());
        m.pr_comments.get_mut(&1).unwrap().push(says(1, "lead", "/approve 1abcdef111111111111111111111111111111111", 3));
        run_with(&mut m, &c, &mut Ledger::default(), &mut roles(), &[]);
        assert_eq!(m.merged, [1]);
    }

    #[test]
    fn changes_requested_blocks_even_with_an_approval() {
        let mut m = green();
        m.reviews.insert(1, vec![approve("bob", &head(1))]);
        m.pr_comments.insert(1, vec![says(1, "lead", "/request-changes names are unreadable", 1)]);
        run(&mut m, &mut Ledger::default(), &[]);
        assert!(m.merged.is_empty());
        // Someone who is not a reviewer cannot block.
        m.pr_comments.insert(1, vec![says(1, "mallory", "/request-changes", 1)]);
        run(&mut m, &mut Ledger::default(), &[]);
        assert_eq!(m.merged, [1]);
    }

    #[test]
    fn nonstandard_title_or_missing_idea_line_blocks_the_merge() {
        for (title, body) in [
            ("Remove a lemma", "Idea: none"),
            ("shorten(Paper/Tree): a shorter proof number 1", "no idea line"),
        ] {
            let mut m = green();
            m.prs[0].title = title.into();
            m.prs[0].body = body.into();
            m.reviews.insert(1, vec![approve("bob", &head(1))]);
            let out = run(&mut m, &mut Ledger::default(), &[]);
            assert!(m.merged.is_empty());
            assert_eq!(out.open[0].status, AttemptStatus::NeedsFix);
            assert_eq!(m.statuses[&head(1)], CommitState::Failure);
            assert!(m.status_text[&head(1)].starts_with("Needs a fix"));
            assert!(m.labels.contains(&(1, LABEL_NEEDS_FIX.into(), true)));
            assert_eq!(out.open[0].gates[0].key, "format");
            assert_eq!(out.open[0].gates[0].status, GateStatus::Fail);
            assert!(out.decided.is_empty(), "a format problem is fixed by editing, not logged as a rejection");
        }
    }

    /// The same result with one gate failed.
    fn failing(mut r: VerifyResult, gate: &str) -> VerifyResult {
        for g in r.gates.iter_mut() {
            if g.key == gate {
                g.status = GateStatus::Fail;
            } else if g.status == GateStatus::Fail {
                g.status = GateStatus::Pass;
            }
        }
        r
    }

    #[test]
    fn failed_gate_rejects_once() {
        // A verdict: the build fails. (A score that does not improve enough
        // is no longer a rejection; see `a_fixable_failure_…`.)
        let mut m = base();
        m.prs.push(pr(1, "alice"));
        m.results.insert(key(1, &head(1)), failing(result(1, "m0", 900, true), "compiles"));
        m.reviews.insert(1, vec![approve("bob", &head(1))]);
        let out = run(&mut m, &mut Ledger::default(), &[]);
        assert!(m.merged.is_empty());
        assert_eq!(out.decided.len(), 1);
        assert_eq!(out.decided[0].status, AttemptStatus::Rejected);
        assert!(out.open.is_empty());
        let again = run(&mut m, &mut Ledger::default(), &out.decided);
        assert!(again.decided.is_empty());
        assert!(again.open.is_empty());
    }

    #[test]
    fn skipped_or_missing_checks_never_merge() {
        for mutate in [0, 1] {
            let mut m = green();
            let r = m.results.get_mut(&key(1, &head(1))).unwrap();
            if mutate == 0 {
                r.gates.iter_mut().find(|g| g.key == "same_theorems").unwrap().status = GateStatus::Skip;
            } else {
                r.gates.retain(|g| g.key != "compiles");
            }
            m.reviews.insert(1, vec![approve("bob", &head(1))]);
            let out = run(&mut m, &mut Ledger::default(), &[]);
            assert!(m.merged.is_empty());
            assert_eq!(out.decided[0].status, AttemptStatus::Rejected);
        }
    }

    #[test]
    fn second_ready_pr_goes_stale_after_first_merges_and_is_reverified() {
        let mut m = base();
        m.prs.push(pr(1, "alice"));
        m.prs.push(pr(2, "dave"));
        for n in [1, 2] {
            m.results.insert(key(n, &head(n)), result(n, "m0", 900, true));
            m.reviews.insert(n, vec![approve("bob", &head(n))]);
        }
        let mut ledger = Ledger::default();
        let out = run(&mut m, &mut ledger, &[]);
        assert_eq!(m.merged, [1]);
        assert_eq!(out.open[0].status, AttemptStatus::Stale);
        assert_eq!(m.dispatched, [2]);
        // A second pass does not dispatch again for the same main.
        m.prs.remove(0);
        run(&mut m, &mut ledger, &out.decided);
        assert_eq!(m.dispatched, [2]);
        assert_eq!(m.merged, [1]);
    }

    #[test]
    fn stale_unreviewed_pr_is_not_reverified() {
        let mut m = base();
        m.prs.push(pr(1, "alice"));
        m.results.insert(key(1, &head(1)), result(1, "old", 900, true));
        let out = run(&mut m, &mut Ledger::default(), &[]);
        assert_eq!(out.open[0].status, AttemptStatus::Stale);
        assert!(m.dispatched.is_empty());
    }

    #[test]
    fn missing_result_is_dispatched_after_a_wait_unless_running() {
        let mut m = base();
        m.prs.push(pr(1, "alice"));
        let mut ledger = Ledger::default();
        run(&mut m, &mut ledger, &[]);
        assert_eq!(m.dispatched, [1]);
        run(&mut m, &mut ledger, &[]);
        assert_eq!(m.dispatched, [1]);

        let mut m = base();
        m.prs.push(pr(1, "alice"));
        m.active.insert(1);
        run(&mut m, &mut Ledger::default(), &[]);
        assert!(m.dispatched.is_empty());
    }

    #[test]
    fn conflict_is_reported_not_redispatched() {
        let mut m = base();
        let mut p = pr(1, "alice");
        p.mergeable = Some(false);
        m.prs.push(p);
        let out = run(&mut m, &mut Ledger::default(), &[]);
        assert_eq!(out.open[0].status, AttemptStatus::Conflict);
        assert!(m.dispatched.is_empty());
    }

    #[test]
    fn retry_is_for_the_author_or_a_leader_and_rate_limited() {
        let mut m = green();
        m.prs[0].created_at = t(0);
        m.comments = vec![says(1, "mallory", "/retry", 0)];
        run(&mut m, &mut Ledger::default(), &[]);
        assert!(m.dispatched.is_empty());
        let mut ledger = Ledger::default();
        m.comments = vec![says(1, "alice", "/retry", 0)];
        run(&mut m, &mut ledger, &[]);
        assert_eq!(m.dispatched, [1]);
        m.comments = vec![says(1, "alice", "/retry", 1)];
        run(&mut m, &mut ledger, &[]);
        assert_eq!(m.dispatched, [1]);
    }

    /// Two approved changes, both verified on an older main.
    fn two_stale() -> Mock {
        let mut m = base();
        for n in [1, 2] {
            m.prs.push(pr(n, "alice"));
            m.results.insert(key(n, &head(n)), result(n, "old", 900, true));
            m.reviews.insert(n, vec![approve("bob", &head(n))]);
        }
        m
    }

    #[test]
    fn approved_before_its_first_result_joins_the_line_at_once() {
        // #1 has a stale result, #2 and #3 were approved while their first runs are going.
        let mut m = two_stale();
        m.results.remove(&key(2, &head(2)));
        m.prs.push(pr(3, "alice"));
        m.reviews.insert(3, vec![approve("bob", &head(3))]);
        m.active.extend([1, 2, 3]);
        m.obsolete.extend([1, 2]);
        let mut ledger = Ledger::default();
        run(&mut m, &mut ledger, &[]);
        assert_eq!(m.stacks, [vec![], vec![1], vec![1, 2]], "one line, in order, without waiting for older runs");
        run(&mut m, &mut ledger, &[]);
        assert_eq!(m.dispatched.len(), 3, "once per main and line");

        // The whole line was verified as a train and merges in one pass.
        m.extra_results.push(on_tree(1, "tree:m0", &[], true));
        m.extra_results.push(on_tree(2, &after(1), &["1@1abcdef111111111111111111111111111111111"], true));
        m.extra_results.push(on_tree(3, &after(2), &["1@1abcdef111111111111111111111111111111111", "2@2abcdef222222222222222222222222222222222"], true));
        run(&mut m, &mut ledger, &[]);
        assert_eq!(m.merged, [1, 2, 3]);
    }

    #[test]
    fn approved_change_alone_on_todays_main_is_not_verified_twice() {
        let mut m = base();
        m.prs.push(pr(1, "alice"));
        m.reviews.insert(1, vec![approve("bob", &head(1))]);
        m.active.insert(1);
        let mut ledger = Ledger::default();
        run(&mut m, &mut ledger, &[]);
        assert!(m.dispatched.is_empty(), "its first run is already about today's main");

        // Main moved under that run: verify on the new main without waiting for it.
        m.obsolete.insert(1);
        run(&mut m, &mut ledger, &[]);
        assert_eq!(m.stacks, [Vec::<u64>::new()]);
    }

    #[test]
    fn a_change_that_passed_on_main_waits_behind_the_line_ahead_of_it() {
        // #1 is approved and being re-verified; #2 already passed on today's main.
        let mut m = two_stale();
        m.results.insert(key(2, &head(2)), result(2, "m0", 900, true));
        let mut ledger = Ledger::default();
        let out = run(&mut m, &mut ledger, &[]);
        assert!(m.merged.is_empty(), "merging #2 first would move main under #1");
        assert_eq!(m.stacks, [vec![], vec![1]]);
        assert_eq!(out.open.iter().find(|a| a.pr == 2).unwrap().status, AttemptStatus::Stale);

        m.active.extend([1, 2]);
        m.extra_results.push(on_tree(1, "tree:m0", &[], true));
        m.extra_results.push(on_tree(2, &after(1), &["1@1abcdef111111111111111111111111111111111"], true));
        run(&mut m, &mut ledger, &[]);
        assert_eq!(m.merged, [1, 2]);
    }

    #[test]
    fn a_change_alone_in_the_line_merges_at_once() {
        // Nothing ahead is being verified, so there is nothing to wait for.
        let mut m = base();
        m.prs.push(pr(1, "alice"));
        m.prs.push(pr(2, "dave"));
        m.results.insert(key(2, &head(2)), result(2, "m0", 900, true));
        m.reviews.insert(2, vec![approve("bob", &head(2))]);
        m.active.insert(1);
        run(&mut m, &mut Ledger::default(), &[]);
        assert_eq!(m.merged, [2], "an unapproved change ahead holds nobody up");
    }

    #[test]
    fn a_run_stacked_on_a_line_whose_front_merged_is_not_replaced() {
        let mut m = two_stale();
        let mut ledger = Ledger::default();
        run(&mut m, &mut ledger, &[]);
        assert_eq!(m.stacks, [vec![], vec![1]]);

        // #1's result arrives first; #2's run on main plus #1 is still going.
        m.active.insert(2);
        m.extra_results.push(on_tree(1, "tree:m0", &[], true));
        let out = run(&mut m, &mut ledger, &[]);
        assert_eq!(m.merged, [1]);
        assert_eq!(m.dispatched.len(), 2, "that run is building exactly today's main");
        m.prs.remove(0);
        run(&mut m, &mut ledger, &out.decided);
        assert_eq!(m.dispatched.len(), 2);

        // It is not seen for a moment, between two of its jobs: still not replaced.
        m.active.clear();
        run(&mut m, &mut ledger, &out.decided);
        assert_eq!(m.dispatched.len(), 2);

        // It ended without a result for today's main: verify again.
        ledger.dispatched.values_mut().for_each(|at| *at -= Duration::hours(1));
        run(&mut m, &mut ledger, &out.decided);
        assert_eq!(m.stacks.last().unwrap(), &Vec::<u64>::new());
        assert_eq!(m.dispatched.len(), 3);
    }

    #[test]
    fn a_finished_run_stacked_on_a_line_whose_front_merged_is_not_repeated() {
        // #2 was verified on main plus #1 before #1's own result arrived.
        let mut m = two_stale();
        let mut ledger = Ledger::default();
        run(&mut m, &mut ledger, &[]);
        m.extra_results.push(on_tree(2, &after(1), &["1@1abcdef111111111111111111111111111111111"], true));
        run(&mut m, &mut ledger, &[]);
        assert_eq!(m.dispatched.len(), 2, "nothing to do until #1's result is in");

        // #1 merges while #3, verified behind both, has its result already:
        // that result is for main plus #2, which is what is left of the line.
        m.prs.push(pr(3, "alice"));
        m.reviews.insert(3, vec![approve("bob", &head(3))]);
        run(&mut m, &mut ledger, &[]);
        assert_eq!(m.stacks.last().unwrap(), &vec![1, 2]);
        m.extra_results.push(on_tree(3, &after(2), &["1@1abcdef111111111111111111111111111111111", "2@2abcdef222222222222222222222222222222222"], true));
        m.extra_results.push(on_tree(1, "tree:m0", &[], true));
        m.extra_results.retain(|r| r.pr != 2);
        m.active.insert(2);
        ledger.dispatched.values_mut().for_each(|at| *at -= Duration::hours(1));
        let out = run(&mut m, &mut ledger, &[]);
        assert_eq!(m.merged, [1]);
        assert_eq!(m.dispatched.len(), 3, "#3's finished run is not repeated");
        m.prs.remove(0);
        run(&mut m, &mut ledger, &out.decided);
        assert_eq!(m.dispatched.len(), 3);

        // A result for the same line from another main does not count.
        ledger.line_bases.insert(key(3, &head(3)), "elsewhere".into());
        ledger.dispatched.values_mut().for_each(|at| *at -= Duration::hours(1));
        run(&mut m, &mut ledger, &out.decided);
        assert_eq!(m.dispatched.len(), 4);
    }

    #[test]
    fn verification_of_a_merged_change_is_stopped() {
        let mut m = green();
        m.reviews.insert(1, vec![approve("bob", &head(1))]);
        m.active.extend([1, 2]);
        m.prs.push(pr(2, "alice"));
        run(&mut m, &mut Ledger::default(), &[]);
        assert_eq!(m.merged, [1]);
        assert_eq!(m.cancelled, BTreeSet::from([1]), "only the merged change's runs");
    }

    fn on_tree(n: u64, base_tree: &str, stack: &[&str], pass: bool) -> VerifyResult {
        let mut r = result(n, "m0", 900, pass);
        r.base_tree = Some(base_tree.into());
        r.merged_tree = Some(after(n));
        r.stack = stack.iter().map(|s| s.to_string()).collect();
        r
    }

    #[test]
    fn approved_changes_are_verified_as_a_stack_and_merge_as_a_train() {
        let mut m = two_stale();
        let mut ledger = Ledger::default();
        run(&mut m, &mut ledger, &[]);
        assert!(m.merged.is_empty());
        assert_eq!(m.stacks, [vec![], vec![1]], "the second is verified on top of the first");

        // Both verifications finish: #1 on main, #2 on main plus #1.
        m.extra_results.push(on_tree(1, "tree:m0", &[], true));
        m.extra_results.push(on_tree(2, &after(1), &["1@1abcdef111111111111111111111111111111111"], true));
        let out = run(&mut m, &mut ledger, &[]);
        assert_eq!(m.merged, [1, 2], "both merge in one pass, with no rebuild in between");
        assert_eq!(m.dispatched.len(), 2);
        assert_eq!(out.decided.len(), 2);
    }

    #[test]
    fn a_failure_seen_only_in_a_stack_decides_nothing_until_the_stack_has_merged() {
        let mut m = two_stale();
        m.results.remove(&key(2, &head(2)));
        m.extra_results.push(failing(on_tree(2, &after(1), &["1@1abcdef111111111111111111111111111111111"], true), "compiles"));
        let out = run(&mut m, &mut Ledger::default(), &[]);
        assert!(out.decided.is_empty());
        assert_eq!(out.open.iter().find(|a| a.pr == 2).unwrap().status, AttemptStatus::Stale);

        // Once #1 has merged, the same result is about today's main and counts.
        m.extra_results.push(on_tree(1, "tree:m0", &[], true));
        let out = run(&mut m, &mut Ledger::default(), &[]);
        assert_eq!(m.merged, [1]);
        assert_eq!(out.decided.iter().find(|a| a.pr == 2).unwrap().status, AttemptStatus::Rejected);
    }

    #[test]
    fn a_result_for_another_tree_never_merges() {
        let mut m = green();
        m.results.clear();
        m.extra_results.push(on_tree(1, "tree:something-else", &[], true));
        m.reviews.insert(1, vec![approve("bob", &head(1))]);
        run(&mut m, &mut Ledger::default(), &[]);
        assert!(m.merged.is_empty());
    }

    #[test]
    fn a_merge_that_differs_from_the_verified_tree_raises_an_alert_and_pauses() {
        let mut m = base();
        for n in [1, 2] {
            m.prs.push(pr(n, "alice"));
            m.reviews.insert(n, vec![approve("bob", &head(n))]);
        }
        let mut r1 = on_tree(1, "tree:m0", &[], true);
        r1.merged_tree = Some("tree:unexpected".into());
        m.extra_results.push(r1);
        m.extra_results.push(on_tree(2, &after(1), &["1@1abcdef111111111111111111111111111111111"], true));
        let mut ledger = Ledger::default();
        run(&mut m, &mut ledger, &[]);
        assert_eq!(m.merged, [1], "the next change is held back");
        assert_eq!(m.created.len(), 1);
        assert!(m.created[0].starts_with("Alert"));
    }

    #[test]
    fn nothing_merges_while_main_does_not_verify() {
        let mut m = green();
        m.reviews.insert(1, vec![approve("bob", &head(1))]);
        m.main_ok = Some(false);
        let mut ledger = Ledger::default();
        run(&mut m, &mut ledger, &[]);
        run(&mut m, &mut ledger, &[]);
        assert!(m.merged.is_empty());
        assert_eq!(m.created.len(), 1, "the alert is raised once");
        m.main_ok = Some(true);
        run(&mut m, &mut ledger, &[]);
        assert_eq!(m.merged, [1]);
    }

    #[test]
    fn a_build_of_main_without_a_verdict_is_repeated_before_any_alert() {
        let mut m = green();
        m.reviews.insert(1, vec![approve("bob", &head(1))]);
        m.main_errors = 1;
        let mut ledger = Ledger::default();
        run(&mut m, &mut ledger, &[]);
        assert!(m.merged.is_empty(), "main is not known to verify");
        assert_eq!(m.dispatched, [0], "main is built again");
        assert!(m.created.is_empty(), "a failed download is not an alert");

        // While that build runs nothing more is dispatched.
        m.active.insert(0);
        run(&mut m, &mut ledger, &[]);
        assert_eq!(m.dispatched, [0]);

        // Three builds without a verdict: now somebody has to look.
        m.active.clear();
        m.main_errors = 3;
        run(&mut m, &mut ledger, &[]);
        run(&mut m, &mut ledger, &[]);
        assert_eq!(m.dispatched, [0]);
        assert_eq!(m.created.len(), 1);
        assert!(m.created[0].contains("could not be built"));

        // A build that passes ends the pause.
        m.main_ok = Some(true);
        run(&mut m, &mut ledger, &[]);
        assert_eq!(m.merged, [1]);
    }

    #[test]
    fn a_merge_missing_from_the_log_is_recovered_once() {
        let mut m = base();
        m.already_merged.push((pr(5, "alice"), "merge5".into(), t(-2)));
        m.results.insert(key(5, &head(5)), result(5, "older", 900, true));
        let out = run(&mut m, &mut Ledger::default(), &[]);
        assert_eq!(out.decided.len(), 1);
        assert_eq!(out.decided[0].status, AttemptStatus::Merged);
        assert_eq!(out.decided[0].delta, Some(-100));
        let again = run(&mut m, &mut Ledger::default(), &out.decided);
        assert!(again.decided.is_empty());
    }

    fn idea(n: u64, author: &str, labels: &[&str]) -> IssueInfo {
        IssueInfo {
            number: n,
            title: "Idea: merge(Nc/Tow): one statement for the Ever lemmas".into(),
            body: "**Targets** x\n**Technique** y\n**How to check** z\n**Estimated saving** 100".into(),
            author: author.into(),
            open: true,
            labels: labels.iter().map(|s| s.to_string()).collect(),
            created_at: t(-48),
            url: String::new(),
        }
    }

    #[test]
    fn claim_then_second_claimant_is_turned_away() {
        let mut m = base();
        m.issues.push(idea(7, "eve", &["idea"]));
        m.comments = vec![says(7, "alice", "/claim", -1), says(7, "dave", "I'll take it\n/claim", 0)];
        let mut ledger = Ledger::default();
        let out = run(&mut m, &mut ledger, &[]);
        assert_eq!(ledger.claims[&7].login, "alice");
        assert_eq!(out.ideas[0].status, IdeaStatus::Claimed);
        assert_eq!(out.ideas[0].claimed_by.as_deref(), Some("alice"));
        assert_eq!(out.ideas[0].technique.as_deref(), Some("merge"));
        assert_eq!(m.posted.len(), 2);
        // Already-seen comments are not processed again.
        run(&mut m, &mut ledger, &[]);
        assert_eq!(m.posted.len(), 2);
    }

    #[test]
    fn claim_expires_without_a_pull_request_but_not_with_one() {
        let mut m = base();
        m.issues.push(idea(7, "eve", &["idea"]));
        let mut ledger = Ledger::default();
        ledger.claims.insert(7, Claim { login: "alice".into(), at: t(-5) });
        let mut p = pr(1, "alice");
        p.body = "Idea: #7".into();
        m.prs.push(p);
        m.active.insert(1);
        run(&mut m, &mut ledger, &[]);
        assert!(ledger.claims.contains_key(&7));

        m.prs.clear();
        let out = run(&mut m, &mut ledger, &[]);
        assert!(ledger.claims.is_empty());
        assert_eq!(out.ideas[0].status, IdeaStatus::Open);
    }

    #[test]
    fn merged_pull_request_marks_its_idea_done() {
        let mut m = green();
        m.issues.push(idea(7, "eve", &["idea"]));
        m.prs[0].body = "Shorter proof.\n\nIdea: #7".into();
        m.reviews.insert(1, vec![approve("bob", &head(1))]);
        let out = run(&mut m, &mut Ledger::default(), &[]);
        assert_eq!(out.ideas[0].status, IdeaStatus::Open, "open until someone closes it: more may remain");
        assert_eq!(out.ideas[0].result_delta, Some(-100));
        assert!(m.posted.iter().any(|(n, body)| *n == 7 && body.contains("#1 merged")));
        m.issues[0].open = false;
        m.prs.clear();
        let out = run(&mut m, &mut Ledger::default(), &out.decided);
        assert_eq!(out.ideas[0].status, IdeaStatus::Done);
    }

    #[test]
    fn a_change_that_completes_its_idea_closes_it() {
        assert!(idea_complete("Idea: #7 (complete)"));
        assert!(idea_complete("text\n\nidea: #7 complete\n"));
        assert!(idea_complete("Closes #7"));
        assert!(!idea_complete("Idea: #7"));
        assert!(!idea_complete("Idea: #7 (part)"));
        assert!(!idea_complete("The proof is complete.\n\nIdea: #7"));
        assert_eq!(idea_ref("Idea: #7 (complete)"), Some(7));

        let mut m = green();
        m.issues.push(idea(7, "eve", &["idea"]));
        m.prs[0].body = "Shorter proof.\n\nIdea: #7 (complete)".into();
        m.reviews.insert(1, vec![approve("bob", &head(1))]);
        run(&mut m, &mut Ledger::default(), &[]);
        assert_eq!(m.closed, [7]);
        assert!(m.posted.iter().any(|(n, body)| *n == 7 && body.contains("completes this idea")));

        // A part leaves the idea open.
        let mut m = green();
        m.issues.push(idea(7, "eve", &["idea"]));
        m.prs[0].body = "Shorter proof.\n\nIdea: #7".into();
        m.reviews.insert(1, vec![approve("bob", &head(1))]);
        run(&mut m, &mut Ledger::default(), &[]);
        assert!(m.closed.is_empty());
    }

    #[test]
    fn ideas_from_newcomers_wait_for_triage() {
        let mut m = base();
        m.issues.push(idea(7, "newcomer", &[]));
        m.issues.push(idea(8, "lead", &[]));
        let mut ledger = Ledger::default();
        let out = run(&mut m, &mut ledger, &[]);
        assert!(m.labels.contains(&(7, LABEL_PROPOSED.into(), true)));
        assert!(m.labels.contains(&(8, "idea".into(), true)));
        let status = |n| out.ideas.iter().find(|i| i.number == n).unwrap().status;
        assert_eq!(status(7), IdeaStatus::Proposed);
        assert_eq!(status(8), IdeaStatus::Open);

        // A proposed idea cannot be claimed; a newcomer cannot accept it; a leader can.
        m.issues[0].labels = vec![LABEL_PROPOSED.into()];
        m.issues[1].labels = vec!["idea".into()];
        m.comments = vec![says(7, "alice", "/claim", 1), says(7, "newcomer", "/accept", 2), says(7, "lead", "/accept", 3)];
        let out = run(&mut m, &mut ledger, &[]);
        assert!(!ledger.claims.contains_key(&7));
        assert_eq!(out.ideas.iter().find(|i| i.number == 7).unwrap().status, IdeaStatus::Open);
    }

    #[test]
    fn malformed_idea_is_told_once_and_stays_out_of_the_queue() {
        let mut m = base();
        let mut bad = idea(7, "lead", &[]);
        bad.title = "Idea: make things shorter".into();
        m.issues.push(bad);
        let mut ledger = Ledger::default();
        let out = run(&mut m, &mut ledger, &[]);
        assert!(out.ideas.is_empty());
        assert_eq!(m.posted.len(), 1);
        run(&mut m, &mut ledger, &[]);
        assert_eq!(m.posted.len(), 1);
    }

    #[test]
    fn application_needs_experience_then_a_leaders_endorsement() {
        // No merged change yet: turned away.
        let mut m = base();
        m.issues.push(idea(7, "eve", &["idea"]));
        m.comments = vec![says(7, "alice", "/apply reviewer", 0)];
        let mut r = roles();
        run_with(&mut m, &cfg(), &mut Ledger::default(), &mut r, &[]);
        assert!(r.applications.is_empty());
        assert!(m.posted[0].1.contains("not yet"));

        // With a merged change: an application issue is opened.
        let past = vec![merged_attempt("alice")];
        let mut m = base();
        m.issues.push(idea(7, "eve", &["idea"]));
        m.comments = vec![says(7, "alice", "/apply reviewer", 0)];
        let mut ledger = Ledger::default();
        run_with(&mut m, &cfg(), &mut ledger, &mut r, &past);
        assert_eq!(m.created, ["Application: reviewer @alice"]);
        let issue = r.applications[0].issue;
        assert!(!r.can_review("alice"));

        // The applicant and non-leaders cannot endorse; a leader can.
        m.comments = vec![says(issue, "alice", "/endorse", 1), says(issue, "bob", "/endorse", 2)];
        run_with(&mut m, &cfg(), &mut ledger, &mut r, &past);
        assert!(!r.can_review("alice"));
        m.comments = vec![says(issue, "lead", "/endorse", 3)];
        run_with(&mut m, &cfg(), &mut ledger, &mut r, &past);
        assert!(r.can_review("alice"));
        assert_eq!(r.applications[0].status, ApplicationStatus::Granted);
        assert_eq!(m.closed, [issue]);
    }

    #[test]
    fn leaders_grant_and_revoke_and_others_cannot() {
        let mut m = base();
        let mut r = roles();
        m.comments = vec![says(3, "mallory", "/grant @mallory reviewer", 0), says(3, "lead", "/grant @carol reviewer", 1)];
        let mut ledger = Ledger::default();
        run_with(&mut m, &cfg(), &mut ledger, &mut r, &[]);
        assert!(!r.can_review("mallory"));
        assert!(r.can_review("carol"));
        m.comments = vec![
            says(3, "lead", "/revoke @carol reviewer", 2),
            says(3, "lead", "/grant @carol leader", 3),
            says(3, "lead", "/revoke @lead leader", 4),
        ];
        run_with(&mut m, &cfg(), &mut ledger, &mut r, &[]);
        assert!(!r.can_review("carol"));
        assert!(!r.is_leader("carol"));
        assert!(r.is_leader("lead"));
    }

    #[test]
    fn a_role_granted_in_this_pass_counts_in_this_pass() {
        let mut m = green();
        m.reviews.insert(1, vec![approve("carol", &head(1))]);
        m.comments = vec![says(1, "lead", "/grant @carol reviewer", 0)];
        run(&mut m, &mut Ledger::default(), &[]);
        assert_eq!(m.merged, [1]);
    }

    #[test]
    fn comment_lists_every_gate() {
        let mut m = green();
        let out = run(&mut m, &mut Ledger::default(), &[]);
        let body = render_comment(&cfg(), &out.open[0]);
        assert!(body.starts_with(COMMENT_MARKER));
        assert!(body.contains("`/approve 1abcdef111111111111111111111111111111111`"));
        for g in GATES.iter().filter(|g| g.0 != "quota") {
            assert!(body.contains(g.1), "missing {}", g.1);
        }
    }

    // ---- Changes built on other changes (`Depends: #n`) ----

    /// A passing result as the verify workflow writes it today: applied to
    /// `base_tree` (main `base` with `stack` applied), with the minimum it
    /// was judged with, the gate for counts, and the tree as it will land.
    fn stacked(n: u64, base: &str, base_tree: &str, stack: &[&str], tokens: u64) -> VerifyResult {
        let mut r = on_tree(n, base_tree, stack, true);
        r.base_sha = base.into();
        r.metrics.tokens = tokens;
        r.min_delta = Some(25);
        r.landing_tree = r.merged_tree.clone();
        r.gates.insert(3, Gate::new("no_increase", GateStatus::Pass, ""));
        r
    }

    /// A pull request built on another one.
    fn built_on(n: u64, author: &str, on: u64) -> PullRequest {
        let mut p = pr(n, author);
        p.body = format!("Shorter.\n\nIdea: none\nDepends: #{on}");
        p
    }

    /// #1 verified on main, and #2, built on it, verified on main with #1 applied.
    fn pair() -> Mock {
        let mut m = base();
        m.prs.push(pr(1, "alice"));
        m.prs.push(built_on(2, "dave", 1));
        m.extra_results.push(stacked(1, "m0", "tree:m0", &[], 900));
        let mut second = stacked(2, "m0", &after(1), &["1@1abcdef111111111111111111111111111111111"], 800);
        second.base_metrics.tokens = 900;
        m.extra_results.push(second);
        m
    }

    fn open(out: &Outcome, n: u64) -> &Attempt {
        out.open.iter().find(|a| a.pr == n).unwrap()
    }

    fn gate_of<'a>(a: &'a Attempt, key: &str) -> &'a Gate {
        a.gates.iter().find(|g| g.key == key).unwrap()
    }

    #[test]
    fn depends_line_is_parsed_strictly() {
        assert_eq!(depends_ref("text\n\nIdea: none"), Ok(None));
        assert_eq!(depends_ref("Idea: #3\nDepends: #12\n"), Ok(Some(12)));
        assert_eq!(depends_ref("depends on: #12\r\nIdea: none"), Ok(Some(12)));
        assert_eq!(depends_ref("Depends: none"), Ok(None));
        assert_eq!(depends_ref("This depends: on nothing in particular? No, mid-line text is not a line."), Ok(None));
        assert!(depends_ref("Depends: the other one").is_err());
        assert!(depends_ref("Depends: #1 and #2").is_err());
        assert!(depends_ref("Depends: #1\nDepends: #2").is_err());
        // A line inside an HTML comment is not shown on the pull request and says nothing.
        assert_eq!(depends_ref("Idea: none\n<!-- to build on another change, add:\nDepends: #5\n-->\n"), Ok(None));
        assert_eq!(depends_ref("<!--\nDepends: #5\n-->\nDepends: #6"), Ok(Some(6)));
        assert_eq!(depends_ref("<!-- a --> text\nDepends: #5\n<!-- b -->"), Ok(Some(5)));
        assert_eq!(depends_ref("Idea: none\n<!-- never closed\nDepends: #5\n"), Ok(None));
    }

    #[test]
    fn a_depends_line_hidden_in_a_comment_does_not_hold_the_change() {
        let mut m = pair();
        m.prs[1].body = "Shorter.\n\nIdea: none\n<!--\nDepends: #1\n-->".into();
        m.extra_results.push(stacked(2, "m0", "tree:m0", &[], 800));
        m.reviews.insert(2, vec![approve("bob", &head(2))]);
        let mut ledger = Ledger::default();
        let out = run(&mut m, &mut ledger, &[]);
        assert_eq!(m.merged, [2], "a change on main like any other, judged as a whole");
        assert!(out.decided[0].depends.is_empty());
    }

    #[test]
    fn a_forge_call_that_fails_changes_no_verdict() {
        // The waived verification cannot be started (an arena whose workflow
        // does not take the input yet): nothing is recorded as started, the
        // pass goes on to the other pull requests, and it is tried again.
        let mut m = small();
        m.pr_comments.insert(1, vec![says(1, "lead", "/waive-min 1abcdef111111111111111111111111111111111 small but right", 0)]);
        m.prs.push(pr(2, "dave"));
        m.extra_results.push(stacked(2, "m0", "tree:m0", &[], 900));
        m.reviews.insert(2, vec![approve("bob", &head(2))]);
        m.failing.insert("dispatch_verify_min");
        let mut ledger = Ledger::default();
        let out = run(&mut m, &mut ledger, &[]);
        assert!(m.waived.is_empty() && m.dispatched.is_empty());
        assert!(!ledger.dispatched.keys().any(|k| k.starts_with("1@")), "no record of a run that did not start");
        assert!(out.log.iter().any(|l| l.contains("#1: could not start a verification with the waived minimum")));
        assert_eq!(m.merged, [2], "the pass continued");
        m.failing.clear();
        m.prs.pop();
        run(&mut m, &mut ledger, &out.decided);
        assert_eq!(m.waived, [(1, WAIVED_MIN)]);

        // The files of a pull request cannot be read while a reservation is
        // in force: the comment says the check could not be made, no gate
        // and no status changes, and the change merges as it would have.
        let reserved = |failing: &[&'static str], first_approved: bool| {
            let mut m = pair();
            m.issues.push(reserving(7, "`Paper/Tree`"));
            m.files.insert(1, vec!["proj/Paper/Tree.lean".into()]);
            m.files.insert(2, vec!["proj/Paper/Tree.lean".into()]);
            m.failing.extend(failing.iter().copied());
            if first_approved {
                m.reviews.insert(1, vec![approve("bob", &head(1))]);
            }
            let mut ledger = Ledger::default();
            ledger.claims.insert(7, Claim { login: "alice".into(), at: t(-1) });
            m.prs[0].author = "erin".into();
            let out = run(&mut m, &mut ledger, &[]);
            (m, out)
        };
        let (works, fine) = reserved(&[], true);
        let (m, out) = reserved(&["changed_files", "changed_between"], true);
        assert_eq!(m.merged, [1]);
        assert_eq!(m.merged, works.merged);
        assert_eq!(m.statuses, works.statuses);
        assert_eq!(out.decided[0].gates, fine.decided[0].gates);
        assert_eq!(open(&out, 2).gates, open(&fine, 2).gates);
        for a in [&out.decided[0], open(&out, 2)] {
            assert_eq!(a.warnings.len(), 1);
            assert!(a.warnings[0].starts_with("the check for files reserved by claimed ideas (#7) could not be made"), "{}", a.warnings[0]);
            assert!(render_comment(&cfg(), a).contains("Warning (does not block): the check for files reserved"));
        }
        // Only the comparison of the two heads fails: the full list is used.
        let (_, out) = reserved(&["changed_between"], false);
        assert!(open(&out, 2).warnings[0].contains("may include files changed only by #1"));

        // Whether the branch contains what it was built on cannot be asked:
        // nothing merges from that commit until it can, and nothing is rejected.
        let mut m = pair();
        m.failing.insert("contains");
        let mut ledger = Ledger::default();
        let out = run(&mut m, &mut ledger, &[]);
        assert_eq!(ledger.judged[&key(2, &head(2))].contains, None);
        assert_eq!(open(&out, 2).status, AttemptStatus::Stale);
        for n in [1, 2] {
            m.reviews.insert(n, vec![approve("bob", &head(n))]);
        }
        let out = run(&mut m, &mut ledger, &[]);
        assert_eq!(m.merged, [1]);
        assert_eq!(out.decided.len(), 1);
        let second = open(&out, 2);
        assert_eq!(second.status, AttemptStatus::Stale);
        assert!(gate_of(second, "fresh").detail.as_deref().unwrap().contains("Could not check that this branch contains"));
        assert!(m.dispatched.is_empty());
        m.failing.clear();
        m.prs.remove(0);
        run(&mut m, &mut ledger, &out.decided);
        assert_eq!(m.merged, [1, 2]);
    }

    #[test]
    fn a_change_built_on_another_is_judged_on_top_of_it_reviewed_while_it_is_open_and_merges_after_it() {
        let mut m = pair();
        let mut ledger = Ledger::default();
        let out = run(&mut m, &mut ledger, &[]);
        assert!(m.merged.is_empty());
        assert!(m.dispatched.is_empty(), "its own run already had #1 applied first");
        let second = open(&out, 2);
        assert_eq!(second.status, AttemptStatus::AwaitingReview, "it can be reviewed while #1 is open");
        assert!(m.labels.contains(&(2, LABEL_READY.into(), true)));
        assert_eq!(second.depends, [1]);
        assert_eq!(second.delta, Some(-100), "measured against #1, not against main");
        assert!(gate_of(second, "fresh").detail.as_deref().unwrap().contains("merges only after it has merged"));
        let body = render_comment(&cfg(), second);
        assert!(body.contains("Depends on #1"), "{body}");
        assert!(body.contains("measured against main with #1 (`1abcdef`) applied"), "{body}");
        assert!(body.contains("| Metric | Main + #1 |"), "{body}");

        // Approved before #1 is: it waits, and holds nobody up.
        m.reviews.insert(2, vec![approve("bob", &head(2))]);
        let out = run(&mut m, &mut ledger, &[]);
        assert!(m.merged.is_empty(), "a change never merges before what it is built on");
        assert_eq!(open(&out, 2).status, AttemptStatus::Stale);
        assert_eq!(m.status_text[&head(2)], "Waiting for #1 to merge first");

        // #1 is approved: both merge in one pass, #2 from its stacked result,
        // which is for exactly the tree main has once #1 has landed.
        m.reviews.insert(1, vec![approve("bob", &head(1))]);
        let out = run(&mut m, &mut ledger, &[]);
        assert_eq!(m.merged, [1, 2]);
        assert!(m.dispatched.is_empty(), "no rebuild in between");
        assert!(out.decided.iter().all(|a| a.status == AttemptStatus::Merged));
        assert_eq!(out.decided[1].measured_on, ["1@1abcdef111111111111111111111111111111111"]);
    }

    #[test]
    fn a_dependent_cannot_merge_before_its_dependency_even_with_a_result_for_todays_main() {
        // #2 is approved and has a passing result made on main alone, for
        // exactly today's tree: its branch contains #1, so that tree has
        // #1's content in it, which nobody approved.
        let mut m = pair();
        m.extra_results.retain(|r| r.pr != 2);
        m.extra_results.push(stacked(2, "m0", "tree:m0", &[], 800));
        m.reviews.insert(2, vec![approve("bob", &head(2))]);
        let mut ledger = Ledger::default();
        let out = run(&mut m, &mut ledger, &[]);
        assert!(m.merged.is_empty());
        assert_eq!(open(&out, 2).status, AttemptStatus::Verifying, "that result is not about this change");
        assert_eq!(m.stacks, [vec![1]], "it is verified with #1 applied first");

        // Dropping the line under the same commit does not turn it into a
        // change on main: the commit was presented as built on #1.
        m.prs[1].body = "Shorter.\n\nIdea: none".into();
        let out = run(&mut m, &mut ledger, &[]);
        assert!(m.merged.is_empty());
        let second = open(&out, 2);
        assert_eq!(second.status, AttemptStatus::NeedsFix);
        assert!(gate_of(second, "format").detail.as_deref().unwrap().contains("on top of #1"));

        // Nor does pointing it at another change.
        m.prs.push(pr(3, "erin"));
        m.prs[1].body = "Shorter.\n\nIdea: none\nDepends: #3".into();
        let out = run(&mut m, &mut ledger, &[]);
        assert!(m.merged.is_empty());
        assert_eq!(open(&out, 2).status, AttemptStatus::NeedsFix);

        // A new push is a new commit: it may stand on its own, and needs its own review.
        m.prs.pop();
        m.prs[1].head_sha = "2b05bed222222222222222222222222222222222".into();
        m.prs[1].body = "Shorter.\n\nIdea: none".into();
        let out = run(&mut m, &mut ledger, &[]);
        assert!(m.merged.is_empty(), "the approval was for the other commit");
        assert_eq!(gate_of(open(&out, 2), "format").status, GateStatus::Pass);
    }

    #[test]
    fn a_dependency_revised_after_the_dependent_was_verified_makes_it_stale() {
        let mut m = pair();
        let mut ledger = Ledger::default();
        run(&mut m, &mut ledger, &[]);

        // #2 is approved as the difference from #1 as it is.
        m.reviews.insert(2, vec![approve("bob", &head(2))]);
        run(&mut m, &mut ledger, &[]);

        // #1 is pushed to again, and #2's branch already contains the new
        // commit (its author made it). #2 is out of date: the approval was
        // given under another statement of what it is built on, and has to
        // be given again; the reviewer is told.
        m.prs[0].head_sha = "1ee15ed111111111111111111111111111111111".into();
        m.active.insert(1);
        let out = run_at(&mut m, &mut ledger, &[], t(1));
        let second = open(&out, 2);
        assert_eq!(second.status, AttemptStatus::Stale);
        assert_eq!(second.judged_on, ["1@1ee15ed111111111111111111111111111111111"]);
        let detail = gate_of(second, "fresh").detail.clone().unwrap();
        assert!(detail.contains("#1 was pushed to after this was verified"), "{detail}");
        assert!(detail.contains("Approvals given before that push do not count"), "{detail}");
        assert!(detail.contains("once it is approved"), "{detail}");
        assert_eq!(gate_of(second, "review").status, GateStatus::Pending);
        assert!(m.posted.iter().any(|(n, b)| *n == 2 && b.contains("@bob your approval") && b.contains("`/approve 2abcdef222222222222222222222222222222222`")));
        assert!(m.dispatched.is_empty());
        run_at(&mut m, &mut ledger, &[], t(1));
        assert_eq!(m.posted.len(), 1, "told once");

        // Approved again: it is verified again on #1's new commit, once.
        m.reviews.insert(2, vec![approve_at("bob", &head(2), t(2))]);
        let out = run_at(&mut m, &mut ledger, &[], t(1));
        assert_eq!(m.stacks, [vec![1]]);
        assert!(gate_of(open(&out, 2), "fresh").detail.as_deref().unwrap().contains("being verified again"));
        run_at(&mut m, &mut ledger, &[], t(1));
        assert_eq!(m.dispatched, [2]);

        // The revised #1 is approved and passes, and merges. #2's old result
        // was for another tree and is not merged from.
        m.extra_results.push(at_head(stacked(1, "m0", "tree:m0", &[], 900), "1ee15ed111111111111111111111111111111111"));
        m.reviews.insert(1, vec![approve_at("bob", "1ee15ed111111111111111111111111111111111", t(2))]);
        m.active.clear();
        let out = run_at(&mut m, &mut ledger, &[], t(1));
        assert_eq!(m.merged, [1]);
        assert_eq!(open(&out, 2).status, AttemptStatus::Stale);

        // Its result on the new #1 is for the tree main now has: it merges.
        let tree = format!("tree:{}", landed_at(1, "1ee15ed111111111111111111111111111111111"));
        let mut second = stacked(2, "m0", &tree, &["1@1ee15ed111111111111111111111111111111111"], 800);
        second.base_metrics.tokens = 900;
        m.extra_results.push(second);
        m.prs.remove(0);
        run_at(&mut m, &mut ledger, &out.decided, t(1));
        assert_eq!(m.merged, [1, 2]);
    }

    #[test]
    fn an_approval_of_a_dependent_does_not_carry_it_past_a_revision_of_its_dependency() {
        // #1 changes X and Y. #2 is built on it and approved as the
        // difference from it. Y is objected to; #1 is revised to drop Y, and
        // #2's branch still has the old commit, with Y.
        let revised = "1ee15ed111111111111111111111111111111111";
        let scene = || {
            let mut m = pair();
            let mut ledger = Ledger::default();
            run(&mut m, &mut ledger, &[]);
            m.reviews.insert(2, vec![approve("bob", &head(2))]);
            run(&mut m, &mut ledger, &[]);
            m.prs[0].head_sha = revised.into();
            m.lacks.insert((head(2), revised.into()));
            m.extra_results.push(at_head(stacked(1, "m0", "tree:m0", &[], 950), revised));
            m.reviews.insert(1, vec![approve("bob", revised)]);
            // #2 would pass on main alone once #1 is in: the build is fine,
            // and it brings Y back.
            let tree = format!("tree:{}", landed_at(1, revised));
            m.extra_results.push(stacked(2, &landed_at(1, revised), &tree, &[], 800));
            (m, ledger)
        };

        // While #1 is open at its new commit: #2 needs a fix, and says what.
        let (mut m, mut ledger) = scene();
        m.reviews.remove(&1);
        let out = run(&mut m, &mut ledger, &[]);
        assert!(m.merged.is_empty());
        let second = open(&out, 2);
        assert_eq!(second.status, AttemptStatus::NeedsFix);
        let detail = gate_of(second, "format").detail.clone().unwrap();
        assert!(detail.contains("#1, which this is built on, is at `1ee15ed111`, and this branch does not contain that commit"), "{detail}");
        assert!(detail.contains("Merge #1's branch into yours and push; the new commit needs a new approval"), "{detail}");

        // #1 merges as revised, in the pass that first sees the revision or
        // in a later one: #2, the same commit with the same approval, does
        // not merge, whatever passes on main.
        for passes_between in [0, 1] {
            let (mut m, mut ledger) = scene();
            let mut log = Vec::new();
            if passes_between == 1 {
                m.reviews.remove(&1);
                run(&mut m, &mut ledger, &log);
                m.reviews.insert(1, vec![approve("bob", revised)]);
            }
            let out = run(&mut m, &mut ledger, &log);
            log.extend(out.decided.iter().cloned());
            assert_eq!(m.merged, [1]);
            m.prs.remove(0);
            for _ in 0..2 {
                let out = run(&mut m, &mut ledger, &log);
                assert_eq!(m.merged, [1], "Y would land with #2, approved by nobody");
                let second = open(&out, 2);
                assert_eq!(second.status, AttemptStatus::NeedsFix);
                let detail = gate_of(second, "format").detail.clone().unwrap();
                assert!(
                    detail.contains("was revised before it merged") || detail.contains("merged as `1ee15ed111`, and this branch does not contain that commit"),
                    "{detail}"
                );
                assert!(detail.contains("Merge `main` into your branch and push"), "{detail}");
                assert!(detail.contains("needs a new approval"), "{detail}");
            }

            // The author merges main and pushes: a new commit, judged against
            // main as a whole, which needs its own approval.
            m.prs[0].head_sha = "2e59ed0222222222222222222222222222222222".into();
            let tree = format!("tree:{}", landed_at(1, revised));
            m.extra_results.push(at_head(stacked(2, &landed_at(1, revised), &tree, &[], 800), "2e59ed0222222222222222222222222222222222"));
            let out = run(&mut m, &mut ledger, &log);
            assert_eq!(m.merged, [1]);
            assert_eq!(open(&out, 2).status, AttemptStatus::AwaitingReview);
            m.reviews.insert(2, vec![approve("bob", "2e59ed0222222222222222222222222222222222")]);
            run(&mut m, &mut ledger, &log);
            assert_eq!(m.merged, [1, 2]);
        }
    }

    #[test]
    fn the_record_of_what_a_commit_was_judged_on_survives_its_absence_from_a_pass() {
        // #2 is approved as the difference from #1.
        let mut m = pair();
        let mut ledger = Ledger::default();
        let out = run(&mut m, &mut ledger, &[]);
        assert!(render_comment(&cfg(), open(&out, 2)).contains("Depends on #1"));
        assert!(render_comment(&cfg(), open(&out, 2)).contains("judged on top of #1 at `1abcdef111111111111111111111111111111111`"));
        assert_eq!(ledger.judged[&key(2, &head(2))].on, ["1@1abcdef111111111111111111111111111111111"]);
        m.reviews.insert(2, vec![approve("bob", &head(2))]);
        run(&mut m, &mut ledger, &[]);
        assert!(m.merged.is_empty());

        // Its author makes it a draft (or closes it, or retargets it) for a
        // pass, then removes the line and brings it back: the same commit.
        for absent in ["draft", "closed", "retargeted"] {
            let mut m = Mock { main: "m0".into(), prs: m.prs.clone(), reviews: m.reviews.clone(), ..Default::default() };
            m.extra_results.push(stacked(1, "m0", "tree:m0", &[], 900));
            let mut ledger = ledger.clone();
            let second = m.prs[1].clone();
            match absent {
                "draft" => m.prs[1].draft = true,
                "closed" => drop(m.prs.remove(1)),
                _ => m.prs[1].base_ref = "other".into(),
            }
            run(&mut m, &mut ledger, &[]);
            assert_eq!(ledger.judged[&key(2, &head(2))].on, ["1@1abcdef111111111111111111111111111111111"], "{absent}: the record is kept");
            m.prs.retain(|p| p.number != 2);
            m.prs.push(PullRequest { body: "Shorter.\n\nIdea: none".into(), ..second });
            m.extra_results.push(stacked(2, "m0", "tree:m0", &[], 800));
            let out = run(&mut m, &mut ledger, &[]);
            assert!(m.merged.is_empty(), "{absent}: #2 would merge with #1's commits; #1 is open and unapproved");
            let second = open(&out, 2);
            assert_eq!(second.status, AttemptStatus::NeedsFix, "{absent}");
            assert!(gate_of(second, "format").detail.as_deref().unwrap().contains("judged as a change on top of #1"));
        }

        // The record goes when the pull request has merged.
        let mut m = green();
        m.reviews.insert(1, vec![approve("bob", &head(1))]);
        let mut ledger = Ledger::default();
        run(&mut m, &mut ledger, &[]);
        assert_eq!(m.merged, [1]);
        assert!(ledger.judged.is_empty());

        // Beyond the bound the oldest records of pull requests that are not
        // open go; those of open ones stay.
        let mut m = green();
        let mut ledger = Ledger::default();
        for n in 0..(JUDGED_KEPT as u64 + 5) {
            ledger.judged.insert(key(100 + n, "gone"), Judged { at: Some(t(-10) + Duration::seconds(n as i64)), names: None, on: vec![], contains: Some(true), has_merged: None });
        }
        run(&mut m, &mut ledger, &[]);
        assert_eq!(ledger.judged.len(), JUDGED_KEPT + 1, "the bound is on the records of pull requests that are not open");
        assert!(ledger.judged.contains_key(&key(1, &head(1))));
        assert!(!ledger.judged.contains_key(&key(104, "gone")));
        assert!(ledger.judged.contains_key(&key(105, "gone")));
    }

    #[test]
    fn records_of_open_pull_requests_cannot_push_another_record_out() {
        // #2 is a draft for now; its commit has a record. Somebody piles up
        // records on an open pull request (one per commit pushed).
        let mut m = pair();
        let mut ledger = Ledger::default();
        run(&mut m, &mut ledger, &[]);
        m.prs[1].draft = true;
        m.prs.push(pr(3, "dave"));
        for n in 0..(JUDGED_KEPT as u64 + 1) {
            ledger.judged.insert(key(3, &format!("c{n}")), Judged { at: Some(t(2)), names: None, on: vec![], contains: Some(true), has_merged: None });
        }
        run(&mut m, &mut ledger, &[]);
        assert_eq!(ledger.judged[&key(2, &head(2))].on, [key(1, &head(1))], "the draft's record is still there");
    }

    /// #2 was judged on #1 and approved; #1 was revised (#2's branch does
    /// not have the new commit), approved and merged; #2 has been told to
    /// merge main and push. Returns the state after that, with the log.
    fn superseded() -> (Mock, Ledger, Vec<Attempt>) {
        let revised = "1ee15ed111111111111111111111111111111111";
        let mut m = pair();
        m.lacks.insert((head(2), revised.into()));
        let mut ledger = Ledger::default();
        run(&mut m, &mut ledger, &[]);
        m.reviews.insert(2, vec![approve("bob", &head(2))]);
        run(&mut m, &mut ledger, &[]);
        m.prs[0].head_sha = revised.into();
        m.extra_results.push(at_head(stacked(1, "m0", "tree:m0", &[], 950), revised));
        m.reviews.insert(1, vec![approve_at("bob", revised, t(5))]);
        let out = run_at(&mut m, &mut ledger, &[], t(1));
        assert_eq!(m.merged, [1]);
        let log = out.decided.clone();
        m.prs.retain(|p| p.number != 1);
        let out = pass(&mut m, &mut ledger, &log, t(2));
        assert_eq!(open(&out, 2).status, AttemptStatus::NeedsFix);
        // #2 would pass on main alone: the build is fine, and it brings
        // back what the revision of #1 dropped.
        let main = m.main.clone();
        let mut alone = stacked(2, &main, &format!("tree:{main}"), &[], 800);
        alone.base_metrics.tokens = 950;
        m.extra_results.push(alone);
        (m, ledger, log)
    }

    #[test]
    fn a_commit_whose_depends_line_names_a_merged_change_must_contain_what_merged_whatever_the_ledger_says() {
        // Instead of merging main, the author gets the record of #2's commit
        // out of the ledger (here it is simply gone; a draft and enough
        // closed commits would do it) and brings #2 back as it was.
        let (mut m, mut ledger, log) = superseded();
        ledger.judged.remove(&key(2, &head(2)));
        let out = pass(&mut m, &mut ledger, &log, t(4));
        assert!(m.merged.iter().all(|n| *n != 2));
        let a = open(&out, 2);
        assert_eq!(a.status, AttemptStatus::NeedsFix, "first seen after #1 merged, and it does not contain what merged");
        let detail = gate_of(a, "format").detail.clone().unwrap();
        assert!(detail.contains("`Depends: #1`: #1 merged as `1ee15ed111`, and this branch does not contain that commit"), "{detail}");
        assert!(detail.contains("Merge `main` into your branch and push"), "{detail}");
        assert!(detail.contains("or remove the line"), "{detail}");
        // A new approval does not help: the commit cannot merge.
        m.reviews.insert(2, vec![approve_at("bob", &head(2), t(5))]);
        pass(&mut m, &mut ledger, &log, t(6));
        pass(&mut m, &mut ledger, &log, t(7));
        assert_eq!(m.merged, [1], "#2, still without #1's merged commit, would have merged");

        // Whether it contains that commit cannot be asked: nothing merges meanwhile.
        let (mut m, mut ledger, log) = superseded();
        ledger.judged.remove(&key(2, &head(2)));
        m.lacks.clear();
        m.failing.insert("contains");
        pass(&mut m, &mut ledger, &log, t(4));
        m.reviews.insert(2, vec![approve_at("bob", &head(2), t(5))]);
        let out = pass(&mut m, &mut ledger, &log, t(6));
        assert_eq!(m.merged, [1]);
        assert!(gate_of(open(&out, 2), "fresh").detail.as_deref().unwrap().contains("Could not check"));

        // A branch that does contain what merged, first seen after the
        // merge, is an ordinary change on main: judged as a whole, the bot
        // says nothing of a dependency, and it merges on an approval given
        // after that.
        m.failing.clear();
        let out = pass(&mut m, &mut ledger, &log, t(7));
        assert_eq!(m.merged, [1, 2]);
        assert!(!render_comment(&cfg(), &out.decided[0]).contains("Depends on"));
    }

    #[test]
    fn an_approval_counts_only_if_given_after_the_bot_comment_that_first_states_the_judgement() {
        // The first pass over a commit starts at t(0); the forge stamps the
        // bot's comment 30 seconds later. An approval stamped in between,
        // which the pass already reads, was given before any comment.
        let during = t(0) + Duration::seconds(10);
        let stated = t(0) + Duration::seconds(COMMENT_DELAY);
        let mut m = green();
        m.reviews.insert(1, vec![approve_at("bob", &head(1), during)]);
        let mut ledger = Ledger::default();
        let out = pass(&mut m, &mut ledger, &[], t(0));
        assert!(m.merged.is_empty(), "no approval counts in the pass that first judges a commit");
        assert_eq!(ledger.judged[&key(1, &head(1))].at, Some(stated), "the forge's time of the comment");
        let review = gate_of(&out.open[0], "review");
        assert_eq!(review.status, GateStatus::Pending);
        assert!(review.detail.as_deref().unwrap().contains("bob were given before the bot's comment"), "{review:?}");
        assert!(review.detail.as_deref().unwrap().contains(&format!("`/approve {}`", head(1))));
        assert!(render_comment(&cfg(), &out.open[0]).contains("An approval counts only if it is given after this comment first appeared"));
        // The reviewer is told on the pull request, once.
        assert_eq!(m.posted.len(), 1);
        assert!(m.posted[0].1.starts_with("@bob your approval of `1abcdef111` does not count"));
        assert!(m.posted[0].1.ends_with(&format!("`/approve {}`.", head(1))));
        // It does not count in a later pass either: it is not after the comment.
        pass(&mut m, &mut ledger, &[], t(1));
        assert!(m.merged.is_empty());
        assert_eq!(m.posted.len(), 1);
        // Nor does one stamped in the same second as the comment.
        m.reviews.insert(1, vec![approve_at("bob", &head(1), stated)]);
        pass(&mut m, &mut ledger, &[], t(1));
        assert!(m.merged.is_empty());
        // One given after the comment counts, in the next pass.
        m.reviews.insert(1, vec![approve_at("bob", &head(1), stated + Duration::seconds(1))]);
        pass(&mut m, &mut ledger, &[], t(1));
        assert_eq!(m.merged, [1]);

        // The runner's clock is hours behind the forge's. Both times that
        // are compared are the forge's, so an approval from before the
        // comment still does not count.
        let mut m = green();
        m.reviews.insert(1, vec![approve_at("bob", &head(1), t(-1))]);
        let mut ledger = Ledger::default();
        for _ in 0..2 {
            m.clock = Some(t(0));
            Gatekeeper { cfg: &cfg(), forge: &mut m, now: t(-5) }.run(&mut ledger, &mut roles(), &[]).unwrap();
            assert!(m.merged.is_empty());
        }

        // A request for changes made early still blocks: only approvals are held to the comment.
        let mut m = green();
        let mut ledger = Ledger::default();
        m.pr_comments.insert(1, vec![CommentInfo { created_at: t(-1), ..says(1, "lead", "/request-changes", 0) }]);
        pass(&mut m, &mut ledger, &[], t(0));
        m.reviews.insert(1, vec![approve("bob", &head(1))]);
        pass(&mut m, &mut ledger, &[], t(1));
        assert!(m.merged.is_empty());

        // A commit whose record was lost is judged for the first time
        // again. The comment has not changed since it was written, so
        // approvals given after it were given under what it says, and count.
        let mut m = green();
        let mut ledger = Ledger::default();
        pass(&mut m, &mut ledger, &[], t(0));
        m.reviews.insert(1, vec![approve("bob", &head(1))]);
        ledger.judged.clear();
        pass(&mut m, &mut ledger, &[], t(0));
        assert!(m.merged.is_empty(), "not in the pass that makes the record");
        pass(&mut m, &mut ledger, &[], t(0));
        assert_eq!(m.merged, [1]);
    }

    /// Two more commits, for pushes in the tests below.
    const C2: &str = "c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2";
    const REVISED: &str = "1ee15ed111111111111111111111111111111111";

    /// When the mock forge stamps the gate comment of a pass started at `at`.
    fn stated(at: DateTime<Utc>) -> DateTime<Utc> {
        at + Duration::seconds(COMMENT_DELAY)
    }

    #[test]
    fn a_new_commit_does_not_inherit_the_time_of_the_previous_commits_comment() {
        // The first commit is open with a leader's request for changes and is
        // still being verified; the bot's comment for it is written.
        let mut m = base();
        m.prs.push(pr(1, "alice"));
        m.active.insert(1);
        m.pr_comments.insert(1, vec![CommentInfo { created_at: t(-5), ..says(1, "lead", "/request-changes too long", 0) }]);
        let mut ledger = Ledger::default();
        pass(&mut m, &mut ledger, &[], t(0));
        let first = ledger.judged[&key(1, &head(1))].at.unwrap();
        assert_eq!(first, stated(t(0)));

        // A second commit is pushed, and approved before any pass has seen
        // it. The pass that first judges it has the same gates to show (the
        // request for changes hides everything else about the review).
        m.prs[0].head_sha = C2.into();
        m.reviews.insert(1, vec![approve_at("bob", C2, t(0) + Duration::minutes(10))]);
        pass(&mut m, &mut ledger, &[], t(1));
        assert_eq!(
            ledger.judged[&key(1, C2)].at,
            Some(stated(t(1))),
            "the comment names the commit, so it was written again for this one"
        );
        assert!(m.gate_comments[&1].0.starts_with(&format!("{COMMENT_MARKER} <!-- {C2}; on main -->\n")));

        // The request is withdrawn and the build passes. The approval is
        // from before the comment for this commit: it does not count.
        m.pr_comments.clear();
        m.active.clear();
        m.extra_results.push(at_head(stacked(1, "m0", "tree:m0", &[], 900), C2));
        let out = pass(&mut m, &mut ledger, &[], t(2));
        assert!(m.merged.is_empty(), "an approval given before the gatekeeper had judged the commit");
        assert!(gate_of(&out.open[0], "review").detail.as_deref().unwrap().contains("bob were given before the bot's comment"));
        assert!(m.posted.iter().any(|(_, b)| b.starts_with("@bob your approval")));
        // Given again after it, it counts.
        m.reviews.insert(1, vec![approve_at("bob", C2, stated(t(1)) + Duration::minutes(1))]);
        pass(&mut m, &mut ledger, &[], t(3));
        assert_eq!(m.merged, [1]);
    }

    #[test]
    fn the_first_line_of_the_comment_names_the_commit_and_what_it_is_judged_on() {
        let mut m = pair();
        let mut ledger = Ledger::default();
        pass(&mut m, &mut ledger, &[], t(0));
        let first_line = |m: &Mock, n: u64| m.gate_comments[&n].0.lines().next().unwrap().to_string();
        assert_eq!(first_line(&m, 1), format!("{COMMENT_MARKER} <!-- {}; on main -->", head(1)));
        assert_eq!(first_line(&m, 2), format!("{COMMENT_MARKER} <!-- {}; on 1@{}; depends #1 -->", head(2), head(1)));
        // The text a reader sees starts as before.
        assert!(m.gate_comments[&1].0.lines().nth(1).unwrap().starts_with("### Hillclimb gate"));
        // The same commit on the same basis: the same text, the same time.
        let at = m.gate_comments[&2].1;
        pass(&mut m, &mut ledger, &[], t(1));
        assert_eq!(m.gate_comments[&2].1, at);
        // What it is built on is pushed to: another basis, written again.
        m.prs[0].head_sha = REVISED.into();
        m.active.insert(1);
        pass(&mut m, &mut ledger, &[], t(2));
        assert_eq!(first_line(&m, 2), format!("{COMMENT_MARKER} <!-- {}; on 1@{REVISED}; depends #1 -->", head(2)));
        assert_eq!(ledger.judged[&key(2, &head(2))].at, Some(stated(t(2))));
    }

    #[test]
    fn a_dependent_pair_from_first_sight_to_both_merged_in_real_passes() {
        let mut m = pair();
        let mut ledger = Ledger::default();
        let out = pass(&mut m, &mut ledger, &[], t(0));
        for n in [1, 2] {
            assert_eq!(open(&out, n).status, AttemptStatus::AwaitingReview);
            assert_eq!(ledger.judged[&key(n, &head(n))].at, Some(stated(t(0))));
        }
        assert_eq!(ledger.judged[&key(2, &head(2))].on, [key(1, &head(1))]);
        assert!(render_comment(&cfg(), open(&out, 2)).contains(&format!("judged on top of #1 at `{}`", head(1))));
        // Both are approved after the bot's comments; the next pass merges
        // #1 and, from its result on top of #1, #2.
        for n in [1, 2] {
            m.reviews.insert(n, vec![approve_at("bob", &head(n), stated(t(0)) + Duration::minutes(5))]);
        }
        let out = pass(&mut m, &mut ledger, &[], t(1));
        assert_eq!(m.merged, [1, 2]);
        assert!(m.dispatched.is_empty() && m.posted.is_empty());
        assert!(ledger.judged.is_empty(), "the records go with the merges");
        assert_eq!(out.decided.len(), 2);
    }

    #[test]
    fn a_push_to_the_dependency_in_real_passes() {
        // The dependent's branch does not have the new commit: it needs a fix,
        // and the commit that fixes it needs an approval given after the
        // bot's comment on that commit.
        let mut m = pair();
        let mut ledger = Ledger::default();
        pass(&mut m, &mut ledger, &[], t(0));
        m.reviews.insert(2, vec![approve_at("bob", &head(2), stated(t(0)) + Duration::minutes(5))]);
        let out = pass(&mut m, &mut ledger, &[], t(1));
        assert_eq!(gate_of(open(&out, 2), "review").status, GateStatus::Pass);
        assert!(m.merged.is_empty());

        m.prs[0].head_sha = REVISED.into();
        m.lacks.insert((head(2), REVISED.into()));
        m.extra_results.push(at_head(stacked(1, "m0", "tree:m0", &[], 950), REVISED));
        let out = pass(&mut m, &mut ledger, &[], t(2));
        let second = open(&out, 2);
        assert_eq!(second.status, AttemptStatus::NeedsFix);
        assert!(gate_of(second, "format").detail.as_deref().unwrap().contains("this branch does not contain that commit"));

        // Its author merges #1's branch in: a new commit. A reviewer approves
        // it at once, before any pass has seen it; #1 is approved too.
        m.prs[1].head_sha = C2.into();
        let hasty = stated(t(2)) + Duration::minutes(10);
        m.reviews.insert(2, vec![approve_at("bob", C2, hasty)]);
        m.reviews.insert(1, vec![approve_at("bob", REVISED, hasty)]);
        let tree = format!("tree:{}", landed_at(1, REVISED));
        let mut on_revised = at_head(stacked(2, "m0", &tree, &[], 800), C2);
        on_revised.stack = vec![key(1, REVISED)];
        on_revised.base_metrics.tokens = 950;
        m.extra_results.push(on_revised);
        let out = pass(&mut m, &mut ledger, &[], t(3));
        assert_eq!(m.merged, [1], "#1's approval came after the comment on its new commit; #2's new commit is judged only now");
        let log = out.decided.clone();
        m.prs.remove(0);
        let out = pass(&mut m, &mut ledger, &log, t(4));
        assert_eq!(m.merged, [1], "the hasty approval is from before the comment on that commit");
        assert!(gate_of(open(&out, 2), "review").detail.as_deref().unwrap().contains("bob were given before the bot's comment"));
        m.reviews.insert(2, vec![approve_at("bob", C2, stated(t(3)) + Duration::minutes(5))]);
        pass(&mut m, &mut ledger, &log, t(5));
        assert_eq!(m.merged, [1, 2]);

        // The dependent's branch already has the new commit: nothing to fix,
        // but the approval was given for the difference from the old one
        // and starts over.
        let mut m = pair();
        let mut ledger = Ledger::default();
        pass(&mut m, &mut ledger, &[], t(0));
        m.reviews.insert(2, vec![approve_at("bob", &head(2), stated(t(0)) + Duration::minutes(5))]);
        pass(&mut m, &mut ledger, &[], t(1));
        m.prs[0].head_sha = REVISED.into();
        m.extra_results.push(at_head(stacked(1, "m0", "tree:m0", &[], 950), REVISED));
        let mut on_revised = stacked(2, "m0", &tree, &[], 800);
        on_revised.stack = vec![key(1, REVISED)];
        on_revised.base_metrics.tokens = 950;
        m.extra_results.push(on_revised);
        m.reviews.insert(1, vec![approve_at("bob", REVISED, stated(t(2)) + Duration::minutes(5))]);
        for at in [t(2), t(3)] {
            let out = pass(&mut m, &mut ledger, &[], at);
            let second = out.open.iter().find(|a| a.pr == 2).unwrap();
            assert_eq!(gate_of(second, "format").status, GateStatus::Pass);
            assert_eq!(gate_of(second, "review").status, GateStatus::Pending, "approved for the difference from the other commit");
            assert!(m.merged.iter().all(|n| *n != 2));
        }
        assert_eq!(m.merged, [1]);
        assert!(m.posted.iter().any(|(n, b)| *n == 2 && b.starts_with("@bob your approval")));
    }

    #[test]
    fn a_push_after_an_approval_in_real_passes() {
        // An ordinary change is judged and approved. Its author pushes
        // again, and the new commit is approved before a pass has seen it.
        let mut m = base();
        m.prs.push(pr(1, "alice"));
        m.active.insert(1);
        let mut ledger = Ledger::default();
        pass(&mut m, &mut ledger, &[], t(0));
        m.reviews.insert(1, vec![approve_at("bob", &head(1), stated(t(0)) + Duration::minutes(5))]);
        m.prs[0].head_sha = C2.into();
        m.reviews.get_mut(&1).unwrap().push(approve_at("bob", C2, stated(t(0)) + Duration::minutes(6)));
        m.extra_results.push(at_head(stacked(1, "m0", "tree:m0", &[], 900), C2));
        m.active.clear();
        let out = pass(&mut m, &mut ledger, &[], t(1));
        assert!(m.merged.is_empty(), "the pass that first judges the new commit counts no approval of it");
        assert_eq!(out.open[0].status, AttemptStatus::AwaitingReview);
        let out = pass(&mut m, &mut ledger, &[], t(2));
        assert!(m.merged.is_empty(), "nor does a later pass: the approval is from before the comment on that commit");
        assert!(gate_of(&out.open[0], "review").detail.as_deref().unwrap().contains("bob were given before the bot's comment"));
        assert_eq!(m.posted.len(), 1);
        m.reviews.get_mut(&1).unwrap().push(approve_at("bob", C2, stated(t(1)) + Duration::minutes(5)));
        pass(&mut m, &mut ledger, &[], t(3));
        assert_eq!(m.merged, [1]);
    }

    #[test]
    fn a_ledger_from_before_these_records_is_read_and_the_first_pass_goes_through() {
        // As written by a version that pinned `Depends` by number only.
        let old = r#"{"claims":{"7":{"login":"alice","at":"2026-01-09T00:00:00Z"}},"comments_seen_until":"2026-01-09T00:00:00Z","dispatched":{},"lines":{},"line_bases":{},"alerts":[],"format_notified":[],"depends":{"2@abc":[1]}}"#;
        let mut ledger: Ledger = serde_json::from_str(old).expect("an old ledger is still read");
        assert!(ledger.judged.is_empty() && ledger.told.is_empty());
        assert_eq!(ledger.claims[&7].login, "alice");
        // The first pass with it: every open commit is judged for the first
        // time, approvals from before do not count, and nothing fails.
        let mut m = pair();
        for n in [1, 2] {
            m.reviews.insert(n, vec![approve_at("bob", &head(n), t(-3))]);
        }
        let out = pass(&mut m, &mut ledger, &[], t(0));
        assert!(m.merged.is_empty());
        assert_eq!(out.open.len(), 2);
        assert_eq!(ledger.judged.len(), 2);
        assert_eq!(m.posted.len(), 2, "each reviewer is told once to approve again");
        // What is written now is read back.
        let again: Ledger = serde_json::from_str(&serde_json::to_string(&ledger).unwrap()).unwrap();
        assert_eq!(again.judged, ledger.judged);
        // A record as the version before this one wrote it (a time, no
        // `names`, no `has_merged`) is read as well.
        let earlier = r#"{"judged":{"1@x":{"at":"2026-01-10T00:00:00Z","on":["3@y"],"contains":true}}}"#;
        let ledger: Ledger = serde_json::from_str(earlier).unwrap();
        assert_eq!(ledger.judged["1@x"].at, Some(t(0)));
        assert_eq!((ledger.judged["1@x"].names, ledger.judged["1@x"].has_merged), (None, None));
    }

    #[test]
    fn from_the_first_pass_to_a_merge_without_any_record_made_beforehand() {
        // The whole course as it runs for real, with nothing in the ledger:
        // a pass judges two new changes and writes its comments; a reviewer
        // approves both after that; the next pass forms the line and
        // verifies the second on top of the first; the one after merges
        // both. The approval costs no pass beyond the one that counts it.
        let mut m = two_stale();
        m.reviews.clear();
        let mut ledger = Ledger::default();
        let out = pass(&mut m, &mut ledger, &[], t(0));
        assert!(out.open.iter().all(|a| a.status == AttemptStatus::Stale));
        assert!(m.dispatched.is_empty());
        for n in [1, 2] {
            m.reviews.insert(n, vec![approve("bob", &head(n))]);
        }
        pass(&mut m, &mut ledger, &[], t(0) + Duration::minutes(2));
        assert_eq!(m.stacks, [vec![], vec![1]], "the approvals count in the first pass after they were given");
        m.extra_results.push(on_tree(1, "tree:m0", &[], true));
        m.extra_results.push(on_tree(2, &after(1), &[&key(1, &head(1))], true));
        pass(&mut m, &mut ledger, &[], t(1));
        assert_eq!(m.merged, [1, 2]);
        assert!(m.posted.is_empty(), "nobody had to be told anything");
    }

    #[test]
    fn the_notice_of_an_early_approval_says_what_the_commit_is_judged_against() {
        // An ordinary change: judged against main as a whole.
        let mut m = green();
        m.reviews.insert(1, vec![approve_at("bob", &head(1), t(-1))]);
        pass(&mut m, &mut Ledger::default(), &[], t(0));
        assert!(m.posted[0].1.contains("It is judged against `main` as a whole: everything in the pull request's difference from `main`"), "{}", m.posted[0].1);

        // A change built on another: as the difference from a named commit.
        let mut m = pair();
        m.reviews.insert(2, vec![approve_at("bob", &head(2), t(-1))]);
        pass(&mut m, &mut Ledger::default(), &[], t(0));
        let notice = &m.posted.iter().find(|(n, _)| *n == 2).unwrap().1;
        assert!(notice.contains(&format!("It is judged as the difference from #1 at `{}`", head(1))), "{notice}");
    }

    #[test]
    fn a_notice_that_cannot_be_posted_does_not_end_the_pass() {
        // #1 has an early approval to be told about, and commenting fails.
        // #2, after it, is still handled, and #1's reviewer is told later.
        let mut m = green();
        m.reviews.insert(1, vec![approve_at("bob", &head(1), t(-2))]);
        m.prs.push(pr(2, "dave"));
        m.extra_results.push(stacked(2, "m0", "tree:m0", &[], 900));
        m.reviews.insert(2, vec![approve("bob", &head(2))]);
        m.failing.insert("comment");
        let mut ledger = Ledger::default();
        seen_before(&m, &mut ledger, &[]);
        m.clock = Some(t(0));
        let out = Gatekeeper { cfg: &cfg(), forge: &mut m, now: t(0) }
            .run(&mut ledger, &mut roles(), &[])
            .expect("a comment that cannot be posted is not fatal");
        assert_eq!(m.merged, [2], "the pass went on");
        assert!(out.log.iter().any(|l| l.contains("#1: could not tell bob")), "{:?}", out.log);
        assert!(m.posted.is_empty());
        m.failing.clear();
        m.prs.pop();
        run(&mut m, &mut ledger, &out.decided);
        assert_eq!(m.posted.len(), 1, "told once it can be");
    }

    #[test]
    fn an_approval_or_a_waiver_names_the_commit_by_all_forty_characters() {
        // A head that is not a full hash can never be approved by comment:
        // an empty one would equal a bare `/approve`, a short one a prefix.
        for odd in ["", "1abcdef", "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"] {
            let mut m = green();
            m.prs[0].head_sha = odd.into();
            m.results.clear();
            m.extra_results.push(at_head(stacked(1, "m0", "tree:m0", &[], 900), odd));
            m.pr_comments.insert(1, vec![says(1, "bob", format!("/approve {odd}").trim(), 0)]);
            run(&mut m, &mut Ledger::default(), &[]);
            assert!(m.merged.is_empty(), "{odd:?}");

            let mut m = small();
            m.prs[0].head_sha = odd.into();
            m.pr_comments.insert(1, vec![says(1, "lead", &format!("/waive-min {odd} small but right"), 0)]);
            let out = run(&mut m, &mut Ledger::default(), &[]);
            assert!(out.open[0].waiver.is_none(), "{odd:?}");
            assert!(m.waived.is_empty(), "{odd:?}");
        }
        // Upper case is the same commit.
        let mut m = green();
        m.pr_comments.insert(1, vec![says(1, "bob", &format!("/approve {}", head(1).to_uppercase()), 0)]);
        run(&mut m, &mut Ledger::default(), &[]);
        assert_eq!(m.merged, [1]);
    }

    #[test]
    fn an_edited_comment_is_not_an_approval_or_a_waiver() {
        // Anyone with write access can edit somebody else's comment, so a
        // comment that was changed after it was posted proves nothing.
        let mut m = green();
        let approval = CommentInfo { edited: true, ..says(1, "bob", &format!("Fine.\n/approve {}", head(1)), 0) };
        m.pr_comments.insert(1, vec![approval]);
        let mut ledger = Ledger::default();
        let out = run(&mut m, &mut ledger, &[]);
        assert!(m.merged.is_empty());
        assert!(gate_of(&out.open[0], "review").detail.as_deref().unwrap().contains("bob was edited after it was posted"));
        assert_eq!(m.posted.len(), 1);
        assert!(m.posted[0].1.starts_with("@bob your `/approve` comment was edited after it was posted, so it does not count"));
        run(&mut m, &mut ledger, &[]);
        assert_eq!(m.posted.len(), 1, "told once");
        // Posted again as a new comment, it counts.
        m.pr_comments.get_mut(&1).unwrap().push(says(1, "bob", &format!("/approve {}", head(1)), 1));
        run(&mut m, &mut ledger, &[]);
        assert_eq!(m.merged, [1]);

        let mut m = small();
        let waiver = CommentInfo { edited: true, ..says(1, "lead", &format!("/waive-min {} small but right", head(1)), 0) };
        m.pr_comments.insert(1, vec![waiver]);
        let out = run(&mut m, &mut Ledger::default(), &[]);
        assert!(out.open[0].waiver.is_none());
        assert!(m.waived.is_empty());
        assert!(m.posted.iter().any(|(_, b)| b.contains("@lead your `/waive-min` comment was edited")));
    }

    #[test]
    fn a_dependent_is_not_rejected_for_a_failed_stacked_build_before_its_dependency_has_passed() {
        // #2's run, with #1 applied first, fails to build. #1 has no result
        // of its own yet: the failure may be #1's.
        let mut m = pair();
        m.extra_results.clear();
        m.extra_results.push(failing(stacked(2, "m0", &after(1), &["1@1abcdef111111111111111111111111111111111"], 800), "compiles"));
        m.active.insert(1);
        let mut ledger = Ledger::default();
        let out = run(&mut m, &mut ledger, &[]);
        assert!(out.decided.is_empty(), "a failure seen only in a stack decides nothing");
        let second = open(&out, 2);
        assert_eq!(second.status, AttemptStatus::Stale);
        assert!(gate_of(second, "compiles").detail.as_deref().unwrap().contains("the failure may not be this change's"));
        assert!(gate_of(second, "fresh").detail.as_deref().unwrap().contains("judged when it has"));
        assert!(!m.labels.contains(&(2, LABEL_REJECTED.into(), true)));
        assert!(m.dispatched.is_empty(), "nothing is gained by building it again now");

        // #1 passes on its own at that commit: now the failure is #2's.
        m.extra_results.push(stacked(1, "m0", "tree:m0", &[], 900));
        let out = run(&mut m, &mut ledger, &[]);
        assert_eq!(out.decided.len(), 1);
        assert_eq!((out.decided[0].pr, out.decided[0].status), (2, AttemptStatus::Rejected));

        // #1 passed, but at another commit than the one that was applied: undecided.
        let mut m = pair();
        m.extra_results.retain(|r| r.pr == 1);
        m.extra_results.push(failing(stacked(2, "m0", &after(1), &["1@101de50111111111111111111111111111111111"], 800), "compiles"));
        let out = run(&mut m, &mut Ledger::default(), &[]);
        assert!(out.decided.is_empty());
    }

    #[test]
    fn depends_on_something_that_is_not_an_open_pull_request_fails_the_format_gate() {
        // #1 is closed without merging; #7 is an issue; #99 does not exist.
        for (line, says) in [
            ("Depends: #1", "not an open pull request"),
            ("Depends: #7", "not an open pull request"),
            ("Depends: #99", "not an open pull request"),
            ("Depends: #2", "cannot be built on itself"),
            ("Depends: the big one", "must be `Depends: #<number>`"),
            ("Depends: #3\nDepends: #4", "only one such line"),
        ] {
            let mut m = base();
            m.issues.push(idea(7, "eve", &["idea"]));
            let mut p = pr(2, "dave");
            p.body = format!("Shorter.\n\nIdea: none\n{line}");
            m.prs.push(p);
            m.extra_results.push(stacked(2, "m0", "tree:m0", &[], 900));
            m.reviews.insert(2, vec![approve("bob", &head(2))]);
            let out = run(&mut m, &mut Ledger::default(), &[]);
            assert!(m.merged.is_empty(), "{line}");
            let a = open(&out, 2);
            assert_eq!(a.status, AttemptStatus::NeedsFix, "{line}");
            let detail = gate_of(a, "format").detail.clone().unwrap();
            assert!(detail.contains(says), "{line}: {detail}");
            assert!(out.decided.is_empty(), "{line}: a wrong line is fixed, not logged as a rejection");
        }

        // A circle: each member is told, and none is verified or merged.
        let mut m = base();
        m.prs.push(built_on(1, "alice", 2));
        m.prs.push(built_on(2, "dave", 1));
        for n in [1, 2] {
            m.extra_results.push(stacked(n, "m0", "tree:m0", &[], 900));
            m.reviews.insert(n, vec![approve("bob", &head(n))]);
        }
        let out = run(&mut m, &mut Ledger::default(), &[]);
        assert!(m.merged.is_empty());
        for n in [1, 2] {
            assert!(gate_of(open(&out, n), "format").detail.as_deref().unwrap().contains("cannot be built on itself"));
        }

        // What it names has merged: the line is satisfied and it is a change on main.
        let mut m = base();
        m.prs.push(built_on(2, "dave", 1));
        m.extra_results.push(stacked(2, "m0", "tree:m0", &[], 900));
        m.reviews.insert(2, vec![approve("bob", &head(2))]);
        run(&mut m, &mut Ledger::default(), &[merged_attempt("alice")]);
        assert_eq!(m.merged, [2]);
    }

    #[test]
    fn when_what_it_is_built_on_is_rejected_or_closed_the_dependent_cannot_merge_and_says_why() {
        let mut m = pair();
        m.extra_results.clear();
        m.extra_results.push(failing(stacked(1, "m0", "tree:m0", &[], 900), "compiles"));
        // Built with #1 applied first, so its build failed as well.
        m.extra_results.push(failing(stacked(2, "m0", &after(1), &["1@1abcdef111111111111111111111111111111111"], 800), "compiles"));
        m.reviews.insert(2, vec![approve("bob", &head(2))]);
        let mut ledger = Ledger::default();
        let out = run(&mut m, &mut ledger, &[]);
        assert!(m.merged.is_empty());
        assert_eq!(out.decided.len(), 1, "only #1 is rejected; the failure of #2 may be #1's");
        assert_eq!(out.decided[0].pr, 1);
        let second = open(&out, 2);
        assert_eq!(second.status, AttemptStatus::Stale);
        let detail = gate_of(second, "fresh").detail.clone().unwrap();
        assert!(detail.contains("#1, which was rejected at its current commit"), "{detail}");
        assert!(detail.contains("cannot merge before it"), "{detail}");
        assert!(m.dispatched.is_empty(), "nothing is built on top of a rejected change");

        // #1 is closed without merging.
        m.prs.remove(0);
        let again = run(&mut m, &mut ledger, &out.decided);
        assert!(m.merged.is_empty());
        assert!(again.decided.is_empty(), "a result with a closed change applied decides nothing");
        let second = open(&again, 2);
        assert_eq!(second.status, AttemptStatus::NeedsFix);
        let detail = gate_of(second, "format").detail.clone().unwrap();
        assert!(detail.contains("#1 is not an open pull request"), "{detail}");
        assert!(detail.contains("closed without merging"), "{detail}");
    }

    #[test]
    fn a_chain_of_changes_merges_in_order_as_one_train() {
        let mut m = pair();
        m.prs.push(built_on(3, "erin", 2));
        let mut third = stacked(3, "m0", &after(2), &["1@1abcdef111111111111111111111111111111111", "2@2abcdef222222222222222222222222222222222"], 700);
        third.base_metrics.tokens = 800;
        m.extra_results.push(third);
        let mut ledger = Ledger::default();
        let out = run(&mut m, &mut ledger, &[]);
        assert_eq!(open(&out, 3).depends, [1, 2]);
        assert_eq!(open(&out, 3).status, AttemptStatus::AwaitingReview);
        assert!(render_comment(&cfg(), open(&out, 3)).contains("Depends on #2, which depends on #1"));
        assert!(m.dispatched.is_empty());

        // The middle one is not approved: the last one waits behind it.
        for n in [1, 3] {
            m.reviews.insert(n, vec![approve("bob", &head(n))]);
        }
        run(&mut m, &mut ledger, &[]);
        assert_eq!(m.merged, [1]);
        m.prs.remove(0);
        m.reviews.insert(2, vec![approve("bob", &head(2))]);
        run(&mut m, &mut ledger, &[merged_attempt("alice")]);
        assert_eq!(m.merged, [1, 2, 3]);
        assert!(m.dispatched.is_empty(), "each result was for exactly the tree main had by then");
    }

    #[test]
    fn a_change_built_on_a_higher_number_is_still_judged_after_it() {
        let mut m = base();
        m.prs.push(built_on(1, "alice", 2));
        m.prs.push(pr(2, "dave"));
        m.extra_results.push(stacked(2, "m0", "tree:m0", &[], 900));
        m.extra_results.push(stacked(1, "m0", &after(2), &["2@2abcdef222222222222222222222222222222222"], 800));
        for n in [1, 2] {
            m.reviews.insert(n, vec![approve("bob", &head(n))]);
        }
        run(&mut m, &mut Ledger::default(), &[]);
        assert_eq!(m.merged, [2, 1]);
    }

    #[test]
    fn a_held_change_does_not_hold_up_the_line() {
        let mut m = pair();
        m.prs.push(pr(3, "erin"));
        m.extra_results.push(stacked(3, "m0", "tree:m0", &[], 900));
        for n in [2, 3] {
            m.reviews.insert(n, vec![approve("bob", &head(n))]);
        }
        run(&mut m, &mut Ledger::default(), &[]);
        assert_eq!(m.merged, [3], "#2 waits for #1 outside the line");
    }

    #[test]
    fn a_stacked_result_that_would_land_as_another_tree_is_not_merged_from() {
        // #2's branch contains #1's commits and changes the same lines. On top
        // of #1 as a real merge it verified one tree; once #1 has landed as a
        // squashed commit, merging the branch gives another tree or a conflict.
        let mut m = pair();
        m.extra_results.iter_mut().find(|r| r.pr == 2).unwrap().landing_tree = Some("conflict".into());
        let mut ledger = Ledger::default();
        run(&mut m, &mut ledger, &[]);
        for n in [1, 2] {
            m.reviews.insert(n, vec![approve("bob", &head(n))]);
        }
        let out = run(&mut m, &mut ledger, &[]);
        assert_eq!(m.merged, [1], "the tree that would land is not the tree that was verified");
        let second = open(&out, 2);
        assert_eq!(second.status, AttemptStatus::Stale);
        assert!(gate_of(second, "fresh").detail.as_deref().unwrap().contains("no longer merges to the tree that was verified"));
        assert_eq!(m.stacks.last().unwrap(), &Vec::<u64>::new(), "verified again on main alone");

        // The run on main alone is the merge that lands: its result merges.
        m.prs.remove(0);
        let mut alone = stacked(2, &landed(1), &after(1), &[], 800);
        alone.base_metrics.tokens = 900;
        m.extra_results.push(alone);
        run(&mut m, &mut ledger, &out.decided);
        assert_eq!(m.merged, [1, 2]);

        // A stacked result that does not say which tree lands (one written
        // before that was recorded) is not merged from either, for a change
        // that was judged on top of another.
        let mut m = pair();
        m.extra_results.retain(|r| r.pr != 2);
        m.extra_results.push(on_tree(2, &after(1), &["1@1abcdef111111111111111111111111111111111"], true));
        let mut ledger = Ledger::default();
        run(&mut m, &mut ledger, &[]);
        for n in [1, 2] {
            m.reviews.insert(n, vec![approve("bob", &head(n))]);
        }
        run(&mut m, &mut ledger, &[]);
        assert_eq!(m.merged, [1]);

        // The same holds for any stacked result, with or without a `Depends` line.
        let mut m = two_stale();
        m.extra_results.push(stacked(1, "m0", "tree:m0", &[], 900));
        let mut second = stacked(2, "m0", &after(1), &["1@1abcdef111111111111111111111111111111111"], 900);
        second.landing_tree = Some("tree:something-else".into());
        m.extra_results.push(second);
        run(&mut m, &mut Ledger::default(), &[]);
        assert_eq!(m.merged, [1]);
    }

    // ---- Failures the author can fix ----

    #[test]
    fn every_gate_says_what_its_failure_means() {
        let fix: Vec<&str> = GATES.iter().filter(|g| g.3 == OnFail::Fix).map(|g| g.0).collect();
        let reject: Vec<&str> = GATES.iter().filter(|g| g.3 == OnFail::Reject).map(|g| g.0).collect();
        assert_eq!(fix, ["format", "quota", "no_increase", "improves"]);
        assert_eq!(reject, ["frozen", "scope", "deny", "compiles", "same_theorems"]);
        assert_eq!(gate_on_fail("something new"), OnFail::Reject);

        for g in GATES.iter().filter(|g| !matches!(g.0, "format" | "fresh" | "review")) {
            let mut m = base();
            m.prs.push(pr(1, "alice"));
            let mut r = stacked(1, "m0", "tree:m0", &[], 900);
            r.gates.insert(0, Gate::new("quota", GateStatus::Pass, ""));
            m.extra_results.push(failing(r, g.0));
            m.reviews.insert(1, vec![approve("bob", &head(1))]);
            let out = run(&mut m, &mut Ledger::default(), &[]);
            assert!(m.merged.is_empty(), "{}", g.0);
            if g.3 == OnFail::Fix {
                assert!(out.decided.is_empty(), "{}", g.0);
                assert_eq!(out.open[0].status, AttemptStatus::NeedsFix, "{}", g.0);
                assert!(m.labels.contains(&(1, LABEL_NEEDS_FIX.into(), true)), "{}", g.0);
                assert!(!m.labels.contains(&(1, LABEL_REJECTED.into(), true)), "{}", g.0);
            } else {
                assert_eq!(out.decided[0].status, AttemptStatus::Rejected, "{}", g.0);
                assert!(out.open.is_empty(), "{}", g.0);
                assert!(m.labels.contains(&(1, LABEL_REJECTED.into(), true)), "{}", g.0);
            }
        }

        // A verdict together with something fixable is a verdict.
        let mut m = base();
        m.prs.push(pr(1, "alice"));
        let mut r = failing(stacked(1, "m0", "tree:m0", &[], 900), "improves");
        r.gates.iter_mut().find(|g| g.key == "frozen").unwrap().status = GateStatus::Fail;
        m.extra_results.push(r);
        let out = run(&mut m, &mut Ledger::default(), &[]);
        assert_eq!(out.decided[0].status, AttemptStatus::Rejected);
    }

    #[test]
    fn a_fixable_failure_then_a_fixing_push_merges_and_leaves_no_rejected_attempt() {
        // Approved, then one line over the length limit.
        let mut m = base();
        m.prs.push(pr(1, "alice"));
        let mut long = failing(stacked(1, "m0", "tree:m0", &[], 900), "no_increase");
        long.gates.iter_mut().find(|g| g.key == "no_increase").unwrap().detail =
            Some("lines longer than 100 characters rose from 0 to 1 (proj/A.lean:7)".into());
        m.extra_results.push(long);
        m.reviews.insert(1, vec![approve("bob", &head(1))]);
        let mut ledger = Ledger::default();
        let mut log: Vec<Attempt> = Vec::new();
        for _ in 0..2 {
            let out = run(&mut m, &mut ledger, &log);
            log.extend(out.decided.iter().cloned());
            assert!(m.merged.is_empty());
            assert!(out.decided.is_empty(), "nothing is written to the attempts log");
            assert_eq!(out.open.len(), 1, "the change stays open");
            let a = &out.open[0];
            assert_eq!(a.status, AttemptStatus::NeedsFix);
            assert_eq!(m.statuses[&head(1)], CommitState::Failure);
            assert_eq!(m.status_text[&head(1)], "Needs a fix: no more long lines or exceptions");
            assert!(m.labels.contains(&(1, LABEL_NEEDS_FIX.into(), true)));
            assert!(!m.labels.iter().any(|l| l.1 == LABEL_REJECTED && l.2));
            let body = render_comment(&cfg(), a);
            assert!(body.contains("Needs a fix. Nothing is rejected"), "{body}");
            assert!(body.contains("- **No more long lines or exceptions**: lines longer than 100 characters rose from 0 to 1 (proj/A.lean:7)"), "{body}");
        }

        // The author pushes the fix; it is verified and approved, and merges.
        m.prs[0].head_sha = "1f1ced0111111111111111111111111111111111".into();
        m.prs[0].labels = vec![LABEL_NEEDS_FIX.into()];
        let mut fixed = stacked(1, "m0", "tree:m0", &[], 900);
        fixed.head_sha = "1f1ced0111111111111111111111111111111111".into();
        m.extra_results.push(fixed);
        m.reviews.insert(1, vec![approve("bob", "1f1ced0111111111111111111111111111111111")]);
        let out = run(&mut m, &mut ledger, &log);
        log.extend(out.decided.iter().cloned());
        assert_eq!(m.merged, [1]);
        assert!(m.labels.contains(&(1, LABEL_NEEDS_FIX.into(), false)));
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].status, AttemptStatus::Merged);
        assert!(log.iter().all(|a| a.status != AttemptStatus::Rejected));
    }

    #[test]
    fn a_result_from_before_the_counts_had_their_own_gate_keeps_its_verdict() {
        // Such a result carries the count checks inside `deny`.
        let mut m = green();
        m.reviews.insert(1, vec![approve("bob", &head(1))]);
        let out = run(&mut m, &mut Ledger::default(), &[]);
        assert_eq!(m.merged, [1]);
        assert_eq!(gate_of(&out.decided[0], "no_increase").status, GateStatus::Pass);

        // A result in today's format without that gate did not run it.
        let mut m = base();
        m.prs.push(pr(1, "alice"));
        let mut r = stacked(1, "m0", "tree:m0", &[], 900);
        r.gates.retain(|g| g.key != "no_increase");
        m.extra_results.push(r);
        m.reviews.insert(1, vec![approve("bob", &head(1))]);
        run(&mut m, &mut Ledger::default(), &[]);
        assert!(m.merged.is_empty());
    }

    // ---- Files reserved by a claimed idea ----

    fn reserving(n: u64, text: &str) -> IssueInfo {
        let mut i = idea(n, "eve", &["idea"]);
        i.body.push_str(&format!("\n**Reserves** {text}\n**Risk** none"));
        i
    }

    #[test]
    fn reserves_section_is_read_as_paths_and_globs() {
        let body = "**Targets** x\n**Reserves**\n- `Paper/Tree`\n- `Nc/*.lean`, and `./proj/Other.lean`\n\n**Risk** `not/this`";
        assert_eq!(reserves(body), ["Paper/Tree", "Nc/*.lean", "proj/Other.lean"]);
        assert_eq!(reserves("## Reserves\nPaper/Tree.lean, Nc/Tow/ (the whole folder).\n## Risk\nnone"), ["Paper/Tree.lean", "Nc/Tow"]);
        assert!(reserves("**Targets** `Paper/Tree`").is_empty());
        let paths: Vec<String> =
            ["proj/Paper/Tree.lean", "proj/Paper/Tree/Cut.lean", "proj/Paper/Trees.lean", "proj/Nc/A.lean", "proj/Other.lean"]
                .iter()
                .map(|s| s.to_string())
                .collect();
        let hits = reserved_hits("proj/", &reserves(body), &paths);
        assert_eq!(hits, ["proj/Paper/Tree.lean", "proj/Paper/Tree/Cut.lean", "proj/Nc/A.lean", "proj/Other.lean"]);
    }

    #[test]
    fn touching_reserved_files_warns_and_never_blocks() {
        let mut m = base();
        m.issues.push(reserving(7, "`Paper/Tree`"));
        let mut ledger = Ledger::default();
        ledger.claims.insert(7, Claim { login: "alice".into(), at: t(-1) });
        // #1 is somebody else's and touches the reserved file; #2 is the
        // claimant's change for the idea; #3 touches something else.
        m.prs.push(pr(1, "dave"));
        let mut own = pr(2, "alice");
        own.body = "Idea: #7".into();
        m.prs.push(own);
        m.prs.push(pr(3, "dave"));
        m.files.insert(1, vec!["proj/Paper/Tree.lean".into(), "proj/Nc/A.lean".into()]);
        m.files.insert(2, vec!["proj/Paper/Tree.lean".into()]);
        m.files.insert(3, vec!["proj/Nc/B.lean".into()]);
        m.extra_results.push(stacked(1, "m0", "tree:m0", &[], 900));
        let out = run(&mut m, &mut ledger, &[]);
        let first = open(&out, 1);
        assert_eq!(first.status, AttemptStatus::AwaitingReview, "a warning fails no gate");
        assert_eq!(first.warnings.len(), 1);
        assert!(first.warnings[0].starts_with("touches files reserved by idea #7 (claimed by @alice): `proj/Paper/Tree.lean`;"));
        assert!(first.warnings[0].ends_with("coordinate there or expect to merge main after it lands"));
        assert!(render_comment(&cfg(), first).contains("Warning (does not block): touches files reserved by idea #7"));
        assert!(first.gates.iter().all(|g| g.status != GateStatus::Fail));
        assert!(open(&out, 2).warnings.is_empty(), "the claimant's own change for the idea");
        assert!(open(&out, 3).warnings.is_empty());

        // It merges like any other change, and the warning is on the record.
        m.reviews.insert(1, vec![approve("bob", &head(1))]);
        let out = run(&mut m, &mut ledger, &[]);
        assert_eq!(m.merged, [1], "a warning never blocks a merge");
        assert_eq!(out.decided[0].warnings.len(), 1);

        // Without a claim nothing is reserved.
        let mut m = base();
        m.issues.push(reserving(7, "`Paper/Tree`"));
        m.prs.push(pr(1, "dave"));
        m.files.insert(1, vec!["proj/Paper/Tree.lean".into()]);
        let out = run(&mut m, &mut Ledger::default(), &[]);
        assert!(open(&out, 1).warnings.is_empty());
    }

    #[test]
    fn a_change_built_on_another_is_warned_only_for_the_files_it_changes_itself() {
        // #1 touches the reserved file; #2 is built on #1 and touches another
        // file, but as a pull request against main it lists both.
        let mut m = pair();
        m.issues.push(reserving(7, "`Paper/Tree`"));
        let mut ledger = Ledger::default();
        ledger.claims.insert(7, Claim { login: "alice".into(), at: t(-1) });
        m.prs[0].author = "erin".into();
        m.files.insert(1, vec!["proj/Paper/Tree.lean".into()]);
        m.files.insert(2, vec!["proj/Paper/Tree.lean".into(), "proj/Nc/A.lean".into()]);
        m.between.insert((head(1), head(2)), vec!["proj/Nc/A.lean".into()]);
        let out = run(&mut m, &mut ledger, &[]);
        assert_eq!(open(&out, 1).warnings.len(), 1);
        assert!(open(&out, 2).warnings.is_empty(), "only #1 changes the reserved file");

        // It changes the reserved file itself: warned.
        m.between.insert((head(1), head(2)), vec!["proj/Paper/Tree.lean".into()]);
        let out = run(&mut m, &mut ledger, &[]);
        assert_eq!(open(&out, 2).warnings.len(), 1);
        assert!(!open(&out, 2).warnings[0].contains("may include"));

        // Its own difference cannot be had: the full list is used, and the warning says so.
        m.between.clear();
        let out = run(&mut m, &mut ledger, &[]);
        assert!(open(&out, 2).warnings[0].ends_with("(the list may include files changed only by #1, which this change is built on)"));
    }

    #[test]
    fn in_the_bootstrap_phase_only_the_change_for_the_idea_is_exempt() {
        // Every agent uses the one account, so the author says nothing.
        let mut c = cfg();
        let mut m = base();
        m.issues.push(reserving(7, "`Paper/Tree`"));
        m.prs.push(pr(1, "lead"));
        let mut own = pr(2, "lead");
        own.body = "Idea: #7".into();
        m.prs.push(own);
        for n in [1, 2] {
            m.files.insert(n, vec!["proj/Paper/Tree.lean".into()]);
        }
        let claimed = || {
            let mut l = Ledger::default();
            l.claims.insert(7, Claim { login: "lead".into(), at: t(-1) });
            l
        };
        let out = run_with(&mut m, &c, &mut claimed(), &mut roles(), &[]);
        assert!(open(&out, 1).warnings.is_empty(), "outside bootstrap the claimant's changes do not warn");
        c.roles.bootstrap = true;
        let out = run_with(&mut m, &c, &mut claimed(), &mut roles(), &[]);
        assert_eq!(open(&out, 1).warnings.len(), 1);
        assert!(open(&out, 2).warnings.is_empty());
    }

    // ---- A leader's waiver of the minimum ----

    /// #1 saves 10, below the minimum of 25; its build passed (or would).
    fn small() -> Mock {
        let mut m = base();
        m.prs.push(pr(1, "alice"));
        m.extra_results.push(failing(stacked(1, "m0", "tree:m0", &[], 990), "improves"));
        m
    }

    /// The same change judged with a minimum of 1.
    fn waived_result(head_sha: &str) -> VerifyResult {
        let mut r = stacked(1, "m0", "tree:m0", &[], 990);
        r.head_sha = head_sha.into();
        r.min_delta = Some(WAIVED_MIN);
        r
    }

    #[test]
    fn a_leader_can_waive_the_minimum_for_one_commit() {
        let mut m = small();
        let mut ledger = Ledger::default();
        let out = run(&mut m, &mut ledger, &[]);
        assert_eq!(out.open[0].status, AttemptStatus::NeedsFix);
        assert!(render_comment(&cfg(), &out.open[0]).contains("`/waive-min 1abcdef111111111111111111111111111111111 <reason>`"));

        let waive = says(1, "lead", "Worth it.\n/waive-min 1abcdef111111111111111111111111111111111 removes a statement that was wrong", 0);
        m.pr_comments.insert(1, vec![waive.clone()]);
        m.comments = vec![waive];
        let out = run(&mut m, &mut ledger, &[]);
        assert_eq!(m.waived, [(1, WAIVED_MIN)], "verified again, judged with the waived minimum");
        assert_eq!(out.open[0].status, AttemptStatus::Verifying);
        let waiver = out.open[0].waiver.clone().unwrap();
        assert_eq!((waiver.by.as_str(), waiver.reason.as_str()), ("lead", "removes a statement that was wrong"));
        assert!(m.posted.iter().any(|(n, b)| *n == 1 && b.contains("is waived for commit 1abcdef111111111111111111111111111111111 by lead")));
        run(&mut m, &mut ledger, &[]);
        assert_eq!(m.waived.len(), 1, "once per commit");

        // That run passes. A review is still needed; then it merges.
        m.extra_results.push(waived_result(&head(1)));
        let out = run(&mut m, &mut ledger, &[]);
        assert!(m.merged.is_empty());
        assert_eq!(out.open[0].status, AttemptStatus::AwaitingReview);
        let body = render_comment(&cfg(), &out.open[0]);
        assert!(body.contains("is waived for this commit by @lead: removes a statement that was wrong"), "{body}");
        m.reviews.insert(1, vec![approve("bob", &head(1))]);
        let out = run(&mut m, &mut ledger, &[]);
        assert_eq!(m.merged, [1]);
        assert_eq!(out.decided[0].waiver.as_ref().unwrap().reason, "removes a statement that was wrong");
        assert_eq!(out.decided[0].delta, Some(-10));
    }

    #[test]
    fn a_waiver_does_not_survive_a_push() {
        let mut m = small();
        m.pr_comments.insert(1, vec![says(1, "lead", "/waive-min 1abcdef111111111111111111111111111111111 small but right", 0)]);
        let mut ledger = Ledger::default();
        run(&mut m, &mut ledger, &[]);
        assert_eq!(m.waived.len(), 1);

        // A new commit, again below the minimum: the waiver named the other one.
        m.prs[0].head_sha = "1b05bed111111111111111111111111111111111".into();
        let mut r = failing(stacked(1, "m0", "tree:m0", &[], 990), "improves");
        r.head_sha = "1b05bed111111111111111111111111111111111".into();
        m.extra_results.push(r);
        m.reviews.insert(1, vec![approve("bob", "1b05bed111111111111111111111111111111111")]);
        let out = run(&mut m, &mut ledger, &[]);
        assert_eq!(out.open[0].status, AttemptStatus::NeedsFix);
        assert!(out.open[0].waiver.is_none());
        assert_eq!(m.waived.len(), 1, "nothing is verified with a waived minimum");

        // Even a result that was judged with the lower minimum does not count
        // for a commit that has no waiver.
        m.extra_results.retain(|r| r.head_sha != "1b05bed111111111111111111111111111111111");
        m.extra_results.push(waived_result("1b05bed111111111111111111111111111111111"));
        let out = run(&mut m, &mut ledger, &[]);
        assert!(m.merged.is_empty());
        assert_eq!(out.open[0].status, AttemptStatus::NeedsFix);
        assert!(gate_of(&out.open[0], "improves").detail.as_deref().unwrap().contains("no waiver is in force"));

        // A new waiver for the new commit does.
        m.pr_comments.get_mut(&1).unwrap().push(says(1, "lead", "/waive-min 1b05bed111111111111111111111111111111111 still right", 1));
        run(&mut m, &mut ledger, &[]);
        assert_eq!(m.merged, [1]);
    }

    #[test]
    fn only_a_leader_can_waive_and_only_with_a_commit_and_a_reason() {
        for (who, text) in [
            ("bob", "/waive-min 1abcdef111111111111111111111111111111111 a reviewer is not a leader"),
            ("mallory", "/waive-min 1abcdef111111111111111111111111111111111 please"),
            ("alice", "/waive-min 1abcdef111111111111111111111111111111111 my own change"),
            ("lead", "/waive-min 1abcdef111111111111111111111111111111111"),
            ("lead", "/waive-min because it is small"),
            ("lead", "/waive-min 0123456 another commit"),
        ] {
            let mut m = small();
            m.extra_results.push(waived_result(&head(1)));
            m.reviews.insert(1, vec![approve("bob", &head(1))]);
            m.pr_comments.insert(1, vec![says(1, who, text, 0)]);
            m.comments = vec![says(1, who, text, 0)];
            let out = run(&mut m, &mut Ledger::default(), &[]);
            assert!(m.merged.is_empty(), "{who}: {text}");
            assert!(m.waived.is_empty(), "{who}: {text}");
            assert_eq!(out.open[0].status, AttemptStatus::NeedsFix, "{who}: {text}");
            // A leader who got the form wrong is told the exact line; others get no answer.
            assert_eq!(m.posted.iter().any(|(_, b)| b.contains("Usage: `/waive-min 1abcdef111111111111111111111111111111111 <reason>`")), who == "lead");
        }

        // A leader cannot waive for a change from the leader's own account,
        // except in the bootstrap phase, as with reviews.
        let mut m = small();
        m.prs[0].author = "lead".into();
        m.pr_comments.insert(1, vec![says(1, "lead", "/waive-min 1abcdef111111111111111111111111111111111 small but right", 0)]);
        run(&mut m, &mut Ledger::default(), &[]);
        assert!(m.waived.is_empty());
        let mut c = cfg();
        c.roles.bootstrap = true;
        run_with(&mut m, &c, &mut Ledger::default(), &mut roles(), &[]);
        assert_eq!(m.waived.len(), 1);
    }

    // ---- A large estimate needs a trial ----

    #[test]
    fn estimates_are_read_as_the_largest_number_in_the_section() {
        assert_eq!(largest_number("about 800 tokens"), 800);
        assert_eq!(largest_number("800-1,500 tokens, measured on 2 files"), 1500);
        assert_eq!(largest_number("~1.5k (see #4000)"), 1500);
        assert_eq!(largest_number("2k to 3K"), 3000);
        assert_eq!(largest_number("unknown"), 0);
        // Digits of other scripts are not read as a number (they would not parse).
        assert_eq!(largest_number("about 300, or ٣٠٠٠٠ by another count; ９９９９９"), 300);
        assert_eq!(section("**Targets** a\n**Estimated saving:** 120\nmaybe 300\n**Trial** none yet", "estimat").as_deref(), Some("120\nmaybe 300"));
        assert_eq!(section("## Estimated saving\n\n900\n\n## Trial\nported `A.lean`: 410", "trial").as_deref(), Some("\nported `A.lean`: 410"));
        assert_eq!(section("no headings here", "trial"), None);
    }

    #[test]
    fn an_idea_above_the_threshold_needs_a_trial_before_it_is_queued() {
        let big = |n: u64, trial: &str| {
            let mut i = idea(n, "lead", &[]);
            i.body = format!("**Targets** x\n**Technique** y\n**How to check** z\n**Estimated saving** about 2,500 tokens\n{trial}");
            i
        };
        let mut m = base();
        m.issues.push(big(7, ""));
        m.issues.push(big(8, "**Trial** ported `Nc/Tow.lean`: 410 of its 1,900 tokens"));
        m.issues.push(idea(9, "lead", &[]));
        // Already in the queue: not judged again.
        m.issues.push(big(10, ""));
        m.issues[3].labels = vec!["idea".into()];
        let mut ledger = Ledger::default();
        let out = run(&mut m, &mut ledger, &[]);
        let queued: Vec<u64> = out.ideas.iter().map(|i| i.number).collect();
        assert_eq!(queued, [10, 9, 8]);
        assert_eq!(m.posted.len(), 1);
        assert_eq!(m.posted[0].0, 7);
        assert!(m.posted[0].1.contains("The estimated saving (2500) is above 1000"), "{}", m.posted[0].1);
        assert!(m.posted[0].1.contains("**Trial** section"));
        run(&mut m, &mut ledger, &[]);
        assert_eq!(m.posted.len(), 1, "told once");

        // An empty Trial section is not a trial; a filled one is picked up.
        m.issues[0] = big(7, "**Trial**\n");
        assert!(run(&mut m, &mut ledger, &[]).ideas.iter().all(|i| i.number != 7));
        m.issues[0] = big(7, "**Trial**\nmeasured on one file: 300");
        assert!(run(&mut m, &mut ledger, &[]).ideas.iter().any(|i| i.number == 7));

        // Without a heading, the paragraph that mentions the estimate is read.
        let mut m = base();
        let mut plain = idea(7, "lead", &[]);
        plain.body = "Targets: x\nTechnique: y\nHow to check: z\nEstimated saving: 4k tokens,\nperhaps more\n\nSee #12.".into();
        m.issues.push(plain);
        assert!(run(&mut m, &mut Ledger::default(), &[]).ideas.is_empty());
        assert!(m.posted[0].1.contains("(4000) is above 1000"));

        // 0 turns the rule off.
        let mut c = cfg();
        c.ideas.trial_above = 0;
        let mut m = base();
        m.issues.push(big(7, ""));
        let out = run_with(&mut m, &c, &mut Ledger::default(), &mut roles(), &[]);
        assert_eq!(out.ideas.len(), 1);
    }
}
