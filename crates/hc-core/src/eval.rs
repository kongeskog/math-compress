//! Static evaluation: measure a checkout and compare it with its base.
//! Nothing here executes project code; it only reads files.

use crate::checks;
use crate::config::{Config, DenyMode};
use crate::lexer;
use crate::model::{FileStat, Gate, GateStatus, Metrics};
use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use walkdir::WalkDir;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DenyCount {
    pub count: u64,
    pub max_value: u64,
    /// `path:line` of the first few matches.
    pub samples: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Measurement {
    pub metrics: Metrics,
    pub files: Vec<FileStat>,
    pub deny: BTreeMap<String, DenyCount>,
    pub long_lines: u64,
    pub long_line_samples: Vec<String>,
    /// Protect rule name -> declaration name -> its statement (everything
    /// before the proof), whitespace-normalized.
    #[serde(default)]
    pub protected: BTreeMap<String, BTreeMap<String, String>>,
    /// Repo-relative path -> content hash, for every file in the checkout.
    #[serde(skip)]
    pub tree: BTreeMap<String, String>,
    /// Repo-relative path -> text, for every measured file. The gates that
    /// read what a change adds and removes compare these.
    #[serde(skip)]
    pub texts: BTreeMap<String, String>,
    /// Repo-relative path -> the full names of the declarations in the file.
    #[serde(skip)]
    pub names: BTreeMap<String, Vec<String>>,
    /// The full names of the declarations in the project's frozen source
    /// files (the statement of the theorems). They are not measured, but a
    /// name they declare is a name that exists.
    #[serde(skip)]
    pub frozen_names: Vec<String>,
}

fn decl_regex() -> Regex {
    Regex::new(
        r"(?m)^\s*(?:@\[[^\]]*\]\s*)*(?:(?:private|protected|public|noncomputable|unsafe|partial|nonrec|scoped|local)\s+)*(?:theorem|lemma|def|abbrev|instance|structure|inductive|class|example|opaque|axiom)\b",
    )
    .expect("static regex")
}

/// The statement of the declaration starting at the beginning of `code`:
/// everything before its proof (the first `:=`, or `where`, or a pattern-match
/// bar at the start of a line), whitespace-normalized.
fn statement(code: &str) -> String {
    let mut words = Vec::new();
    for (i, line) in code.lines().enumerate() {
        if i > 0 && (line.trim_start().starts_with('|')) {
            break;
        }
        let mut done = false;
        for w in line.split_whitespace() {
            if w == ":=" || w == "where" {
                done = true;
                break;
            }
            words.push(w);
        }
        if done || words.len() > 4000 {
            break;
        }
    }
    words.join(" ")
}

/// Hash every file of a checkout. `.git` and Lake build directories are skipped.
pub fn tree(root: &Path) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    let walker = WalkDir::new(root).follow_links(false).into_iter().filter_entry(|e| {
        let name = e.file_name().to_string_lossy();
        !(e.depth() > 0 && (name == ".git" || name == ".lake"))
    });
    for entry in walker {
        let entry = entry?;
        let ft = entry.file_type();
        if ft.is_dir() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(root)?
            .to_string_lossy()
            .replace('\\', "/");
        let hash = if ft.is_symlink() {
            let target = std::fs::read_link(entry.path())?;
            format!("symlink:{}", target.to_string_lossy())
        } else {
            let bytes = std::fs::read(entry.path())
                .with_context(|| format!("reading {}", entry.path().display()))?;
            format!("{:x}", Sha256::digest(&bytes))
        };
        out.insert(rel, hash);
    }
    Ok(out)
}

