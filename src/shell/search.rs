//! Search, in the shell: the box in the bar, and the page of what it finds. A served site's server finds it; a
//! built site's page finds it in the browser, by `web/src/search.ts`, from the index beside it.

use std::fmt::Write;
use std::path::Path;

use super::{Links, Shell};
use crate::search::Hit;

impl Shell {
    /// The search box, in the bar: served, it asks the server, at the commit shown; built, the site's search page.
    pub(super) fn search_box(&self, at: &Path, query: &str) -> String {
        let (action, rev) = match self.links {
            Links::Served => (
                "/_search".to_string(),
                self.revision
                    .as_ref()
                    .map(|r| {
                        format!(
                            "<input type=\"hidden\" name=\"ref\" value=\"{}\">",
                            Self::escape(&r.name)
                        )
                    })
                    .unwrap_or_default(),
            ),
            Links::Built => (self.to(at, "_search.html"), String::new()),
        };
        format!(
            concat!(
                "<form class=\"search\" action=\"{action}\" role=\"search\">{rev}",
                "<input type=\"search\" name=\"q\" value=\"{query}\" placeholder=\"Search\" aria-label=\"Search\"></form>"
            ),
            action = action,
            rev = rev,
            query = Self::escape(query)
        )
    }

    /// What a search found, as a page's body: each page, linked to its best section, and a passage of it with
    /// the words marked.
    #[must_use]
    pub fn results(&self, query: &str, hits: &[Hit]) -> String {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        let summary = match hits.len() {
            _ if words.is_empty() => "Type a word or two above.".to_string(),
            0 => format!("Nothing matches “{}”.", Self::escape(query)),
            1 => format!("1 page matches “{}”.", Self::escape(query)),
            n => format!("{n} pages match “{}”.", Self::escape(query)),
        };
        let mut items = String::new();
        for hit in hits {
            let anchor = hit
                .anchor
                .as_ref()
                .map(|a| format!("#{}", Self::encode_part(a)))
                .unwrap_or_default();
            let under = if hit.heading.is_empty() || hit.heading == hit.title {
                String::new()
            } else {
                format!(
                    " <span class=\"muted\">› {}</span>",
                    Self::mark(&hit.heading, &words)
                )
            };
            let _ = write!(
                items,
                "<li><a href=\"{}{anchor}\"><strong>{}</strong>{under}</a><p>{}</p><small>{}</small></li>",
                self.to(Path::new(""), &Self::encode(&hit.path)),
                Self::mark(&hit.title, &words),
                Self::passage(&hit.text, &words),
                Self::escape(&hit.path.to_string_lossy())
            );
        }
        format!(
            "<h1>Search</h1>\n<p class=\"muted\" id=\"kb-search-said\">{summary}</p>\n<ul class=\"results\" id=\"kb-search-results\">{items}</ul>\n"
        )
    }

    /// A built site's search page, as a body: empty, for its script to fill from the site's index.
    #[must_use]
    pub fn search_page(&self, at: &Path) -> String {
        format!(
            concat!(
                "<h1>Search</h1>\n<p class=\"muted\" id=\"kb-search-said\">Type a word or two above.</p>\n",
                "<ul class=\"results\" id=\"kb-search-results\"></ul>\n",
                "<script src=\"{index}\"></script><script src=\"{script}\"></script>\n"
            ),
            index = self.to(at, "_search.js"),
            script = self.asset(at, super::SEARCH_PATH)
        )
    }

    /// About 220 characters of text around the first of the words, with every word marked.
    fn passage(text: &str, words: &[String]) -> String {
        let lower = text.to_lowercase();
        // Lowering a text may change where its characters are; then the passage is its start.
        let first = (lower.len() == text.len())
            .then(|| words.iter().filter_map(|w| lower.find(w.as_str())).min())
            .flatten()
            .unwrap_or(0);
        let floor = |mut i: usize| {
            while !text.is_char_boundary(i) {
                i -= 1;
            }
            i
        };
        let from = floor(first.saturating_sub(60).min(text.len()));
        let to = floor((from + 220).min(text.len()));
        let (pre, post) = (
            if from > 0 { "…" } else { "" },
            if to < text.len() { "…" } else { "" },
        );
        format!("{pre}{}{post}", Self::mark(text[from..to].trim(), words))
    }

    /// Text as HTML with each time a word is in it in `<mark>`.
    fn mark(text: &str, words: &[String]) -> String {
        let lower = text.to_lowercase();
        if lower.len() != text.len() {
            return Self::escape(text);
        }
        let (mut out, mut i) = (String::new(), 0);
        while let Some(c) = text[i..].chars().next() {
            let word = words.iter().find(|w| {
                !w.is_empty()
                    && lower[i..].starts_with(w.as_str())
                    && text.is_char_boundary(i + w.len())
            });
            let len = word.map_or(c.len_utf8(), String::len);
            let piece = Self::escape(&text[i..i + len]);
            if word.is_some() {
                let _ = write!(out, "<mark>{piece}</mark>");
            } else {
                out.push_str(&piece);
            }
            i += len;
        }
        out
    }
}
