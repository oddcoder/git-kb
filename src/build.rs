//! `git kb build`: a folder of Markdown written as a static site, a folder of files to open from disk or put on
//! any host. The site is in its `docs/`, as the folder is, and, when the folder is in a git repository, in
//! `ref/main/`, `ref/v1.0/` and so on, as it is at each branch and tag, each page saying when it last changed,
//! and `ref/index.html` lists them all, and the commits that changed a page; git-kb's own files, the pages'
//! style sheet and scripts, are beside them, in `_assets/`, and the top's `index.html` sends to `docs/`.
//!
//! Each page is written as HTML beside where its Markdown is, `arch/overview.md` as `arch/overview.html`, with its links to other pages made links to theirs, and every other file, such as an
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
use std::sync::Arc;

use anyhow::{Context, bail};
use ignore::WalkBuilder;
use tracing::debug;

use crate::config::LookConfig;
use crate::diff::Diff;
use crate::git::{History, Repo, Snapshot};
use crate::nav::{Nav, Pages};
use crate::render::{Moved, Renderer};
use crate::shell::{self, Comparison, Links, Margin, PageHistory, PageState, Shell, Version};

/// What a build wrote.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Built {
    /// The branches and tags built, besides the folder as it is.
    pub refs: usize,
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

/// What a site's pages know of their history: the commits up to the one built, and, for the folder as it is,
/// the last commit, to tell which pages have changed since.
struct Past<'a> {
    repo: &'a Repo,
    /// The commit built: the last, for the folder as it is.
    at: Arc<Snapshot>,
    history: Arc<History>,
    head: Option<Arc<Snapshot>>,
}

/// What each page of one site is built within: its shell, the site's pages, the folders shown by their README,
/// and the site's history, when there is one.
struct Within<'a> {
    shell: &'a Shell,
    pages: &'a Pages,
    readmes: &'a BTreeMap<PathBuf, String>,
    past: Option<&'a Past<'a>>,
}

/// A page being built, for what is written of it besides itself.
struct Page<'a> {
    shell: &'a Shell,
    pages: &'a Pages,
    path: &'a Path,
    readmes: &'a BTreeMap<PathBuf, String>,
    title: &'a str,
}

/// Builds folders of Markdown as sites.
#[derive(Debug, Default)]
pub struct Builder {
    renderer: Renderer,
    /// The site's name; its folder's when none is given.
    title: Option<String>,
    /// Where the site lives, ending in `/`, for its pages' canonical addresses.
    url: Option<String>,
    look: LookConfig,
}

impl Builder {
    /// A file in the output folder that says a build wrote it, so that building again may empty it, and
    /// nothing else ever is.
    const MARK: &str = ".git-kb-site";

    /// The files, in `.gitignore`'s form, that say what the site leaves out and git does not.
    pub const IGNORE: &str = ".kbignore";

    /// A builder of sites named `title`, or else by their folders, and living at `url`, if it is given.
    #[must_use]
    pub fn new(title: Option<String>, url: Option<String>) -> Self {
        Self {
            renderer: Renderer::new(),
            title,
            url,
            look: LookConfig::default(),
        }
    }

    /// The same, building sites in a look of their own.
    #[must_use]
    pub fn looking(self, look: LookConfig) -> Self {
        Self { look, ..self }
    }

