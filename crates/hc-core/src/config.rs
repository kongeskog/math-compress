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
    #[serde(default)]
    pub protect: Vec<ProtectRule>,
    #[serde(default)]
    pub roles: Roles,
    #[serde(default)]
    pub format: Format,
    #[serde(default)]
    pub limits: Limits,
    #[serde(default)]
    pub conduct: Conduct,
}

/// Rules a reviewer checks, stated in the prompts. No gate enforces them.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Conduct {
    /// A changed file may take at most this many percent longer to compile
    /// than on the base branch. 0 states no limit.
    pub max_slowdown_pct: u32,
    pub search_tactics: SearchTactics,
}

impl Default for Conduct {
    fn default() -> Self {
        Conduct { max_slowdown_pct: 10, search_tactics: SearchTactics::default() }
    }
}

/// Whether tactics that search for a proof may replace the end of a tactic
/// block to save tokens, and under which conditions. Off unless an arena
/// turns it on: it lowers the score by making proofs less explicit.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchTactics {
    pub enabled: bool,
    pub tactics: Vec<String>,
    /// At most this many such calls in one file.
    pub max_per_file: u32,
    /// Each place must save at least this many tokens.
    pub min_saving: u64,
    /// Files changed this way may take at most this many percent longer.
    pub max_slowdown_pct: u32,
}

impl Default for SearchTactics {
    fn default() -> Self {
        SearchTactics {
            enabled: false,
            tactics: vec!["grind".into(), "simp_all".into()],
            max_per_file: 10,
            min_saving: 25,
            max_slowdown_pct: 20,
        }
    }
}

impl Conduct {
    fn tactic_list(&self) -> String {
        self.search_tactics.tactics.iter().map(|t| format!("`{t}`")).collect::<Vec<_>>().join(", ")
    }

    /// The time limit, as a sentence for contributors. Empty if there is none.
    pub fn time_rule(&self, branch: &str) -> String {
        if self.max_slowdown_pct == 0 {
            return String::new();
        }
        format!(
            "A file you changed may take at most {}% longer to compile than on `{branch}`. Where a proof changed in substance, run `hc time --base <a built checkout of {branch}>` and paste its table into the description.",
            self.max_slowdown_pct
        )
    }

    /// The rule on search tactics, as a sentence for contributors.
    pub fn search_rule(&self) -> String {
        let st = &self.search_tactics;
        if st.tactics.is_empty() {
            return String::new();
        }
        if !st.enabled {
            return format!(
                "Do not replace explicit steps by a tactic that searches for the proof ({}) to save tokens. Such a call already in the proof may stay.",
                self.tactic_list()
            );
        }
        format!(
            "A tactic that searches for the proof ({}) may replace the end of a tactic block only if all of these hold: there are at most {} such calls in the file; the place saves at least {} tokens; the file takes at most {}% longer to compile; no specific tactic closes the goal at similar length; and a one-line comment at the place says what the removed lines used (where they were a chain of inequalities, the comment gives the intermediate bound and where the final constant comes from).",
            self.tactic_list(),
            st.max_per_file,
            st.min_saving,
            st.max_slowdown_pct
        )
    }

    /// Both rules as extra bullets for the reviewer's list of reasons to
    /// request changes. Starts with a newline when not empty.
    pub fn reviewer_bullets(&self) -> String {
        let st = &self.search_tactics;
        let mut out = String::new();
        if self.max_slowdown_pct > 0 {
            out.push_str(&format!(
                "\n- Slower: a changed file takes more than {}% longer to compile than before, or a proof changed in substance and the description gives no timings.",
                self.max_slowdown_pct
            ));
        }
        if !st.tactics.is_empty() {
            if st.enabled {
                out.push_str(&format!(
                    "\n- A search tactic ({}) outside its conditions: more than {} such calls in a file, a place that saves fewer than {} tokens, a file more than {}% slower, a specific tactic that would do, or a missing or inaccurate comment. Check each comment against the lines it replaced: it must say what they used, with names that are in scope there.",
                    self.tactic_list(),
                    st.max_per_file,
                    st.min_saving,
                    st.max_slowdown_pct
                ));
            } else {
                out.push_str(&format!(
                    "\n- Explicit steps replaced by a tactic that searches for the proof ({}).",
                    self.tactic_list()
                ));
            }
        }
        out
    }
}

