//! A page's comments, in a column beside it: a card for each thread, in the order of the page, with its comments;
//! then those whose text has gone. Beside each block with threads, a marker of them, linking to the first. A
//! resolved thread is there but hidden, until Show resolved is ticked.
//!
//! On the folder as it is, served, comments are written there too, by forms that need no script: a + beside
//! each block opens a box in the column to begin a thread on it, as `?comment=<lines>`; and each card has a box
//! to reply in and a button to resolve or reopen it. Built, and at a commit, the threads are only shown.

use std::fmt::Write;
use std::ops::Range;
use std::path::{Path, PathBuf};

use super::{Links, Shell};
use crate::comments::{Located, OnPage, State, Synced, Thread};
use crate::git::When;
use crate::render::Mark;

/// A page's comments as shown: its threads where they are on it, and how a comment's Markdown is rendered.
pub struct Notes<'a> {
    /// The page, from the site's top.
    page: PathBuf,
    on: OnPage,
    /// The page's text, and the lines a new thread is being begun on, if one is.
    text: String,
    composing: Option<Range<usize>>,
    /// What the last sync of the comments did, if there has been one.
    synced: Option<Synced>,
    /// Where each of the page's lines starts.
    starts: Vec<usize>,
    render: &'a dyn Fn(&str) -> String,
    /// Now, in seconds since 1970, for how long ago each comment was written.
    now: i64,
}

impl<'a> Notes<'a> {
    /// The comments on a page of this text, rendered by `render`.
    #[must_use]
    pub fn new(page: &Path, on: OnPage, text: &str, render: &'a dyn Fn(&str) -> String) -> Self {
        let starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        Self {
            page: page.to_path_buf(),
            on,
            text: text.to_string(),
            composing: None,
            synced: None,
            starts,
            render,
            now: When::now().0,
        }
    }

    /// The same, with a box open to begin a thread on these lines.
    #[must_use]
    pub fn composing(self, lines: Option<Range<usize>>) -> Self {
        Self {
            composing: lines,
            ..self
        }
    }