/// Measure one checkout.
pub fn measure(root: &Path, cfg: &Config) -> Result<Measurement> {
    let tree = tree(root)?;
    let counted = cfg.counted_set()?;
    let frozen = cfg.frozen_set()?;
    let prefix = cfg.dir_prefix();
    let decl_re = decl_regex();
    let rules: Vec<(usize, Regex)> = cfg
        .deny
        .iter()
        .enumerate()
        .map(|(i, r)| Ok((i, Regex::new(&r.pattern)?)))
        .collect::<Result<_>>()?;

    let protect: Vec<(String, Regex)> = cfg
        .protect
        .iter()
        .map(|r| Ok((r.name.clone(), Regex::new(&r.pattern)?)))
        .collect::<Result<_>>()?;

    let mut m = Measurement::default();
    for rule in &cfg.deny {
        m.deny.insert(rule.name.clone(), DenyCount::default());
    }

    for (path, hash) in &tree {
        let Some(rel) = path.strip_prefix(&prefix) else { continue };
        if !counted.is_match(rel) || hash.starts_with("symlink:") {
            continue;
        }
        if frozen.is_match(path) {
            let text = String::from_utf8_lossy(&std::fs::read(root.join(path))?).into_owned();
            m.frozen_names.extend(checks::scan(&text).0.into_iter().map(|d| d.full));
            continue;
        }
        let bytes = std::fs::read(root.join(path))?;
        let text = String::from_utf8_lossy(&bytes);
        let tokens = lexer::tokenize(&text);
        let code = lexer::stripped(&tokens);
        let loc = lexer::code_lines(&tokens) as u64;

        m.metrics.tokens += tokens.len() as u64;
        m.metrics.loc += loc;
        m.metrics.decls += decl_re.find_iter(&code).count() as u64;
        m.metrics.files += 1;
        m.metrics.bytes += bytes.len() as u64;
        m.files.push(FileStat { path: path.clone(), tokens: tokens.len() as u64, loc });
        let (decls, _) = checks::scan_tokens(&tokens, text.lines().count());
        m.names.insert(path.clone(), decls.into_iter().map(|d| d.full).collect());
        m.texts.insert(path.clone(), text.to_string());

        for (n, line) in text.lines().enumerate() {
            if line.chars().count() > cfg.score.max_line_length {
                m.long_lines += 1;
                if m.long_line_samples.len() < 5 {
                    m.long_line_samples.push(format!("{}:{}", path, n + 1));
                }
            }
        }

        for (name, re) in &protect {
            let set = m.protected.entry(name.clone()).or_default();
            for cap in re.captures_iter(&code) {
                if let Some(g) = cap.get(1) {
                    let from = cap.get(0).expect("group 0").start();
                    set.insert(g.as_str().to_string(), statement(&code[from..]));
                }
            }
        }

        for (i, re) in &rules {
            let rule = &cfg.deny[*i];
            let entry = m.deny.get_mut(&rule.name).expect("inserted above");
            for cap in re.captures_iter(&code) {
                entry.count += 1;
                if let Some(v) = cap.get(1).and_then(|g| g.as_str().parse::<u64>().ok()) {
                    entry.max_value = entry.max_value.max(v);
                }
                if entry.samples.len() < 5 {
                    let at = cap.get(0).expect("group 0").start();
                    let line = code[..at].matches('\n').count() + 1;
                    entry.samples.push(format!("{path}:{line}"));
                }
            }
        }
    }
    m.files.sort_by(|a, b| b.tokens.cmp(&a.tokens).then(a.path.cmp(&b.path)));
    m.tree = tree;
    Ok(m)
}

/// Paths whose content differs between two checkouts (added, removed, edited).
pub fn changed_paths(base: &Measurement, head: &Measurement) -> Vec<String> {
    let mut out = Vec::new();
    for (path, hash) in &head.tree {
        if base.tree.get(path) != Some(hash) {
            out.push(path.clone());
        }
    }
    for path in base.tree.keys() {
        if !head.tree.contains_key(path) {
            out.push(path.clone());
        }
    }
    out.sort();
    out
}

fn list(items: &[String], max: usize) -> String {
    let mut s = items.iter().take(max).cloned().collect::<Vec<_>>().join(", ");
    if items.len() > max {
        s.push_str(&format!(" and {} more", items.len() - max));
    }
    s
}

/// The declarations of `base` that `head` no longer has, by full name.
pub fn deleted_names(base: &Measurement, head: &Measurement) -> Vec<String> {
    let remaining: BTreeSet<&String> = head.names.values().flatten().chain(&head.frozen_names).collect();
    let gone: BTreeSet<&String> = base.names.values().flatten().filter(|n| !remaining.contains(n)).collect();
    gone.into_iter().cloned().collect()
}

/// Mentions of deleted declarations in the measured files of `head`.
pub fn deleted_mentions(base: &Measurement, head: &Measurement) -> Vec<checks::Hit> {
    let deleted = deleted_names(base, head);
    if deleted.is_empty() {
        return Vec::new();
    }
    let remaining: BTreeSet<String> = head.names.values().flatten().chain(&head.frozen_names).cloned().collect();
    let files: Vec<(&String, &String)> = head.texts.iter().collect();
    checks::deleted_mentions(&deleted, &remaining, &files)
}

