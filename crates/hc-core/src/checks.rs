//! Checks of a change that read text only. They back the static gates
//! `added_lines`, `deleted_names`, `conduct` and `comments` (see
//! `eval::static_gates`) and the reports of `hc precheck`.
//!
//! * Added lines. A line of a file is *added* if the file has more lines
//!   with exactly that text than it had on the base. A line that only moved
//!   inside its file is therefore not added, and no diff algorithm decides
//!   anything.
//! * Deleted names still mentioned. A declaration that exists in the base and
//!   not in the change is searched for, as a whole identifier, in every
//!   counted file, comments and doc comments included. A hit is *definite*
//!   when it is the full name, a dotted tail of it with at least two
//!   components (`Ns.foo` for `A.Ns.foo`), or the last component alone at a
//!   place inside the declaration's namespace (or anywhere, when the
//!   declaration was not in a namespace). Two exceptions keep the last
//!   component alone from being definite. If a remaining declaration has the
//!   same last component, the word still names something (after two copies
//!   were merged into one, say). And in the prose of a comment a name that
//!   is an ordinary word (`round`, `Tree`: letters only, no capital after
//!   the first) is definite only in backticks; "a round" is not a mention of
//!   `NS1.round`. The last component elsewhere, also as `x.foo`, is only
//!   *possible*, and is reported only when it is at least five characters
//!   long and no remaining declaration has the same last component;
//!   otherwise short, common names flood the output. An identifier that
//!   merely contains the name (`PKS_eval` for `PKS`) is no hit. A hit says
//!   whether it is in code or in a comment. The gate fails on definite hits
//!   in comments only: code that still compiles with the word in it names
//!   something else by it (a library lemma of the same name, a binder), and
//!   code that does not compile fails the build. One more exception follows
//!   from the same thought: if the code of the same file still uses the bare
//!   word, it names something there (a library lemma of that name, say, which
//!   the text cannot tell from the deleted one), and a comment in that file
//!   that says the bare word is only a possible mention.
//! * Automation: calls of search tactics, `attribute [local simp]` commands
//!   and their scope, global simp attributes, instance priorities, `clear`,
//!   simp calls that switch a lemma off, and `simp only []`, each counted on
//!   the tokens, so that nothing inside a comment or a string counts. Where
//!   a rule asks for a comment, the comment stands on the same line or on
//!   one of the two lines above (a comment, the `have … := by` or bullet it
//!   is about, then the call); for a call on a later line of a tactic step
//!   that runs over several lines joined by `<;>`, the same counted from
//!   the step's first line. A `clear` whose next tactic is `induction`,
//!   `generalize` or `revert` needs none: there it is part of that step.
//!   A tactic is counted with or without a trailing `?` or `!` (`grind?`,
//!   `simp_all!`). Attributes are counted where they stand as a command
//!   (`attribute [local simp] foo`) and on a declaration (`@[local simp]`,
//!   `@[simp]`).
//! * Text the score does not count: comments, string literals and names in
//!   `«…»`, each of which is one token or none whatever it says. What a
//!   change adds of it is measured on the added lines, so that deleting
//!   comments buys no room for new ones.
//!
//! Every scan here is linear in the size of the file.

use crate::config::Conduct;
use crate::lexer::{self, Token};
use std::collections::{BTreeMap, BTreeSet, HashMap};

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
    scan_tokens(&lexer::tokenize(src), src.lines().count())
}

