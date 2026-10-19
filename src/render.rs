//! Markdown to HTML: CommonMark with GitHub's tables, footnotes, strikethrough and task lists, and
//! a GitHub-style id on every heading, so that a page can be linked into and a table of contents
//! built for it.
//!

use std::collections::HashMap;

use pulldown_cmark::{CowStr, Event, HeadingLevel, Options, Parser, Tag, TagEnd, html};

/// A page rendered: its HTML, and what is known of it from its headings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub html: String,
    /// The text of its first top-level heading, if it has one.
    pub title: Option<String>,
    /// Every heading, in order.
    pub headings: Vec<Heading>,
}

/// A heading, with the id it is linked to by, as `page.md#id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    pub level: HeadingLevel,
    pub id: String,
    /// Its text, without markup.
    pub text: String,
}

impl Heading {
    /// The text of a heading's events, without their markup: text and code, but not raw HTML.
    fn plain_text(events: &[Event]) -> String {
        events.iter().fold(String::new(), |mut s, e| {
            if let Event::Text(t) | Event::Code(t) = e {
                s.push_str(t);
            }
            s
        })
    }
}

/// Renders Markdown as HTML. What it holds is how it renders, so that what later features need, such as a
/// highlighter, is made once and kept, not made again for every page.
#[derive(Debug, Clone)]
pub struct Renderer {
    options: Options,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer {
    /// A renderer of CommonMark, GitHub's extensions, and `{#id}` after a heading to choose its id.
    #[must_use]
    pub fn new() -> Self {
        let options = Options::ENABLE_TABLES
            | Options::ENABLE_FOOTNOTES
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_HEADING_ATTRIBUTES;
        Self { options }
    }

    /// The Markdown it reads, for anything else that must read a page as it does.
    #[must_use]
    pub fn options(&self) -> Options {
        self.options
    }

    /// Renders Markdown as HTML.
    #[must_use]
    pub fn render(&self, markdown: &str) -> Page {
        let mut ids = Ids::default();
        let mut headings = vec![];
        let mut out = vec![];
        let mut events = Parser::new_ext(markdown, self.options);
        while let Some(event) = events.next() {
            let Event::Start(Tag::Heading {
                level,
                id,
                classes,
                attrs,
            }) = event
            else {
                out.push(event);
                continue;
            };

            let inner: Vec<Event> = events
                .by_ref()
                .take_while(|e| !matches!(e, Event::End(TagEnd::Heading(_))))
                .collect();
            let text = Heading::plain_text(&inner);
            let id = ids.claim(id, &text);
            headings.push(Heading {
                level,
                id: id.to_string(),
                text: text.trim().to_string(),
            });
            out.push(Event::Start(Tag::Heading {
                level,
                id: Some(id),
                classes,
                attrs,
            }));
            out.extend(inner);
            out.push(Event::End(TagEnd::Heading(level)));
        }
        let mut page = String::new();
        html::push_html(&mut page, out.into_iter());
        let title = headings
            .iter()
            .find(|h| h.level == HeadingLevel::H1 && !h.text.is_empty())
            .map(|h| h.text.clone());
        Page {
            html: page,
            title,
            headings,
        }
    }
}

/// The ids given out on a page, so that no two headings share one.
#[derive(Default)]
struct Ids {
    /// How many times each id has been given.
    seen: HashMap<String, usize>,
}

impl Ids {
    /// A heading's id: the one its `{#id}` chose, as it is, or else a slug of its text, numbered when the slug is
    /// taken, as GitHub numbers them: `intro`, `intro-1`, `intro-2`.
    fn claim<'a>(&mut self, chosen: Option<CowStr<'a>>, text: &str) -> CowStr<'a> {
        if let Some(id) = chosen {
            *self.seen.entry(id.to_string()).or_default() += 1;
            return id;
        }
        let base = Self::slug(text);
        let mut id = base.clone();
        let mut n = self.seen.get(&base).copied().unwrap_or(0);
        while self.seen.contains_key(&id) {
            id = format!("{base}-{n}");
            n += 1;
        }
        *self.seen.entry(base).or_default() += 1;
        self.seen.entry(id.clone()).or_default();
        id.into()
    }

    /// A GitHub-style slug: lowercase, with letters, digits, `-` and `_` kept, spaces as `-`, and the rest dropped.
    fn slug(text: &str) -> String {
        text.trim()
            .to_lowercase()
            .chars()
            .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | ' '))
            .map(|c| if c == ' ' { '-' } else { c })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Page {
        fn ids(&self) -> Vec<&str> {
            self.headings.iter().map(|h| h.id.as_str()).collect()
        }
    }

    #[test]
    fn headings_get_github_slugs() {
        assert_eq!(
            Renderer::new()
                .render("# Hello, World!\n## The `mat_array` unit\n### Ünïcode stays")
                .ids(),
            ["hello-world", "the-mat_array-unit", "ünïcode-stays"]
        );
    }

    #[test]
    fn repeated_headings_are_numbered() {
        assert_eq!(
            Renderer::new().render("# Intro\n# Intro\n## Intro").ids(),
            ["intro", "intro-1", "intro-2"]
        );
    }

    #[test]
    fn a_numbered_slug_never_takes_one_already_given() {
        assert_eq!(
            Renderer::new().render("# Intro 1\n# Intro\n# Intro").ids(),
            ["intro-1", "intro", "intro-2"]
        );
    }

    #[test]
    fn a_chosen_id_is_kept_and_counted() {
        assert_eq!(
            Renderer::new()
                .render("# Setup {#install}\n# Install")
                .ids(),
            ["install", "install-1"]
        );
    }

    #[test]
    fn ids_are_in_the_html() {
        let page = Renderer::new().render("# Data life cycle\n\nText.");
        assert!(
            page.html
                .contains(r#"<h1 id="data-life-cycle">Data life cycle</h1>"#),
            "{}",
            page.html
        );
    }

    #[test]
    fn the_title_is_the_first_top_level_heading() {
        assert_eq!(
            Renderer::new()
                .render("## Before\n# Architecture\n# Second")
                .title
                .as_deref(),
            Some("Architecture")
        );
        assert_eq!(Renderer::new().render("## Only a subheading").title, None);
        assert_eq!(Renderer::new().render("Just text.").title, None);
    }

    #[test]
    fn heading_text_has_no_markup() {
        let page = Renderer::new().render("# The **fused** _epilogue_ [link](x.md)");
        assert_eq!(page.headings[0].text, "The fused epilogue link");
        assert_eq!(page.headings[0].level, HeadingLevel::H1);
    }

    #[test]
    fn github_extensions_render() {
        let html = Renderer::new().render("| a | b |\n|---|---|\n| 1 | 2 |\n\n~~gone~~\n\n- [x] done\n\nNote[^1].\n\n[^1]: A footnote.").html;
        for wanted in [
            "<table>",
            "<td>1</td>",
            "<del>gone</del>",
            r#"type="checkbox""#,
            "footnote-definition",
        ] {
            assert!(html.contains(wanted), "{wanted} missing from {html}");
        }
    }

    #[test]
    fn raw_html_passes_through() {
        assert!(
            Renderer::new()
                .render("<details><summary>More</summary>\n\nHidden.\n\n</details>")
                .html
                .contains("<details>")
        );
    }
}
