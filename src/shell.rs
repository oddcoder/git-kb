//! The page around every page: a bar with the site's name, linked to its top, where `web/src/look.ts` puts the
//! Aa menu of its look; a sidebar of the page's headings and the site's pages; a trail of the folders a page is
//! in; and the page itself, in a column narrow enough to read, styled by `assets/style.css`. A built site and a
//! served one look the same; only their links differ, as a built site's must work from disk.

use std::fmt::Write;
use std::path::{Path, PathBuf};

use pulldown_cmark::HeadingLevel;

use crate::build::Builder;
use crate::config::LookConfig;
use crate::git::Revision;
use crate::nav::Nav;

mod comments;
mod compare;
mod history;
mod links;
mod margin;
mod search;

pub use comments::Notes;
pub use compare::{Comparison, Version};
pub use history::{PageHistory, PageState};
pub use margin::Margin;

/// The site's style sheet, in the binary, and where it is in the site.
pub const STYLE: &str = include_str!("../assets/style.css");
pub const STYLE_PATH: &str = "_assets/style.css";

/// The script of the pages' look, compiled from `web/src/look.ts` by `build.rs`, and where it is in the site.
pub const LOOK: &str = include_str!(concat!(env!("OUT_DIR"), "/look.js"));
pub const LOOK_PATH: &str = "_assets/look.js";

/// The script of the pages' layout, compiled from `web/src/layout.ts`, and where it is in the site.
pub const LAYOUT: &str = include_str!(concat!(env!("OUT_DIR"), "/layout.js"));
pub const LAYOUT_PATH: &str = "_assets/layout.js";

/// The script of a built site's search, compiled from `web/src/search.ts`, and where it is in the site.
pub const SEARCH: &str = include_str!(concat!(env!("OUT_DIR"), "/search.js"));
pub const SEARCH_PATH: &str = "_assets/search.js";

/// The script that draws the graph of pages, compiled from `web/src/graph.ts`, and where it is in the site.
pub const GRAPH: &str = include_str!(concat!(env!("OUT_DIR"), "/graph.js"));
pub const GRAPH_PATH: &str = "_assets/graph.js";

/// The script that keeps a served page live, compiled from `web/src/live.ts`, and where it is in the site.
pub const LIVE: &str = include_str!(concat!(env!("OUT_DIR"), "/live.js"));
pub const LIVE_PATH: &str = "_assets/live.js";

/// The files every page uses, by their paths in the site.
/// The script of plugins' figures, compiled from `web/src/figures.ts`: their zoom.
pub const FIGURES: &str = include_str!(concat!(env!("OUT_DIR"), "/figures.js"));
pub const FIGURES_PATH: &str = "_assets/figures.js";

pub const ASSETS: [(&str, &str); 7] = [
    (STYLE_PATH, STYLE),
    (LOOK_PATH, LOOK),
    (LAYOUT_PATH, LAYOUT),
    (SEARCH_PATH, SEARCH),
    (GRAPH_PATH, GRAPH),
    (LIVE_PATH, LIVE),
    (FIGURES_PATH, FIGURES),
];

/// How a site's pages link to the rest of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Links {
    /// Served: from the site's top, `/docs/arch/`, as a server answers for a folder.
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
    /// The site's look, before a reader chooses their own.
    look: LookConfig,
    /// The commit a served site is shown at, when it is not the folder as it is.
    revision: Option<Revision>,
    /// Where a built page is among the sites of the folder: `docs`, or `ref/main` for a commit's site, or `ref`
    /// for the list of them.
    place: PathBuf,
    /// Whether the folder is in a git repository, with revisions to list.
    revisions: bool,
    /// What the search box says: what was searched for, on the page of what it found.
    query: String,
}

impl Shell {
    #[must_use]
    pub fn new(name: String, links: Links) -> Self {
        Self {
            name,
            links,
            url: None,
            look: LookConfig::default(),
            revision: None,
            place: PathBuf::from("docs"),
            revisions: false,
            query: String::new(),
        }
    }

    /// The same, for a site shown at a commit: served, its links are under `/ref/name/`; built, it is in
    /// `ref/name/`; and its bar names it.
    #[must_use]
    pub fn showing(self, revision: Option<Revision>) -> Self {
        let place = revision
            .as_ref()
            .map_or_else(|| PathBuf::from("docs"), |r| Path::new("ref").join(&r.name));
        Self {
            revision,
            place,
            ..self
        }
    }

