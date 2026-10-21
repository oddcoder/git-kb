//! Two versions of a page, compared, in its shell: tabs for the changes marked in place, each version whole, and
//! the source line by line.

use std::path::Path;

use super::{Links, Shell};
use crate::diff::Diff;

/// One of the two versions compared: how it is named, and where it is, when it is anywhere.
#[derive(Debug, Clone)]
pub struct Version {
    pub label: String,
    pub href: Option<String>,
}

/// Two versions of a page compared, and each of them whole, as HTML, and its source.
#[derive(Debug, Clone)]
pub struct Comparison<'a> {
    /// The page's title.
    pub title: &'a str,
    pub from: Version,
    pub to: Version,
    pub diff: &'a Diff,
    pub before: String,
    pub after: String,
    /// The source, compared line by line.
    pub lines: String,
}

impl Shell {
    /// A comparison of two versions of a page, as a page's body.
    #[must_use]
    pub fn compare(&self, comparison: &Comparison) -> String {
        let count = match comparison.diff.changes {
            0 => "No changes".to_string(),
            1 => "1 change".into(),
            n => format!("{n} changes"),
        };
        let changes = if comparison.diff.changes == 0 {
            "<p class=\"muted\">The two versions are the same.</p>".to_string()
        } else {
            comparison.diff.html.clone()
        };
        let version = |v: &Version| match &v.href {
            Some(href) => format!("<a href=\"{href}\">{}</a>", Self::escape(&v.label)),
            None => Self::escape(&v.label),
        };
        format!(
            concat!(
                "<h1>Changes to {title}</h1>\n",
                "<p class=\"meta\">{count} from {from} to {to}</p>\n",
                "<div class=\"versions\"><input type=\"radio\" name=\"version\" id=\"v-changes\" checked>",
                "<input type=\"radio\" name=\"version\" id=\"v-before\"><input type=\"radio\" name=\"version\" id=\"v-after\">",
                "<input type=\"radio\" name=\"version\" id=\"v-lines\">",
                "<nav class=\"tabs\" aria-label=\"Versions\"><label for=\"v-changes\">Changes</label>",
                "<label for=\"v-before\">Before</label><label for=\"v-after\">After</label>",
                "<label for=\"v-lines\">Source</label></nav>\n",
                "<div class=\"panel p-changes\">{changes}</div>\n<div class=\"panel p-before\">{before}</div>\n",
                "<div class=\"panel p-after\">{after}</div>\n<div class=\"panel p-lines\">{lines}</div></div>\n"
            ),
            title = Self::escape(comparison.title),
            count = count,
            from = version(&comparison.from),
            to = version(&comparison.to),
            changes = changes,
            before = Diff::without_ids(&comparison.before),
            after = Diff::without_ids(&comparison.after),
            lines = comparison.lines
        )
    }

    /// A link, from a page in the folder `at`, to what a commit changed in a page: served, the page, as it is shown,
    /// asked for it at the commit compared with it at the commit before; built, that comparison's page beside it.
    #[must_use]
    pub fn changes(&self, at: &Path, short: &str, page: &Path) -> String {
        match self.links {
            // On the page as it is shown, so that the site stays as it is.
            Links::Served => format!(
                "{}?diff={}%5E&amp;to={}",
                self.to(at, &Self::encode(&page.with_extension("html"))),
                Self::encode_part(short),
                Self::encode_part(short)
            ),
            Links::Built => self.to(at, &Self::encode(&Self::compared(page, short))),
        }
    }

    /// A link, from a page in the folder `at`, to how a page differs from the last commit.
    #[must_use]
    pub fn uncommitted(&self, at: &Path, page: &Path) -> String {
        match self.links {
            Links::Served => format!("{}?diff=HEAD", self.to(at, &Self::encode(page))),
            Links::Built => self.to(at, &Self::encode(&Self::compared(page, "uncommitted"))),
        }
    }

    /// A link, from a page in the folder `at`, to a page of the site, by its path from the site's top.
    #[must_use]
    pub fn link(&self, at: &Path, page: &Path) -> String {
        self.to(at, &Self::encode(page))
    }

    /// Where a built site has a page compared, beside the page: `arch/overview~3f2a1c9.html`.
    #[must_use]
    pub fn compared(page: &Path, with: &str) -> std::path::PathBuf {
        let stem = page.file_stem().unwrap_or_default().to_string_lossy();
        page.with_file_name(format!("{stem}~{with}.html"))
    }
}
