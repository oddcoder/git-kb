//! The links between a site's pages: for each page, the pages it links to, each with the sections the links sit
//! in, so that a page can show where it links and what links to it, and the graph of them all can be drawn. A
//! link names a page by its path, relative to the page or from the site's top; a page it names that is not there is
//! kept too, as missing.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use percent_encoding::percent_decode_str;

use crate::render::Renderer;

/// The section a link sits in: its heading's id and text, or none above the first heading.
pub type Under = Option<(String, String)>;

/// Each page's links to other pages.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Graph {
    /// Each page, by its path from the site's top, and the pages it links to, in the order it first does, with the
    /// sections it links from, each once.
    out: BTreeMap<PathBuf, Vec<(PathBuf, Vec<Under>)>>,
}

impl Graph {
    /// The links among a site's files, each page read by `read`.
    #[must_use]
    pub fn new(
        files: &[PathBuf],
        renderer: &Renderer,
        read: impl Fn(&Path) -> Option<String>,
    ) -> Self {
        let mut out = BTreeMap::new();
        for page in files.iter().filter(|f| Self::is_page(f)) {
            let Some(text) = read(page) else {
                continue;
            };
            let mut links: Vec<(PathBuf, Vec<Under>)> = vec![];
            for (dest, under) in renderer.link_sites(&text) {
                let Some(to) = Self::join(page, &dest).filter(|to| to != page) else {
                    continue;
                };
                match links.iter_mut().find(|(t, _)| *t == to) {
                    Some((_, sites)) if !sites.contains(&under) => sites.push(under),
                    Some(_) => {}
                    None => links.push((to, vec![under])),
                }
            }
            out.insert(page.clone(), links);
        }
        Self { out }
    }

    /// The pages a page links to, in the order it first does, each with the sections it links from.
    #[must_use]
    pub fn to(&self, page: &Path) -> &[(PathBuf, Vec<Under>)] {
        self.out.get(page).map_or(&[], Vec::as_slice)
    }

    /// The pages that link to a page, each with the sections, in it, that the links sit in.
    #[must_use]
    pub fn from(&self, page: &Path) -> Vec<(&Path, &[Under])> {
        self.out
            .iter()
            .filter(|(from, _)| from.as_path() != page)
            .filter_map(|(from, links)| {
                links
                    .iter()
                    .find(|(to, _)| to == page)
                    .map(|(_, sites)| (from.as_path(), sites.as_slice()))
            })
            .collect()
    }

    /// Every page, and every page linked to, there or not.
    #[must_use]
    pub fn all(&self) -> Vec<&Path> {
        let mut all: Vec<&Path> = self
            .out
            .iter()
            .flat_map(|(from, links)| {
                std::iter::once(from.as_path()).chain(links.iter().map(|(to, _)| to.as_path()))
            })
            .collect();
        all.sort();
        all.dedup();
        all
    }

    /// Whether a page is among the site's.
    #[must_use]
    pub fn has(&self, page: &Path) -> bool {
        self.out.contains_key(page)
    }

    /// A link's target, from a page, as the page it names, if it names a page of the site: relative to the page,
    /// or from the site's top; not to another site, nor to a file that is not Markdown.
    #[must_use]
    pub fn join(page: &Path, dest: &str) -> Option<PathBuf> {
        let path = dest.split(['#', '?']).next()?;
        let elsewhere = path.starts_with("//")
            || path
                .split('/')
                .next()
                .is_some_and(|first| first.contains(':'));
        if elsewhere || path.is_empty() {
            return None;
        }
        let path = percent_decode_str(path).decode_utf8().ok()?;
        let joined = match path.strip_prefix('/') {
            Some(from_top) => PathBuf::from(from_top),
            None => page.parent().unwrap_or(Path::new("")).join(path.as_ref()),
        };
        let mut tidy = PathBuf::new();
        for part in joined.components() {
            match part {
                Component::CurDir => {}
                Component::ParentDir => {
                    tidy.pop();
                }
                part => tidy.push(part),
            }
        }
        Self::is_page(&tidy).then_some(tidy)
    }

    fn is_page(path: &Path) -> bool {
        path.extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("md"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(pages: &[(&str, &str)]) -> Graph {
        let files: Vec<PathBuf> = pages.iter().map(|(p, _)| PathBuf::from(p)).collect();
        Graph::new(&files, &Renderer::new(), |f| {
            pages
                .iter()
                .find(|(p, _)| Path::new(p) == f)
                .map(|(_, t)| (*t).to_string())
        })
    }

    #[test]
    fn a_page_links_to_pages_and_is_linked_from_them() {
        let graph = graph(&[
            (
                "index.md",
                "See [the DMA](arch/dma.md) and [it again](arch/dma.md#x).\n",
            ),
            (
                "arch/dma.md",
                "# DMA\n\n## Back\n\n[Home](../index.md), [gone](missing.md), [web](https://x.org), [image](a.png).\n",
            ),
        ]);
        let to: Vec<&Path> = graph
            .to(Path::new("index.md"))
            .iter()
            .map(|(t, _)| t.as_path())
            .collect();
        assert_eq!(to, [Path::new("arch/dma.md")], "each page once");
        let dma: Vec<(&Path, &[Under])> = graph
            .to(Path::new("arch/dma.md"))
            .iter()
            .map(|(t, s)| (t.as_path(), s.as_slice()))
            .collect();
        let back: Under = Some(("back".into(), "Back".into()));
        assert_eq!(
            dma,
            [
                (Path::new("index.md"), std::slice::from_ref(&back)),
                (Path::new("arch/missing.md"), std::slice::from_ref(&back)),
            ]
        );
        let from = graph.from(Path::new("arch/dma.md"));
        assert_eq!(from.len(), 1);
        assert_eq!(from[0].0, Path::new("index.md"));
        assert!(!graph.has(Path::new("arch/missing.md")));
        assert_eq!(graph.all().len(), 3);
    }
}
