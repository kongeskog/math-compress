//! A small, deterministic tokenizer for Lean 4 source.
//!
//! It is not the Lean parser. It is a fixed, documented approximation whose
//! only job is to give every contributor the same number for the same text:
//!
//! * `--` line comments and nested `/- ... -/` block comments (including doc
//!   comments) are removed and never counted. As in Lean, the character
//!   after `/-` is never part of a marker (`/-/` opens a comment and does not
//!   close it), and after `/--` or `/-!` whitespace, `--` comments and plain
//!   block comments are skipped before the end is looked for. `//`, `\/` and
//!   `<-` are tokens of Lean, so a `-` straight after one starts no comment.
//! * A string literal is one token, a raw string (`r"…"`, `r#"…"#` with any
//!   number of `#`, in which a backslash is an ordinary character) included.
//!   An interpolated string (`s!"…{e}…"`, also `f!` and `m!`) is one token
//!   for each piece of text, up to and including the `{` that opens a hole
//!   and from the `}` that closes it; what stands in a hole is tokenized as
//!   code. A character literal is one token.
//! * A string or a block comment that is not closed runs to the end of the
//!   file. That, and a string with a line break in it, is reported by
//!   [`problems`]; a gate refuses both, so that what is counted here and
//!   what Lean reads cannot drift apart over more than a line. Two more
//!   things are reported because Lean's reading depends on what cannot be
//!   seen here: a `{` in a plain string (syntax such as `throwError "… {e}"`
//!   takes the string as interpolated and reads the hole as code), and a
//!   comment that starts straight after an operator character (a notation
//!   may have made the two one token).
//! * An identifier is a run of letters, digits, `_`, `'`, `!`, `?`; a `.`
//!   followed by an identifier start continues it, so `Foo.bar` is one token.
//!   `λ`, `Π` and `Σ` are not letters: each is a token (`Σ'` too).
//! * A number is one token, read as Lean reads it (`12`, `1_000`, `1.5`,
//!   `2.`, `1.5e-3`, `0xFF`); it ends where its digits end, so `1r"…"` is a
//!   number and a raw string. One exception is kept: the digits of nested
//!   field accesses (`h.2.1`) are read as one number.
//! * Each bracket or comma `( ) [ ] { } ⟨ ⟩ ,` is one token.
//! * Any other run of ASCII operator characters is one token (`:=`, `=>`,
//!   `<;>`); every other non-space character is one token (`∀`, `≤`, `·`).

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub text: String,
    /// 1-based line on which the token starts.
    pub line: usize,
}

/// The Greek letters Lean keeps for binders: no name starts with or holds one.
fn is_binder_letter(c: char) -> bool {
    matches!(c, 'λ' | 'Π' | 'Σ')
}

fn is_ident_start(c: char) -> bool {
    c == '_' || (c.is_alphabetic() && !is_binder_letter(c))
}

fn is_ident_continue(c: char) -> bool {
    c == '_' || c == '\'' || c == '!' || c == '?' || (c.is_alphanumeric() && !is_binder_letter(c))
        // Subscript digits and letters, common in Lean names such as `m₀`.
        || ('\u{2080}'..='\u{209C}').contains(&c)
}

fn is_single(c: char) -> bool {
    matches!(c, '(' | ')' | '[' | ']' | '{' | '}' | '⟨' | '⟩' | ',')
}

fn is_ascii_op(c: char) -> bool {
    c.is_ascii_punctuation() && !is_single(c) && c != '"' && c != '_'
}

/// A comment as it stands in the source, delimiters included: a `--` line
/// comment (without its line break) or a `/- ... -/` block, doc comments
/// and module docs among them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    pub text: String,
    /// 1-based line on which the comment starts.
    pub line: usize,
}

/// Something in a file that the tokenizer and Lean may read differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    /// A string literal that is not closed before the end of the file.
    OpenString,
    /// A block comment that is not closed before the end of the file.
    OpenComment,
    /// A string literal with a line break in it.
    StringOverLines,
    /// A name in `«…»` with a line break in it, or not closed.
    NameOverLines,
    /// An unescaped `{` in a plain string, one that is not raw and does not
    /// follow `s!`, `f!` or `m!`. Other syntax (`throwError "…"`,
    /// `println! "…"`, `dbg_trace "…"`, `trace[…] "…"`, anything declared
    /// with `interpolatedStr`) takes such a string as interpolated, and Lean
    /// then reads the hole as code.
    BraceInString,
    /// A comment marker straight after an operator character. Lean starts a
    /// comment only where a token has ended, and a notation can make the
    /// operator and the first character of the marker one token.
    GluedComment,
}

struct Lexed {
    tokens: Vec<Token>,
    comments: Vec<Comment>,
    problems: Vec<(usize, Problem)>,
}

/// Tokenize `src`, dropping comments and whitespace.
pub fn tokenize(src: &str) -> Vec<Token> {
    lex(src).tokens
}

/// The tokens of `src` and, separately, its comments. The tokens are exactly
/// those of [`tokenize`]; the comments are for checks that read what the
/// score does not count.
pub fn scan(src: &str) -> (Vec<Token>, Vec<Comment>) {
    let l = lex(src);
    (l.tokens, l.comments)
}

/// Where `src` has something the tokenizer and Lean may read differently
/// (see [`Problem`]): (1-based line where it starts, what).
pub fn problems(src: &str) -> Vec<(usize, Problem)> {
    lex(src).problems
}

/// Whether a token is a string literal or a piece of an interpolated one.
pub fn is_string(t: &Token) -> bool {
    let s = t.text.as_str();
    s.starts_with('"') || (s.starts_with('}') && s.len() > 1) || is_raw_string(s)
}

fn is_raw_string(s: &str) -> bool {
    s.strip_prefix('r').is_some_and(|rest| rest.trim_start_matches('#').starts_with('"'))
}

/// If a raw string starts at `i` (`r`, any number of `#`, `"`), the number of `#`.
fn raw_string_start(chars: &[char], i: usize) -> Option<usize> {
    if chars[i] != 'r' {
        return None;
    }
    let hashes = chars[i + 1..].iter().take_while(|c| **c == '#').count();
    (chars.get(i + 1 + hashes) == Some(&'"')).then_some(hashes)
}

/// The index past the block comment that starts at `i` (`/-`), and whether it
/// is closed, as Lean reads it.
fn block_comment(chars: &[char], i: usize) -> (usize, bool) {
    let n = chars.len();
    let at = |j: usize, a: char, b: char| chars.get(j) == Some(&a) && chars.get(j + 1) == Some(&b);
    let doc = matches!(chars.get(i + 2), Some('-' | '!'));
    // The character after `/-` is part of the opening or is skipped; either
    // way it is not looked at again, so `/-/` does not end here.
    let mut j = (i + 3).min(n);
    if doc {
        // `/--` and `/-!` are tokens, and what Lean skips after any token it
        // skips here: whitespace, line comments and plain block comments.
        while j < n {
            if matches!(chars[j], ' ' | '\n' | '\r' | '\t') {
                j += 1;
            } else if at(j, '-', '-') {
                while j < n && chars[j] != '\n' {
                    j += 1;
                }
            } else if at(j, '/', '-') && !matches!(chars.get(j + 2), Some('-' | '!')) {
                let (end, closed) = block_comment(chars, j);
                if !closed {
                    return (n, false);
                }
                j = end;
            } else {
                break;
            }
        }
    }
    let mut depth = 1;
    while j < n {
        if at(j, '/', '-') {
            depth += 1;
            j += 2;
        } else if at(j, '-', '/') {
            depth -= 1;
            j += 2;
            if depth == 0 {
                return (j, true);
            }
        } else {
            j += 1;
        }
    }
    (n, false)
}

/// The index past a run of digits from `i`, in which `_` may stand before a
/// digit (`1_000`, `0x_FF`); `i` if no digit follows.
fn digits(chars: &[char], mut i: usize, digit: fn(char) -> bool) -> usize {
    loop {
        let mut j = i;
        while chars.get(j) == Some(&'_') {
            j += 1;
        }
        if !chars.get(j).is_some_and(|c| digit(*c)) {
            return i;
        }
        while chars.get(j).is_some_and(|c| digit(*c)) {
            j += 1;
        }
        i = j;
    }
}

/// The index past the exponent that starts at `i` (`e`, a sign or none,
/// digits), if one does.
fn exponent(chars: &[char], i: usize) -> Option<usize> {
    if !matches!(chars.get(i), Some('e' | 'E')) {
        return None;
    }
    let j = if matches!(chars.get(i + 1), Some('+' | '-')) { i + 2 } else { i + 1 };
    chars.get(j).is_some_and(char::is_ascii_digit).then(|| digits(chars, j, |c| c.is_ascii_digit()))
}

