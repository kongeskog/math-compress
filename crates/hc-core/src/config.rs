//! `hillclimb.toml`: everything project-specific lives here.

use anyhow::{Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub project: Project,
    #[serde(default)]
    pub score: Score,
    #[serde(default)]
    pub review: Review,
    #[serde(default)]
    pub ideas: Ideas,
    #[serde(default)]
    pub build: Build,
    #[serde(default)]
    pub deny: Vec<DenyRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub slug: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// `owner/name` of the arena repository.
    pub repo: String,
    /// `owner/name` of the harness repository (for install instructions).
    #[serde(default)]
    pub harness_repo: String,
    #[serde(default)]
    pub upstream: String,
    #[serde(default = "default_branch")]
    pub branch: String,
    /// Directory, relative to the repo root, that contributors may change.
    pub dir: String,
    /// Globs relative to `dir` of the files that are measured.
    #[serde(default = "default_counted")]
    pub counted: Vec<String>,
    /// Globs relative to the repo root that must be byte-identical to the base.
    #[serde(default)]
    pub frozen: Vec<String>,
    /// Shell commands a contributor runs inside `dir` to build locally.
    #[serde(default)]
    pub local_build: Vec<String>,
    /// The theorems that must stay proved (shown in prompts and rules).
    #[serde(default)]
    pub theorems: Vec<String>,
}

fn default_branch() -> String {
    "main".into()
}
fn default_counted() -> Vec<String> {
    vec!["**/*.lean".into()]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Score {
    /// Metric that a change must lower.
    pub primary: String,
    /// Minimum drop in the primary metric for a change to count.
    pub min_delta: u64,
    /// Guard metrics and how far (in percent) each may rise.
    pub guards: Vec<Guard>,
    /// Lines longer than this are counted; the count may not grow.
    pub max_line_length: usize,
    /// Build time may be at most this multiple of the base build time.
    pub max_build_ratio: f64,
}

impl Default for Score {
    fn default() -> Self {
        Score {
            primary: "tokens".into(),
            min_delta: 25,
            guards: vec![
                Guard { metric: "loc".into(), max_increase_pct: 0.5 },
                Guard { metric: "decls".into(), max_increase_pct: 0.5 },
                Guard { metric: "bytes".into(), max_increase_pct: 0.5 },
            ],
            max_line_length: 100,
            max_build_ratio: 1.3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Guard {
    pub metric: String,
    pub max_increase_pct: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Review {
    pub required: u32,
    /// Changes touching more files than this need `required_large` approvals.
    pub large_change_files: usize,
    pub required_large: u32,
    pub min_account_age_days: i64,
}

impl Default for Review {
    fn default() -> Self {
        Review { required: 1, large_change_files: 20, required_large: 2, min_account_age_days: 30 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ideas {
    pub label: String,
    pub lease_hours: i64,
}

impl Default for Ideas {
    fn default() -> Self {
        Ideas { label: "idea".into(), lease_hours: 3 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Build {
    /// File name of the verify workflow in the arena repository.
    pub workflow: String,
}

impl Default for Build {
    fn default() -> Self {
        Build { workflow: "hillclimb-verify.yml".into() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DenyMode {
    /// No occurrence allowed.
    Forbid,
    /// The number of occurrences may not exceed the base.
    NoIncrease,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DenyRule {
    pub name: String,
    /// Regex applied to comment-stripped source of every counted file.
    pub pattern: String,
    pub mode: DenyMode,
    #[serde(default)]
    pub description: String,
    /// If set, capture group 1 is parsed as a number and may not exceed this.
    #[serde(default)]
    pub max_value: Option<u64>,
}

impl Config {
    pub fn load(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        let cfg: Config =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<()> {
        for rule in &self.deny {
            regex::Regex::new(&rule.pattern)
                .with_context(|| format!("deny rule `{}` has an invalid pattern", rule.name))?;
        }
        self.counted_set()?;
        self.frozen_set()?;
        anyhow::ensure!(!self.project.dir.is_empty(), "project.dir must not be empty");
        Ok(())
    }

    pub fn counted_set(&self) -> Result<GlobSet> {
        globset(&self.project.counted)
    }

    pub fn frozen_set(&self) -> Result<GlobSet> {
        globset(&self.project.frozen)
    }

    /// `dir` with a trailing slash, for prefix checks.
    pub fn dir_prefix(&self) -> String {
        format!("{}/", self.project.dir.trim_end_matches('/'))
    }
}

fn globset(patterns: &[String]) -> Result<GlobSet> {
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        b.add(Glob::new(p).with_context(|| format!("invalid glob `{p}`"))?);
        // A bare directory name freezes everything under it.
        if !p.contains('*') {
            b.add(Glob::new(&format!("{}/**", p.trim_end_matches('/')))?);
        }
    }
    Ok(b.build()?)
}
