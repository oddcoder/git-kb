//! The sidebar beside every page: its headings, to go to a part of it, and the tree of the site's folders and
//! pages, to go to any other, with the one shown marked. The shell writes it; this is what it is written from.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use crate::build::Builder;
use crate::render::{Heading, Renderer};

/// Every page of a site, by its path from the site's top, with its title.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Pages {
    titles: BTreeMap<PathBuf, String>,
}

/// What a page's sidebar shows: the site's pages, which one is shown, and the page's headings.
#[derive(Debug, Clone, Copy)]
pub struct Nav<'a> {
    pub pages: &'a Pages,
    /// The page shown, as `arch/overview.md`, or the folder, as `arch`, when it is a folder's list.
    pub current: &'a Path,
    pub headings: &'a [Heading],
}

impl Nav<'_> {
    /// A sidebar of nothing, for a page that is none of the site's, as an error's when its pages are not known.
    #[must_use]
    pub fn none() -> Nav<'static> {
        static NONE: Pages = Pages {
            titles: BTreeMap::new(),
        };
        Nav {
            pages: &NONE,
            current: Path::new(""),
            headings: &[],
        }
    }
}

impl Pages {
    /// The pages among a site's files, as `Builder::files` gives them, each read by `read` and titled by its
    /// first heading, or else by its name. A page that cannot be read is titled by its name.
    #[must_use]
    pub fn new(
        files: &[PathBuf],
        renderer: &Renderer,
        read: impl Fn(&Path) -> Option<String>,
    ) -> Self {
        let titles = files
            .iter()
            .filter(|f| f.extension().is_some_and(|e| e.eq_ignore_ascii_case("md")))
            .map(|f| {
                let title = read(f)
                    .and_then(|text| renderer.title(&text))
                    .unwrap_or_else(|| {
                        f.file_stem()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned()
                    });
                (f.clone(), title)
            })
            .collect();
        Self { titles }
    }

    /// A folder's folders that have pages, by name, and its own pages, by path, with their titles, but for the
    /// page the folder is shown by, as it is the folder's.
    #[must_use]
    pub fn entries(&self, folder: &Path) -> (BTreeSet<String>, Vec<(&Path, &str)>) {
        let home = self.home(folder);
        let mut folders = BTreeSet::new();
        let mut pages = vec![];
        for (path, title) in &self.titles {
            let Ok(below) = path.strip_prefix(folder) else {
                continue;
            };
            let mut parts = below.components();
            match (parts.next(), parts.next()) {
                (Some(Component::Normal(sub)), Some(_)) => {
                    folders.insert(sub.to_string_lossy().into_owned());
                }
                (Some(_), None) if Some(path.as_path()) != home => {
                    pages.push((path.as_path(), title.as_str()));
                }
                _ => {}
            }
        }
        pages.sort_by_key(|(_, title)| title.to_lowercase());
        (folders, pages)
    }

    /// The page a folder is shown by: its `index.md`, or else its `README.md`, as a build and the server show it.
    #[must_use]
    pub fn home(&self, folder: &Path) -> Option<&Path> {
        ["index.md", "README.md"].iter().find_map(|name| {
            self.titles
                .keys()
                .find(|p| p.parent() == Some(folder) && Builder::named(p, name))
                .map(PathBuf::as_path)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pages(titles: &[(&str, &str)]) -> Pages {
        Pages {
            titles: titles
                .iter()
                .map(|(path, title)| (PathBuf::from(path), (*title).to_string()))
                .collect(),
        }
    }

    #[test]
    fn a_folder_has_its_folders_and_its_pages_by_title_but_its_own() {
        let pages = pages(&[
            ("README.md", "Home"),
            ("b.md", "Alpha"),
            ("a.md", "beta"),
            ("arch/overview.md", "Overview"),
            ("arch/deep/x.md", "X"),
        ]);
        let (folders, here) = pages.entries(Path::new(""));
        assert_eq!(folders, BTreeSet::from(["arch".to_string()]));
        assert_eq!(
            here,
            [(Path::new("b.md"), "Alpha"), (Path::new("a.md"), "beta")]
        );
        let (folders, here) = pages.entries(Path::new("arch"));
        assert_eq!(folders, BTreeSet::from(["deep".to_string()]));
        assert_eq!(here, [(Path::new("arch/overview.md"), "Overview")]);
    }

    #[test]
    fn a_folder_is_shown_by_its_index_md_or_else_its_readme() {
        let pages = pages(&[
            ("a/index.md", "I"),
            ("a/readme.md", "R"),
            ("b/README.md", "R"),
            ("c/x.md", "X"),
        ]);
        assert_eq!(pages.home(Path::new("a")), Some(Path::new("a/index.md")));
        assert_eq!(pages.home(Path::new("b")), Some(Path::new("b/README.md")));
        assert_eq!(pages.home(Path::new("c")), None);
    }

    #[test]
    fn pages_are_titled_by_their_first_heading_or_their_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.md"), "Intro\n\n# The A\n").unwrap();
        std::fs::write(dir.path().join("b.md"), "No heading").unwrap();
        let files = [
            PathBuf::from("a.md"),
            PathBuf::from("b.md"),
            PathBuf::from("c.png"),
        ];
        let pages = Pages::new(&files, &Renderer::new(), |f| {
            std::fs::read_to_string(dir.path().join(f)).ok()
        });
        assert_eq!(
            pages.entries(Path::new("")).1,
            [(Path::new("b.md"), "b"), (Path::new("a.md"), "The A")]
        );
    }
}