/// As [`scan`], for a file that is already tokenized and has `line_count` lines.
pub fn scan_tokens(tokens: &[Token], line_count: usize) -> (Vec<Decl>, Vec<String>) {
    let lines = line_count + 2;
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
            kw if KEYWORDS.contains(&kw) && is_command(tokens, i) => {
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
fn is_command(tokens: &[Token], i: usize) -> bool {
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
    let is_word = |c: char| {
        c == '_' || c == '\'' || c == '!' || c == '?' || c == '.' || c.is_alphanumeric() || ('\u{2080}'..='\u{209C}').contains(&c)
    };
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

/// Where a word stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spot {
    Code,
    /// In a comment, directly after a backtick.
    Quoted,
    /// In the running text of a comment.
    Prose,
}

/// Whether a name reads as an ordinary word: letters only, and no capital
/// after the first (`round`, `Tree`; not `equivSum`, `PKS`, `thm_alt`).
fn plain_word(name: &str) -> bool {
    name.chars().all(|c| c.is_ascii_alphabetic()) && name.chars().skip(1).all(|c| c.is_ascii_lowercase())
}

/// How `word`, found at `spot` where namespace `here` is open, refers to the
/// deleted declaration `full`, if it does. `common` holds the last components
/// of the declarations that remain.
pub fn mention(word: &str, here: &str, full: &str, common: &BTreeSet<String>, spot: Spot) -> Option<Certainty> {
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
    let in_scope = ns.is_empty() || here == ns || here.starts_with(&format!("{ns}."));
    let names_another = common.contains(last);
    let just_a_word = spot == Spot::Prose && plain_word(last);
    if word == last && in_scope && !names_another && !just_a_word {
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
    /// In code, not in a comment. A line with both gives two hits.
    pub in_code: bool,
}

/// Mentions of `deleted` names in `files` (path, text). `remaining` are the
/// full names of all declarations still in the working tree.
pub fn deleted_mentions<P: AsRef<str>, T: AsRef<str>>(
    deleted: &[String],
    remaining: &BTreeSet<String>,
    files: &[(P, T)],
) -> Vec<Hit> {
    let common: BTreeSet<String> =
        remaining.iter().map(|f| f.rsplit('.').next().unwrap_or(f).to_string()).collect();
    // Every kind of mention contains the last component of the name, so a
    // file or a line without it needs no closer look.
    let lasts: Vec<&str> = deleted.iter().map(|d| d.rsplit('.').next().unwrap_or(d)).collect();
    let mut hits = Vec::new();
    for (path, text) in files {
        let (path, text) = (path.as_ref(), text.as_ref());
        let here_at_all: Vec<usize> = (0..deleted.len()).filter(|k| text.contains(lasts[*k])).collect();
        if here_at_all.is_empty() {
            continue;
        }
        let (tokens, comments) = lexer::scan(text);
        let (_, ns) = scan_tokens(&tokens, text.lines().count());
        // The bare words the code of this file uses.
        let code_words: BTreeSet<&str> = tokens.iter().map(|t| t.text.as_str()).collect();
        // The code of each line, and the pieces of comments on it.
        let code = lexer::stripped(&tokens);
        let code_lines: Vec<&str> = code.lines().collect();
        let mut prose: BTreeMap<usize, Vec<&str>> = BTreeMap::new();
        for c in &comments {
            for (k, piece) in c.text.split('\n').enumerate() {
                prose.entry(c.line + k).or_default().push(piece);
            }
        }
        for (n, line) in text.lines().enumerate() {
            let on_line: Vec<usize> = here_at_all.iter().copied().filter(|k| line.contains(lasts[*k])).collect();
            if on_line.is_empty() {
                continue;
            }
            let here = ns.get(n + 1).map(String::as_str).unwrap_or("");
            let mut ws: Vec<(Spot, String)> = words(code_lines.get(n).copied().unwrap_or(""))
                .into_iter()
                .map(|(_, w)| (Spot::Code, w))
                .collect();
            for piece in prose.get(&(n + 1)).into_iter().flatten() {
                let chars: Vec<char> = piece.chars().collect();
                for (at, w) in words(piece) {
                    // `words` trims dots, so the word may start after `at`.
                    let quoted = (0..=at).rev().take_while(|i| chars[*i] == '.' || *i == at).any(|i| i > 0 && chars[i - 1] == '`');
                    ws.push((if quoted { Spot::Quoted } else { Spot::Prose }, w));
                }
            }
            for k in on_line {
                let name = &deleted[k];
                for in_code in [false, true] {
                    let best = ws
                        .iter()
                        .filter(|(spot, _)| (*spot == Spot::Code) == in_code)
                        .filter_map(|(spot, w)| {
                            let m = mention(w, here, name, &common, *spot)?;
                            // The code of this file still uses the bare word:
                            // it names something here, and the comment may mean that.
                            let still_used = !in_code && w == lasts[k] && w != name && code_words.contains(w.as_str());
                            Some(if still_used { Certainty::Possible } else { m })
                        })
                        .min();
                    if let Some(certainty) = best {
                        hits.push(Hit { name: name.clone(), path: path.to_string(), line: n + 1, certainty, in_code });
                    }
                }
            }
        }
    }
    hits
}

/// The lines of a text as the tokenizer counts them: split at line feeds
/// only, so that a carriage return stays in its line and can be seen.
pub fn lines_of(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

/// The lines of `head` that `base` does not have: (1-based line number in
/// `head`, text). A line is added if `head` has more lines with exactly that
/// text than `base`; of several equal lines the later ones are the added
/// ones. A line that moved inside the file is not added, a line whose
/// indentation or content changed is.
pub fn added_lines(base: &str, head: &str) -> Vec<(usize, String)> {
    let mut have: HashMap<&str, usize> = HashMap::new();
    for l in lines_of(base) {
        *have.entry(l).or_default() += 1;
    }
    let mut out = Vec::new();
    for (i, l) in lines_of(head).into_iter().enumerate() {
        match have.get_mut(l) {
            Some(n) if *n > 0 => *n -= 1,
            _ => out.push((i + 1, l.to_string())),
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub enum LineIssue {
    TooLong(usize),
    /// The line ends in whitespace of any kind: a space, a tab, a carriage
    /// return, a form feed, a no-break space.
    TrailingSpace,
    /// The line holds a control character or one that does not show
    /// (zero-width characters, direction marks and overrides).
    Hidden(char),
    Break,
}

/// A character a reader does not see, or that changes how the characters
/// around it are shown.
pub fn is_hidden(c: char) -> bool {
    (c.is_control() && c != '\t')
        || matches!(c, '\u{ad}' | '\u{61c}' | '\u{180e}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{feff}')
}

/// Problems with one added line. `code` is the same line with comments removed.
pub fn line_issues(raw: &str, code: &str, max: usize) -> Vec<LineIssue> {
    let mut out = Vec::new();
    let n = raw.chars().count();
    if n > max {
        out.push(LineIssue::TooLong(n));
    }
    if raw.chars().next_back().is_some_and(char::is_whitespace) {
        out.push(LineIssue::TrailingSpace);
    }
    // Whitespace at the end is said above, whatever kind it is.
    if let Some(c) = raw.trim_end().chars().find(|c| is_hidden(*c)) {
        out.push(LineIssue::Hidden(c));
    }
    let c = code.trim_start();
    let after_parens = c.trim_start_matches([')', ' ']);
    const OPS: [char; 13] = ['+', '-', '*', '/', '^', '•', '∣', '≤', '≥', '<', '>', '=', '≠'];
    if c.starts_with(['*', '/', '^']) || (c.starts_with(')') && after_parens.starts_with(OPS)) {
        out.push(LineIssue::Break);
    }
    out
}

/// One place where something countable stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Place {
    pub line: usize,
    /// Whether a comment stands at the place: on that line or one of the
    /// two lines above it (or above the first line of its tactic step).
    pub commented: bool,
}

/// What a file does to automation, counted on its tokens.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Automation {
    /// Calls of a search tactic (outside attribute lists), with or without
    /// a trailing `?` or `!`.
    pub search: Vec<Place>,
    /// Lines of those with a trailing `?`: they print a suggestion on every build.
    pub suggesting: Vec<usize>,
    /// `attribute [local simp]` commands and declarations marked `@[local simp]`.
    pub local_simp: Vec<Place>,
    /// Lines of those that are neither inside a `section` or `namespace`
    /// block nor of the form `attribute [local simp] … in`.
    pub unscoped_local_simp: Vec<usize>,
    /// Lines of `attribute [simp]` commands that are neither local nor scoped.
    pub global_simp: Vec<usize>,
    /// Lines of declarations marked `@[simp]`.
    pub simp_declarations: Vec<usize>,
    /// Lines of `attribute [-simp]` commands.
    pub erased_simp: Vec<usize>,
    /// Lines of instance priorities: `attribute [instance 100]`,
    /// `@[instance 100]`, `instance (priority := …)`.
    pub instance_priority: Vec<usize>,
    /// Lines of other attributes that change what automation does, as a
    /// command or on a declaration: `scoped simp`, `local instance`,
    /// `scoped instance`, and `grind` of any kind.
    pub tuning: Vec<usize>,
    /// `clear` tactics.
    pub clear: Vec<Place>,
    /// Simp calls with a lemma switched off (`simp [-foo]`).
    pub disabled_lemmas: Vec<Place>,
    /// Lines of `simp only []`.
    pub empty_simp_only: Vec<usize>,
}

/// Tactics whose bracketed list is a simp set.
const SIMP_LIKE: [&str; 17] = [
    "simp", "simpa", "simp_all", "dsimp", "simp!", "simpa!", "simp_all!", "dsimp!", "simp?", "simpa?", "simp_all?",
    "dsimp?", "simp_arith", "norm_num", "field_simp", "push_cast", "only",
];

/// Words that end the list of lemma names after `attribute [...]`.
const COMMAND_WORDS: [&str; 26] = [
    "theorem", "lemma", "def", "abbrev", "structure", "inductive", "class", "instance", "example", "private",
    "protected", "public", "noncomputable", "section", "namespace", "end", "open", "variable", "attribute",
    "set_option", "omit", "include", "mutual", "universe", "local", "scoped",
];

/// The lines on which a comment stands (every line of a block comment).
pub fn comment_lines(comments: &[lexer::Comment]) -> BTreeSet<usize> {
    let mut out = BTreeSet::new();
    for c in comments {
        for k in 0..=c.text.matches('\n').count() {
            out.insert(c.line + k);
        }
    }
    out
}

const NONE: usize = usize::MAX;

/// For every token that opens a bracket, the index of the token that closes
/// it, and the other way round; `NONE` where there is no partner.
fn brackets(tokens: &[Token]) -> Vec<usize> {
    let mut partner = vec![NONE; tokens.len()];
    let mut open: Vec<usize> = Vec::new();
    for (i, t) in tokens.iter().enumerate() {
        match t.text.as_str() {
            "(" | "[" | "{" | "⟨" => open.push(i),
            ")" | "]" | "}" | "⟩" => {
                if let Some(o) = open.pop() {
                    partner[o] = i;
                    partner[i] = o;
                }
            }
            _ => {}
        }
    }
    partner
}

/// Count what `text` does to automation. `tactics` are the search tactics.
pub fn automation(text: &str, tactics: &[String]) -> Automation {
    let (tokens, comments) = lexer::scan(text);
    let partner = brackets(&tokens);
    let with_comment = comment_lines(&comments);
    let text_at = |i: usize| tokens.get(i).map(|t| t.text.as_str()).unwrap_or("");
    let first_on_line = |i: usize| i == 0 || tokens[i - 1].line != tokens[i].line;
    // The lines that have a token, in order, with their first token, and for
    // each the first line of the tactic step it belongs to: lines joined by
    // `<;>` at the end of one or the start of the next are one step.
    let mut lines: Vec<(usize, &str, &str)> = Vec::new();
    for t in &tokens {
        match lines.last_mut() {
            Some(l) if l.0 == t.line => l.2 = t.text.as_str(),
            _ => lines.push((t.line, t.text.as_str(), t.text.as_str())),
        }
    }
    let mut step_start: HashMap<usize, usize> = HashMap::new();
    let mut next_first: HashMap<usize, &str> = HashMap::new();
    for k in 0..lines.len() {
        let (line, first, _) = lines[k];
        let start = match k.checked_sub(1).map(|j| lines[j]) {
            Some((above, _, last)) if last == "<;>" || first == "<;>" => step_start[&above],
            _ => line,
        };
        step_start.insert(line, start);
        next_first.insert(line, lines.get(k + 1).map(|l| l.1).unwrap_or(""));
    }
    let on_or_above = |line: usize| (line.saturating_sub(2)..=line).any(|l| with_comment.contains(&l));
    let commented = |line: usize| on_or_above(line) || on_or_above(step_start.get(&line).copied().unwrap_or(line));
    let place = |line: usize| Place { line, commented: commented(line) };

    let mut a = Automation::default();
    // Open `section`, `namespace` and `mutual` blocks.
    let mut depth = 0usize;
    // Tokens inside an attribute list are attributes, not tactic calls.
    let mut in_attribute_until = 0usize;
    let mut i = 0;
    while i < tokens.len() {
        let t = tokens[i].text.as_str();
        let line = tokens[i].line;
        match t {
            "section" | "namespace" | "mutual" if first_on_line(i) => depth += 1,
            "end" if first_on_line(i) => depth = depth.saturating_sub(1),
            "attribute" | "@" if text_at(i + 1) == "[" && partner[i + 1] != NONE => {
                let close = partner[i + 1];
                in_attribute_until = in_attribute_until.max(close);
                let command = t == "attribute";
                let mut local_simp = false;
                // The entries of the list, split at commas; of each the
                // first words are enough, and what stands in brackets
                // inside an entry is passed over.
                let mut entry: Vec<&str> = Vec::new();
                let mut k = i + 2;
                loop {
                    let at_end = k >= close;
                    if at_end || text_at(k) == "," {
                        let (scope, rest) = match entry.first() {
                            Some(&"local") => ("local", &entry[1..]),
                            Some(&"scoped") => ("scoped", &entry[1..]),
                            _ => ("", &entry[..]),
                        };
                        match rest {
                            ["simp", ..] if scope == "local" => local_simp = true,
                            ["simp", ..] if scope == "scoped" => a.tuning.push(line),
                            ["simp", ..] if command => a.global_simp.push(line),
                            ["simp", ..] => a.simp_declarations.push(line),
                            ["-", "simp", ..] if command => a.erased_simp.push(line),
                            ["instance", _, ..] => a.instance_priority.push(line),
                            ["instance"] if !scope.is_empty() => a.tuning.push(line),
                            ["grind", ..] => a.tuning.push(line),
                            _ => {}
                        }
                        entry.clear();
                        if at_end {
                            break;
                        }
                        k += 1;
                        continue;
                    }
                    if entry.len() < 3 {
                        entry.push(text_at(k));
                    }
                    k = if partner[k] != NONE && partner[k] > k { partner[k] + 1 } else { k + 1 };
                }
                if local_simp {
                    a.local_simp.push(place(line));
                    // `attribute [local simp] a b in`: the names, then `in`.
                    let mut k = close + 1;
                    while command && starts_ident(text_at(k)) && text_at(k) != "in" && !COMMAND_WORDS.contains(&text_at(k)) {
                        k += 1;
                    }
                    if depth == 0 && !(command && text_at(k) == "in") {
                        a.unscoped_local_simp.push(line);
                    }
                }
            }
            "priority" if text_at(i + 1) == ":=" && i > 0 && text_at(i - 1) == "(" => a.instance_priority.push(line),
            // Before an induction or a generalization the clear is part of
            // that step: what the hypothesis must not mention is plain there.
            "clear" if matches!(next_first.get(&line).copied().unwrap_or(""), "induction" | "generalize" | "revert") => {}
            "clear" => a.clear.push(place(line)),
            "simp" if text_at(i + 1) == "only" && text_at(i + 2) == "[" && text_at(i + 3) == "]" => {
                a.empty_simp_only.push(line)
            }
            "[" if i > in_attribute_until && i > 0 && partner[i] != NONE => {
                // The tactic this list belongs to, behind options such as
                // `+contextual` or `(config := …)`: a handful at most.
                let mut j = i - 1;
                for _ in 0..8 {
                    if j >= 2 && starts_ident(text_at(j)) && matches!(text_at(j - 1), "+" | "-") {
                        j -= 2;
                    } else if text_at(j) == ")" && partner[j] != NONE && partner[j] > 0 {
                        j = partner[j] - 1;
                    } else {
                        break;
                    }
                }
                if SIMP_LIKE.contains(&text_at(j)) {
                    let call_line = tokens[j].line;
                    let close = partner[i];
                    let mut k = i + 1;
                    let mut entry_start = true;
                    while k < close {
                        match text_at(k) {
                            "," => {
                                entry_start = true;
                                k += 1;
                                continue;
                            }
                            "-" if entry_start && starts_ident(text_at(k + 1)) => {
                                let at = tokens[k].line;
                                a.disabled_lemmas.push(Place { line: at, commented: commented(at) || commented(call_line) });
                                break;
                            }
                            _ => {}
                        }
                        entry_start = false;
                        k = if partner[k] != NONE && partner[k] > k { partner[k] + 1 } else { k + 1 };
                    }
                }
            }
            _ if i > in_attribute_until => {
                let name = t.trim_end_matches(['?', '!']);
                if tactics.iter().any(|s| s == name) {
                    a.search.push(place(line));
                    if t.ends_with('?') {
                        a.suggesting.push(line);
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    a
}

/// A counted file as a change has it: the path, and what it does to
/// automation on the base and in the change (empty where it does not exist).
pub struct Counted {
    pub path: String,
    pub base: Automation,
    pub head: Automation,
}

/// Up to `limit` places as `path:line`, and how many more there are.
fn places(path: &str, lines: impl Iterator<Item = usize>, limit: usize) -> String {
    let all: Vec<usize> = lines.collect();
    let mut s = all.iter().take(limit).map(|l| format!("{path}:{l}")).collect::<Vec<_>>().join(", ");
    if all.len() > limit {
        s.push_str(&format!(" and {} more", all.len() - limit));
    }
    s
}

/// The files in which a count rose, with the count before and after. If the
/// total over all changed files did not rise, nothing rose: code moved from
/// one file to another, and what it does was there before.
fn rises<'a>(files: &'a [Counted], count: &dyn Fn(&Automation) -> usize) -> Vec<(&'a Counted, usize, usize)> {
    let total = |side: &dyn Fn(&Counted) -> usize| files.iter().map(side).sum::<usize>();
    if total(&|f| count(&f.head)) <= total(&|f| count(&f.base)) {
        return Vec::new();
    }
    files.iter().map(|f| (f, count(&f.base), count(&f.head))).filter(|(_, b, h)| h > b).collect()
}

fn uncommented(places: &[Place]) -> usize {
    places.iter().filter(|p| !p.commented).count()
}

/// What the changed files do that the rules of conduct do not allow, as far
/// as it can be counted. Every rule compares a count in a file with the same
/// count on the base, so nothing that was already there is a problem. Each
/// problem names up to `limit` places.
pub fn conduct_problems(conduct: &Conduct, files: &[Counted], limit: usize) -> Vec<String> {
    let mut out = Vec::new();
    let here_or_above = "on the same line or one of the two lines above";
    // `what` without a comment may not become more in a file.
    let mut need_comment = |what: &str, why: &str, get: &dyn Fn(&Automation) -> &Vec<Place>| {
        for (f, b, h) in rises(files, &|a| uncommented(get(a))) {
            out.push(format!(
                "{} more {what} without a comment {here_or_above} in {} ({}); {why}",
                h - b,
                f.path,
                places(&f.path, get(&f.head).iter().filter(|p| !p.commented).map(|p| p.line), limit)
            ));
        }
    };
    let st = &conduct.search_tactics;
    let tactics = st.tactics.iter().map(|t| format!("`{t}`")).collect::<Vec<_>>().join(", ");
    if !st.tactics.is_empty() && st.enabled {
        need_comment(
            &format!("call(s) of a search tactic ({tactics})"),
            "the comment says what the removed lines used",
            &|a| &a.search,
        );
    }
    if conduct.local_automation.enabled {
        need_comment("local simp attribute(s)", "the comment says what the set is for", &|a| &a.local_simp);
    }
    if conduct.comment_clear {
        need_comment("`clear` tactic(s)", "the comment says why the hypotheses must go", &|a| &a.clear);
    }
    if conduct.comment_disabled_lemmas {
        need_comment(
            "simp call(s) that switch a lemma off (`simp [-foo]`)",
            "the comment says why the lemma is in the way",
            &|a| &a.disabled_lemmas,
        );
    }
    // A count that may rise only up to a limit per file.
    let mut capped = |what: &str, max: u32, count: &dyn Fn(&Automation) -> usize| {
        for f in files {
            let (b, h) = (count(&f.base), count(&f.head));
            if h > b && h > max as usize {
                out.push(format!("{} has {h} {what} (it had {b}); at most {max} in one file", f.path));
            }
        }
    };
    if !st.tactics.is_empty() && st.enabled {
        capped(&format!("calls of a search tactic ({tactics})"), st.max_per_file, &|a| a.search.len());
    }
    if conduct.local_automation.enabled {
        capped("local simp attributes", conduct.local_automation.max_local_simp_per_file, &|a| a.local_simp.len());
    }
    // A count that may not rise at all.
    let mut never = |what: &str, why: &str, get: &dyn Fn(&Automation) -> Vec<usize>| {
        for (f, b, h) in rises(files, &|a| get(a).len()) {
            out.push(format!(
                "{what} rose from {b} to {h} in {} ({}); {why}",
                f.path,
                places(&f.path, get(&f.head).into_iter(), limit)
            ));
        }
    };
    if !st.tactics.is_empty() && !st.enabled {
        never(
            &format!("calls of a search tactic ({tactics})"),
            "this arena does not allow new ones",
            &|a| a.search.iter().map(|p| p.line).collect(),
        );
    }
    if !st.tactics.is_empty() {
        never(
            "calls of a search tactic with `?`",
            "that form prints a suggestion on every build; write the tactic itself",
            &|a| a.suggesting.clone(),
        );
    }
    if conduct.local_automation.enabled {
        never(
            "local simp attributes outside a `section` or `namespace` block and not of the form `attribute [local simp] … in`",
            "the attribute must end where its use ends",
            &|a| a.unscoped_local_simp.clone(),
        );
    } else {
        never(
            "local simp attributes (`attribute [local simp]`, `@[local simp]`)",
            "this arena does not allow new ones",
            &|a| a.local_simp.iter().map(|p| p.line).collect(),
        );
    }
    never("global `attribute [simp]` commands", "a global attribute is never allowed", &|a| a.global_simp.clone());
    never(
        "declarations marked `@[simp]`",
        "a new global simp lemma changes every later `simp`; pass the lemma to the calls that need it",
        &|a| a.simp_declarations.clone(),
    );
    never(
        "`attribute [-simp]` commands",
        "erasing a lemma from the simp set is never allowed",
        &|a| a.erased_simp.clone(),
    );
    never("instance priorities", "changing an instance priority is never allowed", &|a| a.instance_priority.clone());
    never(
        "attributes that change what automation does (`scoped simp`, `local instance`, `scoped instance`, `grind`)",
        "only local simp attributes are provided for, where the arena allows them",
        &|a| a.tuning.clone(),
    );
    if conduct.no_empty_simp_only {
        never("`simp only []` calls", "write the step it stands for", &|a| a.empty_simp_only.clone());
    }
    out
}

/// The text of a file that the score does not count, line by line: what
/// stands in comments, in string literals and in names in `«…»` on each line.
pub fn free_text(text: &str) -> BTreeMap<usize, String> {
    let (tokens, comments) = lexer::scan(text);
    let mut out: BTreeMap<usize, String> = BTreeMap::new();
    let literal = tokens
        .iter()
        .filter(|t| lexer::is_string(t) || t.text.contains('«'))
        .map(|t| (t.line, t.text.as_str()));
    for (line, piece) in comments.iter().map(|c| (c.line, c.text.as_str())).chain(literal) {
        for (k, part) in piece.split('\n').enumerate() {
            out.entry(line + k).or_default().push_str(part);
        }
    }
    out
}

/// Whether a piece of text holds a link: a scheme and `://` (`https://`,
/// `ftp://`), or a bare `www.`. Characters that do not show are ignored, so
/// that one cannot split a link. Bare domain names are not looked for.
pub fn has_link(text: &str) -> bool {
    let t: Vec<char> = text.chars().filter(|c| !is_hidden(*c)).flat_map(char::to_lowercase).collect();
    let at = |i: usize, s: &str| s.chars().enumerate().all(|(k, c)| t.get(i + k) == Some(&c));
    (0..t.len()).any(|i| {
        (at(i, "://") && i > 0 && t[i - 1].is_ascii_alphanumeric())
            || (at(i, "www.") && (i == 0 || !t[i - 1].is_alphanumeric()) && t.get(i + 4).is_some_and(|c| c.is_alphanumeric()))
    })
}

/// What is wrong with the path of an added file, relative to the project
/// directory: only ASCII letters, digits, `_`, `/` and `.` are allowed, no
/// empty or dot-only component, at most `max_len` characters, and no link.
pub fn path_problem(rel: &str, max_len: usize) -> Option<String> {
    let plain = rel.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '/' | '.'));
    let components = rel.split('/').all(|c| !c.is_empty() && !c.starts_with('.'));
    if !plain || !components {
        return Some("a path may hold only ASCII letters, digits, `_`, `/` and `.`, and no component may be empty or start with a dot".into());
    }
    if rel.chars().count() > max_len {
        return Some(format!("a path may have at most {max_len} characters"));
    }
    if has_link(rel) {
        return Some("a path may not hold a link".into());
    }
    None
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
        let m = |w: &str, here: &str, full: &str| mention(w, here, full, &common, Spot::Code);
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
        // A remaining declaration with the same last component: the word names that one.
        assert_eq!(m("other_name", "A", "A.other_name"), None);
        // In the prose of a comment an ordinary word is a mention only in backticks.
        let at = |w: &str, full: &str, spot: Spot| mention(w, "NS1", full, &common, spot);
        assert_eq!(at("round", "NS1.round", Spot::Prose), Some(Certainty::Possible));
        assert_eq!(at("round", "NS1.round", Spot::Quoted), Some(Certainty::Definite));
        assert_eq!(at("round", "NS1.round", Spot::Code), Some(Certainty::Definite));
        assert_eq!(at("NS1.round", "NS1.round", Spot::Prose), Some(Certainty::Definite));
        assert_eq!(at("Tree", "NS1.Tree", Spot::Prose), None);
        for name in ["equivSum", "PKS", "thm_alt", "x₀", "h'"] {
            assert_eq!(at(name, &format!("NS1.{name}"), Spot::Prose), Some(Certainty::Definite), "{name}");
        }
    }

    #[test]
    fn past_cases_of_deleted_names() {
        let all = |deleted: &str, remaining: &[&str], text: &str| {
            let remaining: BTreeSet<String> = remaining.iter().map(|s| s.to_string()).collect();
            deleted_mentions(&[deleted.to_string()], &remaining, &[("p/X.lean", text)])
                .into_iter()
                .filter(|h| h.certainty == Certainty::Definite)
                .collect::<Vec<_>>()
        };
        // What the gate fails on: definite, in a comment.
        let hits = |deleted: &str, remaining: &[&str], text: &str| {
            all(deleted, remaining, text).into_iter().filter(|h| !h.in_code).map(|h| h.line).collect::<Vec<_>>()
        };
        // A local lemma replaced by the library's of the same name: the
        // call is a hit in code, which the build judges, not the gate.
        let library = "namespace Nc\ntheorem u : a + b = b + a := by\n  rw [add_comm']\nend Nc\n";
        let found = all("Nc.add_comm'", &["Nc.u"], library);
        assert_eq!(found.len(), 1);
        assert!(found[0].in_code && found[0].line == 3);
        assert!(hits("Nc.add_comm'", &["Nc.u"], library).is_empty());
        // Comments that still named `equivSum`, with and without backticks.
        assert_eq!(hits("equivSum", &["t"], "/-- Like `equivSum`, and\nas in equivSum. -/\ntheorem t : True := trivial\n"), [1, 2]);
        // `PKS_eval` kept after `PKS` was deleted: another identifier.
        assert!(hits("PKS", &["PKS_eval"], "-- by PKS_eval\ntheorem PKS_eval : True := trivial\n").is_empty());
        // Two copies merged into one in the enclosing namespace: the short
        // name inside the old namespace now means the one that remains.
        let merged = "namespace Nc.Dl\ntheorem u : True := by\n  have lw := length_uwBits_le b\n  trivial\nend Nc.Dl\n";
        assert!(hits("Nc.Dl.length_uwBits_le", &["Nc.length_uwBits_le", "Nc.Dl.u"], merged).is_empty());
        // A definition called `round` was deleted; "a round" in a doc comment is English.
        let prose = "namespace NS1\n/-- A round: guess budgets. See `round`. -/\ndef stepA := 1\n/-- The general round. -/\ndef stepB := 2\nend NS1\n";
        assert_eq!(hits("NS1.round", &["NS1.stepA", "NS1.stepB"], prose), [2]);
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
    fn added_lines_are_those_the_base_has_fewer_of() {
        let base = "a\n  simp\nb\n  simp\nlong\n";
        // `long` moved up, a third `  simp` and a re-indented `b` are new.
        let head = "long\na\n  simp\n b\n  simp\n  simp\n";
        assert_eq!(added_lines(base, head), [(4, " b".to_string()), (6, "  simp".to_string())]);
        assert!(added_lines(base, base).is_empty());
        assert_eq!(added_lines("", "x\n"), [(1, "x".to_string())]);
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

    fn auto(text: &str) -> Automation {
        automation(text, &["grind".to_string(), "simp_all".to_string()])
    }

    #[test]
    fn search_tactics_are_counted_on_tokens() {
        let a = auto(
            "theorem t : P := by\n  -- uses foo and bar\n  grind\ntheorem u : P := by\n  simp_all -- closes it with h\n\n\n  grind\n-- grind in a comment\ndef s := \"grind\"\n@[grind] theorem v : P := by grind_pattern\nattribute [local grind] v\n",
        );
        assert_eq!(
            a.search,
            [Place { line: 3, commented: true }, Place { line: 5, commented: true }, Place { line: 8, commented: false }]
        );
        // The past cases: the comment, the `have` or the bullet it is about, then the call.
        let a = auto(
            "theorem t : P := by\n  -- `pre 0` asks `o ν = d` for every `ν`\n  have e1 : pre 0 = {d} := by\n    grind\n  constructor\n  -- each condition forces the same entry, against `hc`\n  · simp_all\n  · grind\n  · grind\n",
        );
        let commented: Vec<bool> = a.search.iter().map(|p| p.commented).collect();
        assert_eq!(commented, [true, true, true, false]);
    }

    #[test]
    fn a_comment_above_a_step_of_several_lines_counts_for_its_last_line() {
        // The past case: the comment stands above the first of four lines joined by `<;>`.
        let a = auto(
            "theorem t : x = y := by\n  -- equal targets are edges of one stage (`isMin_unique`)\n  rcases x with _ | a <;>\n    rcases y with _ | b <;>\n    simp at he <;>\n    grind [isMin_unique]\n  rcases z with _ | c\n    <;> grind\n  simp\n  grind\n",
        );
        assert_eq!(
            a.search,
            [Place { line: 6, commented: true }, Place { line: 8, commented: false }, Place { line: 10, commented: false }]
        );
    }

    #[test]
    fn local_simp_and_its_scope() {
        let a = auto(
            "attribute [local simp] foo\nsection\n-- the bounds of this section\nattribute [local simp] bar baz\nend\nattribute [local simp] qux in\ntheorem t : P := by simp\nattribute [local simp ←, local instance] a b\n  in\ntheorem u : P := by simp\nnamespace N\nattribute [local simp] w\nend N\nattribute [scoped simp] z\n",
        );
        let lines: Vec<usize> = a.local_simp.iter().map(|p| p.line).collect();
        assert_eq!(lines, [1, 4, 6, 8, 12]);
        assert_eq!(a.unscoped_local_simp, [1]);
        assert_eq!(uncommented(&a.local_simp), 4);
        assert!(a.global_simp.is_empty() && a.instance_priority.is_empty());
    }

    #[test]
    fn global_attributes_and_priorities() {
        let a = auto(
            "attribute [simp] foo\nattribute [-simp] Nat.add_zero\nattribute [instance 100] inst\nattribute [local instance 10] inst2\n@[instance 50] def i : C := x\ninstance (priority := low) j : C := x\n@[simp] theorem t : P := p\nattribute [instance] k\nattribute [local simp] l\n-- attribute [simp] in a comment\n",
        );
        assert_eq!(a.global_simp, [1]);
        assert_eq!(a.erased_simp, [2]);
        assert_eq!(a.instance_priority, [3, 4, 5, 6]);
        assert_eq!(a.local_simp.len(), 1);
    }

    #[test]
    fn clear_disabled_lemmas_and_empty_simp_only() {
        let a = auto(
            "theorem t : P := by\n  clear h1 h2\n  -- `foo` loops with `bar`\n  simp [bar, -foo]\n  simp only []\n  simp +contextual (config := { zeta := false }) [-baz] at h\n  simpa [← x, h (-1)] using [-y, z]\n  exact f [-a]\n  clear h3 -- slows the linarith below\n  norm_num [\n    -qux]\n",
        );
        assert_eq!(a.clear, [Place { line: 2, commented: false }, Place { line: 9, commented: true }]);
        // Before an induction the clear is part of that step and is not counted.
        let b = auto("theorem t : P := by\n  clear ih\n  induction S using Finset.induction_on with\n  | empty => simp\n  clear h\n  simp\n");
        assert_eq!(b.clear, [Place { line: 5, commented: false }]);
        assert_eq!(
            a.disabled_lemmas,
            [Place { line: 4, commented: true }, Place { line: 6, commented: false }, Place { line: 11, commented: true }]
        );
        assert_eq!(a.empty_simp_only, [5]);
    }

    fn problems(c: &Conduct, files: &[Counted]) -> Vec<String> {
        conduct_problems(c, files, 3)
    }

    fn counted(base: &str, head: &str) -> Vec<Counted> {
        vec![Counted { path: "p/A.lean".into(), base: auto(base), head: auto(head) }]
    }

    #[test]
    fn conduct_counts_follow_the_arena_settings() {
        let mut c = Conduct::default();
        // Off: no new call, commented or not; one that was there may stay or move.
        let base = "theorem t : P := by\n  grind\n";
        assert!(problems(&c, &counted(base, "theorem t : P := by\n\n  grind\n")).is_empty());
        let more = "theorem t : P := by\n  grind\ntheorem u : P := by\n  -- uses h\n  grind\n";
        let p = problems(&c, &counted(base, more));
        assert!(p.len() == 1 && p[0].contains("rose from 1 to 2 in p/A.lean"), "{p:?}");
        // On: a new call needs a comment, and the file has a limit.
        c.search_tactics.enabled = true;
        assert!(problems(&c, &counted(base, more)).is_empty());
        let bare = "theorem t : P := by\n  grind\ntheorem u : P := by\n  grind\n";
        let p = problems(&c, &counted(base, bare));
        assert!(p.len() == 1 && p[0].contains("1 more call(s) of a search tactic") && p[0].contains("p/A.lean:2"), "{p:?}");
        c.search_tactics.max_per_file = 1;
        let p = problems(&c, &counted(base, more));
        assert!(p.len() == 1 && p[0].contains("has 2 calls of a search tactic") && p[0].contains("at most 1"), "{p:?}");
        // A file already over the limit is not a problem while its count does not rise.
        assert!(problems(&c, &counted(more, more)).is_empty());

        // Local simp attributes: none when off; with a comment and a scope when on.
        let c = Conduct::default();
        let p = problems(&c, &counted("", "section\n-- for the bounds\nattribute [local simp] foo\nend\n"));
        assert!(p.len() == 1 && p[0].contains("local simp attributes") && p[0].contains("rose from 0 to 1"), "{p:?}");
        let mut c = Conduct::default();
        c.local_automation.enabled = true;
        assert!(problems(&c, &counted("", "section\n-- for the bounds\nattribute [local simp] foo\nend\n")).is_empty());
        assert!(problems(&c, &counted("", "-- for t only\nattribute [local simp] foo in\ntheorem t : P := p\n")).is_empty());
        let p = problems(&c, &counted("", "attribute [local simp] foo\n"));
        assert_eq!(p.len(), 2, "{p:?}");
        assert!(p[0].contains("without a comment") && p[1].contains("outside a `section`"), "{p:?}");
        // Never, in either setting.
        for c in [Conduct::default(), c] {
            let p = problems(&c, &counted("", "attribute [simp] foo\ninstance (priority := 10) i : C := x\n"));
            assert_eq!(p.len(), 2, "{p:?}");
            assert!(p[0].contains("global `attribute [simp]`") && p[1].contains("instance priorities"), "{p:?}");
        }
    }

    #[test]
    fn clear_and_switched_off_lemmas_need_a_comment_and_can_be_turned_off() {
        let mut c = Conduct::default();
        let base = "theorem t : P := by\n  clear h\n  simp\n";
        let head = "theorem t : P := by\n  clear h\n  clear k\n  simp [-foo]\n  simp only []\n";
        let p = problems(&c, &counted(base, head));
        assert_eq!(p.len(), 3, "{p:?}");
        assert!(p[0].contains("1 more `clear` tactic(s) without a comment"), "{p:?}");
        assert!(p[1].contains("switch a lemma off"), "{p:?}");
        assert!(p[2].contains("`simp only []` calls rose from 0 to 1"), "{p:?}");
        let fixed = "theorem t : P := by\n  clear h\n  clear k -- slows omega\n  -- foo loops here\n  simp [-foo]\n";
        assert!(problems(&c, &counted(base, fixed)).is_empty());
        c.comment_clear = false;
        c.comment_disabled_lemmas = false;
        c.no_empty_simp_only = false;
        assert!(problems(&c, &counted(base, head)).is_empty());
    }

    #[test]
    fn code_that_moves_between_files_adds_nothing() {
        let c = Conduct::default();
        let body = "theorem t : P := by\n  clear h\n  grind\n";
        let files = vec![
            Counted { path: "p/Old.lean".into(), base: auto(body), head: auto("") },
            Counted { path: "p/New.lean".into(), base: auto(""), head: auto(body) },
        ];
        assert!(problems(&c, &files).is_empty());
    }

    #[test]
    fn text_the_score_does_not_count() {
        let src = "-- one\ntheorem t : s = \"two\" := by -- three\n  /- four\n  five -/ simp\ndef «a name» := r#\"raw\"#\n";
        let by_line = free_text(src);
        assert_eq!(by_line[&1], "-- one");
        assert_eq!(by_line[&2], "-- three\"two\"");
        assert_eq!(by_line[&3], "/- four");
        assert_eq!(by_line[&4], "  five -/");
        assert_eq!(by_line[&5], "«a name»r#\"raw\"#");
        assert!(!by_line.contains_key(&6));
    }

    #[test]
    fn links() {
        for yes in [
            "see HTTPS://example.org",
            "at www.example.org",
            "http://x",
            "ftp://evil.example/buy",
            "theorem «see https://evil.example/buy now» : True",
            "-- https:/\u{200b}/evil.example",
            "x (www.a.b)",
        ] {
            assert!(has_link(yes), "{yes}");
        }
        for no in ["the map http : A → B", "awww.x", "a :: b // c", "h : a ∣ b", "-- evil.example/buy", "x ://", "www. alone"] {
            assert!(!has_link(no), "{no}");
        }
    }

    #[test]
    fn paths_of_added_files() {
        assert_eq!(path_problem("Paper/Tree_2.lean", 80), None);
        for bad in ["Buy cheap things.lean", "Paper/Trée.lean", "Paper/a-b.lean", "Paper//A.lean", ".hidden/A.lean", "Paper/../A.lean"] {
            assert!(path_problem(bad, 80).unwrap().contains("ASCII letters"), "{bad}");
        }
        assert!(path_problem(&format!("{}.lean", "x".repeat(80)), 80).unwrap().contains("at most 80"));
        assert!(path_problem("see/www.evil.example/A.lean", 80).unwrap().contains("link"));
    }

    #[test]
    fn whitespace_and_hidden_characters_on_a_line() {
        let issues = |raw: &str| line_issues(raw, "", 100);
        for trailing in ["x ", "x\t", "x\u{a0}", "x\u{c}", "x\r", "x \r", "x\u{2003}", "x\u{3000}"] {
            assert_eq!(issues(trailing), [LineIssue::TrailingSpace], "{trailing:?}");
        }
        assert_eq!(issues("x -- a\rhidden"), [LineIssue::Hidden('\r')]);
        assert_eq!(issues("x \u{200b}y"), [LineIssue::Hidden('\u{200b}')]);
        assert_eq!(issues("x \u{202e}y"), [LineIssue::Hidden('\u{202e}')]);
        assert_eq!(issues("x\u{c}y"), [LineIssue::Hidden('\u{c}')]);
        assert_eq!(issues("  x ≤ y → ∀ ε, «n» ⟨a, b⟩ -- fine"), []);
        // A carriage return before the line feed stays in its line and is seen.
        assert_eq!(added_lines("a\n", "a\nb\r\n"), [(2, "b\r".to_string())]);
        assert_eq!(lines_of("a\n\nb"), ["a", "", "b"]);
    }

    #[test]
    fn search_tactics_with_a_mark_are_the_same_tactic() {
        let a = auto("theorem t : P := by\n  grind?\n  simp_all!\n  -- uses h\n  simp_all?\n  grind!\n  grind_pattern\n");
        assert_eq!(a.search.iter().map(|p| p.line).collect::<Vec<_>>(), [2, 3, 5, 6]);
        assert_eq!(a.suggesting, [2, 5]);
        // The suggesting form is refused in an arena that allows search tactics too.
        let mut c = Conduct::default();
        c.search_tactics.enabled = true;
        let p = problems(&c, &counted("", "theorem t : P := by\n  -- uses h\n  grind?\n"));
        assert!(p.len() == 1 && p[0].contains("with `?` rose from 0 to 1"), "{p:?}");
        for head in ["  grind?\n", "  simp_all?\n", "  simp_all!\n"] {
            let p = problems(&Conduct::default(), &counted("theorem t : P := by\n  trivial\n", &format!("theorem t : P := by\n{head}")));
            assert!(p.iter().any(|m| m.contains("calls of a search tactic") && m.contains("rose from 0 to 1")), "{head}: {p:?}");
        }
    }

    #[test]
    fn attributes_on_declarations_count_as_the_commands_do() {
        let a = auto(
            "@[local simp] theorem a : P := p\nsection\n-- for the bounds below\n@[local simp] theorem b : P := p\nend\n@[simp] theorem c : P := p\n@[simp, norm_cast] lemma d : P := p\n@[scoped simp] theorem e : P := p\nattribute [local instance] f\nattribute [scoped instance] f\nattribute [local grind] g\n@[grind =] theorem h : P := p\nattribute [instance] i\n@[ext] theorem j : P := p\n@[local simp ←, local instance] theorem k : P := p\n",
        );
        assert_eq!(a.local_simp.iter().map(|p| (p.line, p.commented)).collect::<Vec<_>>(), [(1, false), (4, true), (15, false)]);
        assert_eq!(a.unscoped_local_simp, [1, 15]);
        assert_eq!(a.simp_declarations, [6, 7]);
        assert_eq!(a.tuning, [8, 9, 10, 11, 12, 15]);
        assert!(a.global_simp.is_empty() && a.instance_priority.is_empty() && a.search.is_empty());

        // Every one of them is refused where automation may not be tuned,
        let base = "theorem a : True := trivial\n";
        for head in [
            "@[local simp] theorem a : True := trivial\n",
            "@[simp] theorem a : True := trivial\n",
            "@[scoped simp] theorem a : True := trivial\n",
            "theorem a : True := trivial\nattribute [local simp] a\n",
            "theorem a : True := trivial\nattribute [simp] a\n",
            "theorem a : True := trivial\nattribute [local instance] a\n",
            "theorem a : True := trivial\nattribute [local grind] a\n",
        ] {
            assert_eq!(problems(&Conduct::default(), &counted(base, head)).len(), 1, "{head}");
        }
        // and where it may, only the local simp attribute passes, with its comment and scope.
        let mut c = Conduct::default();
        c.local_automation.enabled = true;
        assert!(problems(&c, &counted(base, "section\n-- for t\n@[local simp] theorem a : True := trivial\nend\n")).is_empty());
        assert_eq!(problems(&c, &counted(base, "@[local simp] theorem a : True := trivial\n")).len(), 2);
        for head in ["@[simp] theorem a : True := trivial\n", "@[scoped simp] theorem a : True := trivial\n", "theorem a : True := trivial\nattribute [local instance] a\n"] {
            assert_eq!(problems(&c, &counted(base, head)).len(), 1, "{head}");
        }
        // A marked declaration that moves to another file, or is replaced by another, adds nothing.
        assert!(problems(&c, &counted("@[simp] theorem a : P := p\n", "@[simp] theorem b : P := q\n")).is_empty());
    }

    #[test]
    fn a_library_lemma_of_the_same_name_still_in_use() {
        let remaining: BTreeSet<String> = ["A.t".to_string()].into();
        let definite = |text: &str| {
            deleted_mentions(&["A.two_mul".to_string()], &remaining, &[("p/X.lean", text)])
                .into_iter()
                .filter(|h| h.certainty == Certainty::Definite && !h.in_code)
                .count()
        };
        // The code still says `two_mul`, so it names something there, and the comment may mean that.
        assert_eq!(definite("namespace A\n/-- Uses `two_mul`. -/\ntheorem t : 2 * n = n + n := two_mul n\nend A\n"), 0);
        // No code of the file uses the word: the comment names what was deleted.
        assert_eq!(definite("namespace A\n/-- Uses `two_mul`. -/\ntheorem t : 2 * n = n + n := by omega\nend A\n"), 1);
        // The full name is the deleted declaration whatever the code says.
        assert_eq!(definite("namespace A\n/-- Uses `A.two_mul`. -/\ntheorem t : 2 * n = n + n := two_mul n\nend A\n"), 1);
    }

    /// The inputs an adversarial review found to take quadratic time. Each is
    /// a few hundred kilobytes; a linear scan takes milliseconds, and the
    /// bound leaves room for a slow machine and an unoptimized build.
    #[test]
    fn scans_are_linear() {
        use std::time::{Duration, Instant};
        let tactics = vec!["grind".to_string(), "simp_all".to_string()];
        let inputs = [
            ("open attribute lists", "@[ ".repeat(40_000)),
            ("open attribute commands", "attribute [ ".repeat(40_000)),
            ("nested attribute lists", format!("{}{}", "@[ ".repeat(40_000), "] ".repeat(40_000))),
            ("parentheses before lists", format!("{}x{}", "(".repeat(40_000), ") [ ".repeat(40_000))),
            ("nested simp lists", format!("{}{}", "simp [ ".repeat(40_000), "] ".repeat(40_000))),
            ("a long chain of steps", "  clear h <;>\n".repeat(40_000)),
            ("options before a list", format!("simp {}[x]\n", "+a ".repeat(40_000)).repeat(2)),
            ("names after an attribute", format!("attribute [local simp] {}\n", "a ".repeat(40_000))),
        ];
        for (label, src) in &inputs {
            let t = Instant::now();
            let a = automation(src, &tactics);
            let _ = free_text(src);
            let _ = added_lines("", src);
            let took = t.elapsed();
            assert!(took < Duration::from_secs(5), "{label}: {} bytes took {took:?}", src.len());
            if *label == "a long chain of steps" {
                assert_eq!(a.clear.len(), 40_000);
            }
        }
    }

    /// No text makes a check panic: every token in every order, in short pieces.
    #[test]
    fn checks_do_not_panic_on_any_text() {
        let tactics = vec!["grind".to_string(), "simp_all".to_string()];
        let pieces = [
            "@", "[", "]", "(", ")", "attribute", "local", "simp", "-", "+", ",", "in", "clear", "<;>", "\n", "only",
            "priority", ":=", "instance", "section", "end", "namespace", "grind", "\"", "--", "/-", "-/", "`", ".", "«",
            "»", "'", "theorem", "x", "A.b", "  ", "\r", "\t", "{", "}", "⟨", "⟩", "scoped", "\\", "r\"", "r#\"", "s!",
            "\"#", "grind?", "#",
        ];
        let mut seed = 0x2545F4914F6CDD1Du64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let text = |next: &mut dyn FnMut() -> u64| {
            let mut s = String::new();
            for _ in 0..(next() % 40) {
                s.push_str(pieces[(next() % pieces.len() as u64) as usize]);
                if next() % 3 == 0 {
                    s.push(' ');
                }
            }
            s
        };
        for _ in 0..4000 {
            let (s, s2) = (text(&mut next), text(&mut next));
            let _ = automation(&s, &tactics);
            let _ = free_text(&s);
            let _ = lexer::problems(&s);
            let _ = added_lines(&s, &s2);
            for (_, l) in added_lines(&s2, &s) {
                let _ = line_issues(&l, "", 100);
                let _ = has_link(&l);
            }
            let (d, _) = scan(&s);
            let deleted: Vec<String> =
                d.into_iter().map(|d| d.full).chain(["x".to_string(), "A.b".to_string(), "simp".to_string()]).collect();
            let _ = deleted_mentions(&deleted, &BTreeSet::new(), &[("p", s2.as_str())]);
        }
    }}
