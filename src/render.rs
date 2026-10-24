//! Markdown to HTML: CommonMark with GitHub's tables, footnotes, strikethrough and task lists, and
//! a GitHub-style id on every heading, so that a page can be linked into and a table of contents
//! built for it.
//!

use std::collections::HashMap;
use std::fmt::Write;
use std::ops::Range;
use std::sync::OnceLock;

use pulldown_cmark::{
    CodeBlockKind, CowStr, Event, HeadingLevel, Options, Parser, Tag, TagEnd, html,
};
use syntect::html::{ClassStyle, ClassedHTMLGenerator, line_tokens_to_classed_spans};
use syntect::parsing::{ParseState, ScopeStack, ScopeStackOp, SyntaxSet};
use syntect::util::LinesWithEndings;

use crate::plugins::{self, Plugins};

/// The syntaxes highlighter is loaded once and shared by every renderer.
static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();

/// Code is marked with its syntax's scopes as classes, `hl-keyword` and the like, for a style sheet to color.
const CLASSES: ClassStyle = ClassStyle::SpacedPrefixed { prefix: "hl-" };

/// A page rendered: its HTML, and what is known of it from its headings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub html: String,
    /// Where in the HTML its title ends: after its first block, when that is a top-level heading, or else at 0.
    pub title_end: usize,
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
    fn new(mut events: Vec<Event>, headings: Vec<Heading>, title_end: usize) -> Self {
        let rest = events.split_off(title_end);
        let mut html = String::new();
        html::push_html(&mut html, events.into_iter());
        let title_end = html.len();
        html::push_html(&mut html, rest.into_iter());
        let title = headings
            .iter()
            .find(|h| h.level == HeadingLevel::H1 && !h.text.is_empty())
            .map(|h| h.text.clone());
        Self {
            html,
            title_end,
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

/// A diagram labelled on its fence, ```` ```tikz #fig:life "Data life cycle" ````: its id, its number on its
/// page, in the order its figures come, and its caption.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Figure {
    pub id: String,
    pub number: usize,
    pub caption: String,
}

impl Figure {
    /// A fence's label and caption, from its info after the language: `#fig:id` and `"caption"`, in either
    /// order; none when it has no `#fig:` label.
    #[must_use]
    pub fn label(info: &str) -> Option<(String, String)> {
        let rest = info
            .trim()
            .split_once(char::is_whitespace)
            .map_or("", |(_, r)| r);
        let id = rest
            .split_whitespace()
            .find_map(|t| t.strip_prefix('#'))?
            .trim_matches('"');
        let caption = match (rest.find('"'), rest.rfind('"')) {
            (Some(a), Some(b)) if b > a => &rest[a + 1..b],
            _ => "",
        };
        id.starts_with("fig:")
            .then(|| (id.to_string(), caption.to_string()))
    }
}

/// Another page's figure, for `@page.md#fig:id`, given the page as written and the figure's id: its href,
/// number and caption.
pub type Elsewhere<'a> = &'a dyn Fn(&str, &str) -> Option<(String, usize, String)>;

/// A mark beside a top-level block of a page: a class for the block's wrapper, and the HTML that goes in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mark {
    pub class: &'static str,
    pub html: String,
}

impl Mark {
    /// A marked block's wrapper, holding its mark, with the lines of the text it holds, for what follows the text,
    /// as live.js, to find it again.
    fn wrapper<'a>(&self, lines: &Range<usize>) -> Event<'a> {
        Event::Html(
            format!(
                "<div class=\"blk {}\" data-lines=\"{}-{}\">{}",
                self.class, lines.start, lines.end, self.html
            )
            .into(),
        )
    }
}

/// Where each line of a text starts, to tell which lines a range of its bytes spans.
struct Lines(Vec<usize>);

impl Lines {
    fn of(text: &str) -> Self {
        Self(
            std::iter::once(0)
                .chain(text.match_indices('\n').map(|(i, _)| i + 1))
                .collect(),
        )
    }

