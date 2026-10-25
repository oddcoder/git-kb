//! A doctype's schema: what its pages' front matter may be, as JSON Schema, written in git-kb.toml as TOML. It is
//! checked by a JSON Schema validator, formats too, and walked, with a value, to show the value by its kind: an
//! `enum` as a badge, a list as its items, a record as its fields, to any depth, a `$ref` followed. Two kinds are
//! git-kb's own, marked `x-kb`: `page`, a value that names a page of the site, and `reference`, a source a page
//! cites by `[@key]`, its key being the field it is under.

use serde_json::{Map, Value};

/// A doctype, ready to show its pages: its id, its name and its schema, compiled.
#[derive(Debug)]
pub struct Doctype {
    pub id: String,
    pub name: String,
    pub schema: Schema,
}

/// The doctypes git-kb.toml has, by id.
pub type Doctypes = std::sync::Arc<std::collections::BTreeMap<String, Doctype>>;

/// The doctypes of the settings, each schema compiled once.
#[must_use]
pub fn doctypes(
    config: &std::collections::BTreeMap<String, crate::config::DoctypeConfig>,
) -> Doctypes {
    std::sync::Arc::new(
        config
            .iter()
            .map(|(id, d)| {
                let schema = if d.schema.is_null() {
                    Value::Bool(true)
                } else {
                    d.schema.clone()
                };
                let doctype = Doctype {
                    id: id.clone(),
                    name: d.name.clone().unwrap_or_else(|| id.clone()),
                    schema: Schema::new(schema),
                };
                (id.clone(), doctype)
            })
            .collect(),
    )
}

/// A doctype's schema, compiled; or, when it is not one, why.
#[derive(Debug)]
pub struct Schema {
    root: Value,
    validator: Result<jsonschema::Validator, String>,
}

/// What a value is, as its schema says.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind<'a> {
    /// One of a few values: shown as a badge.
    Enum,
    /// A page of the site, by its path or its title: shown as a link to it.
    Page,
    /// A source, cited by its key.
    Reference,
    /// A day, as `2026-10-25`.
    Date,
    /// A list, of items of this schema.
    List(&'a Value),
    /// A record: these fields, and, for others, this schema, if any.
    Record(&'a Map<String, Value>, Option<&'a Value>),
    /// Anything else, shown as it is.
    Plain,
}

/// No schema: what a list's items or a record's fields are when nothing says.
static ANY: Value = Value::Bool(true);

impl Schema {
    /// A schema, from what git-kb.toml says.
    #[must_use]
    pub fn new(root: Value) -> Self {
        let validator = jsonschema::options()
            .should_validate_formats(true)
            .build(&root)
            .map_err(|e| e.to_string());
        Self { root, validator }
    }