    /// The same, saying what the last sync did.
    #[must_use]
    pub fn synced(self, synced: Option<Synced>) -> Self {
        Self { synced, ..self }
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
    /// Whether comments are written here: on the folder as it is, served; not at a commit, nor built.
    fn writes(&self) -> bool {
        self.links == Links::Served && self.revision.is_none()
    }

    /// Where a page is served, from the server's top, for a form to come back to.
    fn served(page: &Path) -> String {
        format!("/docs/{}", Self::encode(&page.with_extension("html")))
    }

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
    /// open, or a tick when all are resolved; and, where comments are written, a + to begin one on it.
    #[must_use]
    pub fn comment_mark(&self, notes: &Notes, range: &Range<usize>) -> String {
        let lines = notes.lines(range);
        let add = if self.writes() {
            format!(
                "<a class=\"cmt add\" href=\"?comment={}-{}#composer\" title=\"Comment on this paragraph\">+</a>",
                lines.start, lines.end
            )
        } else {
            String::new()
        };
        let here: Vec<&Located> = notes
            .on
            .found
            .iter()
            .filter(|l| lines.contains(&l.lines.start))
            .collect();
        let Some(first) = here.first() else {
            return add;
        };
        let id = Self::escape(&first.thread.id);
        let threads = match here.iter().filter(|l| !l.thread.resolved).count() {
            0 => {
                format!("<a class=\"cmt done\" href=\"#t-{id}\" title=\"Resolved comments\">✓</a>")
            }
            1 => format!("<a class=\"cmt\" href=\"#t-{id}\" title=\"1 open thread\">💬 1</a>"),
            n => format!("<a class=\"cmt\" href=\"#t-{id}\" title=\"{n} open threads\">💬 {n}</a>"),
        };
        threads + &add
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
            cards.push_str(&self.thread(notes, &located.thread, Some(located.state)));
        }
        if !notes.on.lost.is_empty() {
            let all_resolved = notes.on.lost.iter().all(|t| t.resolved);
            let _ = write!(
                cards,
                "<h2 class=\"rail-lost{}\">On text that has since gone</h2>",
                if all_resolved { " resolved" } else { "" }
            );
            for thread in &notes.on.lost {
                cards.push_str(&self.thread(notes, thread, None));
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
        let empty = match (open, self.writes()) {
            (0, true) => {
                "<p class=\"muted rail-empty\">No open comments. Press + beside a paragraph to begin one.</p>"
            }
            (0, false) => "<p class=\"muted rail-empty\">No open comments.</p>",
            _ => "",
        };
        let composer = self.composer(notes);
        let sync = self.sync_form(notes);
        let all = if self.writes() {
            "<a class=\"all-comments\" href=\"/_comments\">All comments</a>"
        } else {
            ""
        };
        format!(
            "<aside class=\"rail\" aria-label=\"Comments\"><div class=\"rail-head\"><h2>Comments</h2>{all}{show}</div>{sync}{composer}{empty}{cards}</aside>\n<div class=\"edge edge-rail\" title=\"Drag to size the comments, or close them; double-click for the default\"></div>\n"
        )
    }

    /// Where comments are written, a button to sync them with every remote now, and what the last sync did.
    fn sync_form(&self, notes: &Notes) -> String {
        if !self.writes() {
            return String::new();
        }
        let last = notes.synced.as_ref().map_or_else(
            || "Not synced yet".to_string(),
            |s| {
                format!(
                    "Synced {}: {}",
                    When(s.time).ago(notes.now),
                    Self::escape(&s.said)
                )
            },
        );
        format!(
            concat!(
                "<form class=\"sync\" method=\"post\" action=\"/_comments/sync\">",
                "<input type=\"hidden\" name=\"back\" value=\"{back}\">",
                "<button type=\"submit\" title=\"Fetch everyone's comments from each remote, and push yours\">Sync</button>",
                " <span class=\"muted\">{last}</span></form>"
            ),
            back = Self::escape(&Self::served(&notes.page)),
            last = last,
        )
    }

    /// Every thread of the site, as a page's body: by page, the open first, each its first comment and a link to
    /// it on its page.
    #[must_use]
    pub fn comments_page(&self, threads: &[Thread]) -> String {
        let mut by_page: std::collections::BTreeMap<&Path, Vec<&Thread>> =
            std::collections::BTreeMap::new();
        for thread in threads {
            by_page
                .entry(thread.page.as_path())
                .or_default()
                .push(thread);
        }
        let open = threads.iter().filter(|t| !t.resolved).count();
        let mut out = format!(
            "<h1>Comments</h1>\n<p class=\"meta\">{open} open, {} resolved, on {} pages</p>\n",
            threads.len() - open,
            by_page.len()
        );
        let now = When::now().0;
        for (page, mut threads) in by_page {
            threads.sort_by_key(|t| {
                (
                    t.resolved,
                    std::cmp::Reverse(t.comments.last().map_or(0, |c| c.time)),
                )
            });
            let href = Self::served(page);
            let _ = write!(
                out,
                "<h2><a href=\"{}\">{}</a></h2>\n<ul class=\"all-threads\">",
                Self::escape(&href),
                Self::escape(&page.to_string_lossy())
            );
            for thread in threads {
                let Some(first) = thread.comments.first() else {
                    continue;
                };
                let _ = write!(
                    out,
                    concat!(
                        "<li class=\"{class}\"><a href=\"{href}#t-{id}\">{quote}</a>",
                        "<p><b>{who}</b>: {body}</p><p class=\"muted\">{count} · {when}{resolved}</p></li>"
                    ),
                    class = if thread.resolved { "resolved" } else { "open" },
                    href = Self::escape(&href),
                    id = Self::encode_id(&thread.id),
                    quote = Self::escape(&thread.quote),
                    who = Self::escape(&first.author),
                    body = Self::escape(first.body.lines().next().unwrap_or("")),
                    count = match thread.comments.len() {
                        1 => "1 comment".to_string(),
                        n => format!("{n} comments"),
                    },
                    when = When(thread.comments.last().map_or(0, |c| c.time)).ago(now),
                    resolved = if thread.resolved { " · resolved" } else { "" },
                );
            }
            out.push_str("</ul>\n");
        }
        out
    }

    /// The box to begin a thread in, on the lines asked for, when one is being begun here.
    fn composer(&self, notes: &Notes) -> String {
        let Some(lines) = notes.composing.clone().filter(|_| self.writes()) else {
            return String::new();
        };
        let quote: Vec<&str> = notes
            .text
            .lines()
            .skip(lines.start)
            .take(lines.end.saturating_sub(lines.start))
            .collect();
        let back = Self::escape(&Self::served(&notes.page));
        format!(
            concat!(
                "<form class=\"composer thread\" id=\"composer\" method=\"post\" action=\"/_comments/new\">",
                "<blockquote class=\"on\">{quote}</blockquote>",
                "<input type=\"hidden\" name=\"page\" value=\"{page}\">",
                "<input type=\"hidden\" name=\"lines\" value=\"{start}-{end}\">",
                "<input type=\"hidden\" name=\"back\" value=\"{back}\">",
                "<textarea name=\"body\" rows=\"4\" required placeholder=\"Comment, in Markdown\" autofocus></textarea>",
                "<p class=\"actions\"><button type=\"submit\">Comment</button> <a href=\"{back}\">Cancel</a></p></form>"
            ),
            quote = Self::escape(&quote.join("\n")),
            page = Self::escape(&notes.page.to_string_lossy()),
            start = lines.start,
            end = lines.end,
            back = back,
        )
    }

    /// A thread's forms, where comments are written: a box to reply in, and a button to resolve or reopen it.
    fn answer(&self, notes: &Notes, thread: &Thread) -> String {
        if !self.writes() {
            return String::new();
        }
        let (verb, resolved) = if thread.resolved {
            ("Reopen", "0")
        } else {
            ("Resolve", "1")
        };
        format!(
            concat!(
                "<form class=\"reply\" method=\"post\" action=\"/_comments/reply\">",
                "<input type=\"hidden\" name=\"id\" value=\"{id}\"><input type=\"hidden\" name=\"back\" value=\"{back}\">",
                "<textarea name=\"body\" rows=\"2\" required placeholder=\"Reply\"></textarea>",
                "<p class=\"actions\"><button type=\"submit\">Reply</button></p></form>",
                "<form class=\"resolve\" method=\"post\" action=\"/_comments/resolve\">",
                "<input type=\"hidden\" name=\"id\" value=\"{id}\"><input type=\"hidden\" name=\"back\" value=\"{back}\">",
                "<input type=\"hidden\" name=\"resolved\" value=\"{resolved}\"><button type=\"submit\">{verb}</button></form>"
            ),
            id = Self::escape(&thread.id),
            back = Self::escape(&Self::served(&notes.page)),
            resolved = resolved,
            verb = verb,
        )
    }

    /// A thread's card: who began it and how many comments it has, where it is when that has changed, and its
    /// comments.
    fn thread(&self, notes: &Notes, thread: &Thread, state: Option<State>) -> String {
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
            // An AI's comment says so, by the agent and for whom it wrote; a person's does not.
            let ai = comment.ai.as_ref().map_or(String::new(), |ai| {
                format!(
                    " <span class=\"ai-badge\" title=\"Written by an AI, {}, for {}, through /_ai\">🤖 AI</span>",
                    Self::escape(&ai.agent),
                    Self::escape(&ai.for_whom)
                )
            });
            let _ = write!(
                comments,
                "<div class=\"c{}\"><p class=\"c-head\"><b>{}</b>{ai} <span title=\"{}\">{}</span></p><div class=\"c-body\">{}</div></div>",
                if comment.ai.is_some() { " by-ai" } else { "" },
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
        let kind = if thread.kind.is_empty() {
            String::new()
        } else {
            let mut label = thread.kind.clone();
            label[..1].make_ascii_uppercase();
            format!(
                "<span class=\"kind-badge kind-{}\">{}</span> ",
                Self::escape(&thread.kind),
                Self::escape(&label)
            )
        };
        format!(
            "<details class=\"thread{resolved}\" id=\"t-{id}\"{open}><summary>{kind}{who} · {count}{state}</summary>{on}{comments}{answer}</details>",
            resolved = if thread.resolved { " resolved" } else { "" },
            id = Self::escape(&thread.id),
            open = if thread.resolved { "" } else { " open" },
            who = Self::escape(thread.comments.first().map_or("", |c| c.author.as_str())),
            state = if thread.resolved { " · resolved" } else { "" },
            answer = self.answer(notes, thread),
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
            kind: String::new(),
            resolved,
            comments: bodies
                .iter()
                .map(|b| Comment {
                    author: "Ada".into(),
                    email: "ada@x".into(),
                    time: 0,
                    body: (*b).into(),
                    ai: None,
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
        let notes = Notes::new(Path::new("a.md"), on, TEXT, &render);
        let shell = Shell::new("Site".into(), Links::Built);
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
        let rail = shell().rail(&Notes::new(Path::new("a.md"), on, TEXT, &render));
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
        let rail = shell().rail(&Notes::new(Path::new("a.md"), on, TEXT, &render));
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
        let rail = shell().rail(&Notes::new(Path::new("a.md"), on, TEXT, &render));
        assert!(
            !rail.contains("<b>Eve</b>") && rail.contains("&lt;b&gt;Eve"),
            "{rail}"
        );
        assert!(rail.contains("id=\"t-&lt;x&gt;\""), "{rail}");
    }

    #[test]
    fn where_comments_are_written_each_block_has_a_plus_to_begin_one() {
        let render = |b: &str| b.to_string();
        let notes = Notes::new(Path::new("a.md"), OnPage::default(), TEXT, &render);
        let start = TEXT.find("Two.").unwrap();
        assert_eq!(
            shell().comment_mark(&notes, &(start..start + 5)),
            r#"<a class="cmt add" href="?comment=4-5#composer" title="Comment on this paragraph">+</a>"#
        );
        let built = Shell::new("Site".into(), Links::Built);
        assert_eq!(built.comment_mark(&notes, &(start..start + 5)), "");
    }

    #[test]
    fn the_box_to_begin_a_thread_quotes_its_lines_and_comes_back_to_the_page() {
        let render = |b: &str| b.to_string();
        let notes = Notes::new(
            Path::new("arch/my page.md"),
            OnPage::default(),
            TEXT,
            &render,
        )
        .composing(Some(2..3));
        let rail = shell().rail(&notes);
        assert!(
            rail.contains(r#"<blockquote class="on">One.</blockquote>"#),
            "{rail}"
        );
        assert!(
            rail.contains(r#"name="page" value="arch/my page.md""#),
            "{rail}"
        );
        assert!(rail.contains(r#"name="lines" value="2-3""#), "{rail}");
        assert!(
            rail.contains(r#"name="back" value="/docs/arch/my%20page.html""#),
            "{rail}"
        );
    }

    #[test]
    fn where_comments_are_written_the_column_says_what_the_last_sync_did() {
        let render = |b: &str| b.to_string();
        let never = shell().rail(&Notes::new(
            Path::new("a.md"),
            OnPage::default(),
            TEXT,
            &render,
        ));
        assert!(
            never.contains(r#"<form class="sync" method="post" action="/_comments/sync">"#),
            "{never}"
        );
        assert!(never.contains("Not synced yet"), "{never}");
        let synced = Synced {
            time: When::now().0,
            brought: 0,
            said: "up to date with <origin>".into(),
        };
        let notes =
            Notes::new(Path::new("a.md"), OnPage::default(), TEXT, &render).synced(Some(synced));
        let rail = shell().rail(&notes);
        assert!(
            rail.contains("Synced just now: up to date with &lt;origin&gt;"),
            "{rail}"
        );
        let built = Shell::new("Site".into(), Links::Built).rail(&notes);
        assert!(!built.contains("/_comments/sync"), "{built}");
    }

    #[test]
    fn every_thread_is_listed_by_page_the_open_first() {
        let mut other = thread("b1", 0..1, false, &["On b"]);
        other.page = PathBuf::from("arch/b.md");
        let threads = [
            thread("done", 2..3, true, &["Old"]),
            thread("open", 4..5, false, &["New <one>", "reply"]),
            other,
        ];
        let page = shell().comments_page(&threads);
        assert!(
            page.contains("1 resolved") && page.contains("2 open") && page.contains("on 2 pages"),
            "{page}"
        );
        let (open, done) = (page.find("#t-open").unwrap(), page.find("#t-done").unwrap());
        assert!(open < done, "{page}");
        assert!(
            page.contains(r#"<a href="/docs/a.html#t-open">The quoted line.</a>"#),
            "{page}"
        );
        assert!(page.contains("<b>Ada</b>: New &lt;one&gt;"), "{page}");
        assert!(
            page.contains(r#"<h2><a href="/docs/arch/b.html">arch/b.md</a></h2>"#),
            "{page}"
        );
    }
}
