//! JSON with comments, made into JSON.
//!
//! `deno.jsonc` is the only manifest here written in it, and `serde_json`
//! rejects both of the things the format adds. Stripping them is enough:
//! nothing here has to *write* the file back, so the comments are not being
//! lost, only ignored.

/// Strip comments and trailing commas so `serde_json` will read the text.
///
/// String literals are left alone, escapes included — a `//` inside a package
/// specifier is not the start of a comment.
pub fn to_json(text: &str) -> String {
    drop_trailing_commas(&drop_comments(text))
}

fn drop_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;

    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        // Kept, so the rest of the document keeps its lines.
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut previous = '\0';
                for c in chars.by_ref() {
                    if previous == '*' && c == '/' {
                        break;
                    }
                    previous = c;
                }
            }
            _ => out.push(c),
        }
    }

    out
}

/// Drop each comma that closes its own collection. Runs after comments, so the
/// only thing between a comma and the bracket after it is whitespace.
fn drop_trailing_commas(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0;
    let mut in_string = false;
    let mut escaped = false;

    while index < chars.len() {
        let c = chars[index];
        index += 1;

        if in_string {
            out.push(c);
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(c);
            continue;
        }
        if c == ','
            && chars[index..]
                .iter()
                .find(|next| !next.is_whitespace())
                .is_some_and(|next| *next == '}' || *next == ']')
        {
            continue;
        }
        out.push(c);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_json_is_left_exactly_as_it_was() {
        let json = "{\n  \"a\": [1, 2],\n  \"b\": \"c\"\n}";
        assert_eq!(to_json(json), json);
    }

    #[test]
    fn line_and_block_comments_are_removed() {
        assert_eq!(to_json("{} // trailing"), "{} ");
        assert_eq!(to_json("// leading\n{}"), "\n{}");
        assert_eq!(to_json("{ /* inline */ }"), "{  }");
        assert_eq!(to_json("/* one\n   two */{}"), "{}");
    }

    #[test]
    fn a_comment_marker_inside_a_string_is_part_of_the_string() {
        assert_eq!(
            to_json(r#"{"url": "https://x/y", "p": "a/*b*/c"}"#),
            r#"{"url": "https://x/y", "p": "a/*b*/c"}"#
        );
    }

    #[test]
    fn an_escaped_quote_does_not_end_a_string() {
        assert_eq!(to_json(r#"{"a": "b\"// c"}"#), r#"{"a": "b\"// c"}"#);
    }

    #[test]
    fn trailing_commas_are_dropped_from_both_kinds_of_collection() {
        assert_eq!(to_json("{\"a\": 1,\n}"), "{\"a\": 1\n}");
        assert_eq!(to_json("[1, 2, ]"), "[1, 2 ]");
    }

    #[test]
    fn a_comma_between_entries_survives() {
        assert_eq!(to_json("[1, 2]"), "[1, 2]");
        assert_eq!(to_json(r#"{"a": ",", "b": 1}"#), r#"{"a": ",", "b": 1}"#);
    }

    #[test]
    fn a_comment_before_a_closing_bracket_still_leaves_the_comma_trailing() {
        assert_eq!(to_json("[\n  1, // last\n]"), "[\n  1 \n]");
    }
}
