//! Markdown to HTML: CommonMark with GitHub's tables, footnotes, strikethrough and task lists, and
//! a GitHub-style id on every heading, so that a page can be linked into and a table of contents
//! built for it.
//!

use std::collections::HashMap;
use std::ops::Range;
use std::sync::OnceLock;

use pulldown_cmark::{
    CodeBlockKind, CowStr, Event, HeadingLevel, Options, Parser, Tag, TagEnd, html,
};
use syntect::html::{ClassStyle, ClassedHTMLGenerator};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

/// The syntaxes highlighter is loaded once and shared by every renderer.
static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();

/// A page rendered: its HTML, and what is known of it from its headings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub html: String,
    /// The text of its first top-level heading, if it has one.
    pub title: Option<String>,
    /// Every heading, in order.
    pub headings: Vec<Heading>,
}

/// A part of a page under one heading, or above the first: the heading's id and text, and the section's own text,
/// without markup, as search reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub id: Option<String>,
    pub heading: String,
    pub text: String,
}

/// A heading, with the id it is linked to by, as `page.md#id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    pub level: HeadingLevel,
    pub id: String,
    /// Its text, without markup.
    pub text: String,
}

impl Page {
    /// A page from its events, as rendered, and its headings.
    fn new(events: Vec<Event>, headings: Vec<Heading>) -> Self {
        let mut html = String::new();
        html::push_html(&mut html, events.into_iter());
        let title = headings
            .iter()
            .find(|h| h.level == HeadingLevel::H1 && !h.text.is_empty())
            .map(|h| h.text.clone());
        Self {
            html,
            title,
            headings,
        }
    }
}

impl Heading {
    /// The heading's events, with its id: its start, what is inside it, and its end.
    fn events<'a>(
        &self,
        classes: Vec<CowStr<'a>>,
        attrs: Vec<(CowStr<'a>, Option<CowStr<'a>>)>,
        inner: Vec<Event<'a>>,
    ) -> Vec<Event<'a>> {
        let start = Tag::Heading {
            level: self.level,
            id: Some(self.id.clone().into()),
            classes,
            attrs,
        };
        Renderer::wrap(start, inner, TagEnd::Heading(self.level))
    }
}

/// Where a link's or image's target is now, given it as written, if it has moved.
pub type Moved<'a> = &'a dyn Fn(&str) -> Option<String>;

/// A mark beside a top-level block of a page: a class for the block's wrapper, and the HTML that goes in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mark {
    pub class: &'static str,
    pub html: String,
}