    /// Why it is not a schema, if it is not one.
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        self.validator.as_ref().err().map(String::as_str)
    }

    /// What is wrong with a page's front matter, as its schema has it: each problem, with where it is, as
    /// `actions/1/owner`. The doctype and the number are git-kb's, not the schema's.
    #[must_use]
    pub fn problems(&self, fields: &Map<String, Value>) -> Vec<String> {
        let Ok(validator) = &self.validator else {
            return vec![];
        };
        let mut own = fields.clone();
        own.remove("doctype");
        own.remove("number");
        let own = Value::Object(own);
        validator
            .iter_errors(&own)
            .map(|e| {
                let at = e.instance_path().to_string();
                let at = at.trim_start_matches('/');
                if at.is_empty() {
                    e.to_string()
                } else {
                    format!("{at}: {e}")
                }
            })
            .collect()
    }

    /// The schema of a field of the page's front matter, if the schema says.
    #[must_use]
    pub fn field(&self, name: &str) -> Option<&Value> {
        self.root.get("properties")?.get(name)
    }

    /// The fields the schema has, in its order.
    #[must_use]
    pub fn fields(&self) -> Vec<&str> {
        self.root
            .get("properties")
            .and_then(Value::as_object)
            .map(|props| props.keys().map(String::as_str).collect())
            .unwrap_or_default()
    }

    /// What a value of a schema is: by its `x-kb`, its `enum`, its format and its type, a `$ref` followed.
    #[must_use]
    pub fn kind<'a>(&'a self, schema: &'a Value) -> Kind<'a> {
        let schema = self.resolve(schema);
        match schema.get("x-kb").and_then(Value::as_str) {
            Some("page") => return Kind::Page,
            Some("reference") => return Kind::Reference,
            _ => {}
        }
        if schema.get("enum").is_some() {
            return Kind::Enum;
        }
        if schema.get("format").and_then(Value::as_str) == Some("date") {
            return Kind::Date;
        }
        let typed = |t: &str| match schema.get("type") {
            Some(Value::String(s)) => s == t,
            Some(Value::Array(types)) => types.iter().any(|x| x == t),
            _ => false,
        };
        if typed("array") {
            return Kind::List(schema.get("items").unwrap_or(&ANY));
        }
        if typed("object") || schema.get("properties").is_some() {
            static NONE: std::sync::LazyLock<Map<String, Value>> =
                std::sync::LazyLock::new(Map::new);
            let props = schema
                .get("properties")
                .and_then(Value::as_object)
                .unwrap_or(&NONE);
            let others = schema.get("additionalProperties").filter(|v| v.is_object());
            return Kind::Record(props, others);
        }
        Kind::Plain
    }

    /// A schema, its `$ref` followed, to its own `$defs` or `definitions`.
    fn resolve<'a>(&'a self, schema: &'a Value) -> &'a Value {
        let mut schema = schema;
        for _ in 0..16 {
            let Some(to) = schema.get("$ref").and_then(Value::as_str) else {
                break;
            };
            let Some(found) = to
                .strip_prefix('#')
                .and_then(|pointer| self.root.pointer(pointer))
            else {
                break;
            };
            schema = found;
        }
        schema
    }

    /// Every value in a field that is of a kind, at any depth, with the key of the record field it is under, if
    /// any: the pages a field names, or the sources it gives.
    #[must_use]
    pub fn all_of<'v>(
        &self,
        field: &str,
        value: &'v Value,
        kind: Kind,
    ) -> Vec<(Option<&'v str>, &'v Value)> {
        let mut found = vec![];
        if let Some(schema) = self.field(field) {
            self.walk(schema, value, None, kind, &mut found);
        }
        found
    }

    fn walk<'v>(
        &self,
        schema: &Value,
        value: &'v Value,
        key: Option<&'v str>,
        wanted: Kind,
        found: &mut Vec<(Option<&'v str>, &'v Value)>,
    ) {
        let kind = self.kind(schema);
        if kind == wanted {
            found.push((key, value));
            return;
        }
        match (kind, value) {
            (Kind::List(items), Value::Array(values)) => {
                for v in values {
                    self.walk(items, v, key, wanted, found);
                }
            }
            (Kind::Record(props, others), Value::Object(values)) => {
                for (k, v) in values {
                    if let Some(s) = props.get(k).or(others) {
                        self.walk(s, v, Some(k), wanted, found);
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn schema() -> Schema {
        Schema::new(json!({
            "type": "object",
            "required": ["status"],
            "properties": {
                "status": { "enum": ["draft", "review"] },
                "replaces": { "type": "string", "x-kb": "page" },
                "rounds": { "type": "array", "items": { "type": "array", "items": { "type": "array", "items": { "$ref": "#/$defs/source" } } } },
                "sources": { "type": "object", "additionalProperties": { "$ref": "#/$defs/source" } },
                "actions": { "type": "array", "items": { "type": "object", "required": ["owner"], "properties": { "owner": { "type": "string" }, "due": { "type": "string", "format": "date" } } } }
            },
            "$defs": { "source": { "type": "string", "x-kb": "reference" } }
        }))
    }

    #[test]
    fn front_matter_is_checked_against_its_schema_formats_too_but_for_doctype_and_number() {
        let schema = schema();
        assert_eq!(schema.error(), None);
        let fields = |v: Value| v.as_object().unwrap().clone();
        assert_eq!(
            schema.problems(&fields(
                json!({ "doctype": "design", "number": 3, "status": "draft" })
            )),
            Vec::<String>::new()
        );
        let problems = schema.problems(&fields(json!({
            "status": "shipped",
            "actions": [{ "owner": "Ada", "due": "soon" }, { "due": "2026-11-01" }]
        })));
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(
            problems.iter().any(|p| p.starts_with("status: ")),
            "{problems:?}"
        );
        assert!(
            problems.iter().any(|p| p.starts_with("actions/0/due: ")),
            "{problems:?}"
        );
        assert!(
            problems
                .iter()
                .any(|p| p.starts_with("actions/1: ") && p.contains("owner")),
            "{problems:?}"
        );
        assert!(Schema::new(json!({ "type": 12 })).error().is_some());
    }

    #[test]
    fn values_are_found_by_their_kind_at_any_depth_refs_followed() {
        let schema = schema();
        assert_eq!(schema.kind(schema.field("status").unwrap()), Kind::Enum);
        assert_eq!(schema.kind(schema.field("replaces").unwrap()), Kind::Page);
        let rounds = json!([[["a", "b"]], [["c"]]]);
        let found: Vec<&Value> = schema
            .all_of("rounds", &rounds, Kind::Reference)
            .into_iter()
            .map(|(_, v)| v)
            .collect();
        assert_eq!(
            found,
            [&json!("a"), &json!("b"), &json!("c")],
            "a list of lists of lists of references"
        );
        let sources = json!({ "kung": "Kung, 1982", "tpu": "Jouppi, 2017" });
        let found = schema.all_of("sources", &sources, Kind::Reference);
        assert_eq!(
            found[0],
            (Some("kung"), &json!("Kung, 1982")),
            "by the key they are under"
        );
        assert_eq!(
            schema.fields(),
            ["status", "replaces", "rounds", "sources", "actions"]
        );
    }
}
