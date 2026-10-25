//! What a page is, in its shell, from its front matter and its doctype's schema: under its title, its doctype and
//! number, as Design-12, then each of its fields, shown by its kind, as the schema says, to any depth: one of a
//! few values as a badge, a page as a link to it, a list as its items, a list of records as a table. Each field
//! says, pointed at, when it was last set, by whom. What the schema finds wrong is said under them. A page another
//! names in a field says so; a page's citations of the sources it gives are numbered, and listed at its end.

use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::Path;

use serde_json::Value;

use super::{Links, Shell};
use crate::front::Front;
use crate::git::Change;
use crate::nav::Pages;
use crate::schema::{Doctype, Kind, Schema};

/// Who last set each field of a page's front matter, by the field.
pub type Setters = BTreeMap<String, Change>;

/// A schema that says nothing, for a page of no doctype the settings have.
static ANY: std::sync::LazyLock<Schema> =
    std::sync::LazyLock::new(|| Schema::new(Value::Bool(true)));

impl Shell {
    /// The lines under a page's title that say what it is, what is wrong with its front matter, and which pages
    /// name it in theirs; none for a page without front matter that none names.
    #[must_use]
    pub fn doc_head(&self, at: &Path, page: &Path, pages: &Pages, setters: &Setters) -> String {
        let named_by = self.naming(at, page, pages);
        let Some(front) = pages.front(page) else {
            return named_by;
        };
        if let Some(error) = &front.error {
            return named_by
                + &format!(
                    "<div class=\"problems\"><p>Its front matter cannot be read: {}</p></div>\n",
                    Self::escape(error)
                );
        }
        named_by + &self.what_it_is(at, front, pages, setters) + &self.problems_of(front)
    }

    /// The doctype of a page's front matter, if the settings have it.
    fn doctype_of(&self, front: &Front) -> Option<&Doctype> {
        self.doctypes.get(front.doctype()?)
    }

    /// A page's doctype and number, as it is shown: `Design-12`; its doctype's id when the settings do not have it.
    fn numbered(&self, front: &Front) -> Option<String> {
        let id = front.doctype()?;
        let name = self.doctypes.get(id).map_or(id, |d| d.name.as_str());
        Some(
            front
                .number()
                .map_or_else(|| name.to_string(), |n| format!("{name}-{n}")),
        )
    }

