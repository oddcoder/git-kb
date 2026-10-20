//! A page's links, in its sidebar: the graph of the pages around it, which `web/src/graph.ts` draws, and each page
//! it links to or that links to it, marked → for one way, ← for the other and ↔ for both, with the sections the
//! links sit in. And the graph of every page, as a page of its own.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use std::path::Path;

use super::{Links, Shell};
use crate::graph::{Graph, Under};
use crate::nav::{Nav, Pages};

impl Shell {
    /// The sidebar's links of the page shown: none for a page that links nowhere and that nothing links to.
    pub(super) fn links(&self, at: &Path, nav: &Nav) -> String {
        let (Some(graph), page) = (nav.graph, nav.current) else {
            return String::new();
        };
        let (to, from) = (graph.to(page), graph.from(page));
        if !graph.has(page) || (to.is_empty() && from.is_empty()) {
            return String::new();
        }
        // Every page once: the ones linked to in the order this page links them, then the others linking here.
        let mut others: Vec<&Path> = to.iter().map(|(p, _)| p.as_path()).collect();
        others.extend(
            from.iter()
                .map(|(p, _)| *p)
                .filter(|p| !to.iter().any(|(t, _)| t == p)),
        );
        let mut items = String::new();
        for other in others {
            let out = to
                .iter()
                .find(|(p, _)| p == other)
                .map_or(&[][..], |(_, s)| s.as_slice());
            let into = from
                .iter()
                .find(|(p, _)| *p == other)
                .map_or(&[][..], |(_, s)| *s);
            let arrow = match (out.is_empty(), into.is_empty()) {
                (false, false) => "↔",
                (false, true) => "→",
                _ => "←",
            };
            let name = match nav.pages.title(other) {
                Some(title) => format!(
                    "<a class=\"ref\" href=\"{}\"><span class=\"dir\">{arrow}</span> {}</a>",
                    self.page_link(at, other),
                    Self::escape(title)
                ),
                None => format!(
                    "<span class=\"ref missing\" title=\"No such page\"><span class=\"dir\">{arrow}</span> {}</span>",
                    Self::escape(&other.to_string_lossy())
                ),
            };
            let spots: String = out
                .iter()
                .map(|s| self.spot(at, nav, "→", None, s))
                .chain(into.iter().map(|s| self.spot(at, nav, "←", Some(other), s)))
                .collect();
            let _ = write!(items, "<li>{name}<span class=\"spots\">{spots}</span></li>");
        }
        format!(
            concat!(
                "<section class=\"links\"><h2>Links <a class=\"graph-all\" href=\"{all}\" title=\"Every page and link\">⤢</a></h2>",
                "<div class=\"kb-graph\" data-graph=\"{graph}\"></div><ul class=\"refs-list\">{items}</ul></section>\n"
            ),
            all = self.graph_link(at),
            graph = Self::escape(&self.graph_json(at, graph, nav.pages, Some(page))),
            items = items
        )
    }

    /// The graph of every page and link, as a page's body.
    #[must_use]
    pub fn graph_page(&self, at: &Path, graph: &Graph, pages: &Pages) -> String {
        format!(
            "<h1>Every page</h1>\n<div class=\"kb-graph full\" data-graph=\"{}\"></div>\n",
            Self::escape(&self.graph_json(at, graph, pages, None))
        )
    }

    /// Where a link sits, as a link to it: `→` a section of this page, `←` one of `on`, the page linking here. A link
    /// under no heading, or under the page's title, sits at its top.
    fn spot(&self, at: &Path, nav: &Nav, arrow: &str, on: Option<&Path>, under: &Under) -> String {
        let title = nav
            .pages
            .title(on.unwrap_or(nav.current))
            .unwrap_or_default();
        let (anchor, text) = match under {
            Some((id, heading)) if heading != title => {
                (format!("#{}", Self::encode_part(id)), Self::escape(heading))
            }
            _ => (String::new(), "top".to_string()),
        };
        let base = on.map(|p| self.page_link(at, p)).unwrap_or_default();
        let href = if base.is_empty() && anchor.is_empty() {
            "#".to_string()
        } else {
            format!("{base}{anchor}")
        };
        format!("<a class=\"spot\" href=\"{href}\">{arrow} {text}</a>")
    }

    /// The graph as `graph.ts` draws it: its pages, each its title, link, kind and how many links from `here` it
    /// is, and its links, each two pages and whether it goes both ways. Around a page, the pages one link from it;
    /// with none, every page.
    fn graph_json(&self, at: &Path, graph: &Graph, pages: &Pages, here: Option<&Path>) -> String {
        // The pages shown, and how far each is from here.
        let mut hop: BTreeMap<&Path, usize> = BTreeMap::new();
        match here {
            Some(page) => {
                hop.insert(page, 0);
                for (to, _) in graph.to(page) {
                    hop.entry(to).or_insert(1);
                }
                for (from, _) in graph.from(page) {
                    hop.entry(from).or_insert(1);
                }
            }
            None => graph.all().into_iter().for_each(|p| {
                hop.insert(p, 1);
            }),
        }
        let ids: BTreeMap<&Path, usize> = hop.keys().enumerate().map(|(i, p)| (*p, i)).collect();
        let nodes: Vec<serde_json::Value> = hop
            .iter()
            .map(|(p, d)| match pages.title(p) {
                Some(title) => serde_json::json!({
                    "t": title,
                    "h": self.page_link(at, p),
                    "k": if *d == 0 { "here" } else { "page" },
                    "d": d,
                }),
                None => serde_json::json!({
                    "t": format!("{} (missing)", p.display()),
                    "h": "",
                    "k": "missing",
                    "d": d,
                }),
            })
            .collect();
        // One line for each pair of pages: a link each way is one line with a head at both ends.
        let one_way: BTreeSet<(usize, usize)> = hop
            .keys()
            .flat_map(|from| {
                graph
                    .to(from)
                    .iter()
                    .map(move |(to, _)| (*from, to.as_path()))
            })
            .filter(|(f, t)| here.is_none_or(|h| *f == h || *t == h))
            .filter_map(|(f, t)| Some((*ids.get(f)?, *ids.get(t)?)))
            .collect();
        let edges: Vec<serde_json::Value> = one_way
            .iter()
            .filter(|(a, b)| !(one_way.contains(&(*b, *a)) && b < a))
            .map(|(a, b)| {
                if one_way.contains(&(*b, *a)) {
                    serde_json::json!([a, b, 1])
                } else {
                    serde_json::json!([a, b])
                }
            })
            .collect();
        serde_json::json!({ "nodes": nodes, "edges": edges }).to_string()
    }

    /// A link, from a page in the folder `at`, to a page of the site, as its built name, which a served site
    /// answers too.
    fn page_link(&self, at: &Path, page: &Path) -> String {
        self.to(at, &Self::encode(&page.with_extension("html")))
    }

    /// A link, from a page in the folder `at`, to the graph of every page: served, of the commit shown.
    fn graph_link(&self, at: &Path) -> String {
        match (self.links, &self.revision) {
            (Links::Served, None) => "/_graph".into(),
            (Links::Served, Some(r)) => format!("/_graph?ref={}", Self::encode_part(&r.name)),
            (Links::Built, _) => self.to(at, "_graph.html"),
        }
    }
}
