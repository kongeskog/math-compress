//! [`Forge`] backed by the `gh` command line tool, which is preinstalled and
//! authenticated on GitHub-hosted runners.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration, Utc};
use hc_core::gate::*;
use hc_core::model::VerifyResult;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

/// Every state of a workflow run that is not finished. Between two jobs a run
/// is briefly neither queued nor in progress; missing it there once made the
/// gatekeeper replace a verification seconds before it published its result.
const UNFINISHED: [&str; 5] = ["queued", "in_progress", "pending", "waiting", "requested"];

/// The step of the verify workflow that fails only when main's own content
/// did not build or did not pass the independent check.
const MAIN_VERDICT_STEP: &str = "Fail if main itself does not verify";

pub struct GhForge {
    pub repo: String,
    pub branch: String,
    pub workflow: String,
    pub dry_run: bool,
    comment_cache: BTreeMap<u64, Vec<Value>>,
    labels: BTreeMap<u64, BTreeSet<String>>,
    accounts: BTreeMap<String, DateTime<Utc>>,
}

fn s(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn time(v: &Value, key: &str) -> DateTime<Utc> {
    v.get(key)
        .and_then(Value::as_str)
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or_else(Utc::now)
}

impl GhForge {
    pub fn new(repo: &str, branch: &str, workflow: &str, dry_run: bool) -> GhForge {
        GhForge {
            repo: repo.into(),
            branch: branch.into(),
            workflow: workflow.into(),
            dry_run,
            comment_cache: BTreeMap::new(),
            labels: BTreeMap::new(),
            accounts: BTreeMap::new(),
        }
    }

    fn gh(&self, args: &[&str]) -> Result<String> {
        let out = Command::new("gh").args(args).output().context("running gh")?;
        if !out.status.success() {
            bail!("gh {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn get(&self, path: &str) -> Result<Value> {
        let text = self.gh(&["api", &format!("repos/{}/{}", self.repo, path)])?;
        Ok(serde_json::from_str(&text)?)
    }

    /// GET every page of a list endpoint.
    fn list(&self, path: &str) -> Result<Vec<Value>> {
        let text = self.gh(&["api", "--paginate", "--slurp", &format!("repos/{}/{}", self.repo, path)])?;
        let pages: Vec<Value> = serde_json::from_str(&text)?;
        Ok(pages
            .into_iter()
            .flat_map(|p| match p {
                Value::Array(items) => items,
                other => vec![other],
            })
            .collect())
    }

    /// A write call. Skipped (and logged) in dry-run mode.
    fn write(&self, method: &str, path: &str, fields: &[(&str, &str)]) -> Result<Value> {
        if self.dry_run {
            eprintln!("dry-run: {method} {path} {fields:?}");
            return Ok(Value::Null);
        }
        let url = format!("repos/{}/{}", self.repo, path);
        let mut args = vec!["api".to_string(), "-X".into(), method.into(), url];
        for (k, v) in fields {
            args.push("-f".into());
            args.push(format!("{k}={v}"));
        }
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let text = self.gh(&refs)?;
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    /// All comments of an issue or pull request, fetched once per pass.
    fn issue_comments(&mut self, number: u64) -> Result<Vec<Value>> {
        if let Some(c) = self.comment_cache.get(&number) {
            return Ok(c.clone());
        }
        let c = self.list(&format!("issues/{number}/comments?per_page=100"))?;
        self.comment_cache.insert(number, c.clone());
        Ok(c)
    }

    /// Descriptions of the open pull requests against the branch (drafts
    /// excluded), for resolving `Depends` lines outside a gatekeeper pass.
    pub fn open_bodies(&self) -> Result<BTreeMap<u64, String>> {
        Ok(self
            .list("pulls?state=open&per_page=100")?
            .iter()
            .filter(|p| s(&p["base"], "ref") == self.branch && !p["draft"].as_bool().unwrap_or(false))
            .map(|p| (p["number"].as_u64().unwrap_or(0), s(p, "body")))
            .collect())
    }

    /// Reservations in force, read from the issues alone: open ideas with
    /// the queue's label that are assigned (the gatekeeper assigns an idea
    /// to its claimant) and have a Reserves section.
    pub fn reservations(&self, label: &str) -> Result<Vec<Reservation>> {
        let mut out = Vec::new();
        for i in self.list(&format!("issues?state=open&labels={label}&per_page=100"))? {
            if i.get("pull_request").is_some() {
                continue;
            }
            let Some(claimant) = i["assignees"].as_array().and_then(|a| a.first()).map(|a| s(a, "login")) else {
                continue;
            };
            let patterns = reserves(&s(&i, "body"));
            if !patterns.is_empty() {
                out.push(Reservation { idea: i["number"].as_u64().unwrap_or(0), claimant, patterns });
            }
        }
        Ok(out)
    }

    fn dispatch(&mut self, pr: u64, stack: &[u64], min_delta: Option<u64>) -> Result<()> {
        let stack = stack.iter().map(u64::to_string).collect::<Vec<_>>().join(" ");
        if self.dry_run {
            eprintln!("dry-run: dispatch verify for #{pr} on [{stack}] (minimum: {min_delta:?})");
            return Ok(());
        }
        let (pr_arg, stack_arg) = (format!("pr={pr}"), format!("stack={stack}"));
        let mut args =
            vec!["workflow", "run", &self.workflow, "-R", &self.repo, "--ref", &self.branch, "-f", &pr_arg, "-f", &stack_arg];
        // Sent only when a minimum is waived, so that an arena whose workflow
        // does not take the input yet keeps working for everything else.
        let min_arg = min_delta.map(|m| format!("min_delta={m}"));
        if let Some(m) = &min_arg {
            args.extend(["-f", m.as_str()]);
        }
        self.gh(&args)?;
        Ok(())
    }

    fn trusted_run(&self, run_id: u64) -> Result<Option<String>> {
        let run = self.get(&format!("actions/runs/{run_id}"))?;
        let event = s(&run, "event");
        let same_repo = |k: &str| run.get(k).map(|r| s(r, "full_name")) == Some(self.repo.clone());
        let our_workflow = s(&run, "path").starts_with(&format!(".github/workflows/{}", self.workflow))
            && same_repo("repository");
        let ok = our_workflow
            && match event.as_str() {
                // Always runs the definition from the base branch, whatever the
                // head is; the head may be a fork.
                "pull_request_target" => true,
                // Only people with write access can dispatch; it must be from main.
                "workflow_dispatch" => s(&run, "head_branch") == self.branch && same_repo("head_repository"),
                _ => false,
            };
        Ok(ok.then(|| s(&run, "html_url")))
    }
}

impl Forge for GhForge {
    fn main_head(&mut self) -> Result<String> {
        Ok(s(&self.get(&format!("commits/{}", self.branch))?, "sha"))
    }

    fn main_tree(&mut self) -> Result<String> {
        let c = self.get(&format!("commits/{}", self.branch))?;
        Ok(s(&c["commit"]["tree"], "sha"))
    }

    fn main_verifies(&mut self, main: &str) -> Result<MainBuild> {
        let runs = self.get(&format!(
            "actions/workflows/{}/runs?event=workflow_dispatch&status=completed&branch={}&per_page=30",
            self.workflow, self.branch
        ))?;
        let mut errors = 0;
        let mut failed = false;
        for run in runs["workflow_runs"].as_array().into_iter().flatten() {
            if pr_number(&s(run, "display_title")) != Some(0) || s(run, "head_sha") != main {
                continue;
            }
            match s(run, "conclusion").as_str() {
                "success" => return Ok(MainBuild::Passed),
                "failure" | "timed_out" | "startup_failure" => {
                    // Main's own content failed only if the step that says so
                    // failed; a run that ended elsewhere judged nothing.
                    let id = run["id"].as_u64().unwrap_or(0);
                    let jobs = self.get(&format!("actions/runs/{id}/jobs?per_page=100"))?;
                    let verdict = jobs["jobs"].as_array().into_iter().flatten().any(|j| {
                        j["steps"].as_array().into_iter().flatten().any(|st| {
                            s(st, "name") == MAIN_VERDICT_STEP && s(st, "conclusion") == "failure"
                        })
                    });
                    if verdict {
                        failed = true;
                    } else {
                        errors += 1;
                    }
                }
                _ => {}
            }
        }
        Ok(if failed {
            MainBuild::Failed
        } else if errors > 0 {
            MainBuild::Errored(errors)
        } else {
            MainBuild::Pending
        })
    }

    fn open_prs(&mut self) -> Result<Vec<PullRequest>> {
        let mut out = Vec::new();
        for item in self.list("pulls?state=open&per_page=100")? {
            let number = item["number"].as_u64().unwrap_or(0);
            // The list endpoint omits `mergeable`; ask for each one.
            let full = self.get(&format!("pulls/{number}")).unwrap_or(item);
            let labels: Vec<String> = full["labels"]
                .as_array()
                .map(|a| a.iter().map(|l| s(l, "name")).collect())
                .unwrap_or_default();
            self.labels.insert(number, labels.iter().cloned().collect());
            out.push(PullRequest {
                number,
                title: s(&full, "title"),
                author: s(&full["user"], "login"),
                body: s(&full, "body"),
                head_sha: s(&full["head"], "sha"),
                base_ref: s(&full["base"], "ref"),
                created_at: time(&full, "created_at"),
                url: s(&full, "html_url"),
                draft: full["draft"].as_bool().unwrap_or(false),
                mergeable: full["mergeable"].as_bool(),
                labels,
            });
        }
        Ok(out)
    }

    fn merged_prs(&mut self) -> Result<Vec<(PullRequest, String, DateTime<Utc>)>> {
        let recent = self.get("pulls?state=closed&sort=updated&direction=desc&per_page=40")?;
        let mut out = Vec::new();
        for p in recent.as_array().into_iter().flatten() {
            if p["merged_at"].is_null() {
                continue;
            }
            out.push((
                PullRequest {
                    number: p["number"].as_u64().unwrap_or(0),
                    title: s(p, "title"),
                    author: s(&p["user"], "login"),
                    body: s(p, "body"),
                    head_sha: s(&p["head"], "sha"),
                    base_ref: s(&p["base"], "ref"),
                    created_at: time(p, "created_at"),
                    url: s(p, "html_url"),
                    draft: false,
                    mergeable: None,
                    labels: Vec::new(),
                },
                s(p, "merge_commit_sha"),
                time(p, "merged_at"),
            ));
        }
        out.sort_by_key(|x| x.2);
        Ok(out)
    }

    fn verify_results(&mut self, pr: u64, head_sha: &str) -> Result<Vec<VerifyResult>> {
        let mut out = Vec::new();
        let name = format!("hillclimb-result-{pr}-{head_sha}");
        let listing = self.get(&format!("actions/artifacts?name={name}&per_page=30"))?;
        let mut artifacts: Vec<&Value> = listing["artifacts"]
            .as_array()
            .map(|a| a.iter().filter(|x| !x["expired"].as_bool().unwrap_or(false)).collect())
            .unwrap_or_default();
        artifacts.sort_by_key(|a| std::cmp::Reverse(s(a, "created_at")));
        for a in artifacts {
            let Some(run_id) = a["workflow_run"]["id"].as_u64() else { continue };
            // Anyone can upload an artifact with this name from their own
            // pull request workflow; only runs of the trusted workflow count.
            let Some(run_url) = self.trusted_run(run_id)? else { continue };
            let dir = std::env::temp_dir().join(format!("hc-artifact-{run_id}-{pr}"));
            let _ = std::fs::remove_dir_all(&dir);
            if let Err(e) = self.gh(&[
                "run", "download", &run_id.to_string(), "-R", &self.repo, "-n", &name, "-D",
                &dir.to_string_lossy(),
            ]) {
                eprintln!("#{pr}: could not download result of run {run_id}: {e:#}");
                continue;
            }
            let Ok(text) = std::fs::read_to_string(dir.join("result.json")) else { continue };
            let Ok(mut result) = serde_json::from_str::<VerifyResult>(&text) else { continue };
            if result.pr != pr || result.head_sha != head_sha {
                continue;
            }
            result.run_url = Some(run_url);
            out.push(result);
            if out.len() >= 6 {
                break;
            }
        }
        Ok(out)
    }

    fn active_verifies(&mut self) -> Result<BTreeSet<u64>> {
        let mut out = BTreeSet::new();
        for status in UNFINISHED {
            let runs = self.get(&format!(
                "actions/workflows/{}/runs?status={status}&per_page=100",
                self.workflow
            ))?;
            for run in runs["workflow_runs"].as_array().into_iter().flatten() {
                if let Some(n) = pr_number(&s(run, "display_title")) {
                    out.insert(n);
                }
            }
        }
        Ok(out)
    }

    fn current_verifies(&mut self) -> Result<BTreeSet<u64>> {
        // A run reads main when it starts, so one created before main's
        // latest commit is about an older main.
        let head = self.get(&format!("commits/{}", self.branch))?;
        let moved = time(&head["commit"]["committer"], "date");
        let mut out = BTreeSet::new();
        for status in UNFINISHED {
            let runs = self.get(&format!(
                "actions/workflows/{}/runs?status={status}&per_page=100",
                self.workflow
            ))?;
            for run in runs["workflow_runs"].as_array().into_iter().flatten() {
                if time(run, "created_at") >= moved {
                    if let Some(n) = pr_number(&s(run, "display_title")) {
                        out.insert(n);
                    }
                }
            }
        }
        Ok(out)
    }

    fn dispatch_verify(&mut self, pr: u64, stack: &[u64]) -> Result<()> {
        self.dispatch(pr, stack, None)
    }

    fn dispatch_verify_min(&mut self, pr: u64, stack: &[u64], min_delta: u64) -> Result<()> {
        self.dispatch(pr, stack, Some(min_delta))
    }

    fn changed_between(&mut self, base: &str, head: &str) -> Result<Vec<String>> {
        // Three dots: compared with the common ancestor, as a pull request is.
        // One page holds up to 300 files; a longer list is not trusted.
        let v = self.get(&format!("compare/{base}...{head}"))?;
        let files = v["files"].as_array().context("no file list in the comparison")?;
        if files.len() >= 300 {
            bail!("the comparison lists too many files to be complete");
        }
        let mut out = Vec::new();
        for f in files {
            out.push(s(f, "filename"));
            let previous = s(f, "previous_filename");
            if !previous.is_empty() {
                out.push(previous);
            }
        }
        Ok(out)
    }

    fn contains(&mut self, head: &str, ancestor: &str) -> Result<bool> {
        if head == ancestor {
            return Ok(true);
        }
        // `head` compared with `ancestor`: it contains it exactly when it is
        // not behind it by any commit.
        let v = self.get(&format!("compare/{ancestor}...{head}"))?;
        let behind = v["behind_by"].as_u64().context("no `behind_by` in the comparison")?;
        Ok(behind == 0)
    }

    fn changed_files(&mut self, pr: u64) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for f in self.list(&format!("pulls/{pr}/files?per_page=100"))? {
            out.push(s(&f, "filename"));
            // A renamed file is touched under its old name too.
            let previous = s(&f, "previous_filename");
            if !previous.is_empty() {
                out.push(previous);
            }
        }
        Ok(out)
    }

    fn cancel_verifies(&mut self, prs: &BTreeSet<u64>) -> Result<()> {
        for status in UNFINISHED {
            let runs = self.get(&format!(
                "actions/workflows/{}/runs?status={status}&per_page=100",
                self.workflow
            ))?;
            for run in runs["workflow_runs"].as_array().into_iter().flatten() {
                let Some(id) = run["id"].as_u64() else { continue };
                if pr_number(&s(run, "display_title")).is_some_and(|n| n != 0 && prs.contains(&n)) {
                    if let Err(e) = self.write("POST", &format!("actions/runs/{id}/cancel"), &[]) {
                        eprintln!("could not cancel run {id}: {e:#}");
                    }
                }
            }
        }
        Ok(())
    }

    fn reviews(&mut self, pr: u64) -> Result<Vec<ReviewInfo>> {
        Ok(self
            .list(&format!("pulls/{pr}/reviews?per_page=100"))?
            .iter()
            .map(|r| ReviewInfo {
                user: s(&r["user"], "login"),
                state: s(r, "state"),
                commit_id: s(r, "commit_id"),
                submitted_at: time(r, "submitted_at"),
            })
            .collect())
    }

    fn account_created(&mut self, login: &str) -> Result<DateTime<Utc>> {
        if let Some(t) = self.accounts.get(login) {
            return Ok(*t);
        }
        let text = self.gh(&["api", &format!("users/{login}")])?;
        let v: Value = serde_json::from_str(&text)?;
        // Unknown creation date counts as brand new.
        let t = v["created_at"]
            .as_str()
            .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
            .map(|t| t.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);
        self.accounts.insert(login.into(), t);
        Ok(t)
    }

    fn set_status(&mut self, sha: &str, state: CommitState, description: &str) -> Result<()> {
        let state = match state {
            CommitState::Success => "success",
            CommitState::Pending => "pending",
            CommitState::Failure => "failure",
        };
        // A commit accepts a limited number of statuses; only post changes.
        let current = self.get(&format!("commits/{sha}/status"))?;
        let unchanged = current["statuses"].as_array().into_iter().flatten().any(|st| {
            s(st, "context") == STATUS_CONTEXT && s(st, "state") == state && s(st, "description") == description
        });
        if !unchanged {
            self.write(
                "POST",
                &format!("statuses/{sha}"),
                &[("state", state), ("context", STATUS_CONTEXT), ("description", description)],
            )?;
        }
        Ok(())
    }

    fn upsert_comment(&mut self, pr: u64, body: &str) -> Result<()> {
        let existing = self.issue_comments(pr)?;
        let mine = existing
            .iter()
            .find(|c| s(c, "body").starts_with(COMMENT_MARKER) && s(&c["user"], "type") == "Bot");
        match mine {
            Some(c) if s(c, "body").trim() == body.trim() => {}
            Some(c) => {
                let id = c["id"].as_u64().unwrap_or(0);
                self.write("PATCH", &format!("issues/comments/{id}"), &[("body", body)])?;
            }
            None => {
                self.write("POST", &format!("issues/{pr}/comments"), &[("body", body)])?;
            }
        }
        Ok(())
    }

    fn set_label(&mut self, issue: u64, label: &str, present: bool) -> Result<()> {
        let has = self.labels.get(&issue).is_some_and(|l| l.contains(label));
        if has == present {
            return Ok(());
        }
        let result = if present {
            self.write("POST", &format!("issues/{issue}/labels"), &[("labels[]", label)])
        } else {
            self.write("DELETE", &format!("issues/{issue}/labels/{label}"), &[])
        };
        if let Err(e) = result {
            eprintln!("label {label} on #{issue}: {e:#}");
        }
        let set = self.labels.entry(issue).or_default();
        if present {
            set.insert(label.into());
        } else {
            set.remove(label);
        }
        Ok(())
    }

    fn merge(&mut self, pr: &PullRequest, message: &str) -> Result<String> {
        if self.dry_run {
            eprintln!("dry-run: merge #{}", pr.number);
            bail!("dry run");
        }
        let (title, body) = message.split_once("\n\n").unwrap_or((message, ""));
        let v = self.write(
            "PUT",
            &format!("pulls/{}/merge", pr.number),
            &[
                ("merge_method", "squash"),
                ("sha", &pr.head_sha),
                ("commit_title", title),
                ("commit_message", body),
            ],
        )?;
        let sha = s(&v, "sha");
        if sha.is_empty() {
            bail!("merge of #{} returned no commit", pr.number);
        }
        Ok(sha)
    }

    fn issues(&mut self) -> Result<Vec<IssueInfo>> {
        let mut out = Vec::new();
        for i in self.list("issues?state=all&per_page=100")? {
            if i.get("pull_request").is_some() {
                continue;
            }
            let number = i["number"].as_u64().unwrap_or(0);
            let labels: Vec<String> =
                i["labels"].as_array().map(|a| a.iter().map(|l| s(l, "name")).collect()).unwrap_or_default();
            self.labels.insert(number, labels.iter().cloned().collect());
            out.push(IssueInfo {
                number,
                title: s(&i, "title"),
                body: s(&i, "body"),
                author: s(&i["user"], "login"),
                open: s(&i, "state") == "open",
                labels,
                created_at: time(&i, "created_at"),
                url: s(&i, "html_url"),
            });
        }
        Ok(out)
    }

    fn pr_comments(&mut self, pr: u64) -> Result<Vec<CommentInfo>> {
        Ok(self
            .issue_comments(pr)?
            .iter()
            .filter(|c| s(&c["user"], "type") != "Bot")
            .map(|c| CommentInfo {
                issue: pr,
                user: s(&c["user"], "login"),
                body: s(c, "body"),
                created_at: time(c, "created_at"),
            })
            .collect())
    }

    fn create_issue(&mut self, title: &str, body: &str, label: &str) -> Result<(u64, String)> {
        if self.dry_run {
            eprintln!("dry-run: create issue {title}");
            return Ok((0, String::new()));
        }
        let v = self.write("POST", "issues", &[("title", title), ("body", body), ("labels[]", label)])?;
        Ok((v["number"].as_u64().unwrap_or(0), s(&v, "html_url")))
    }

    fn close_issue(&mut self, issue: u64) -> Result<()> {
        self.write("PATCH", &format!("issues/{issue}"), &[("state", "closed")])?;
        Ok(())
    }

    fn comments_since(&mut self, since: Option<DateTime<Utc>>) -> Result<Vec<CommentInfo>> {
        let since = since.unwrap_or_else(|| Utc::now() - Duration::days(1));
        let path = format!(
            "issues/comments?since={}&sort=created&direction=asc&per_page=100",
            since.format("%Y-%m-%dT%H:%M:%SZ")
        );
        Ok(self
            .list(&path)?
            .iter()
            .filter(|c| s(&c["user"], "type") != "Bot")
            .filter_map(|c| {
                let issue = s(c, "issue_url").rsplit('/').next()?.parse().ok()?;
                Some(CommentInfo {
                    issue,
                    user: s(&c["user"], "login"),
                    body: s(c, "body"),
                    created_at: time(c, "created_at"),
                })
            })
            .collect())
    }

    fn assign(&mut self, issue: u64, login: &str, assigned: bool) -> Result<()> {
        let method = if assigned { "POST" } else { "DELETE" };
        self.write(method, &format!("issues/{issue}/assignees"), &[("assignees[]", login)])?;
        Ok(())
    }

    fn comment(&mut self, issue: u64, body: &str) -> Result<()> {
        self.write("POST", &format!("issues/{issue}/comments"), &[("body", body)])?;
        Ok(())
    }
}

/// The first `#<number>` in a run title such as `verify #12`.
fn pr_number(text: &str) -> Option<u64> {
    let at = text.find('#')?;
    let digits: String = text[at + 1..].chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}
