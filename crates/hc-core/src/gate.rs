//! The gatekeeper: one reconcile pass over every open pull request and idea.
//!
//! It is written against the [`Forge`] trait so the whole state machine can be
//! tested without a network.

use crate::config::Config;
use crate::model::*;
use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};

pub const COMMENT_MARKER: &str = "<!-- hillclimb-gate -->";
pub const STATUS_CONTEXT: &str = "hillclimb/gate";
pub const LABEL_REJECTED: &str = "hillclimb:rejected";
/// Gates the verify workflow must report as passed.
pub const REQUIRED_RESULT_GATES: &[&str] = &["frozen", "scope", "deny", "improves", "compiles", "same_theorems"];
pub const LABEL_READY: &str = "hillclimb:needs-review";

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

#[derive(Debug, Clone)]
pub struct IssueInfo {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub open: bool,
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
    fn open_prs(&mut self) -> Result<Vec<PullRequest>>;
    /// Latest result for this head produced by the trusted verify workflow.
    fn verify_result(&mut self, pr: u64, head_sha: &str) -> Result<Option<VerifyResult>>;
    /// Pull requests that have a verify run queued or in progress.
    fn active_verifies(&mut self) -> Result<BTreeSet<u64>>;
    fn dispatch_verify(&mut self, pr: u64) -> Result<()>;
    fn reviews(&mut self, pr: u64) -> Result<Vec<ReviewInfo>>;
    fn account_created(&mut self, login: &str) -> Result<DateTime<Utc>>;
    fn set_status(&mut self, sha: &str, state: CommitState, description: &str) -> Result<()>;
    /// Create or update the single gate comment on a pull request.
    fn upsert_comment(&mut self, pr: u64, body: &str) -> Result<()>;
    fn set_label(&mut self, issue: u64, label: &str, present: bool) -> Result<()>;
    /// Squash-merge; returns the new commit on the main branch.
    fn merge(&mut self, pr: &PullRequest, message: &str) -> Result<String>;
    fn idea_issues(&mut self) -> Result<Vec<IssueInfo>>;
    fn comments_since(&mut self, since: Option<DateTime<Utc>>) -> Result<Vec<CommentInfo>>;
    fn assign(&mut self, issue: u64, login: &str, assigned: bool) -> Result<()>;
    fn comment(&mut self, issue: u64, body: &str) -> Result<()>;
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

fn idea_ref(body: &str) -> Option<u64> {
    let re = Regex::new(r"(?i)\b(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?|idea:?)\s+#(\d+)").expect("static regex");
    re.captures(body).and_then(|c| c[1].parse().ok())
}

fn key(pr: u64, sha: &str) -> String {
    format!("{pr}@{sha}")
}

impl<'a, F: Forge> Gatekeeper<'a, F> {
    /// `decided_keys` are the `pr@head_sha` keys already present in the log.
    pub fn run(
        &mut self,
        ledger: &mut Ledger,
        past: &[Attempt],
    ) -> Result<Outcome> {
        let mut out = Outcome::default();
        let decided_keys: BTreeSet<String> = past.iter().map(|a| key(a.pr, &a.head_sha)).collect();
        let mut main = self.forge.main_head()?;
        let active = self.forge.active_verifies()?;
        let mut prs = self.forge.open_prs()?;
        prs.retain(|p| p.base_ref == self.cfg.project.branch && !p.draft);
        prs.sort_by_key(|p| p.number);

        for pr in &prs {
            let result = self.forge.verify_result(pr.number, &pr.head_sha)?;
            let reviews = self.forge.reviews(pr.number)?;
            let changed = result.as_ref().map(|r| r.changed_files).unwrap_or(0);
            let review = self.review_gate(pr, &reviews, changed)?;
            let mut attempt = self.attempt(pr, result.as_ref());

            if pr.mergeable == Some(false) {
                attempt.gates = result.as_ref().map(|r| r.gates.clone()).unwrap_or_default();
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
                attempt.gates = pending_gates(&review);
                let k = key(pr.number, &pr.head_sha);
                let waited = self.now - pr.created_at > Duration::minutes(20);
                let recently = ledger
                    .dispatched
                    .get(&k)
                    .is_some_and(|t| self.now - *t < Duration::hours(3));
                if !active.contains(&pr.number) && waited && !recently {
                    self.forge.dispatch_verify(pr.number)?;
                    ledger.dispatched.insert(k, self.now);
                    out.log.push(format!("#{}: dispatched verification", pr.number));
                }
                self.publish(pr, &attempt, CommitState::Pending, "Verification in progress")?;
                out.open.push(attempt);
                continue;
            };

            let mut gates = result.gates.clone();
            // Anything short of an explicit pass on every required gate is a
            // failure: a skipped or missing check never lets a change through.
            for required in REQUIRED_RESULT_GATES {
                match gates.iter_mut().find(|g| g.key == *required) {
                    Some(g) if g.status == GateStatus::Pending => {
                        g.status = GateStatus::Fail;
                        g.detail = Some("The check did not finish".into());
                    }
                    Some(_) => {}
                    None => gates.push(Gate::new(required, GateStatus::Fail, "The check did not run")),
                }
            }
            let any_failed = gates.iter().any(|g| g.status == GateStatus::Fail);
            for g in gates.iter_mut().filter(|g| g.status == GateStatus::Skip) {
                if !any_failed {
                    g.status = GateStatus::Fail;
                    g.detail = Some("The check did not run".into());
                }
            }
            let failed: Vec<&Gate> = gates.iter().filter(|g| g.status == GateStatus::Fail).collect();

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
            let fresh = result.base_sha == main;

            if !fresh {
                gates.push(Gate::new(
                    "fresh",
                    GateStatus::Pending,
                    if approved {
                        "Main moved; re-verifying on the new main"
                    } else {
                        "Main moved; will be re-verified once approved"
                    },
                ));
                gates.push(review);
                attempt.gates = gates;
                attempt.status = AttemptStatus::Stale;
                if approved && !active.contains(&pr.number) {
                    let k = format!("{}@{}", key(pr.number, &pr.head_sha), main);
                    if !ledger.dispatched.contains_key(&k) {
                        self.forge.dispatch_verify(pr.number)?;
                        ledger.dispatched.insert(k, self.now);
                        out.log.push(format!("#{}: re-verifying on new main", pr.number));
                    }
                }
                self.forge.set_label(pr.number, LABEL_READY, !approved)?;
                self.publish(pr, &attempt, CommitState::Pending, "Waiting for re-verification on current main")?;
                out.open.push(attempt);
                continue;
            }

            gates.push(Gate::new("fresh", GateStatus::Pass, ""));
            gates.push(review);
            attempt.gates = gates;

            if !approved {
                attempt.status = AttemptStatus::AwaitingReview;
                self.forge.set_label(pr.number, LABEL_READY, true)?;
                self.publish(pr, &attempt, CommitState::Pending, "All checks passed; waiting for independent review")?;
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
                    out.current = Some(Point { sha: merge_sha, at: self.now, metrics: result.metrics });
                    out.log.push(format!("#{}: merged ({delta})", pr.number));
                    out.decided.push(attempt);
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
        out.ideas = self.ideas(ledger, &prs, &all, &mut out.log)?;
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
        }
    }

    fn publish(&mut self, pr: &PullRequest, attempt: &Attempt, state: CommitState, text: &str) -> Result<()> {
        self.forge.set_status(&pr.head_sha, state, text)?;
        self.forge.upsert_comment(pr.number, &render_comment(self.cfg, attempt))
    }

    fn review_gate(&mut self, pr: &PullRequest, reviews: &[ReviewInfo], changed_files: usize) -> Result<Gate> {
        let r = &self.cfg.review;
        let needed = if changed_files > r.large_change_files { r.required_large } else { r.required };
        if needed == 0 {
            return Ok(Gate::new("review", GateStatus::Pass, "No review required"));
        }
        // Latest decisive review per reviewer.
        let mut latest: BTreeMap<String, &ReviewInfo> = BTreeMap::new();
        let mut sorted: Vec<&ReviewInfo> = reviews.iter().collect();
        sorted.sort_by_key(|r| r.submitted_at);
        for rv in sorted {
            if rv.user.eq_ignore_ascii_case(&pr.author) {
                continue;
            }
            match rv.state.as_str() {
                "APPROVED" | "CHANGES_REQUESTED" => {
                    latest.insert(rv.user.to_lowercase(), rv);
                }
                "DISMISSED" => {
                    latest.remove(&rv.user.to_lowercase());
                }
                _ => {}
            }
        }
        let mut approvals = Vec::new();
        let mut blocking = Vec::new();
        let mut outdated = 0;
        let mut too_new = Vec::new();
        for rv in latest.values() {
            if rv.state == "CHANGES_REQUESTED" {
                blocking.push(rv.user.clone());
                continue;
            }
            if rv.commit_id != pr.head_sha {
                outdated += 1;
                continue;
            }
            let created = self.forge.account_created(&rv.user)?;
            if self.now - created < Duration::days(r.min_account_age_days) {
                too_new.push(rv.user.clone());
                continue;
            }
            approvals.push(rv.user.clone());
        }
        if !blocking.is_empty() {
            return Ok(Gate::new(
                "review",
                GateStatus::Fail,
                format!("Changes requested by {}", blocking.join(", ")),
            ));
        }
        if approvals.len() as u32 >= needed {
            return Ok(Gate::new("review", GateStatus::Pass, format!("Approved by {}", approvals.join(", "))));
        }
        let mut detail = format!("{} of {} approvals", approvals.len(), needed);
        if outdated > 0 {
            detail.push_str(&format!("; {outdated} approval(s) are for an older commit"));
        }
        if !too_new.is_empty() {
            detail.push_str(&format!(
                "; accounts younger than {} days do not count ({})",
                r.min_account_age_days,
                too_new.join(", ")
            ));
        }
        Ok(Gate::new("review", GateStatus::Pending, detail))
    }

    fn ideas(
        &mut self,
        ledger: &mut Ledger,
        prs: &[PullRequest],
        attempts: &[Attempt],
        log: &mut Vec<String>,
    ) -> Result<Vec<Idea>> {
        let issues = self.forge.idea_issues()?;
        let open: BTreeSet<u64> = issues.iter().filter(|i| i.open).map(|i| i.number).collect();
        let lease = Duration::hours(self.cfg.ideas.lease_hours);

        // Commands.
        let comments = self.forge.comments_since(ledger.comments_seen_until)?;
        for c in &comments {
            if ledger.comments_seen_until.is_some_and(|t| c.created_at <= t) || !open.contains(&c.issue) {
                continue;
            }
            let cmd = c.body.lines().map(str::trim).find(|l| l.starts_with('/')).unwrap_or("");
            let holder = ledger.claims.get(&c.issue).map(|cl| cl.login.clone());
            match cmd.split_whitespace().next().unwrap_or("") {
                "/claim" => match holder {
                    None => {
                        ledger.claims.insert(c.issue, Claim { login: c.user.clone(), at: c.created_at });
                        let _ = self.forge.assign(c.issue, &c.user, true);
                        self.forge.comment(
                            c.issue,
                            &format!(
                                "Claimed by @{} until {}. Open a pull request whose description says `Closes #{}` before then, or the claim is released. Comment `/unclaim` to release it early.",
                                c.user,
                                (c.created_at + lease).format("%Y-%m-%d %H:%M UTC"),
                                c.issue
                            ),
                        )?;
                        log.push(format!("idea #{}: claimed by {}", c.issue, c.user));
                    }
                    Some(h) if h.eq_ignore_ascii_case(&c.user) => {}
                    Some(h) => {
                        self.forge.comment(
                            c.issue,
                            &format!("@{} this idea is already claimed by {h}. Pick another open idea.", c.user),
                        )?;
                    }
                },
                "/unclaim" | "/release" => {
                    if holder.is_some_and(|h| h.eq_ignore_ascii_case(&c.user)) {
                        ledger.claims.remove(&c.issue);
                        let _ = self.forge.assign(c.issue, &c.user, false);
                        self.forge.comment(c.issue, "Claim released. This idea is open again.")?;
                        log.push(format!("idea #{}: released", c.issue));
                    }
                }
                _ => {}
            }
        }
        if let Some(last) = comments.iter().map(|c| c.created_at).max() {
            ledger.comments_seen_until = Some(ledger.comments_seen_until.map_or(last, |t| t.max(last)));
        }

        // Expiry: a claim lapses unless its holder has an open pull request for it.
        let expired: Vec<(u64, String)> = ledger
            .claims
            .iter()
            .filter(|(n, cl)| {
                !open.contains(n)
                    || (self.now - cl.at > lease
                        && !prs.iter().any(|p| {
                            idea_ref(&p.body) == Some(**n) && p.author.eq_ignore_ascii_case(&cl.login)
                        }))
            })
            .map(|(n, cl)| (*n, cl.login.clone()))
            .collect();
        for (n, login) in expired {
            ledger.claims.remove(&n);
            if open.contains(&n) {
                let _ = self.forge.assign(n, &login, false);
                self.forge.comment(n, "The claim expired without a pull request. This idea is open again.")?;
                log.push(format!("idea #{n}: claim expired"));
            }
        }

        let mut ideas = Vec::new();
        for i in issues {
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
            } else if claim.is_some() || open_pr.is_some() {
                IdeaStatus::Claimed
            } else {
                IdeaStatus::Open
            };
            ideas.push(Idea {
                number: i.number,
                title: i.title,
                status,
                author: i.author,
                claimed_by: claim.map(|c| c.login.clone()).or(open_pr.map(|p| p.author.clone())),
                lease_expires: claim.map(|c| c.at + lease),
                pr: merged.map(|a| a.pr).or(open_pr.map(|p| p.number)),
                result_delta: merged.and_then(|a| a.delta),
                created_at: i.created_at,
                url: i.url,
            });
        }
        ideas.sort_by_key(|i| std::cmp::Reverse(i.number));
        Ok(ideas)
    }
}

fn pending_gates(review: &Gate) -> Vec<Gate> {
    let mut gates: Vec<Gate> = GATES
        .iter()
        .filter(|g| g.0 != "review")
        .map(|g| Gate::new(g.0, GateStatus::Pending, ""))
        .collect();
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
    let headline = match a.status {
        AttemptStatus::Verifying => "Verification is running.".to_string(),
        AttemptStatus::AwaitingReview => "All automatic gates passed. Waiting for an independent review.".to_string(),
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
        reviews: BTreeMap<u64, Vec<ReviewInfo>>,
        young: BTreeSet<String>,
        active: BTreeSet<u64>,
        issues: Vec<IssueInfo>,
        comments: Vec<CommentInfo>,
        merged: Vec<u64>,
        dispatched: Vec<u64>,
        statuses: BTreeMap<String, CommitState>,
        posted: Vec<(u64, String)>,
        assigned: Vec<(u64, String, bool)>,
        labels: Vec<(u64, String, bool)>,
    }

    impl Forge for Mock {
        fn main_head(&mut self) -> Result<String> {
            Ok(self.main.clone())
        }
        fn open_prs(&mut self) -> Result<Vec<PullRequest>> {
            Ok(self.prs.clone())
        }
        fn verify_result(&mut self, pr: u64, head: &str) -> Result<Option<VerifyResult>> {
            Ok(self.results.get(&key(pr, head)).cloned())
        }
        fn active_verifies(&mut self) -> Result<BTreeSet<u64>> {
            Ok(self.active.clone())
        }
        fn dispatch_verify(&mut self, pr: u64) -> Result<()> {
            self.dispatched.push(pr);
            Ok(())
        }
        fn reviews(&mut self, pr: u64) -> Result<Vec<ReviewInfo>> {
            Ok(self.reviews.get(&pr).cloned().unwrap_or_default())
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
        fn idea_issues(&mut self) -> Result<Vec<IssueInfo>> {
            Ok(self.issues.clone())
        }
        fn comments_since(&mut self, _: Option<DateTime<Utc>>) -> Result<Vec<CommentInfo>> {
            Ok(self.comments.clone())
        }
        fn assign(&mut self, issue: u64, login: &str, on: bool) -> Result<()> {
            self.assigned.push((issue, login.into(), on));
            Ok(())
        }
        fn comment(&mut self, issue: u64, body: &str) -> Result<()> {
            self.posted.push((issue, body.into()));
            Ok(())
        }
    }

    fn pr(n: u64, author: &str) -> PullRequest {
        PullRequest {
            number: n,
            title: format!("change {n}"),
            author: author.into(),
            body: String::new(),
            head_sha: format!("head{n}"),
            base_ref: "main".into(),
            created_at: t(-1),
            url: format!("https://x/pull/{n}"),
            draft: false,
            mergeable: Some(true),
            labels: vec![],
        }
    }

    fn result(n: u64, base: &str, tokens: u64, pass: bool) -> VerifyResult {
        let gate = |k: &str, ok: bool| Gate::new(k, if ok { GateStatus::Pass } else { GateStatus::Fail }, "");
        VerifyResult {
            pr: n,
            head_sha: format!("head{n}"),
            base_sha: base.into(),
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

    fn run(m: &mut Mock, ledger: &mut Ledger, past: &[Attempt]) -> Outcome {
        let c = cfg();
        Gatekeeper { cfg: &c, forge: m, now: t(0) }.run(ledger, past).unwrap()
    }

    fn base() -> Mock {
        Mock { main: "m0".into(), ..Default::default() }
    }

    #[test]
    fn green_and_reviewed_merges() {
        let mut m = base();
        m.prs.push(pr(1, "alice"));
        m.results.insert(key(1, "head1"), result(1, "m0", 900, true));
        m.reviews.insert(1, vec![approve("bob", "head1")]);
        let out = run(&mut m, &mut Ledger::default(), &[]);
        assert_eq!(m.merged, [1]);
        assert_eq!(out.decided[0].status, AttemptStatus::Merged);
        assert_eq!(out.decided[0].delta, Some(-100));
        assert_eq!(out.current.unwrap().metrics.tokens, 900);
        assert_eq!(m.statuses["head1"], CommitState::Success);
    }

    #[test]
    fn green_without_review_waits() {
        let mut m = base();
        m.prs.push(pr(1, "alice"));
        m.results.insert(key(1, "head1"), result(1, "m0", 900, true));
        let out = run(&mut m, &mut Ledger::default(), &[]);
        assert!(m.merged.is_empty());
        assert_eq!(out.open[0].status, AttemptStatus::AwaitingReview);
    }

    #[test]
    fn self_approval_stale_approval_and_new_accounts_do_not_count() {
        let mut m = base();
        m.prs.push(pr(1, "alice"));
        m.results.insert(key(1, "head1"), result(1, "m0", 900, true));
        m.young.insert("newbie".into());
        m.reviews.insert(1, vec![approve("Alice", "head1"), approve("bob", "older"), approve("newbie", "head1")]);
        run(&mut m, &mut Ledger::default(), &[]);
        assert!(m.merged.is_empty());
    }

    #[test]
    fn changes_requested_blocks_even_with_an_approval() {
        let mut m = base();
        m.prs.push(pr(1, "alice"));
        m.results.insert(key(1, "head1"), result(1, "m0", 900, true));
        let mut block = approve("carol", "head1");
        block.state = "CHANGES_REQUESTED".into();
        m.reviews.insert(1, vec![approve("bob", "head1"), block]);
        run(&mut m, &mut Ledger::default(), &[]);
        assert!(m.merged.is_empty());
    }

    #[test]
    fn failed_gate_rejects_once() {
        let mut m = base();
        m.prs.push(pr(1, "alice"));
        m.results.insert(key(1, "head1"), result(1, "m0", 1000, false));
        m.reviews.insert(1, vec![approve("bob", "head1")]);
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
    fn second_ready_pr_goes_stale_after_first_merges_and_is_reverified() {
        let mut m = base();
        m.prs.push(pr(1, "alice"));
        m.prs.push(pr(2, "dave"));
        for n in [1, 2] {
            m.results.insert(key(n, &format!("head{n}")), result(n, "m0", 900, true));
            m.reviews.insert(n, vec![approve("bob", &format!("head{n}"))]);
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
        m.results.insert(key(1, "head1"), result(1, "old", 900, true));
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
    fn conflict_is_reported() {
        let mut m = base();
        let mut p = pr(1, "alice");
        p.mergeable = Some(false);
        m.prs.push(p);
        m.results.insert(key(1, "head1"), result(1, "m0", 900, true));
        m.reviews.insert(1, vec![approve("bob", "head1")]);
        let out = run(&mut m, &mut Ledger::default(), &[]);
        assert!(m.merged.is_empty());
        assert_eq!(out.open[0].status, AttemptStatus::Conflict);
    }

    #[test]
    fn skipped_or_missing_checks_never_merge() {
        for mutate in [0, 1] {
            let mut m = base();
            m.prs.push(pr(1, "alice"));
            let mut r = result(1, "m0", 900, true);
            if mutate == 0 {
                r.gates.iter_mut().find(|g| g.key == "same_theorems").unwrap().status = GateStatus::Skip;
            } else {
                r.gates.retain(|g| g.key != "compiles");
            }
            m.results.insert(key(1, "head1"), r);
            m.reviews.insert(1, vec![approve("bob", "head1")]);
            let out = run(&mut m, &mut Ledger::default(), &[]);
            assert!(m.merged.is_empty());
            assert_eq!(out.decided[0].status, AttemptStatus::Rejected);
        }
    }

    #[test]
    fn conflict_without_a_result_is_reported_not_redispatched() {
        let mut m = base();
        let mut p = pr(1, "alice");
        p.mergeable = Some(false);
        m.prs.push(p);
        let out = run(&mut m, &mut Ledger::default(), &[]);
        assert_eq!(out.open[0].status, AttemptStatus::Conflict);
        assert!(m.dispatched.is_empty());
    }

    fn issue(n: u64, open: bool) -> IssueInfo {
        IssueInfo { number: n, title: format!("idea {n}"), author: "eve".into(), open, created_at: t(-48), url: String::new() }
    }

    fn says(issue: u64, user: &str, body: &str, h: i64) -> CommentInfo {
        CommentInfo { issue, user: user.into(), body: body.into(), created_at: t(h) }
    }

    #[test]
    fn claim_then_second_claimant_is_turned_away() {
        let mut m = base();
        m.issues.push(issue(7, true));
        m.comments = vec![says(7, "alice", "/claim", -1), says(7, "dave", "I'll take it\n/claim", 0)];
        let mut ledger = Ledger::default();
        let out = run(&mut m, &mut ledger, &[]);
        assert_eq!(ledger.claims[&7].login, "alice");
        assert_eq!(out.ideas[0].status, IdeaStatus::Claimed);
        assert_eq!(out.ideas[0].claimed_by.as_deref(), Some("alice"));
        assert_eq!(m.posted.len(), 2);
        // Already-seen comments are not processed again.
        run(&mut m, &mut ledger, &[]);
        assert_eq!(m.posted.len(), 2);
    }

    #[test]
    fn claim_expires_without_a_pull_request_but_not_with_one() {
        let mut m = base();
        m.issues.push(issue(7, true));
        let mut ledger = Ledger::default();
        ledger.claims.insert(7, Claim { login: "alice".into(), at: t(-5) });
        let mut p = pr(1, "alice");
        p.body = "Closes #7".into();
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
        let mut m = base();
        m.issues.push(issue(7, true));
        let mut p = pr(1, "alice");
        p.body = "Shorter proof.\n\nCloses #7".into();
        m.prs.push(p);
        m.results.insert(key(1, "head1"), result(1, "m0", 900, true));
        m.reviews.insert(1, vec![approve("bob", "head1")]);
        let out = run(&mut m, &mut Ledger::default(), &[]);
        assert_eq!(out.ideas[0].status, IdeaStatus::Done);
        assert_eq!(out.ideas[0].result_delta, Some(-100));
    }

    #[test]
    fn comment_lists_every_gate() {
        let mut m = base();
        m.prs.push(pr(1, "alice"));
        m.results.insert(key(1, "head1"), result(1, "m0", 900, true));
        let out = run(&mut m, &mut Ledger::default(), &[]);
        let body = render_comment(&cfg(), &out.open[0]);
        assert!(body.starts_with(COMMENT_MARKER));
        for g in GATES {
            assert!(body.contains(g.1), "missing {}", g.1);
        }
    }
}
