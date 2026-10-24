//! A page's comments, in a column beside it: a card for each thread, in the order of the page, with its comments;
//! then those whose text has gone. Beside each block with threads, a marker of them, linking to the first. A
//! resolved thread is there but hidden, until Show resolved is ticked.

use std::fmt::Write;
use std::ops::Range;

use super::Shell;
use crate::comments::{Located, OnPage, State, Thread};
use crate::git::When;
use crate::render::Mark;

/// A page's comments as shown: its threads where they are on it, and how a comment's Markdown is rendered.
pub struct Notes<'a> {
    on: OnPage,
    /// Where each of the page's lines starts.
    starts: Vec<usize>,
    render: &'a dyn Fn(&str) -> String,
    /// Now, in seconds since 1970, for how long ago each comment was written.
    now: i64,
}

impl<'a> Notes<'a> {
    /// The comments on a page of this text, rendered by `render`.
    #[must_use]
    pub fn new(on: OnPage, text: &str, render: &'a dyn Fn(&str) -> String) -> Self {
        let starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        Self {
            on,
            starts,
            render,
            now: When::now().0,
        }
    }

    /// Whether the page has any threads to show, found on it or not.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.on.found.is_empty() && self.on.lost.is_empty()
    }

    /// The lines of the text a range of its bytes spans, from 0, its last not among them.
    fn lines(&self, range: &Range<usize>) -> Range<usize> {
        let line = |offset: usize| {
            self.starts
                .partition_point(|&s| s <= offset)
                .saturating_sub(1)
        };
        line(range.start)..line(range.end.saturating_sub(1).max(range.start)) + 1
    }
}

impl Shell {
    /// A block's mark, from when it last changed, with the marker of its comments beside it, if it has any.
    #[must_use]
    pub fn marked(
        &self,
        changed: Option<Mark>,
        notes: Option<&Notes>,
        range: &Range<usize>,
    ) -> Option<Mark> {
        let comments = notes
            .map(|notes| self.comment_mark(notes, range))
            .unwrap_or_default();
        match changed {
            Some(mut mark) => {
                mark.html.push_str(&comments);
                Some(mark)
            }
            None if comments.is_empty() => None,
            None => Some(Mark {
                class: "",
                html: comments,
            }),
        }
    }

    /// The marker beside the block at a range of the page's text: a link to its first thread, with how many are
    /// open, or a tick when all are resolved; nothing when it has none.
    #[must_use]
    pub fn comment_mark(&self, notes: &Notes, range: &Range<usize>) -> String {
        let lines = notes.lines(range);
        let here: Vec<&Located> = notes
            .on
            .found
            .iter()
            .filter(|l| lines.contains(&l.lines.start))
            .collect();
        let Some(first) = here.first() else {
            return String::new();
        };
        let id = Self::escape(&first.thread.id);
        match here.iter().filter(|l| !l.thread.resolved).count() {
            0 => {
                format!("<a class=\"cmt done\" href=\"#t-{id}\" title=\"Resolved comments\">✓</a>")
            }
            1 => format!("<a class=\"cmt\" href=\"#t-{id}\" title=\"1 open thread\">💬 1</a>"),
            n => format!("<a class=\"cmt\" href=\"#t-{id}\" title=\"{n} open threads\">💬 {n}</a>"),
        }
    }

    /// The column of a page's comments: a card for each thread found on it, by where it is, then those whose text
    /// has gone.
    #[must_use]
    pub fn rail(&self, notes: &Notes) -> String {
        let mut found: Vec<&Located> = notes.on.found.iter().collect();
        found.sort_by_key(|l| {
            (
                l.lines.start,
                l.thread.comments.first().map_or(0, |c| c.time),
            )
        });
        let mut cards = String::new();
        for located in &found {
            cards.push_str(&Self::thread(notes, &located.thread, Some(located.state)));
        }
        if !notes.on.lost.is_empty() {
            let all_resolved = notes.on.lost.iter().all(|t| t.resolved);
            let _ = write!(
                cards,
                "<h2 class=\"rail-lost{}\">On text that has since gone</h2>",
                if all_resolved { " resolved" } else { "" }
            );
            for thread in &notes.on.lost {
                cards.push_str(&Self::thread(notes, thread, None));
            }
        }
        let resolved = found.iter().filter(|l| l.thread.resolved).count()
            + notes.on.lost.iter().filter(|t| t.resolved).count();
        let open = found.len() + notes.on.lost.len() - resolved;
        let show = if resolved == 0 {
            String::new()
        } else {
            format!(
                "<label class=\"show-resolved\"><input type=\"checkbox\" id=\"show-resolved\"> Show resolved ({resolved})</label>"
            )
        };
        let empty = if open == 0 {
            "<p class=\"muted rail-empty\">No open comments.</p>"
        } else {
            ""
        };
        format!(
            "<aside class=\"rail\" aria-label=\"Comments\"><div class=\"rail-head\"><h2>Comments</h2>{show}</div>{empty}{cards}</aside>\n"
        )
    }

