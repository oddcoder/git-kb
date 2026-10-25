//! A page's front matter: a block of YAML at its very top, between two `---` lines, that says what the page is and
//! what only it can say, as its doctype, number, status and authors. It is not part of the page's text: it is not
//! rendered, nor a block, nor searched. Its dates and who wrote it are git's to say, not its.
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

use yaml_rust2::{Yaml, YamlLoader};

/// A field's value: a word or a sentence, a list of them, or, as for a survey's sources, a list of them by key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Text(String),
    List(Vec<String>),
    Map(Vec<(String, String)>),
}

/// A page's front matter: its fields, in the order it gives them; or, when it is not YAML that git-kb reads, why.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Front {
    pub fields: Vec<(String, Value)>,
    pub error: Option<String>,
}

impl Front {
    /// The front matter of this YAML: a mapping of fields, each a scalar, a list of scalars, or a mapping of them.
    #[must_use]
    pub fn parse(yaml: &str) -> Self {
        let docs = match YamlLoader::load_from_str(yaml) {
            Ok(docs) => docs,
            Err(e) => {
                return Self {
                    fields: vec![],
                    error: Some(e.to_string()),
                };
            }
        };
        let Some(Yaml::Hash(top)) = docs.into_iter().next() else {
            return Self {
                fields: vec![],
                error: Some("front matter is fields, as `status: draft`".into()),
            };
        };
        let mut front = Self::default();
        for (key, value) in top {
            let Some(key) = Self::scalar(&key) else {
                continue;
            };
            let value = match value {
                Yaml::Array(items) => Value::List(items.iter().filter_map(Self::scalar).collect()),
                Yaml::Hash(entries) => Value::Map(
                    entries
                        .iter()
                        .filter_map(|(k, v)| Some((Self::scalar(k)?, Self::scalar(v)?)))
                        .collect(),
                ),
                other => match Self::scalar(&other) {
                    Some(text) => Value::Text(text),
                    None => continue,
                },
            };
            front.fields.push((key, value));
        }
        front
    }

    /// A field, by its name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.fields.iter().find(|(k, _)| k == name).map(|(_, v)| v)
    }

    /// A field that is a word or a sentence, by its name, unless it is empty.
    #[must_use]
    pub fn text(&self, name: &str) -> Option<&str> {
        match self.get(name)? {
            Value::Text(text) if !text.is_empty() => Some(text),
            _ => None,
        }
    }

    /// What the page is, by the id of a doctype in git-kb.toml.
    #[must_use]
    pub fn doctype(&self) -> Option<&str> {
        self.text("doctype")
    }

    /// The page's number among its doctype's.
    #[must_use]
    pub fn number(&self) -> Option<u64> {
        self.text("number")?.parse().ok()
    }

    /// The line, from 0, of a page's text that gives a field of its front matter, as `status: review`.
    #[must_use]
    pub fn line_of(markdown: &str, field: &str) -> Option<usize> {
        let mut lines = markdown.lines().enumerate();
        if lines.next()?.1.trim_end() != "---" {
            return None;
        }
        lines
            .take_while(|(_, line)| !matches!(line.trim_end(), "---" | "..."))
            .find(|(_, line)| {
                line.strip_prefix(field)
                    .is_some_and(|rest| rest.trim_start().starts_with(':'))
            })
            .map(|(i, _)| i)
    }

    /// A scalar as text: a string as it is, a number or a truth as written; nothing for null.
    fn scalar(yaml: &Yaml) -> Option<String> {
        match yaml {
            Yaml::String(s) | Yaml::Real(s) => Some(s.clone()),
            Yaml::Integer(i) => Some(i.to_string()),
            Yaml::Boolean(b) => Some(b.to_string()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_are_read_in_order_as_text_lists_and_maps() {
        let front = Front::parse(
            "doctype: design\nnumber: 12\nauthors: [Ada, Grace]\nsources:\n  k1: \"A, B, 2024\"\ndraft: true\n",
        );
        assert_eq!(front.error, None);
        assert_eq!(front.doctype(), Some("design"));
        assert_eq!(front.number(), Some(12));
        assert_eq!(
            front.get("authors"),
            Some(&Value::List(vec!["Ada".into(), "Grace".into()]))
        );
        assert_eq!(
            front.get("sources"),
            Some(&Value::Map(vec![("k1".into(), "A, B, 2024".into())]))
        );
        assert_eq!(front.text("draft"), Some("true"));
        let names: Vec<&str> = front.fields.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(names, ["doctype", "number", "authors", "sources", "draft"]);
    }

    #[test]
    fn a_field_s_line_is_found_in_the_front_matter_only() {
        let page = "---\ndoctype: design\nstatus: review\n---\nstatus: not this\n";
        assert_eq!(Front::line_of(page, "status"), Some(2));
        assert_eq!(Front::line_of(page, "authors"), None);
        assert_eq!(Front::line_of("# T\nstatus: x\n", "status"), None);
    }

    #[test]
    fn front_matter_that_is_not_fields_says_why() {
        assert!(Front::parse("status: [unclosed\n").error.is_some());
        assert!(Front::parse("- a list\n").error.is_some());
        assert_eq!(Front::parse("number: \"\"\n").number(), None);
    }
}