    /// The same, for the list of revisions, which a built site has in `ref/`.
    #[must_use]
    pub fn listing_revisions(self) -> Self {
        Self {
            revision: None,
            place: PathBuf::from("ref"),
            ..self
        }
    }

    /// The same, its search box saying what was searched for.
    #[must_use]
    pub fn asking(self, query: &str) -> Self {
        Self {
            query: query.to_owned(),
            ..self
        }
    }

    /// The same, for a folder in a git repository: its bar links to the list of its revisions.
    #[must_use]
    pub fn in_git(self, revisions: bool) -> Self {
        Self { revisions, ..self }
    }

    /// The same, for a built site that lives at `url`, ending in `/`: each page names its address there.
    #[must_use]
    pub fn living_at(self, url: Option<String>) -> Self {
        Self { url, ..self }
    }

    /// The site's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The same, in a look of its own.
    #[must_use]
    pub fn looking(self, look: LookConfig) -> Self {
        Self { look, ..self }
    }

    /// A page of the site, in the folder `at`, from the site's top, as `arch` for `arch/overview.md`, written
    /// as `file` there, or as the folder's own page when `file` is empty: its title, for the window; its body,
    /// the page itself; and what its sidebar shows.
    #[must_use]
    pub fn page(&self, at: &Path, file: &str, title: &str, body: &str, nav: &Nav) -> String {
        self.page_with_rail(at, file, title, body, nav, "")
    }

    /// The same, with a column beside the text, as of the page's comments, when `rail` is not empty.
    #[must_use]
    pub fn page_with_rail(
        &self,
        at: &Path,
        file: &str,
        title: &str,
        body: &str,
        nav: &Nav,
        rail: &str,
    ) -> String {
        let window = if title == self.name {
            Self::escape(title)
        } else {
            format!("{} · {}", Self::escape(title), Self::escape(&self.name))
        };
        format!(
            concat!(
                "<!doctype html>\n<html lang=\"en\"{look}>\n<head>\n<meta charset=\"utf-8\">\n",
                "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n",
                "<title>{window}</title>\n{canonical}<link rel=\"stylesheet\" href=\"{style}\">\n",
                "<script src=\"{script}\"></script>\n<script src=\"{layout}\"></script>\n<script src=\"{graph}\" defer></script>\n<script src=\"{figures}\" defer></script>\n{plugins}</head>\n<body>\n",
                "<header class=\"bar\"><a class=\"site\" href=\"{top}\">{name}</a>{revision}{revisions}{search}</header>\n",
                "<div class=\"layout{has_rail}\">\n{side}<main class=\"page\">\n{trail}{body}",
                "<div class=\"edge edge-left\" title=\"Drag to move the text; let go near the middle to centre it\"></div>",
                "<div class=\"edge edge-right\" title=\"Drag to set the text's width; double-click for the default\"></div>",
                "</main>\n{rail}<div class=\"edge edge-side\" title=\"Drag to size the sidebar, or close it; double-click for the default\"></div>\n",
                "</div>\n",
                "{live}</body>\n</html>\n"
            ),
            // A commit does not change; nor does a page of it.
            live = match (self.links, &self.revision) {
                (Links::Served, None) => format!("<script src=\"/{LIVE_PATH}\" defer></script>\n"),
                _ => String::new(),
            },
            revision = self.badge(at),
            revisions = self.revisions_link(at),
            search = self.search_box(at, &self.query),
            look = self.look.attributes(),
            window = window,
            canonical = self.canonical(at, file),
            style = self.asset(at, STYLE_PATH),
            script = self.asset(at, LOOK_PATH),
            layout = self.asset(at, LAYOUT_PATH),
            graph = self.asset(at, GRAPH_PATH),
            figures = self.asset(at, FIGURES_PATH),
            // The plugins whose blocks are on the page or its comments, each with its own files, beside the site's.
            plugins = crate::plugins::get().head(&[body, rail], &self.asset(at, "")),
            top = self.folder(at, Path::new("")),
            name = Self::escape(&self.name),
            side = self.side(at, nav),
            trail = self.trail(at),
            body = body,
            has_rail = if rail.is_empty() { "" } else { " has-rail" },
            rail = rail,
        )
    }

