//! `git kb build`: a folder of Markdown written as a static site, a folder of files to open from disk or put on
//! any host. Each page is written as HTML beside where its Markdown is, `arch/overview.md` as
//! `arch/overview.html`, and every other file, such as an image, is copied as it is. What `.gitignore` and
//! `.kbignore` files ignore is left out: `.gitignore` for what git leaves out too, such as `target/`, and
//! `.kbignore`, in the same form, for what only the site does. So is `.git`, git's own store, and links,
//! which are neither followed nor copied.

use std::fs;
use std::path::Path;

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
        let mut built = Built { pages: 0, files: 0 };

        let skip = out.clone();
        let walk = WalkBuilder::new(&folder)
            .standard_filters(false)
            .git_ignore(true)
            .parents(true)
            .require_git(false)
            .add_custom_ignore_filename(Self::IGNORE)
            .filter_entry(move |entry| entry.path() != skip && entry.file_name() != ".git")
            .build();

        for entry in walk {
            let entry = entry?;
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let from = entry.path();
            let path = from.strip_prefix(&folder)?;
            let markdown = path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("md"));
            let to = out.join(if markdown {
                path.with_extension("html")
            } else {
                path.to_path_buf()
            });
            if let Some(dir) = to.parent() {
                fs::create_dir_all(dir).with_context(|| dir.display().to_string())?;
            }
            if markdown {
                let text = fs::read_to_string(from).with_context(|| from.display().to_string())?;
                fs::write(&to, self.page(&text, path)).with_context(|| to.display().to_string())?;
                debug!(page = %path.display(), "rendered");
                built.pages += 1;
            } else {
                fs::copy(from, &to).with_context(|| to.display().to_string())?;
                debug!(file = %path.display(), "copied");
                built.files += 1;
            }
        }
        fs::write(
            out.join(Self::MARK),
            "Written by git kb build, which replaces everything here when it builds again.\n",
        )?;
        Ok(built)
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

    /// A page's HTML document: the Markdown rendered, titled by its first heading, or else by its file's name.
    fn page(&self, markdown: &str, path: &Path) -> String {
        let page = self.renderer.render(markdown);
        let name = path.file_stem().unwrap_or_default().to_string_lossy();
        let title = Self::escape(page.title.as_deref().unwrap_or(&name));
        format!(
            "<!doctype html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n<title>{title}</title>\n</head>\n<body>\n{}</body>\n</html>\n",
            page.html
        )
    }

    /// Text as HTML, with the characters that would read as markup escaped.
    fn escape(text: &str) -> String {
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
        assert_eq!(built, Built { pages: 2, files: 0 });
        assert_eq!(
            tree.files("site"),
            [".git-kb-site", "arch/overview.html", "index.html"]
        );
        let home = tree.read("site/index.html");
        assert!(home.starts_with("<!doctype html>"), "{home}");
        assert!(home.contains("<title>Home</title>"), "{home}");
        assert!(home.contains(r#"<h1 id="home">Home</h1>"#), "{home}");
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
        assert_eq!(built, Built { pages: 1, files: 1 });
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
        assert_eq!(tree.files("site"), [".git-kb-site", ".gitignore", "a.html"]);
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
                "arch/c.html"
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
            [".git-kb-site", ".notes.html", "a.html"]
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
        assert_eq!(built, Built { pages: 1, files: 0 });
        assert_eq!(tree.files("docs/site"), [".git-kb-site", "a.html"]);
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
        assert_eq!(tree.files("site"), [".git-kb-site", "a.html"]);
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
}
