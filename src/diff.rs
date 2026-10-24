//! Two versions of a page, as one page with the changes marked in place. The versions are compared block by
//! block: a block that is the same shows as it is, a new one as inserted, a gone one as deleted. A block that was
//! edited, sharing at least half its words with the old, shows once with its changed words marked; code and
//! tables, whose words are not prose, show as the old block deleted and the new one inserted.

use std::fmt::Write;

use similar::{Algorithm, DiffTag, capture_diff_slices};

use crate::render::{Figure, Moved, Opts, Renderer};

/// Two versions of a page compared: the newer, with what changed since the older marked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diff {
    pub html: String,
    /// How many places changed; each is `#change-N`.
    pub changes: usize,
}

impl Diff {
    /// A block of a page rendered alone, with the page's link references and figures, without its ids.
    fn alone(
        renderer: &Renderer,
        block: &str,
        refs: &str,
        moved: Moved,
        figures: &[Figure],
    ) -> String {
        let opts = Opts {
            moved: Some(moved),
            figures: Some(figures),
            ..Opts::default()
        };
        Self::without_ids(
            &renderer
                .render_opts(&format!("{block}\n\n{refs}"), &opts)
                .html,
        )
    }

    /// Two versions of a page compared, each block rendered as the page renders it, its links where `moved`
    /// says they are.
    #[must_use]
    pub fn pages(old: &str, new: &str, renderer: &Renderer, moved: Moved) -> Self {
        let (old_refs, new_refs) = (renderer.references(old), renderer.references(new));
        let (a, b) = (renderer.blocks(old), renderer.blocks(new));
        // A block rendered alone keeps its page's links and figures, by their numbers on that page.
        let (old_figures, new_figures) = (renderer.figures(old), renderer.figures(new));
        let old_block = |block: &str| Self::alone(renderer, block, &old_refs, moved, &old_figures);
        let new_block = |block: &str| Self::alone(renderer, block, &new_refs, moved, &new_figures);
        let mut diff = Self {
            html: String::new(),
            changes: 0,
        };
        for op in capture_diff_slices(Algorithm::Patience, &a, &b) {
            let (tag, old_range, new_range) = op.as_tag_tuple();
            match tag {
                DiffTag::Equal => {
                    for i in new_range {
                        diff.html.push_str(&new_block(b[i]));
                    }
                }
                DiffTag::Delete => {
                    for i in old_range {
                        diff.mark("del", &old_block(a[i]));
                    }
                }
                DiffTag::Insert => {
                    for i in new_range {
                        diff.mark("ins", &new_block(b[i]));
                    }
                }
                DiffTag::Replace => {
                    // The blocks paired in order: edits in place, then whatever is left over gone or new.
                    let (olds, news) = (&a[old_range], &b[new_range]);
                    for i in 0..olds.len().max(news.len()) {
                        match (olds.get(i), news.get(i)) {
                            (Some(o), Some(n)) => {
                                let (o, n) = (old_block(o), new_block(n));
                                if Self::prose(&o) && Self::prose(&n) && Self::alike(&o, &n) {
                                    diff.mark("chg", &Self::words(&o, &n));
                                } else {
                                    diff.mark("del", &o);
                                    diff.mark("ins", &n);
                                }
                            }
                            (Some(o), None) => diff.mark("del", &old_block(o)),
                            (None, Some(n)) => diff.mark("ins", &new_block(n)),
                            (None, None) => {}
                        }
                    }
                }
            }
        }
        diff
    }

