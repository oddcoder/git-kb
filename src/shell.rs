//! The page around every page: a bar with the site's name, linked to its top; a trail of the folders a page is
//! in; and the page itself, in a column narrow enough to read, styled by `assets/style.css`. A built site and a
//! served one look the same; only their links differ, as a built site's must work from disk.

use std::path::Path;

/// The site's style sheet, in the binary, and where it is in the site.
pub const STYLE: &str = include_str!("../assets/style.css");
pub const STYLE_PATH: &str = "_assets/style.css";

/// How a site's pages link to the rest of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Links {
    /// Served: from the site's top, `/arch/`, as a server answers for a folder.
    Served,
    /// Built: relative to the page, `../arch/index.html`, so that the site works from disk and on any host.
    Built,
}

/// The page around a site's pages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shell {
    /// The site's name, in its bar.
    name: String,
    links: Links,
    /// Where a built site lives, for its pages' canonical addresses.
    url: Option<String>,
}

impl Shell {
    /// What makes a served page load again when the site changes, once a burst of changes, as a save that
    /// writes twice, has settled. A built site has no server to tell it.
    const LIVE: &str = concat!(
        "<script>new EventSource(\"/_events\").onmessage = () => ",
        "{ clearTimeout(window.kbReload); window.kbReload = setTimeout(() => location.reload(), 100); };</script>\n"
    );

    #[must_use]
    pub fn new(name: String, links: Links) -> Self {
        Self {
            name,
            links,
            url: None,
        }
    }

    /// The same, for a built site that lives at `url`, ending in `/`: each page names its address there.
    #[must_use]
    pub fn living_at(self, url: Option<String>) -> Self {
        Self { url, ..self }
    }

    /// A page of the site, in the folder `at`, from the site's top, as `arch` for `arch/overview.md`, written
    /// as `file` there, or as the folder's own page when `file` is empty: its title, for the window, and its
    /// body, the page itself.
    #[must_use]
    pub fn page(&self, at: &Path, file: &str, title: &str, body: &str) -> String {
        let window = if title == self.name {
            Self::escape(title)
        } else {
            format!("{} · {}", Self::escape(title), Self::escape(&self.name))
        };
        format!(
            concat!(
                "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n",
                "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n",
                "<title>{window}</title>\n{canonical}<link rel=\"stylesheet\" href=\"{style}\">\n</head>\n<body>\n",
                "<header class=\"bar\"><a class=\"site\" href=\"{top}\">{name}</a></header>\n",
                "<main class=\"page\">\n{trail}{body}</main>\n{live}</body>\n</html>\n"
            ),
            live = match self.links {
                Links::Served => Self::LIVE,
                Links::Built => "",
            },
            window = window,
            canonical = self.canonical(at, file),
            style = self.to(at, STYLE_PATH),
            top = self.folder(at, Path::new("")),
            name = Self::escape(&self.name),
            trail = self.trail(at),
            body = body,
        )
    }

    /// A built page's address where its site lives, for search to know it by, when the site says where.
    fn canonical(&self, at: &Path, file: &str) -> String {
        let (Links::Built, Some(url)) = (self.links, &self.url) else {
            return String::new();
        };
        let folder = Self::encode(at);
        let folder = if folder.is_empty() {
            folder
        } else {
            format!("{folder}/")
        };
        let file = if file.eq_ignore_ascii_case("index.html") {
            String::new()
        } else {
            Self::encode(Path::new(file))
        };
        format!(
            "<link rel=\"canonical\" href=\"{}\">\n",
            Self::escape(&format!("{url}{folder}{file}"))
        )
    }

    /// The folders a page is in, each linked, from the top's down to its own; none for a page at the top.
    fn trail(&self, at: &Path) -> String {
        let mut parts = vec![format!(
            "<a href=\"{}\">{}</a>",
            self.folder(at, Path::new("")),
            Self::escape(&self.name)
        )];
        let mut folder = std::path::PathBuf::new();
        for part in at.components() {
            folder.push(part);
            parts.push(format!(
                "<a href=\"{}\">{}</a>",
                self.folder(at, &folder),
                Self::escape(&part.as_os_str().to_string_lossy())
            ));
        }
        if parts.len() == 1 {
            return String::new();
        }
        format!(
            "<nav class=\"crumbs\">{}</nav>\n",
            parts.join("<span class=\"sep\">/</span>")
        )
    }

    /// A link, from a page in the folder `at`, to a folder of the site.
    fn folder(&self, at: &Path, folder: &Path) -> String {
        let path = Self::encode(folder);
        let path = if path.is_empty() {
            path
        } else {
            format!("{path}/")
        };
        match self.links {
            Links::Served => format!("/{path}"),
            Links::Built => format!("{}{path}index.html", Self::up(at)),
        }
    }