    /// Writes a folder of Markdown as a site, into `out`, in place of whatever an earlier build wrote there: the
    /// folder as it is, and as it is at each branch and tag of its repository. An output folder inside the folder
    /// built is not built itself.
    ///
    /// # Errors
    ///
    /// When `out` holds files no build wrote, or holds the folder built, or a file cannot be read or written.
    pub fn build(&self, folder: &Path, out: &Path) -> anyhow::Result<Built> {
        let folder = fs::canonicalize(folder).with_context(|| folder.display().to_string())?;
        Self::prepare(out, &folder)?;
        let out = fs::canonicalize(out).with_context(|| out.display().to_string())?;
        // A folder in a repository without commits yet has no history.
        let repo = Repo::discover(&folder);
        let head = repo.as_ref().and_then(|repo| repo.snapshot("HEAD").ok());
        let shell = Shell::new(self.title(&folder), Links::Built)
            .living_at(self.url.clone())
            .looking(self.look.clone())
            .in_git(head.is_some());

        let past = match (&repo, &head) {
            (Some(repo), Some(head)) => Some(Past {
                repo,
                at: Arc::clone(head),
                history: repo.history(head)?,
                head: Some(Arc::clone(head)),
            }),
            _ => None,
        };
        let files = Self::files(&folder, Some(&out))?;
        let read = |f: &Path| {
            let from = folder.join(f);
            fs::read(&from).with_context(|| from.display().to_string())
        };
        let mut built = self.site(&shell, &files, &out.join("docs"), read, past.as_ref())?;
        if let (Some(repo), Some(past)) = (&repo, &past) {
            let refs = repo.refs()?;
            for snapshot in &refs {
                let shell = shell.clone().showing(Some(snapshot.revision.clone()));
                let to = out.join("ref").join(&snapshot.revision.name);
                let of = Past {
                    repo,
                    at: Arc::clone(snapshot),
                    history: repo.history(snapshot)?,
                    head: None,
                };
                let read = |f: &Path| repo.read(snapshot, f);
                let of = self.site(&shell, &snapshot.files(), &to, read, Some(&of))?;
                debug!(
                    name = snapshot.revision.name,
                    pages = of.pages,
                    "built a ref"
                );
                built.refs += 1;
            }
            let revisions: Vec<_> = refs.iter().map(|s| s.revision.clone()).collect();
            let listing = shell.clone().listing_revisions();
            let body = listing.revisions(&revisions, &past.history);
            let index = out.join("ref").join("index.html");
            fs::create_dir_all(out.join("ref"))?;
            fs::write(
                &index,
                listing.page(Path::new(""), "", "Revisions", &body, &Nav::none()),
            )
            .with_context(|| index.display().to_string())?;
        }

        for (path, text) in shell::ASSETS {
            let to = out.join(path);
            fs::create_dir_all(to.parent().unwrap_or(&out))?;
            fs::write(&to, text).with_context(|| to.display().to_string())?;
        }
        let index = out.join("index.html");
        let docs = shell
            .address("")
            .unwrap_or_else(|| "docs/index.html".into());
        fs::write(
            &index,
            Self::redirect(shell.name(), "docs/index.html", Some(&docs)),
        )
        .with_context(|| index.display().to_string())?;
        fs::write(
            out.join(Self::MARK),
            "Written by git kb build, which replaces everything here when it builds again.\n",
        )?;
        Ok(built)
    }