    /// The sidebar: the page's headings, when it has a few, and the tree of the site's pages.
    fn side(&self, at: &Path, nav: &Nav) -> String {
        let mut side = String::from("<aside class=\"side\">\n");
        side.push_str(&Self::contents(nav));
        side.push_str(&self.links(at, nav));
        let tree = self.tree(at, nav, Path::new(""));
        if tree != "<ul></ul>" {
            let _ = writeln!(
                side,
                "<nav class=\"tree\" aria-label=\"Pages\"><h2>Pages</h2>{tree}</nav>"
            );
        }
        side.push_str("</aside>\n");
        side
    }

    /// The page's headings, down to the third level, but for its title: none for a page with fewer than two.
    fn contents(nav: &Nav) -> String {
        let title = nav
            .headings
            .iter()
            .position(|h| h.level == HeadingLevel::H1);
        let items: Vec<String> = nav
            .headings
            .iter()
            .enumerate()
            .filter(|(i, h)| Some(*i) != title && h.level <= HeadingLevel::H3)
            .map(|(_, h)| {
                format!(
                    "<li data-level=\"{}\"><a href=\"#{}\">{}</a></li>",
                    h.level as usize,
                    Self::escape(&Self::encode(Path::new(&h.id))),
                    Self::escape(&h.text)
                )
            })
            .collect();
        if items.len() < 2 {
            return String::new();
        }
        format!(
            "<nav class=\"contents\" aria-label=\"On this page\"><h2>On this page</h2><ul>{}</ul></nav>\n",
            items.concat()
        )
    }

    /// A folder's pages, by title, and then its folders, each with its own, the page shown marked. A folder's
    /// own page is not among its pages: the folder's link is its link.
    fn tree(&self, at: &Path, nav: &Nav, folder: &Path) -> String {
        let (folders, pages) = nav.pages.entries(folder);
        let mut tree = String::from("<ul>");
        for (path, title) in pages {
            let _ = write!(
                tree,
                "<li><a href=\"{}\"{}>{}</a></li>",
                self.to(at, &Self::encode(&path.with_extension("html"))),
                Self::current(path == nav.current),
                Self::escape(title)
            );
        }
        for name in folders {
            let path = folder.join(&name);
            let home = nav.pages.home(&path);
            let here = path == nav.current || home == Some(nav.current);
            let _ = write!(
                tree,
                "<li><details open><summary><a href=\"{}\"{}>{}</a></summary>{}</details></li>",
                self.folder_shown(at, &path, home),
                Self::current(here),
                Self::escape(&name),
                self.tree(at, nav, &path)
            );
        }
        tree.push_str("</ul>");
        tree
    }

    /// A link to a folder, from a page in the folder `at`: in a built site, to its README's page itself when it is
    /// shown by it, as its `index.html` only redirects there.
    fn folder_shown(&self, at: &Path, folder: &Path, home: Option<&Path>) -> String {
        match (self.links, home) {
            (Links::Built, Some(home)) if !Builder::named(home, "index.md") => {
                self.to(at, &Self::encode(&home.with_extension("html")))
            }
            _ => self.folder(at, folder),
        }
    }

