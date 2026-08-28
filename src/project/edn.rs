//! Just enough EDN to read a `deps.edn`.
//!
//! Not a general reader: it keeps the shape of the collections and the text of
//! everything else, which is all a dependency listing needs. A form it does not
//! understand costs the listing that one entry rather than the file.

/// An EDN form, with every scalar left as the text it was written as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// Key/value pairs in the order they were written.
    Map(Vec<(Value, Value)>),
    /// A vector, list or set — which of the three it was is not something any
    /// caller here asks.
    Seq(Vec<Value>),
    /// A symbol, keyword, number, string or anything else atomic. A string
    /// keeps its quotes off but is otherwise untouched.
    Atom(String),
}

impl Value {
    /// The value `key` maps to, where this is a map with a key written exactly
    /// like that — `:deps`, `:mvn/version`.
    pub fn get(&self, key: &str) -> Option<&Value> {
        let Value::Map(entries) = self else {
            return None;
        };
        entries
            .iter()
            .find(|(k, _)| matches!(k, Value::Atom(name) if name == key))
            .map(|(_, value)| value)
    }

    /// This map's entries, or nothing when it is not a map.
    pub fn entries(&self) -> &[(Value, Value)] {
        match self {
            Value::Map(entries) => entries,
            _ => &[],
        }
    }

    /// The text of an atom.
    pub fn text(&self) -> Option<&str> {
        match self {
            Value::Atom(text) => Some(text),
            _ => None,
        }
    }
}

/// Read the first form in `text`, or nothing when there is none to read.
pub fn parse(text: &str) -> Option<Value> {
    Reader {
        chars: text.chars().collect(),
        index: 0,
    }
    .form()
}

struct Reader {
    chars: Vec<char>,
    index: usize,
}

impl Reader {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.index).copied()
    }

    fn next(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.index += 1;
        Some(c)
    }

    /// Whitespace, the commas EDN also counts as whitespace, and `;` comments.
    fn skip_blank(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_whitespace() || c == ',' {
                self.index += 1;
            } else if c == ';' {
                while self.peek().is_some_and(|c| c != '\n') {
                    self.index += 1;
                }
            } else {
                break;
            }
        }
    }

    fn form(&mut self) -> Option<Value> {
        self.skip_blank();
        match self.peek()? {
            '{' => self.collection('}').map(pairs),
            '[' => self.collection(']').map(Value::Seq),
            '(' => self.collection(')').map(Value::Seq),
            '"' => self.string(),
            '#' => self.dispatch(),
            '}' | ']' | ')' => None,
            _ => self.atom(),
        }
    }

    /// The forms up to `close`, which is consumed. An unclosed collection ends
    /// at the end of the text with what it had.
    fn collection(&mut self, close: char) -> Option<Vec<Value>> {
        self.index += 1;
        let mut items = Vec::new();
        loop {
            self.skip_blank();
            match self.peek() {
                None => return Some(items),
                Some(c) if c == close => {
                    self.index += 1;
                    return Some(items);
                }
                _ => {
                    let before = self.index;
                    match self.form() {
                        Some(value) => items.push(value),
                        // A discard (`#_`) that ran to the end of the
                        // collection: it read something, and contributes
                        // nothing.
                        None if self.index > before => {}
                        // A form the reader gave up on: step past one character
                        // so an unreadable entry cannot stall the whole file.
                        None => self.index += 1,
                    }
                }
            }
        }
    }

    /// `#{...}` is a set, `#_` discards the form after it, and any other tag is
    /// a label on the form that follows — which is the form the caller wants.
    fn dispatch(&mut self) -> Option<Value> {
        self.index += 1;
        match self.peek()? {
            '{' => self.collection('}').map(Value::Seq),
            '_' => {
                self.index += 1;
                self.form();
                self.form()
            }
            _ => {
                self.atom();
                self.form()
            }
        }
    }

    fn string(&mut self) -> Option<Value> {
        self.index += 1;
        let mut out = String::new();
        while let Some(c) = self.next() {
            match c {
                '"' => return Some(Value::Atom(out)),
                '\\' => out.push(self.next()?),
                _ => out.push(c),
            }
        }
        Some(Value::Atom(out))
    }

    /// Everything up to the next delimiter: a symbol, keyword, number, `nil`.
    /// A leading `\` takes the character after it with it, so `\,` is one atom.
    fn atom(&mut self) -> Option<Value> {
        let start = self.index;
        if self.peek() == Some('\\') {
            self.index = (self.index + 2).min(self.chars.len());
        }
        while let Some(c) = self.peek() {
            if c.is_whitespace() || matches!(c, ',' | '{' | '}' | '[' | ']' | '(' | ')' | '"' | ';')
            {
                break;
            }
            self.index += 1;
        }
        (self.index > start)
            .then(|| Value::Atom(self.chars[start..self.index].iter().collect::<String>()))
    }
}

