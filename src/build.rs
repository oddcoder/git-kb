//! `git kb build`: a folder of Markdown written as a static site, a folder of files to open from disk or put on
//! any host. Each page is written as HTML beside where its Markdown is, `arch/overview.md` as
//! `arch/overview.html`, with its links to other pages made links to theirs, and every other file, such as an
//! image, is copied as it is. Every folder with pages in it, or below it, gets an `index.html`: its
//! `index.md`; or else a page that redirects to its `README.md`'s, as a forge shows a folder by its README;
//! or else a list of its pages and folders. Links to such a folder go to its README's page itself.
//! What `.gitignore` and
//! `.kbignore` files ignore is left out: `.gitignore` for what git leaves out too, such as `target/`, and
//! `.kbignore`, in the same form, for what only the site does. So is `.git`, git's own store, and links,
//! which are neither followed nor copied.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use ignore::WalkBuilder;
use tracing::debug;

use crate::render::Renderer;

/// What a build wrote.
#[derive(Debug, PartialEq, Eq)]
pub struct Built {
    /// The pages rendered.
    pub pages: usize,
    /// The other files copied.
    pub files: usize,
    /// The folders given an `index.html` of their `README.md`, or of a list of what is in them.
    pub folders: usize,
}

/// What is in a folder of the site, for its list: its pages, as their file's name and title, and the
/// folders in it that have pages.
#[derive(Debug, Default)]
pub(crate) struct Folder {
    pub(crate) pages: BTreeMap<String, String>,
    pub(crate) folders: BTreeSet<String>,
}

/// Builds folders of Markdown as sites.
#[derive(Debug, Default)]
pub struct Builder {
    renderer: Renderer,
}

impl Builder {
    /// A file in the output folder that says a build wrote it, so that building again may empty it, and
    /// nothing else ever is.
    const MARK: &str = ".git-kb-site";

