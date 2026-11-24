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
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};

pub const COMMENT_MARKER: &str = "<!-- hillclimb-gate -->";
pub const STATUS_CONTEXT: &str = "hillclimb/gate";
pub const LABEL_REJECTED: &str = "hillclimb:rejected";
pub const LABEL_READY: &str = "hillclimb:needs-review";
pub const LABEL_PROPOSED: &str = "idea:proposed";
pub const LABEL_APPLICATION: &str = "application";
pub const LABEL_ALERT: &str = "hillclimb:alert";
/// How many approved changes may be verified stacked on one another.
pub const MAX_STACK: usize = 6;
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
    fn main_verifies(&mut self, main: &str) -> Result<Option<bool>>;
    fn open_prs(&mut self) -> Result<Vec<PullRequest>>;
    /// Recently merged pull requests with their merge commit and time.
    fn merged_prs(&mut self) -> Result<Vec<(PullRequest, String, DateTime<Utc>)>>;
    /// Results for this head produced by the trusted verify workflow, newest first.
    fn verify_results(&mut self, pr: u64, head_sha: &str) -> Result<Vec<VerifyResult>>;
    /// Pull requests that have a verify run queued or in progress.
    fn active_verifies(&mut self) -> Result<BTreeSet<u64>>;
    /// Verify `pr` on top of main with the pull requests in `stack` applied
    /// first, in order. `pr == 0` builds main itself.
    fn dispatch_verify(&mut self, pr: u64, stack: &[u64]) -> Result<()>;
    fn reviews(&mut self, pr: u64) -> Result<Vec<ReviewInfo>>;
    /// Every comment on a pull request written by a person.
    fn pr_comments(&mut self, pr: u64) -> Result<Vec<CommentInfo>>;
    fn account_created(&mut self, login: &str) -> Result<DateTime<Utc>>;
    fn set_status(&mut self, sha: &str, state: CommitState, description: &str) -> Result<()>;
    /// Create or update the single gate comment on a pull request.
    fn upsert_comment(&mut self, pr: u64, body: &str) -> Result<()>;
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

fn names_an_idea(body: &str) -> bool {
    Regex::new(r"(?im)^\s*idea:\s*(#\d+|none)\s*$").expect("static regex").is_match(body)
        || idea_ref(body).is_some()
}

