//! A page's history, in its shell: the line under its title that says when it last changed, the list of the
//! commits that changed it, and the site's own list of its branches, tags and commits. A served site links each
//! commit to the site as it was then, under `/ref/`; a built one has only its branches and tags, so it names
//! commits plainly.

use std::fmt::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use super::{Links, Shell};
use crate::git::{Change, History, Revision, Snapshot};

/// Where a page stands against the last commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageState {
    /// As the last commit has it, or shown at a commit.
    Committed,
    /// Changed since.
    Edited,
    /// Not in the last commit at all.
    New,
}

/// What is known of a page's history: where it stands, and the commits that changed it, newest first.
#[derive(Debug, Clone)]
pub struct PageHistory<'a> {
    pub state: PageState,
    pub changes: Vec<&'a Change>,
}

impl<'a> PageHistory<'a> {
    /// A page's, of these bytes: against `head`, the last commit, when the folder as it is is shown, and with
    /// its commits from `history`.
    #[must_use]
    pub fn of(
        head: Option<&Snapshot>,
        history: Option<&'a History>,
        page: &Path,
        bytes: &[u8],
    ) -> Self {
        let state = match head.map(|head| head.holds(page, bytes)) {
            Some(None) => PageState::New,
            Some(Some(false)) => PageState::Edited,
            Some(Some(true)) | None => PageState::Committed,
        };
        Self {
            state,
            changes: history.iter().flat_map(|h| h.of(page)).collect(),
        }
    }
}

impl Shell {
    /// A page's body with the line that says when it last changed under its title, or at its top.
    #[must_use]
    pub fn noted(&self, at: &Path, page: &Path, body: &str, history: &PageHistory) -> String {
        let meta = self.meta(at, page, history);
        match body.find("</h1>") {
            Some(end) => {
                let end = end + "</h1>".len();
                format!("{}\n{meta}{}", &body[..end], &body[end..])
            }
            None => format!("{meta}{body}"),
        }
    }

    /// When a page last changed, and by whom, in what commit; whether it has changed since; and, served, a
    /// link to its history.
    fn meta(&self, at: &Path, page: &Path, history: &PageHistory) -> String {
        let mut parts = vec![];
        match history.state {
            PageState::New => parts.push("New page, not committed yet".to_string()),
            PageState::Edited => parts.push("Changed since the last commit".to_string()),
            PageState::Committed => {}
        }
        if let Some(last) = history.changes.first() {
            let commit = match self.commit(&last.short, page) {
                Some(link) => format!(
                    "<a href=\"{link}\" title=\"{}\">{}</a>",
                    Self::escape(&last.summary),
                    last.short
                ),
                None => format!(
                    "<code title=\"{}\">{}</code>",
                    Self::escape(&last.summary),
                    last.short
                ),
            };
            // A built site is read long after it is built: when, by the day.
            let when = match self.links {
                Links::Served => format!(
                    "<span title=\"{}\">{}</span>",
                    last.date(),
                    last.ago(Self::now())
                ),
                Links::Built => format!("on {}", last.date()),
            };
            parts.push(format!(
                "Last changed {when} by {} in {commit}",
                Self::escape(&last.author)
            ));
        }
        if self.links == Links::Served && !history.changes.is_empty() {
            parts.push(format!(
                "<a href=\"{}?history\">History</a>",
                self.to(at, &Self::encode(page))
            ));
        }
        if parts.is_empty() {
            return String::new();
        }
        format!("<p class=\"meta\">{}</p>\n", parts.join(" · "))
    }

    /// A page's history, as a page of its own: the commits that changed it, newest first, each a link to the
    /// page as it was then.
    #[must_use]
    pub fn history(&self, at: &Path, page: &Path, title: &str, history: &PageHistory) -> String {
        let mut rows = String::new();
        if history.state != PageState::Committed {
            let said = if history.state == PageState::New {
                "a new page"
            } else {
                "changed since"
            };
            let _ = write!(
                rows,
                "<li class=\"pending\"><b>Not committed yet</b> <small>{said}</small></li>"
            );
        }
        for change in &history.changes {
            let said = format!(
                "<code>{}</code> {}",
                change.short,
                Self::escape(&change.summary)
            );
            let said = match self.commit(&change.short, page) {
                Some(href) => format!("<a href=\"{href}\">{said}</a>"),
                None => said,
            };
            let _ = write!(
                rows,
                "<li>{said} <small>{} · {}</small></li>",
                Self::escape(&change.author),
                change.date()
            );
        }
        if rows.is_empty() {
            rows.push_str("<li class=\"muted\">No commits yet.</li>");
        }
        format!(
            "<h1>History of <a href=\"{}\">{}</a></h1>\n<ul class=\"log\">{rows}</ul>\n",
            self.to(at, &Self::encode(page)),
            Self::escape(title)
        )
    }

    /// The site's branches and tags, each a link to the site at it, and then every commit that changed its
    /// pages, with the pages it changed.
    #[must_use]
    pub fn revisions(&self, refs: &[Revision], history: &History) -> String {
        let mut body = String::from("<h1>Revisions</h1>\n<ul class=\"refs\">");
        for revision in refs {
            let _ = write!(
                body,
                "<li><a href=\"{}\">{}</a> <code>{}</code></li>",
                self.revision(revision),
                Self::escape(&revision.name),
                revision.short
            );
        }
        body.push_str("</ul>\n<h2>Commits</h2>\n<ul class=\"log\">");
        for (change, pages) in &history.log {
            let pages: Vec<String> = pages
                .iter()
                .map(|page| {
                    let name = Self::escape(&page.to_string_lossy());
                    match self.commit(&change.short, page) {
                        Some(link) => format!("<a href=\"{link}\">{name}</a>"),
                        None => name,
                    }
                })
                .collect();
            let _ = write!(
                body,
                "<li><code>{}</code> {} <small>{} · {}</small><div>{}</div></li>",
                change.short,
                Self::escape(&change.summary),
                Self::escape(&change.author),
                change.date(),
                pages.join(", ")
            );
        }
        body.push_str("</ul>\n");
        body
    }

    /// A link to a page as it was at a commit: served only, as a built site has its branches and tags, and no
    /// other commits.
    fn commit(&self, short: &str, page: &Path) -> Option<String> {
        (self.links == Links::Served).then(|| {
            format!(
                "/ref/{}/{}",
                Self::encode_part(short),
                Self::encode(&page.with_extension("html"))
            )
        })
    }

    /// A link, from the list of revisions, to the site at a branch or tag.
    fn revision(&self, revision: &Revision) -> String {
        match self.links {
            Links::Served => format!("/ref/{}/", Self::encode_part(&revision.name)),
            // The list is at ref/index.html, beside the branches' and tags' own folders.
            Links::Built => format!("{}/index.html", Self::encode(Path::new(&revision.name))),
        }
    }

    /// The time now, in seconds since 1970.
    fn now() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
    }
}
