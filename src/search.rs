//! Search: the pages that hold every word asked for, best first. A page scores for each time a word is in it: most in
//! its title, more in a heading than in the text; and each hit points to the page's section that scores best. The
//! server searches as it is asked; a built site has its index as a script, which `web/src/search.ts` searches the
//! same way in the browser.

use std::path::{Path, PathBuf};

use crate::render::{Renderer, Section};

/// The site's pages, as search reads them: each one's title and sections.
#[derive(Debug, Default, Clone)]
pub struct Index {
    pages: Vec<Indexed>,
}

#[derive(Debug, Clone)]
struct Indexed {
    path: PathBuf,
    title: String,
    sections: Vec<Section>,
}

/// A page that holds every word asked for, and its section that holds them most.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub path: PathBuf,
    pub title: String,
    /// The section's heading's id, and its heading and text.
    pub anchor: Option<String>,
    pub heading: String,
    pub text: String,
    score: usize,
}

impl Index {
    /// The pages among a site's files, each read by `read`, titled by its first heading or else by its name.
    #[must_use]
    pub fn new(
        files: &[PathBuf],
        renderer: &Renderer,
        read: impl Fn(&Path) -> Option<String>,
    ) -> Self {
        let pages = files
            .iter()
            .filter(|f| f.extension().is_some_and(|e| e.eq_ignore_ascii_case("md")))
            .filter_map(|f| {
                let text = read(f)?;
                let title = renderer.title(&text).unwrap_or_else(|| {
                    f.file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                });
                Some(Indexed {
                    path: f.clone(),
                    title,
                    sections: renderer.sections(&text),
                })
            })
            .collect();
        Self { pages }
    }

    /// The pages that hold every word of a query, best first, and those as good by title.
    #[must_use]
    pub fn search(&self, query: &str) -> Vec<Hit> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        if words.is_empty() {
            return vec![];
        }
        let count = |hay: &str| {
            let hay = hay.to_lowercase();
            words
                .iter()
                .map(|w| hay.matches(w.as_str()).count())
                .sum::<usize>()
        };
        let mut hits: Vec<Hit> = self
            .pages
            .iter()
            .filter_map(|page| {
                let everything = page
                    .sections
                    .iter()
                    .fold(format!("{} {}", page.title, page.path.display()), |s, x| {
                        s + " " + &x.heading + " " + &x.text
                    });
                let everything = everything.to_lowercase();
                if !words.iter().all(|w| everything.contains(w.as_str())) {
                    return None;
                }
                let scores: Vec<usize> = page
                    .sections
                    .iter()
                    .map(|s| 5 * count(&s.heading) + count(&s.text))
                    .collect();
                let best =
                    (0..page.sections.len()).max_by_key(|&i| (scores[i], std::cmp::Reverse(i)))?;
                let section = &page.sections[best];
                Some(Hit {
                    path: page.path.clone(),
                    title: page.title.clone(),
                    anchor: section.id.clone(),
                    heading: section.heading.clone(),
                    text: section
                        .text
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" "),
                    score: 20 * count(&page.title) + scores.iter().sum::<usize>(),
                })
            })
            .collect();
        hits.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        });
        hits
    }

    /// The index as a script, for a built site to search in the browser: `window.KB_SEARCH = [...]`, a script
    /// rather than JSON, so that it loads from disk too. Each page is its path, its file, its title, and its
    /// sections as [heading id, heading, text].
    #[must_use]
    pub fn script(&self) -> String {
        let pages: Vec<serde_json::Value> = self
            .pages
            .iter()
            .map(|p| {
                let sections: Vec<serde_json::Value> = p
                    .sections
                    .iter()
                    .map(|s| {
                        serde_json::json!([
                            s.id,
                            s.heading,
                            s.text.split_whitespace().collect::<Vec<_>>().join(" ")
                        ])
                    })
                    .collect();
                serde_json::json!({
                    "path": p.path.to_string_lossy(),
                    "href": p.path.with_extension("html").to_string_lossy(),
                    "title": p.title,
                    "sections": sections,
                })
            })
            .collect();
        // "</" kept out, so that no text can close the script early.
        format!(
            "window.KB_SEARCH = {};\n",
            serde_json::Value::Array(pages)
                .to_string()
                .replace("</", "<\\/")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(pages: &[(&str, &str)]) -> Index {
        let files: Vec<PathBuf> = pages.iter().map(|(p, _)| PathBuf::from(p)).collect();
        Index::new(&files, &Renderer::new(), |f| {
            pages
                .iter()
                .find(|(p, _)| Path::new(p) == f)
                .map(|(_, t)| (*t).to_string())
        })
    }

    #[test]
    fn pages_with_every_word_are_found_best_first() {
        let index = index(&[
            ("a.md", "# DMA engine\n\nMoves data.\n"),
            (
                "b.md",
                "# Other\n\nThe dma moves data too.\n\n## Engine\n\nIt is a dma engine.\n",
            ),
            ("c.md", "# Nothing\n\nUnrelated.\n"),
        ]);
        let hits = index.search("DMA engine");
        let found: Vec<&str> = hits.iter().map(|h| h.title.as_str()).collect();
        assert_eq!(
            found,
            ["DMA engine", "Other"],
            "a word in the title counts most"
        );
        let other = &hits[1];
        assert_eq!(
            (other.anchor.as_deref(), other.heading.as_str()),
            (Some("engine"), "Engine"),
            "a hit points to its best section"
        );
        assert!(
            index.search("unrelated dma").is_empty(),
            "every word, or none"
        );
        assert_eq!(index.search("  "), []);
    }

    #[test]
    fn the_index_is_a_script_that_cannot_close_itself() {
        let script = index(&[("arch/a.md", "# A\n\n</script>\n")]).script();
        assert!(script.starts_with("window.KB_SEARCH = [{"), "{script}");
        assert!(script.contains(r#""href":"arch/a.html""#), "{script}");
        assert!(!script.contains("</script>"), "{script}");
    }
}
