//! `hc precheck`: what reviewers and the bot most often sent back, checked
//! before a pull request is opened.
//!
//! * Deleted names still mentioned. A declaration that exists in the base and
//!   not in the working tree is searched for, as a whole identifier, in every
//!   counted file, comments and doc comments included. A hit is *definite*
//!   when it is the full name, a dotted tail of it with at least two
//!   components (`Ns.foo` for `A.Ns.foo`), or the last component alone at a
//!   place inside the declaration's namespace (or anywhere, when the
//!   declaration was not in a namespace). The last component elsewhere, also
//!   as `x.foo`, is only *possible*, and is reported only when it is at least
//!   five characters long and no remaining declaration has the same last
//!   component; otherwise short, common names flood the output.
//! * Added lines: over the length limit (an error), trailing whitespace, and a
//!   line of code that starts with `*`, `/` or `^`, or with `)` and then an
//!   operator, which usually means a break in the middle of a product.
//! * Open pull requests that change the same files.
//! * The title and description, with the format gate, and any "N files" in
//!   the description against the diff.

use crate::Compared;
use anyhow::Result;
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
}

const KEYWORDS: [&str; 8] = ["theorem", "lemma", "def", "abbrev", "structure", "inductive", "class", "instance"];

#[derive(Debug, Clone, PartialEq)]
pub struct Decl {
    /// Fully qualified name.
    pub full: String,
    pub line: usize,
}

/// The declarations of a file, with their names qualified by the enclosing
/// namespaces, and the namespace in force at the start of each line
/// (`namespaces[l]` for 1-based line `l`; index 0 is unused).
pub fn scan(src: &str) -> (Vec<Decl>, Vec<String>) {
    let tokens = lexer::tokenize(src);
    let lines = src.lines().count() + 2;
    let mut by_line = vec![String::new(); lines];
    // Each entry is a namespace (possibly dotted) or "" for a section or mutual block.
    let mut stack: Vec<String> = Vec::new();
    let current = |stack: &[String]| stack.iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(".");
    let mut filled = 0;
    let mut decls = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let t = &tokens[i];
        while filled < t.line && filled + 1 < lines {
            filled += 1;
            by_line[filled] = current(&stack);
        }
        let next = tokens.get(i + 1);
        match t.text.as_str() {
            "namespace" => {
                if let Some(n) = tokens.get(i + 1) {
                    stack.push(n.text.clone());
                    i += 1;
                }
            }
            "section" | "mutual" => stack.push(String::new()),
            "end" => {
                stack.pop();
            }
            kw if KEYWORDS.contains(&kw) && is_command(&tokens, i) => {
                if let Some(n) = next.filter(|n| starts_ident(&n.text)) {
                    let name = n.text.strip_prefix("_root_.");
                    let ns = current(&stack);
                    let full = match name {
                        Some(r) => r.to_string(),
                        None if ns.is_empty() => n.text.clone(),
                        None => format!("{ns}.{}", n.text),
                    };
                    decls.push(Decl { full, line: t.line });
                }
            }
            _ => {}
        }
        i += 1;
    }
    while filled + 1 < lines {
        filled += 1;
        by_line[filled] = current(&stack);
    }
    (decls, by_line)
}

fn starts_ident(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c == '_' || c == '«' || c.is_alphabetic())
}

/// Whether the keyword at `i` starts a command: first on its line after
/// attributes and modifiers, not, say, `deriving instance`.
fn is_command(tokens: &[lexer::Token], i: usize) -> bool {
    const MODIFIERS: [&str; 9] =
        ["private", "protected", "public", "noncomputable", "unsafe", "partial", "nonrec", "scoped", "local"];
    let line = tokens[i].line;
    let mut j = i;
    let mut depth = 0i32;
    while j > 0 && tokens[j - 1].line == line {
        j -= 1;
        let t = tokens[j].text.as_str();
        match t {
            "]" => depth += 1,
            "[" | "@[" => depth -= 1,
            _ if depth > 0 => {}
            _ if MODIFIERS.contains(&t) => {}
            _ if t.ends_with("@[") || t == "@" => {}
            _ => return false,
        }
    }
    true
}