fn key(pr: u64, sha: &str) -> String {
    format!("{pr}@{sha}")
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
        if self.forge.main_verifies(&main)? == Some(false) {
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
        let mut prs = self.forge.open_prs()?;
        prs.retain(|p| p.base_ref == self.cfg.project.branch && !p.draft);
        prs.sort_by_key(|p| p.number);
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
            attempt.merge_sha = Some(merge_sha);
            out.log.push(format!("#{}: merge recovered into the log", pr.number));
            out.decided.push(attempt);
        }

        // Commands first, so a role granted in this pass already counts below.
        self.commands(ledger, roles, &prs, &mut issues, past, &active, &mut out.log)?;

        for pr in &prs {
            // A result is valid for exactly the tree it was applied to. Prefer the
            // one for today's main; otherwise the newest one made on main alone
            // says whether the change is sound in itself; a result stacked on
            // other changes says nothing until those have merged.
            let results = self.forge.verify_results(pr.number, &pr.head_sha)?;
            let is_fresh = |r: &VerifyResult| match &r.base_tree {
                Some(t) => *t == main_tree,
                None => r.base_sha == main,
            };
            let fresh_result = results.iter().find(|r| is_fresh(r));
            let fresh = fresh_result.is_some();
            let speculative_only = !fresh && !results.is_empty() && results.iter().all(|r| !r.stack.is_empty());
            let result = fresh_result
                .or(results.iter().find(|r| r.stack.is_empty()))
                .or(results.first())
                .cloned();
            let changed = result.as_ref().map(|r| r.changed_files).unwrap_or(0);
            let review = self.review_gate(pr, roles, changed)?;
            let (format, parsed) = self.format_gate(pr);
            let format_ok = format.status == GateStatus::Pass;
            let mut attempt = self.attempt(pr, result.as_ref());
            if let Some((technique, scope, _)) = parsed {
                attempt.technique = Some(technique);
                attempt.scope = Some(scope);
            }

            if pr.mergeable == Some(false) {
                attempt.gates = vec![format];
                attempt.gates.extend(result.as_ref().map(|r| r.gates.clone()).unwrap_or_default());
                attempt.gates.push(Gate::new(
                    "fresh",
                    GateStatus::Fail,
                    "Conflicts with main; merge main into your branch and push",
                ));
                attempt.gates.push(review);
                attempt.status = AttemptStatus::Conflict;
                self.publish(pr, &attempt, CommitState::Failure, "Conflicts with main")?;
                out.open.push(attempt);
                continue;
            }

            let Some(result) = result else {
                // Not verified yet. Re-dispatch if the automatic trigger was lost.
                attempt.gates = pending_gates(&format, &review);
                let k = key(pr.number, &pr.head_sha);
                let waited = self.now - pr.created_at > Duration::minutes(20);
                let recently = ledger
                    .dispatched
                    .get(&k)
                    .is_some_and(|t| self.now - *t < Duration::hours(3));
                if !active.contains(&pr.number) && waited && !recently {
                    self.forge.dispatch_verify(pr.number, &[])?;
                    ledger.dispatched.insert(k, self.now);
                    out.log.push(format!("#{}: dispatched verification", pr.number));
                }
                self.publish(pr, &attempt, CommitState::Pending, "Verification in progress")?;
                out.open.push(attempt);
                continue;
            };

            let mut gates = vec![format];
            gates.extend(result.gates.iter().cloned());
            // Anything short of an explicit pass on every required gate is a
            // failure: a skipped or missing check never lets a change through.
            for required in REQUIRED_RESULT_GATES.iter().filter(|_| !speculative_only) {
                match gates.iter_mut().find(|g| g.key == *required) {
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
                    g.detail = Some("Verified only together with changes queued ahead of it".into());
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
                gates.push(Gate::new("fresh", GateStatus::Skip, ""));
                gates.push(Gate::new("review", GateStatus::Skip, ""));
                attempt.gates = gates;
                attempt.status = AttemptStatus::Rejected;
                attempt.reason = Some(reason);
                attempt.decided_at = Some(self.now);
                self.publish(pr, &attempt, CommitState::Failure, "Rejected: a gate failed")?;
                self.forge.set_label(pr.number, LABEL_REJECTED, true)?;
                self.forge.set_label(pr.number, LABEL_READY, false)?;
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

            if !fresh {
                gates.push(Gate::new(
                    "fresh",
                    GateStatus::Pending,
                    if approved && format_ok {
                        "Main moved; re-verifying on the new main"
                    } else {
                        "Main moved; will be re-verified once approved"
                    },
                ));
                gates.push(review);
                attempt.gates = gates;
                attempt.status = AttemptStatus::Stale;
                if approved && format_ok {
                    // Verify on top of the approved changes ahead of this one,
                    // so the whole line can merge without a rebuild per merge.
                    let stack: Vec<u64> = ahead.iter().copied().take(MAX_STACK).collect();
                    if !active.contains(&pr.number) && ahead.len() <= MAX_STACK {
                        let heads: Vec<String> = stack
                            .iter()
                            .map(|n| prs.iter().find(|p| p.number == *n).map(|p| key(*n, &p.head_sha)).unwrap_or_default())
                            .collect();
                        let k = format!("{}@{}@{}", key(pr.number, &pr.head_sha), main_tree, heads.join("+"));
                        if !ledger.dispatched.contains_key(&k) {
                            self.forge.dispatch_verify(pr.number, &stack)?;
                            ledger.dispatched.insert(k, self.now);
                            out.log.push(if stack.is_empty() {
                                format!("#{}: re-verifying on new main", pr.number)
                            } else {
                                format!("#{}: verifying on top of {:?}", pr.number, stack)
                            });
                        }
                    }
                    ahead.push(pr.number);
                }
                self.forge.set_label(pr.number, LABEL_READY, !approved)?;
                self.publish(pr, &attempt, CommitState::Pending, "Waiting for re-verification on current main")?;
                out.open.push(attempt);
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
                self.publish(pr, &attempt, CommitState::Pending, text)?;
                out.open.push(attempt);
                continue;
            }

            // Everything is green: merge.
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
                    self.forge.upsert_comment(pr.number, &render_comment(self.cfg, &attempt))?;
                    main = merge_sha.clone();
                    main_tree = self.forge.main_tree()?;
                    out.current = Some(Point { sha: merge_sha.clone(), at: self.now, metrics: result.metrics });
                    out.log.push(format!("#{}: merged ({delta})", pr.number));
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

        // Forget dispatch records for pull requests that are gone.
        let open_numbers: BTreeSet<String> = prs.iter().map(|p| format!("{}@", p.number)).collect();
        ledger.dispatched.retain(|k, _| open_numbers.iter().any(|p| k.starts_with(p)));

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
        }
    }

    fn publish(&mut self, pr: &PullRequest, attempt: &Attempt, state: CommitState, text: &str) -> Result<()> {
        self.forge.set_status(&pr.head_sha, state, text)?;
        self.forge.upsert_comment(pr.number, &render_comment(self.cfg, attempt))
    }

    /// Title and description follow the standard. Editing them needs no new push.
    fn format_gate(&self, pr: &PullRequest) -> (Gate, Option<(String, String, String)>) {
        let parsed = self.cfg.format.parse_title(&pr.title);
        let mut problems = Vec::new();
        if parsed.is_none() {
            problems.push(format!("The title must be {}", self.cfg.format.title_help()));
        }
        if !names_an_idea(&pr.body) {
            problems.push(
                "The description must contain a line `Idea: #<number>` (the idea this implements) or `Idea: none`"
                    .to_string(),
            );
        }
        let gate = if problems.is_empty() {
            Gate::new("format", GateStatus::Pass, "")
        } else {
            Gate::new(
                "format",
                GateStatus::Fail,
                format!("{}. Edit the pull request; no new push is needed.", problems.join(". ")),
            )
        };
        (gate, parsed)
    }

    /// Approvals count only from vetted reviewers and leaders. A verdict is
    /// either a GitHub review or a comment `/approve <commit>` or
    /// `/request-changes`; the latest verdict of each reviewer wins.
    fn review_gate(&mut self, pr: &PullRequest, roles: &RolesState, changed_files: usize) -> Result<Gate> {
        let r = &self.cfg.review;
        let needed = if changed_files > r.large_change_files { r.required_large } else { r.required };
        if needed == 0 {
            return Ok(Gate::new("review", GateStatus::Pass, "No review required"));
        }
        let mut verdicts = self.forge.reviews(pr.number)?;
        for c in self.forge.pr_comments(pr.number)? {
            let cmd = command(&c.body);
            match cmd.first().map(String::as_str) {
                Some("/approve") => {
                    let sha = cmd.get(1).map(String::as_str).unwrap_or("");
                    let current = sha.len() >= 7 && pr.head_sha.starts_with(sha);
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
        if !blocking.is_empty() {
            return Ok(Gate::new(
                "review",
                GateStatus::Fail,
                format!("Changes requested by {}", blocking.join(", ")),
            ));
        }
        if approvals.len() as u32 >= needed {
            let own = approvals.iter().any(|a| a.eq_ignore_ascii_case(&pr.author));
            let count = approvals.len();
            approvals.dedup();
            return Ok(Gate::new(
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
            ));
        }
        let mut detail = format!("{} of {} approvals from vetted reviewers", approvals.len(), needed);
        if outdated > 0 {
            detail.push_str(&format!("; {outdated} approval(s) are for an older commit"));
        }
        if !unvetted.is_empty() {
            detail.push_str(&format!(
                "; approvals from {} do not count because they are not vetted reviewers yet",
                unvetted.into_iter().collect::<Vec<_>>().join(", ")
            ));
        }
        Ok(Gate::new("review", GateStatus::Pending, detail))
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
                    self.forge.dispatch_verify(n, &[])?;
                    ledger.dispatched.insert(k, self.now);
                    log.push(format!("#{n}: retry requested by {}", c.user));
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
            if self.cfg.format.parse_idea_title(&i.title).is_none() || !missing.is_empty() {
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
            let merged = attempts
                .iter()
                .rev()
                .find(|a| a.idea == Some(i.number) && a.status == AttemptStatus::Merged);
            let open_pr = prs.iter().find(|p| idea_ref(&p.body) == Some(i.number));
            let claim = ledger.claims.get(&i.number);
            let status = if merged.is_some() {
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
                result_delta: merged.and_then(|a| a.delta),
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
    let headline = match a.status {
        AttemptStatus::Verifying => "Verification is running.".to_string(),
        AttemptStatus::AwaitingReview if format_failed => {
            "The checks passed, but the title or description does not follow the standard. Edit the pull request; no new push is needed.".to_string()
        }
        AttemptStatus::AwaitingReview => format!(
            "All automatic gates passed. Waiting for a vetted reviewer, who approves with a review or the comment `/approve {}`.",
            &a.head_sha[..a.head_sha.len().min(10)]
        ),
        AttemptStatus::Stale => "Main moved since this was verified.".to_string(),
        AttemptStatus::Conflict => "This branch conflicts with main.".to_string(),
        AttemptStatus::Merged => "Merged. Thank you.".to_string(),
        AttemptStatus::Rejected => "Not mergeable yet: a gate failed. Push a fix to try again.".to_string(),
    };
    let mut s = format!("{COMMENT_MARKER}\n### Hillclimb gate\n\n{headline}\n\n");
    if let (Some(m), Some(b)) = (a.metrics, a.base_metrics) {
        s.push_str("| Metric | Main | This change | Δ |\n|---|--:|--:|--:|\n");
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
        stacks: Vec<Vec<u64>>,
        reviews: BTreeMap<u64, Vec<ReviewInfo>>,
        pr_comments: BTreeMap<u64, Vec<CommentInfo>>,
        young: BTreeSet<String>,
        active: BTreeSet<u64>,
        issues: Vec<IssueInfo>,
        comments: Vec<CommentInfo>,
        merged: Vec<u64>,
        dispatched: Vec<u64>,
        statuses: BTreeMap<String, CommitState>,
        posted: Vec<(u64, String)>,
        labels: Vec<(u64, String, bool)>,
        closed: Vec<u64>,
        created: Vec<String>,
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
        fn main_verifies(&mut self, _: &str) -> Result<Option<bool>> {
            Ok(self.main_ok)
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
        fn dispatch_verify(&mut self, pr: u64, stack: &[u64]) -> Result<()> {
            self.dispatched.push(pr);
            self.stacks.push(stack.to_vec());
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
        fn set_status(&mut self, sha: &str, state: CommitState, _: &str) -> Result<()> {
            self.statuses.insert(sha.into(), state);
            Ok(())
        }
        fn upsert_comment(&mut self, _: u64, _: &str) -> Result<()> {
            Ok(())
        }
        fn set_label(&mut self, issue: u64, label: &str, present: bool) -> Result<()> {
            self.labels.push((issue, label.into(), present));
            Ok(())
        }
        fn merge(&mut self, pr: &PullRequest, _: &str) -> Result<String> {
            self.merged.push(pr.number);
            self.main = format!("merge{}", pr.number);
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
            head_sha: format!("head{n}abcdef"),
            base_ref: "main".into(),
            created_at: t(-1),
            url: format!("https://x/pull/{n}"),
            draft: false,
            mergeable: Some(true),
            labels: vec![],
        }
    }

    fn head(n: u64) -> String {
        format!("head{n}abcdef")
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

    fn approve(user: &str, commit: &str) -> ReviewInfo {
        ReviewInfo { user: user.into(), state: "APPROVED".into(), commit_id: commit.into(), submitted_at: t(0) }
    }

    /// Roles as they stand in most tests: `lead` leads, `bob` reviews.
    fn roles() -> RolesState {
        let mut r = RolesState::default();
        r.grant("bob", Role::Reviewer, vec!["lead".into()], t(-100));
        r
    }

    fn run_with(m: &mut Mock, c: &Config, ledger: &mut Ledger, roles: &mut RolesState, past: &[Attempt]) -> Outcome {
        Gatekeeper { cfg: c, forge: m, now: t(0) }.run(ledger, roles, past).unwrap()
    }

    fn run(m: &mut Mock, ledger: &mut Ledger, past: &[Attempt]) -> Outcome {
        run_with(m, &cfg(), ledger, &mut roles(), past)
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
        CommentInfo { issue, user: user.into(), body: body.into(), created_at: t(h) }
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
        m.pr_comments.insert(1, vec![says(1, "bob", "/approve head1abcdef", 0)]);
        run(&mut m, &mut Ledger::default(), &[]);
        assert!(m.merged.is_empty());
    }

    #[test]
    fn comment_approval_needs_the_current_commit() {
        let mut m = green();
        m.pr_comments.insert(1, vec![says(1, "bob", "Looks right.\n/approve 0123456", 0)]);
        run(&mut m, &mut Ledger::default(), &[]);
        assert!(m.merged.is_empty());

        m.pr_comments.insert(1, vec![says(1, "bob", "Looks right.\n/approve head1ab", 0)]);
        run(&mut m, &mut Ledger::default(), &[]);
        assert_eq!(m.merged, [1]);
    }

    #[test]
    fn bootstrap_lets_a_leader_review_changes_from_the_leaders_account() {
        let mut m = green();
        m.prs[0].author = "lead".into();
        m.pr_comments.insert(1, vec![says(1, "lead", "/approve head1abcdef", 0)]);
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
        m.pr_comments.insert(1, vec![says(1, "lead", "/approve head1abcdef", 0)]);
        run_with(&mut m, &c, &mut Ledger::default(), &mut roles(), &[]);
        assert!(m.merged.is_empty(), "one review is not enough for a large change");

        // A second, separate review arrives; a request for changes in between would reset the count.
        m.pr_comments.insert(
            1,
            vec![
                says(1, "lead", "/approve head1abcdef", 0),
                says(1, "lead", "/request-changes", 1),
                says(1, "lead", "/approve head1abcdef", 2),
            ],
        );
        run_with(&mut m, &c, &mut Ledger::default(), &mut roles(), &[]);
        assert!(m.merged.is_empty());
        m.pr_comments.get_mut(&1).unwrap().push(says(1, "lead", "/approve head1abcdef", 3));
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
            assert_eq!(out.open[0].status, AttemptStatus::AwaitingReview);
            assert_eq!(out.open[0].gates[0].key, "format");
            assert_eq!(out.open[0].gates[0].status, GateStatus::Fail);
            assert!(out.decided.is_empty(), "a format problem is fixed by editing, not logged as a rejection");
        }
    }

    #[test]
    fn failed_gate_rejects_once() {
        let mut m = base();
        m.prs.push(pr(1, "alice"));
        m.results.insert(key(1, &head(1)), result(1, "m0", 1000, false));
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

    fn on_tree(n: u64, base_tree: &str, stack: &[&str], pass: bool) -> VerifyResult {
        let mut r = result(n, "m0", 900, pass);
        r.base_tree = Some(base_tree.into());
        r.merged_tree = Some(format!("tree:merge{n}"));
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
        m.extra_results.push(on_tree(2, "tree:merge1", &["1@head1abcdef"], true));
        let out = run(&mut m, &mut ledger, &[]);
        assert_eq!(m.merged, [1, 2], "both merge in one pass, with no rebuild in between");
        assert_eq!(m.dispatched.len(), 2);
        assert_eq!(out.decided.len(), 2);
    }

    #[test]
    fn a_failure_seen_only_in_a_stack_decides_nothing_until_the_stack_has_merged() {
        let mut m = two_stale();
        m.results.remove(&key(2, &head(2)));
        m.extra_results.push(on_tree(2, "tree:merge1", &["1@head1abcdef"], false));
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
        m.extra_results.push(on_tree(2, "tree:merge1", &["1@head1abcdef"], true));
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
        assert_eq!(out.ideas[0].status, IdeaStatus::Done);
        assert_eq!(out.ideas[0].result_delta, Some(-100));
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
        assert!(body.contains("/approve head1abcde"));
        for g in GATES.iter().filter(|g| g.0 != "quota") {
            assert!(body.contains(g.1), "missing {}", g.1);
        }
    }
}