    /// The lines a range of the text spans, from 0, its last not among them.
    fn spanned(&self, range: &Range<usize>) -> Range<usize> {
        let line = |offset: usize| self.0.partition_point(|&s| s <= offset).saturating_sub(1);
        line(range.start)..line(range.end.saturating_sub(1).max(range.start)) + 1
    }
}

/// The mark for the block at a range of a page's text, if it has one.
pub type Marks<'a> = &'a dyn Fn(std::ops::Range<usize>) -> Option<Mark>;

/// Whether a link's target, as written, names a page that is not there.
pub type Broken<'a> = &'a dyn Fn(&str) -> bool;

/// What a page is rendered with, beyond its Markdown. Each is optional: without any, it renders as `render` does.
#[derive(Default, Clone, Copy)]
pub struct Opts<'a> {
    /// Where each link's and image's target is now.
    pub moved: Option<Moved<'a>>,
    /// The mark beside each top-level block, which is then wrapped in `<div class="blk">` holding it.
    pub gutter: Option<Marks<'a>>,
    /// Which links name a page that is not there, to be marked.
    pub broken: Option<Broken<'a>>,
    /// Whether the text is a reader's, as a comment is: its HTML is shown as text, and its `[[name]]` links are
    /// links, to `kb:name`, for `moved` to say where.
    pub safe: bool,
    /// Another page's figure, for `@page.md#fig:id`.
    pub elsewhere: Option<Elsewhere<'a>>,
    /// The page's figures, numbered, when what is rendered is a part of a page, as a block of a diff is; else the
    /// text's own are found.
    pub figures: Option<&'a [Figure]>,
}

/// Renders Markdown as HTML. What it holds is how it renders, so that what later features need, such as a
/// highlighter, is made once and kept, not made again for every page.
#[derive(Debug, Clone)]
pub struct Renderer {
    options: Options,
    /// What renders the blocks that Markdown alone does not.
    plugins: &'static Plugins,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer {
    /// A renderer of CommonMark, GitHub's extensions, and `{#id}` after a heading to choose its id, with the
    /// plugins git-kb.toml names.
    #[must_use]
    pub fn new() -> Self {
        Self::with_plugins(plugins::get())
    }

    /// The same, with these plugins; and of `$...$` as math, when one of them renders math.
    #[must_use]
    pub fn with_plugins(plugins: &'static Plugins) -> Self {
        let mut options = Options::ENABLE_TABLES
            | Options::ENABLE_FOOTNOTES
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_HEADING_ATTRIBUTES;
        if plugins.math().is_some() {
            options |= Options::ENABLE_MATH;
        }
        Self { options, plugins }
    }

    /// The Markdown it reads, for anything else that must read a page as it does.
    #[must_use]
    pub fn options(&self) -> Options {
        self.options
    }

