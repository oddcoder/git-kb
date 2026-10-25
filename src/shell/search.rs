//! Search, in the shell: the box in the bar, and the page of what it finds. A served site's server finds it; a
//! built site's page finds it in the browser, by `web/src/search.ts`, from the index beside it.

use std::path::Path;

use super::{Links, Shell};

impl Shell {
    /// The search box, in the bar: served, it asks the server, at the commit shown; built, the site's search page.
    pub(super) fn search_box(&self, at: &Path, query: &str) -> String {
        let (action, rev) = match self.links {
            Links::Served => (
                "/_search".to_string(),
                self.revision
                    .as_ref()
                    .map(|r| {
                        format!(
                            "<input type=\"hidden\" name=\"ref\" value=\"{}\">",
                            Self::escape(&r.name)
                        )
                    })
                    .unwrap_or_default(),
            ),
            Links::Built => (self.to(at, "_search.html"), String::new()),
        };
        format!(
            concat!(
                "<form class=\"search\" action=\"{action}\" role=\"search\">{rev}",
                "<input type=\"search\" name=\"q\" value=\"{query}\" placeholder=\"Search\" aria-label=\"Search\"></form>"
            ),
            action = action,
            rev = rev,
            query = Self::escape(query)
        )
    }

    /// The search page's body: Pagefind's, which searches the site's bundle in the browser and shows what it finds,
    /// ranked, with excerpts, its words marked, and filters; `web/src/search.ts` starts it with what was asked.
    /// `bundle` is where the site's bundle is, from the page; `base`, what its pages' addresses are under.
    #[must_use]
    pub fn search_page(&self, at: &Path, bundle: &str, base: &str) -> String {
        format!(
            concat!(
                "<h1>Search</h1>\n<link rel=\"stylesheet\" href=\"{bundle}/pagefind-ui.css\">\n",
                "<div id=\"kb-search\" data-bundle=\"{bundle}/\" data-base=\"{base}\"></div>\n",
                "<script src=\"{bundle}/pagefind-ui.js\"></script><script src=\"{script}\"></script>\n"
            ),
            bundle = Self::escape(bundle),
            base = Self::escape(base),
            script = self.asset(at, super::SEARCH_PATH)
        )
    }
}
