//! `ops.json`: the raw timings of the pipeline, for the dashboard's operations
//! page. Everything here is read from GitHub and from the attempts log; the
//! page derives lead times, queues and waste from it. Nothing is written to
//! GitHub.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration, NaiveTime, Utc};
use hc_core::config::Config;
use hc_core::model::{Attempt, State};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

/// How far back the page looks.
const WINDOW_DAYS: i64 = 3;

#[derive(Serialize)]
pub struct Ops {
    /// Start of the window: midnight UTC, so the file changes only when the data does.
    pub window_start: DateTime<Utc>,
    pub prs: Vec<OpsPr>,
    pub runs: Vec<OpsRun>,
    pub ideas: Vec<OpsIdea>,
}

#[derive(Serialize)]
pub struct OpsPr {
    pub pr: u64,
    pub title: String,
    /// `open`, `merged` or `closed`.
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    /// Change of the primary metric, for merged changes.
    pub delta: Option<i64>,
    pub idea: Option<u64>,
    pub verdicts: Vec<Verdict>,
}

#[derive(Serialize)]
pub struct Verdict {
    pub at: DateTime<Utc>,
    pub approve: bool,
}

#[derive(Serialize)]
pub struct OpsRun {
    pub id: u64,
    /// The pull request verified; 0 is a build of main.
    pub pr: u64,
    /// `own` (started by the pull request), `line` (dispatched by the gatekeeper) or `main`.
    pub kind: &'static str,
    pub created_at: DateTime<Utc>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    /// The conclusion of a finished run, otherwise its status.
    pub outcome: String,
}

#[derive(Serialize)]
pub struct OpsIdea {
    pub number: u64,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    pub claimed_at: Option<DateTime<Utc>>,
    /// When the first pull request for it was opened.
    pub pr_at: Option<DateTime<Utc>>,
}

