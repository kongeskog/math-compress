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

/// What a failed gate means for the change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnFail {
    /// The author can repair it with a push or an edit. The change stays
    /// open as "needs a fix" and nothing is written to the attempts log.
    Fix,
    /// A verdict on the commit: the attempt is recorded as rejected.
    Reject,
    /// Not a failure of the change: it waits (for main, or for a reviewer).
    Wait,
}

/// Every gate, in display order, with its label, what it protects, and what
/// its failure means. This table is the one place that classifies gates.
pub const GATES: &[(&str, &str, &str, OnFail)] = &[
    ("format", "Standard title and description", "The title follows `technique(scope): summary` and the description names the idea it implements (and, if it is built on another open change, that change), so every change can be identified and grouped.", OnFail::Fix),
    ("quota", "Within the open-change limit", "One author may have only a limited number of changes in verification at once, so the build queue stays available to everyone.", OnFail::Fix),
    ("frozen", "Frozen files untouched", "The statement of the theorems, the toolchain, the dependency pins and the harness configuration must be byte-identical to the main branch.", OnFail::Reject),
    ("scope", "Changes stay in scope", "Only files inside the project directory may change.", OnFail::Reject),
    ("deny", "No banned constructs", "No new axioms, no unchecked native evaluation, no escape hatches, no code that runs during the build, and no protected declaration removed.", OnFail::Reject),
    ("no_increase", "No more long lines or exceptions", "The number of lines over the length limit, and of constructs the project tolerates but wants no more of (private macro languages, raised resource limits), may not grow.", OnFail::Fix),
    ("improves", "Score improves", "The primary metric must drop by at least the minimum step and no guard metric may rise beyond its tolerance.", OnFail::Fix),
    ("compiles", "Project compiles", "The full project builds on the pinned toolchain with no unfinished proofs.", OnFail::Reject),
    ("same_theorems", "Same theorems proved", "An independent checker with a second proof kernel confirms the original theorem statements are still proved from the standard axioms only.", OnFail::Reject),
    ("fresh", "Verified on current main", "The verification ran on top of the latest main branch.", OnFail::Wait),
    ("review", "Independent review", "Approved by a vetted reviewer other than the author.", OnFail::Wait),
];

pub fn gate_label(key: &str) -> &'static str {
    GATES.iter().find(|g| g.0 == key).map(|g| g.1).unwrap_or("Gate")
}

/// What a failure of this gate means. A gate this table does not know is a
/// verdict: nothing unknown is ever treated as a small matter.
pub fn gate_on_fail(key: &str) -> OnFail {
    GATES.iter().find(|g| g.0 == key).map(|g| g.3).unwrap_or(OnFail::Reject)
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
    /// Commit of the main branch the verification started from.
    pub base_sha: String,
    /// Git tree the change was applied to: main, plus the changes queued ahead
    /// of this one (`stack`). The result is valid for exactly this tree.
    #[serde(default)]
    pub base_tree: Option<String>,
    /// Git tree after applying the change; what main must look like once merged.
    #[serde(default)]
    pub merged_tree: Option<String>,
    /// `pr@head_sha` of the changes that were applied before this one.
    #[serde(default)]
    pub stack: Vec<String>,
    /// Git tree main will have if the changes in `stack` land one after
    /// another as squashed commits and this change is then merged, or
    /// `conflict` if that sequence does not merge cleanly. It can differ
    /// from `merged_tree` only when this change's branch contains commits of
    /// a change in the stack. A stacked result may be merged from only when
    /// the two are equal.
    #[serde(default)]
    pub landing_tree: Option<String>,
    /// The minimum drop the `improves` gate was judged with. Every result
    /// written since stacked changes were introduced records it, so its
    /// presence also marks a result in the current format.
    #[serde(default)]
    pub min_delta: Option<u64>,
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
    /// A gate failed that the author can fix with a push or an edit. The
    /// change stays open; this is never written to the attempts log.
    NeedsFix,
    Merged,
    Rejected,
}

