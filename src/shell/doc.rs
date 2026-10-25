//! What a page is, in its shell: the line under its title, from its front matter, that says its doctype and
//! number, as Design-12, its status, and the fields its doctype shows, in the order git-kb.toml gives them. A value
//! that names a page of the site, by its path or its title, links to it.

use std::fmt::Write;
use std::path::Path;

use super::{Links, Shell};
use crate::front::{Front, Value};
use crate::git::Change;
use crate::nav::Pages;

impl Shell {
    /// The lines under a page's title that say what it is, and what replaces it, if a page says it does; none for a
    /// page without front matter that nothing replaces.
    #[must_use]
    pub fn doc_head(
        &self,
        at: &Path,
        page: &Path,
        pages: &Pages,
        since: Option<&Change>,
    ) -> String {
        let replaced = pages.replacing(page);
        let banner = if replaced.is_empty() {
            String::new()
        } else {
            let by: Vec<String> = replaced
                .iter()
                .map(|(other, title)| {
                    format!(
                        "<a href=\"{}\">{}</a>",
                        self.link(at, &other.with_extension("html")),
                        Self::escape(title)
                    )
                })
                .collect();
            format!(
                "<p class=\"replaced-by\">Replaced by {}</p>\n",
                by.join(", ")
            )
        };
        banner + &self.what_it_is(at, page, pages.front(page), pages, since)
    }