    fn current(here: bool) -> &'static str {
        if here { " aria-current=\"page\"" } else { "" }
    }

    /// A page's body with a switch to show it or its source instead, by the page alone, so a built site has it
    /// too.
    #[must_use]
    pub fn with_source(body: &str, source: &str) -> String {
        format!(
            concat!(
                "<input type=\"checkbox\" id=\"v-source\" class=\"toggle\">",
                "<label for=\"v-source\" class=\"view-switch\" title=\"Show the page or its Markdown source\">",
                "<span>Page</span><span>Source</span></label>\n",
                "<div class=\"p-page\">\n{body}</div>\n<div class=\"p-source\">\n{source}</div>\n"
            ),
            body = body,
            source = source
        )
    }

    /// A built page's address where its site lives, for search to know it by, when the site says where.
    fn canonical(&self, at: &Path, file: &str) -> String {
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
        self.address(&format!("{folder}{file}"))
            .map(|address| {
                format!(
                    "<link rel=\"canonical\" href=\"{}\">\n",
                    Self::escape(&address)
                )
            })
            .unwrap_or_default()
    }

    /// A built page's address where its site lives, by its path from the site's top, as a link, when the site
    /// says where it lives.
    #[must_use]
    pub fn address(&self, path: &str) -> Option<String> {
        let (Links::Built, Some(url)) = (self.links, &self.url) else {
            return None;
        };
        Some(format!("{url}{}/{path}", Self::encode(&self.place())))
    }

    /// The folders a page is in, each linked, from the top's down to its own; none for a page at the top.
    fn trail(&self, at: &Path) -> String {
        let mut parts = vec![format!(
            "<a href=\"{}\">{}</a>",
            self.folder(at, Path::new("")),
            Self::escape(&self.name)
        )];
        let mut folder = PathBuf::new();
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
            Links::Served => format!("{}/{path}", self.base()),
            Links::Built => format!("{}{path}index.html", Self::up(at)),
        }
    }

    /// A link, from a page in the folder `at`, to a file of the site, by its path from the site's top.
    fn to(&self, at: &Path, file: &str) -> String {
        match self.links {
            Links::Served => format!("{}/{file}", self.base()),
            Links::Built => format!("{}{file}", Self::up(at)),
        }
    }

    /// Where a served site's pages are: `/docs` for the folder as it is, or `/ref/name` for a commit, its name
    /// as one part.
    fn base(&self) -> String {
        self.revision.as_ref().map_or_else(
            || "/docs".into(),
            |r| format!("/ref/{}", Self::encode_part(&r.name)),
        )
    }

    /// A link, from a page in the folder `at`, to one of git-kb's own files, by its path: they are beside the
    /// sites, not among their pages, so that none of the docs' own can be taken for one.
    fn asset(&self, at: &Path, path: &str) -> String {
        match self.links {
            Links::Served => format!("/{path}"),
            Links::Built => format!("{}{path}", self.out(at)),
        }
    }

    /// The commit shown, in the bar, linked to the site as it is: its name, and its id when that is not its name.
    fn badge(&self, at: &Path) -> String {
        let Some(revision) = &self.revision else {
            return String::new();
        };
        let id = if revision.short == revision.name {
            String::new()
        } else {
            format!(" · {}", revision.short)
        };
        format!(
            "<a class=\"revision\" href=\"{}\" title=\"The site at commit {}; open it as it is now\">@ {}{id}</a>",
            self.docs(at),
            revision.id,
            Self::escape(&revision.name)
        )
    }

    /// Where a site is among the sites of a folder: `docs` for the folder as it is, or `ref/main` for a commit,
    /// its name's parts as folders.
    fn place(&self) -> PathBuf {
        self.place.clone()
    }

    /// The link to the list of revisions, in the bar, when there are some.
    fn revisions_link(&self, at: &Path) -> String {
        if !self.revisions {
            return String::new();
        }
        let href = match self.links {
            Links::Served => "/ref/".to_string(),
            Links::Built => format!("{}ref/index.html", self.out(at)),
        };
        format!("<a class=\"revisions\" href=\"{href}\">Revisions</a>")
    }

    /// A link, from a page in the folder `at`, to the site as it is.
    fn docs(&self, at: &Path) -> String {
        match self.links {
            Links::Served => "/docs/".into(),
            Links::Built => format!("{}docs/index.html", self.out(at)),
        }
    }

    /// The way up from a folder of a built site to the folder its sites are in.
    fn out(&self, at: &Path) -> String {
        "../".repeat(at.components().count() + self.place().components().count())
    }

    /// The way up from a folder to the site's top: `../` for each folder it is in, or `./` at the top.
    fn up(at: &Path) -> String {
        match at.components().count() {
            0 => "./".into(),
            n => "../".repeat(n),
        }
    }

    /// A path as a link: each of its parts with what a URL does not hold as itself as %XX.
    /// A thread's id, as part of an address: `#t-<id>`.
    #[must_use]
    pub fn encode_id(id: &str) -> String {
        Self::encode_part(id)
    }

    fn encode(path: &Path) -> String {
        path.components()
            .map(|part| Self::encode_part(&part.as_os_str().to_string_lossy()))
            .collect::<Vec<_>>()
            .join("/")
    }

    /// Text as one part of a link, a `/` in it too as %XX.
    fn encode_part(text: &str) -> String {
        text.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    char::from(b).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect()
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
    use crate::nav::Pages;
    use crate::render::Renderer;

    /// A site's pages, each titled by its path's last part's stem, made in a folder that is removed after.
    fn pages(paths: &[&str]) -> Pages {
        let dir = tempfile::tempdir().unwrap();
        let files: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        for file in &files {
            let path = dir.path().join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let stem = file.file_stem().unwrap().to_string_lossy();
            std::fs::write(path, format!("# {stem}\n")).unwrap();
        }
        Pages::new(&files, &Renderer::new(), |f| {
            std::fs::read_to_string(dir.path().join(f)).ok()
        })
    }

    #[test]
    fn the_sidebar_has_the_page_s_headings_but_its_title() {
        let page = Renderer::new().render("# Title\n## One\n### Deep\n#### Too deep\n");
        let nav = Nav {
            headings: &page.headings,
            ..Nav::none()
        };
        let html = Shell::new("docs".into(), Links::Served).page(Path::new(""), "", "T", "", &nav);
        assert!(
            html.contains(concat!(
                r##"<h2>On this page</h2><ul><li data-level="2"><a href="#one">One</a></li>"##,
                r##"<li data-level="3"><a href="#deep">Deep</a></li></ul>"##
            )),
            "{html}"
        );
        let few = Renderer::new().render("# Title\n## Only\n");
        let nav = Nav {
            headings: &few.headings,
            ..Nav::none()
        };
        let html = Shell::new("docs".into(), Links::Served).page(Path::new(""), "", "T", "", &nav);
        assert!(!html.contains("On this page"), "{html}");
    }

    #[test]
    fn the_sidebar_has_the_site_s_pages_and_marks_the_one_shown() {
        let pages = pages(&["index.md", "b.md", "arch/README.md", "arch/dma.md"]);
        let nav = Nav {
            pages: &pages,
            current: Path::new("arch/dma.md"),
            headings: &[],
            graph: None,
        };
        let served =
            Shell::new("docs".into(), Links::Served).page(Path::new("arch"), "", "", "", &nav);
        assert!(
            served.contains(concat!(
                r#"<h2>Pages</h2><ul><li><a href="/docs/b.html">b</a></li>"#,
                r#"<li><details open><summary><a href="/docs/arch/">arch</a></summary>"#,
                r#"<ul><li><a href="/docs/arch/dma.html" aria-current="page">dma</a></li></ul></details></li></ul>"#
            )),
            "{served}"
        );
        let built =
            Shell::new("docs".into(), Links::Built).page(Path::new("arch"), "", "", "", &nav);
        assert!(built.contains(r#"<a href="../b.html">b</a>"#), "{built}");
        assert!(
            built.contains(r#"<summary><a href="../arch/README.html">arch</a></summary>"#),
            "a folder shown by its README links to its page, not to the page that redirects there: {built}"
        );
        let readme = Nav {
            current: Path::new("arch/README.md"),
            ..nav
        };
        let folder =
            Shell::new("docs".into(), Links::Served).page(Path::new("arch"), "", "", "", &readme);
        assert!(
            folder.contains(r#"<a href="/docs/arch/" aria-current="page">arch</a>"#),
            "{folder}"
        );
    }

    #[test]
    fn a_page_has_the_style_the_bar_and_its_body() {
        let page = Shell::new("docs".into(), Links::Served).page(
            Path::new(""),
            "",
            "Home",
            "<p>Hi</p>\n",
            &Nav::none(),
        );
        for wanted in [
            "<title>Home · docs</title>",
            r#"<link rel="stylesheet" href="/_assets/style.css">"#,
            r#"<a class="site" href="/docs/">docs</a>"#,
            "<main class=\"page\">\n<p>Hi</p>\n<div class=\"edge edge-left\"",
        ] {
            assert!(page.contains(wanted), "{wanted} missing from {page}");
        }
        assert!(
            !page.contains("crumbs"),
            "a page at the top has no trail: {page}"
        );
    }

    #[test]
    fn a_page_has_its_look_and_its_script() {
        let look = LookConfig {
            palette: Some("nord".into()),
            ..LookConfig::default()
        };
        let page = Shell::new("docs".into(), Links::Built).looking(look).page(
            Path::new("arch"),
            "",
            "A",
            "",
            &Nav::none(),
        );
        for wanted in [
            r#"<html lang="en" data-palette="nord">"#,
            r#"<script src="../../_assets/look.js"></script>"#,
        ] {
            assert!(page.contains(wanted), "{wanted} missing from {page}");
        }
    }

    #[test]
    fn a_page_has_its_layout_s_script_and_edges() {
        let page = Shell::new("docs".into(), Links::Built).page(
            Path::new("arch"),
            "",
            "A",
            "",
            &Nav::none(),
        );
        for wanted in [
            r#"<script src="../../_assets/layout.js"></script>"#,
            r#"<div class="edge edge-left""#,
            r#"<div class="edge edge-right""#,
            r#"<div class="edge edge-side""#,
        ] {
            assert!(page.contains(wanted), "{wanted} missing from {page}");
        }
    }

    #[test]
    fn served_links_are_from_the_top() {
        let page = Shell::new("docs".into(), Links::Served).page(
            Path::new("arch/a b"),
            "",
            "DMA",
            "",
            &Nav::none(),
        );
        assert!(page.contains(r#"<a href="/docs/">docs</a><span class="sep">/</span><a href="/docs/arch/">arch</a><span class="sep">/</span><a href="/docs/arch/a%20b/">a b</a>"#), "{page}");
    }

    #[test]
    fn built_links_are_relative_so_the_site_works_from_disk() {
        let shell = Shell::new("docs".into(), Links::Built);
        let page = shell.page(Path::new("arch/deep"), "", "X", "", &Nav::none());
        assert!(
            page.contains(r#"href="../../../_assets/style.css""#),
            "git-kb's files are beside the site, out of its docs/: {page}"
        );
        assert!(
            page.contains(r#"<a class="site" href="../../index.html">docs</a>"#),
            "{page}"
        );
        assert!(
            page.contains(r#"<a href="../../arch/index.html">arch</a>"#),
            "{page}"
        );
        let top = shell.page(Path::new(""), "", "Home", "", &Nav::none());
        assert!(top.contains(r#"href="../_assets/style.css""#), "{top}");
        let at_a_tag = shell
            .showing(Some(Revision {
                name: "v1/rc".into(),
                id: gix::ObjectId::null(gix::hash::Kind::Sha1),
                short: "0000000".into(),
            }))
            .page(Path::new("arch"), "", "X", "", &Nav::none());
        assert!(
            at_a_tag.contains(r#"href="../../../../_assets/style.css""#),
            "a tag's site is in ref/v1/rc/: {at_a_tag}"
        );
        assert!(
            at_a_tag.contains(r#"<a class="revision" href="../../../../docs/index.html""#),
            "{at_a_tag}"
        );
    }

    #[test]
    fn names_and_titles_are_escaped() {
        let page = Shell::new("a & b".into(), Links::Served).page(
            Path::new(""),
            "",
            "<x>",
            "",
            &Nav::none(),
        );
        assert!(
            page.contains("<title>&lt;x&gt; · a &amp; b</title>"),
            "{page}"
        );
    }

    #[test]
    fn served_pages_keep_live_and_built_ones_do_not() {
        let served =
            Shell::new("docs".into(), Links::Served).page(Path::new(""), "", "A", "", &Nav::none());
        assert!(
            served.contains(r#"<script src="/_assets/live.js" defer></script>"#),
            "{served}"
        );
        let built =
            Shell::new("docs".into(), Links::Built).page(Path::new(""), "", "A", "", &Nav::none());
        assert!(!built.contains("live.js"), "{built}");
    }

    #[test]
    fn a_built_page_names_its_address_where_its_site_lives() {
        let shell = Shell::new("docs".into(), Links::Built)
            .living_at(Some("https://foobar.xyz/kb/".into()));
        let page = shell.page(Path::new("arch"), "a b.html", "A", "", &Nav::none());
        assert!(
            page.contains(
                r#"<link rel="canonical" href="https://foobar.xyz/kb/docs/arch/a%20b.html">"#
            ),
            "{page}"
        );
        let folder = shell.page(Path::new("arch"), "", "arch", "", &Nav::none());
        assert!(
            folder.contains(r#"href="https://foobar.xyz/kb/docs/arch/">"#),
            "{folder}"
        );
        let served =
            Shell::new("docs".into(), Links::Served).living_at(Some("https://foobar.xyz/".into()));
        assert!(
            !served
                .page(Path::new(""), "", "A", "", &Nav::none())
                .contains("canonical")
        );
        assert!(
            !Shell::new("docs".into(), Links::Built)
                .page(Path::new(""), "", "A", "", &Nav::none())
                .contains("canonical")
        );
    }
}
