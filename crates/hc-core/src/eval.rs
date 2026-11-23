//! Static evaluation: measure a checkout and compare it with its base.
//! Nothing here executes project code; it only reads files.

use crate::config::{Config, DenyMode};
use crate::lexer;
use crate::model::{FileStat, Gate, GateStatus, Metrics};
use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
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
    /// Repo-relative path -> content hash, for every file in the checkout.
    #[serde(skip)]
    pub tree: BTreeMap<String, String>,
}

fn decl_regex() -> Regex {
    Regex::new(
        r"(?m)^\s*(?:@\[[^\]]*\]\s*)*(?:(?:private|protected|public|noncomputable|unsafe|partial|nonrec|scoped|local)\s+)*(?:theorem|lemma|def|abbrev|instance|structure|inductive|class|example|opaque|axiom)\b",
    )
    .expect("static regex")
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

    let mut m = Measurement::default();
    for rule in &cfg.deny {
        m.deny.insert(rule.name.clone(), DenyCount::default());
    }

    for (path, hash) in &tree {
        let Some(rel) = path.strip_prefix(&prefix) else { continue };
        if !counted.is_match(rel) || frozen.is_match(path) || hash.starts_with("symlink:") {
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

        for (n, line) in text.lines().enumerate() {
            if line.chars().count() > cfg.score.max_line_length {
                m.long_lines += 1;
                if m.long_line_samples.len() < 5 {
                    m.long_line_samples.push(format!("{}:{}", path, n + 1));
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

/// The gates that can be decided without building: frozen, scope, deny, improves.
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
    let outside: Vec<String> = changed.iter().filter(|p| !p.starts_with(&prefix)).cloned().collect();
    gates.push(if outside.is_empty() {
        Gate::new("scope", GateStatus::Pass, format!("{} files changed", changed.len()))
    } else {
        Gate::new(
            "scope",
            GateStatus::Fail,
            format!("Files outside `{}` changed: {}", cfg.project.dir, list(&outside, 5)),
        )
    });

    // deny
    let mut problems = Vec::new();
    for rule in &cfg.deny {
        let h = head.deny.get(&rule.name).cloned().unwrap_or_default();
        let b = base.deny.get(&rule.name).cloned().unwrap_or_default();
        match rule.mode {
            DenyMode::Forbid if h.count > 0 => {
                problems.push(format!("`{}` is not allowed ({})", rule.name, list(&h.samples, 3)))
            }
            DenyMode::NoIncrease if h.count > b.count => problems.push(format!(
                "`{}` occurrences rose from {} to {}",
                rule.name, b.count, h.count
            )),
            _ => {}
        }
        if let Some(max) = rule.max_value {
            if h.max_value > max {
                problems.push(format!("`{}` value {} exceeds the cap {}", rule.name, h.max_value, max));
            }
        }
    }
    if head.long_lines > base.long_lines {
        problems.push(format!(
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
        assert_eq!(status(&g, "deny"), GateStatus::Fail);
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
    fn cramming_lines_fails() {
        let g = gates(&[
            ("proj/Statement.lean", STATEMENT),
            ("proj/A.lean", "theorem a : True := trivial; theorem bb : True := a\n"),
            ("README.md", "x"),
        ]);
        assert_eq!(status(&g, "deny"), GateStatus::Fail);
    }
}
