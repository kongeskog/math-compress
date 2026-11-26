//! `hc precheck`: everything the gates will say, and what they cannot say,
//! checked before a pull request is opened.
//!
//! * `hc eval`: every static gate, among them the ones that read the text a
//!   change adds and removes (added lines, deleted names still mentioned,
//!   automation within the stated limits, added comment text). Their rules
//!   are in `hc_core::checks`; a failure there fails the pull request too.
//! * Deleted names still mentioned, place by place. The definite mentions
//!   in comments are what the gate fails on. A mention in code is a warning
//!   (if the file still compiles, the word names something else there), and
//!   so are the possible ones (the last component of the name outside its
//!   namespace); both are reported here and nowhere else.
//! * Added lines, place by place: over the length limit or ending in
//!   whitespace (both fail the gate), and a line of code that starts with
//!   `*`, `/` or `^`, or with `)` and then an operator, which usually means
//!   a break in the middle of a product (a warning).
//! * Open pull requests that change the same files.
//! * The title and description, with the format gate, and any "N files" in
//!   the description against the diff. Nothing else in a description is
//!   checked by a machine.

use crate::Compared;
use anyhow::Result;
use hc_core::checks::{added_lines, line_issues, scan, Certainty, Hit, LineIssue};
use hc_core::lexer;
use regex::Regex;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct Options {
    pub offline: bool,
    pub title: Option<String>,
    pub body_file: Option<PathBuf>,
    /// The comparison was made against the change this one is built on
    /// (`--depends-ref`), not against main.
    pub on_dependency: bool,
}

/// "N files" (and sites, lemmas, tails) claimed in a description.
pub fn claimed_counts(body: &str) -> Vec<(u64, String)> {
    let re = Regex::new(r"(?i)\b(\d+)\s+(site|file|lemma|tail)s?\b").expect("static regex");
    re.captures_iter(body)
        .filter_map(|c| Some((c[1].parse().ok()?, c[2].to_lowercase())))
        .collect()
}

/// Warnings for counts in the description; only file counts can be checked.
pub fn count_warnings(body: &str, files_changed: usize) -> Vec<String> {
    let mut out = Vec::new();
    for (n, what) in claimed_counts(body) {
        if what == "file" && n as usize != files_changed {
            out.push(format!("the description says {n} file(s) but the diff changes {files_changed}"));
        }
    }
    out
}

/// Files of the project a description names: paths ending in `.lean`, and backticked
/// `Folder/File` forms that are a known file with `.lean` added. Returned as repository paths.
pub fn described_files(body: &str, known: &[&str], prefix: &str) -> BTreeSet<String> {
    let resolve = |w: &str| -> Option<String> {
        let w = w.strip_suffix(".lean").unwrap_or(w);
        if w.is_empty() || !w.contains('/') {
            return None;
        }
        [format!("{w}.lean"), format!("{prefix}{w}.lean")].into_iter().find(|c| known.contains(&c.as_str()))
    };
    let mut out = BTreeSet::new();
    let re = Regex::new(r"`([A-Za-z0-9_./-]+)`|([A-Za-z0-9_./-]+\.lean)\b").unwrap();
    for c in re.captures_iter(body) {
        let w = c.get(1).or_else(|| c.get(2)).map(|m| m.as_str()).unwrap_or("");
        if c.get(1).is_some() && !w.ends_with(".lean") && !w.contains('/') {
            continue;
        }
        if let Some(f) = resolve(w) {
            out.insert(f);
        }
    }
    out
}

/// A warning when the description names files of the project and none of them is changed:
/// usually a description written for another change.
pub fn described_files_warning(body: &str, known: &[&str], changed: &[String], prefix: &str) -> Option<String> {
    let named = described_files(body, known, prefix);
    if named.is_empty() || named.iter().any(|f| changed.contains(f)) {
        return None;
    }
    let shown: Vec<&str> = named.iter().take(5).map(String::as_str).collect();
    Some(format!(
        "the description names {} file(s) of the project ({}), none of which this change touches: is it the description of another change?",
        named.len(),
        shown.join(", ")
    ))
}

/// An open pull request: number, title, branch and the files it changes.
type OpenPr = (u64, String, String, Vec<String>);