    /// How one block of a page came to be what it is: compared with the block most like it in an older version
    /// of the page. Prose shows its changed words; code and tables their changed source lines.
    #[must_use]
    pub fn block(
        old_page: &str,
        new_page: &str,
        block: &str,
        renderer: &Renderer,
        moved: Moved,
    ) -> String {
        let new_words = Self::source(block);
        let best = renderer
            .blocks(old_page)
            .into_iter()
            .map(|o| (Self::likeness(&Self::source(o), &new_words), o))
            .max_by(|a, b| a.0.total_cmp(&b.0));
        if best.is_some_and(|(_, o)| o == block.trim_end()) {
            return "<p class=\"muted\">The same as before.</p>".into();
        }
        let render = |block: &str, page: &str| {
            let figures = renderer.figures(page);
            Self::alone(renderer, block, &renderer.references(page), moved, &figures)
        };
        let new = render(block, new_page);
        let old = best
            .filter(|(likeness, _)| *likeness >= 0.3)
            .map(|(_, o)| o);
        if !Self::prose(&new) {
            return Self::lines(old.unwrap_or(""), block);
        }
        match old {
            Some(o) => {
                let old = render(o, old_page);
                if Self::alike(&old, &new) {
                    Self::words(&old, &new)
                } else {
                    format!(
                        "<div class=\"diff-del\">{old}</div><div class=\"diff-ins\">{new}</div>"
                    )
                }
            }
            None => format!("<div class=\"diff-ins\">{new}</div>"),
        }
    }

    /// Two texts compared line by line, as a listing with each line marked as kept, removed or added.
    #[must_use]
    pub fn lines(old: &str, new: &str) -> String {
        let (a, b): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
        let mut out = String::from("<pre class=\"diff-lines\"><code>");
        let mut line = |class: &str, sign: char, text: &str| {
            // Each line is a block of its own, so no newline goes between them.
            let _ = write!(
                out,
                "<span class=\"{class}\">{sign} {}</span>",
                Self::escape(text)
            );
        };
        for op in capture_diff_slices(Algorithm::Patience, &a, &b) {
            let (tag, old_range, new_range) = op.as_tag_tuple();
            if tag == DiffTag::Equal {
                new_range.for_each(|i| line("l-same", ' ', b[i]));
            } else {
                old_range.for_each(|i| line("l-del", '-', a[i]));
                new_range.for_each(|i| line("l-ins", '+', b[i]));
            }
        }
        out.push_str("</code></pre>");
        out
    }

    /// A block's source, as words, to find the block most like it.
    fn source(block: &str) -> Vec<&str> {
        block.split_whitespace().collect()
    }

    /// HTML with its ids taken out, so that a copy of a heading, in a comparison or a hidden tab, is not where a
    /// link to the heading goes.
    #[must_use]
    pub fn without_ids(html: &str) -> String {
        html.replace(" id=\"", " data-id=\"")
    }

    /// A changed block, marked as one of the changes, and numbered.
    fn mark(&mut self, class: &str, body: &str) {
        self.changes += 1;
        let _ = write!(
            self.html,
            "<div class=\"diff-{class}\" id=\"change-{}\">{body}</div>",
            self.changes
        );
    }

    /// Whether two blocks of HTML share enough of their words, at least half, to read as one edited rather
    /// than replaced.
    fn alike(old: &str, new: &str) -> bool {
        let words = |html| -> Vec<&str> {
            Self::tokens(html)
                .into_iter()
                .filter(|t| !t.starts_with('<') && !t.trim().is_empty())
                .collect()
        };
        Self::likeness(&words(old), &words(new)) >= 0.5
    }

    /// The share of two texts' words that they have in common, from 0 to 1. Words are compared without case or
    /// punctuation, so "Paragraph." and "paragraph," are the same word.
    #[must_use]
    pub fn similarity(a: &str, b: &str) -> f64 {
        let words = |t: &str| {
            t.split(|c: char| !c.is_alphanumeric())
                .filter(|w| !w.is_empty())
                .map(str::to_lowercase)
                .collect::<Vec<_>>()
        };
        let (a, b) = (words(a), words(b));
        Self::likeness(
            &a.iter().map(String::as_str).collect::<Vec<_>>(),
            &b.iter().map(String::as_str).collect::<Vec<_>>(),
        )
    }