    /// A thread's card: who began it and how many comments it has, where it is when that has changed, and its
    /// comments.
    fn thread(notes: &Notes, thread: &Thread, state: Option<State>) -> String {
        let quote = || {
            format!(
                "<blockquote class=\"on\">{}</blockquote>",
                Self::escape(&thread.quote)
            )
        };
        let on = match state {
            Some(State::Same) => String::new(),
            Some(State::Edited) => "<p class=\"muted\">Its text has changed since.</p>".into(),
            Some(State::Moved) => {
                "<p class=\"muted\">Its lines are gone; it is on the paragraph most like them.</p>"
                    .into()
            }
            None => quote(),
        };
        let mut comments = String::new();
        for comment in &thread.comments {
            let _ = write!(
                comments,
                "<div class=\"c\"><p class=\"c-head\"><b>{}</b> <span title=\"{}\">{}</span></p><div class=\"c-body\">{}</div></div>",
                Self::escape(&comment.author),
                When(comment.time).date(),
                When(comment.time).ago(notes.now),
                (notes.render)(&comment.body)
            );
        }
        let count = match thread.comments.len() {
            1 => "1 comment".to_string(),
            n => format!("{n} comments"),
        };
        format!(
            "<details class=\"thread{resolved}\" id=\"t-{id}\"{open}><summary>{who} · {count}{state}</summary>{on}{comments}</details>",
            resolved = if thread.resolved { " resolved" } else { "" },
            id = Self::escape(&thread.id),
            open = if thread.resolved { "" } else { " open" },
            who = Self::escape(thread.comments.first().map_or("", |c| c.author.as_str())),
            state = if thread.resolved { " · resolved" } else { "" },
        )
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use gix::ObjectId;

    use super::*;
    use crate::comments::{Comment, Snippet};
    use crate::shell::Links;

    fn thread(id: &str, lines: Range<usize>, resolved: bool, bodies: &[&str]) -> Thread {
        Thread {
            id: id.into(),
            page: PathBuf::from("a.md"),
            commit: ObjectId::null(gix::hash::Kind::Sha1),
            text: ObjectId::null(gix::hash::Kind::Sha1),
            lines,
            quote: "The quoted line.".into(),
            snippet: Snippet::default(),
            resolved,
            comments: bodies
                .iter()
                .map(|b| Comment {
                    author: "Ada".into(),
                    email: "ada@x".into(),
                    time: 0,
                    body: (*b).into(),
                })
                .collect(),
        }
    }

    fn found(thread: Thread, state: State) -> Located {
        Located {
            lines: thread.lines.clone(),
            thread,
            state,
        }
    }

    const TEXT: &str = "# A\n\nOne.\n\nTwo.\n";

    fn shell() -> Shell {
        Shell::new("Site".into(), Links::Served)
    }

    #[test]
    fn a_block_s_marker_links_to_its_threads_and_counts_the_open_ones() {
        let on = OnPage {
            found: vec![
                found(thread("t1", 2..3, false, &["Hi"]), State::Same),
                found(thread("t2", 2..3, false, &["Again"]), State::Same),
                found(thread("t3", 4..5, true, &["Done"]), State::Same),
            ],
            lost: vec![],
        };
        let render = |b: &str| b.to_string();
        let notes = Notes::new(on, TEXT, &render);
        let shell = shell();
        let at = |text: &str| {
            let start = TEXT.find(text).unwrap();
            shell.comment_mark(&notes, &(start..start + text.len() + 1))
        };
        assert_eq!(
            at("One."),
            r##"<a class="cmt" href="#t-t1" title="2 open threads">💬 2</a>"##
        );
        assert_eq!(
            at("Two."),
            r##"<a class="cmt done" href="#t-t3" title="Resolved comments">✓</a>"##
        );
        assert_eq!(at("# A"), "");
    }

    #[test]
    fn the_rail_has_a_card_for_each_thread_in_page_order_then_the_lost() {
        let on = OnPage {
            found: vec![
                found(thread("late", 4..5, false, &["Second"]), State::Edited),
                found(
                    thread("early", 2..3, false, &["First", "A reply"]),
                    State::Same,
                ),
            ],
            lost: vec![thread("gone", 9..10, false, &["Lost one"])],
        };
        let render = |b: &str| format!("<p>{b}</p>");
        let rail = shell().rail(&Notes::new(on, TEXT, &render));
        let (early, late, gone) = (
            rail.find("id=\"t-early\"").unwrap(),
            rail.find("id=\"t-late\"").unwrap(),
            rail.find("id=\"t-gone\"").unwrap(),
        );
        assert!(early < late && late < gone, "{rail}");
        assert!(rail.contains("Ada · 2 comments"), "{rail}");
        assert!(
            rail.contains("<div class=\"c-body\"><p>A reply</p></div>"),
            "{rail}"
        );
        assert!(rail.contains("Its text has changed since."), "{rail}");
        assert!(
            rail[gone..].contains("<blockquote class=\"on\">The quoted line.</blockquote>"),
            "a lost thread shows what it was on: {rail}"
        );
        assert!(rail.contains("On text that has since gone"), "{rail}");
    }

    #[test]
    fn resolved_threads_are_hidden_until_asked_for() {
        let on = OnPage {
            found: vec![found(thread("t1", 2..3, true, &["Done"]), State::Same)],
            lost: vec![],
        };
        let render = |b: &str| b.to_string();
        let rail = shell().rail(&Notes::new(on, TEXT, &render));
        assert!(
            rail.contains("class=\"thread resolved\" id=\"t-t1\">"),
            "closed: {rail}"
        );
        assert!(rail.contains("Show resolved (1)"), "{rail}");
        assert!(rail.contains("No open comments."), "{rail}");
    }

    #[test]
    fn what_a_comment_says_about_itself_is_escaped() {
        let mut t = thread("<x>", 2..3, false, &["Hi"]);
        t.comments[0].author = "<b>Eve</b>".into();
        let on = OnPage {
            found: vec![found(t, State::Same)],
            lost: vec![],
        };
        let render = |b: &str| b.to_string();
        let rail = shell().rail(&Notes::new(on, TEXT, &render));
        assert!(
            !rail.contains("<b>Eve</b>") && rail.contains("&lt;b&gt;Eve"),
            "{rail}"
        );
        assert!(rail.contains("id=\"t-&lt;x&gt;\""), "{rail}");
    }
}