    /// Renders Markdown as HTML.
    #[must_use]
    pub fn render(&self, markdown: &str) -> Page {
        self.render_opts(markdown, &Opts::default())
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

    /// A page's Markdown, highlighted, as its source is shown beside it: each line a span of its own, with its
    /// number, from 0, as `data-lines="i-j"`, so that the source can be commented on by line as the page is by
    /// block.
    #[must_use]
    pub fn source(&self, markdown: &str) -> String {
        let lines = Self::highlight_lines("md", markdown)
            .unwrap_or_else(|| LinesWithEndings::from(markdown).map(Self::escape).collect());
        let mut code = String::new();
        for (i, line) in lines.iter().enumerate() {
            let _ = write!(
                code,
                "<span class=\"src-line\" data-lines=\"{i}-{}\">{}</span>",
                i + 1,
                line.trim_end_matches('\n')
            );
        }
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
                // A diagram's source is not prose.
                Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info)))
                    if self.plugins.is_figure(Self::lang(&info)) =>
                {
                    drop(Self::inside(&mut events));
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

    /// A page's labelled figures, numbered in the order they come.
    #[must_use]
    pub fn figures(&self, markdown: &str) -> Vec<Figure> {
        Parser::new_ext(markdown, self.options)
            .filter_map(|e| match e {
                Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info)))
                    if self.plugins.is_figure(Self::lang(&info)) =>
                {
                    Figure::label(&info)
                }
                _ => None,
            })
            .enumerate()
            .map(|(i, (id, caption))| Figure {
                id,
                number: i + 1,
                caption,
            })
            .collect()
    }

    /// Renders Markdown as HTML, with each link's and image's target where `moved` says it is now, if it
    /// says, as a built site's pages are `.html` files rather than `.md`.
    #[must_use]
    pub fn render_with(&self, markdown: &str, moved: Moved) -> Page {
        self.render_opts(
            markdown,
            &Opts {
                moved: Some(moved),
                ..Opts::default()
            },
        )
    }

    /// The same, with each top-level block that `marks` gives a mark wrapped in `<div class="blk">`, holding
    /// its mark, so that the block can show it.
    #[must_use]
    pub fn render_marked(&self, markdown: &str, moved: Moved, marks: Marks) -> Page {
        self.render_opts(
            markdown,
            &Opts {
                moved: Some(moved),
                gutter: Some(marks),
                ..Opts::default()
            },
        )
    }

    /// A reader's text, as a comment is, rendered: any HTML in it shown as text, so that it cannot run or
    /// restyle the page it is shown on.
    #[must_use]
    pub fn render_safe(&self, markdown: &str, moved: Moved) -> Page {
        self.render_opts(
            markdown,
            &Opts {
                moved: Some(moved),
                safe: true,
                ..Opts::default()
            },
        )
    }

    /// Markdown rendered as `opts` asks.
    #[must_use]
    pub fn render_opts(&self, markdown: &str, opts: &Opts) -> Page {
        let wiki;
        let markdown = if opts.safe {
            wiki = Self::wiki_links(markdown);
            wiki.as_str()
        } else {
            markdown
        };
        // The figures are numbered first, so that a reference to one further down the page finds it.
        let own;
        let figures: &[Figure] = if let Some(given) = opts.figures {
            given
        } else {
            own = self.figures(markdown);
            &own
        };
        let ranges = self.block_ranges(markdown);
        let lines = Lines::of(markdown);
        let (mut depth, mut block, mut open, mut title_end) = (0usize, 0usize, false, 0);
        let mut ids = Ids::default();
        let mut headings = vec![];
        let mut out = vec![];
        let mut events = Parser::new_ext(markdown, self.options);
        while let Some(event) = events.next() {
            // The title is the page's first block, when that is a top-level heading.
            let title = depth == 0 && block == 0 && Self::top_heading(&event);
            if depth == 0 && matches!(event, Event::Start(_) | Event::Rule | Event::Html(_)) {
                if let Some(range) = ranges.get(block).cloned()
                    && let Some(mark) = opts.gutter.and_then(|gutter| gutter(range.clone()))
                {
                    out.push(mark.wrapper(&lines.spanned(&range)));
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
                    let inner = inner
                        .into_iter()
                        .map(|e| self.math(Self::referred(e, figures, opts)))
                        .collect();
                    out.extend(heading.events(classes, attrs, inner));
                    headings.push(heading);
                }
                Event::Start(Tag::CodeBlock(kind)) => {
                    let inner = Self::inside(&mut events);
                    out.extend(self.code_block(kind, inner, figures));
                }
                // A link to a page that is not there is marked, its target as written; its end is a link's end.
                Event::Start(Tag::Link { dest_url, .. })
                    if opts.broken.is_some_and(|broken| broken(&dest_url)) =>
                {
                    out.push(Self::broken_link(&dest_url));
                }
                Event::Start(Tag::Link {
                    link_type,
                    dest_url,
                    title,
                    id,
                }) => out.push(Event::Start(Tag::Link {
                    link_type,
                    dest_url: Self::target(dest_url, opts.moved),
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
                    dest_url: Self::target(dest_url, opts.moved),
                    title,
                    id,
                })),
                Event::Html(html) | Event::InlineHtml(html) if opts.safe => {
                    out.push(Event::Text(html));
                }
                event => out.push(self.math(Self::referred(event, figures, opts))),
            }
            if depth == 0 && open {
                out.push(Event::Html("</div>".into()));
                open = false;
            }
            if title {
                title_end = out.len();
            }
        }
        Page::new(out, headings, title_end)
    }

    /// Text with its figure references as links, by their figures' numbers; any other event as it is.
    fn referred<'a>(event: Event<'a>, figures: &[Figure], opts: &Opts) -> Event<'a> {
        match event {
            Event::Text(text) if text.contains('@') => {
                match Self::refer(&text, figures, opts.elsewhere) {
                    Some(html) => Event::InlineHtml(html.into()),
                    None => Event::Text(text),
                }
            }
            event => event,
        }
    }

    /// Text with its figure references, `@fig:id` or `@page.md#fig:id`, as links named by the figure's number,
    /// with its caption to hover; one to no figure is marked. None when the text has no reference.
    fn refer(text: &str, figures: &[Figure], elsewhere: Option<Elsewhere>) -> Option<String> {
        let mut out = String::new();
        let mut rest = text;
        let mut referred = false;
        while let Some(at) = rest.find('@') {
            let tail = &rest[at + 1..];
            let len = tail
                .find(|c: char| !(c.is_alphanumeric() || "_-./:#".contains(c)))
                .unwrap_or(tail.len());
            // A sentence's full stop or colon after a reference is not part of it.
            let token = tail[..len].trim_end_matches(['.', ':']);
            let (page, id) = token
                .split_once('#')
                .map_or((None, token), |(page, id)| (Some(page), id));
            // An @ inside a word, as in an address, is not a reference.
            let starts_word = rest[..at]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_alphanumeric());
            if !starts_word || id.len() <= "fig:".len() || !id.starts_with("fig:") {
                out.push_str(&Self::escape(&rest[..=at]));
                rest = tail;
                continue;
            }
            out.push_str(&Self::escape(&rest[..at]));
            let found = match page {
                None => figures
                    .iter()
                    .find(|f| f.id == id)
                    .map(|f| (format!("#{}", f.id), f.number, f.caption.clone())),
                Some(page) => elsewhere.and_then(|elsewhere| elsewhere(page, id)),
            };
            let _ = match found {
                Some((href, number, caption)) => {
                    let on = page.map_or(String::new(), |page| {
                        format!(" in {}", Self::escape(page.trim_end_matches(".md")))
                    });
                    write!(
                        out,
                        r#"<a class="fig-ref" href="{}" title="{}">Figure {number}{on}</a>"#,
                        Self::escape(&href),
                        Self::escape(&caption)
                    )
                }
                None => write!(
                    out,
                    r#"<span class="fig-ref broken" title="No figure {token}">{token}?</span>"#,
                    token = Self::escape(token)
                ),
            };
            referred = true;
            rest = &tail[token.len()..];
        }
        out.push_str(&Self::escape(rest));
        referred.then_some(out)
    }

    /// Math, `$...$` or `$$...$$`, as the math plugin renders it, or as it was written when none does; any other
    /// event as it is.
    fn math<'a>(&self, event: Event<'a>) -> Event<'a> {
        let (math, display) = match &event {
            Event::InlineMath(math) => (math, false),
            Event::DisplayMath(math) => (math, true),
            _ => return event,
        };
        let html = self.plugins.render_math(math, display).unwrap_or_else(|| {
            let dollars = if display { "$$" } else { "$" };
            Self::escape(&format!("{dollars}{math}{dollars}"))
        });
        Event::InlineHtml(html.into())
    }

    /// Whether an event starts a top-level heading, as a page's title.
    fn top_heading(event: &Event) -> bool {
        matches!(
            event,
            Event::Start(Tag::Heading {
                level: HeadingLevel::H1,
                ..
            })
        )
    }

    /// The start of a link to a page that is not there, marked, its target as written; its end is a link's end.
    fn broken_link<'a>(dest: &str) -> Event<'a> {
        let dest = Self::escape(dest);
        Event::Html(
            format!(r#"<a class="broken" href="{dest}" title="No such page: {dest}">"#).into(),
        )
    }

    /// A link's or image's target, or where `moved` says it is now.
    fn target<'a>(dest: CowStr<'a>, moved: Option<Moved>) -> CowStr<'a> {
        moved
            .and_then(|moved| moved(&dest))
            .map_or(dest, Into::into)
    }

    /// A code block: a plugin's, as the plugin renders it; highlighted, when its language is known; or else as it
    /// was, for pulldown-cmark to escape.
    fn code_block<'a>(
        &self,
        kind: CodeBlockKind<'a>,
        inner: Vec<Event<'a>>,
        figures: &[Figure],
    ) -> Vec<Event<'a>> {
        let (lang, label) = match &kind {
            CodeBlockKind::Fenced(info) => (Self::lang(info).to_string(), Figure::label(info)),
            CodeBlockKind::Indented => (String::new(), None),
        };
        if let Some(plugin) = self.plugins.block(&lang) {
            let source = Self::plain_text(&inner);
            let figure = label.and_then(|(id, _)| figures.iter().find(|f| f.id == id));
            return vec![Event::Html(
                self.plugins.render_block(plugin, &source, figure).into(),
            )];
        }
        match Self::highlight(&lang, &Self::plain_text(&inner)) {
            Some(html) => vec![Event::Html(
                format!(r#"<pre><code class="language-{lang}">{html}</code></pre>"#).into(),
            )],
            None => Self::wrap(Tag::CodeBlock(kind), inner, TagEnd::CodeBlock),
        }
    }

    /// A reader's `[[name]]` links, as Markdown links to `kb:name`: `[name](<kb:name>)`.
    fn wiki_links(text: &str) -> String {
        let mut out = String::new();
        let mut rest = text;
        while let Some(start) = rest.find("[[") {
            let Some(len) = rest[start + 2..].find("]]") else {
                break;
            };
            let name = &rest[start + 2..start + 2 + len];
            out.push_str(&rest[..start]);
            if name.is_empty() || name.contains('\n') {
                out.push_str(&rest[start..start + 4 + len]);
            } else {
                let _ = write!(out, "[{name}](<kb:{name}>)");
            }
            rest = &rest[start + 4 + len..];
        }
        out + rest
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

    /// Text as HTML, with the characters that would read as markup escaped, for text and attribute values alike.
    #[must_use]
    pub fn escape(text: &str) -> String {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }

    /// Code as HTML, its tokens in spans whose classes name what they are, as `hl-keyword` and
    /// `hl-string`, for a style sheet to color; or none, when the language is not known. A language
    /// is known by its name or its files' extension, as `rust` or `rs`.
    #[must_use]
    pub fn highlight(lang: &str, code: &str) -> Option<String> {
        let syntaxes = SYNTAXES.get_or_init(SyntaxSet::load_defaults_newlines);
        let syntax = syntaxes.find_syntax_by_token(lang)?;
        let mut html = ClassedHTMLGenerator::new_with_class_style(syntax, syntaxes, CLASSES);
        for line in LinesWithEndings::from(code) {
            html.parse_html_for_line_which_includes_newline(line).ok()?;
        }
        Some(html.finalize())
    }

    /// The same, line by line: each line's spans closed at its end and opened again on the next, so that each
    /// line stands alone.
    fn highlight_lines(lang: &str, code: &str) -> Option<Vec<String>> {
        let syntaxes = SYNTAXES.get_or_init(SyntaxSet::load_defaults_newlines);
        let syntax = syntaxes.find_syntax_by_token(lang)?;
        let mut state = ParseState::new(syntax);
        let mut stack = ScopeStack::new();
        let mut lines = vec![];
        for line in LinesWithEndings::from(code) {
            // The scopes open at the line's start, opened again, as an empty push of each writes them.
            let reopen: Vec<_> = stack
                .as_slice()
                .iter()
                .map(|s| (0, ScopeStackOp::Push(*s)))
                .collect();
            let (open, _) =
                line_tokens_to_classed_spans("", &reopen, CLASSES, &mut ScopeStack::new()).ok()?;
            let ops = state.parse_line(line, syntaxes).ok()?;
            let (html, _) = line_tokens_to_classed_spans(line, &ops, CLASSES, &mut stack).ok()?;
            // A line's newline is its end, which its own span stands for.
            let html = html.replace('\n', "");
            lines.push(format!("{open}{html}{}", "</span>".repeat(stack.len())));
        }
        Some(lines)
    }

    /// A fence's language: the first word of its info, so that what follows it, as in
    /// ```` ```rust ignore ````, is left for other uses.
    #[must_use]
    pub fn lang(info: &str) -> &str {
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
            "<div class=\"blk x\" data-lines=\"0-1\"><i>0</i>\n<h1 id=\"title\">Title</h1>\n</div>",
            "<div class=\"blk x\" data-lines=\"2-3\"><i>9</i>\n<p>One.</p>\n</div>",
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
        let source = source.trim_end();
        assert!(
            source.starts_with(r#"<pre class="source"><code>"#),
            "{source}"
        );
        assert!(source.contains("hl-"), "highlighted: {source}");
        assert!(
            source.contains(r#"<span class="src-line" data-lines="1-2">"#)
                && !source.contains('\n'),
            "each line numbered: {source}"
        );
        assert_eq!(
            source.matches("<span").count(),
            source.matches("</span>").count(),
            "each line stands alone: {source}"
        );
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

    /// A renderer with one plugin of the repository's own, `dots`, whose blocks are figures.
    fn with_dots() -> Renderer {
        let (dir, given) = crate::plugins::tests::own(
            "name = \"dots\"\nblocks = [\"dots\"]\nfigure = true\nstage = '<pre class=\"dots\">{source.html}</pre>'\n",
            &[],
        );
        let config = crate::config::Config {
            plugins: [("dots".to_string(), given)].into_iter().collect(),
            base: dir.path().to_path_buf(),
            ..crate::config::Config::default()
        };
        let plugins =
            Plugins::load(&config, Some(std::path::Path::new("git-kb.toml")), None).unwrap();
        Renderer::with_plugins(Box::leak(Box::new(plugins)))
    }

    #[test]
    fn a_plugin_s_block_is_the_plugin_s_to_render() {
        let html = with_dots()
            .render("```dots\na < b\n```\n\n```rust\nfn f() {}\n```\n")
            .html;
        assert!(
            html.contains(r#"<figure class="kb-figure kb-block" data-plugin="dots"><div class="kb-stage" title="Click to zoom"><pre class="dots">a &lt; b
</pre></div>"#),
            "{html}"
        );
        assert!(
            html.contains(r#"<pre><code class="language-rust">"#),
            "other code is highlighted still: {html}"
        );
        assert!(
            !Renderer::new()
                .render("```dots\nx\n```")
                .html
                .contains("kb-figure"),
            "without the plugin, code"
        );
    }

    #[test]
    fn a_diagram_s_source_is_not_searched() {
        let text: String = with_dots()
            .sections("# A\n\nWords.\n\n```dots\nsecret source\n```\n")
            .iter()
            .map(|s| s.text.clone())
            .collect();
        assert!(text.contains("Words") && !text.contains("secret"), "{text}");
    }

    #[test]
    fn math_is_the_math_plugin_s_and_a_dollar_is_a_dollar_without_one() {
        let config = crate::config::Config {
            plugins: [("mathjax".to_string(), toml::Table::new())]
                .into_iter()
                .collect(),
            ..crate::config::Config::default()
        };
        let plugins =
            Plugins::load(&config, Some(std::path::Path::new("git-kb.toml")), None).unwrap();
        let renderer = Renderer::with_plugins(Box::leak(Box::new(plugins)));
        let html = renderer
            .render("# Sizes $n$\n\nSo $x^2$ costs\n\n$$2MNK$$\n")
            .html;
        assert_eq!(
            html.matches(r#"data-plugin="mathjax""#).count(),
            3,
            "in the heading too: {html}"
        );
        assert!(
            html.contains(r#"<span class="math">\(x^2\)</span>"#),
            "{html}"
        );
        let plain = Renderer::new().render("It costs $5, or $6.").html;
        assert!(
            plain.contains("It costs $5, or $6.") && !plain.contains("data-plugin"),
            "{plain}"
        );
    }

    #[test]
    fn a_link_to_a_page_that_is_not_there_is_marked() {
        let broken = |dest: &str| dest == "gone.md";
        let html = Renderer::new()
            .render_opts(
                "[Gone](gone.md) and [here](here.md).\n",
                &Opts {
                    broken: Some(&broken),
                    ..Opts::default()
                },
            )
            .html;
        assert!(
            html.contains(r#"<a class="broken" href="gone.md" title="No such page: gone.md">Gone</a> and <a href="here.md">here</a>."#),
            "{html}"
        );
    }

    #[test]
    fn a_link_is_judged_broken_as_written_not_where_it_moved() {
        let broken = |dest: &str| dest == "gone.md";
        let moved = |dest: &str| dest.strip_suffix(".md").map(|d| format!("{d}.html"));
        let html = Renderer::new()
            .render_opts(
                "[Gone](gone.md) and [here](here.md).\n",
                &Opts {
                    broken: Some(&broken),
                    moved: Some(&moved),
                    ..Opts::default()
                },
            )
            .html;
        assert!(html.contains(r#"class="broken" href="gone.md""#), "{html}");
        assert!(html.contains(r#"<a href="here.html">here</a>"#), "{html}");
    }

    /// A renderer with the plugins a site without git-kb.toml has, every built-in one: `TikZ` among them,
    /// whose blocks are figures.
    fn with_plugins() -> Renderer {
        static PLUGINS: OnceLock<Plugins> = OnceLock::new();
        Renderer::with_plugins(PLUGINS.get_or_init(|| {
            Plugins::load(&crate::config::Config::default(), None, None)
                .expect("the built-in plugins load")
        }))
    }

    /// A page with two labelled diagrams and one unlabelled, referring to them.
    const FIGURES: &str = concat!(
        "# Title\n\nSee @fig:b, then @fig:a.\n\n",
        "```tikz #fig:a \"First\"\n\\draw (0,0);\n```\n\n",
        "```tikz\n\\draw (1,1);\n```\n\n",
        "```tikz \"Second\" #fig:b\n\\draw (2,2);\n```\n",
    );

    #[test]
    fn a_fence_s_label_is_its_id_and_caption_in_either_order() {
        let label = |info| Figure::label(info);
        assert_eq!(
            label("tikz #fig:life \"Data life cycle\""),
            Some(("fig:life".into(), "Data life cycle".into()))
        );
        assert_eq!(
            label("tikz \"Cap\" #fig:x"),
            Some(("fig:x".into(), "Cap".into()))
        );
        assert_eq!(label("tikz #fig:x"), Some(("fig:x".into(), String::new())));
        assert_eq!(label("tikz"), None);
        assert_eq!(label("rust ignore"), None);
        assert_eq!(
            label("tikz #tbl:x"),
            None,
            "only figures are labelled so far"
        );
    }

    #[test]
    fn labelled_figures_are_numbered_in_page_order() {
        let figures = with_plugins().figures(FIGURES);
        let said: Vec<(&str, usize, &str)> = figures
            .iter()
            .map(|f| (f.id.as_str(), f.number, f.caption.as_str()))
            .collect();
        assert_eq!(said, [("fig:a", 1, "First"), ("fig:b", 2, "Second")]);
    }

    #[test]
    fn only_a_figure_plugin_s_blocks_are_figures() {
        assert_eq!(
            Renderer::new().figures(FIGURES),
            [],
            "no plugins, no figures"
        );
        assert_eq!(
            with_plugins().figures("```rust #fig:a\nfn f() {}\n```\n"),
            []
        );
    }

    #[test]
    fn a_labelled_figure_is_drawn_with_its_number_and_caption() {
        let html = with_plugins().render(FIGURES).html;
        assert!(html.contains(r#"data-plugin="tikz" id="fig:a""#), "{html}");
        assert!(html.contains("<b>Figure 1</b>: First"), "{html}");
        assert!(html.contains("<b>Figure 2</b>: Second"), "{html}");
        assert_eq!(
            html.matches("<figure").count(),
            3,
            "the unlabelled one too: {html}"
        );
    }

    #[test]
    fn a_figure_reference_is_a_link_named_by_the_figure_s_number() {
        let html = with_plugins().render(FIGURES).html;
        assert!(
            html.contains(r##"See <a class="fig-ref" href="#fig:b" title="Second">Figure 2</a>, then <a class="fig-ref" href="#fig:a" title="First">Figure 1</a>."##),
            "{html}"
        );
    }

    #[test]
    fn a_reference_to_no_figure_is_marked() {
        let html = with_plugins().render("See @fig:none.\n").html;
        assert!(
            html.contains(
                r#"<span class="fig-ref broken" title="No figure fig:none">fig:none?</span>."#
            ),
            "{html}"
        );
    }

    #[test]
    fn a_reference_to_another_page_s_figure_names_the_page() {
        let elsewhere = |page: &str, id: &str| {
            (page == "arch/overview.md" && id == "fig:life").then(|| {
                (
                    "overview.html#fig:life".to_string(),
                    3,
                    "Data life cycle".to_string(),
                )
            })
        };
        let html = with_plugins()
            .render_opts(
                "As @arch/overview.md#fig:life shows.\n",
                &Opts {
                    elsewhere: Some(&elsewhere),
                    ..Opts::default()
                },
            )
            .html;
        assert!(
            html.contains(r#"<a class="fig-ref" href="overview.html#fig:life" title="Data life cycle">Figure 3 in arch/overview</a> shows."#),
            "{html}"
        );
    }

    #[test]
    fn an_at_that_is_not_a_figure_reference_is_left_as_it_is() {
        let html = with_plugins()
            .render(&format!(
                "{FIGURES}\nMail me@fig:a.com, or @someone, or `@fig:a`.\n\n```\n@fig:a\n```\n"
            ))
            .html;
        assert!(
            html.contains("Mail me@fig:a.com, or @someone, or <code>@fig:a</code>."),
            "{html}"
        );
        assert!(html.contains("<pre><code>@fig:a\n</code></pre>"), "{html}");
    }

    #[test]
    fn a_reader_s_wiki_links_are_links_for_moved_to_place() {
        let moved = |dest: &str| dest.strip_prefix("kb:").map(|name| format!("/{name}.html"));
        let html = Renderer::new()
            .render_opts(
                "See [[Overview]], not [[]].\n",
                &Opts {
                    safe: true,
                    moved: Some(&moved),
                    ..Opts::default()
                },
            )
            .html;
        assert!(
            html.contains(r#"See <a href="/Overview.html">Overview</a>, not [[]]."#),
            "{html}"
        );
        assert!(
            Renderer::new()
                .render("[[Overview]]")
                .html
                .contains("[[Overview]]"),
            "a page's own text has no wiki links"
        );
    }

    #[test]
    fn the_title_ends_after_a_first_top_level_heading() {
        let page = Renderer::new().render("# Title\n\nBody.\n");
        assert_eq!(
            &page.html[..page.title_end],
            "<h1 id=\"title\">Title</h1>\n"
        );
        assert_eq!(&page.html[page.title_end..], "<p>Body.</p>\n");
        for untitled in ["Body.\n\n# Title\n", "## Sub\n\nBody.\n"] {
            assert_eq!(Renderer::new().render(untitled).title_end, 0, "{untitled}");
        }
    }

    #[test]
    fn the_title_ends_after_its_mark_too() {
        let page = Renderer::new().render_marked("# Title\n\nBody.\n", &|_| None, &|_| {
            Some(Mark {
                class: "x",
                html: String::new(),
            })
        });
        assert!(
            page.html[..page.title_end].ends_with("</div>"),
            "{}",
            page.html
        );
    }
}