/// The mark for the block at a range of a page's text, if it has one.
pub type Marks<'a> = &'a dyn Fn(std::ops::Range<usize>) -> Option<Mark>;

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
        self.render_with(markdown, &|_| None)
    }

    /// Where each of a page's top-level blocks is in its text: each paragraph, heading, list, table, code block
    /// and so on, as a range of its bytes.
    #[must_use]
    pub fn block_ranges(&self, markdown: &str) -> Vec<Range<usize>> {
        let mut depth = 0usize;
        let mut out = vec![];
        for (event, range) in Parser::new_ext(markdown, self.options).into_offset_iter() {
            match event {
                Event::Start(_) => {
                    if depth == 0 {
                        out.push(range);
                    }
                    depth += 1;
                }
                Event::End(_) => depth -= 1,
                Event::Rule | Event::Html(_) if depth == 0 => out.push(range),
                _ => {}
            }
        }
        out
    }

    /// The source of each of a page's top-level blocks, for comparing two versions of it block by block.
    #[must_use]
    pub fn blocks<'t>(&self, markdown: &'t str) -> Vec<&'t str> {
        self.block_ranges(markdown)
            .into_iter()
            .map(|r| markdown[r].trim_end())
            .collect()
    }

    /// A page's link reference definitions, as Markdown, so that a block of it rendered alone keeps its links.
    #[must_use]
    pub fn references(&self, markdown: &str) -> String {
        let mut parser = Parser::new_ext(markdown, self.options);
        parser.by_ref().for_each(drop);
        parser
            .reference_definitions()
            .iter()
            .map(|(label, def)| match &def.title {
                Some(title) => format!(
                    "[{label}]: <{}> \"{}\"\n",
                    def.dest,
                    title.replace('"', "\\\"")
                ),
                None => format!("[{label}]: <{}>\n", def.dest),
            })
            .collect()
    }

    /// A page's Markdown, highlighted, as its source is shown beside it.
    #[must_use]
    pub fn source(&self, markdown: &str) -> String {
        let code = Self::highlight("md", markdown).unwrap_or_else(|| {
            markdown
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
        });
        format!("<pre class=\"source\"><code>{code}</code></pre>\n")
    }

    /// Where a page links: each link's target, in order, with the heading it sits under, as its id and text; none
    /// above the first heading. A link in a heading sits under it.
    #[must_use]
    pub fn link_sites(&self, markdown: &str) -> Vec<(String, Option<(String, String)>)> {
        let (mut out, mut ids, mut under) = (vec![], Ids::default(), None);
        let mut events = Parser::new_ext(markdown, self.options);
        while let Some(event) = events.next() {
            match event {
                Event::Start(Tag::Heading { level, id, .. }) => {
                    let inner = Self::inside(&mut events);
                    let heading = ids.heading(level, id, &inner);
                    let here = Some((heading.id, heading.text));
                    for e in &inner {
                        if let Event::Start(Tag::Link { dest_url, .. }) = e {
                            out.push((dest_url.to_string(), here.clone()));
                        }
                    }
                    under = here;
                }
                Event::Start(Tag::Link { dest_url, .. }) => {
                    out.push((dest_url.to_string(), under.clone()));
                }
                _ => {}
            }
        }
        out
    }

    /// A page's sections, each its heading and its text, without markup: the first, above any heading, has none.
    #[must_use]
    pub fn sections(&self, markdown: &str) -> Vec<Section> {
        let mut out = vec![Section {
            id: None,
            heading: String::new(),
            text: String::new(),
        }];
        let mut ids = Ids::default();
        let mut events = Parser::new_ext(markdown, self.options);
        while let Some(event) = events.next() {
            match event {
                Event::Start(Tag::Heading { level, id, .. }) => {
                    let inner = Self::inside(&mut events);
                    let heading = ids.heading(level, id, &inner);
                    out.push(Section {
                        id: Some(heading.id),
                        heading: heading.text,
                        text: String::new(),
                    });
                }
                Event::Text(t) | Event::Code(t) => {
                    if let Some(section) = out.last_mut() {
                        section.text.push_str(&t);
                    }
                }
                // Inline tags end inside a sentence; any other ends a block, so the next text starts a new word.
                Event::End(
                    TagEnd::Emphasis
                    | TagEnd::Strong
                    | TagEnd::Strikethrough
                    | TagEnd::Link
                    | TagEnd::Image,
                ) => {}
                Event::SoftBreak | Event::HardBreak | Event::End(_) => {
                    if let Some(section) = out.last_mut() {
                        section.text.push(' ');
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// A page's title, as `render` gives it, without rendering the rest: the text of its first top-level heading.
    #[must_use]
    pub fn title(&self, markdown: &str) -> Option<String> {
        let mut events = Parser::new_ext(markdown, self.options);
        while let Some(event) = events.next() {
            if let Event::Start(Tag::Heading {
                level: HeadingLevel::H1,
                ..
            }) = event
            {
                let text = Self::plain_text(&Self::inside(&mut events));
                let text = text.trim();
                if !text.is_empty() {
                    return Some(text.to_string());
                }
            }
        }
        None
    }

    /// Renders Markdown as HTML, with each link's and image's target where `moved` says it is now, if it
    /// says, as a built site's pages are `.html` files rather than `.md`.
    #[must_use]
    pub fn render_with(&self, markdown: &str, moved: Moved) -> Page {
        self.render_marked(markdown, moved, &|_| None)
    }

    /// The same, with each top-level block that `marks` gives a mark wrapped in `<div class="blk">`, holding
    /// its mark, so that the block can show it.
    #[must_use]
    pub fn render_marked(&self, markdown: &str, moved: Moved, marks: Marks) -> Page {
        self.rendered(markdown, moved, marks, false)
    }

    /// A reader's text, as a comment is, rendered: any HTML in it shown as text, so that it cannot run or
    /// restyle the page it is shown on.
    #[must_use]
    pub fn render_safe(&self, markdown: &str, moved: Moved) -> Page {
        self.rendered(markdown, moved, &|_| None, true)
    }

    /// Markdown rendered, its links and images where `moved` says, its blocks marked as `marks` says, and, when
    /// it is a reader's, its HTML as text.
    fn rendered(&self, markdown: &str, moved: Moved, marks: Marks, safe: bool) -> Page {
        let ranges = self.block_ranges(markdown);
        let (mut depth, mut block, mut open) = (0usize, 0usize, false);
        let mut ids = Ids::default();
        let mut headings = vec![];
        let mut out = vec![];
        let mut events = Parser::new_ext(markdown, self.options);
        while let Some(event) = events.next() {
            if depth == 0 && matches!(event, Event::Start(_) | Event::Rule | Event::Html(_)) {
                if let Some(mark) = ranges.get(block).cloned().and_then(marks) {
                    out.push(Event::Html(
                        format!("<div class=\"blk {}\">{}", mark.class, mark.html).into(),
                    ));
                    open = true;
                }
                block += 1;
            }
            // Headings and code blocks are read whole, below, so they leave the depth as it was.
            match &event {
                Event::Start(Tag::Heading { .. } | Tag::CodeBlock(_)) => {}
                Event::Start(_) => depth += 1,
                Event::End(_) => depth -= 1,
                _ => {}
            }
            match event {
                Event::Start(Tag::Heading {
                    level,
                    id,
                    classes,
                    attrs,
                }) => {
                    let inner = Self::inside(&mut events);
                    let heading = ids.heading(level, id, &inner);
                    out.extend(heading.events(classes, attrs, inner));
                    headings.push(heading);
                }
                Event::Start(Tag::CodeBlock(kind)) => {
                    let inner = Self::inside(&mut events);
                    out.extend(Self::code_block(kind, inner));
                }
                Event::Start(Tag::Link {
                    link_type,
                    dest_url,
                    title,
                    id,
                }) => out.push(Event::Start(Tag::Link {
                    link_type,
                    dest_url: Self::target(dest_url, moved),
                    title,
                    id,
                })),
                Event::Start(Tag::Image {
                    link_type,
                    dest_url,
                    title,
                    id,
                }) => out.push(Event::Start(Tag::Image {
                    link_type,
                    dest_url: Self::target(dest_url, moved),
                    title,
                    id,
                })),
                Event::Html(html) | Event::InlineHtml(html) if safe => out.push(Event::Text(html)),
                event => out.push(event),
            }
            if depth == 0 && open {
                out.push(Event::Html("</div>".into()));
                open = false;
            }
        }
        Page::new(out, headings)
    }

    /// A link's or image's target, or where `moved` says it is now.
    fn target<'a>(dest: CowStr<'a>, moved: Moved) -> CowStr<'a> {
        moved(&dest).map_or(dest, Into::into)
    }

    /// A code block: highlighted, when its language is known, or else as it was, for pulldown-cmark to
    /// escape.
    fn code_block<'a>(kind: CodeBlockKind<'a>, inner: Vec<Event<'a>>) -> Vec<Event<'a>> {
        let lang = match &kind {
            CodeBlockKind::Fenced(info) => Self::lang(info).to_string(),
            CodeBlockKind::Indented => String::new(),
        };
        match Self::highlight(&lang, &Self::plain_text(&inner)) {
            Some(html) => vec![Event::Html(
                format!(r#"<pre><code class="language-{lang}">{html}</code></pre>"#).into(),
            )],
            None => Self::wrap(Tag::CodeBlock(kind), inner, TagEnd::CodeBlock),
        }
    }

    /// The events inside a tag that has just started, up to its end, which is taken too but not
    /// returned.
    fn inside<'a>(events: &mut impl Iterator<Item = Event<'a>>) -> Vec<Event<'a>> {
        let mut depth = 0usize;
        events
            .take_while(|e| match e {
                Event::Start(_) => {
                    depth += 1;
                    true
                }
                Event::End(_) if depth == 0 => false,
                Event::End(_) => {
                    depth -= 1;
                    true
                }
                _ => true,
            })
            .collect()
    }

    /// A tag's events again: its start, what is inside it, and its end.
    fn wrap<'a>(start: Tag<'a>, inner: Vec<Event<'a>>, end: TagEnd) -> Vec<Event<'a>> {
        let mut events = Vec::with_capacity(inner.len() + 2);
        events.push(Event::Start(start));
        events.extend(inner);
        events.push(Event::End(end));
        events
    }

    /// The text of some events, without their markup: text and code, but not raw HTML.
    fn plain_text(events: &[Event]) -> String {
        events.iter().fold(String::new(), |mut s, e| {
            if let Event::Text(t) | Event::Code(t) = e {
                s.push_str(t);
            }
            s
        })
    }

    /// Code as HTML, its tokens in spans whose classes name what they are, as `hl-keyword` and
    /// `hl-string`, for a style sheet to color; or none, when the language is not known. A language
    /// is known by its name or its files' extension, as `rust` or `rs`.
    fn highlight(lang: &str, code: &str) -> Option<String> {
        const CLASSES: ClassStyle = ClassStyle::SpacedPrefixed { prefix: "hl-" };
        let syntaxes = SYNTAXES.get_or_init(SyntaxSet::load_defaults_newlines);
        let syntax = syntaxes.find_syntax_by_token(lang)?;
        let mut html = ClassedHTMLGenerator::new_with_class_style(syntax, syntaxes, CLASSES);
        for line in LinesWithEndings::from(code) {
            html.parse_html_for_line_which_includes_newline(line).ok()?;
        }
        Some(html.finalize())
    }

    /// A fence's language: the first word of its info, so that what follows it, as in
    /// ```` ```rust ignore ````, is left for other uses.
    fn lang(info: &str) -> &str {
        info.split_whitespace().next().unwrap_or("")
    }
}

/// The ids given out on a page, so that no two headings share one.
#[derive(Default)]
struct Ids {
    /// How many times each id has been given.
    seen: HashMap<String, usize>,
}

impl Ids {
    /// A heading at a level, from what is inside it: its text, and its id, claimed for it.
    fn heading(&mut self, level: HeadingLevel, chosen: Option<CowStr>, inner: &[Event]) -> Heading {
        let text = Renderer::plain_text(inner);
        let id = self.claim(chosen, &text);
        Heading {
            level,
            id,
            text: text.trim().to_string(),
        }
    }

    /// A heading's id: the one its `{#id}` chose, as it is, or else a slug of its text, numbered when the slug is
    /// taken, as GitHub numbers them: `intro`, `intro-1`, `intro-2`.
    fn claim(&mut self, chosen: Option<CowStr>, text: &str) -> String {
        if let Some(id) = chosen {
            *self.seen.entry(id.to_string()).or_default() += 1;
            return id.to_string();
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
        id
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
    fn a_page_is_its_blocks_and_keeps_its_references() {
        let renderer = Renderer::new();
        let page = "# Title\n\nA [link][x].\n\n- one\n- two\n\n---\n\n[x]: dma.md \"DMA\"\n";
        assert_eq!(
            renderer.blocks(page),
            ["# Title", "A [link][x].", "- one\n- two", "---"]
        );
        let references = renderer.references(page);
        assert_eq!(references, "[x]: <dma.md> \"DMA\"\n");
        let alone = renderer.render(&format!("A [link][x].\n\n{references}"));
        assert!(
            alone
                .html
                .contains(r#"<a href="dma.md" title="DMA">link</a>"#),
            "{}",
            alone.html
        );
    }

    #[test]
    fn each_block_marked_is_wrapped_with_its_mark() {
        let page = "# Title\n\nOne.\n\n- a\n- b\n\n```\ncode\n```\n";
        let html = Renderer::new()
            .render_marked(page, &|_| None, &|range| {
                Some(Mark {
                    class: "x",
                    html: format!("<i>{}</i>", range.start),
                })
            })
            .html;
        for wanted in [
            "<div class=\"blk x\"><i>0</i>\n<h1 id=\"title\">Title</h1>\n</div>",
            "<div class=\"blk x\"><i>9</i>\n<p>One.</p>\n</div>",
            "<li>b</li>\n</ul>\n</div>",
            "<i>",
        ] {
            assert!(html.contains(wanted), "{wanted} missing from {html}");
        }
        assert_eq!(html.matches("</div>").count(), 4, "{html}");
    }

    #[test]
    fn a_page_s_source_is_its_markdown_highlighted() {
        let source = Renderer::new().source("# Title\n\n<b> & *it*\n");
        assert!(
            source.starts_with(r#"<pre class="source"><code>"#),
            "{source}"
        );
        assert!(source.contains("hl-"), "highlighted: {source}");
        assert!(
            source.contains("&lt;") && source.contains("&amp;") && !source.contains("<b>"),
            "escaped: {source}"
        );
    }

    #[test]
    fn a_page_is_its_sections_under_its_headings() {
        let sections = Renderer::new().sections(
            "Intro **bold**.\n\n# Title\n\nA [link](x.md).\n\n## Sub part\n\n- one\n- two\n",
        );
        let said: Vec<(Option<&str>, &str, String)> = sections
            .iter()
            .map(|s| {
                (
                    s.id.as_deref(),
                    s.heading.as_str(),
                    s.text.split_whitespace().collect::<Vec<_>>().join(" "),
                )
            })
            .collect();
        assert_eq!(
            said,
            [
                (None, "", "Intro bold.".to_string()),
                (Some("title"), "Title", "A link.".to_string()),
                (Some("sub-part"), "Sub part", "one two".to_string()),
            ]
        );
    }

    #[test]
    fn a_page_s_links_are_under_their_headings() {
        let sites =
            Renderer::new().link_sites("[a](a.md)\n\n# Top [b](b.md)\n\nSee [c](c.md#x).\n");
        assert_eq!(
            sites,
            [
                ("a.md".to_string(), None),
                (
                    "b.md".to_string(),
                    Some(("top-b".to_string(), "Top b".to_string()))
                ),
                (
                    "c.md#x".to_string(),
                    Some(("top-b".to_string(), "Top b".to_string()))
                ),
            ]
        );
    }

    #[test]
    fn the_title_is_the_rendered_page_s() {
        let renderer = Renderer::new();
        for markdown in [
            "# The **fused** epilogue\n# Second",
            "Intro\n\n## Sub\n\n#  \n\n# Title",
            "Setext\n======\n",
            "No heading",
        ] {
            assert_eq!(
                renderer.title(markdown),
                renderer.render(markdown).title,
                "{markdown}"
            );
        }
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
    fn code_in_a_known_language_is_highlighted() {
        let html = Renderer::new()
            .render("```rust\nfn main() { let a = b < c; }\n```")
            .html;
        assert!(
            html.starts_with(r#"<pre><code class="language-rust">"#),
            "{html}"
        );
        assert!(
            html.contains(r#"<span class="hl-storage hl-type hl-function hl-rust">fn</span>"#),
            "{html}"
        );
        assert!(
            html.contains("&lt;"),
            "the < in the code is escaped: {html}"
        );
    }

    #[test]
    fn a_language_is_known_by_its_extension_too() {
        assert!(Renderer::highlight("rs", "fn main() {}").is_some());
        assert!(Renderer::highlight("rust", "fn main() {}").is_some());
        assert_eq!(Renderer::highlight("no-such-language", "x"), None);
    }

    #[test]
    fn code_in_an_unknown_language_is_only_escaped() {
        let html = Renderer::new()
            .render("```no-such-language\na < b && c\n```")
            .html;
        assert_eq!(
            html,
            "<pre><code class=\"language-no-such-language\">a &lt; b &amp;&amp; c\n</code></pre>\n"
        );
    }

    #[test]
    fn only_the_first_word_of_a_fence_names_its_language() {
        let html = Renderer::new()
            .render("```rust ignore\nfn f() {}\n```")
            .html;
        assert!(
            html.starts_with(r#"<pre><code class="language-rust">"#),
            "{html}"
        );
        assert!(html.contains("hl-"), "{html}");
    }

    #[test]
    fn indented_code_is_only_escaped() {
        assert_eq!(
            Renderer::new().render("    a < b\n").html,
            "<pre><code>a &lt; b\n</code></pre>\n"
        );
    }

    #[test]
    fn links_and_images_go_where_they_have_moved() {
        let moved = |dest: &str| dest.strip_suffix(".md").map(|p| format!("{p}.html"));
        let html = Renderer::new()
            .render_with(
                "[a](a.md) [web](https://x.org) ![i](i.png) [r][ref]\n\n[ref]: r.md",
                &moved,
            )
            .html;
        for wanted in [
            r#"href="a.html""#,
            r#"href="https://x.org""#,
            r#"src="i.png""#,
            r#"href="r.html""#,
        ] {
            assert!(html.contains(wanted), "{wanted} missing from {html}");
        }
    }

    #[test]
    fn without_a_move_links_are_as_written() {
        assert!(
            Renderer::new()
                .render("[a](a.md)")
                .html
                .contains(r#"href="a.md""#)
        );
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

    #[test]
    fn a_reader_s_html_is_shown_as_text() {
        let html = Renderer::new()
            .render_safe(
                "<script>alert(1)</script>\n\nSo <b>bold</b>, [a link](a.md).\n",
                &|_| None,
            )
            .html;
        assert!(
            !html.contains("<script>") && !html.contains("<b>"),
            "{html}"
        );
        assert!(html.contains("&lt;b&gt;bold&lt;/b&gt;"), "{html}");
        assert!(html.contains(r#"<a href="a.md">a link</a>"#), "{html}");
        assert!(
            Renderer::new()
                .render("So <b>bold</b>.")
                .html
                .contains("<b>bold</b>"),
            "a page's own HTML is kept"
        );
    }
}