fn list(repo: &str, path: &str) -> Result<Vec<Value>> {
    let out = Command::new("gh")
        .args(["api", "--paginate", "--slurp", &format!("repos/{repo}/{path}")])
        .output()
        .context("running gh")?;
    if !out.status.success() {
        bail!("gh api {path} failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let pages: Vec<Value> = serde_json::from_slice(&out.stdout)?;
    Ok(pages)
}

fn text(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn when(v: &Value, key: &str) -> Option<DateTime<Utc>> {
    v.get(key)
        .and_then(Value::as_str)
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .map(|t| t.with_timezone(&Utc))
}

/// The first line of a comment that is a command.
fn command(body: &str) -> Option<&str> {
    body.lines().map(str::trim).find(|l| l.starts_with('/')).and_then(|l| l.split_whitespace().next())
}

fn run_pr(title: &str) -> Option<u64> {
    let rest = &title[title.find('#')? + 1..];
    rest.chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok()
}

pub fn collect(cfg: &Config, attempts: &[Attempt], state: Option<&State>, now: DateTime<Utc>) -> Result<Ops> {
    let repo = &cfg.project.repo;
    let window_start = (now - Duration::days(WINDOW_DAYS)).date_naive().and_time(NaiveTime::MIN).and_utc();
    let since = window_start.format("%Y-%m-%dT%H:%M:%SZ").to_string();

    // Slash commands, by issue or pull request number.
    let mut verdicts: BTreeMap<u64, Vec<Verdict>> = BTreeMap::new();
    let mut claims: BTreeMap<u64, DateTime<Utc>> = BTreeMap::new();
    for page in list(repo, &format!("issues/comments?since={since}&sort=created&direction=asc&per_page=100"))? {
        for c in page.as_array().into_iter().flatten() {
            if text(&c["user"], "type") == "Bot" {
                continue;
            }
            let (Some(number), Some(at)) = (
                text(c, "issue_url").rsplit('/').next().and_then(|n| n.parse::<u64>().ok()),
                when(c, "created_at"),
            ) else {
                continue;
            };
            match command(&text(c, "body")) {
                Some("/approve") => verdicts.entry(number).or_default().push(Verdict { at, approve: true }),
                Some("/request-changes") => verdicts.entry(number).or_default().push(Verdict { at, approve: false }),
                Some("/claim") => {
                    claims.entry(number).or_insert(at);
                }
                _ => {}
            }
        }
    }

    // What the gatekeeper recorded about each pull request.
    let known: BTreeMap<u64, &Attempt> = state
        .iter()
        .flat_map(|s| s.open.iter())
        .chain(attempts.iter())
        .map(|a| (a.pr, a))
        .collect();
    let mut first_pr: BTreeMap<u64, DateTime<Utc>> = BTreeMap::new();

    let mut prs = Vec::new();
    let mut ideas = Vec::new();
    let label = &cfg.ideas.label;
    for page in list(repo, &format!("issues?state=all&since={since}&sort=created&direction=asc&per_page=100"))? {
        for i in page.as_array().into_iter().flatten() {
            let number = i["number"].as_u64().unwrap_or(0);
            let Some(created_at) = when(i, "created_at") else { continue };
            let closed_at = when(i, "closed_at");
            if let Some(p) = i.get("pull_request").filter(|p| !p.is_null()) {
                let a = known.get(&number);
                let idea = a.and_then(|a| a.idea);
                if let Some(n) = idea {
                    let e = first_pr.entry(n).or_insert(created_at);
                    *e = (*e).min(created_at);
                }
                let merged = when(p, "merged_at").is_some();
                prs.push(OpsPr {
                    pr: number,
                    title: text(i, "title"),
                    status: if merged { "merged" } else if closed_at.is_some() { "closed" } else { "open" }.into(),
                    created_at,
                    closed_at,
                    delta: a.filter(|_| merged).and_then(|a| a.delta),
                    idea,
                    verdicts: verdicts.remove(&number).unwrap_or_default(),
                });
            } else {
                let labelled = i["labels"].as_array().into_iter().flatten().any(|l| &text(l, "name") == label);
                let title = text(i, "title");
                if labelled || cfg.format.parse_idea_title(&title).is_some() {
                    ideas.push(OpsIdea {
                        number,
                        title,
                        created_at,
                        closed_at,
                        claimed_at: claims.get(&number).copied(),
                        pr_at: None,
                    });
                }
            }
        }
    }
    for i in &mut ideas {
        i.pr_at = first_pr.get(&i.number).copied();
    }

    let mut runs = Vec::new();
    let created = window_start.format("%Y-%m-%d");
    let path = format!("actions/workflows/{}/runs?created=%3E%3D{created}&per_page=100", cfg.build.workflow);
    for page in list(repo, &path)? {
        for r in page["workflow_runs"].as_array().into_iter().flatten() {
            let (Some(pr), Some(created_at)) = (run_pr(&text(r, "display_title")), when(r, "created_at")) else {
                continue;
            };
            let done = text(r, "status") == "completed";
            runs.push(OpsRun {
                id: r["id"].as_u64().unwrap_or(0),
                pr,
                kind: if pr == 0 {
                    "main"
                } else if text(r, "event") == "workflow_dispatch" {
                    "line"
                } else {
                    "own"
                },
                created_at,
                started_at: when(r, "run_started_at").unwrap_or(created_at),
                ended_at: when(r, "updated_at").filter(|_| done),
                outcome: if done { text(r, "conclusion") } else { text(r, "status") },
            });
        }
    }
    runs.sort_by_key(|r| (r.created_at, r.id));
    prs.sort_by_key(|p| p.pr);
    ideas.sort_by_key(|i| i.number);

    Ok(Ops { window_start, prs, runs, ideas })
}

pub fn write(root: &Path, data: &Path, now: DateTime<Utc>) -> Result<()> {
    let cfg = Config::load(&root.join("hillclimb.toml"))?;
    let attempts: Vec<Attempt> = std::fs::read_to_string(data.join("attempts.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let state: Option<State> =
        std::fs::read_to_string(data.join("state.json")).ok().and_then(|t| serde_json::from_str(&t).ok());
    let ops = collect(&cfg, &attempts, state.as_ref(), now)?;
    println!(
        "operations data: {} pull requests, {} verification runs, {} ideas since {}",
        ops.prs.len(),
        ops.runs.len(),
        ops.ideas.len(),
        ops.window_start.format("%Y-%m-%d")
    );
    std::fs::write(data.join("ops.json"), serde_json::to_string(&ops)? + "\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commands_and_run_titles() {
        assert_eq!(command("Looks right.\n\n/approve abc1234"), Some("/approve"));
        assert_eq!(command("no command here"), None);
        assert_eq!(command("  /request-changes"), Some("/request-changes"));
        assert_eq!(run_pr("verify #42"), Some(42));
        assert_eq!(run_pr("verify #0"), Some(0));
        assert_eq!(run_pr("something else"), None);
    }
}
