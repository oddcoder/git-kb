//! Sentences numbered: each sentence of a page's paragraphs and list items is marked where it begins with its
//! number, counted down the page, so that readers can name it, as "14", and find it on any screen, however its lines
//! wrap there. The mark is an empty `<span class="sn" id="s14" data-n="14">`, its number drawn by the style sheet, so
//! that the number is not in the text: it is not copied with it, searched, or quoted in a comment; and `#s14` after
//! the page's address goes to it.
//!
//! A sentence ends at `.`, `!`, `?` or `…`, with any closing quotes or brackets after, and a space; the next begins at
//! what follows, even in lowercase, as a sentence may with a name, "git-kb". A period does not end one after a capital
//! alone, as an initial is; after letters each with a period, as "e.g" or "U.S"; or after a known abbreviation, as
//! "Dr" or "Fig". After "etc", it does unless a lowercase letter follows.

use pulldown_cmark::{Event, Tag};

/// Words whose period is theirs, not a sentence's, by their letters in lowercase; and those whose period may be
/// either, as what follows is in lowercase or not.
const ABBREVIATIONS: [&str; 22] = [
    "mr", "mrs", "ms", "dr", "prof", "sr", "jr", "st", "vs", "cf", "fig", "figs", "eq", "eqs",
    "no", "sec", "ch", "vol", "pp", "al", "approx", "resp",
];
const UNSURE: [&str; 1] = ["etc"];

/// What can end a sentence, and what may follow that, closing what the sentence opened.
const ENDS: [char; 7] = ['.', '!', '?', '…', '。', '！', '？'];
const CLOSERS: [char; 9] = [')', ']', '}', '"', '\'', '’', '”', '»', '›'];

/// Each sentence in the events of a page's text marked where it begins, numbered from 1; with an id, `s1`, unless a
/// heading already has it, as `taken` says.
pub(super) fn number<'a>(events: Vec<Event<'a>>, taken: &dyn Fn(&str) -> bool) -> Vec<Event<'a>> {
    let mut begins = scan(&events).into_iter().peekable();
    let mut out = Vec::with_capacity(events.len() + begins.len());
    let mut n = 0;
    let mut mark = |out: &mut Vec<Event<'a>>| {
        n += 1;
        let id = format!("s{n}");
        let id = if taken(&id) {
            String::new()
        } else {
            format!(" id=\"{id}\"")
        };
        out.push(Event::InlineHtml(
            format!(
                "<span class=\"sn\"{id} data-n=\"{n}\" aria-hidden=\"true\" data-pagefind-ignore></span>"
            )
            .into(),
        ));
    };
    for (i, event) in events.into_iter().enumerate() {
        match event {
            Event::Text(text) => {
                let mut from = 0;
                while let Some((_, at)) = begins.next_if(|&(j, _)| j == i) {
                    if at > from {
                        out.push(Event::Text(text[from..at].to_owned().into()));
                        from = at;
                    }
                    mark(&mut out);
                }
                out.push(if from == 0 {
                    Event::Text(text)
                } else {
                    Event::Text(text[from..].to_owned().into())
                });
            }
            event => {
                while begins.next_if(|&(j, _)| j == i).is_some() {
                    mark(&mut out);
                }
                out.push(event);
            }
        }
    }
    out
}

/// What an element is to the sentences in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Holds sentences: a paragraph, or a list item, whose text is in no paragraph when its list is tight.
    Prose,
    /// Lies within a sentence: emphasis, a link.
    Inline,
    /// Holds no sentences of its own: a heading, a table, a code block; and an image, whose text describes it.
    Other,
}

impl Kind {
    fn of(tag: &Tag) -> Self {
        match tag {
            Tag::Paragraph | Tag::Item => Self::Prose,
            Tag::Emphasis
            | Tag::Strong
            | Tag::Strikethrough
            | Tag::Superscript
            | Tag::Subscript
            | Tag::Link { .. } => Self::Inline,
            _ => Self::Other,
        }
    }
}

/// Where in a sentence the text read so far leaves off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum At {
    /// The start of a paragraph or an item: its first sentence begins at whatever comes first.
    Start,
    Within,
    /// After what may end one: `.`, `!` or `?`, and any closing quotes or brackets.
    End,
    /// After that and a space: the next begins with what follows.
    Space,
}