    /// A link, from a page in the folder `at`, to a file of the site, by its path from the site's top.
    fn to(&self, at: &Path, file: &str) -> String {
        match self.links {
            Links::Served => format!("/{file}"),
            Links::Built => format!("{}{file}", Self::up(at)),
        }
    }

    /// The way up from a folder to the site's top: `../` for each folder it is in, or `./` at the top.
    fn up(at: &Path) -> String {
        match at.components().count() {
            0 => "./".into(),
            n => "../".repeat(n),
        }
    }

    /// A path as a link: each of its parts with what a URL does not hold as itself as %XX.
    fn encode(path: &Path) -> String {
        path.components()
            .map(|part| {
                part.as_os_str()
                    .to_string_lossy()
                    .bytes()
                    .map(|b| match b {
                        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                            char::from(b).to_string()
                        }
                        _ => format!("%{b:02X}"),
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("/")
    }

    fn escape(text: &str) -> String {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_has_the_style_the_bar_and_its_body() {
        let page =
            Shell::new("docs".into(), Links::Served).page(Path::new(""), "", "Home", "<p>Hi</p>\n");
        for wanted in [
            "<title>Home · docs</title>",
            r#"<link rel="stylesheet" href="/_assets/style.css">"#,
            r#"<a class="site" href="/">docs</a>"#,
            "<main class=\"page\">\n<p>Hi</p>\n</main>",
        ] {
            assert!(page.contains(wanted), "{wanted} missing from {page}");
        }
        assert!(
            !page.contains("crumbs"),
            "a page at the top has no trail: {page}"
        );
    }

    #[test]
    fn served_links_are_from_the_top() {
        let page =
            Shell::new("docs".into(), Links::Served).page(Path::new("arch/a b"), "", "DMA", "");
        assert!(page.contains(r#"<a href="/">docs</a><span class="sep">/</span><a href="/arch/">arch</a><span class="sep">/</span><a href="/arch/a%20b/">a b</a>"#), "{page}");
    }

    #[test]
    fn built_links_are_relative_so_the_site_works_from_disk() {
        let shell = Shell::new("docs".into(), Links::Built);
        let page = shell.page(Path::new("arch/deep"), "", "X", "");
        assert!(page.contains(r#"href="../../_assets/style.css""#), "{page}");
        assert!(
            page.contains(r#"<a class="site" href="../../index.html">docs</a>"#),
            "{page}"
        );
        assert!(
            page.contains(r#"<a href="../../arch/index.html">arch</a>"#),
            "{page}"
        );
        let top = shell.page(Path::new(""), "", "Home", "");
        assert!(top.contains(r#"href="./_assets/style.css""#), "{top}");
    }

    #[test]
    fn names_and_titles_are_escaped() {
        let page = Shell::new("a & b".into(), Links::Served).page(Path::new(""), "", "<x>", "");
        assert!(
            page.contains("<title>&lt;x&gt; · a &amp; b</title>"),
            "{page}"
        );
    }

    #[test]
    fn served_pages_load_again_when_the_site_changes_and_built_ones_do_not() {
        let served = Shell::new("docs".into(), Links::Served).page(Path::new(""), "", "A", "");
        assert!(
            served.contains(r#"new EventSource("/_events")"#),
            "{served}"
        );
        let built = Shell::new("docs".into(), Links::Built).page(Path::new(""), "", "A", "");
        assert!(!built.contains("EventSource"), "{built}");
    }

    #[test]
    fn a_built_page_names_its_address_where_its_site_lives() {
        let shell = Shell::new("docs".into(), Links::Built)
            .living_at(Some("https://foobar.xyz/docs/".into()));
        let page = shell.page(Path::new("arch"), "a b.html", "A", "");
        assert!(
            page.contains(
                r#"<link rel="canonical" href="https://foobar.xyz/docs/arch/a%20b.html">"#
            ),
            "{page}"
        );
        let folder = shell.page(Path::new("arch"), "", "arch", "");
        assert!(
            folder.contains(r#"href="https://foobar.xyz/docs/arch/">"#),
            "{folder}"
        );
        let served =
            Shell::new("docs".into(), Links::Served).living_at(Some("https://foobar.xyz/".into()));
        assert!(
            !served
                .page(Path::new(""), "", "A", "")
                .contains("canonical")
        );
        assert!(
            !Shell::new("docs".into(), Links::Built)
                .page(Path::new(""), "", "A", "")
                .contains("canonical")
        );
    }
}