/// A leader's waiver of the minimum step for one commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Waiver {
    pub by: String,
    pub reason: String,
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
    #[serde(default)]
    pub technique: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    /// Open pull requests this change is built on (`Depends: #n`), the
    /// first to merge first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends: Vec<u64>,
    /// `pr@head_sha` of those changes as this commit is judged on top of
    /// them: the commits the bot's comment names, and approvals are given under.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub judged_on: Vec<String>,
    /// `pr@head_sha` of the changes that were applied before this one in
    /// the verification the metrics and gates shown come from. Empty when
    /// they were measured against main alone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub measured_on: Vec<String>,
    /// Notes that never fail a gate, such as files reserved by an idea.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// Set when a leader waived the minimum step for this commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiver: Option<Waiver>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdeaStatus {
    /// Filed by someone who is not yet a vetted ideator; waiting for triage.
    Proposed,
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
    #[serde(default)]
    pub technique: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
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
    /// `pr@head_sha` -> the line (`n@head_sha` of each change ahead) its
    /// latest queued verification was stacked on.
    #[serde(default)]
    pub lines: BTreeMap<String, Vec<String>>,
    /// `pr@head_sha` -> the commit of main that verification was dispatched on.
    #[serde(default)]
    pub line_bases: BTreeMap<String, String>,
    /// Alerts already raised, so each is raised once.
    #[serde(default)]
    pub alerts: std::collections::BTreeSet<String>,
    /// Issues that were already told their title or body is malformed.
    #[serde(default)]
    pub format_notified: std::collections::BTreeSet<u64>,
    /// `pr@head_sha` -> when the gatekeeper first judged that commit and what
    /// it judged it on top of. Only approvals given after that moment count,
    /// so every counted approval was given under a bot comment whose
    /// statement of what the commit is built on cannot change. A record is
    /// kept while the pull request has not merged, also while it is a draft
    /// or closed; a commit without a record is judged for the first time.
    #[serde(default)]
    pub judged: BTreeMap<String, Judged>,
    /// Notices already given on pull requests, so each is given once.
    #[serde(default)]
    pub told: std::collections::BTreeSet<String>,
}

/// How many records of judged commits the ledger keeps. Records of open pull
/// requests are always kept; beyond this number the oldest others go. Losing
/// one is safe: the commit is then judged anew and needs a new approval.
pub const JUDGED_KEPT: usize = 2000;

/// The gatekeeper's first judgement of one commit of a pull request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Judged {
    /// The forge's time of the bot comment that first made the statement
    /// below for this commit. Approvals submitted up to that moment do not
    /// count. `None` until that comment has been made: in the pass that
    /// makes or changes the record no approval counts.
    #[serde(default)]
    pub at: Option<DateTime<Utc>>,
    /// The pull request the commit's `Depends` line named when the statement
    /// was made, if it had one.
    #[serde(default)]
    pub names: Option<u64>,
    /// `pr@head_sha` of the open changes the commit was judged on top of (its
    /// `Depends` chain at their heads then), the first to merge first. Empty
    /// for a change on main.
    #[serde(default)]
    pub on: Vec<String>,
    /// Whether the commit contains the head of the change it is built on
    /// directly (the last of `on`). `None` until that could be checked.
    #[serde(default)]
    pub contains: Option<bool>,
    /// For a commit whose `Depends` line names a pull request that has
    /// merged: whether the commit contains the commit that merged. `None`
    /// until that could be checked, or when there is no such line.
    #[serde(default)]
    pub has_merged: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Leader,
    Reviewer,
    Ideator,
}

impl Role {
    pub fn parse(s: &str) -> Option<Role> {
        match s.trim().to_lowercase().as_str() {
            "leader" => Some(Role::Leader),
            "reviewer" => Some(Role::Reviewer),
            "ideator" => Some(Role::Ideator),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Role::Leader => "leader",
            Role::Reviewer => "reviewer",
            Role::Ideator => "ideator",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Member {
    pub login: String,
    pub role: Role,
    pub since: DateTime<Utc>,
    /// Leaders who endorsed or granted the role; empty for founding leaders.
    pub by: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplicationStatus {
    Open,
    Granted,
    Declined,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Application {
    pub issue: u64,
    pub login: String,
    pub role: Role,
    pub opened_at: DateTime<Utc>,
    pub status: ApplicationStatus,
    pub endorsements: Vec<String>,
    pub needed: u32,
    /// Experience at the time of applying.
    pub merged: u64,
    pub saved: u64,
    pub url: String,
}

/// `roles.json` on the data branch: who holds which role, and open applications.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RolesState {
    #[serde(default)]
    pub bootstrap: bool,
    #[serde(default)]
    pub members: Vec<Member>,
    #[serde(default)]
    pub applications: Vec<Application>,
}

impl RolesState {
    pub fn has(&self, login: &str, role: Role) -> bool {
        self.members.iter().any(|m| m.role == role && m.login.eq_ignore_ascii_case(login))
    }
    pub fn is_leader(&self, login: &str) -> bool {
        self.has(login, Role::Leader)
    }
    pub fn can_review(&self, login: &str) -> bool {
        self.is_leader(login) || self.has(login, Role::Reviewer)
    }
    pub fn can_triage(&self, login: &str) -> bool {
        self.is_leader(login) || self.has(login, Role::Ideator)
    }
    pub fn leaders(&self) -> usize {
        self.members.iter().filter(|m| m.role == Role::Leader).count()
    }
    pub fn grant(&mut self, login: &str, role: Role, by: Vec<String>, at: DateTime<Utc>) {
        if !self.has(login, role) {
            self.members.push(Member { login: login.to_string(), role, since: at, by });
        }
    }
    pub fn revoke(&mut self, login: &str, role: Role) -> bool {
        let before = self.members.len();
        self.members.retain(|m| !(m.role == role && m.login.eq_ignore_ascii_case(login)));
        self.members.len() != before
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claim {
    pub login: String,
    pub at: DateTime<Utc>,
}