/// Reading the text for where its sentences begin.
struct Scan {
    at: At,
    /// Where the next sentence begins, if one does: the first of what follows a sentence's end, before its first
    /// letter says whether it is one.
    next: Option<(usize, usize)>,
    /// The word so far, to tell an abbreviation's period from a sentence's end; and whether the end was after one
    /// that may end a sentence or not, as "etc", which the case of the next letter says.
    word: String,
    unsure: bool,
    /// Where each sentence begins: the index of an event, and a byte of its text, or 0.
    begins: Vec<(usize, usize)>,
}

/// Where each sentence of the events begins, in order.
fn scan(events: &[Event]) -> Vec<(usize, usize)> {
    let mut scan = Scan {
        at: At::Start,
        next: None,
        word: String::new(),
        unsure: false,
        begins: vec![],
    };
    let mut open: Vec<Kind> = vec![];
    for (i, event) in events.iter().enumerate() {
        let prose = open.iter().rev().find(|k| **k != Kind::Inline) == Some(&Kind::Prose);
        match event {
            Event::Start(tag) => {
                let kind = Kind::of(tag);
                if kind == Kind::Prose {
                    scan.restart();
                } else if prose && (kind == Kind::Inline || matches!(tag, Tag::Image { .. })) {
                    scan.note((i, 0));
                }
                open.push(kind);
            }
            Event::End(_) => {
                if open.pop() == Some(Kind::Prose) {
                    scan.restart();
                }
            }
            _ if !prose => {}
            Event::Text(text) => scan.text(i, text),
            Event::Code(_) => scan.content((i, 0), false),
            Event::SoftBreak | Event::HardBreak => scan.space(),
            Event::InlineHtml(_) | Event::InlineMath(_) | Event::DisplayMath(_) => {
                scan.note((i, 0));
            }
            _ => {}
        }
    }
    scan.begins
}

impl Scan {
    /// A paragraph or an item begins, or ends: what follows begins a sentence.
    fn restart(&mut self) {
        self.at = At::Start;
        self.next = None;
        self.word.clear();
    }

    /// Something that is not a letter, at a place: where a sentence begins, if the next letter says one does.
    fn note(&mut self, place: (usize, usize)) {
        if matches!(self.at, At::Start | At::Space) {
            self.next.get_or_insert(place);
        }
    }

    /// What says whether a sentence begins: at the start of a paragraph, or after a sentence's end, anything but a
    /// lowercase letter after "etc".
    fn content(&mut self, place: (usize, usize), lowercase: bool) {
        if self.at == At::Start || (self.at == At::Space && !(lowercase && self.unsure)) {
            self.begins.push(self.next.unwrap_or(place));
        }
        self.next = None;
        self.at = At::Within;
        self.word.clear();
    }

    fn space(&mut self) {
        self.word.clear();
        if self.at == At::End {
            self.at = At::Space;
        }
    }

    fn text(&mut self, i: usize, text: &str) {
        for (b, c) in text.char_indices() {
            if c.is_whitespace() {
                self.space();
                continue;
            }
            match self.at {
                At::Start => self.content((i, b), false),
                At::Space if c.is_alphabetic() => self.content((i, b), c.is_lowercase()),
                At::Space => self.note((i, b)),
                At::Within if ENDS.contains(&c) && !(c == '.' && abbreviation(&self.word)) => {
                    self.at = At::End;
                    self.unsure = c == '.' && UNSURE.contains(&self.word.to_lowercase().as_str());
                }
                At::End if !ENDS.contains(&c) && !CLOSERS.contains(&c) => self.at = At::Within,
                At::Within | At::End => {}
            }
            self.word.push(c);
        }
    }
}

/// Whether a word before a period is an abbreviation, whose period does not end the sentence.
fn abbreviation(word: &str) -> bool {
    let word = word.trim_start_matches(|c: char| !c.is_alphanumeric());
    let mut chars = word.chars();
    let initial = chars.next().is_some_and(char::is_uppercase) && chars.next().is_none();
    let lettered = word.contains('.')
        && word
            .split('.')
            .all(|part| part.chars().count() == 1 && part.chars().all(char::is_alphabetic));
    initial || lettered || ABBREVIATIONS.contains(&word.to_lowercase().as_str())
}

#[cfg(test)]
mod tests {
    use std::fmt::Write;

    use super::super::{Opts, Renderer};

    fn html(markdown: &str) -> String {
        let opts = Opts {
            numbered: true,
            ..Opts::default()
        };
        Renderer::new().render_opts(markdown, &opts).html
    }