/// Identifiers in a raw line, comments included: runs of identifier
/// characters and dots, with the column (in characters) where each starts.
fn words(line: &str) -> Vec<(usize, String)> {
    let is_word = |c: char| c == '_' || c == '\'' || c == '!' || c == '?' || c == '.' || c.is_alphanumeric() || ('\u{2080}'..='\u{209C}').contains(&c);
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut start = 0;
    for (k, c) in line.chars().enumerate() {
        if is_word(c) {
            if cur.is_empty() {
                start = k;
            }
            cur.push(c);
        } else if !cur.is_empty() {
            out.push((start, std::mem::take(&mut cur)));
        }
    }
    if !cur.is_empty() {
        out.push((start, cur));
    }
    out.into_iter()
        .map(|(s, w)| (s, w.trim_matches('.').to_string()))
        .filter(|(_, w)| !w.is_empty())
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Certainty {
    Definite,
    Possible,
}

/// How `word`, found at a place where namespace `here` is open, refers to the
/// deleted declaration `full`, if it does. `common` holds the last components
/// of the declarations that remain.
pub fn mention(word: &str, here: &str, full: &str, common: &BTreeSet<String>) -> Option<Certainty> {
    let parts: Vec<&str> = full.split('.').collect();
    let last = *parts.last()?;
    let ns = parts[..parts.len() - 1].join(".");
    if word == full {
        return Some(Certainty::Definite);
    }
    let wparts: Vec<&str> = word.split('.').collect();
    if wparts.len() >= 2 && wparts.len() <= parts.len() && parts[parts.len() - wparts.len()..] == wparts[..] {
        return Some(Certainty::Definite);
    }
    if word == last && (ns.is_empty() || here == ns || here.starts_with(&format!("{ns}."))) {
        return Some(Certainty::Definite);
    }
    let tail_matches = word == last || word.ends_with(&format!(".{last}"));
    if tail_matches && last.chars().count() >= 5 && !common.contains(last) {
        return Some(Certainty::Possible);
    }
    None
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub name: String,
    pub path: String,
    pub line: usize,
    pub certainty: Certainty,
}

/// Mentions of `deleted` names in `files` (path, text). `remaining` are the
/// full names of all declarations still in the working tree.
pub fn deleted_mentions(deleted: &[String], remaining: &BTreeSet<String>, files: &[(String, String)]) -> Vec<Hit> {
    let common: BTreeSet<String> =
        remaining.iter().map(|f| f.rsplit('.').next().unwrap_or(f).to_string()).collect();
    let mut hits = Vec::new();
    for (path, text) in files {
        let (_, ns) = scan(text);
        for (n, line) in text.lines().enumerate() {
            let here = ns.get(n + 1).map(String::as_str).unwrap_or("");
            let ws = words(line);
            for name in deleted {
                let best = ws.iter().filter_map(|(_, w)| mention(w, here, name, &common)).min();
                if let Some(certainty) = best {
                    hits.push(Hit { name: name.clone(), path: path.clone(), line: n + 1, certainty });
                }
            }
        }
    }
    hits
}

/// Added lines of a `git diff -U0` (one file): (line number in the new file, text).
pub fn added_lines(diff: &str) -> Vec<(usize, String)> {
    let hunk = Regex::new(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@").expect("static regex");
    let mut out = Vec::new();
    let mut next = 0;
    let mut in_hunk = false;
    for l in diff.lines() {
        if let Some(c) = hunk.captures(l) {
            next = c[1].parse().unwrap_or(0);
            in_hunk = true;
        } else if !in_hunk {
            continue;
        } else if let Some(text) = l.strip_prefix('+') {
            out.push((next, text.to_string()));
            next += 1;
        } else if l.starts_with(' ') {
            next += 1;
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub enum LineIssue {
    TooLong(usize),
    TrailingSpace,
    Break,
}

/// Problems with one added line. `code` is the same line with comments removed.
pub fn line_issues(raw: &str, code: &str, max: usize) -> Vec<LineIssue> {
    let mut out = Vec::new();
    let n = raw.chars().count();
    if n > max {
        out.push(LineIssue::TooLong(n));
    }
    if raw.ends_with([' ', '\t']) {
        out.push(LineIssue::TrailingSpace);
    }
    let c = code.trim_start();
    let after_parens = c.trim_start_matches([')', ' ']);
    const OPS: [char; 13] = ['+', '-', '*', '/', '^', '•', '∣', '≤', '≥', '<', '>', '=', '≠'];
    if c.starts_with(['*', '/', '^']) || (c.starts_with(')') && after_parens.starts_with(OPS)) {
        out.push(LineIssue::Break);
    }
    out
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

fn git_diff(a: &Path, b: &Path) -> String {
    Command::new("git")
        .args(["diff", "--no-index", "--no-color", "-U0", "--"])
        .arg(a)
        .arg(b)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

fn read(p: &Path) -> String {
    std::fs::read(p).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()
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
    let mut base_names = BTreeMap::new();
    for p in &base_files {
        for d in scan(&read(&c.base_dir.join(p))).0 {
            base_names.entry(d.full).or_insert((p.to_string(), d.line));
        }
    }
    let texts: Vec<(String, String)> = head_files.iter().map(|p| (p.to_string(), read(&head.join(p)))).collect();
    let remaining: BTreeSet<String> = texts.iter().flat_map(|(_, t)| scan(t).0).map(|d| d.full).collect();
    let deleted: Vec<String> = base_names.keys().filter(|n| !remaining.contains(*n)).cloned().collect();
    let hits = deleted_mentions(&deleted, &remaining, &texts);
    let definite = hits.iter().filter(|h| h.certainty == Certainty::Definite).count();
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
                let tag = if h.certainty == Certainty::Definite { "MENTIONED" } else { "possible " };
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
    let mut found = false;
    for p in &changed {
        if !head_files.contains(p.as_str()) {
            continue;
        }
        let new = head.join(p);
        let text = read(&new);
        let added = if base_files.contains(p.as_str()) {
            added_lines(&git_diff(&c.base_dir.join(p), &new))
        } else {
            text.lines().enumerate().map(|(i, l)| (i + 1, l.to_string())).collect()
        };
        let code = lexer::stripped(&lexer::tokenize(&text));
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
                        warnings += 1;
                        println!("  [warn ] {p}:{n}: trailing whitespace");
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
                let (gate, _) = hc_core::gate::format_check(&cfg.format, title, &body);
                format_ok = gate.status == hc_core::model::GateStatus::Pass;
                match gate.detail.as_deref() {
                    Some(d) if !format_ok => println!("  [FAIL] format - {d}"),
                    _ => println!("  [pass] format"),
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

    let ok = gates_ok && definite == 0 && too_long == 0 && format_ok;
    println!(
        "\nprecheck: {}; static gates {}; {} mention(s) of deleted names ({} possible); {} added line(s) over {} characters; {} warning(s).",
        if ok { "OK" } else { "NOT READY" },
        if gates_ok { "pass" } else { "FAIL" },
        definite,
        possible,
        too_long,
        max,
        warnings
    );
    Ok(ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "\
namespace A
/-- `helper` is used below. -/
theorem helper : True := trivial
@[simp] private lemma B.inner : True := trivial
deriving instance Repr for Nat
section
def plain := 1
end
end A
instance : Inhabited Nat := ⟨0⟩
theorem _root_.top : True := trivial
noncomputable def outer := 2
";

    #[test]
    fn declarations_and_namespaces() {
        let (d, ns) = scan(SRC);
        let names: Vec<&str> = d.iter().map(|d| d.full.as_str()).collect();
        assert_eq!(names, ["A.helper", "A.B.inner", "A.plain", "top", "outer"]);
        assert_eq!(d[0].line, 3);
        assert_eq!(ns[1], "");
        assert_eq!(ns[2], "A");
        assert_eq!(ns[7], "A");
        assert_eq!(ns[10], "");
    }

    #[test]
    fn kinds_of_mention() {
        let common: BTreeSet<String> = ["le".to_string(), "other_name".to_string()].into();
        let m = |w: &str, here: &str, full: &str| mention(w, here, full, &common);
        assert_eq!(m("A.helper", "", "A.helper"), Some(Certainty::Definite));
        assert_eq!(m("B.inner", "", "A.B.inner"), Some(Certainty::Definite));
        assert_eq!(m("helper", "A", "A.helper"), Some(Certainty::Definite));
        assert_eq!(m("helper", "A.Sub", "A.helper"), Some(Certainty::Definite));
        assert_eq!(m("helper", "Other", "A.helper"), Some(Certainty::Possible));
        assert_eq!(m("h.helper", "Other", "A.helper"), Some(Certainty::Possible));
        assert_eq!(m("top", "X", "top"), Some(Certainty::Definite));
        // Short or still-used last components are not reported outside the namespace.
        assert_eq!(m("le", "Other", "A.le"), None);
        assert_eq!(m("foo", "Other", "A.foo"), None);
        assert_eq!(m("other_name", "Other", "A.other_name"), None);
        assert_eq!(m("helpers", "A", "A.helper"), None);
        assert_eq!(m("AA.helper", "", "A.helper"), Some(Certainty::Possible));
    }

    #[test]
    fn mentions_in_comments_are_found() {
        let files = vec![
            ("p/X.lean".to_string(), "namespace A\n-- uses `helper` here\ntheorem t : True := trivial\nend A\n".to_string()),
            ("p/Y.lean".to_string(), "/-! See A.helper and also helper. -/\ntheorem u : True := trivial\n".to_string()),
            ("p/Z.lean".to_string(), "theorem helperish : True := trivial\n".to_string()),
        ];
        let remaining: BTreeSet<String> = ["A.t", "u", "helperish"].iter().map(|s| s.to_string()).collect();
        let hits = deleted_mentions(&["A.helper".to_string()], &remaining, &files);
        assert_eq!(hits.len(), 2, "{hits:?}");
        assert_eq!((hits[0].path.as_str(), hits[0].line, hits[0].certainty), ("p/X.lean", 2, Certainty::Definite));
        assert_eq!((hits[1].path.as_str(), hits[1].line, hits[1].certainty), ("p/Y.lean", 1, Certainty::Definite));
    }

    #[test]
    fn added_lines_from_a_diff() {
        let diff = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -3,0 +4,2 @@\n+one\n+two\n@@ -10 +12 @@\n-old\n+new\n";
        assert_eq!(added_lines(diff), [(4, "one".into()), (5, "two".into()), (12, "new".into())]);
        assert!(added_lines("").is_empty());
    }

    #[test]
    fn line_checks() {
        assert_eq!(line_issues("abc", "abc", 3), []);
        assert_eq!(line_issues("abcd", "abcd", 3), [LineIssue::TooLong(4)]);
        assert_eq!(line_issues("  ≤≤≤", "≤≤≤", 3), [LineIssue::TooLong(5)]);
        assert_eq!(line_issues("x := 1 ", "x := 1", 100), [LineIssue::TrailingSpace]);
        assert_eq!(line_issues("    * (b + c) := by", "* ( b + c ) := by", 100), [LineIssue::Break]);
        assert_eq!(line_issues("    ^ 2", "^ 2", 100), [LineIssue::Break]);
        assert_eq!(line_issues("    ) * x", ") * x", 100), [LineIssue::Break]);
        assert_eq!(line_issues("    )) ≤ x", ") ) ≤ x", 100), [LineIssue::Break]);
        assert_eq!(line_issues("    ) := by", ") := by", 100), []);
        // A bullet in a doc comment is not code.
        assert_eq!(line_issues("  * a list item", "", 100), []);
        assert_eq!(line_issues("  · simp", "· simp", 100), []);
    }

    #[test]
    fn description_counts() {
        assert_eq!(claimed_counts("Touches 3 files and 12 lemmas, 1 site"), [(3, "file".into()), (12, "lemma".into()), (1, "site".into())]);
        assert!(count_warnings("Changes 3 files.", 3).is_empty());
        assert_eq!(count_warnings("Changes 2 files.", 3).len(), 1);
        assert!(count_warnings("Shortens 7 tails.", 3).is_empty());
    }
}