    /// The fields of a page's front matter that are shown, in its schema's order, then the others it has: all but
    /// its doctype and number, and the sources it gives, which are listed at its end.
    fn shown_fields<'f>(schema: &Schema, front: &'f Front) -> Vec<(&'f str, &'f Value)> {
        let mut names: Vec<&str> = schema.fields();
        for name in front.fields.keys() {
            if !names.contains(&name.as_str()) {
                names.push(name);
            }
        }
        names
            .into_iter()
            .filter(|n| !matches!(*n, "doctype" | "number"))
            .filter_map(|n| front.fields.get_key_value(n))
            .filter(|(n, v)| schema.all_of(n, v, Kind::Reference).is_empty())
            .map(|(n, v)| (n.as_str(), v))
            .collect()
    }

    /// The line under a page's title that says what it is, and the tables of its fields that are lists of records.
    fn what_it_is(&self, at: &Path, front: &Front, pages: &Pages, setters: &Setters) -> String {
        let doctype = self.doctype_of(front);
        let schema = doctype.map_or(&*ANY, |d| &d.schema);
        let mut parts = vec![];
        if let (Some(id), Some(numbered)) = (front.doctype(), self.numbered(front)) {
            let unknown = if doctype.is_none() {
                " title=\"No such doctype in git-kb.toml\""
            } else {
                ""
            };
            parts.push(format!(
                "<a class=\"doctype\" href=\"{}\"{unknown}>{}</a>",
                self.doctype_href(at, id),
                Self::escape(&numbered)
            ));
        }
        let mut tables = String::new();
        for (name, value) in Self::shown_fields(schema, front) {
            let field = schema.field(name).unwrap_or(&Value::Bool(true));
            let set = setters.get(name).map_or_else(String::new, |c| {
                format!(
                    " title=\"Set {} by {} in {}: {}\"",
                    self.ago(c),
                    Self::escape(&c.author),
                    c.short,
                    Self::escape(&c.summary)
                )
            });
            if let (Kind::List(items), Value::Array(rows)) = (schema.kind(field), value)
                && let Kind::Record(..) = schema.kind(items)
                && !rows.is_empty()
            {
                tables.push_str(&self.table(at, name, items, rows, schema, pages, &set));
                continue;
            }
            let shown = self.value(at, field, value, schema, pages);
            if shown.is_empty() {
                continue;
            }
            if schema.kind(field) == Kind::Enum {
                parts.push(format!("<span class=\"field\"{set}>{shown}</span>"));
            } else {
                parts.push(format!(
                    "<span class=\"field\"{set}><span class=\"label\">{}</span> {shown}</span>",
                    Self::label(name)
                ));
            }
        }
        if parts.is_empty() && tables.is_empty() {
            return String::new();
        }
        format!("<p class=\"doc-head\">{}</p>\n{tables}", parts.join(""))
    }

    /// A value, shown by its kind, as its schema says: a badge, a link, a date, a list of its items, a record's
    /// fields; or as it is.
    fn value(
        &self,
        at: &Path,
        schema_of: &Value,
        value: &Value,
        schema: &Schema,
        pages: &Pages,
    ) -> String {
        match (schema.kind(schema_of), value) {
            (_, Value::Null) => String::new(),
            (Kind::Enum, v) => {
                let v = Self::plain(v);
                format!(
                    "<span class=\"badge v-{}\">{}</span>",
                    Self::value_class(&v),
                    Self::escape(&v)
                )
            }
            (Kind::Page, Value::String(name)) => self.named(at, name, pages),
            (Kind::List(items), Value::Array(values)) => values
                .iter()
                .map(|v| self.value(at, items, v, schema, pages))
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(", "),
            (Kind::Record(props, others), Value::Object(values)) => values
                .iter()
                .map(|(k, v)| {
                    let s = props.get(k).or(others).unwrap_or(&Value::Bool(true));
                    format!(
                        "<span class=\"label\">{}</span> {}",
                        Self::label(k),
                        self.value(at, s, v, schema, pages)
                    )
                })
                .collect::<Vec<_>>()
                .join(" · "),
            // Of no kind the schema says: a list or a record shown by its shape.
            (Kind::Plain, Value::Array(_) | Value::Object(_)) => {
                let shape = if value.is_array() {
                    serde_json::json!({ "type": "array" })
                } else {
                    serde_json::json!({ "type": "object" })
                };
                self.value(at, &shape, value, schema, pages)
            }
            (_, v) => Self::escape(&Self::plain(v)),
        }
    }

    /// A field that is a list of records, as a table: a column for each of their fields, the records' schema's
    /// first, then any other they have.
    #[allow(clippy::too_many_arguments)]
    fn table(
        &self,
        at: &Path,
        name: &str,
        items: &Value,
        rows: &[Value],
        schema: &Schema,
        pages: &Pages,
        set: &str,
    ) -> String {
        let Kind::Record(props, others) = schema.kind(items) else {
            return String::new();
        };
        let mut columns: Vec<&str> = props.keys().map(String::as_str).collect();
        for row in rows.iter().filter_map(Value::as_object) {
            for k in row.keys() {
                if !columns.contains(&k.as_str()) {
                    columns.push(k);
                }
            }
        }
        let mut out = format!("<table class=\"doc-list doc-table\"{set}><thead><tr>");
        for (i, c) in columns.iter().enumerate() {
            let head = if i == 0 {
                format!("{} · {}", Self::label(name), Self::label(c))
            } else {
                Self::label(c)
            };
            let _ = write!(out, "<th>{head}</th>");
        }
        out.push_str("</tr></thead><tbody>");
        for row in rows {
            out.push_str("<tr>");
            for c in &columns {
                let s = props.get(*c).or(others).unwrap_or(&Value::Bool(true));
                let cell = row
                    .get(*c)
                    .map_or_else(String::new, |v| self.value(at, s, v, schema, pages));
                let _ = write!(out, "<td>{cell}</td>");
            }
            out.push_str("</tr>");
        }
        out + "</tbody></table>\n"
    }

    /// What is wrong with a page's front matter: a doctype the settings do not have, a schema that is not one, or
    /// what its schema says of it.
    #[must_use]
    pub fn problems(&self, front: &Front) -> Vec<String> {
        if let Some(error) = &front.error {
            return vec![format!("its front matter cannot be read: {error}")];
        }
        let Some(id) = front.doctype() else {
            return vec![];
        };
        let Some(doctype) = self.doctypes.get(id) else {
            return vec![format!("no doctype {id} in git-kb.toml")];
        };
        if let Some(error) = doctype.schema.error() {
            return vec![format!("{id}'s schema in git-kb.toml is not one: {error}")];
        }
        doctype.schema.problems(&front.fields)
    }

    /// The same, as shown under a page's title.
    fn problems_of(&self, front: &Front) -> String {
        let problems = self.problems(front);
        if problems.is_empty() {
            return String::new();
        }
        let lines = Self::problem_lines(&problems);
        format!("<div class=\"problems\">{lines}</div>\n")
    }

    /// Problems, a line each.
    fn problem_lines(problems: &[String]) -> String {
        let mut lines = String::new();
        for p in problems {
            let _ = write!(lines, "<p>⚠ {}</p>", Self::escape(p));
        }
        lines
    }

    /// The pages that name a page in a field of theirs a schema says names one: each, as "Design-2 replaces this".
    fn naming(&self, at: &Path, page: &Path, pages: &Pages) -> String {
        let mut lines = String::new();
        for (other, front) in pages.fronts() {
            if other == page {
                continue;
            }
            let Some(doctype) = self.doctype_of(front) else {
                continue;
            };
            for (name, value) in &front.fields {
                let names_it = doctype
                    .schema
                    .all_of(name, value, Kind::Page)
                    .iter()
                    .any(|(_, v)| v.as_str().and_then(|n| pages.named(n)) == Some(page));
                if names_it {
                    let who = self
                        .numbered(front)
                        .unwrap_or_else(|| pages.title(other).unwrap_or_default().to_string());
                    let _ = write!(
                        lines,
                        "<p><a href=\"{}\">{}</a> {} this</p>",
                        self.link(at, &other.with_extension("html")),
                        Self::escape(&who),
                        Self::escape(&Self::label(name).to_lowercase())
                    );
                }
            }
        }
        if lines.is_empty() {
            lines
        } else {
            format!("<div class=\"named-by\">{lines}</div>\n")
        }
    }

    /// A page's rendered body with its citations: each `[@key]`, or `[@a; @b]`, a link to the source its front
    /// matter gives under that key, in a field its schema says gives sources, numbered in the order they are first
    /// cited, and a list of them at its end. A key it has no source for is marked. Code is left as it is.
    #[must_use]
    pub fn cited(&self, html: &str, front: Option<&Front>) -> String {
        let Some((front, doctype)) = front.and_then(|f| Some((f, self.doctype_of(f)?))) else {
            return html.to_string();
        };
        let sources: Vec<(String, String)> = front
            .fields
            .iter()
            .flat_map(|(name, value)| doctype.schema.all_of(name, value, Kind::Reference))
            .filter_map(|(key, value)| Some((key?.to_string(), Self::plain(value))))
            .collect();
        if sources.is_empty() {
            return html.to_string();
        }
        let mut order: Vec<String> = vec![];
        let mut out = String::with_capacity(html.len());
        let mut rest = html;
        let mut code = 0usize;
        while !rest.is_empty() {
            // Tags are copied as they are, counting whether they open or close code.
            if rest.starts_with('<') {
                let end = rest.find('>').map_or(rest.len(), |i| i + 1);
                let tag = &rest[..end];
                if tag.starts_with("<code") || tag.starts_with("<pre") {
                    code += 1;
                } else if tag.starts_with("</code") || tag.starts_with("</pre") {
                    code = code.saturating_sub(1);
                }
                out.push_str(tag);
                rest = &rest[end..];
                continue;
            }
            let text_end = rest.find('<').unwrap_or(rest.len());
            let text = &rest[..text_end];
            if code > 0 {
                out.push_str(text);
            } else {
                out.push_str(&Self::cite_in(text, &sources, &mut order));
            }
            rest = &rest[text_end..];
        }
        if order.is_empty() {
            return out;
        }
        out.push_str("<section class=\"references\"><h2 id=\"references\">References</h2><ol>\n");
        for key in &order {
            let said = sources.iter().find(|(k, _)| k == key).map_or_else(
                || "No such source in its front matter".to_string(),
                |(_, v)| Self::linked_urls(v),
            );
            let _ = writeln!(out, "<li id=\"ref-{}\">{said}</li>", Self::escape(key));
        }
        out + "</ol></section>\n"
    }

    /// Text with its citations made links, each key numbered as it is first cited.
    fn cite_in(text: &str, sources: &[(String, String)], order: &mut Vec<String>) -> String {
        let mut out = String::new();
        let mut rest = text;
        while let Some(start) = rest.find("[@") {
            let Some(len) = rest[start..].find(']') else {
                break;
            };
            let inside = &rest[start + 1..start + len];
            let keys: Vec<&str> = inside
                .split(';')
                .map(|k| k.trim().trim_start_matches('@'))
                .collect();
            let well_formed = inside.split(';').all(|k| k.trim().starts_with('@'))
                && keys.iter().all(|k| {
                    !k.is_empty() && k.chars().all(|c| c.is_alphanumeric() || "-_:.".contains(c))
                });
            out.push_str(&rest[..start]);
            if !well_formed {
                out.push_str(&rest[start..=start + len]);
                rest = &rest[start + len + 1..];
                continue;
            }
            let links: Vec<String> = keys
                .iter()
                .map(|key| {
                    let n = order.iter().position(|k| k == key).unwrap_or_else(|| {
                        order.push((*key).to_string());
                        order.len() - 1
                    }) + 1;
                    let missing = if sources.iter().any(|(k, _)| k == key) {
                        ""
                    } else {
                        " broken"
                    };
                    format!(
                        "<a class=\"cite{missing}\" href=\"#ref-{}\">{n}</a>",
                        Self::escape(key)
                    )
                })
                .collect();
            let _ = write!(out, "<sup class=\"cites\">[{}]</sup>", links.join(", "));
            rest = &rest[start + len + 1..];
        }
        out + rest
    }

    /// A source as it is written, escaped, with the addresses in it made links.
    fn linked_urls(source: &str) -> String {
        source
            .split(' ')
            .map(|word| {
                let source = Self::escape(word);
                if word.starts_with("https://") || word.starts_with("http://") {
                    format!("<a href=\"{source}\">{source}</a>")
                } else {
                    source
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Where a doctype's list of pages is: served, `/_doctypes/<id>`, at the commit shown; built, beside the site's
    /// graph.
    #[must_use]
    pub fn doctype_href(&self, at: &Path, id: &str) -> String {
        let id = Self::encode_part(id);
        match (self.links, &self.revision) {
            (Links::Served, None) => format!("/_doctypes/{id}"),
            (Links::Served, Some(r)) => {
                format!("/_doctypes/{id}?ref={}", Self::encode_part(&r.name))
            }
            (Links::Built, _) => self.to(at, &format!("_doctype-{id}.html")),
        }
    }

    /// A doctype's list of pages, as a page's body: a row of each, by number, with its fields that are not lists
    /// of records and what is wrong with it; and, for each field that is one of a few values, a choice of which to
    /// show.
    #[must_use]
    pub fn doctype_list(&self, at: &Path, id: &str, pages: &Pages) -> String {
        let doctype = self.doctypes.get(id);
        let schema = doctype.map_or(&*ANY, |d| &d.schema);
        let found = pages.of_doctype(id);
        // The fields shown as columns: the schema's, but for lists of records and sources.
        let columns: Vec<&str> = schema
            .fields()
            .into_iter()
            .filter(|name| {
                let field = schema.field(name).unwrap_or(&Value::Bool(true));
                let records = matches!(schema.kind(field), Kind::List(items) if matches!(schema.kind(items), Kind::Record(..)));
                let sources = matches!(schema.kind(field), Kind::Reference)
                    || matches!(schema.kind(field), Kind::Record(_, Some(others)) if schema.kind(others) == Kind::Reference);
                !records && !sources
            })
            .collect();
        let choices = Self::choices(schema, &found);
        let mut head = String::from("<tr><th>#</th><th>Title</th>");
        for c in &columns {
            let _ = write!(head, "<th>{}</th>", Self::label(c));
        }
        // A column of what is wrong with each page, only when any page has something wrong.
        let flagged = found
            .iter()
            .any(|(_, _, front)| !self.problems(front).is_empty());
        if flagged {
            head.push_str("<th>Problems</th>");
        }
        head.push_str("</tr>");
        let mut rows = String::new();
        for (path, title, front) in &found {
            let mut data = String::new();
            for (field, _) in &choices {
                let v = front.get(field).map(Self::plain).unwrap_or_default();
                let _ = write!(
                    data,
                    " data-{}=\"{}\"",
                    Self::value_class(field),
                    Self::value_class(&v)
                );
            }
            let _ = write!(
                rows,
                "<tr{data}><td>{}</td><td><a href=\"{}\">{}</a></td>",
                front.number().map_or_else(String::new, |n| format!(
                    "{}-{n}",
                    Self::escape(doctype.map_or(id, |d| d.name.as_str()))
                )),
                self.link(at, &path.with_extension("html")),
                Self::escape(title)
            );
            for c in &columns {
                let field = schema.field(c).unwrap_or(&Value::Bool(true));
                let cell = front
                    .get(c)
                    .map_or_else(String::new, |v| self.value(at, field, v, schema, pages));
                let _ = write!(rows, "<td>{cell}</td>");
            }
            if flagged {
                let problems = self.problems(front);
                let flag = if problems.is_empty() {
                    String::new()
                } else {
                    format!(
                        "<span class=\"problem-flag\" title=\"{}\">⚠ {}</span>",
                        Self::escape(&problems.join("\n")),
                        problems.len()
                    )
                };
                let _ = write!(rows, "<td>{flag}</td>");
            }
            rows.push_str("</tr>\n");
        }
        let name = doctype.map_or(id, |d| d.name.as_str());
        let empty = if found.is_empty() {
            format!(
                "<p class=\"muted\">No pages say they are a {} yet.</p>\n",
                Self::escape(name)
            )
        } else {
            String::new()
        };
        format!(
            "<h1>{}</h1>\n{}{empty}<table class=\"doc-list\"><thead>{head}</thead><tbody>\n{rows}</tbody></table>\n",
            Self::escape(&self.doctype_title(id)),
            Self::choice(&choices)
        )
    }

    /// The fields of a doctype's pages that are one of a few values, each with the values: its schema's, then any
    /// other its pages have.
    fn choices<'a>(
        schema: &'a Schema,
        found: &[(&Path, &str, &'a Front)],
    ) -> Vec<(&'a str, Vec<String>)> {
        let mut choices = vec![];
        for name in schema.fields() {
            let Some(field) = schema.field(name) else {
                continue;
            };
            if schema.kind(field) != Kind::Enum {
                continue;
            }
            let mut values: Vec<String> = field
                .get("enum")
                .and_then(Value::as_array)
                .map(|vs| vs.iter().map(Self::plain).collect())
                .unwrap_or_default();
            for (_, _, front) in found {
                if let Some(v) = front.get(name).map(Self::plain)
                    && !values.contains(&v)
                {
                    values.push(v);
                }
            }
            choices.push((name, values));
        }
        choices
    }

    /// For each field that is one of a few values, which of them a doctype's list shows: all, or one; by a choice
    /// CSS reads, so that it needs no script, and a built site has it too.
    fn choice(choices: &[(&str, Vec<String>)]) -> String {
        let mut out = String::new();
        let mut rules = String::new();
        for (field, values) in choices {
            let f = Self::value_class(field);
            let _ = write!(
                out,
                "<p class=\"doc-filter\" data-field=\"{f}\"><span class=\"label\">{}</span><label><input type=\"radio\" name=\"{f}\" value=\"\" checked> All</label>",
                Self::label(field)
            );
            for v in values {
                let c = Self::value_class(v);
                let _ = write!(
                    out,
                    "<label><input type=\"radio\" name=\"{f}\" value=\"{c}\"> {}</label>",
                    Self::escape(v)
                );
                let _ = write!(
                    rules,
                    "main:has(.doc-filter [name=\"{f}\"][value=\"{c}\"]:checked) .doc-list tbody tr:not([data-{f}=\"{c}\"]) {{ display: none; }}"
                );
            }
            out.push_str("</p>\n");
        }
        if rules.is_empty() {
            out
        } else {
            out + "<style>" + &rules + "</style>\n"
        }
    }

    /// What a page is, for its card in a folder's list: its doctype and number, and its fields that are one of a
    /// few values.
    #[must_use]
    pub fn card_badges(&self, front: Option<&Front>) -> String {
        let Some(front) = front.filter(|f| f.error.is_none()) else {
            return String::new();
        };
        let mut badges = String::new();
        if let Some(numbered) = self.numbered(front) {
            let _ = write!(
                badges,
                "<b class=\"doctype\">{}</b>",
                Self::escape(&numbered)
            );
        }
        if let Some(doctype) = self.doctype_of(front) {
            for name in doctype.schema.fields() {
                let (Some(field), Some(value)) = (doctype.schema.field(name), front.get(name))
                else {
                    continue;
                };
                if doctype.schema.kind(field) == Kind::Enum {
                    let v = Self::plain(value);
                    let _ = write!(
                        badges,
                        "<b class=\"badge v-{}\">{}</b>",
                        Self::value_class(&v),
                        Self::escape(&v)
                    );
                }
            }
        }
        if badges.is_empty() {
            badges
        } else {
            format!("<em class=\"doc-head\">{badges}</em>")
        }
    }

    /// A link to each doctype's list of pages, with how many it has, for the top of the site's contents.
    #[must_use]
    pub fn doctype_links(&self, at: &Path, pages: &Pages) -> String {
        let links: Vec<String> = self
            .doctypes
            .keys()
            .map(|id| {
                format!(
                    "<a href=\"{}\">{} <span class=\"muted\">{}</span></a>",
                    self.doctype_href(at, id),
                    Self::escape(&self.doctype_title(id)),
                    pages.of_doctype(id).len()
                )
            })
            .collect();
        if links.is_empty() {
            String::new()
        } else {
            format!("<p class=\"doctypes\">{}</p>\n", links.join(""))
        }
    }

    /// Every page whose front matter has problems, with them, as a page's body.
    #[must_use]
    pub fn problems_page(&self, at: &Path, pages: &Pages) -> String {
        let mut items = String::new();
        for (page, front) in pages.fronts() {
            let problems = self.problems(front);
            if problems.is_empty() {
                continue;
            }
            let lines = Self::problem_lines(&problems);
            let _ = write!(
                items,
                "<li><a href=\"{}\">{}</a>{lines}</li>",
                self.link(at, &page.with_extension("html")),
                Self::escape(pages.title(page).unwrap_or_default())
            );
        }
        if items.is_empty() {
            "<h1>Problems</h1>\n<p class=\"muted\">Every page's front matter is as its doctype's schema says.</p>\n".into()
        } else {
            format!("<h1>Problems</h1>\n<ul class=\"problem-list\">{items}</ul>\n")
        }
    }

    /// The ids of the doctypes the settings have.
    #[must_use]
    pub fn doctype_ids(&self) -> Vec<String> {
        self.doctypes.keys().cloned().collect()
    }

    /// The title of a doctype's list of pages: `Designs`.
    #[must_use]
    pub fn doctype_title(&self, id: &str) -> String {
        Self::plural(self.doctypes.get(id).map_or(id, |d| d.name.as_str()))
    }

    /// A doctype's name for many of its pages: `Designs`, `Studies`.
    fn plural(name: &str) -> String {
        if let Some(stem) = name.strip_suffix('y')
            && !stem.ends_with(['a', 'e', 'i', 'o', 'u'])
        {
            format!("{stem}ies")
        } else if name.ends_with('s') {
            format!("{name}es")
        } else {
            format!("{name}s")
        }
    }

    /// A field's name, as a label: `depends-on` as `Depends on`.
    fn label(name: &str) -> String {
        let mut label = name.replace(['-', '_'], " ");
        if let Some(first) = label.get_mut(..1) {
            first.make_ascii_uppercase();
        }
        Self::escape(&label)
    }

    /// A value as text: text as it is, anything else as JSON writes it.
    fn plain(value: &Value) -> String {
        match value {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    }

    /// A value, as a class or an attribute's value: lower case, a `-` for a space.
    fn value_class(value: &str) -> String {
        Self::escape(&value.to_lowercase().replace(' ', "-"))
    }

    /// How long ago a commit was, as the site says it: served, `3 days ago`; built, by the day.
    fn ago(&self, change: &Change) -> String {
        match self.links {
            Links::Served => change.ago(crate::git::When::now().0),
            Links::Built => format!("on {}", change.date()),
        }
    }

    /// A value, as text; or, when it names a page of the site, by its path or its title, a link to it.
    fn named(&self, at: &Path, value: &str, pages: &Pages) -> String {
        match pages.named(value) {
            Some(page) => format!(
                "<a href=\"{}\">{}</a>",
                self.link(at, &page.with_extension("html")),
                Self::escape(pages.title(page).unwrap_or(value))
            ),
            None => Self::escape(value),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;

    use super::*;
    use crate::config::DoctypeConfig;
    use crate::render::Renderer;

    fn shell() -> Shell {
        let design = DoctypeConfig {
            name: Some("Design".into()),
            schema: json!({
                "type": "object",
                "required": ["status", "authors"],
                "properties": {
                    "status": { "enum": ["draft", "review"] },
                    "authors": { "type": "array", "items": { "type": "string" } },
                    "replaces": { "type": "string", "x-kb": "page" },
                    "actions": { "type": "array", "items": { "type": "object", "properties": { "what": { "type": "string" }, "owner": { "type": "string" } } } },
                    "sources": { "type": "object", "additionalProperties": { "type": "string", "x-kb": "reference" } }
                }
            }),
            ..DoctypeConfig::default()
        };
        let config = BTreeMap::from([("design".to_string(), design)]);
        Shell::new("Site".into(), Links::Built).typing(crate::schema::doctypes(&config))
    }

    fn pages(texts: &[(&str, &str)]) -> Pages {
        let files: Vec<PathBuf> = texts.iter().map(|(p, _)| PathBuf::from(p)).collect();
        let texts: BTreeMap<String, String> = texts
            .iter()
            .map(|(p, t)| ((*p).into(), (*t).into()))
            .collect();
        Pages::new(&files, &Renderer::new(), |f| {
            texts.get(f.to_str().unwrap()).cloned()
        })
    }

    #[test]
    fn a_page_s_head_shows_each_field_by_its_kind_with_who_set_it() {
        let pages = pages(&[
            ("designs/old.md", "# Old tiling\n"),
            (
                "designs/new.md",
                "---\ndoctype: design\nnumber: 12\nstatus: review\nauthors: [Ada, Grace]\nreplaces: designs/old.md\nactions:\n  - what: Retry\n    owner: Grace\nextra: 3\n---\n# New\n",
            ),
        ]);
        let setters = Setters::new();
        let head = shell().doc_head(
            Path::new("designs"),
            Path::new("designs/new.md"),
            &pages,
            &setters,
        );
        assert!(
            head.starts_with(concat!(
                "<p class=\"doc-head\"><a class=\"doctype\" href=\"../_doctype-design.html\">Design-12</a>",
                "<span class=\"field\"><span class=\"badge v-review\">review</span></span>",
                "<span class=\"field\"><span class=\"label\">Authors</span> Ada, Grace</span>",
                "<span class=\"field\"><span class=\"label\">Replaces</span> <a href=\"../designs/old.html\">Old tiling</a></span>",
                "<span class=\"field\"><span class=\"label\">Extra</span> 3</span></p>\n",
                "<table class=\"doc-list doc-table\"><thead><tr><th>Actions · What</th><th>Owner</th></tr></thead>",
                "<tbody><tr><td>Retry</td><td>Grace</td></tr></tbody></table>\n"
            )),
            "{head}"
        );
        let old = shell().doc_head(
            Path::new("designs"),
            Path::new("designs/old.md"),
            &pages,
            &setters,
        );
        assert_eq!(
            old,
            "<div class=\"named-by\"><p><a href=\"../designs/new.html\">Design-12</a> replaces this</p></div>\n"
        );
    }

    #[test]
    fn what_the_schema_finds_wrong_is_said_under_the_head() {
        let pages = pages(&[
            ("a.md", "---\ndoctype: design\nstatus: shipped\n---\n# A\n"),
            ("b.md", "---\ndoctype: memo\n---\n# B\n"),
        ]);
        let shell = shell();
        let head = shell.doc_head(Path::new(""), Path::new("a.md"), &pages, &Setters::new());
        let problems = &head[head.find("<div class=\"problems\">").expect("problems")..];
        assert!(
            problems.contains("status: ") && problems.contains("authors"),
            "{problems}"
        );
        assert_eq!(
            shell.problems(pages.front(Path::new("b.md")).unwrap()),
            ["no doctype memo in git-kb.toml"]
        );
        let all = shell.problems_page(Path::new(""), &pages);
        assert!(
            all.contains("<a href=\"./a.html\">A</a>")
                && all.contains("<a href=\"./b.html\">B</a>"),
            "{all}"
        );
    }

    #[test]
    fn a_doctype_lists_its_pages_by_number_with_a_choice_of_each_enum() {
        let pages = pages(&[
            (
                "a.md",
                "---\ndoctype: design\nnumber: 2\nstatus: draft\nauthors: [Ada]\n---\n# Second\n",
            ),
            (
                "b.md",
                "---\ndoctype: design\nnumber: 1\nstatus: review\nauthors: [Bo]\n---\n# First\n",
            ),
        ]);
        let list = shell().doctype_list(Path::new(""), "design", &pages);
        assert!(list.starts_with("<h1>Designs</h1>"), "{list}");
        assert!(
            list.find("First").unwrap() < list.find("Second").unwrap(),
            "{list}"
        );
        assert!(
            list.contains("<tr data-status=\"draft\"><td>Design-2</td><td><a href=\"./a.html\">Second</a></td>"),
            "{list}"
        );
        assert!(
            list.contains("[name=\"status\"][value=\"review\"]:checked) .doc-list tbody tr:not([data-status=\"review\"])"),
            "{list}"
        );
        assert!(
            !list.contains("<th>Actions</th>") && !list.contains("<th>Sources</th>"),
            "{list}"
        );
        assert!(
            !list.contains("<th>Problems</th>") && !list.contains("<th></th>"),
            "no column of problems when there are none: {list}"
        );
        let broken = self::tests::pages(&[(
            "c.md",
            "---\ndoctype: design\nnumber: 3\nstatus: shipped\nauthors: [Cy]\n---\n# C\n",
        )]);
        let list = shell().doctype_list(Path::new(""), "design", &broken);
        assert!(
            list.contains("<th>Problems</th>") && list.contains("class=\"problem-flag\""),
            "{list}"
        );
    }

    #[test]
    fn citations_are_numbered_by_first_use_with_their_sources_at_the_end() {
        let front = Front::parse(
            "doctype: design\nsources:\n  kung: \"Kung, Systolic arrays, 1982. https://x.org/k\"\n  tpu: \"Jouppi, TPU, 2017\"\n",
        );
        let html = "<p>As in [@tpu] and [@kung; @tpu], not [@nope] nor [a link].</p><pre><code>[@tpu]</code></pre>";
        let cited = shell().cited(html, Some(&front));
        assert!(
            cited.starts_with(concat!(
                "<p>As in <sup class=\"cites\">[<a class=\"cite\" href=\"#ref-tpu\">1</a>]</sup> and ",
                "<sup class=\"cites\">[<a class=\"cite\" href=\"#ref-kung\">2</a>, <a class=\"cite\" href=\"#ref-tpu\">1</a>]</sup>, ",
                "not <sup class=\"cites\">[<a class=\"cite broken\" href=\"#ref-nope\">3</a>]</sup> nor [a link].</p>",
                "<pre><code>[@tpu]</code></pre>"
            )),
            "{cited}"
        );
        assert!(
            cited.contains("<li id=\"ref-kung\">Kung, Systolic arrays, 1982. <a href=\"https://x.org/k\">https://x.org/k</a></li>"),
            "{cited}"
        );
        let none = Front::parse("doctype: design\n");
        assert_eq!(
            shell().cited(html, Some(&none)),
            html,
            "only a page that gives sources"
        );
        assert_eq!(Shell::plural("Study"), "Studies");
    }
}
