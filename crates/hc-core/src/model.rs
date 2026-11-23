//! Data shared between the CLI, the workflows and the dashboard.
//! The JSON shapes here are the dashboard's contract; change them with care.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Metrics {
    pub tokens: u64,
    pub loc: u64,
    pub decls: u64,
    pub files: u64,
    pub bytes: u64,
    #[serde(default)]
    pub build_seconds: Option<u64>,
}

impl Metrics {
    pub fn get(&self, key: &str) -> Option<u64> {
        match key {
            "tokens" => Some(self.tokens),
            "loc" => Some(self.loc),
            "decls" => Some(self.decls),
            "files" => Some(self.files),
            "bytes" => Some(self.bytes),
            "build_seconds" => self.build_seconds,
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricDef {
    pub key: String,
    pub label: String,
    pub unit: String,
    pub description: String,
}

pub fn metric_defs() -> Vec<MetricDef> {
    let d = |key: &str, label: &str, unit: &str, description: &str| MetricDef {
        key: key.into(),
        label: label.into(),
        unit: unit.into(),
        description: description.into(),
    };
    vec![
        d("tokens", "Tokens", "tokens", "Source tokens after removing comments and whitespace. Reformatting cannot change it."),
        d("loc", "Lines of code", "lines", "Lines that contain at least one token."),
        d("decls", "Declarations", "decls", "Theorems, lemmas, definitions, structures, instances and similar."),
        d("files", "Files", "files", "Measured source files."),
        d("bytes", "Bytes", "bytes", "Size of the measured source files."),
        d("build_seconds", "Build time", "s", "Wall-clock time of the verification build. Informational."),
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateStatus {
    Pass,
    Fail,
    Pending,
    Skip,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Gate {
    pub key: String,
    pub label: String,
    pub status: GateStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl Gate {
    pub fn new(key: &str, status: GateStatus, detail: impl Into<String>) -> Gate {
        let detail = detail.into();
        Gate {
            key: key.into(),
            label: gate_label(key).into(),
            status,
            detail: if detail.is_empty() { None } else { Some(detail) },
        }
    }
}

/// Every gate, in display order, with its label and what it protects.
pub const GATES: &[(&str, &str, &str)] = &[
    ("frozen", "Frozen files untouched", "The statement of the theorems, the toolchain, the dependency pins and the harness configuration must be byte-identical to the main branch."),
    ("scope", "Changes stay in scope", "Only files inside the project directory may change."),
    ("deny", "No banned constructs", "No new axioms, no unchecked native evaluation, no escape hatches, no private macro languages, no raised resource limits."),
    ("improves", "Score improves", "The primary metric must drop by at least the minimum step and no guard metric may rise beyond its tolerance."),
    ("compiles", "Project compiles", "The full project builds on the pinned toolchain with no unfinished proofs."),
    ("same_theorems", "Same theorems proved", "An independent checker with a second proof kernel confirms the original theorem statements are still proved from the standard axioms only."),
    ("fresh", "Verified on current main", "The verification ran on top of the latest main branch."),
    ("review", "Independent review", "Approved by someone other than the author."),
];

pub fn gate_label(key: &str) -> &'static str {
    GATES.iter().find(|g| g.0 == key).map(|g| g.1).unwrap_or("Gate")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileStat {
    pub path: String,
    pub tokens: u64,
    pub loc: u64,
}

/// Output of the verify workflow for one pull request head.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyResult {
    pub pr: u64,
    pub head_sha: String,
    pub base_sha: String,
    pub metrics: Metrics,
    pub base_metrics: Metrics,
    pub gates: Vec<Gate>,
    pub changed_files: usize,
    #[serde(default)]
    pub top_files: Vec<FileStat>,
    #[serde(default)]
    pub run_url: Option<String>,
}

impl VerifyResult {
    pub fn gate(&self, key: &str) -> Option<&Gate> {
        self.gates.iter().find(|g| g.key == key)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptStatus {
    Verifying,
    AwaitingReview,
    Stale,
    Conflict,
    Merged,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attempt {
    pub pr: u64,
    pub title: String,
    pub author: String,
    pub head_sha: String,
    #[serde(default)]
    pub base_sha: Option<String>,
    pub status: AttemptStatus,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub decided_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub metrics: Option<Metrics>,
    #[serde(default)]
    pub base_metrics: Option<Metrics>,
    /// Change in the primary metric; negative is an improvement.
    #[serde(default)]
    pub delta: Option<i64>,
    pub gates: Vec<Gate>,
    #[serde(default)]
    pub idea: Option<u64>,
    pub url: String,
    #[serde(default)]
    pub run_url: Option<String>,
    #[serde(default)]
    pub merge_sha: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdeaStatus {
    Open,
    Claimed,
    Done,
    Closed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Idea {
    pub number: u64,
    pub title: String,
    pub status: IdeaStatus,
    pub author: String,
    #[serde(default)]
    pub claimed_by: Option<String>,
    #[serde(default)]
    pub lease_expires: Option<DateTime<Utc>>,
    #[serde(default)]
    pub pr: Option<u64>,
    #[serde(default)]
    pub result_delta: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Point {
    pub sha: String,
    pub at: DateTime<Utc>,
    pub metrics: Metrics,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub slug: String,
    pub title: String,
    pub description: String,
    pub repo: String,
    pub dir: String,
    pub upstream: String,
    pub primary: String,
    pub metrics: Vec<MetricDef>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Counts {
    pub merged: u64,
    pub rejected: u64,
    pub open: u64,
    pub contributors: u64,
    pub ideas_open: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Leader {
    pub login: String,
    pub merged: u64,
    pub attempts: u64,
    pub saved: u64,
}

/// `state.json` on the data branch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct State {
    pub project: ProjectInfo,
    pub updated_at: DateTime<Utc>,
    pub baseline: Point,
    pub current: Point,
    pub counts: Counts,
    pub leaderboard: Vec<Leader>,
    pub ideas: Vec<Idea>,
    pub open: Vec<Attempt>,
    pub top_files: Vec<FileStat>,
}

/// `ledger.json` on the data branch: gatekeeper bookkeeping the dashboard
/// does not need.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Ledger {
    /// Idea number -> claim.
    #[serde(default)]
    pub claims: BTreeMap<u64, Claim>,
    /// Last issue-comment timestamp that was processed.
    #[serde(default)]
    pub comments_seen_until: Option<DateTime<Utc>>,
    /// `pr@head_sha` keys for which a verify run was dispatched.
    #[serde(default)]
    pub dispatched: BTreeMap<String, DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claim {
    pub login: String,
    pub at: DateTime<Utc>,
}