    fn likeness(a: &[&str], b: &[&str]) -> f64 {
        if a.is_empty() && b.is_empty() {
            return 1.0;
        }
        let same: usize = capture_diff_slices(Algorithm::Myers, a, b)
            .iter()
            .filter(|op| op.as_tag_tuple().0 == DiffTag::Equal)
            .map(|op| op.new_range().len())
            .sum();
        // Counts of words, far below where a float loses any.
        #[expect(clippy::cast_precision_loss)]
        let share = 2.0 * same as f64 / (a.len() + b.len()) as f64;
        share
    }

    /// Whether rendered HTML is prose, whose words can be compared, rather than code or a table.
    fn prose(html: &str) -> bool {
        !["<script", "<pre", "<table", "<figure"]
            .iter()
            .any(|t| html.contains(t))
    }

    /// The new HTML with the words that changed from the old in `<del>` and `<ins>`. Tags are kept from the new
    /// version only, so the result stays well formed; words are compared as words, tags as wholes.
    fn words(old: &str, new: &str) -> String {
        let (a, b) = (Self::tokens(old), Self::tokens(new));
        let mut out = String::new();
        for op in capture_diff_slices(Algorithm::Myers, &a, &b) {
            let (tag, old_range, new_range) = op.as_tag_tuple();
            match tag {
                DiffTag::Equal => b[new_range].iter().for_each(|t| out.push_str(t)),
                DiffTag::Delete => Self::wrap(&a[old_range], "del", false, &mut out),
                DiffTag::Insert => Self::wrap(&b[new_range], "ins", true, &mut out),
                DiffTag::Replace => {
                    Self::wrap(&a[old_range], "del", false, &mut out);
                    Self::wrap(&b[new_range], "ins", true, &mut out);
                }
            }
        }
        out
    }

    /// Runs of words in `<del>` or `<ins>`, with tags between them closed around, and kept only when asked.
    fn wrap(tokens: &[&str], tag: &str, keep_tags: bool, out: &mut String) {
        let mut open = false;
        for t in tokens {
            if t.starts_with('<') {
                if open {
                    let _ = write!(out, "</{tag}>");
                    open = false;
                }
                if keep_tags {
                    out.push_str(t);
                }
            } else {
                if !open && !t.trim().is_empty() {
                    let _ = write!(out, "<{tag}>");
                    open = true;
                }
                out.push_str(t);
            }
        }
        if open {
            let _ = write!(out, "</{tag}>");
        }
    }

    /// HTML as tags, runs of whitespace and words.
    fn tokens(html: &str) -> Vec<&str> {
        let bytes = html.as_bytes();
        let (mut out, mut i) = (vec![], 0);
        while i < bytes.len() {
            let start = i;
            if bytes[i] == b'<' {
                i = html[i..].find('>').map_or(bytes.len(), |j| i + j + 1);
            } else if bytes[i].is_ascii_whitespace() {
                while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
            } else {
                while i < bytes.len() && bytes[i] != b'<' && !bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
            }
            out.push(&html[start..i]);
        }
        out
    }