    /// A page's HTML, numbered, with each sentence's mark as `[n]`.
    fn numbered(markdown: &str) -> String {
        let html = html(markdown);
        let (mut out, mut rest) = (String::new(), html.as_str());
        while let Some(at) = rest.find("<span class=\"sn\"") {
            out.push_str(&rest[..at]);
            let end = at + rest[at..].find("</span>").unwrap() + "</span>".len();
            let n = rest[at..end].split("data-n=\"").nth(1).unwrap();
            write!(out, "[{}]", &n[..n.find('"').unwrap()]).unwrap();
            rest = &rest[end..];
        }
        out + rest
    }

    #[test]
    fn sentences_are_numbered_down_the_page_but_not_the_title() {
        assert_eq!(
            numbered("# Title\n\nOne. Two! Three?\n\nFour…\n"),
            "<h1 id=\"title\">Title</h1>\n<p>[1]One. [2]Two! [3]Three?</p>\n<p>[4]Four…</p>\n"
        );
    }

    #[test]
    fn a_sentence_runs_on_over_the_lines_of_its_source() {
        assert_eq!(
            numbered("One sentence\nruns on. Then\nanother.\n"),
            "<p>[1]One sentence\nruns on. [2]Then\nanother.</p>\n"
        );
    }

    #[test]
    fn abbreviations_initials_and_numbers_end_no_sentence() {
        assert_eq!(
            numbered("See e.g. the cache. Dr. Smith ran it at 3.5 GHz. A. Turing agreed.\n"),
            "<p>[1]See e.g. the cache. [2]Dr. Smith ran it at 3.5 GHz. [3]A. Turing agreed.</p>\n"
        );
    }

    #[test]
    fn a_sentence_may_begin_in_lowercase_but_not_after_etc() {
        assert_eq!(
            numbered(
                "It is a folder. git-kb shows it. Pages, files, etc. are listed, etc. Then it stops.\n"
            ),
            "<p>[1]It is a folder. [2]git-kb shows it. [3]Pages, files, etc. are listed, etc. [4]Then it stops.</p>\n"
        );
    }

    #[test]
    fn a_file_s_name_code_and_quotes_end_sentences_as_text_does() {
        assert_eq!(
            numbered(
                "Edit config.toml. Then run `make`. `make` builds it. Ada said “stop.” Then left.\n"
            ),
            "<p>[1]Edit config.toml. [2]Then run <code>make</code>. [3]<code>make</code> builds it. \
             [4]Ada said “stop.” [5]Then left.</p>\n"
        );
    }

    #[test]
    fn a_sentence_begins_before_the_markup_it_begins_with() {
        assert_eq!(
            numbered("Done. **Next** thing. [A link](x) follows.\n"),
            "<p>[1]Done. [2]<strong>Next</strong> thing. [3]<a href=\"x\">A link</a> follows.</p>\n"
        );
    }

    #[test]
    fn list_items_and_quotes_are_numbered_but_not_headings_code_or_tables() {
        let page = numbered(
            "# T\n\n- One.\n- Two.\n  - Three.\n\n> Four.\n\n## Five.\n\n```\nSix.\n```\n\n| a |\n|---|\n| Seven. |\n",
        );
        for numbered in ["[1]One.", "[2]Two.", "[3]Three.", "[4]Four."] {
            assert!(page.contains(numbered), "{numbered}: {page}");
        }
        assert!(!page.contains("[5]"), "{page}");
    }

    #[test]
    fn an_image_s_description_is_not_a_sentence() {
        let page = html("![A cat. Asleep.](cat.png)\n\nText.\n");
        assert!(page.contains("alt=\"A cat. Asleep.\""), "{page}");
        assert_eq!(page.matches("class=\"sn\"").count(), 1, "{page}");
    }

    #[test]
    fn a_mark_is_empty_and_hidden_from_search_and_readers_aloud() {
        assert_eq!(
            html("One.\n"),
            "<p><span class=\"sn\" id=\"s1\" data-n=\"1\" aria-hidden=\"true\" data-pagefind-ignore></span>One.</p>\n"
        );
    }

    #[test]
    fn a_heading_s_id_is_not_taken_for_a_sentence() {
        let page = html("## S2\n\nOne. Two.\n");
        assert!(page.contains("<h2 id=\"s2\">"), "{page}");
        assert!(page.contains("id=\"s1\" data-n=\"1\""), "{page}");
        assert!(page.contains("<span class=\"sn\" data-n=\"2\""), "{page}");
    }

    #[test]
    fn only_a_page_asked_to_be_is_numbered() {
        assert!(!Renderer::new().render("One. Two.\n").html.contains("sn"));
    }
}
