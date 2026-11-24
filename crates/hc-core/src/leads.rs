//! Leads for contributors: places where the measured source repeats itself.

use crate::config::Config;
use crate::eval;
use crate::lexer;
use anyhow::Result;
use regex::Regex;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct Occurrence {
    pub path: String,
    pub line: usize,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DuplicateGroup {
    /// Tokens of one copy.
    pub tokens: usize,
    /// Tokens that would go if all but one copy were removed.
    pub saving: usize,
    /// True when the copies also share their name; otherwise only the
    /// statement and proof are the same.
    pub same_name: bool,
    pub occurrences: Vec<Occurrence>,
}

/// Declarations whose text is identical, token for token, apart from comments
/// and layout (and optionally apart from the declaration's own name).
pub fn duplicates(root: &Path, cfg: &Config, min_tokens: usize) -> Result<Vec<DuplicateGroup>> {
    let tree = eval::tree(root)?;
    let counted = cfg.counted_set()?;
    let frozen = cfg.frozen_set()?;
    let prefix = cfg.dir_prefix();
    let start = Regex::new(
        r"^(?:(?:private|protected|public|noncomputable|nonrec|scoped|local)\s+)*(theorem|lemma|def|abbrev|instance|structure|inductive|class)\b",
    )
    .expect("static regex");
    let boundary = Regex::new(r"^(end|namespace|section|open|variable|universe|set_option|attribute|import|omit|include)\b")
        .expect("static regex");

    // Key: tokens after the name. Value: (name, occurrence, token count).
    let mut by_body: BTreeMap<String, Vec<(Occurrence, usize)>> = BTreeMap::new();
    for (path, hash) in &tree {
        let Some(rel) = path.strip_prefix(&prefix) else { continue };
        if !counted.is_match(rel) || frozen.is_match(path) || hash.starts_with("symlink:") {
            continue;
        }
        let text = String::from_utf8_lossy(&std::fs::read(root.join(path))?).into_owned();
        let tokens = lexer::tokenize(&text);
        let code = lexer::stripped(&tokens);
        // Group the stripped lines into declarations.
        let mut current: Option<(usize, Vec<&str>)> = None;
        let mut flush = |decl: Option<(usize, Vec<&str>)>| {
            let Some((line, lines)) = decl else { return };
            let joined = lines.join(" ");
            let words: Vec<&str> = joined.split(' ').filter(|w| !w.is_empty()).collect();
            // Skip attribute and modifier words to find the keyword and the name.
            let Some(k) = words.iter().position(|w| {
                matches!(*w, "theorem" | "lemma" | "def" | "abbrev" | "instance" | "structure" | "inductive" | "class")
            }) else {
                return;
            };
            if words.len() < min_tokens || k + 1 >= words.len() {
                return;
            }
            let name = words[k + 1].to_string();
            let body = format!("{} {}", words[..=k].join(" "), words[k + 2..].join(" "));
            by_body.entry(body).or_default().push((Occurrence { path: path.clone(), line, name }, words.len()));
        };
        for (n, line) in code.lines().enumerate() {
            let trimmed = line.trim_start();
            let indented = line.len() != trimmed.len();
            let begins = !indented && (start.is_match(trimmed) || trimmed.starts_with("@["));
            if begins {
                // An attribute line belongs to the declaration that follows it.
                let continuing_attr = current.as_ref().is_some_and(|(_, l)| l.iter().all(|x| x.trim_start().starts_with("@[")));
                if !continuing_attr {
                    flush(current.take());
                    current = Some((n + 1, Vec::new()));
                }
            } else if !indented && boundary.is_match(trimmed) {
                flush(current.take());
            }
            if let Some((_, lines)) = current.as_mut() {
                if !trimmed.is_empty() {
                    lines.push(line);
                }
            }
        }
        flush(current.take());
    }

    let mut out = Vec::new();
    for (_, mut group) in by_body {
        if group.len() < 2 {
            continue;
        }
        group.sort_by(|a, b| a.0.path.cmp(&b.0.path).then(a.0.line.cmp(&b.0.line)));
        let tokens = group[0].1;
        let same_name = group.iter().all(|g| last(&g.0.name) == last(&group[0].0.name));
        out.push(DuplicateGroup {
            tokens,
            saving: tokens * (group.len() - 1),
            same_name,
            occurrences: group.into_iter().map(|g| g.0).collect(),
        });
    }
    out.sort_by(|a, b| b.saving.cmp(&a.saving));
    Ok(out)
}

fn last(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}