    /// Writes one site, of the files given, each read by `read`, into `to`, its pages with their history.
    fn site(
        &self,
        shell: &Shell,
        files: &[PathBuf],
        to: &Path,
        read: impl Fn(&Path) -> anyhow::Result<Vec<u8>>,
        past: Option<&Past>,
    ) -> anyhow::Result<Built> {
        let mut built = Built::default();
        let mut folders: BTreeMap<PathBuf, Folder> = BTreeMap::new();
        // Every file first, so that each page knows, as it is rendered, which folders are shown by their
        // README: those with one and without an index.md, each with its README's page.
        let readmes = Self::readmes(files);
        let pages = Pages::new(files, &self.renderer, |f| {
            read(f)
                .ok()
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        });
        for path in files {
            let markdown = path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("md"));
            let written = if markdown {
                path.with_extension("html")
            } else {
                path.clone()
            };
            let into = to.join(&written);
            if let Some(dir) = into.parent() {
                fs::create_dir_all(dir).with_context(|| dir.display().to_string())?;
            }
            let bytes = read(path)?;
            if markdown {
                let text = String::from_utf8_lossy(&bytes);
                let history = PageHistory::of(
                    past.and_then(|p| p.head.as_deref()),
                    past.map(|p| &*p.history),
                    path,
                    &bytes,
                );
                let within = Within {
                    shell,
                    pages: &pages,
                    readmes: &readmes,
                    past,
                };
                let (html, title) = self.page(&within, &text, path, &history);
                fs::write(&into, html).with_context(|| into.display().to_string())?;
                if let Some(past) = past {
                    let page = Page {
                        shell,
                        pages: &pages,
                        path,
                        readmes: &readmes,
                        title: &title,
                    };
                    self.comparisons(&page, past, &history, &text, to)?;
                }
                debug!(page = %path.display(), "rendered");
                built.pages += 1;
                Self::file_under(&mut folders, &written, title);
            } else {
                fs::write(&into, bytes).with_context(|| into.display().to_string())?;
                debug!(file = %path.display(), "copied");
                built.files += 1;
            }
        }
        for (path, folder) in &folders {
            if Self::index(shell, &pages, to, path, folder)? {
                built.folders += 1;
            }
        }
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
        within: &Within,
        markdown: &str,
        path: &Path,
        history: &PageHistory,
    ) -> (String, String) {
        let (shell, pages) = (within.shell, within.pages);
        let moved = |dest: &str| Self::moved(path, dest, within.readmes);
        let page = match within.past {
            Some(past) => self.marked(shell, markdown, path, &moved, past),
            None => self.renderer.render_with(markdown, &moved),
        };
        let name = path.file_stem().unwrap_or_default().to_string_lossy();
        let title = page.title.unwrap_or_else(|| name.into_owned());
        let at = path.parent().unwrap_or(Path::new(""));
        let file = path.with_extension("html");
        let file = file.file_name().unwrap_or_default().to_string_lossy();
        let nav = Nav {
            pages,
            current: path,
            headings: &page.headings,
        };
        let body = shell.noted(at, path, &page.html, history);
        (shell.page(at, &file, &title, &body, &nav), title)
    }

    /// A page rendered with each block marked in its margin with when it last changed.
    fn marked(
        &self,
        shell: &Shell,
        markdown: &str,
        path: &Path,
        moved: Moved,
        past: &Past,
    ) -> crate::render::Page {
        // The folder as it is is blamed as it is on disk, its lines not committed yet as such.
        let now = past.head.is_some().then_some(markdown.as_bytes());
        let Ok(blame) = past.repo.blame(&past.at, path, now) else {
            return self.renderer.render_with(markdown, moved);
        };
        let margin = Margin::new(
            shell,
            path,
            markdown,
            &blame,
            past.repo,
            &self.renderer,
            moved,
        );
        self.renderer
            .render_marked(markdown, moved, &|range| margin.mark(range))
    }

    /// A page's comparisons, beside it: what each commit that changed it changed, and, in the folder as it is,
    /// how it differs from the last commit.
    fn comparisons(
        &self,
        page: &Page,
        past: &Past,
        history: &PageHistory,
        now: &str,
        to: &Path,
    ) -> anyhow::Result<()> {
        let text = |bytes: Vec<u8>| String::from_utf8_lossy(&bytes).into_owned();
        // The page at a commit, or nothing where it is not, as before the first commit.
        let at = |rev: &str| -> String {
            past.repo
                .snapshot(rev)
                .and_then(|at| past.repo.read(&at, page.path))
                .map(text)
                .unwrap_or_default()
        };
        let here = Version {
            label: "the working copy".into(),
            href: Some(page.shell.link(
                page.path.parent().unwrap_or(Path::new("")),
                &page.path.with_extension("html"),
            )),
        };
        for change in &history.changes {
            let id = change.id.to_string();
            let (old, new) = (at(&format!("{id}^")), at(&id));
            let from = Version {
                label: format!("before {}", change.short),
                href: None,
            };
            let then = Version {
                label: change.short.clone(),
                href: None,
            };
            self.comparison(page, &change.short, &old, &new, from, then, to)?;
        }
        if past.head.is_some() && history.state != PageState::Committed {
            let from = Version {
                label: "the last commit".into(),
                href: None,
            };
            let old = at("HEAD");
            self.comparison(page, "uncommitted", &old, now, from, here, to)?;
        }
        Ok(())
    }

    /// One comparison of a page, written beside it as `page~with.html`.
    #[expect(clippy::too_many_arguments)]
    fn comparison(
        &self,
        page: &Page,
        with: &str,
        old: &str,
        new: &str,
        from: Version,
        then: Version,
        to: &Path,
    ) -> anyhow::Result<()> {
        let moved = |dest: &str| Self::moved(page.path, dest, page.readmes);
        let diff = Diff::pages(old, new, &self.renderer, &moved);
        let comparison = Comparison {
            title: page.title,
            from,
            to: then,
            diff: &diff,
            before: self.renderer.render_with(old, &moved).html,
            after: self.renderer.render_with(new, &moved).html,
            lines: Diff::lines(old, new),
        };
        let at = page.path.parent().unwrap_or(Path::new(""));
        let written = Shell::compared(page.path, with);
        let file = written.file_name().unwrap_or_default().to_string_lossy();
        let nav = Nav {
            pages: page.pages,
            current: page.path,
            headings: &[],
        };
        let title = format!("Changes to {}", page.title);
        let html = page
            .shell
            .page(at, &file, &title, &page.shell.compare(&comparison), &nav);
        let into = to.join(&written);
        fs::write(&into, html).with_context(|| into.display().to_string())
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
    fn index(
        shell: &Shell,
        pages: &Pages,
        out: &Path,
        path: &Path,
        folder: &Folder,
    ) -> anyhow::Result<bool> {
        let named = |name: &str| folder.pages.keys().find(|p| p.eq_ignore_ascii_case(name));
        if named("index.html").is_some() {
            return Ok(false);
        }
        let dir = out.join(path);
        let index = dir.join("index.html");
        if let Some(readme) = named("README.html") {
            let canonical = shell.address(&Self::page_path(path, readme));
            fs::write(
                &index,
                Self::redirect(
                    &folder.pages[readme],
                    &Self::href(readme),
                    canonical.as_deref(),
                ),
            )
            .with_context(|| index.display().to_string())?;
            debug!(folder = %path.display(), "redirected to its README");
            return Ok(true);
        }
        fs::write(&index, Self::listing(shell, pages, path, folder))
            .with_context(|| index.display().to_string())?;
        debug!(folder = %path.display(), "indexed");
        Ok(true)
    }

    /// A folder's list: its folders, each a link to its `index.html`, and then its pages, by title.
    pub(crate) fn listing(shell: &Shell, pages: &Pages, path: &Path, folder: &Folder) -> String {
        let name = path
            .file_name()
            .map_or_else(|| "Contents".into(), |n| n.to_string_lossy().into_owned());
        let items: String = folder
            .folders
            .iter()
            .map(|f| {
                format!(
                    "<li class=\"folder\"><a href=\"{}/index.html\">{}/</a></li>\n",
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
        let body = format!(
            "<h1>{}</h1>\n<ul class=\"listing\">\n{items}</ul>\n",
            Self::escape(&name)
        );
        let nav = Nav {
            pages,
            current: path,
            headings: &[],
        };
        shell.page(path, "", &name, &body, &nav)
    }

    /// A page's path in the site, as a link from its top: `arch/README.html`.
    fn page_path(folder: &Path, file: &str) -> String {
        folder
            .components()
            .map(|c| Self::href(&c.as_os_str().to_string_lossy()))
            .chain(std::iter::once(Self::href(file)))
            .collect::<Vec<_>>()
            .join("/")
    }

    /// A site's name: its title, or else its folder's.
    pub(crate) fn title(&self, folder: &Path) -> String {
        self.title.clone().unwrap_or_else(|| Self::name(folder))
    }

    /// A folder's name, for a site without a title.
    pub(crate) fn name(folder: &Path) -> String {
        folder
            .file_name()
            .map_or_else(|| "/".into(), |n| n.to_string_lossy().into_owned())
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

    /// A page that sends whoever opens it to another, by a link to it: by the HTML itself, as a site on disk or
    /// on a host such as GitHub Pages cannot redirect, and naming that page for search.
    fn redirect(title: &str, to: &str, canonical: Option<&str>) -> String {
        let to = Self::escape(to);
        let canonical = canonical.map_or_else(|| to.clone(), Self::escape);
        format!(
            concat!(
                "<!doctype html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n<title>{title}</title>\n",
                "<link rel=\"canonical\" href=\"{canonical}\">\n",
                "<meta http-equiv=\"refresh\" content=\"0; url={to}\">\n</head>\n<body>\n",
                "<p>This page is at <a href=\"{to}\">{to}</a>.</p>\n</body>\n</html>\n"
            ),
            title = Self::escape(title),
            canonical = canonical,
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
                // The style sheet is in every site; a test of its own sees it.
                .filter(|e| !e.path().starts_with(root.join("_assets")))
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
        let built = Builder::default()
            .build(&tree.path("docs"), &tree.path("site"))
            .unwrap();
        assert_eq!(
            built,
            Built {
                refs: 0,
                pages: 2,
                files: 0,
                folders: 1
            }
        );
        assert_eq!(
            tree.files("site/docs"),
            ["arch/index.html", "arch/overview.html", "index.html"]
        );
        let home = tree.read("site/docs/index.html");
        assert!(home.starts_with("<!doctype html>"), "{home}");
        assert!(home.contains("<title>Home · docs</title>"), "{home}");
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
        Builder::default()
            .build(&tree.path("docs"), &tree.path("site"))
            .unwrap();
        let home = tree.read("site/docs/index.html");
        assert!(home.contains(r#"href="arch/overview.html#data""#), "{home}");
        assert!(home.contains(r#"src="img/chip.png""#), "{home}");
        let overview = tree.read("site/docs/arch/overview.html");
        for link in [
            r#"<a href="../index.html">home</a>"#,
            r#"<a href="../index.html">from the top</a>"#,
        ] {
            assert!(overview.contains(link), "{link} missing from {overview}");
        }
    }

    #[test]
    fn a_page_without_a_heading_is_titled_by_its_name() {
        let tree = Tree::with(&[("docs/notes.md", b"Just <text>.")]);
        Builder::default()
            .build(&tree.path("docs"), &tree.path("site"))
            .unwrap();
        assert!(
            tree.read("site/docs/notes.html")
                .contains("<title>notes · docs</title>")
        );
    }

    #[test]
    fn titles_are_escaped() {
        let tree = Tree::with(&[("docs/a.md", b"# A < B & \"c\"")]);
        Builder::default()
            .build(&tree.path("docs"), &tree.path("site"))
            .unwrap();
        assert!(
            tree.read("site/docs/a.html")
                .contains("<title>A &lt; B &amp; &quot;c&quot; · docs</title>")
        );
    }

    #[test]
    fn other_files_are_copied_as_they_are() {
        let image: &[u8] = &[0x89, b'P', b'N', b'G', 0, 1, 2, 255];
        let tree = Tree::with(&[("docs/img/chip.png", image), ("docs/a.md", b"# A")]);
        let built = Builder::default()
            .build(&tree.path("docs"), &tree.path("site"))
            .unwrap();
        assert_eq!(
            built,
            Built {
                refs: 0,
                pages: 1,
                files: 1,
                folders: 1
            }
        );
        assert_eq!(
            fs::read(tree.path("site/docs/img/chip.png")).unwrap(),
            image
        );
    }

    #[test]
    fn what_git_ignores_is_left_out() {
        let tree = Tree::with(&[
            ("docs/.gitignore", b"target/\n*.log\n"),
            ("docs/a.md", b"# A"),
            ("docs/target/debug/huge.bin", b"no"),
            ("docs/build.log", b"no"),
        ]);
        Builder::default()
            .build(&tree.path("docs"), &tree.path("site"))
            .unwrap();
        assert_eq!(
            tree.files("site/docs"),
            [".gitignore", "a.html", "index.html"]
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
        Builder::default()
            .build(&tree.path("docs"), &tree.path("site"))
            .unwrap();
        assert_eq!(
            tree.files("site/docs"),
            [
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
        Builder::default()
            .build(&tree.path("docs"), &tree.path("site"))
            .unwrap();
        assert_eq!(
            tree.files("site/docs"),
            [".notes.html", "a.html", "index.html"]
        );
    }

    #[test]
    fn an_output_folder_inside_the_folder_is_not_built_into_itself() {
        let tree = Tree::with(&[("docs/a.md", b"# A")]);
        let builder = Builder::default();
        builder
            .build(&tree.path("docs"), &tree.path("docs/site"))
            .unwrap();
        let built = builder
            .build(&tree.path("docs"), &tree.path("docs/site"))
            .unwrap();
        assert_eq!(
            built,
            Built {
                refs: 0,
                pages: 1,
                files: 0,
                folders: 1
            }
        );
        assert_eq!(tree.files("docs/site/docs"), ["a.html", "index.html"]);
    }

    #[test]
    fn building_again_leaves_nothing_of_what_is_gone() {
        let tree = Tree::with(&[("docs/a.md", b"# A"), ("docs/b.md", b"# B")]);
        Builder::default()
            .build(&tree.path("docs"), &tree.path("site"))
            .unwrap();
        fs::remove_file(tree.path("docs/b.md")).unwrap();
        Builder::default()
            .build(&tree.path("docs"), &tree.path("site"))
            .unwrap();
        assert_eq!(
            tree.files("site"),
            [
                ".git-kb-site",
                "docs/a.html",
                "docs/index.html",
                "index.html"
            ]
        );
    }

    #[test]
    fn a_folder_a_build_did_not_write_is_never_emptied() {
        let tree = Tree::with(&[("docs/a.md", b"# A"), ("site/mine.txt", b"keep me")]);
        let error = Builder::default()
            .build(&tree.path("docs"), &tree.path("site"))
            .unwrap_err();
        assert!(error.to_string().contains("did not write"), "{error}");
        assert_eq!(tree.read("site/mine.txt"), "keep me");
    }

    #[test]
    fn the_output_folder_may_not_hold_the_folder_built() {
        let tree = Tree::with(&[("docs/a.md", b"# A")]);
        let error = Builder::default()
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
        let built = Builder::default()
            .build(&tree.path("docs"), &tree.path("site"))
            .unwrap();
        assert_eq!(built.folders, 0);
        assert!(
            tree.read("site/docs/index.html")
                .contains("<title>Home · docs</title>")
        );
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
        let built = Builder::default()
            .build(&tree.path("docs"), &tree.path("site"))
            .unwrap();
        assert_eq!(
            built,
            Built {
                refs: 0,
                pages: 2,
                files: 0,
                folders: 2
            }
        );
        assert!(
            tree.read("site/docs/arch/README.html")
                .contains("<h1 id=\"architecture\">Architecture</h1>")
        );
        let index = tree.read("site/docs/arch/index.html");
        for wanted in [
            "<title>Architecture</title>",
            r#"<link rel="canonical" href="README.html">"#,
            r#"<meta http-equiv="refresh" content="0; url=README.html">"#,
            r#"<a href="README.html">"#,
        ] {
            assert!(index.contains(wanted), "{wanted} missing from {index}");
        }
        let dma = tree.read("site/docs/arch/dma.html");
        assert!(dma.contains(r#"href="README.html""#), "{dma}");
        assert!(dma.contains(r#"href="./README.html""#), "{dma}");
        assert!(dma.contains(r#"href="../arch/README.html""#), "{dma}");
        assert!(
            tree.read("site/docs/index.html")
                .contains(r#"href="arch/index.html""#)
        );
    }

    #[test]
    fn with_an_index_md_a_readme_is_a_page_of_its_own() {
        let tree = Tree::with(&[
            ("docs/index.md", b"# Home\n\n[Read me](README.md)"),
            ("docs/README.md", b"# Readme"),
        ]);
        Builder::default()
            .build(&tree.path("docs"), &tree.path("site"))
            .unwrap();
        assert!(
            tree.read("site/docs/README.html")
                .contains("<h1 id=\"readme\">Readme</h1>")
        );
        assert!(
            tree.read("site/docs/index.html")
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
        Builder::default()
            .build(&tree.path("docs"), &tree.path("site"))
            .unwrap();
        assert!(!tree.path("site/docs/img/index.html").exists());
        assert!(!tree.read("site/docs/index.html").contains("img/"));
    }

    #[test]
    fn a_built_site_has_its_style_sheet_and_script() {
        let tree = Tree::with(&[("docs/arch/a.md", b"# A")]);
        Builder::default()
            .build(&tree.path("docs"), &tree.path("site"))
            .unwrap();
        assert_eq!(tree.read("site/_assets/style.css"), shell::STYLE);
        assert_eq!(tree.read("site/_assets/look.js"), shell::LOOK);
        assert!(
            tree.read("site/docs/arch/a.html")
                .contains(r#"href="../../_assets/style.css""#)
        );
    }

    #[test]
    fn built_pages_have_the_site_s_pages_beside_them() {
        let tree = Tree::with(&[
            ("docs/index.md", b"# Home"),
            ("docs/arch/dma.md", b"# DMA\n## Queues\n## Channels"),
        ]);
        Builder::default()
            .build(&tree.path("docs"), &tree.path("site"))
            .unwrap();
        let dma = tree.read("site/docs/arch/dma.html");
        assert!(
            dma.contains(r#"<a href="../arch/dma.html" aria-current="page">DMA</a>"#),
            "{dma}"
        );
        assert!(dma.contains(r##"<a href="#queues">Queues</a>"##), "{dma}");
        let home = tree.read("site/docs/index.html");
        assert!(
            home.contains(r#"<a href="./arch/index.html">arch</a>"#),
            "{home}"
        );
    }

    #[test]
    fn each_branch_and_tag_is_built_beside_the_folder_as_it_is() {
        let git = crate::git::tests::Git::new();
        git.write("docs/a.md", "# A, first");
        git.commit("first");
        git.run(&["tag", "v1"]);
        git.run(&["branch", "topic/x"]);
        git.write("docs/a.md", "# A, second");
        git.commit("second");
        git.write("docs/a.md", "# A, not committed");
        let site = git.top().join("site");
        let built = Builder::default()
            .build(&git.top().join("docs"), &site)
            .unwrap();
        assert_eq!(built.refs, 3);
        let read = |path: &str| fs::read_to_string(site.join(path)).unwrap();
        assert!(read("docs/a.html").contains("<title>A, not committed"));
        assert!(read("ref/main/a.html").contains("<title>A, second"));
        assert!(read("ref/v1/a.html").contains("<title>A, first"));
        let topic = read("ref/topic/x/a.html");
        assert!(topic.contains("<title>A, first"), "{topic}");
        assert!(
            topic.contains(r#"href="../../../_assets/style.css""#),
            "{topic}"
        );
        assert!(
            topic.contains(r#"<a class="revision" href="../../../docs/index.html""#),
            "{topic}"
        );
        assert!(read("index.html").contains("url=docs/index.html"));
        let main = read("ref/main/a.html");
        assert!(
            main.contains("<p class=\"meta\">Last changed on ") && main.contains(" in <code"),
            "a built page names its last commit, plainly: {main}"
        );
        assert!(
            read("docs/a.html").contains("Changed since the last commit"),
            "{}",
            read("docs/a.html")
        );
        let revisions = read("ref/index.html");
        assert!(
            revisions.contains(r#"<a href="main/index.html">main</a>"#),
            "{revisions}"
        );
        assert!(
            revisions.contains(r#"<a href="topic/x/index.html">topic/x</a>"#),
            "{revisions}"
        );
        assert!(revisions.contains("<code>"), "{revisions}");
        assert!(
            revisions.contains(r#"href="../_assets/style.css""#),
            "{revisions}"
        );
    }

    #[test]
    fn each_page_s_changes_are_built_beside_it() {
        let git = crate::git::tests::Git::new();
        git.write("docs/a.md", "# A\n\nThe quick brown fox.\n");
        git.commit("write a");
        git.write("docs/a.md", "# A\n\nThe quick red fox.\n");
        git.commit("edit a");
        git.write("docs/a.md", "# A\n\nThe quick red fox.\n\nNot committed.\n");
        let site = git.top().join("site");
        Builder::default()
            .build(&git.top().join("docs"), &site)
            .unwrap();
        let read = |path: &Path| fs::read_to_string(site.join(path)).unwrap();
        let page = read(Path::new("docs/a.html"));
        assert!(
            page.contains(r#"<a href="./a~uncommitted.html">Changed since the last commit</a>"#),
            "{page}"
        );
        let uncommitted = read(Path::new("docs/a~uncommitted.html"));
        assert!(
            uncommitted.contains("<p>Not committed.</p>"),
            "{uncommitted}"
        );
        let commits: Vec<String> = fs::read_dir(site.join("ref/main"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("a~"))
            .collect();
        assert_eq!(
            commits.len(),
            2,
            "one for each commit that changed it: {commits:?}"
        );
        let edit = commits
            .iter()
            .map(|n| read(&Path::new("ref/main").join(n)))
            .find(|html| html.contains("<del>brown</del><ins>red</ins>"));
        assert!(edit.is_some(), "the edit's changes are marked");
        assert!(
            read(Path::new("ref/main/a.html")).contains(r#"(<a href="./a~"#),
            "a page links what its last commit changed"
        );
        let margin = read(Path::new("ref/main/a.html"));
        assert!(margin.contains(r#"<button class="blame""#), "{margin}");
        assert!(
            margin.contains(r#"<a href="./a~"#)
                && margin.contains("Everything this commit changed"),
            "{margin}"
        );
        assert!(read(Path::new("docs/a.html")).contains(r#"<div class="blk new">"#));
    }

    #[test]
    fn the_site_s_title_names_it() {
        let tree = Tree::with(&[("docs/a.md", b"# A")]);
        Builder::new(Some("pentasil".into()), None)
            .build(&tree.path("docs"), &tree.path("site"))
            .unwrap();
        let page = tree.read("site/docs/a.html");
        assert!(page.contains("<title>A · pentasil</title>"), "{page}");
        assert!(
            page.contains(r#"<a class="site" href="./index.html">pentasil</a>"#),
            "{page}"
        );
    }

    #[test]
    fn where_the_site_lives_names_each_page_s_address() {
        let tree = Tree::with(&[
            ("docs/arch/README.md", b"# Architecture"),
            ("docs/arch/dma.md", b"# DMA"),
        ]);
        Builder::new(None, Some("https://foobar.xyz/kb/".into()))
            .build(&tree.path("docs"), &tree.path("site"))
            .unwrap();
        let dma = tree.read("site/docs/arch/dma.html");
        assert!(
            dma.contains(
                r#"<link rel="canonical" href="https://foobar.xyz/kb/docs/arch/dma.html">"#
            ),
            "{dma}"
        );
        let stub = tree.read("site/docs/arch/index.html");
        assert!(
            stub.contains(
                r#"<link rel="canonical" href="https://foobar.xyz/kb/docs/arch/README.html">"#
            ),
            "{stub}"
        );
        assert!(stub.contains(r#"content="0; url=README.html""#), "{stub}");
    }
}
