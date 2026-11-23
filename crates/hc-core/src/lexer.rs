//! A small, deterministic tokenizer for Lean 4 source.
//!
//! It is not the Lean parser. It is a fixed, documented approximation whose
//! only job is to give every contributor the same number for the same text:
//!
//! * `--` line comments and nested `/- ... -/` block comments (including doc
//!   comments) are removed and never counted.
//! * A string literal is one token. A character literal is one token.
//! * An identifier is a run of letters, digits, `_`, `'`, `!`, `?`; a `.`
//!   followed by an identifier start continues it, so `Foo.bar` is one token.
//! * A number is one token (`12`, `1.5`, `0xFF`).
//! * Each bracket or comma `( ) [ ] { } ⟨ ⟩ ,` is one token.
//! * Any other run of ASCII operator characters is one token (`:=`, `=>`,
//!   `<;>`); every other non-space character is one token (`∀`, `≤`, `·`).

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub text: String,
    /// 1-based line on which the token starts.
    pub line: usize,
}

fn is_ident_start(c: char) -> bool {
    c == '_' || c.is_alphabetic()
}

fn is_ident_continue(c: char) -> bool {
    c == '_' || c == '\'' || c == '!' || c == '?' || c.is_alphanumeric()
        // Subscript digits and letters, common in Lean names such as `m₀`.
        || ('\u{2080}'..='\u{209C}').contains(&c)
}

fn is_single(c: char) -> bool {
    matches!(c, '(' | ')' | '[' | ']' | '{' | '}' | '⟨' | '⟩' | ',')
}

fn is_ascii_op(c: char) -> bool {
    c.is_ascii_punctuation() && !is_single(c) && c != '"' && c != '_'
}

/// Tokenize `src`, dropping comments and whitespace.
pub fn tokenize(src: &str) -> Vec<Token> {
    let chars: Vec<char> = src.chars().collect();
    let n = chars.len();
    let mut out = Vec::new();
    let mut i = 0;
    let mut line = 1;

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
        // Line comment.
        if c == '-' && i + 1 < n && chars[i + 1] == '-' {
            while i < n && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        // Block comment, nested.
        if c == '/' && i + 1 < n && chars[i + 1] == '-' {
            let mut depth = 1;
            i += 2;
            while i < n && depth > 0 {
                if chars[i] == '/' && i + 1 < n && chars[i + 1] == '-' {
                    depth += 1;
                    i += 2;
                } else if chars[i] == '-' && i + 1 < n && chars[i + 1] == '/' {
                    depth -= 1;
                    i += 2;
                } else {
                    if chars[i] == '\n' {
                        line += 1;
                    }
                    i += 1;
                }
            }
            continue;
        }
        let start = i;
        let start_line = line;
        // String literal.
        if c == '"' {
            i += 1;
            while i < n && chars[i] != '"' {
                if chars[i] == '\\' && i + 1 < n {
                    i += 1;
                }
                if chars[i] == '\n' {
                    line += 1;
                }
                i += 1;
            }
            i = (i + 1).min(n);
        } else if c == '\'' && is_char_literal(&chars, i).is_some() {
            i = is_char_literal(&chars, i).unwrap();
        } else if c.is_ascii_digit() {
            while i < n && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            if i + 1 < n && chars[i] == '.' && chars[i + 1].is_ascii_digit() {
                i += 1;
                while i < n && chars[i].is_ascii_digit() {
                    i += 1;
                }
            }
        } else if is_ident_start(c) || c == '«' {
            loop {
                if i < n && chars[i] == '«' {
                    while i < n && chars[i] != '»' && chars[i] != '\n' {
                        i += 1;
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
            i += 1;
        } else if is_ascii_op(c) {
            while i < n && is_ascii_op(chars[i]) {
                // Never swallow the start of a comment.
                if i > start
                    && i + 1 < n
                    && ((chars[i] == '-' && chars[i + 1] == '-')
                        || (chars[i] == '/' && chars[i + 1] == '-'))
                {
                    break;
                }
                i += 1;
            }
        } else {
            i += 1;
        }
        out.push(Token { text: chars[start..i].iter().collect(), line: start_line });
    }
    out
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