/// The index past the number that starts at `i` with a digit, as Lean reads
/// it. Where Lean reports an error (`0x` without a digit, `1e` without one,
/// `2.x`), the number ends before what Lean does not accept.
fn number_end(chars: &[char], i: usize) -> usize {
    let radix: Option<fn(char) -> bool> = match (chars[i], chars.get(i + 1)) {
        ('0', Some('x' | 'X')) => Some(|c| c.is_ascii_hexdigit()),
        ('0', Some('b' | 'B')) => Some(|c| matches!(c, '0' | '1')),
        ('0', Some('o' | 'O')) => Some(|c| matches!(c, '0'..='7')),
        _ => None,
    };
    if let Some(digit) = radix {
        let end = digits(chars, i + 2, digit);
        return if end > i + 2 { end } else { i + 1 };
    }
    let decimal: fn(char) -> bool = |c| c.is_ascii_digit();
    let mut j = digits(chars, i + 1, decimal);
    if chars.get(j) == Some(&'.') {
        match chars.get(j + 1) {
            // `1..` and `1...` are a number and a range.
            Some('.') => return j,
            Some(c) if c.is_ascii_digit() => j = digits(chars, j + 1, decimal),
            // `2.` is a number, also before an exponent (`2.e3`); before a
            // name (`2.succ`) Lean refuses it.
            next => {
                if let Some(end) = exponent(chars, j + 1) {
                    return end;
                }
                return if next.is_some_and(|c| is_ident_start(*c) || *c == '«') { j } else { j + 1 };
            }
        }
    }
    exponent(chars, j).unwrap_or(j)
}

/// Whether the `'` at `i` ends a token of Lean or Mathlib that begins before
/// it (`]'`, `×'`, `⊕'`, `∑'`, `∏'`, `∘'`, `⟧'`, `⁻¹'`): Lean takes the longer
/// token, so no character literal starts there.
fn quote_ends_token(chars: &[char], i: usize) -> bool {
    match i.checked_sub(1).map(|p| chars[p]) {
        Some(']' | '×' | '⊕' | '∑' | '∏' | '∘' | '⟧') => true,
        Some('¹') => i >= 2 && chars[i - 2] == '⁻',
        _ => false,
    }
}