/// Declarations that may be rewritten but never removed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtectRule {
    pub name: String,
    /// Regex over comment-stripped source; capture group 1 is the declaration
    /// name. Every name found in the base must still be found in the change.
    pub pattern: String,
    #[serde(default)]
    pub description: String,
}

/// Who may do what. Anyone may contribute changes; reviewing and adding to
/// the ideas queue directly are earned.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Roles {
    /// The founding leaders. Further leaders are added through applications.
    pub leaders: Vec<String>,
    /// While true, a leader's review counts even on a change opened from the
    /// leader's own account (for the phase in which the leaders run every
    /// agent themselves). Turn off before opening the project up.
    pub bootstrap: bool,
    /// Merged changes needed before applying to review.
    pub reviewer_min_merged: u64,
    /// Merged changes needed before applying to file ideas directly.
    pub ideator_min_merged: u64,
    /// Merged changes needed before applying to lead.
    pub leader_min_merged: u64,
    /// Leader endorsements an application needs.
    pub endorsements: u32,
    /// Leader endorsements an application to lead needs (capped at the number of leaders).
    pub leader_endorsements: u32,
    /// Accounts younger than this cannot apply.
    pub min_account_age_days: i64,
}

impl Default for Roles {
    fn default() -> Self {
        Roles {
            leaders: Vec::new(),
            bootstrap: false,
            reviewer_min_merged: 2,
            ideator_min_merged: 1,
            leader_min_merged: 5,
            endorsements: 1,
            leader_endorsements: 2,
            min_account_age_days: 30,
        }
    }
}

/// Standard titles, so changes and ideas can be identified and grouped.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Format {
    /// Allowed techniques, the first word of every title.
    pub techniques: Vec<Technique>,
    pub summary_min: usize,
    pub summary_max: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Technique {
    pub key: String,
    pub description: String,
}

impl Default for Format {
    fn default() -> Self {
        let t = |key: &str, description: &str| Technique { key: key.into(), description: description.into() };
        Format {
            techniques: vec![
                t("dead-code", "Remove declarations nothing uses."),
                t("library", "Use a lemma or definition from the library instead of a local one."),
                t("merge", "Merge duplicate or near-duplicate declarations."),
                t("generalize", "Replace several special cases with one general statement."),
                t("shorten", "A shorter proof of the same statement."),
                t("restructure", "Reorganize definitions or the argument so that less is needed."),
            ],
            summary_min: 10,
            summary_max: 90,
        }
    }
}

impl Format {
    fn title_regex(&self, prefix: &str) -> regex::Regex {
        let keys: Vec<String> = self.techniques.iter().map(|t| regex::escape(&t.key)).collect();
        regex::Regex::new(&format!(
            r"^{prefix}({})\(([A-Za-z0-9_./*,-]+)\): (\S.{{{},{}}})$",
            keys.join("|"),
            self.summary_min.saturating_sub(1),
            self.summary_max.saturating_sub(1)
        ))
        .expect("title regex")
    }

    /// `technique(scope): summary`
    pub fn parse_title(&self, title: &str) -> Option<(String, String, String)> {
        let c = self.title_regex("").captures(title.trim())?;
        Some((c[1].to_string(), c[2].to_string(), c[3].to_string()))
    }

    /// `Idea: technique(scope): summary`
    pub fn parse_idea_title(&self, title: &str) -> Option<(String, String, String)> {
        let c = self.title_regex("Idea: ").captures(title.trim())?;
        Some((c[1].to_string(), c[2].to_string(), c[3].to_string()))
    }

