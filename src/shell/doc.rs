//! What a page is, in its shell: the line under its title, from its front matter, that says its doctype and
//! number, as Design-12, its status, and the fields its doctype shows, in the order git-kb.toml gives them. A value
//! that names a page of the site, by its path or its title, links to it.

use std::path::Path;

use super::Shell;
use crate::front::{Front, Value};
use crate::nav::Pages;

impl Shell {
    /// The line under a page's title that says what it is; none for a page without front matter.
    #[must_use]
    pub fn doc_head(&self, at: &Path, front: Option<&Front>, pages: &Pages) -> String {
        let Some(front) = front else {
            return String::new();
        };
        if let Some(error) = &front.error {
            return format!(
                "<p class=\"doc-head unread\" title=\"{}\">Its front matter could not be read</p>\n",
                Self::escape(error)
            );
        }
        let id = front.doctype();
        let doctype = id.and_then(|id| self.doctypes.get(id));
        let mut parts = vec![];
        if let Some(id) = id {
            let name = doctype.and_then(|d| d.name.as_deref()).unwrap_or(id);
            let numbered = front
                .number()
                .map_or_else(|| name.to_string(), |n| format!("{name}-{n}"));
            let unknown = if doctype.is_none() {
                " title=\"No such doctype in git-kb.toml\""
            } else {
                ""
            };
            parts.push(format!(
                "<span class=\"doctype\"{unknown}>{}</span>",
                Self::escape(&numbered)
            ));
        }
        if let Some(status) = front.text("status") {
            let known = doctype
                .is_none_or(|d| d.statuses.is_empty() || d.statuses.iter().any(|s| s == status));
            parts.push(format!(
                "<span class=\"status status-{}{}\"{}>{}</span>",
                Self::escape(&status.to_lowercase().replace(' ', "-")),
                if known { "" } else { " unknown" },
                if known {
                    ""
                } else {
                    " title=\"Not one of its doctype's statuses\""
                },
                Self::escape(status)
            ));
        }
        // The doctype's fields, in its order; or, of a page of no doctype git-kb.toml has, all of them.
        let fields: Vec<&str> = match doctype {
            Some(d) => d.fields.iter().map(String::as_str).collect(),
            None => front.fields.iter().map(|(k, _)| k.as_str()).collect(),
        };
        for field in fields {
            if matches!(field, "doctype" | "number" | "status") {
                continue;
            }
            let shown = match front.get(field) {
                Some(Value::Text(text)) if !text.is_empty() => self.named(at, text, pages),
                Some(Value::List(items)) if !items.is_empty() => items
                    .iter()
                    .map(|item| self.named(at, item, pages))
                    .collect::<Vec<_>>()
                    .join(", "),
                _ => continue,
            };
            let mut label = field.replace(['-', '_'], " ");
            label[..1].make_ascii_uppercase();
            parts.push(format!(
                "<span class=\"field\"><span class=\"label\">{}</span> {shown}</span>",
                Self::escape(&label)
            ));
        }
        if parts.is_empty() {
            return String::new();
        }
        format!("<p class=\"doc-head\">{}</p>\n", parts.join(""))
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
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use super::*;
    use crate::config::DoctypeConfig;
    use crate::render::Renderer;
    use crate::shell::Links;

    fn shell() -> Shell {
        let design = DoctypeConfig {
            name: Some("Design".into()),
            statuses: vec!["draft".into(), "review".into()],
            fields: vec!["status".into(), "authors".into(), "replaces".into()],
            ..DoctypeConfig::default()
        };
        Shell::new("Site".into(), Links::Built).typing(BTreeMap::from([("design".into(), design)]))
    }

    fn pages() -> Pages {
        let files = [PathBuf::from("designs/old.md")];
        Pages::new(&files, &Renderer::new(), |_| Some("# Old tiling\n".into()))
    }

    #[test]
    fn a_page_s_head_says_its_doctype_number_status_and_fields_linking_pages() {
        let front = Front::parse(
            "doctype: design\nnumber: 12\nstatus: review\nauthors: [Ada, Grace]\nreplaces: designs/old.md\nhidden: x\n",
        );
        let head = shell().doc_head(Path::new("designs"), Some(&front), &pages());
        assert_eq!(
            head,
            concat!(
                "<p class=\"doc-head\"><span class=\"doctype\">Design-12</span>",
                "<span class=\"status status-review\">review</span>",
                "<span class=\"field\"><span class=\"label\">Authors</span> Ada, Grace</span>",
                "<span class=\"field\"><span class=\"label\">Replaces</span> <a href=\"../designs/old.html\">Old tiling</a></span></p>\n"
            )
        );
    }

    #[test]
    fn what_its_doctype_does_not_know_is_said_and_an_unread_front_matter_too() {
        let odd = Front::parse("doctype: design\nstatus: shipped\n");
        let head = shell().doc_head(Path::new(""), Some(&odd), &pages());
        assert!(head.contains("status status-shipped unknown"), "{head}");
        let none = Front::parse("doctype: memo\nowner: Ada\n");
        let head = shell().doc_head(Path::new(""), Some(&none), &pages());
        assert!(
            head.contains("title=\"No such doctype in git-kb.toml\">memo</span>")
                && head.contains("Owner</span> Ada"),
            "a doctype not declared shows all its fields: {head}"
        );
        let broken = Front::parse("status: [unclosed\n");
        assert!(
            shell()
                .doc_head(Path::new(""), Some(&broken), &pages())
                .contains("could not be read")
        );
        assert_eq!(shell().doc_head(Path::new(""), None, &pages()), "");
    }
}