fn lex(src: &str) -> Lexed {
    let chars: Vec<char> = src.chars().collect();
    let n = chars.len();
    let mut out: Vec<Token> = Vec::new();
    let mut comments = Vec::new();
    let mut problems = Vec::new();
    let mut i = 0;
    let mut line = 1;
    // The holes of interpolated strings that are open, innermost last: for
    // each, how many `{` inside it are open.
    let mut holes: Vec<usize> = Vec::new();
    // Where the last token ended, if it was an operator or another symbol.
    let mut operator_end: Option<usize> = None;

    while i < n {
        let c = chars[i];
        if c == '\n' {
            line += 1;
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        let comment = i + 1 < n && chars[i + 1] == '-' && (c == '-' || c == '/');
        if comment && operator_end == Some(i) {
            problems.push((line, Problem::GluedComment));
        }
        // Line comment.
        if comment && c == '-' {
            let start = i;
            while i < n && chars[i] != '\n' {
                i += 1;
            }
            comments.push(Comment { text: chars[start..i].iter().collect(), line });
            continue;
        }
        // Block comment, nested.
        if comment {
            let (start, start_line) = (i, line);
            let (end, closed) = block_comment(&chars, i);
            i = end;
            line += chars[start..i].iter().filter(|c| **c == '\n').count();
            if !closed {
                problems.push((start_line, Problem::OpenComment));
            }
            comments.push(Comment { text: chars[start..i].iter().collect(), line: start_line });
            continue;
        }
        let start = i;
        let start_line = line;
        let mut operator = false;
        // A piece of an interpolated string: after `s!`, `f!` or `m!`, or
        // after the `}` that closes a hole.
        let resumes = c == '}' && holes.last() == Some(&0);
        let interpolated =
            resumes || (c == '"' && out.last().is_some_and(|t| matches!(t.text.as_str(), "s!" | "f!" | "m!")));
        if interpolated || c == '"' {
            if resumes {
                holes.pop();
            }
            i += 1;
            let mut closed = false;
            let mut brace = false;
            while i < n {
                match chars[i] {
                    '"' => {
                        closed = true;
                        i += 1;
                        break;
                    }
                    '{' if interpolated => {
                        closed = true;
                        holes.push(0);
                        i += 1;
                        break;
                    }
                    '{' => {
                        brace = true;
                        i += 1;
                    }
                    '\\' if i + 1 < n => {
                        // An escape; a backslash before a line break is a gap.
                        if chars[i + 1] == '\n' {
                            line += 1;
                        }
                        i += 2;
                    }
                    ch => {
                        if ch == '\n' {
                            line += 1;
                        }
                        i += 1;
                    }
                }
            }
            if !closed {
                problems.push((start_line, Problem::OpenString));
            }
            if brace {
                problems.push((start_line, Problem::BraceInString));
            }
        } else if let Some(hashes) = raw_string_start(&chars, i) {
            // A raw string ends at the first `"` followed by as many `#`.
            i += hashes + 2;
            let mut closed = false;
            while i < n {
                if chars[i] == '"' && chars[i + 1..].iter().take(hashes).filter(|c| **c == '#').count() == hashes {
                    closed = true;
                    i += 1 + hashes;
                    break;
                }
                if chars[i] == '\n' {
                    line += 1;
                }
                i += 1;
            }
            if !closed {
                problems.push((start_line, Problem::OpenString));
            }
        } else if c == '\'' && !quote_ends_token(&chars, i) && is_char_literal(&chars, i).is_some() {
            i = is_char_literal(&chars, i).unwrap();
            // The character may be a line break.
            line += chars[start..i].iter().filter(|c| **c == '\n').count();
        } else if c.is_ascii_digit() {
            i = number_end(&chars, i);
        } else if is_ident_start(c) || c == '«' {
            loop {
                if i < n && chars[i] == '«' {
                    // Lean reads on to the closing quote, over line breaks too.
                    let mut broken = false;
                    while i < n && chars[i] != '»' {
                        if chars[i] == '\n' {
                            line += 1;
                            broken = true;
                        }
                        i += 1;
                    }
                    if broken || i == n {
                        problems.push((start_line, Problem::NameOverLines));
                    }
                    i = (i + 1).min(n);
                } else {
                    while i < n && is_ident_continue(chars[i]) {
                        i += 1;
                    }
                }
                if i + 1 < n && chars[i] == '.' && (is_ident_start(chars[i + 1]) || chars[i + 1] == '«')
                {
                    i += 1;
                } else {
                    break;
                }
            }
        } else if is_single(c) {
            if let Some(depth) = holes.last_mut() {
                match c {
                    '{' => *depth += 1,
                    '}' => *depth = depth.saturating_sub(1),
                    _ => {}
                }
            }
            i += 1;
        } else if is_ascii_op(c) {
            operator = true;
            if c == '\'' && chars.get(i + 1) == Some(&'\'') {
                // Lean reads no character literal at `''`: the two are a
                // token (Mathlib's image of a set), also in `f ''s'`.
                i += 2;
            }
            while i < n && is_ascii_op(chars[i]) {
                if i > start && i + 1 < n {
                    // `//`, `\/` and `<-` are tokens of Lean, and no token of
                    // Lean or Mathlib continues one of them with a `-`: the
                    // `-` that follows starts a new token and no comment.
                    if chars[i + 1] == '-' && matches!((chars[i - 1], chars[i]), ('/', '/') | ('\\', '/') | ('<', '-')) {
                        if i - 1 > start {
                            // Whether Lean's token starts where the pair
                            // does is not known after other characters.
                            problems.push((line, Problem::GluedComment));
                        }
                        i += 1;
                        break;
                    }
                    // Never swallow the start of a comment or of a character literal.
                    if (chars[i] == '-' && chars[i + 1] == '-')
                        || (chars[i] == '/' && chars[i + 1] == '-')
                        || (chars[i] == '\'' && is_char_literal(&chars, i).is_some())
                    {
                        break;
                    }
                }
                i += 1;
            }
        } else {
            operator = true;
            // `Σ'` is a token of Lean.
            i += if c == 'Σ' && chars.get(i + 1) == Some(&'\'') { 2 } else { 1 };
        }
        operator_end = operator.then_some(i);
        let text: String = chars[start..i].iter().collect();
        if text.contains('\n') && (interpolated || c == '"' || raw_string_start(&chars, start).is_some()) {
            problems.push((start_line, Problem::StringOverLines));
        }
        out.push(Token { text, line: start_line });
    }
    Lexed { tokens: out, comments, problems }
}

/// If a character literal starts at `i`, return the index just past it.
fn is_char_literal(chars: &[char], i: usize) -> Option<usize> {
    let n = chars.len();
    if i + 2 < n && chars[i + 1] != '\\' && chars[i + 1] != '\'' && chars[i + 2] == '\'' {
        return Some(i + 3);
    }
    if i + 3 < n && chars[i + 1] == '\\' {
        // '\n', '\'', '\x41', '\u{1F600}'
        let mut j = i + 3;
        while j < n && j < i + 14 && chars[j] != '\'' && chars[j] != '\n' {
            j += 1;
        }
        if j < n && chars[j] == '\'' {
            return Some(j + 1);
        }
    }
    None
}

/// Source with comments removed, tokens on a line joined by single spaces,
/// lines preserved. Used for pattern rules so they never match inside comments.
pub fn stripped(tokens: &[Token]) -> String {
    let mut out = String::new();
    let mut line = 1;
    let mut first = true;
    for t in tokens {
        while line < t.line {
            out.push('\n');
            line += 1;
            first = true;
        }
        if !first {
            out.push(' ');
        }
        out.push_str(&t.text);
        line += t.text.matches('\n').count();
        first = false;
    }
    out
}

/// Number of distinct lines that carry at least one token.
pub fn code_lines(tokens: &[Token]) -> usize {
    let mut count = 0;
    let mut last = 0;
    for t in tokens {
        if t.line != last {
            count += 1;
            last = t.line;
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(src: &str) -> Vec<String> {
        tokenize(src).into_iter().map(|t| t.text).collect()
    }

    #[test]
    fn comments_are_dropped() {
        assert_eq!(texts("a -- b c\nd"), ["a", "d"]);
        assert_eq!(texts("a /- x /- nested -/ y -/ b"), ["a", "b"]);
        assert_eq!(texts("/-- doc -/\ntheorem t : True := trivial"), ["theorem", "t", ":", "True", ":=", "trivial"]);
    }

    #[test]
    fn comments_are_returned_beside_the_tokens() {
        let src = "a -- b c\n/-- doc\n  more -/ d \"x -- y\"\n/- open";
        let (tokens, comments) = scan(src);
        assert_eq!(tokens, tokenize(src));
        let got: Vec<(usize, &str)> = comments.iter().map(|c| (c.line, c.text.as_str())).collect();
        assert_eq!(got, [(1, "-- b c"), (2, "/-- doc\n  more -/"), (4, "/- open")]);
    }

    /// Each case was compiled with Lean 4 to see what it reads.
    #[test]
    fn raw_strings_do_not_take_a_backslash_for_an_escape() {
        // The reported case: with `\"` read as an escape, the string ran on to
        // the next quote and the code between was one token.
        let src = "def qa := r\"\\\"\ntheorem big : a = c := by\n  grind\ndef qb := \"\\\\\"\ntheorem last : True := trivial\n";
        assert_eq!(
            texts(src),
            ["def", "qa", ":=", "r\"\\\"", "theorem", "big", ":", "a", "=", "c", ":=", "by", "grind", "def", "qb", ":=", "\"\\\\\"", "theorem", "last", ":", "True", ":=", "trivial"]
        );
        assert!(problems(src).is_empty());
        assert_eq!(texts(r###"x r#"a "q" \ b"# y r##"a "# c"## z"###), ["x", "r#\"a \"q\" \\ b\"#", "y", "r##\"a \"# c\"##", "z"]);
        // `r` is an ordinary name when no quote follows at once, and a name
        // that ends in `r` starts no raw string.
        assert_eq!(texts(r##"r #"x"# r + 1"##), ["r", "#", "\"x\"", "#", "r", "+", "1"]);
        assert_eq!(texts(r#"bar"\"" z"#), ["bar", "\"\\\"\"", "z"]);
        assert!(tokenize(src).iter().filter(|t| is_string(t)).count() == 2);
    }

    #[test]
    fn interpolated_strings_have_code_in_their_holes() {
        // A hole may hold a quote character or a raw string; read as part of
        // one string, either would shift every quote after it.
        assert_eq!(texts(r#"s!"a {n} b" c"#), ["s!", "\"a {", "n", "} b\"", "c"]);
        assert_eq!(texts(r#"s!"a {'"'} b" grind "x""#), ["s!", "\"a {", "'\"'", "} b\"", "grind", "\"x\""]);
        assert_eq!(texts(r#"s!"a {r"\"} b" grind"#), ["s!", "\"a {", "r\"\\\"", "} b\"", "grind"]);
        assert_eq!(
            texts(r#"m!"x {s!"y {n}"} z {f {a := 1}} w" t"#),
            ["m!", "\"x {", "s!", "\"y {", "n", "}\"", "} z {", "f", "{", "a", ":=", "1", "}", "} w\"", "t"]
        );
        assert_eq!(texts(r#"s!"brace \{ no hole } {n}" t"#), ["s!", "\"brace \\{ no hole } {", "n", "}\"", "t"]);
        // Braces in a string that is not interpolated are text.
        assert_eq!(texts(r#"f "a {'"'} b" grind"#), ["f", "\"a {'\"", "'", "}", "b", "\" grind"]);
        let pieces = tokenize(r#"s!"a {n} b""#);
        assert!(is_string(&pieces[1]) && !is_string(&pieces[2]) && is_string(&pieces[3]));
    }

    #[test]
    fn character_literals_and_other_forms() {
        assert_eq!(texts(r#"'"' x '\'' y '\\' z '\x41' w"#), ["'\"'", "x", "'\\''", "y", "'\\\\'", "z", "'\\x41'", "w"]);
        // Straight after an operator, too.
        assert_eq!(texts(r#"c ='"' ∧ d"#), ["c", "=", "'\"'", "∧", "d"]);
        assert_eq!(texts("f '' s"), ["f", "''", "s"]);
        assert_eq!(texts("«weird -- name \"q» x"), ["«weird -- name \"q»", "x"]);
        assert_eq!(texts("\"\\x41\\u1F60\\n\\\\\\\"\" x"), ["\"\\x41\\u1F60\\n\\\\\\\"\"", "x"]);
        assert_eq!(texts("a /- c /- nested -/ still -/ + 1"), ["a", "+", "1"]);
    }

    #[test]
    fn what_may_be_read_differently_is_reported() {
        assert!(problems("def a := \"x\" -- c\n/- d -/ def b := r#\"y\"#\n").is_empty());
        assert_eq!(problems("def a := 1\ndef u := \"open\ndef b := 2\n"), [(2, Problem::OpenString), (2, Problem::StringOverLines)]);
        assert_eq!(problems("def a := 1\n\n/- open\ndef b := 2\n"), [(3, Problem::OpenComment)]);
        assert_eq!(problems("def g := \"abc\\\n  def\"\n"), [(1, Problem::StringOverLines)]);
        assert_eq!(problems("def g := r\"abc\ndef\"\n"), [(1, Problem::StringOverLines)]);
        assert_eq!(problems("def g := s!\"a {\n n} b\"\n"), []);
        assert_eq!(problems("def g := r##\"abc\"#\n"), [(1, Problem::OpenString), (1, Problem::StringOverLines)]);
        assert_eq!(problems("def «a\n\"b» := 1\ndef c := \"x\"\n"), [(1, Problem::NameOverLines)]);
        assert_eq!(texts("def «a\n\"b» := 1 c"), ["def", "«a\n\"b»", ":=", "1", "c"]);
        // A gap keeps the line count right for what follows.
        assert_eq!(tokenize("\"a\\\nb\" x\ny").iter().map(|t| t.line).collect::<Vec<_>>(), [1, 2, 3]);
    }

    fn reported(src: &str) -> Vec<String> {
        problems(src).iter().map(|(line, p)| format!("{line} {p:?}")).collect()
    }

    /// Files of a comparison with Lean 4.33.1 on which the tokens are exactly
    /// the leaves of Lean's syntax tree: the text, what Lean reads, and what
    /// `problems` reports. Where Lean stops at an error, so does the list.
    #[test]
    fn reads_what_lean_reads() {
        let cases: &[(&str, &str, &[&str], &[&str])] = &[
        // Block comments: the character after `/-` is no part of a marker, and what
        // follows `/--` and `/-!` is skipped as after any token.
        ("x_slash", "/-/- -/\ntheorem hidden : 1 + 1 = 2 := rfl\n-- -/\ndef g := \"q\"\ndef zz := 1\n", &["theorem", "hidden", ":", "1", "+", "1", "=", "2", ":=", "rfl", "def", "g", ":=", "\"q\"", "def", "zz", ":=", "1"], &[]),
        ("x_slash2", "def a := 1 /-/-/ + 2\ndef g := \"q\"\ndef zz := 1\n", &["def", "a", ":=", "1", "+", "2", "def", "g", ":=", "\"q\"", "def", "zz", ":=", "1"], &[]),
        ("x_slash3", "/-\n/- -/\ndef g := \"q\"\n-/\ndef zz := 1\n", &["def", "zz", ":=", "1"], &[]),
        ("d5", "/--/ hidden \" -/\ndef c := 1\ndef zz := 1\n", &["def", "c", ":=", "1", "def", "zz", ":=", "1"], &[]),
        ("d6", "/-!/ hidden \" -/\ndef c := 1\ndef zz := 1\n", &["def", "c", ":=", "1", "def", "zz", ":=", "1"], &[]),
        ("d7", "def c := 1 /-/ hidden \" -/ + 2\ndef zz := 1\n", &["def", "c", ":=", "1", "+", "2", "def", "zz", ":=", "1"], &[]),
        ("d2", "def b := 1 /- \" --/ + 2\ndef zz := 1\n", &["def", "b", ":=", "1", "+", "2", "def", "zz", ":=", "1"], &[]),
        ("d3", "def b := 1 /- \" -/ + 2\n/-- x --/\ndef c := 1\ndef zz := 1\n", &["def", "b", ":=", "1", "+", "2", "def", "c", ":=", "1", "def", "zz", ":=", "1"], &[]),
        ("cmt_slash3", "def a := 1 /-/ \" -/ + 2\ndef g := \"q\"\ndef zz := 1\n", &["def", "a", ":=", "1", "+", "2", "def", "g", ":=", "\"q\"", "def", "zz", ":=", "1"], &[]),
        ("cmt_empty_doc2", "/---/\ndef a := 1\ndef g := \"q\"\ndef zz := 1\n", &["def", "a", ":=", "1", "def", "g", ":=", "\"q\"", "def", "zz", ":=", "1"], &[]),
        ("e1", "/-- -- x -/\ndef c := 1\n-/\ndef d := 2\ndef zz := 1\n", &["def", "d", ":=", "2", "def", "zz", ":=", "1"], &[]),
        ("e2", "/-! -- x -/\ndef c := 1\n-/\ndef d := 2\ndef zz := 1\n", &["def", "d", ":=", "2", "def", "zz", ":=", "1"], &[]),
        ("e3", "/-- /- a -/ b -/\ndef c := 1\ndef zz := 1\n", &["def", "c", ":=", "1", "def", "zz", ":=", "1"], &[]),
        ("e4", "/--\n-- x -/\ndef c := 1\n-/\ndef d := 2\ndef zz := 1\n", &["def", "d", ":=", "2", "def", "zz", ":=", "1"], &[]),
        ("e5", "/-- x -- y -/\ndef c := 1\ndef zz := 1\n", &["def", "c", ":=", "1", "def", "zz", ":=", "1"], &[]),
        ("cmt_doc", "/-! module \" doc -/\n/-- doc \" don't -/\ndef a := 1\n/-- d /- nested \" -/ more \" -/\ndef b := 2\ndef g := \"q\"\ndef zz := 1\n", &["def", "a", ":=", "1", "def", "b", ":=", "2", "def", "g", ":=", "\"q\"", "def", "zz", ":=", "1"], &[]),
        ("cmt_block", "def a := 1 /- c \" -/ + 2 /- ' -/\n/- multi\n \" line -/\ndef g := \"q\"\ndef zz := 1\n", &["def", "a", ":=", "1", "+", "2", "def", "g", ":=", "\"q\"", "def", "zz", ":=", "1"], &[]),
        ("cmt_nested", "def a := 1 /- a /- b /- c \" -/ \" -/ \" -/ + 2\ndef g := \"q\"\ndef zz := 1\n", &["def", "a", ":=", "1", "+", "2", "def", "g", ":=", "\"q\"", "def", "zz", ":=", "1"], &[]),
        ("cmt_line_in_block", "def a := 1 /- x -- -/ + 2\n/- -- /- -/ \" -/\ndef g := \"q\"\ndef zz := 1\n", &["def", "a", ":=", "1", "+", "2", "def", "g", ":=", "\"q\"", "def", "zz", ":=", "1"], &[]),
        ("cmt_line", "def a := 1 -- c \"\ndef b := 2-- c '\ndef c := 3 --\n--\ndef g := \"q\"\ndef zz := 1\n", &["def", "a", ":=", "1", "def", "b", ":=", "2", "def", "c", ":=", "3", "def", "g", ":=", "\"q\"", "def", "zz", ":=", "1"], &[]),
        // The same where Lean reports a comment that is not closed.
        ("d1", "def a := 1 /------ \" ------/ + 2\ndef zz := 1\n", &["def", "a", ":=", "1"], &["1 OpenComment"]),
        ("d4", "/--- x -/\ndef c := 1\n/-!! y -/\n/-- -/\ndef d := 1\n/---/\ndef e := 1\n/----/\ndef f := 1\ndef zz := 1\n", &["def", "c", ":=", "1", "def", "d", ":=", "1", "def", "e", ":=", "1"], &["8 OpenComment"]),
        ("cmt_dashes", "def a := 1 /------ \" ------/ + 2\ndef b := 1 /- \" --/ + 2\ndef g := \"q\"\ndef zz := 1\n", &["def", "a", ":=", "1"], &["1 OpenComment"]),
        ("cmt_empty_doc", "/--/ def a := 1 /- -/\ndef g := \"q\"\ndef zz := 1\n", &[], &["1 OpenComment"]),
        // `//`, `\\/` and `<-` are tokens: the `-` after them starts no comment.
        ("x_subtype", "def junk := {x : Int //-1 < x}\ntheorem hidden1 : 1 + 1 = 2 := rfl\ntheorem hidden2 : 2 + 2 = 4 := rfl\n-- -/\ndef g := \"q\"\ndef zz := 1\n", &["def", "junk", ":=", "{", "x", ":", "Int", "//", "-", "1", "<", "x", "}", "theorem", "hidden1", ":", "1", "+", "1", "=", "2", ":=", "rfl", "theorem", "hidden2", ":", "2", "+", "2", "=", "4", ":=", "rfl", "def", "g", ":=", "\"q\"", "def", "zz", ":=", "1"], &[]),
        ("x_or", "def junk := True \\/-1 = (1 : Int)\ntheorem hidden1 : 1 + 1 = 2 := rfl\n-- -/\ndef g := \"q\"\ndef zz := 1\n", &["def", "junk", ":=", "True", "\\/", "-", "1", "=", "(", "1", ":", "Int", ")", "theorem", "hidden1", ":", "1", "+", "1", "=", "2", ":=", "rfl", "def", "g", ":=", "\"q\"", "def", "zz", ":=", "1"], &[]),
        ("x_larrow", "def a : IO Int := do\n  let y : Int <--(1 + 2 + 3 + 4 + 5 + 6 + 7 + 8 + 9)\n  return y\ndef zz := 1\n", &["def", "a", ":", "IO", "Int", ":=", "do", "let", "y", ":", "Int", "<-", "-", "(", "1", "+", "2", "+", "3", "+", "4", "+", "5", "+", "6", "+", "7", "+", "8", "+", "9", ")", "return", "y", "def", "zz", ":=", "1"], &[]),
        ("fix", "theorem t (p q : Prop) (h : p <-> q) : q <->p := h.symm\ndef a : Id Int := do\n  let y <--(1 : Int)\n  return y\ndef b := {x : Int //-1 < x}\ndef c := True \\/-1 = (1 : Int)\n", &["theorem", "t", "(", "p", "q", ":", "Prop", ")", "(", "h", ":", "p", "<->", "q", ")", ":", "q", "<->", "p", ":=", "h.symm", "def", "a", ":", "Id", "Int", ":=", "do", "let", "y", "<-", "-", "(", "1", ":", "Int", ")", "return", "y", "def", "b", ":=", "{", "x", ":", "Int", "//", "-", "1", "<", "x", "}", "def", "c", ":=", "True", "\\/", "-", "1", "=", "(", "1", ":", "Int", ")"], &[]),
        ("cmt_subtype", "def a := {x : Int //-1 < x}\ndef g := \"q\"\ndef zz := 1\n", &["def", "a", ":=", "{", "x", ":", "Int", "//", "-", "1", "<", "x", "}", "def", "g", ":=", "\"q\"", "def", "zz", ":=", "1"], &[]),
        ("cmt_or", "def a (p q : Prop) := p \\/-q = q\ndef g := \"q\"\ndef zz := 1\n", &["def", "a", "(", "p", "q", ":", "Prop", ")", ":=", "p", "\\/", "-", "q", "=", "q", "def", "g", ":=", "\"q\"", "def", "zz", ":=", "1"], &[]),
        ("cmt_larrow", "def a : IO Nat := do\n  let x : Int ← pure 1\n  let y : Int <--x |>.natAbs |> pure\n  return 0\ndef g := \"q\"\ndef zz := 1\n", &["def", "a", ":", "IO", "Nat", ":=", "do", "let", "x", ":", "Int", "←", "pure", "1", "let", "y", ":", "Int", "<-", "-", "x", "|>.", "natAbs", "|>", "pure", "return", "0", "def", "g", ":=", "\"q\"", "def", "zz", ":=", "1"], &[]),
        // Comments that do start straight after code.
        ("x_sep", "def a := 1 /- c -/+ 2\ndef b := (1 : Int)-- c\n  + 2\ndef c := (1 : Int) - -2 + (1 : Int)- -2\ndef zz := 1\n", &["def", "a", ":=", "1", "+", "2", "def", "b", ":=", "(", "1", ":", "Int", ")", "+", "2", "def", "c", ":=", "(", "1", ":", "Int", ")", "-", "-", "2", "+", "(", "1", ":", "Int", ")", "-", "-", "2", "def", "zz", ":=", "1"], &[]),
        ("cmt_tight", "def a (x : Nat) := x/-c \"-/+x--c \"\ndef g := \"q\"\ndef zz := 1\n", &["def", "a", "(", "x", ":", "Nat", ")", ":=", "x", "+", "x", "def", "g", ":=", "\"q\"", "def", "zz", ":=", "1"], &[]),
        ("cmt_after_op", "def a (x : Nat) := x +-- c \"\n  x */- \" -/ x\ndef g := \"q\"\ndef zz := 1\n", &["def", "a", "(", "x", ":", "Nat", ")", ":=", "x", "+", "x", "*", "x", "def", "g", ":=", "\"q\"", "def", "zz", ":=", "1"], &["1 GluedComment", "2 GluedComment"]),
        ("semi", "theorem t : True ∧ True := by constructor <;> trivial\ntheorem u : True := by\n  first | trivial | skip; done\ndef g := \"q\"\ndef zz := 1\n", &["theorem", "t", ":", "True", "∧", "True", ":=", "by", "constructor", "<;>", "trivial", "theorem", "u", ":", "True", ":=", "by", "first", "|", "trivial", "|", "skip", ";", "done", "def", "g", ":=", "\"q\"", "def", "zz", ":=", "1"], &[]),
        // Numbers.
        ("num2", "def a : List Float := [1.5e10, 0x10, 1e3, 3.0]\ndef b := (1, 2).1\ndef c := [0xff, 0XFF, 0b11, 0B11, 0o7, 0O7]\ndef g := \"q\"\ndef zz := 1\n", &["def", "a", ":", "List", "Float", ":=", "[", "1.5e10", ",", "0x10", ",", "1e3", ",", "3.0", "]", "def", "b", ":=", "(", "1", ",", "2", ")", ".", "1", "def", "c", ":=", "[", "0xff", ",", "0XFF", ",", "0b11", ",", "0B11", ",", "0o7", ",", "0O7", "]", "def", "g", ":=", "\"q\"", "def", "zz", ":=", "1"], &[]),
        ("where_dot", "def f (x : Nat × Nat) := x.1.succ + x.2 + (x).1 + x.fst\ndef g := \"q\"\ndef zz := 1\n", &["def", "f", "(", "x", ":", "Nat", "×", "Nat", ")", ":=", "x", ".", "1", ".", "succ", "+", "x", ".", "2", "+", "(", "x", ")", ".", "1", "+", "x.fst", "def", "g", ":=", "\"q\"", "def", "zz", ":=", "1"], &[]),
        // `λ` is not a letter.
        ("raw_after_lambda", "def f : String → Nat := λr\"\\\" => 1\ndef g := \"x\"\ndef zz := 1\n", &["def", "f", ":", "String", "→", "Nat", ":=", "λ", "r\"\\\"", "=>", "1", "def", "g", ":=", "\"x\"", "def", "zz", ":=", "1"], &[]),
        // Character literals and primed names.
        ("char_basic", "def a := ['\"', '\\'', '\\\\', '\\x41', '\\u03bb', '\\n', '\\t', 'a', ' ', '«', '-', '/', 'λ', '\\\"']\ndef g := \"q\"\ndef zz := 1\n", &["def", "a", ":=", "[", "'\"'", ",", "'\\''", ",", "'\\\\'", ",", "'\\x41'", ",", "'\\u03bb'", ",", "'\\n'", ",", "'\\t'", ",", "'a'", ",", "' '", ",", "'«'", ",", "'-'", ",", "'/'", ",", "'λ'", ",", "'\\\"'", "]", "def", "g", ":=", "\"q\"", "def", "zz", ":=", "1"], &[]),
        ("char_after_op", "def f (c : Char) := c='\"' ∨ c≠'\"' ∨ (c='\\\\')\ndef g := \"q\"\ndef zz := 1\n", &["def", "f", "(", "c", ":", "Char", ")", ":=", "c", "=", "'\"'", "∨", "c", "≠", "'\"'", "∨", "(", "c", "=", "'\\\\'", ")", "def", "g", ":=", "\"q\"", "def", "zz", ":=", "1"], &[]),
        ("char_dash", "def f := ['-','-'] ++ ['/','-'] ++ ['-','/']\ndef g := \"q\"\ndef zz := 1\n", &["def", "f", ":=", "[", "'-'", ",", "'-'", "]", "++", "[", "'/'", ",", "'-'", "]", "++", "[", "'-'", ",", "'/'", "]", "def", "g", ":=", "\"q\"", "def", "zz", ":=", "1"], &[]),
        ("char_newline", "def f := '\n'\ndef g := \"q\"\ndef zz := 1\n", &["def", "f", ":=", "'\n'", "def", "g", ":=", "\"q\"", "def", "zz", ":=", "1"], &[]),
        ("char_prime_ident_glue", "def f (x'a' : Nat) (y'' : Nat) := x'a' + y''\ndef g := \"q\"\ndef zz := 1\n", &["def", "f", "(", "x'a'", ":", "Nat", ")", "(", "y''", ":", "Nat", ")", ":=", "x'a'", "+", "y''", "def", "g", ":=", "\"q\"", "def", "zz", ":=", "1"], &[]),
        ("char_then_prime", "def f (c : Char) (h' : Bool) := c == 'a' && h' && c == '\\'' && h' && c == '\\\\' && h'\ndef g := \"q\"\ndef zz := 1\n", &["def", "f", "(", "c", ":", "Char", ")", "(", "h'", ":", "Bool", ")", ":=", "c", "==", "'a'", "&&", "h'", "&&", "c", "==", "'\\''", "&&", "h'", "&&", "c", "==", "'\\\\'", "&&", "h'", "def", "g", ":=", "\"q\"", "def", "zz", ":=", "1"], &[]),
        ("tick_char", "def f (x' y' : Nat) := (x', y') = (y', x') ∧ x'=y' ∧ (x')=(y')\ndef g := \"q\"\ndef zz := 1\n", &["def", "f", "(", "x'", "y'", ":", "Nat", ")", ":=", "(", "x'", ",", "y'", ")", "=", "(", "y'", ",", "x'", ")", "∧", "x'", "=", "y'", "∧", "(", "x'", ")", "=", "(", "y'", ")", "def", "g", ":=", "\"q\"", "def", "zz", ":=", "1"], &[]),
        ("paren_prime", "def f (x' y' : Nat) := [x',y',x']\ndef g := \"q\"\ndef zz := 1\n", &["def", "f", "(", "x'", "y'", ":", "Nat", ")", ":=", "[", "x'", ",", "y'", ",", "x'", "]", "def", "g", ":=", "\"q\"", "def", "zz", ":=", "1"], &[]),
        // Strings: raw, interpolated, and beside names.
        ("raw0", "def a := r\"a\\\"\ndef b := \"\\\\\"\ndef c := r\"\" ++ r\"\\n\"\ndef zz := 1\n", &["def", "a", ":=", "r\"a\\\"", "def", "b", ":=", "\"\\\\\"", "def", "c", ":=", "r\"\"", "++", "r\"\\n\"", "def", "zz", ":=", "1"], &[]),
        ("raw1", "def a := r#\"a \"q\" \\\"#\ndef b := r#\"\"\"#\ndef c := r#\"\\\"#\ndef zz := 1\n", &["def", "a", ":=", "r#\"a \"q\" \\\"#", "def", "b", ":=", "r#\"\"\"#", "def", "c", ":=", "r#\"\\\"#", "def", "zz", ":=", "1"], &[]),
        ("raw3", "def a := r###\"a \"## b\"# \" \"\"## \"###\ndef b := r##\"#\"#\"##\ndef zz := 1\n", &["def", "a", ":=", "r###\"a \"## b\"# \" \"\"## \"###", "def", "b", ":=", "r##\"#\"#\"##", "def", "zz", ":=", "1"], &[]),
        ("raw_after_paren_op", "def f := (r\"\\\")++r\"\\\"\ndef g := [r\"\\\",r#\"\\\"#]\ndef zz := 1\n", &["def", "f", ":=", "(", "r\"\\\"", ")", "++", "r\"\\\"", "def", "g", ":=", "[", "r\"\\\"", ",", "r#\"\\\"#", "]", "def", "zz", ":=", "1"], &[]),
        ("x_notation_glue", "notation \"⟪\"xr\"\\\"\"y\"⟫\" => xr + y\nnotation \"⟦\"«x»r\"\\\"y\"⟧\" => «x» + y\ndef g := \"q\"\ndef zz := 1\n", &["notation", "\"⟪\"", "xr", "\"\\\"\"", "y", "\"⟫\"", "=>", "xr", "+", "y", "notation", "\"⟦\"", "«x»", "r\"\\\"", "y", "\"⟧\"", "=>", "«x»", "+", "y", "def", "g", ":=", "\"q\"", "def", "zz", ":=", "1"], &[]),
        ("str_escapes", "def a := \"\\\\ \\\" \\' \\n \\t \\r \\x41 \\u03bb end\"\ndef b := \"\\\\\"\ndef c := \"\\\"\" ++ \"x\"\ndef zz := 1\n", &["def", "a", ":=", "\"\\\\ \\\" \\' \\n \\t \\r \\x41 \\u03bb end\"", "def", "b", ":=", "\"\\\\\"", "def", "c", ":=", "\"\\\"\"", "++", "\"x\"", "def", "zz", ":=", "1"], &[]),
        ("str_comment_markers", "def a := \"-- x\" ++ \"/- y\" ++ \"-/\" ++ \"/-- z\"\ndef b := \"q\" -- \"c\n\ndef zz := 1\n", &["def", "a", ":=", "\"-- x\"", "++", "\"/- y\"", "++", "\"-/\"", "++", "\"/-- z\"", "def", "b", ":=", "\"q\"", "def", "zz", ":=", "1"], &[]),
        ("interp_basic", "def f (n : Nat) := s!\"a {n} b\" ++ f!\"a {n} b\" ++ toString (m!\"a {n} b\" : Lean.MessageData) ++ s!\"\" ++ s!\"{n}\" ++ s!\"{n}{n}\"\ndef zz := 1\n", &["def", "f", "(", "n", ":", "Nat", ")", ":=", "s!", "\"a {", "n", "} b\"", "++", "f!", "\"a {", "n", "} b\"", "++", "toString", "(", "m!", "\"a {", "n", "} b\"", ":", "Lean.MessageData", ")", "++", "s!", "\"\"", "++", "s!", "\"{", "n", "}\"", "++", "s!", "\"{", "n", "}{", "n", "}\"", "def", "zz", ":=", "1"], &[]),
        ("interp_braces", "structure P where\n  a : Nat\ndef f (g : P → Nat) := s!\"x {g {a := 1}} y {({a := 2} : P).a} z\"\ndef zz := 1\n", &["structure", "P", "where", "a", ":", "Nat", "def", "f", "(", "g", ":", "P", "→", "Nat", ")", ":=", "s!", "\"x {", "g", "{", "a", ":=", "1", "}", "} y {", "(", "{", "a", ":=", "2", "}", ":", "P", ")", ".", "a", "} z\"", "def", "zz", ":=", "1"], &[]),
        ("interp_chars", "def f := s!\"x {'\"'} y {'}'} z {'{'} w {'\\''} v\"\ndef g := \"q\"\ndef zz := 1\n", &["def", "f", ":=", "s!", "\"x {", "'\"'", "} y {", "'}'", "} z {", "'{'", "} w {", "'\\''", "} v\"", "def", "g", ":=", "\"q\"", "def", "zz", ":=", "1"], &[]),
        ("interp_comments", "def f (n : Nat) := s!\"x {n /- \" } -/} y {n -- \" }\n} z\"\ndef g := \"q\"\ndef zz := 1\n", &["def", "f", "(", "n", ":", "Nat", ")", ":=", "s!", "\"x {", "n", "} y {", "n", "} z\"", "def", "g", ":=", "\"q\"", "def", "zz", ":=", "1"], &[]),
        ("interp_escaped_brace", "def f (n : Nat) := s!\"brace \\{ no hole } {n} \\\" {n}\"\ndef g := \"q\"\ndef zz := 1\n", &["def", "f", "(", "n", ":", "Nat", ")", ":=", "s!", "\"brace \\{ no hole } {", "n", "} \\\" {", "n", "}\"", "def", "g", ":=", "\"q\"", "def", "zz", ":=", "1"], &[]),
        ("interp_nested", "def f (n : Nat) := s!\"x {s!\"y {s!\"z {n}\"} w\"} v\" ++ m!\"x {s!\"y {n}\"} z {f {a := 1}} w\"\ndef zz := 1\n", &["def", "f", "(", "n", ":", "Nat", ")", ":=", "s!", "\"x {", "s!", "\"y {", "s!", "\"z {", "n", "}\"", "} w\"", "} v\"", "++", "m!", "\"x {", "s!", "\"y {", "n", "}\"", "} z {", "f", "{", "a", ":=", "1", "}", "} w\"", "def", "zz", ":=", "1"], &[]),
        ("interp_raw_in_hole", "def f := s!\"a {r\"\\\"} b {r#\"}\"#} c\"\ndef g := \"q\"\ndef zz := 1\n", &["def", "f", ":=", "s!", "\"a {", "r\"\\\"", "} b {", "r#\"}\"#", "} c\"", "def", "g", ":=", "\"q\"", "def", "zz", ":=", "1"], &[]),
        ("interp_space", "def f (n : Nat) := s! \"a {n} {'\"'} b\"\ndef g := \"q\"\ndef zz := 1\n", &["def", "f", "(", "n", ":", "Nat", ")", ":=", "s!", "\"a {", "n", "} {", "'\"'", "} b\"", "def", "g", ":=", "\"q\"", "def", "zz", ":=", "1"], &[]),
        ("interp_struct_end", "structure P where\n  a : Nat\ndef f := s!\"{({a := 1} : P).a}{({a := 1} : P).a}\" ++ s!\"{(⟨1⟩ : P).a}\"\ndef g := (fun x : Nat => s!\"{x}\") 1 |> fun s => s!\"{s}}\" ++ \"}\"\ndef zz := 1\n", &["structure", "P", "where", "a", ":", "Nat", "def", "f", ":=", "s!", "\"{", "(", "{", "a", ":=", "1", "}", ":", "P", ")", ".", "a", "}{", "(", "{", "a", ":=", "1", "}", ":", "P", ")", ".", "a", "}\"", "++", "s!", "\"{", "(", "⟨", "1", "⟩", ":", "P", ")", ".", "a", "}\"", "def", "g", ":=", "(", "fun", "x", ":", "Nat", "=>", "s!", "\"{", "x", "}\"", ")", "1", "|>", "fun", "s", "=>", "s!", "\"{", "s", "}}\"", "++", "\"}\"", "def", "zz", ":=", "1"], &[]),
        ];
        for (name, src, want, problems) in cases {
            assert_eq!(texts(src), *want, "{name}");
            assert_eq!(reported(src), *problems, "{name}");
        }
    }

    /// Each case was compiled with Lean 4.33.1 to see what it reads.
    #[test]
    fn block_comments_end_where_lean_ends_them() {
        // `/-/` opens a comment and closes none; once inside, `/-` nests at once.
        assert_eq!(texts("a /-/-/ b"), ["a", "b"]);
        assert_eq!(texts("a /-/ x -/ b"), ["a", "b"]);
        assert_eq!(texts("a /- /-/ x -/ y -/ b"), ["a", "b"]);
        assert_eq!(texts("def c := 1 /-/-/ hidden \" -/ -/ + 2\n"), ["def", "c", ":=", "1", "hidden", "\" -/ -/ + 2\n"]);
        // After `/--` a line comment hides a `-/` on its line, and a plain
        // block comment is read on its own, with its own first character.
        assert_eq!(texts("/-- -- x -/\na\n-/ b"), ["b"]);
        assert_eq!(texts("/-- /-/-/ x -/ b"), ["b"]);
        assert_eq!(texts("/-- /- a -/ -- b -/\nc\n-/ d"), ["d"]);
        assert_eq!(texts("/-! /- a -/ /- b -/ -- x -/\n c -/ d"), ["d"]);
        // A line comment later in a doc comment hides nothing.
        assert_eq!(texts("/-- x -- y -/ b"), ["b"]);
        // `/---/` is closed; in `/----/` the last four characters are a line comment.
        assert_eq!(texts("/---/ a"), ["a"]);
        assert_eq!(texts("/----/ a\n-/ b"), ["b"]);
        // `/--/` and `/-!/` are open.
        assert_eq!(reported("/--/ a"), ["1 OpenComment"]);
        assert_eq!(reported("/-!/ a"), ["1 OpenComment"]);
        assert_eq!(reported("/-- /- a"), ["1 OpenComment"]);
        // Lines are counted through all of it.
        let (tokens, comments) = scan("/-- -- x -/\n/- y\n-/ z -/\na /-/\n-/ b");
        assert_eq!(tokens.iter().map(|t| (t.line, t.text.as_str())).collect::<Vec<_>>(), [(4, "a"), (5, "b")]);
        assert_eq!(comments.iter().map(|c| (c.line, c.text.as_str())).collect::<Vec<_>>(), [(1, "/-- -- x -/\n/- y\n-/ z -/"), (4, "/-/\n-/")]);
    }

    /// Each case was compiled with Lean 4.33.1 to see what it reads.
    #[test]
    fn a_minus_after_a_token_of_lean_starts_no_comment() {
        assert_eq!(texts("{x : Int //-1 < x}"), ["{", "x", ":", "Int", "//", "-", "1", "<", "x", "}"]);
        assert_eq!(texts("True \\/-1 = (1 : Int)"), ["True", "\\/", "-", "1", "=", "(", "1", ":", "Int", ")"]);
        assert_eq!(texts("let y <--(1 : Int)"), ["let", "y", "<-", "-", "(", "1", ":", "Int", ")"]);
        assert_eq!(texts("q <->p"), ["q", "<->", "p"]);
        assert!(problems("{x : Int //-1 < x} \\/-1 <--1 <->p").is_empty());
        // Two of them after the token are a comment, for Lean too.
        assert_eq!(texts("let y <---(1 : Int)\n  z"), ["let", "y", "<-", "z"]);
        assert_eq!(texts("{x : Int //-- c\n  1 < x}"), ["{", "x", ":", "Int", "//", "1", "<", "x", "}"]);
        // Anything else before the marker is no token that holds its first character.
        assert_eq!(texts("x +-- c\n  x */- c -/ x =/- c -/ x"), ["x", "+", "x", "*", "x", "=", "x"]);
        assert_eq!(texts("1 /- c -/+ 2 (1 : Int)-- c\n - -2"), ["1", "+", "2", "(", "1", ":", "Int", ")", "-", "-", "2"]);
    }

    #[test]
    fn a_comment_straight_after_an_operator_is_reported() {
        // With a notation `+-`, Lean would read `+-`, `-`, `c` in the first.
        for src in ["a +-- c\nb", "a */- c -/ b", "a :=/- c -/ b", "f ∘-- c\ng", "y <---1\nz", "{x : Int //-- c\n1 < x}"] {
            assert_eq!(reported(src), ["1 GluedComment"], "{src}");
        }
        // Lean reads `//` and a comment here; the pair is not looked for
        // after other characters, so what is read is reported.
        assert_eq!(texts("{x : Int ///- c -/ 1 < x}"), ["{", "x", ":", "Int", "///", "-", "c", "-/", "1", "<", "x", "}"]);
        assert_eq!(reported("{x : Int ///- c -/ 1 < x}"), ["1 GluedComment"]);
        // After a name, a number, a bracket, a string or another comment, and
        // after a space, nothing is.
        for src in ["a-- c\nb", "a'-- c\nb", "1/- c -/ + b", "(a)-- c\nb", "f (a)/- c -/ b", "\"s\"-- c", "a + -- c\nb", "a /- c -//- d -/-- e\nb"] {
            assert_eq!(reported(src), [] as [&str; 0], "{src}");
        }
    }

    /// What Lean reads in each was seen with Lean 4.33.1: the holes are code.
    #[test]
    fn a_brace_in_a_plain_string_is_reported() {
        for (src, line) in [
            ("open Lean in\ndef f (n : Nat) : MetaM Unit := throwError \"a {n} b\"\n", 2),
            ("open Lean in\ndef f (n : Nat) : MetaM Unit := throwError \"{n + 1 + 2 + 3 + 4 + 5}\"\n", 2),
            ("def f (n : Nat) : IO Unit := println! \"a {n} b\"\n", 1),
            ("def f (n : Nat) : Nat := dbg_trace \"{n + 1 + 2 + 3 + 4 + 5}\"; n\n", 1),
            ("def f (n : Nat) : MetaM Unit := do\n  trace[Meta.debug] \"a {n} b\"\n", 2),
            ("macro \"p!\" s:interpolatedStr(term) : term => pure ⟨s.raw[1]⟩\ntheorem u (a b : Nat) : a + b = b + a := p!\"{by omega}\"\n", 2),
            // Lean reads these as one string each, which cannot be told from the others.
            ("def f (n : Nat) : Nat := panic! \"a {n} b\"\n", 1),
            ("def a := \"{\"\n", 1),
        ] {
            assert_eq!(problems(src), [(line, Problem::BraceInString)], "{src}");
        }
        // A quote in a hole: the string ends there for a plain string and
        // goes on for Lean, so the rest of the file is read differently.
        let src = "def f (n : Nat) : IO Unit := println! \"a {n} {'\"'} b\"\ndef g := \"q\"\n";
        assert_eq!(problems(src)[0], (1, Problem::BraceInString));
        // Interpolated by `s!`, `f!` or `m!`, raw, escaped, or a closing brace only.
        for src in ["s!\"a {n} b\"", "f!\"{n}\" ++ m!\"x {n}\"", "s! \"a {n}\"", "r\"a { b\"", "r#\"{\"#", "s!\"\\{ {n}\"", "\"a } b\"", "'{'"] {
            assert!(problems(src).is_empty(), "{src}");
        }
        // The pieces of an interpolated string are not plain strings, the
        // strings in its holes are.
        assert!(problems("s!\"a {f {x := 1}} b {n} c\"").is_empty());
        assert_eq!(problems("s!\"a {\"{\"} b\""), [(1, Problem::BraceInString)]);
    }

    /// Each form was compiled with Lean 4.33.1 to see what it reads.
    #[test]
    fn numbers_are_read_as_lean_reads_them() {
        let one = ["1.5e-3", "1e-5", "1.5e+3", "1.5E-3", "1E5", "1e10", "1e1_0", "2.e3", "1_0.2_5", "1.5_0", "1_000_000", "1__0", "0xFF_FF", "0x_F", "0XaB", "0b1010", "0b_1", "0B11", "0o17", "0O7", "00", "12"];
        assert_eq!(texts(&one.join(" ")), one);
        assert_eq!(texts(&one.join(",")), one.join(" , ").split(' ').collect::<Vec<_>>());
        // `2.` is a number where no name and no digit follows.
        assert_eq!(texts("[2., 1.] 2. x 2.+1 a[:2.]"), ["[", "2.", ",", "1.", "]", "2.", "x", "2.", "+", "1", "a", "[", ":", "2.", "]"]);
        assert_eq!(texts("[1...5] 1..2"), ["[", "1", "...", "5", "]", "1", "..", "2"]);
        // Lean refuses `2.succ`; a field of a name or of a bracket is as before.
        assert_eq!(texts("2.succ (2).succ h.1 x.2.symm (1, 2).1 (p).1.succ"), ["2", ".", "succ", "(", "2", ")", ".", "succ", "h", ".", "1", "x", ".", "2", ".", "symm", "(", "1", ",", "2", ")", ".", "1", "(", "p", ")", ".", "1", ".", "succ"]);
        // The exception that is kept: Lean reads `x`, `.`, `2`, `.`, `1`.
        assert_eq!(texts("x.2.1 + p.1.2.symm"), ["x", ".", "2.1", "+", "p", ".", "1.2", ".", "symm"]);
        // A number ends where its digits end: what follows is a token of its own.
        assert_eq!(texts("f 1x 0xFFg 0b12 1.5e-3x 2then"), ["f", "1", "x", "0xFF", "g", "0b1", "2", "1.5e-3", "x", "2", "then"]);
        // Where Lean reports an error, the number ends before it.
        assert_eq!(texts("1_x 1_ 1e 1e+ 1.0e 0x 0b2 1._5"), ["1", "_x", "1", "_", "1", "e", "1", "e", "+", "1.0", "e", "0", "x", "0", "b2", "1", ".", "_5"]);
        // The reported case: the `r` of a raw string is not a digit of the number.
        let src = "def f := 1r\"\\\"\ndef g := \"x\"\n";
        assert_eq!(texts(src), ["def", "f", ":=", "1", "r\"\\\"", "def", "g", ":=", "\"x\""]);
        assert!(problems(src).is_empty());
        assert_eq!(
            texts("notation \"⟪\" x:1r\"\\\" y \"⟫\" => x + y + 0 -- \"\ndef h := 1"),
            ["notation", "\"⟪\"", "x", ":", "1", "r\"\\\"", "y", "\"⟫\"", "=>", "x", "+", "y", "+", "0", "def", "h", ":=", "1"]
        );
        // The file of the comparison, in which only `.2.1` is not as in Lean.
        assert_eq!(
            texts("def a := [1_000_000, 0xFF_FF, 0b1010, 0o17, 0XaB, 12]\ndef b : List Float := [1.5, 1e10, 1.5e-3, 1E5, 2., 1e-5, 1.5e+3, 1_0.2_5]\ndef c := (1, 2).1 + (1, (2, 3)).2.1\n"),
            ["def", "a", ":=", "[", "1_000_000", ",", "0xFF_FF", ",", "0b1010", ",", "0o17", ",", "0XaB", ",", "12", "]", "def", "b", ":", "List", "Float", ":=", "[", "1.5", ",", "1e10", ",", "1.5e-3", ",", "1E5", ",", "2.", ",", "1e-5", ",", "1.5e+3", ",", "1_0.2_5", "]", "def", "c", ":=", "(", "1", ",", "2", ")", ".", "1", "+", "(", "1", ",", "(", "2", ",", "3", ")", ")", ".", "2.1"]
        );
    }

    /// Each form was compiled with Lean 4.33.1 to see what it reads.
    #[test]
    fn the_binder_letters_are_tokens_and_no_letters() {
        assert_eq!(texts("λx => x"), ["λ", "x", "=>", "x"]);
        assert_eq!(texts("λ x => x"), ["λ", "x", "=>", "x"]);
        assert_eq!(texts("Σx : Nat, Fin x"), ["Σ", "x", ":", "Nat", ",", "Fin", "x"]);
        assert_eq!(texts("Πa, f a"), ["Π", "a", ",", "f", "a"]);
        // `Σ'` is one token, also before a name that ends in a prime.
        assert_eq!(texts("Σ' i : Nat, i = i"), ["Σ'", "i", ":", "Nat", ",", "i", "=", "i"]);
        assert_eq!(texts("Σ'i' : Nat, i' = i'"), ["Σ'", "i'", ":", "Nat", ",", "i'", "=", "i'"]);
        // Inside a name they end it (Lean then reports an error).
        assert_eq!(texts("xλ aΣ h.Π"), ["x", "λ", "a", "Σ", "h", ".", "Π"]);
        // The other Greek letters are letters.
        assert_eq!(texts("α + β.π + σ' + Λ₁ + μλ"), ["α", "+", "β.π", "+", "σ'", "+", "Λ₁", "+", "μ", "λ"]);
        // The reported case: a raw string after `λ`, and the code after it.
        let src = "def f : String → Nat := λr\"\\\" => 1 + 2 + 3 + 4 + 5 + 6 | _ => 0 -- \"\ndef g := \"q\"\n";
        assert_eq!(
            texts(src),
            ["def", "f", ":", "String", "→", "Nat", ":=", "λ", "r\"\\\"", "=>", "1", "+", "2", "+", "3", "+", "4", "+", "5", "+", "6", "|", "_", "=>", "0", "def", "g", ":=", "\"q\""]
        );
        assert!(problems(src).is_empty());
    }

    /// What Lean reads in each was seen with Lean 4.33.1 and Mathlib. Lean
    /// has the tokens `''`, `∑'`, `]'`, `⁻¹'`, `×'`; here the prime is a token
    /// of its own, as it is before a name without a prime.
    #[test]
    fn a_prime_that_ends_a_token_starts_no_character_literal() {
        assert_eq!(texts("f ''s' ∪ f ⁻¹'s'"), ["f", "''", "s'", "∪", "f", "⁻", "¹", "'", "s'"]);
        assert_eq!(texts("f '' s' ∪ f ⁻¹' s'"), ["f", "''", "s'", "∪", "f", "⁻", "¹", "'", "s'"]);
        assert_eq!(texts("∑' i, f i + ∑'i', f i' + ∏'j', g"), ["∑", "'", "i", ",", "f", "i", "+", "∑", "'", "i'", ",", "f", "i'", "+", "∏", "'", "j'", ",", "g"]);
        assert_eq!(texts("a[i]'h' + a[i]'h + a[i]'(by simp)"), ["a", "[", "i", "]", "'", "h'", "+", "a", "[", "i", "]", "'", "h", "+", "a", "[", "i", "]", "'", "(", "by", "simp", ")"]);
        assert_eq!(texts("Nat ×'p' = p'"), ["Nat", "×", "'", "p'", "=", "p'"]);
        // Character literals after a bracket, a comma or an operator are literals.
        assert_eq!(texts("['a','b'] ('c') x='d' ≠'e' #['f']"), ["[", "'a'", ",", "'b'", "]", "(", "'c'", ")", "x", "=", "'d'", "≠", "'e'", "#", "[", "'f'", "]"]);
        // A line break in a character literal is counted.
        assert_eq!(tokenize("def f := '\n'\ndef g := 1\n").iter().map(|t| t.line).collect::<Vec<_>>(), [1, 1, 1, 1, 3, 3, 3, 3]);
    }

    #[test]
    fn comment_markers_inside_strings_are_kept() {
        assert_eq!(texts(r#"x "a -- b" y"#), ["x", "\"a -- b\"", "y"]);
        assert_eq!(texts(r#""a \" /- b" z"#), ["\"a \\\" /- b\"", "z"]);
    }

    #[test]
    fn identifiers_and_names() {
        assert_eq!(texts("Foo.bar_baz' h.1 x₀ simp!"), ["Foo.bar_baz'", "h", ".", "1", "x₀", "simp!"]);
        assert_eq!(texts("«weird name».x"), ["«weird name».x"]);
    }

    #[test]
    fn operators_and_brackets() {
        assert_eq!(texts("f(x):=⟨a,b⟩"), ["f", "(", "x", ")", ":=", "⟨", "a", ",", "b", "⟩"]);
        assert_eq!(texts("∀ x ≤ y, x <;> simp"), ["∀", "x", "≤", "y", ",", "x", "<;>", "simp"]);
        assert_eq!(texts("a +-- c\nb"), ["a", "+", "b"]);
    }

    #[test]
    fn numbers_and_chars() {
        assert_eq!(texts("1.5 0xFF 12"), ["1.5", "0xFF", "12"]);
        assert_eq!(texts("'a' '\\n' x'"), ["'a'", "'\\n'", "x'"]);
    }

    #[test]
    fn reformatting_does_not_change_count() {
        let a = "theorem t (a b : Nat) : a + b = b + a := by\n  omega\n";
        let b = "theorem t (a b : Nat) :\n    a + b = b + a :=\n  by omega -- done";
        assert_eq!(tokenize(a).len(), tokenize(b).len());
    }

    #[test]
    fn code_lines_ignore_blank_and_comment_lines() {
        let t = tokenize("a\n\n-- c\n/- x\n y -/\nb c\n");
        assert_eq!(code_lines(&t), 2);
        assert_eq!(stripped(&t), "a\n\n\n\n\nb c");
    }
}