    /// The line under a page's title that says what it is, from its front matter; and, given the commit that last
    /// changed its status, since when, by whom.
    fn what_it_is(
        &self,
        at: &Path,
        page: &Path,
        front: Option<&Front>,
        pages: &Pages,
        since: Option<&Change>,
    ) -> String {
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
                "<a class=\"doctype\" href=\"{}\"{unknown}>{}</a>",
                self.doctype_href(at, id),
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
            if let Some(change) = since {
                let (when, commit) = self.when_and_commit(page, change);
                parts.push(format!(
                    "<span class=\"since\">since {when}, by {} in {commit}</span>",
                    Self::escape(&change.author)
                ));
            }
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

    /// A doctype's list of pages, as a page's body: a row of each, by number, with its title, status and the
    /// fields its doctype shows; and, when its pages have statuses, a choice of which to show.
    #[must_use]
    pub fn doctype_list(&self, at: &Path, id: &str, pages: &Pages) -> String {
        let doctype = self.doctypes.get(id);
        let name = doctype.and_then(|d| d.name.as_deref()).unwrap_or(id);
        let fields: Vec<&str> = doctype
            .map(|d| {
                d.fields
                    .iter()
                    .map(String::as_str)
                    .filter(|f| *f != "status")
                    .collect()
            })
            .unwrap_or_default();
        let found = pages.of_doctype(id);
        let mut statuses: Vec<&str> = doctype
            .map(|d| d.statuses.iter().map(String::as_str).collect())
            .unwrap_or_default();
        for (_, _, front) in &found {
            if let Some(status) = front.text("status")
                && !statuses.contains(&status)
            {
                statuses.push(status);
            }
        }
        let class = |status: &str| Self::status_class(status);
        let choice = Self::status_choice(&statuses);
        let mut head = String::from("<tr><th>#</th><th>Title</th>");
        if !statuses.is_empty() {
            head.push_str("<th>Status</th>");
        }
        for field in &fields {
            let mut label = field.replace(['-', '_'], " ");
            label[..1].make_ascii_uppercase();
            let _ = write!(head, "<th>{}</th>", Self::escape(&label));
        }
        head.push_str("</tr>");
        let mut rows = String::new();
        for (path, title, front) in &found {
            let status = front.text("status").unwrap_or_default();
            let _ = write!(
                rows,
                "<tr data-status=\"{}\"><td>{}</td><td><a href=\"{}\">{}</a></td>",
                class(status),
                front
                    .number()
                    .map_or_else(String::new, |n| format!("{}-{n}", Self::escape(name))),
                self.link(at, &path.with_extension("html")),
                Self::escape(title)
            );
            if !statuses.is_empty() {
                let _ = write!(
                    rows,
                    "<td><span class=\"status status-{}\">{}</span></td>",
                    class(status),
                    Self::escape(status)
                );
            }
            for field in &fields {
                let shown = match front.get(field) {
                    Some(Value::Text(text)) => self.named(at, text, pages),
                    Some(Value::List(items)) => items
                        .iter()
                        .map(|item| self.named(at, item, pages))
                        .collect::<Vec<_>>()
                        .join(", "),
                    _ => String::new(),
                };
                let _ = write!(rows, "<td>{shown}</td>");
            }
            rows.push_str("</tr>\n");
        }
        let empty = if found.is_empty() {
            format!(
                "<p class=\"muted\">No pages say they are a {} yet.</p>\n",
                Self::escape(name)
            )
        } else {
            String::new()
        };
        format!(
            "<h1>{}</h1>\n{choice}{empty}<table class=\"doc-list\"><thead>{head}</thead><tbody>\n{rows}</tbody></table>\n",
            Self::escape(&self.doctype_title(id))
        )
    }

    /// What a page is, for its card in a folder's list: its doctype and number, and its status.
    #[must_use]
    pub fn card_badges(&self, front: Option<&Front>) -> String {
        let Some(front) = front.filter(|f| f.error.is_none()) else {
            return String::new();
        };
        let mut badges = String::new();
        if let Some(id) = front.doctype() {
            let name = self
                .doctypes
                .get(id)
                .and_then(|d| d.name.as_deref())
                .unwrap_or(id);
            let numbered = front
                .number()
                .map_or_else(|| name.to_string(), |n| format!("{name}-{n}"));
            let _ = write!(
                badges,
                "<b class=\"doctype\">{}</b>",
                Self::escape(&numbered)
            );
        }
        if let Some(status) = front.text("status") {
            let _ = write!(
                badges,
                "<b class=\"status status-{}\">{}</b>",
                Self::escape(&status.to_lowercase().replace(' ', "-")),
                Self::escape(status)
            );
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

    /// A status, as a class: `status-<it>`'s end.
    fn status_class(status: &str) -> String {
        Self::escape(&status.to_lowercase().replace(' ', "-"))
    }

    /// Which statuses a doctype's list shows: all, or one; by a choice CSS reads, so that it needs no script, and a
    /// built site has it too.
    fn status_choice(statuses: &[&str]) -> String {
        if statuses.is_empty() {
            return String::new();
        }
        let mut choice = String::from(
            "<p class=\"doc-filter\"><label><input type=\"radio\" name=\"status\" value=\"\" checked> All</label>",
        );
        for status in statuses {
            let _ = write!(
                choice,
                "<label><input type=\"radio\" name=\"status\" value=\"{}\"> {}</label>",
                Self::status_class(status),
                Self::escape(status)
            );
        }
        choice.push_str("</p>\n<style>");
        for status in statuses {
            let status = Self::status_class(status);
            let _ = write!(
                choice,
                ".doc-filter:has([value=\"{status}\"]:checked) ~ .doc-list tbody tr:not([data-status=\"{status}\"]) {{ display: none; }}"
            );
        }
        choice + "</style>\n"
    }

    /// A page's rendered body with its citations, for a doctype that cites: each `[@key]`, or `[@a; @b]`, a link
    /// to its source, numbered in the order they are first cited, and a list of them at its end, from the
    /// `sources` of its front matter. A key it has no source for is marked. Code is left as it is.
    #[must_use]
    pub fn cited(&self, html: &str, front: Option<&Front>) -> String {
        let Some(front) = front else {
            return html.to_string();
        };
        let cites = front
            .doctype()
            .and_then(|id| self.doctypes.get(id))
            .is_some_and(|d| d.citations);
        if !cites {
            return html.to_string();
        }
        let sources = match front.get("sources") {
            Some(Value::Map(sources)) => sources.as_slice(),
            _ => &[],
        };
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
                out.push_str(&Self::cite_in(text, sources, &mut order));
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

    /// The ids of the doctypes the settings have.
    #[must_use]
    pub fn doctype_ids(&self) -> Vec<String> {
        self.doctypes.keys().cloned().collect()
    }

    /// The title of a doctype's list of pages: `Designs`.
    #[must_use]
    pub fn doctype_title(&self, id: &str) -> String {
        Self::plural(
            self.doctypes
                .get(id)
                .and_then(|d| d.name.as_deref())
                .unwrap_or(id),
        )
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
        let survey = DoctypeConfig {
            name: Some("Survey".into()),
            citations: true,
            ..DoctypeConfig::default()
        };
        let design = DoctypeConfig {
            name: Some("Design".into()),
            statuses: vec!["draft".into(), "review".into()],
            fields: vec!["status".into(), "authors".into(), "replaces".into()],
            ..DoctypeConfig::default()
        };
        Shell::new("Site".into(), Links::Built).typing(BTreeMap::from([
            ("design".into(), design),
            ("survey".into(), survey),
        ]))
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
        let head = shell().what_it_is(
            Path::new("designs"),
            Path::new("designs/x.md"),
            Some(&front),
            &pages(),
            None,
        );
        assert_eq!(
            head,
            concat!(
                "<p class=\"doc-head\"><a class=\"doctype\" href=\"../_doctype-design.html\">Design-12</a>",
                "<span class=\"status status-review\">review</span>",
                "<span class=\"field\"><span class=\"label\">Authors</span> Ada, Grace</span>",
                "<span class=\"field\"><span class=\"label\">Replaces</span> <a href=\"../designs/old.html\">Old tiling</a></span></p>\n"
            )
        );
    }

    #[test]
    fn what_its_doctype_does_not_know_is_said_and_an_unread_front_matter_too() {
        let odd = Front::parse("doctype: design\nstatus: shipped\n");
        let head = shell().what_it_is(Path::new(""), Path::new("x.md"), Some(&odd), &pages(), None);
        assert!(head.contains("status status-shipped unknown"), "{head}");
        let none = Front::parse("doctype: memo\nowner: Ada\n");
        let head = shell().what_it_is(
            Path::new(""),
            Path::new("x.md"),
            Some(&none),
            &pages(),
            None,
        );
        assert!(
            head.contains("title=\"No such doctype in git-kb.toml\">memo</a>")
                && head.contains("Owner</span> Ada"),
            "a doctype not declared shows all its fields: {head}"
        );
        let broken = Front::parse("status: [unclosed\n");
        assert!(
            shell()
                .what_it_is(
                    Path::new(""),
                    Path::new("x.md"),
                    Some(&broken),
                    &pages(),
                    None
                )
                .contains("could not be read")
        );
        assert_eq!(
            shell().what_it_is(Path::new(""), Path::new("x.md"), None, &pages(), None),
            ""
        );
    }

    #[test]
    fn a_doctype_lists_its_pages_by_number_with_a_choice_of_status() {
        let files = [
            PathBuf::from("b.md"),
            PathBuf::from("a.md"),
            PathBuf::from("c.md"),
        ];
        let texts = BTreeMap::from([
            (
                "a.md",
                "---\ndoctype: design\nnumber: 2\nstatus: draft\nauthors: [Ada]\n---\n# Second\n",
            ),
            (
                "b.md",
                "---\ndoctype: design\nnumber: 1\nstatus: review\n---\n# First\n",
            ),
            ("c.md", "---\ndoctype: guide\n---\n# Not a design\n"),
        ]);
        let pages = Pages::new(&files, &Renderer::new(), |f| {
            texts.get(f.to_str().unwrap()).map(|t| (*t).to_string())
        });
        let list = shell().doctype_list(Path::new(""), "design", &pages);
        assert!(list.starts_with("<h1>Designs</h1>"), "{list}");
        let (first, second) = (list.find("First").unwrap(), list.find("Second").unwrap());
        assert!(first < second && !list.contains("Not a design"), "{list}");
        assert!(
            list.contains(
                "<tr data-status=\"draft\"><td>Design-2</td><td><a href=\"./a.html\">Second</a></td>"
            ),
            "{list}"
        );
        assert!(
            list.contains("<td>Ada</td>"),
            "its doctype's fields: {list}"
        );
        assert!(
            list.contains(
                "[value=\"review\"]:checked) ~ .doc-list tbody tr:not([data-status=\"review\"])"
            ),
            "{list}"
        );
        assert_eq!(Shell::plural("Study"), "Studies");
        assert_eq!(Shell::plural("Survey"), "Surveys");
    }

    #[test]
    fn a_page_another_says_it_replaces_says_what_replaces_it() {
        let files = [PathBuf::from("old.md"), PathBuf::from("new.md")];
        let pages = Pages::new(&files, &Renderer::new(), |f| {
            Some(if f == Path::new("new.md") {
                "---\ndoctype: design\nreplaces: Old tiling\n---\n# New tiling\n".into()
            } else {
                "# Old tiling\n".into()
            })
        });
        let head = shell().doc_head(Path::new(""), Path::new("old.md"), &pages, None);
        assert_eq!(
            head,
            "<p class=\"replaced-by\">Replaced by <a href=\"./new.html\">New tiling</a></p>\n",
            "by its title, as here, or its path"
        );
        assert!(
            !shell()
                .doc_head(Path::new(""), Path::new("new.md"), &pages, None)
                .contains("replaced-by")
        );
    }

    #[test]
    fn citations_are_numbered_by_first_use_with_their_sources_at_the_end() {
        let front = Front::parse(
            "doctype: survey\nsources:\n  kung: \"Kung, Systolic arrays, 1982. https://x.org/k\"\n  tpu: \"Jouppi, TPU, 2017\"\n",
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
            cited.contains("<li id=\"ref-tpu\">Jouppi, TPU, 2017</li>")
                && cited.contains("<li id=\"ref-kung\">Kung, Systolic arrays, 1982. <a href=\"https://x.org/k\">https://x.org/k</a></li>")
                && cited.contains("<li id=\"ref-nope\">No such source"),
            "{cited}"
        );
        let design = Front::parse("doctype: design\n");
        assert_eq!(
            shell().cited(html, Some(&design)),
            html,
            "only a doctype that cites"
        );
    }
}
