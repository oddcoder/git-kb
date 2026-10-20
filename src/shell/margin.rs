//! A page's margin: beside each block, when its lines last changed, and by whom, or that they are not committed
//! yet; clicked, a card of how the block changed then, and links to the rest of that commit's changes.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;

use super::{Links, Shell};
use crate::diff::Diff;
use crate::git::{Blame, Change, Repo, When};
use crate::render::{Mark, Moved, Renderer};

/// The margin of one page, mark by mark.
pub struct Margin<'a> {
    shell: &'a Shell,
    /// The folder the page is in, and the page.
    at: &'a Path,
    page: &'a Path,
    text: &'a str,
    blame: &'a Blame,
    /// Where each of the text's lines starts.
    starts: Vec<usize>,
    /// How many blocks have a mark, so that each card has an id of its own.
    count: Cell<usize>,
    repo: &'a Repo,
    renderer: &'a Renderer,
    moved: Moved<'a>,
    /// The page as it was before each commit, by the commit, read once.
    olds: RefCell<HashMap<String, String>>,
}

impl<'a> Margin<'a> {
    /// The margin of a page, of this text, whose lines `blame` says who last changed, the older versions it is
    /// compared with read from `repo`, and rendered with their links where `moved` says.
    #[must_use]
    pub fn new(
        shell: &'a Shell,
        page: &'a Path,
        text: &'a str,
        blame: &'a Blame,
        repo: &'a Repo,
        renderer: &'a Renderer,
        moved: Moved<'a>,
    ) -> Self {
        let starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        Self {
            shell,
            at: page.parent().unwrap_or(Path::new("")),
            page,
            text,
            blame,
            starts,
            count: Cell::new(0),
            repo,
            renderer,
            moved,
            olds: RefCell::default(),
        }
    }

    /// The mark beside the block at a range of the text: when its lines last changed, from the newest of them,
    /// or that one of them is not committed yet.
    #[must_use]
    pub fn mark(&self, range: Range<usize>) -> Option<Mark> {
        let line = |offset: usize| {
            self.starts
                .partition_point(|&s| s <= offset)
                .saturating_sub(1)
        };
        let (first, last) = (
            line(range.start),
            line(range.end.saturating_sub(1).max(range.start)),
        );
        let lines = self
            .blame
            .get(first..=last.min(self.blame.len().checked_sub(1)?))?;
        let change = if lines.iter().any(Option::is_none) {
            None
        } else {
            lines.iter().flatten().max_by_key(|c| c.time)
        };
        self.count.set(self.count.get() + 1);
        let id = format!("blk-{}", self.count.get());
        let (class, label, head, foot) = match change {
            None => (
                "new",
                "not committed yet".to_string(),
                "<b>Not committed yet</b>".to_string(),
                format!(
                    "<a href=\"{}\">Every uncommitted change to this page</a>",
                    self.shell.uncommitted(self.at, self.page)
                ),
            ),
            Some(c) => (
                "",
                format!(
                    "{} · {} · {}",
                    Shell::escape(&self.when(c)),
                    Shell::escape(&c.author),
                    c.short
                ),
                format!(
                    "<b>{}</b> · <span title=\"{}\">{}</span> · <code>{}</code> {}",
                    Shell::escape(&c.author),
                    c.date(),
                    Shell::escape(&self.when(c)),
                    c.short,
                    Shell::escape(&c.summary)
                ),
                format!(
                    "<a href=\"{}\">Everything this commit changed on the page</a>",
                    self.shell.changes(self.at, &c.short, self.page)
                ),
            ),
        };
        let diff = self.diff(change, &self.text[range]);
        Some(Mark {
            class,
            html: format!(
                concat!(
                    "<button class=\"blame\" popovertarget=\"{id}\" style=\"anchor-name: --{id}\">{label}</button>",
                    "<div class=\"pop\" id=\"{id}\" popover style=\"position-anchor: --{id}\">",
                    "<p class=\"pop-head\">{head}</p><div class=\"pop-body\">{diff}</div>",
                    "<p class=\"pop-foot\">{foot}</p></div>"
                ),
                id = id,
                label = label,
                head = head,
                diff = diff,
                foot = foot
            ),
        })
    }

    /// How a block changed: compared with the page before the commit that last changed it, or with the last
    /// commit, for changes not committed yet.
    fn diff(&self, change: Option<&Change>, block: &str) -> String {
        let rev = change.map_or_else(|| "HEAD".to_string(), |c| format!("{}^", c.id));
        let old = self
            .olds
            .borrow_mut()
            .entry(rev.clone())
            .or_insert_with(|| {
                self.repo
                    .snapshot(&rev)
                    .and_then(|at| self.repo.read(&at, self.page))
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .unwrap_or_default()
            })
            .clone();
        Diff::block(&old, self.text, block, self.renderer, self.moved)
    }

    /// When a commit was made: how long ago, served, or the day, built, as a built site is read long after.
    fn when(&self, change: &Change) -> String {
        match self.shell.links {
            Links::Served => change.ago(When::now().0),
            Links::Built => change.date(),
        }
    }
}