fn open_prs(repo: &str) -> Result<Vec<OpenPr>> {
    let out = Command::new("gh")
        .args(["pr", "list", "-R", repo, "--state", "open", "--limit", "200", "--json", "number,title,headRefName,files"])
        .output()?;
    if !out.status.success() {
        anyhow::bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let v: Vec<Value> = serde_json::from_slice(&out.stdout)?;
    Ok(v.iter()
        .map(|p| {
            let s = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            let files = p
                .get("files")
                .and_then(Value::as_array)
                .map(|fs| fs.iter().filter_map(|f| f.get("path").and_then(Value::as_str)).map(str::to_string).collect())
                .unwrap_or_default();
            (p.get("number").and_then(Value::as_u64).unwrap_or(0), s("title"), s("headRefName"), files)
        })
        .collect())
}

fn current_branch(head: &Path) -> String {
    Command::new("git")
        .arg("-C")
        .arg(head)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Run every check; true if nothing blocks the pull request.
pub fn run(head: &Path, c: &Compared, o: &Options) -> Result<bool> {
    let cfg = &c.cfg;
    println!("== hc eval\n");
    let gates = hc_core::eval::static_gates(cfg, &c.base, &c.head)?;
    let gates_ok = crate::report(c, &gates);

    let changed = hc_core::eval::changed_paths(&c.base, &c.head);
    let head_files: BTreeSet<&str> = c.head.files.iter().map(|f| f.path.as_str()).collect();
    let base_files: BTreeSet<&str> = c.base.files.iter().map(|f| f.path.as_str()).collect();
    let mut warnings = 0;

    // Deleted names.
    println!("\n== Deleted names still mentioned\n");
    let empty = String::new();
    let mut base_names = BTreeMap::new();
    for p in &base_files {
        for d in scan(c.base.texts.get(*p).unwrap_or(&empty)).0 {
            base_names.entry(d.full).or_insert((p.to_string(), d.line));
        }
    }
    let deleted = hc_core::eval::deleted_names(&c.base, &c.head);
    let hits: Vec<Hit> = hc_core::eval::deleted_mentions(&c.base, &c.head);
    let definite = hits.iter().filter(|h| h.certainty == Certainty::Definite && !h.in_code).count();
    let possible = hits.len() - definite;
    if deleted.is_empty() {
        println!("No declaration was deleted.");
    } else {
        let shown: Vec<&str> = deleted.iter().take(10).map(String::as_str).collect();
        let more = if deleted.len() > 10 { format!(" and {} more", deleted.len() - 10) } else { String::new() };
        println!("{} declaration(s) deleted: {}{more}", deleted.len(), shown.join(", "));
        for name in &deleted {
            let mine: Vec<&Hit> = hits.iter().filter(|h| &h.name == name).collect();
            if mine.is_empty() {
                continue;
            }
            let (from, line) = &base_names[name];
            println!("{name} (was {from}:{line})");
            for h in mine {
                let tag = match (h.certainty, h.in_code) {
                    (Certainty::Definite, false) => "MENTIONED",
                    (Certainty::Definite, true) => "in code  ",
                    (Certainty::Possible, _) => "possible ",
                };
                println!("  [{tag}] {}:{}", h.path, h.line);
            }
        }
        if hits.is_empty() {
            println!("None is mentioned any more.");
        }
    }
    warnings += possible;

    // Added lines.
    let max = cfg.score.max_line_length;
    println!("\n== Added lines\n");
    let mut too_long = 0;
    let mut padded = 0;
    let mut found = false;
    for p in &changed {
        let Some(text) = c.head.texts.get(p) else { continue };
        let added = added_lines(c.base.texts.get(p).unwrap_or(&empty), text);
        let code = lexer::stripped(&lexer::tokenize(text));
        let code_lines: Vec<&str> = code.lines().collect();
        for (n, raw) in added {
            for issue in line_issues(&raw, code_lines.get(n - 1).copied().unwrap_or(""), max) {
                found = true;
                match issue {
                    LineIssue::TooLong(len) => {
                        too_long += 1;
                        println!("  [ERROR] {p}:{n}: {len} characters, over the limit of {max}");
                    }
                    LineIssue::TrailingSpace => {
                        padded += 1;
                        println!("  [ERROR] {p}:{n}: trailing whitespace");
                    }
                    LineIssue::Break => {
                        warnings += 1;
                        println!("  [check] {p}:{n}: check this break (the line starts in the middle of a product or sum): {}", raw.trim());
                    }
                }
            }
        }
    }
    if !found {
        println!("Nothing to report.");
    }

    // Open pull requests touching the same files.
    println!("\n== Open pull requests that change the same files\n");
    let mine: BTreeSet<&str> = changed.iter().map(String::as_str).collect();
    let have_gh = Command::new("gh").arg("--version").output().is_ok_and(|o| o.status.success());
    if o.offline {
        println!("Skipped (--offline).");
    } else if !have_gh {
        println!("Skipped: `gh` is not available.");
    } else {
        match open_prs(&cfg.project.repo) {
            Err(e) => println!("Skipped: could not list pull requests of {} ({e:#}).", cfg.project.repo),
            Ok(prs) => {
                let branch = current_branch(head);
                let mut any = false;
                for (number, title, head_ref, files) in prs {
                    if head_ref == branch && branch != "HEAD" {
                        continue;
                    }
                    let shared: Vec<&str> = files.iter().map(String::as_str).filter(|f| mine.contains(f)).collect();
                    if !shared.is_empty() {
                        any = true;
                        warnings += 1;
                        println!("  #{number} {title}\n      {}", shared.join(", "));
                    }
                }
                if !any && branch != "HEAD" {
                    println!("None (pull requests from branch `{branch}` are taken to be this one).");
                } else if !any {
                    println!("None.");
                }
            }
        }
    }

    // Title and description.
    let mut format_ok = true;
    if o.title.is_some() || o.body_file.is_some() {
        println!("\n== Title and description\n");
        match (&o.title, &o.body_file) {
            (Some(title), Some(path)) => {
                let body = std::fs::read_to_string(path)?;
                // Only the form of a `Depends` line can be checked here; the
                // gatekeeper also checks that it names an open pull request.
                let depends = hc_core::gate::depends_ref(&body);
                let (gate, _) =
                    hc_core::gate::format_check(&cfg.format, title, &body, depends.as_ref().err().map(String::as_str));
                format_ok = gate.status == hc_core::model::GateStatus::Pass;
                match gate.detail.as_deref() {
                    Some(d) if !format_ok => println!("  [FAIL] format - {d}"),
                    _ => println!("  [pass] format"),
                }
                match (depends, o.on_dependency) {
                    (Ok(Some(n)), false) => {
                        warnings += 1;
                        println!("  [warn ] the description says `Depends: #{n}`, but everything above was compared with main: run again with --depends-ref <a ref of #{n}'s head>");
                    }
                    (Ok(None), true) => {
                        warnings += 1;
                        println!("  [warn ] --depends-ref was given, but the description has no `Depends: #<number>` line: the gatekeeper will compare with main");
                    }
                    _ => {}
                }
                let known: Vec<&str> = head_files.iter().chain(base_files.iter()).copied().collect();
                if let Some(w) = described_files_warning(&body, &known, &changed, &cfg.dir_prefix()) {
                    warnings += 1;
                    println!("  [warn ] {w}");
                }
                for w in count_warnings(&body, changed.len()) {
                    warnings += 1;
                    println!("  [warn ] {w}");
                }
                let unchecked: Vec<String> = claimed_counts(&body)
                    .into_iter()
                    .filter(|(_, w)| w != "file")
                    .map(|(n, w)| format!("{n} {w}(s)"))
                    .collect();
                if !unchecked.is_empty() {
                    println!("  Not checked, count these yourself: {}", unchecked.join(", "));
                }
            }
            _ => {
                warnings += 1;
                println!("  Give both --title and --body-file to check them.");
            }
        }
    }

    let ok = gates_ok && definite == 0 && too_long == 0 && padded == 0 && format_ok;
    println!(
        "\nprecheck: {}; static gates {}; {} mention(s) of deleted names in comments ({} in code or possible); {} added line(s) over {} characters, {} ending in whitespace; {} warning(s).",
        if ok { "OK" } else { "NOT READY" },
        if gates_ok { "pass" } else { "FAIL" },
        definite,
        possible,
        too_long,
        max,
        padded,
        warnings
    );
    Ok(ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_naming_other_files() {
        let known = ["vp-vnp/Nc/Tow/Ok.lean", "vp-vnp/Paper/Groups.lean", "vp-vnp/Paper/Fan.lean"];
        let body = "This change flattens clusters in `Paper/Groups` and `Paper/Fan`; see also Nc/Tow/Ok.lean.";
        let named = described_files(body, &known, "vp-vnp/");
        assert_eq!(named.len(), 3);
        // The pull request changes only Ok.lean of another cluster: fine, one named file is changed.
        assert!(described_files_warning(body, &known, &["vp-vnp/Nc/Tow/Ok.lean".into()], "vp-vnp/").is_none());
        // Opened with another change's description: nothing it names is in the diff.
        let other = "Flattens `Paper/Groups` and `Paper/Fan`, re-cuts them.";
        let w = described_files_warning(other, &known, &["vp-vnp/Nc/Tow/Load3.lean".into()], "vp-vnp/");
        assert!(w.unwrap().contains("none of which"));
        // A description that names no file of the project gives no warning.
        assert!(described_files_warning("Shorter proofs in `Nc/Tow`.", &known, &[], "vp-vnp/").is_none());
    }

    #[test]
    fn description_counts() {
        assert_eq!(claimed_counts("Touches 3 files and 12 lemmas, 1 site"), [(3, "file".into()), (12, "lemma".into()), (1, "site".into())]);
        assert!(count_warnings("Changes 3 files.", 3).is_empty());
        assert_eq!(count_warnings("Changes 2 files.", 3).len(), 1);
        assert!(count_warnings("Shortens 7 tails.", 3).is_empty());
    }
}