    /// The files, in `.gitignore`'s form, that say what the site leaves out and git does not.
    pub const IGNORE: &str = ".kbignore";

    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Writes a folder of Markdown as a site, into `out`, in place of whatever an earlier build wrote there.
    /// An output folder inside the folder built is not built itself.
    ///
    /// # Errors
    ///
    /// When `out` holds files no build wrote, or holds the folder built, or a file cannot be read or written.
    pub fn build(&self, folder: &Path, out: &Path) -> anyhow::Result<Built> {
        let folder = fs::canonicalize(folder).with_context(|| folder.display().to_string())?;
        Self::prepare(out, &folder)?;
        let out = fs::canonicalize(out).with_context(|| out.display().to_string())?;
        let mut built = Built {
            pages: 0,
            files: 0,
            folders: 0,
        };
        let mut folders: BTreeMap<PathBuf, Folder> = BTreeMap::new();

        // Every file first, so that each page knows, as it is rendered, which folders are shown by their
        // README: those with one and without an index.md, each with its README's page.
        let files = Self::files(&folder, Some(&out))?;
        let readmes = Self::readmes(&files);

        for path in &files {
            let from = folder.join(path);
            let markdown = path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("md"));
            let written = if markdown {
                path.with_extension("html")
            } else {
                path.clone()
            };
            let to = out.join(&written);
            if let Some(dir) = to.parent() {
                fs::create_dir_all(dir).with_context(|| dir.display().to_string())?;
            }
            if markdown {
                let text = fs::read_to_string(&from).with_context(|| from.display().to_string())?;
                let (html, title) = self.page(&text, path, &readmes);
                fs::write(&to, html).with_context(|| to.display().to_string())?;
                debug!(page = %path.display(), "rendered");
                built.pages += 1;
                Self::file_under(&mut folders, &written, title);
            } else {
                fs::copy(&from, &to).with_context(|| to.display().to_string())?;
                debug!(file = %path.display(), "copied");
                built.files += 1;
            }
        }
        for (path, folder) in &folders {
            if Self::index(&out, path, folder)? {
                built.folders += 1;
            }
        }
        fs::write(
            out.join(Self::MARK),
            "Written by git kb build, which replaces everything here when it builds again.\n",
        )?;
        Ok(built)
    }

    /// The files of a folder that are its site's, by their paths in it: all but what `.gitignore` and
    /// `.kbignore` files ignore, in it and above it, `.git`, links, and `skip`, a folder inside it to leave
    /// out, as the output folder is. The server serves these, and no others.
    pub(crate) fn files(folder: &Path, skip: Option<&Path>) -> anyhow::Result<Vec<PathBuf>> {
        let skip = skip.map(Path::to_path_buf);
        let walk = WalkBuilder::new(folder)
            .standard_filters(false)
            .git_ignore(true)
            .parents(true)
            .require_git(false)
            .add_custom_ignore_filename(Self::IGNORE)
            .filter_entry(move |entry| {
                skip.as_deref() != Some(entry.path()) && entry.file_name() != ".git"
            })
            .build();
        let mut files = vec![];
        for entry in walk {
            let entry = entry?;
            if entry.file_type().is_some_and(|t| t.is_file()) {
                files.push(entry.path().strip_prefix(folder)?.to_path_buf());
            }
        }
        Ok(files)
    }

    /// The output folder, ready to write: made when it is missing, and emptied when a build wrote it.
    fn prepare(out: &Path, folder: &Path) -> anyhow::Result<()> {
        if !out.exists() {
            return fs::create_dir_all(out).with_context(|| out.display().to_string());
        }
        let full = fs::canonicalize(out).with_context(|| out.display().to_string())?;
        if folder.starts_with(&full) {
            bail!(
                "{}: the output folder holds the folder being built",
                out.display()
            );
        }
        if fs::read_dir(out)?.next().is_none() {
            return Ok(());
        }
        if !out.join(Self::MARK).is_file() {
            bail!(
                "{}: it holds files git kb build did not write; empty it, or build elsewhere",
                out.display()
            );
        }
        fs::remove_dir_all(out)
            .and_then(|()| fs::create_dir(out))
            .with_context(|| out.display().to_string())
    }

    /// A page's HTML document, and its title: the Markdown rendered, titled by its first heading, or else by
    /// its file's name.
    fn page(
        &self,
        markdown: &str,
        path: &Path,
        readmes: &BTreeMap<PathBuf, String>,
    ) -> (String, String) {
        let page = self
            .renderer
            .render_with(markdown, &|dest| Self::moved(path, dest, readmes));
        let name = path.file_stem().unwrap_or_default().to_string_lossy();
        let title = page.title.unwrap_or_else(|| name.into_owned());
        (Self::document(&title, &page.html), title)
    }

    /// An HTML document with a title and a body.
    pub(crate) fn document(title: &str, body: &str) -> String {
        format!(
            "<!doctype html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n<title>{}</title>\n</head>\n<body>\n{body}</body>\n</html>\n",
            Self::escape(title)
        )
    }

    /// A page filed under its folder, by the name of the HTML file it was written as and its title, and each
    /// folder above it under the one above that, so that each has a list.
    fn file_under(folders: &mut BTreeMap<PathBuf, Folder>, written: &Path, title: String) {
        let folder = written.parent().unwrap_or(Path::new(""));
        let name = written
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        folders
            .entry(folder.to_path_buf())
            .or_default()
            .pages
            .insert(name, title);
        let mut below = folder;
        while let Some(above) = below.parent() {
            let name = below
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            folders
                .entry(above.to_path_buf())
                .or_default()
                .folders
                .insert(name);
            below = above;
        }
    }

    /// A folder's `index.html`, unless its `index.md` is it already: a page that redirects to its README's,
    /// or else a list of its folders and its pages, by title. Answers whether it wrote one.
    fn index(out: &Path, path: &Path, folder: &Folder) -> anyhow::Result<bool> {
        let named = |name: &str| folder.pages.keys().find(|p| p.eq_ignore_ascii_case(name));
        if named("index.html").is_some() {
            return Ok(false);
        }
        let dir = out.join(path);
        let index = dir.join("index.html");
        if let Some(readme) = named("README.html") {
            fs::write(&index, Self::redirect(&folder.pages[readme], readme))
                .with_context(|| index.display().to_string())?;
            debug!(folder = %path.display(), "redirected to its README");
            return Ok(true);
        }
        fs::write(&index, Self::listing(path, folder))
            .with_context(|| index.display().to_string())?;
        debug!(folder = %path.display(), "indexed");
        Ok(true)
    }

    /// A folder's list: its folders, each a link to its `index.html`, and then its pages, by title.
    pub(crate) fn listing(path: &Path, folder: &Folder) -> String {
        let name = path
            .file_name()
            .map_or_else(|| "Contents".into(), |n| n.to_string_lossy().into_owned());
        let items: String = folder
            .folders
            .iter()
            .map(|f| {
                format!(
                    "<li><a href=\"{}/index.html\">{}/</a></li>\n",
                    Self::href(f),
                    Self::escape(f)
                )
            })
            .chain(folder.pages.iter().map(|(file, title)| {
                format!(
                    "<li><a href=\"{}\">{}</a></li>\n",
                    Self::href(file),
                    Self::escape(title)
                )
            }))
            .collect();
        let body = format!("<h1>{}</h1>\n<ul>\n{items}</ul>\n", Self::escape(&name));
        Self::document(&name, &body)
    }

    /// A file's name as a link to it: every character a URL holds as itself kept, and the rest as %XX.
    fn href(name: &str) -> String {
        name.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    char::from(b).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect()
    }

    /// Where a link from a page goes in the site: a link to a page, `arch/overview.md#data`, to its HTML,
    /// `arch/overview.html#data`; one to a folder, `arch/`, to its README's page, when it is shown by its
    /// README, or else to its `index.html`; one from the site's top,
    /// `/arch/overview.md`, made relative to the page, so that the site works from disk; and any other, to
    /// another site or to a file, as it is.
    fn moved(page: &Path, dest: &str, readmes: &BTreeMap<PathBuf, String>) -> Option<String> {
        let (path, rest) = dest.split_at(dest.find(['#', '?']).unwrap_or(dest.len()));
        let elsewhere = path.starts_with("//")
            || path
                .split('/')
                .next()
                .is_some_and(|first| first.contains(':'));
        let markdown = path.len() > 3
            && path
                .get(path.len() - 3..)
                .is_some_and(|end| end.eq_ignore_ascii_case(".md"));
        let folder = path.ends_with('/');
        if elsewhere || !(markdown || folder) {
            return None;
        }
        let html = if folder {
            let index = readmes
                .get(&Self::folder_of(page, path))
                .map_or("index.html", String::as_str);
            format!("{path}{index}")
        } else {
            format!("{}.html", &path[..path.len() - 3])
        };
        let html = match html.strip_prefix('/') {
            Some(from_top) => {
                let depth = page.components().count().saturating_sub(1);
                format!("{}{from_top}", "../".repeat(depth))
            }
            None => html,
        };
        Some(format!("{html}{rest}"))
    }

    /// The folder a link to a folder from a page names, from the site's top: `../algebra/` from
    /// `arch/dma.md` is `algebra`.
    fn folder_of(page: &Path, path: &str) -> PathBuf {
        match path.strip_prefix('/') {
            Some(from_top) => Self::tidy(Path::new(from_top)),
            None => Self::tidy(&page.parent().unwrap_or(Path::new("")).join(path)),
        }
    }

    /// The folders shown by their README, each with its README's page: those with a `README.md` and no
    /// `index.md`, whatever the case of their letters.
    fn readmes(files: &[PathBuf]) -> BTreeMap<PathBuf, String> {
        let folder = |p: &PathBuf| p.parent().unwrap_or(Path::new("")).to_path_buf();
        let indexed: BTreeSet<PathBuf> = files
            .iter()
            .filter(|p| Self::named(p, "index.md"))
            .map(folder)
            .collect();
        files
            .iter()
            .filter(|p| Self::named(p, "README.md") && !indexed.contains(&folder(p)))
            .map(|p| {
                (
                    folder(p),
                    p.with_extension("html")
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                )
            })
            .collect()
    }

    /// A relative path with its `.` and `..` worked out, as `arch/../docs/./a` is `docs/a`.
    fn tidy(path: &Path) -> PathBuf {
        let mut out = PathBuf::new();
        for part in path.components() {
            match part {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    out.pop();
                }
                part => out.push(part),
            }
        }
        out
    }

    /// Whether a file is named so, whatever the case of its letters.
    pub(crate) fn named(path: &Path, name: &str) -> bool {
        path.file_name()
            .is_some_and(|n| n.eq_ignore_ascii_case(name))
    }

    /// A page that sends whoever opens it to another in its folder: by the HTML itself, as a site on disk or
    /// on a host such as GitHub Pages cannot redirect, and naming that page for search.
    fn redirect(title: &str, to: &str) -> String {
        let to = Self::escape(&Self::href(to));
        format!(
            concat!(
                "<!doctype html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n<title>{title}</title>\n",
                "<link rel=\"canonical\" href=\"{to}\">\n",
                "<meta http-equiv=\"refresh\" content=\"0; url={to}\">\n</head>\n<body>\n",
                "<p>This page is at <a href=\"{to}\">{to}</a>.</p>\n</body>\n</html>\n"
            ),
            title = Self::escape(title),
            to = to
        )
    }

    /// Text as HTML, with the characters that would read as markup escaped.
    pub(crate) fn escape(text: &str) -> String {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// A folder for a test, made in a temporary one, with these files, which is removed after.
    struct Tree(tempfile::TempDir);

    impl Tree {
        fn with(files: &[(&str, &[u8])]) -> Self {
            let dir = tempfile::tempdir().unwrap();
            for (path, bytes) in files {
                let path = dir.path().join(path);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, bytes).unwrap();
            }
            Self(dir)
        }

        fn path(&self, rest: &str) -> PathBuf {
            self.0.path().join(rest)
        }

        fn read(&self, rest: &str) -> String {
            fs::read_to_string(self.path(rest)).unwrap()
        }

        /// Every file under a folder of it, hidden ones too, by path, sorted.
        fn files(&self, under: &str) -> Vec<String> {
            let root = self.path(under);
            let mut files: Vec<String> = WalkBuilder::new(&root)
                .standard_filters(false)
                .build()
                .flatten()
                .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
                .map(|e| {
                    e.path()
                        .strip_prefix(&root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned()
                })
                .collect();
            files.sort();
            files
        }
    }

    #[test]
    fn pages_are_written_as_html_beside_where_they_were() {
        let tree = Tree::with(&[
            ("docs/index.md", b"# Home\n\nHello."),
            ("docs/arch/overview.md", b"# Overview"),
        ]);
        let built = Builder::new()
            .build(&tree.path("docs"), &tree.path("site"))
            .unwrap();
        assert_eq!(
            built,
            Built {
                pages: 2,
                files: 0,
                folders: 1
            }
        );
        assert_eq!(
            tree.files("site"),
            [
                ".git-kb-site",
                "arch/index.html",
                "arch/overview.html",
                "index.html"
            ]
        );
        let home = tree.read("site/index.html");
        assert!(home.starts_with("<!doctype html>"), "{home}");
        assert!(home.contains("<title>Home</title>"), "{home}");
        assert!(home.contains(r#"<h1 id="home">Home</h1>"#), "{home}");
    }

    #[test]
    fn links_to_pages_go_to_their_html() {
        let page = Path::new("arch/overview.md");
        for (dest, moved) in [
            ("dma.md", Some("dma.html")),
            ("../index.md#top", Some("../index.html#top")),
            ("NOTES.MD?x=1#y", Some("NOTES.html?x=1#y")),
            ("/algebra/gemm.md", Some("../algebra/gemm.html")),
            ("https://example.org/a.md", None),
            ("//example.org/a.md", None),
            ("mailto:a@b.md", None),
            ("img/chip.png", None),
            ("arch/", Some("arch/index.html")),
            ("../", Some("../index.html")),
            ("#section", None),
            (".md", None),
            ("€a", None),
        ] {
            assert_eq!(
                Builder::moved(page, dest, &BTreeMap::new()).as_deref(),
                moved,
                "{dest}"
            );
        }
        assert_eq!(
            Builder::moved(Path::new("index.md"), "/a.md", &BTreeMap::new()).as_deref(),
            Some("a.html")
        );
    }

    #[test]
    fn built_pages_link_to_each_other() {
        let tree = Tree::with(&[
            (
                "docs/index.md",
                b"See [the overview](arch/overview.md#data) and ![chip](img/chip.png).",
            ),
            (
                "docs/arch/overview.md",
                b"Back [home](../index.md), or [from the top](/index.md).",
            ),
        ]);
        Builder::new()
            .build(&tree.path("docs"), &tree.path("site"))
            .unwrap();
        let home = tree.read("site/index.html");
        assert!(home.contains(r#"href="arch/overview.html#data""#), "{home}");
        assert!(home.contains(r#"src="img/chip.png""#), "{home}");
        let overview = tree.read("site/arch/overview.html");
        assert!(overview.contains(r#"href="../index.html""#), "{overview}");
        assert_eq!(
            overview.matches(r#"href="../index.html""#).count(),
            2,
            "{overview}"
        );
    }

    #[test]
    fn a_page_without_a_heading_is_titled_by_its_name() {
        let tree = Tree::with(&[("docs/notes.md", b"Just <text>.")]);
        Builder::new()
            .build(&tree.path("docs"), &tree.path("site"))
            .unwrap();
        assert!(
            tree.read("site/notes.html")
                .contains("<title>notes</title>")
        );
    }

    #[test]
    fn titles_are_escaped() {
        let tree = Tree::with(&[("docs/a.md", b"# A < B & \"c\"")]);
        Builder::new()
            .build(&tree.path("docs"), &tree.path("site"))
            .unwrap();
        assert!(
            tree.read("site/a.html")
                .contains("<title>A &lt; B &amp; &quot;c&quot;</title>")
        );
    }

    #[test]
    fn other_files_are_copied_as_they_are() {
        let image: &[u8] = &[0x89, b'P', b'N', b'G', 0, 1, 2, 255];
        let tree = Tree::with(&[("docs/img/chip.png", image), ("docs/a.md", b"# A")]);
        let built = Builder::new()
            .build(&tree.path("docs"), &tree.path("site"))
            .unwrap();
        assert_eq!(
            built,
            Built {
                pages: 1,
                files: 1,
                folders: 1
            }
        );
        assert_eq!(fs::read(tree.path("site/img/chip.png")).unwrap(), image);
    }

    #[test]
    fn what_git_ignores_is_left_out() {
        let tree = Tree::with(&[
            ("docs/.gitignore", b"target/\n*.log\n"),
            ("docs/a.md", b"# A"),
            ("docs/target/debug/huge.bin", b"no"),
            ("docs/build.log", b"no"),
        ]);
        Builder::new()
            .build(&tree.path("docs"), &tree.path("site"))
            .unwrap();
        assert_eq!(
            tree.files("site"),
            [".git-kb-site", ".gitignore", "a.html", "index.html"]
        );
    }

    #[test]
    fn what_kbignore_ignores_is_left_out() {
        let tree = Tree::with(&[
            ("docs/.kbignore", b"drafts/\n"),
            ("docs/arch/.kbignore", b"scratch.md\n"),
            ("docs/a.md", b"# A"),
            ("docs/drafts/b.md", b"# B"),
            ("docs/arch/scratch.md", b"# Scratch"),
            ("docs/arch/c.md", b"# C"),
        ]);
        Builder::new()
            .build(&tree.path("docs"), &tree.path("site"))
            .unwrap();
        assert_eq!(
            tree.files("site"),
            [
                ".git-kb-site",
                ".kbignore",
                "a.html",
                "arch/.kbignore",
                "arch/c.html",
                "arch/index.html",
                "index.html"
            ]
        );
    }

    #[test]
    fn of_what_is_hidden_only_git_s_store_is_left_out() {
        let tree = Tree::with(&[
            ("docs/a.md", b"# A"),
            ("docs/.notes.md", b"# Notes"),
            ("docs/.git/HEAD", b"no"),
        ]);
        Builder::new()
            .build(&tree.path("docs"), &tree.path("site"))
            .unwrap();
        assert_eq!(
            tree.files("site"),
            [".git-kb-site", ".notes.html", "a.html", "index.html"]
        );
    }

    #[test]
    fn an_output_folder_inside_the_folder_is_not_built_into_itself() {
        let tree = Tree::with(&[("docs/a.md", b"# A")]);
        let builder = Builder::new();
        builder
            .build(&tree.path("docs"), &tree.path("docs/site"))
            .unwrap();
        let built = builder
            .build(&tree.path("docs"), &tree.path("docs/site"))
            .unwrap();
        assert_eq!(
            built,
            Built {
                pages: 1,
                files: 0,
                folders: 1
            }
        );
        assert_eq!(
            tree.files("docs/site"),
            [".git-kb-site", "a.html", "index.html"]
        );
    }

    #[test]
    fn building_again_leaves_nothing_of_what_is_gone() {
        let tree = Tree::with(&[("docs/a.md", b"# A"), ("docs/b.md", b"# B")]);
        Builder::new()
            .build(&tree.path("docs"), &tree.path("site"))
            .unwrap();
        fs::remove_file(tree.path("docs/b.md")).unwrap();
        Builder::new()
            .build(&tree.path("docs"), &tree.path("site"))
            .unwrap();
        assert_eq!(tree.files("site"), [".git-kb-site", "a.html", "index.html"]);
    }

    #[test]
    fn a_folder_a_build_did_not_write_is_never_emptied() {
        let tree = Tree::with(&[("docs/a.md", b"# A"), ("site/mine.txt", b"keep me")]);
        let error = Builder::new()
            .build(&tree.path("docs"), &tree.path("site"))
            .unwrap_err();
        assert!(error.to_string().contains("did not write"), "{error}");
        assert_eq!(tree.read("site/mine.txt"), "keep me");
    }

    #[test]
    fn the_output_folder_may_not_hold_the_folder_built() {
        let tree = Tree::with(&[("docs/a.md", b"# A")]);
        let error = Builder::new()
            .build(&tree.path("docs"), &tree.path(""))
            .unwrap_err();
        assert!(error.to_string().contains("holds the folder"), "{error}");
        assert_eq!(tree.read("docs/a.md"), "# A");
    }

    #[test]
    fn a_folder_s_index_md_is_its_index() {
        let tree = Tree::with(&[
            ("docs/index.md", b"# Home"),
            ("docs/README.md", b"# Readme"),
        ]);
        let built = Builder::new()
            .build(&tree.path("docs"), &tree.path("site"))
            .unwrap();
        assert_eq!(built.folders, 0);
        assert!(tree.read("site/index.html").contains("<title>Home</title>"));
    }

    #[test]
    fn without_one_a_folder_s_index_sends_to_its_readme() {
        let tree = Tree::with(&[
            ("docs/arch/README.md", b"# Architecture"),
            (
                "docs/arch/dma.md",
                b"# DMA\n\n[Read me](README.md), [the folder](./), [from the top](/arch/).",
            ),
        ]);
        let built = Builder::new()
            .build(&tree.path("docs"), &tree.path("site"))
            .unwrap();
        assert_eq!(
            built,
            Built {
                pages: 2,
                files: 0,
                folders: 2
            }
        );
        assert!(
            tree.read("site/arch/README.html")
                .contains("<h1 id=\"architecture\">Architecture</h1>")
        );
        let index = tree.read("site/arch/index.html");
        for wanted in [
            "<title>Architecture</title>",
            r#"<link rel="canonical" href="README.html">"#,
            r#"<meta http-equiv="refresh" content="0; url=README.html">"#,
            r#"<a href="README.html">"#,
        ] {
            assert!(index.contains(wanted), "{wanted} missing from {index}");
        }
        let dma = tree.read("site/arch/dma.html");
        assert!(dma.contains(r#"href="README.html""#), "{dma}");
        assert!(dma.contains(r#"href="./README.html""#), "{dma}");
        assert!(dma.contains(r#"href="../arch/README.html""#), "{dma}");
        assert!(
            tree.read("site/index.html")
                .contains(r#"href="arch/index.html""#)
        );
    }

    #[test]
    fn with_an_index_md_a_readme_is_a_page_of_its_own() {
        let tree = Tree::with(&[
            ("docs/index.md", b"# Home\n\n[Read me](README.md)"),
            ("docs/README.md", b"# Readme"),
        ]);
        Builder::new()
            .build(&tree.path("docs"), &tree.path("site"))
            .unwrap();
        assert!(
            tree.read("site/README.html")
                .contains("<h1 id=\"readme\">Readme</h1>")
        );
        assert!(
            tree.read("site/index.html")
                .contains(r#"href="README.html""#)
        );
    }

    #[test]
    fn a_link_to_a_folder_shown_by_its_readme_goes_to_the_readme() {
        let readmes = BTreeMap::from([
            (PathBuf::from("arch"), "README.html".to_string()),
            (PathBuf::new(), "readme.html".to_string()),
        ]);
        let page = Path::new("arch/dma.md");
        for (dest, moved) in [
            ("./", "./README.html"),
            ("../", "../readme.html"),
            ("../algebra/", "../algebra/index.html"),
            ("/arch/", "../arch/README.html"),
            ("README.md#top", "README.html#top"),
        ] {
            assert_eq!(
                Builder::moved(page, dest, &readmes).as_deref(),
                Some(moved),
                "{dest}"
            );
        }
    }

    #[test]
    fn a_folder_without_pages_has_no_index() {
        let tree = Tree::with(&[("docs/a.md", b"# A"), ("docs/img/chip.png", b"png")]);
        Builder::new()
            .build(&tree.path("docs"), &tree.path("site"))
            .unwrap();
        assert!(!tree.path("site/img/index.html").exists());
        assert!(!tree.read("site/index.html").contains("img/"));
    }
}