/// The gates that read the text a change adds and removes: `added_lines`,
/// `deleted_names`, `conduct`, `comments`. All are repaired by a push.
fn text_gates(cfg: &Config, base: &Measurement, head: &Measurement, changed: &[String]) -> Vec<Gate> {
    let empty = String::new();
    // The measured files that differ, with their text on each side.
    let files: Vec<(&String, &String, Option<&String>)> = changed
        .iter()
        .filter(|p| base.texts.contains_key(*p) || head.texts.contains_key(*p))
        .map(|p| (p, base.texts.get(p).unwrap_or(&empty), head.texts.get(p)))
        .collect();
    let max = cfg.score.max_line_length;
    let mut long = Vec::new();
    let mut trailing = Vec::new();
    let mut links = Vec::new();
    let mut text_growth = 0i64;
    let mut counted = Vec::new();
    for (path, was, now) in &files {
        text_growth -= checks::free_text_chars(was) as i64;
        let tactics = &cfg.conduct.search_tactics.tactics;
        let now = now.unwrap_or(&empty);
        counted.push(checks::Counted {
            path: path.to_string(),
            base: checks::automation(was, tactics),
            head: checks::automation(now, tactics),
        });
        text_growth += checks::free_text_chars(now) as i64;
        let free = checks::free_text(now);
        for (n, raw) in checks::added_lines(was, now) {
            for issue in checks::line_issues(&raw, "", max) {
                match issue {
                    checks::LineIssue::TooLong(len) => long.push(format!("{path}:{n} ({len})")),
                    checks::LineIssue::TrailingSpace => trailing.push(format!("{path}:{n}")),
                    checks::LineIssue::Break => {}
                }
            }
            if free.get(&n).is_some_and(|t| checks::has_link(t)) {
                links.push(format!("{path}:{n}"));
            }
        }
    }
    let gate = |key: &str, problems: Vec<String>| {
        if problems.is_empty() {
            Gate::new(key, GateStatus::Pass, "")
        } else {
            Gate::new(key, GateStatus::Fail, problems.join("; "))
        }
    };
    let mut gates = Vec::new();

    // added_lines
    let mut problems = Vec::new();
    if !long.is_empty() {
        problems.push(format!("{} added line(s) longer than {max} characters: {}", long.len(), list(&long, 5)));
    }
    if !trailing.is_empty() {
        problems.push(format!("{} added line(s) end in whitespace: {}", trailing.len(), list(&trailing, 5)));
    }
    gates.push(gate("added_lines", problems));

    // deleted_names
    let mut by_name: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for h in deleted_mentions(base, head) {
        if h.certainty == checks::Certainty::Definite && !h.in_code {
            by_name.entry(h.name).or_default().push(format!("{}:{}", h.path, h.line));
        }
    }
    let named: Vec<String> = by_name.iter().map(|(name, at)| format!("`{name}` ({})", list(at, 3))).collect();
    gates.push(gate(
        "deleted_names",
        if named.is_empty() {
            Vec::new()
        } else {
            vec![format!(
                "{} deleted declaration(s) are still named in a comment: {}",
                named.len(),
                list(&named, 5)
            )]
        },
    ));

    // conduct
    gates.push(gate("conduct", checks::conduct_problems(&cfg.conduct, &counted)));

    // comments
    let mut problems = Vec::new();
    let limit = cfg.limits.max_added_comment_chars;
    if limit > 0 && text_growth > limit as i64 {
        problems.push(format!(
            "comments and string literals grew by {text_growth} characters; one change may add at most {limit}"
        ));
    }
    if !links.is_empty() {
        problems.push(format!(
            "{} added line(s) have a link in a comment or a string: {}",
            links.len(),
            list(&links, 5)
        ));
    }
    gates.push(if problems.is_empty() {
        Gate::new("comments", GateStatus::Pass, format!("comments and string literals {text_growth:+} characters"))
    } else {
        Gate::new("comments", GateStatus::Fail, problems.join("; "))
    });
    gates
}