/// Pair a map's forms up. An odd trailing form has no value and is dropped.
fn pairs(items: Vec<Value>) -> Value {
    let mut entries = Vec::with_capacity(items.len() / 2);
    let mut items = items.into_iter();
    while let (Some(key), Some(value)) = (items.next(), items.next()) {
        entries.push((key, value));
    }
    Value::Map(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atom(text: &str) -> Value {
        Value::Atom(text.to_string())
    }

    #[test]
    fn a_map_keeps_its_keys_in_order() {
        let value = parse("{:a 1 :b 2}").unwrap();
        assert_eq!(
            value.entries().to_vec(),
            vec![(atom(":a"), atom("1")), (atom(":b"), atom("2")),]
        );
    }

    #[test]
    fn a_key_is_looked_up_by_the_text_it_was_written_as() {
        let value = parse("{:deps {org/lib {:mvn/version \"1.2.3\"}}}").unwrap();
        let deps = value.get(":deps").unwrap();
        let coord = deps.get("org/lib").unwrap();

        assert_eq!(
            coord.get(":mvn/version").and_then(Value::text),
            Some("1.2.3")
        );
    }

    #[test]
    fn commas_and_comments_are_whitespace() {
        let value = parse("{:a 1, ; the first\n :b 2}").unwrap();
        assert_eq!(value.get(":b"), Some(&atom("2")));
    }

    #[test]
    fn nesting_survives_vectors_lists_and_sets() {
        let value = parse("{:paths [\"src\" \"test\"] :s #{1 2} :l (a b)}").unwrap();
        assert_eq!(
            value.get(":paths"),
            Some(&Value::Seq(vec![atom("src"), atom("test")]))
        );
        assert_eq!(
            value.get(":s"),
            Some(&Value::Seq(vec![atom("1"), atom("2")]))
        );
        assert_eq!(
            value.get(":l"),
            Some(&Value::Seq(vec![atom("a"), atom("b")]))
        );
    }

    #[test]
    fn a_discarded_form_contributes_nothing() {
        assert_eq!(
            parse("[1 #_2 3]"),
            Some(Value::Seq(vec![atom("1"), atom("3")]))
        );
    }

    #[test]
    fn a_discard_at_the_end_of_a_collection_does_not_eat_its_bracket() {
        let value = parse("{:deps {a {:mvn/version \"1\"} #_b #_{}} :paths [\"src\"]}").unwrap();
        assert!(value.get(":deps").unwrap().get("a").is_some());
        assert_eq!(value.get(":paths"), Some(&Value::Seq(vec![atom("src")])));
    }

    #[test]
    fn a_string_keeps_what_its_escapes_meant() {
        let value = parse(r#"{:a "one \"two\" three"}"#).unwrap();
        assert_eq!(
            value.get(":a").and_then(Value::text),
            Some(r#"one "two" three"#)
        );
    }

    #[test]
    fn a_tagged_form_reads_as_the_form_it_tags() {
        let value = parse("{:when #inst \"2026-01-01\"}").unwrap();
        assert_eq!(value.get(":when").and_then(Value::text), Some("2026-01-01"));
    }

    #[test]
    fn an_unclosed_map_gives_back_what_it_had() {
        let value = parse("{:deps {a {:mvn/version \"1\"}}").unwrap();
        assert!(value.get(":deps").is_some());
    }

    #[test]
    fn nothing_to_read_is_not_a_form() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("  ; only a comment\n"), None);
    }

    #[test]
    fn a_trailing_key_with_no_value_is_dropped() {
        let value = parse("{:a 1 :b}").unwrap();
        assert_eq!(value.entries().len(), 1);
    }

    #[test]
    fn lookups_on_something_that_is_not_a_map_find_nothing() {
        assert_eq!(atom("x").get(":a"), None);
        assert!(atom("x").entries().is_empty());
        assert_eq!(Value::Seq(vec![]).text(), None);
    }
}
