//! A page's front matter: a block of YAML at its very top, between two `---` lines, that says what the page is and
//! what only it can say, as its doctype, number, status and authors. It is not part of the page's text: it is not
//! rendered, nor a block, nor searched. Its dates and who wrote it are git's to say, not its. What its fields may
//! be is its doctype's schema's to say; see `schema.rs`.
//!
//! ```markdown
//! ---
//! doctype: design
//! number: 12
//! status: review
//! authors: [Ada, Grace]
//! ---
//! # Cache tiling
//! ```

use serde_json::{Map, Number, Value};
use yaml_rust2::{Yaml, YamlLoader};

/// A page's front matter: its fields, as JSON, in the order it gives them; or, when it is not YAML of fields that
/// git-kb reads, why.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Front {
    pub fields: Map<String, Value>,
    pub error: Option<String>,
}

impl Front {
    /// The front matter of this YAML: a mapping of fields, each any YAML value.
    #[must_use]
    pub fn parse(yaml: &str) -> Self {
        let docs = match YamlLoader::load_from_str(yaml) {
            Ok(docs) => docs,
            Err(e) => {
                return Self {
                    fields: Map::new(),
                    error: Some(e.to_string()),
                };
            }
        };
        match docs.into_iter().next().map(|doc| Self::json(&doc)) {
            Some(Value::Object(fields)) => Self {
                fields,
                error: None,
            },
            _ => Self {
                fields: Map::new(),
                error: Some("front matter is fields, as `status: draft`".into()),
            },
        }
    }

    /// A field, by its name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.fields.get(name)
    }

    /// A field that is text, by its name, unless it is empty.
    #[must_use]
    pub fn text(&self, name: &str) -> Option<&str> {
        self.get(name)?.as_str().filter(|t| !t.is_empty())
    }

    /// What the page is, by the id of a doctype in git-kb.toml.
    #[must_use]
    pub fn doctype(&self) -> Option<&str> {
        self.text("doctype")
    }

    /// The page's number among its doctype's.
    #[must_use]
    pub fn number(&self) -> Option<u64> {
        match self.get("number")? {
            Value::Number(n) => n.as_u64(),
            Value::String(s) => s.parse().ok(),
            _ => None,
        }
    }

    /// The line, from 0, of a page's text that gives a field of its front matter, as `status: review`.
    #[must_use]
    pub fn line_of(markdown: &str, field: &str) -> Option<usize> {
        Self::lines_of(markdown, field).map(|lines| lines.start)
    }

    /// The lines of a page's text that give a field of its front matter, from its own to the next field's.
    #[must_use]
    pub fn lines_of(markdown: &str, field: &str) -> Option<std::ops::Range<usize>> {
        let mut lines = markdown.lines().enumerate();
        if lines.next()?.1.trim_end() != "---" {
            return None;
        }
        let front: Vec<(usize, &str)> = lines
            .take_while(|(_, line)| !matches!(line.trim_end(), "---" | "..."))
            .collect();
        let top = |line: &str| !line.starts_with([' ', '\t', '-', '#']) && line.contains(':');
        let start = front.iter().position(|(_, line)| {
            line.strip_prefix(field)
                .is_some_and(|rest| rest.trim_start().starts_with(':'))
        })?;
        let end = front[start + 1..]
            .iter()
            .position(|(_, line)| top(line))
            .map_or(front.len(), |i| start + 1 + i);
        Some(front[start].0..front.get(end).map_or(front[end - 1].0 + 1, |(i, _)| *i))
    }

    /// A YAML value as JSON: its numbers, truths and text as they are, a mapping's keys as text.
    fn json(yaml: &Yaml) -> Value {
        match yaml {
            Yaml::String(s) => Value::String(s.clone()),
            Yaml::Integer(i) => Value::Number((*i).into()),
            Yaml::Real(r) => r
                .parse::<f64>()
                .ok()
                .and_then(Number::from_f64)
                .map_or_else(|| Value::String(r.clone()), Value::Number),
            Yaml::Boolean(b) => Value::Bool(*b),
            Yaml::Array(items) => Value::Array(items.iter().map(Self::json).collect()),
            Yaml::Hash(entries) => Value::Object(
                entries
                    .iter()
                    .map(|(k, v)| {
                        let key = match Self::json(k) {
                            Value::String(s) => s,
                            other => other.to_string(),
                        };
                        (key, Self::json(v))
                    })
                    .collect(),
            ),
            _ => Value::Null,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn fields_are_read_in_order_nested_as_they_are_written() {
        let front = Front::parse(
            "doctype: design\nnumber: 12\nauthors: [Ada, Grace]\nactions:\n  - what: Retry\n    due: 2026-11-01\ndraft: true\n",
        );
        assert_eq!(front.error, None);
        assert_eq!(front.doctype(), Some("design"));
        assert_eq!(front.number(), Some(12));
        assert_eq!(front.get("authors"), Some(&json!(["Ada", "Grace"])));
        assert_eq!(
            front.get("actions"),
            Some(&json!([{ "what": "Retry", "due": "2026-11-01" }])),
            "dates are text, for a schema's format to check"
        );
        assert_eq!(front.get("draft"), Some(&json!(true)));
        let names: Vec<&str> = front.fields.keys().map(String::as_str).collect();
        assert_eq!(names, ["doctype", "number", "authors", "actions", "draft"]);
    }

    #[test]
    fn front_matter_that_is_not_fields_says_why() {
        assert!(Front::parse("status: [unclosed\n").error.is_some());
        assert!(Front::parse("- a list\n").error.is_some());
        assert_eq!(Front::parse("number: \"\"\n").number(), None);
    }

    #[test]
    fn a_field_s_lines_are_found_in_the_front_matter_only() {
        let page = "---\ndoctype: design\nstatus: review\nactions:\n  - what: a\n  - what: b\nnumber: 3\n---\nstatus: not this\n";
        assert_eq!(Front::line_of(page, "status"), Some(2));
        assert_eq!(Front::lines_of(page, "actions"), Some(3..6));
        assert_eq!(Front::lines_of(page, "number"), Some(6..7));
        assert_eq!(Front::line_of(page, "authors"), None);
        assert_eq!(Front::line_of("# T\nstatus: x\n", "status"), None);
    }
}