    pub fn title_help(&self) -> String {
        format!(
            "`<technique>(<scope>): <summary>` where technique is one of {}; scope is the file or folder under the project directory without `.lean` (for example `Paper/Tree`), or `*` for many; and the summary is {} to {} characters",
            self.techniques.iter().map(|t| format!("`{}`", t.key)).collect::<Vec<_>>().join(", "),
            self.summary_min,
            self.summary_max
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Limits {
    /// Open pull requests one author may have being verified at once. Leaders are exempt.
    pub max_open_prs: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_open_prs: 4 }
    }
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
#[serde(default)]
pub struct Review {
    pub required: u32,
    /// Changes touching more files than this need `required_large` approvals.
    pub large_change_files: usize,
    pub required_large: u32,
}

impl Default for Review {
    fn default() -> Self {
        Review { required: 1, large_change_files: 20, required_large: 2 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ideas {
    pub label: String,
    pub lease_hours: i64,
    /// An idea whose estimated saving is above this many tokens needs a
    /// Trial section before it enters the queue. 0 turns the rule off.
    #[serde(default = "default_trial_above")]
    pub trial_above: u64,
}

fn default_trial_above() -> u64 {
    1000
}

impl Default for Ideas {
    fn default() -> Self {
        Ideas { label: "idea".into(), lease_hours: 3, trial_above: default_trial_above() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Build {
    /// File name of the verify workflow in the arena repository.
    pub workflow: String,
    /// File name of the workflow that deploys the dashboard.
    #[serde(default = "default_pages")]
    pub pages_workflow: String,
}

fn default_pages() -> String {
    "hillclimb-pages.yml".into()
}

impl Default for Build {
    fn default() -> Self {
        Build { workflow: "hillclimb-verify.yml".into(), pages_workflow: default_pages() }
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
        for rule in &self.protect {
            regex::Regex::new(&rule.pattern)
                .with_context(|| format!("protect rule `{}` has an invalid pattern", rule.name))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles() {
        let f = Format::default();
        assert_eq!(
            f.parse_title("dead-code(Paper/Tree): remove unreferenced lemma cutUnits_id_cur"),
            Some(("dead-code".into(), "Paper/Tree".into(), "remove unreferenced lemma cutUnits_id_cur".into()))
        );
        assert!(f.parse_title("shorten(*): use omega in the arithmetic side goals").is_some());
        assert!(f.parse_title("Remove a lemma").is_none());
        assert!(f.parse_title("golfing(Paper/Tree): something reasonable here").is_none());
        assert!(f.parse_title("merge(Paper/Tree): tiny").is_none());
        assert!(f.parse_idea_title("Idea: merge(Nc/Tow): one statement for the Ever lemmas").is_some());
        assert!(f.parse_idea_title("merge(Nc/Tow): one statement for the Ever lemmas").is_none());
    }

    #[test]
    fn conduct_rules() {
        let mut c = Conduct::default();
        assert!(c.time_rule("main").contains("10%"));
        assert!(c.search_rule().starts_with("Do not replace"));
        assert!(c.reviewer_bullets().contains("Explicit steps replaced"));
        c.search_tactics.enabled = true;
        assert!(c.search_rule().contains("at most 10 such calls"));
        assert!(c.reviewer_bullets().contains("outside its conditions"));
        c.max_slowdown_pct = 0;
        c.search_tactics.tactics.clear();
        assert!(c.time_rule("main").is_empty());
        assert!(c.search_rule().is_empty());
        assert!(c.reviewer_bullets().is_empty());
        let parsed: Conduct = toml::from_str("[search_tactics]\nenabled = true\nmin_saving = 40").unwrap();
        assert!(parsed.search_tactics.enabled);
        assert_eq!(parsed.search_tactics.min_saving, 40);
        assert_eq!(parsed.max_slowdown_pct, 10);
    }
}