/// The gates that can be decided without building: frozen, scope, deny,
/// no_increase, added_lines, deleted_names, conduct, comments, improves.
/// `deny` holds what is never allowed (forbidden constructs, protected
/// declarations); `no_increase` holds the counts that may not grow (long
/// lines, `no_increase` rules and their caps); the four after it read the
/// text the change adds and removes (see `checks`).
pub fn static_gates(cfg: &Config, base: &Measurement, head: &Measurement) -> Result<Vec<Gate>> {
    let changed = changed_paths(base, head);
    let frozen = cfg.frozen_set()?;
    let prefix = cfg.dir_prefix();
    let mut gates = Vec::new();

    // frozen
    let touched: Vec<String> = changed.iter().filter(|p| frozen.is_match(p.as_str())).cloned().collect();
    gates.push(if touched.is_empty() {
        Gate::new("frozen", GateStatus::Pass, "")
    } else {
        Gate::new("frozen", GateStatus::Fail, format!("Frozen files changed: {}", list(&touched, 5)))
    });

    // scope
    let counted = cfg.counted_set()?;
    let outside: Vec<String> = changed.iter().filter(|p| !p.starts_with(&prefix)).cloned().collect();
    // Inside the project only measured source files may change, and never
    // through a symbolic link: everything that is compiled must be counted.
    let unmeasured: Vec<String> = changed
        .iter()
        .filter(|p| p.strip_prefix(&prefix).is_some_and(|rel| !counted.is_match(rel)))
        .cloned()
        .collect();
    let links: Vec<String> = changed
        .iter()
        .filter(|p| head.tree.get(*p).is_some_and(|h| h.starts_with("symlink:")))
        .cloned()
        .collect();
    let mut problems = Vec::new();
    if !outside.is_empty() {
        problems.push(format!("Files outside `{}` changed: {}", cfg.project.dir, list(&outside, 5)));
    }
    if !unmeasured.is_empty() {
        problems.push(format!("Only source files may change: {}", list(&unmeasured, 5)));
    }
    if !links.is_empty() {
        problems.push(format!("Symbolic links are not allowed: {}", list(&links, 5)));
    }
    gates.push(if problems.is_empty() {
        Gate::new("scope", GateStatus::Pass, format!("{} files changed", changed.len()))
    } else {
        Gate::new("scope", GateStatus::Fail, problems.join("; "))
    });

    // deny: constructs that are never allowed. A failure is a verdict.
    let mut problems = Vec::new();
    // no_increase: counts that may not grow. The author repairs these with a push.
    let mut grew = Vec::new();
    for rule in &cfg.deny {
        let h = head.deny.get(&rule.name).cloned().unwrap_or_default();
        let b = base.deny.get(&rule.name).cloned().unwrap_or_default();
        match rule.mode {
            DenyMode::Forbid if h.count > 0 => {
                problems.push(format!("`{}` is not allowed ({})", rule.name, list(&h.samples, 3)))
            }
            DenyMode::NoIncrease if h.count > b.count => grew.push(format!(
                "`{}` occurrences rose from {} to {}",
                rule.name, b.count, h.count
            )),
            _ => {}
        }
        if let Some(max) = rule.max_value {
            if h.max_value > max {
                let problem = format!("`{}` value {} exceeds the cap {}", rule.name, h.max_value, max);
                match rule.mode {
                    DenyMode::Forbid => problems.push(problem),
                    DenyMode::NoIncrease => grew.push(problem),
                }
            }
        }
    }
    for rule in &cfg.protect {
        let empty = BTreeMap::new();
        let b = base.protected.get(&rule.name).unwrap_or(&empty);
        let h = head.protected.get(&rule.name).unwrap_or(&empty);
        let gone: Vec<String> = b.keys().filter(|k| !h.contains_key(*k)).cloned().collect();
        let altered: Vec<String> =
            b.iter().filter(|(k, v)| h.get(*k).is_some_and(|hv| hv != *v)).map(|(k, _)| k.clone()).collect();
        if !gone.is_empty() {
            problems.push(format!("protected declarations removed ({}): {}", rule.name, list(&gone, 5)));
        }
        if !altered.is_empty() {
            problems.push(format!(
                "the statement of a protected declaration changed ({}): {}; only its proof may change",
                rule.name,
                list(&altered, 5)
            ));
        }
    }
    if head.long_lines > base.long_lines {
        grew.push(format!(
            "lines longer than {} characters rose from {} to {} ({})",
            cfg.score.max_line_length,
            base.long_lines,
            head.long_lines,
            list(&head.long_line_samples, 3)
        ));
    }
    gates.push(if problems.is_empty() {
        Gate::new("deny", GateStatus::Pass, "")
    } else {
        Gate::new("deny", GateStatus::Fail, problems.join("; "))
    });
    gates.push(if grew.is_empty() {
        Gate::new("no_increase", GateStatus::Pass, "")
    } else {
        Gate::new("no_increase", GateStatus::Fail, grew.join("; "))
    });
    gates.extend(text_gates(cfg, base, head, &changed));

    // improves
    let key = cfg.score.primary.as_str();
    let b = base.metrics.get(key).with_context(|| format!("unknown primary metric `{key}`"))?;
    let h = head.metrics.get(key).unwrap_or(0);
    let mut problems = Vec::new();
    if h + cfg.score.min_delta > b {
        problems.push(format!(
            "{key} went from {b} to {h}; it must drop by at least {}",
            cfg.score.min_delta
        ));
    }
    for guard in &cfg.score.guards {
        let (Some(gb), Some(gh)) = (base.metrics.get(&guard.metric), head.metrics.get(&guard.metric))
        else {
            continue;
        };
        let limit = gb as f64 * (1.0 + guard.max_increase_pct / 100.0);
        if gh as f64 > limit {
            problems.push(format!(
                "{} rose from {gb} to {gh} (limit +{}%)",
                guard.metric, guard.max_increase_pct
            ));
        }
    }
    gates.push(if problems.is_empty() {
        Gate::new("improves", GateStatus::Pass, format!("{key} {b} → {h} ({})", h as i64 - b as i64))
    } else {
        Gate::new("improves", GateStatus::Fail, problems.join("; "))
    });

    Ok(gates)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn cfg() -> Config {
        toml::from_str(
            r#"
            [project]
            slug = "t"
            title = "T"
            repo = "o/r"
            dir = "proj"
            frozen = ["proj/Statement.lean", "hillclimb.toml", ".github"]
            [score]
            primary = "tokens"
            min_delta = 2
            guards = [{ metric = "loc", max_increase_pct = 0.0 }]
            max_line_length = 40
            max_build_ratio = 1.3
            [[deny]]
            name = "axiom"
            pattern = '\baxiom\b'
            mode = "forbid"
            [[deny]]
            name = "maxHeartbeats"
            pattern = 'maxHeartbeats (\d+)'
            mode = "no_increase"
            max_value = 400
            "#,
        )
        .unwrap()
    }

    fn checkout(files: &[(&str, &str)]) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "hc-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&dir);
        for (path, text) in files {
            let p = dir.join(path);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
        }
        dir
    }

    const STATEMENT: &str = "def Goal : Prop := True\n";
    const BASE: &str = "-- helper\ntheorem a : True := by\n  trivial\n\ntheorem b : True := by\n  exact a\n";

    fn gates(head_files: &[(&str, &str)]) -> Vec<Gate> {
        let c = cfg();
        let base = checkout(&[("proj/Statement.lean", STATEMENT), ("proj/A.lean", BASE), ("README.md", "x")]);
        let head = checkout(head_files);
        let b = measure(&base, &c).unwrap();
        let h = measure(&head, &c).unwrap();
        static_gates(&c, &b, &h).unwrap()
    }

    fn status(gates: &[Gate], key: &str) -> GateStatus {
        gates.iter().find(|g| g.key == key).unwrap().status
    }

    #[test]
    fn measures_only_unfrozen_project_files() {
        let c = cfg();
        let base = checkout(&[("proj/Statement.lean", STATEMENT), ("proj/A.lean", BASE), ("other/B.lean", "def x := 1")]);
        let m = measure(&base, &c).unwrap();
        assert_eq!(m.metrics.files, 1);
        assert_eq!(m.metrics.decls, 2);
        assert_eq!(m.metrics.loc, 4);
        assert_eq!(m.metrics.tokens, 15);
    }

    #[test]
    fn real_improvement_passes() {
        let g = gates(&[
            ("proj/Statement.lean", STATEMENT),
            ("proj/A.lean", "theorem a : True := trivial\n"),
            ("README.md", "x"),
        ]);
        assert!(g.iter().all(|g| g.status == GateStatus::Pass), "{g:?}");
    }

    #[test]
    fn no_change_is_not_an_improvement() {
        let g = gates(&[("proj/Statement.lean", STATEMENT), ("proj/A.lean", BASE), ("README.md", "x")]);
        assert_eq!(status(&g, "improves"), GateStatus::Fail);
    }

    #[test]
    fn deleting_comments_earns_nothing() {
        let g = gates(&[
            ("proj/Statement.lean", STATEMENT),
            ("proj/A.lean", &BASE.replace("-- helper\n", "")),
            ("README.md", "x"),
        ]);
        assert_eq!(status(&g, "improves"), GateStatus::Fail);
    }

    #[test]
    fn editing_the_statement_fails_frozen() {
        let g = gates(&[
            ("proj/Statement.lean", "def Goal : Prop := False\n"),
            ("proj/A.lean", "theorem a : True := trivial\n"),
            ("README.md", "x"),
        ]);
        assert_eq!(status(&g, "frozen"), GateStatus::Fail);
    }

    #[test]
    fn adding_a_workflow_fails_frozen_and_scope() {
        let g = gates(&[
            ("proj/Statement.lean", STATEMENT),
            ("proj/A.lean", "theorem a : True := trivial\n"),
            ("README.md", "x"),
            (".github/workflows/x.yml", "on: push"),
        ]);
        assert_eq!(status(&g, "frozen"), GateStatus::Fail);
        assert_eq!(status(&g, "scope"), GateStatus::Fail);
    }

    #[test]
    fn only_source_files_may_change_and_never_through_links() {
        let g = gates(&[
            ("proj/Statement.lean", STATEMENT),
            ("proj/A.lean", "theorem a : True := trivial\n"),
            ("proj/notes.bin", "x"),
            ("README.md", "x"),
        ]);
        assert_eq!(status(&g, "scope"), GateStatus::Fail);

        let c = cfg();
        let base = checkout(&[("proj/Statement.lean", STATEMENT), ("proj/A.lean", BASE), ("other/Big.lean", BASE)]);
        let head = checkout(&[("proj/Statement.lean", STATEMENT), ("proj/A.lean", "theorem a : True := trivial\n"), ("other/Big.lean", BASE)]);
        #[cfg(unix)]
        std::os::unix::fs::symlink("../other/Big.lean", head.join("proj/B.lean")).unwrap();
        let g = static_gates(&c, &measure(&base, &c).unwrap(), &measure(&head, &c).unwrap()).unwrap();
        #[cfg(unix)]
        assert_eq!(status(&g, "scope"), GateStatus::Fail);
    }

    #[test]
    fn deleting_outside_scope_fails() {
        let g = gates(&[("proj/Statement.lean", STATEMENT), ("proj/A.lean", "theorem a : True := trivial\n")]);
        assert_eq!(status(&g, "scope"), GateStatus::Fail);
    }

    #[test]
    fn axioms_and_raised_limits_fail_deny() {
        let g = gates(&[
            ("proj/Statement.lean", STATEMENT),
            ("proj/A.lean", "axiom a : True\n"),
            ("README.md", "x"),
        ]);
        assert_eq!(status(&g, "deny"), GateStatus::Fail);
        let g = gates(&[
            ("proj/Statement.lean", STATEMENT),
            ("proj/A.lean", "set_option maxHeartbeats 100 in\ntheorem a : True := trivial\n"),
            ("README.md", "x"),
        ]);
        // One more of a tolerated construct is repaired by a push, not a verdict.
        assert_eq!(status(&g, "deny"), GateStatus::Pass);
        assert_eq!(status(&g, "no_increase"), GateStatus::Fail);
    }

    #[test]
    fn banned_words_inside_comments_are_ignored() {
        let g = gates(&[
            ("proj/Statement.lean", STATEMENT),
            ("proj/A.lean", "-- no axiom here\ntheorem a : True := trivial\n"),
            ("README.md", "x"),
        ]);
        assert_eq!(status(&g, "deny"), GateStatus::Pass);
    }

    #[test]
    fn removing_a_protected_declaration_fails_but_rewriting_it_passes() {
        let mut c = cfg();
        c.protect.push(crate::config::ProtectRule {
            name: "paper".into(),
            pattern: r"(?m)^\s*theorem (thm_\w+)".into(),
            description: String::new(),
        });
        let base_src = "theorem thm_main : True := by\n  trivial\n\ntheorem helper : True := by\n  exact thm_main\n";
        let run = |head_src: &str| {
            let base = checkout(&[("proj/A.lean", base_src)]);
            let head = checkout(&[("proj/A.lean", head_src)]);
            let g = static_gates(&c, &measure(&base, &c).unwrap(), &measure(&head, &c).unwrap()).unwrap();
            status(&g, "deny")
        };
        assert_eq!(run("theorem helper : True := trivial\n"), GateStatus::Fail);
        assert_eq!(run("theorem thm_main : True := trivial\n"), GateStatus::Pass);
        // Same name, weaker statement: not allowed.
        assert_eq!(run("theorem thm_main : True ∨ False := .inl trivial\n"), GateStatus::Fail);
        // Reformatting the statement is fine.
        assert_eq!(run("theorem thm_main :\n    True :=\n  trivial\n"), GateStatus::Pass);
    }

    /// Base and head of one file `proj/A.lean`, judged by `c`.
    fn judged(c: &Config, base_src: &str, head_src: &str) -> Vec<Gate> {
        let base = checkout(&[("proj/A.lean", base_src)]);
        let head = checkout(&[("proj/A.lean", head_src)]);
        static_gates(c, &measure(&base, c).unwrap(), &measure(&head, c).unwrap()).unwrap()
    }

    fn detail(gates: &[Gate], key: &str) -> String {
        gates.iter().find(|g| g.key == key).unwrap().detail.clone().unwrap_or_default()
    }

    #[test]
    fn a_new_long_line_fails_even_when_another_long_line_goes() {
        // 41 characters each; the limit in these tests is 40.
        let long_a = "theorem a : True ∧ True := ⟨trivial, trivial⟩ --";
        let long_b = "theorem b : True ∨ False := Or.inl (trivial) --";
        assert!(long_a.chars().count() > 40 && long_b.chars().count() > 40);
        let base = format!("{long_a}\ntheorem c : True := trivial\n");
        // One long line removed, another added: the total does not grow.
        let g = judged(&cfg(), &base, &format!("theorem c : True := trivial\n{long_b}\n"));
        assert_eq!(status(&g, "no_increase"), GateStatus::Pass);
        assert_eq!(status(&g, "added_lines"), GateStatus::Fail);
        assert!(detail(&g, "added_lines").contains("proj/A.lean:2"), "{g:?}");
        // The same long line, moved: nothing was added.
        let g = judged(&cfg(), &base, &format!("theorem c : True := trivial\n\n{long_a}\n"));
        assert_eq!(status(&g, "added_lines"), GateStatus::Pass);
        // Trailing whitespace on a new line, not on one that was there.
        let g = judged(&cfg(), "theorem c : True := by \n  trivial\n", "theorem c : True := by \n  exact trivial \n");
        assert_eq!(status(&g, "added_lines"), GateStatus::Fail);
        assert!(detail(&g, "added_lines").contains("1 added line(s) end in whitespace: proj/A.lean:2"), "{g:?}");
    }

    #[test]
    fn a_deleted_name_may_not_be_mentioned_afterwards() {
        // The past case: comments that still named `equivSum` after it was deleted.
        let base = "def equivSum : Nat := 1\ntheorem uses : equivSum = 1 := rfl\ntheorem other : True := trivial\n";
        let g = judged(&cfg(), base, "/-- As in `equivSum`, split the sum. -/\ntheorem other : True := trivial\n");
        assert_eq!(status(&g, "deleted_names"), GateStatus::Fail);
        assert!(detail(&g, "deleted_names").contains("`equivSum` (proj/A.lean:1)"), "{g:?}");
        let g = judged(&cfg(), base, "/-- Splits the sum. -/\ntheorem other : True := trivial\n");
        assert_eq!(status(&g, "deleted_names"), GateStatus::Pass);
        // The other past case: `PKS_eval` kept after `PKS` was deleted is not a
        // mention of `PKS`, in code or in a comment.
        let base = "def PKS : Nat := 1\ntheorem PKS_eval : (1 : Nat) = 1 := rfl\ntheorem last : PKS = 1 := rfl\n";
        let g = judged(&cfg(), base, "-- see PKS_eval\ntheorem PKS_eval : (1 : Nat) = 1 := rfl\n");
        assert_eq!(status(&g, "deleted_names"), GateStatus::Pass, "{g:?}");
        let g = judged(&cfg(), base, "-- was PKS\ntheorem PKS_eval : (1 : Nat) = 1 := rfl\n");
        assert_eq!(status(&g, "deleted_names"), GateStatus::Fail);
        // A name the frozen statement file declares still exists: after a
        // local copy is deleted, the word means that one.
        let c = cfg();
        let statement = "def VP : Prop := True\n";
        let base = checkout(&[("proj/Statement.lean", statement), ("proj/A.lean", "namespace Nc\ndef VP : Prop := True\n/-- `VP` holds. -/\ntheorem t : VP := trivial\nend Nc\n")]);
        let head = checkout(&[("proj/Statement.lean", statement), ("proj/A.lean", "namespace Nc\n/-- `VP` holds. -/\ntheorem t : VP := trivial\nend Nc\n")]);
        let g = static_gates(&c, &measure(&base, &c).unwrap(), &measure(&head, &c).unwrap()).unwrap();
        assert_eq!(status(&g, "deleted_names"), GateStatus::Pass, "{g:?}");
        // A word in code that still compiles is the build's business.
        let g = judged(&cfg(), "def helper := 1\ntheorem t : helper = 1 := rfl\n", "theorem t : helper = 1 := rfl\n");
        assert_eq!(status(&g, "deleted_names"), GateStatus::Pass, "{g:?}");
    }

    #[test]
    fn conduct_is_judged_by_the_arena_settings() {
        let base = "theorem a : True := by\n  trivial\n\ntheorem b : True := by\n  exact a\n";
        let head = "theorem a : True := by\n  grind\n";
        let mut c = cfg();
        let g = judged(&c, base, head);
        assert_eq!(status(&g, "conduct"), GateStatus::Fail);
        assert!(detail(&g, "conduct").contains("this arena does not allow new ones"), "{g:?}");
        c.conduct.search_tactics.enabled = true;
        let g = judged(&c, base, head);
        assert_eq!(status(&g, "conduct"), GateStatus::Fail);
        assert!(detail(&g, "conduct").contains("without a comment"), "{g:?}");
        let g = judged(&c, base, "theorem a : True := by\n  grind -- nothing to use\n");
        assert_eq!(status(&g, "conduct"), GateStatus::Pass, "{g:?}");
        // A word in a comment or a string is not a call.
        let g = judged(&cfg(), base, "-- grind would do\ntheorem a : True := trivial\n");
        assert_eq!(status(&g, "conduct"), GateStatus::Pass, "{g:?}");
    }

    #[test]
    fn added_comment_text_is_limited_and_carries_no_links() {
        let base = "-- helper\ntheorem a : True := by\n  trivial\n";
        let mut c = cfg();
        c.limits.max_added_comment_chars = 30;
        let g = judged(&c, base, "-- helper, now direct\ntheorem a : True := trivial\n");
        assert_eq!(status(&g, "comments"), GateStatus::Pass, "{g:?}");
        let g = judged(&c, base, &format!("-- helper {}\ntheorem a : True := trivial\n", "x".repeat(31)));
        assert_eq!(status(&g, "comments"), GateStatus::Fail);
        assert!(detail(&g, "comments").contains("grew by 32 characters"), "{g:?}");
        // Text moved into a string literal is counted the same way.
        let g = judged(&c, base, &format!("def s := \"{}\"\ntheorem a : True := trivial\n", "x".repeat(40)));
        assert_eq!(status(&g, "comments"), GateStatus::Fail, "{g:?}");
        c.limits.max_added_comment_chars = 0;
        let g = judged(&c, base, &format!("-- helper {}\ntheorem a : True := trivial\n", "x".repeat(31)));
        assert_eq!(status(&g, "comments"), GateStatus::Pass, "0 turns the limit off");
        // A link in an added comment, with any limit; one that was there may stay.
        let g = judged(&c, base, "-- see https://example.org\ntheorem a : True := trivial\n");
        assert_eq!(status(&g, "comments"), GateStatus::Fail);
        assert!(detail(&g, "comments").contains("proj/A.lean:1"), "{g:?}");
        let with_link = "-- see www.example.org\ntheorem a : True := by\n  trivial\n";
        let g = judged(&c, with_link, "-- see www.example.org\ntheorem a : True := trivial\n");
        assert_eq!(status(&g, "comments"), GateStatus::Pass, "{g:?}");
    }

    #[test]
    fn cramming_lines_fails() {
        let g = gates(&[
            ("proj/Statement.lean", STATEMENT),
            ("proj/A.lean", "theorem a : True := trivial; theorem bb : True := a\n"),
            ("README.md", "x"),
        ]);
        assert_eq!(status(&g, "deny"), GateStatus::Pass);
        assert_eq!(status(&g, "no_increase"), GateStatus::Fail);
    }
}