    fn escape(text: &str) -> String {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diff(old: &str, new: &str) -> Diff {
        Diff::pages(old, new, &Renderer::new(), &|_| None)
    }

    #[test]
    fn the_same_page_has_no_changes() {
        let page = "# A\n\nSome text.\n";
        let same = diff(page, page);
        assert_eq!(same.changes, 0);
        assert!(same.html.contains("<p>Some text.</p>"), "{}", same.html);
    }

    #[test]
    fn a_block_edited_shows_its_changed_words() {
        let edited = diff(
            "# A\n\nThe quick brown fox jumps.\n",
            "# A\n\nThe quick red fox jumps.\n",
        );
        assert_eq!(edited.changes, 1);
        assert!(
            edited.html.contains(r#"<div class="diff-chg" id="change-1"><p>The quick <del>brown</del><ins>red</ins> fox jumps.</p>"#),
            "{}",
            edited.html
        );
    }

    #[test]
    fn blocks_added_removed_or_rewritten_are_marked_whole() {
        let changed = diff(
            "# A\n\nGone.\n\nKept.\n",
            "# A\n\nKept.\n\nNew.\n\n```\ncode\n```\n",
        );
        assert!(
            changed
                .html
                .contains(r#"<div class="diff-del" id="change-1"><p>Gone.</p>"#),
            "{}",
            changed.html
        );
        assert!(
            changed.html.contains(r#"class="diff-ins""#) && changed.html.contains("<p>New.</p>")
        );
        assert!(
            changed.html.contains(r#"<h1 data-id="a">A</h1>"#),
            "copies of headings are not where links to them go: {}",
            changed.html
        );
        let rewritten = diff("Old words here.\n", "Entirely other text.\n");
        assert!(
            rewritten.html.contains("diff-del") && rewritten.html.contains("diff-ins"),
            "{}",
            rewritten.html
        );
    }

    #[test]
    fn links_go_where_moved_says() {
        let moved = Diff::pages("x\n", "See [it](dma.md).\n", &Renderer::new(), &|d| {
            (d == "dma.md").then(|| "dma.html".to_string())
        });
        assert!(
            moved.html.contains(r#"<a href="dma.html">it</a>"#),
            "{}",
            moved.html
        );
    }

    #[test]
    fn a_block_is_compared_with_the_one_most_like_it() {
        let r = Renderer::new();
        let old = "# A\n\nThe quick brown fox jumps over the dog.\n\nOther.\n";
        let new = "# A\n\nThe quick red fox jumps over the dog.\n\nOther.\n";
        let edited = Diff::block(
            old,
            new,
            "The quick red fox jumps over the dog.",
            &r,
            &|_| None,
        );
        assert!(
            edited.contains("<del>brown</del><ins>red</ins>"),
            "{edited}"
        );
        let same = Diff::block(old, new, "Other.", &r, &|_| None);
        assert!(same.contains("The same as before."), "{same}");
        let added = Diff::block(old, new, "Something else entirely.", &r, &|_| None);
        assert!(added.starts_with(r#"<div class="diff-ins">"#), "{added}");
    }

    #[test]
    fn source_is_compared_line_by_line() {
        assert_eq!(
            Diff::lines("a\nb <\n", "a\nc\n"),
            concat!(
                r#"<pre class="diff-lines"><code><span class="l-same">  a</span>"#,
                r#"<span class="l-del">- b &lt;</span><span class="l-ins">+ c</span></code></pre>"#
            )
        );
    }
    #[test]
    fn similar_texts_are_their_share_of_words_in_common() {
        assert!((Diff::similarity("The Paragraph.", "the paragraph,") - 1.0).abs() < 1e-9);
        assert!(Diff::similarity("one two three four", "one two five six") > 0.4);
        assert!(Diff::similarity("one two", "three four") < 1e-9);
    }

    #[test]
    fn a_block_shown_alone_keeps_its_page_s_figure_numbers() {
        static PLUGINS: std::sync::OnceLock<crate::plugins::Plugins> = std::sync::OnceLock::new();
        let renderer = Renderer::with_plugins(PLUGINS.get_or_init(|| {
            crate::plugins::Plugins::load(&crate::config::Config::default(), None, None).unwrap()
        }));
        let page = |said: &str| {
            format!(
                "```tikz #fig:a \"A\"\n\\draw (0,0);\n```\n\n```tikz #fig:b \"B\"\n\\draw (1,1);\n```\n\n{said}\n"
            )
        };
        let diff = Diff::pages(
            &page("See @fig:b."),
            &page("See @fig:b, again."),
            &renderer,
            &|_| None,
        );
        assert!(
            diff.html
                .contains(r##"<a class="fig-ref" href="#fig:b" title="B">Figure 2</a>"##),
            "{}",
            diff.html
        );
        assert!(!diff.html.contains("fig-ref broken"), "{}", diff.html);
    }
}
