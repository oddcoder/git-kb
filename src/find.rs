//! Search, by Pagefind: an index of the site's pages, ranked, by words stemmed as their language has them, each hit
//! with an excerpt, its words marked, and the sections of the page that hold them; filtered by doctype and by
//! each field that is one of a few values. The index is a bundle of files, Pagefind's script and its look among them,
//! that the browser searches itself: a built site has them beside it, in `_pagefind/`; a served one has them from
//! the server, which builds them from the pages as they are, or at a commit, when they are first asked for.

use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use pagefind::api::PagefindIndex;
use pagefind::options::PagefindServiceConfig;
use serde_json::Value;

use crate::front::Front;
use crate::render::Renderer;
use crate::schema::{Doctypes, Kind};

/// Where a site's search bundle is, in the site.
pub const BUNDLE: &str = "_pagefind";

/// A page, as the index reads it: where it is, its title, its text rendered, and what it is filtered by.
#[derive(Debug, Clone)]
pub struct Page {
    pub url: String,
    pub title: String,
    pub html: String,
    pub filters: Vec<(String, String)>,
}

/// The files of a search bundle, by their names in it.
#[derive(Debug, Default)]
pub struct Bundle {
    files: BTreeMap<String, Vec<u8>>,
}

impl Bundle {
    /// A file of it, by its name, as `pagefind-ui.js`.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.files.get(name).map(Vec::as_slice)
    }

    /// Its files, each with its name.
    pub fn files(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.files.iter().map(|(n, b)| (n.as_str(), b.as_slice()))
    }
}

/// The site's pages, as the index reads them: each read by `read`, at the address `url` gives it, filtered by its
/// doctype and its fields that are one of a few values, as its doctype's schema has them.
#[must_use]
pub fn pages(
    files: &[std::path::PathBuf],
    renderer: &Renderer,
    doctypes: &Doctypes,
    read: impl Fn(&Path) -> Option<String>,
    url: impl Fn(&Path) -> String,
) -> Vec<Page> {
    files
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
            let filters = renderer
                .front(&text)
                .map(|front| filters(&front, doctypes))
                .unwrap_or_default();
            Some(Page {
                url: url(f),
                title,
                html: renderer.render(&text).html,
                filters,
            })
        })
        .collect()
}

/// What a page is filtered by: its doctype's name, and each of its fields its schema says is one of a few values.
fn filters(front: &Front, doctypes: &Doctypes) -> Vec<(String, String)> {
    let Some(doctype) = front.doctype().and_then(|id| doctypes.get(id)) else {
        return vec![];
    };
    let mut filters = vec![("doctype".to_string(), doctype.name.clone())];
    for name in doctype.schema.fields() {
        let (Some(field), Some(value)) = (doctype.schema.field(name), front.get(name)) else {
            continue;
        };
        if doctype.schema.kind(field) == Kind::Enum {
            let value = match value {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            filters.push((name.to_string(), value));
        }
    }
    filters
}

/// A page as HTML for the index: its title, and its text, the only part indexed, with its filters.
fn html(page: &Page) -> String {
    let escape = |t: &str| {
        t.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    };
    // Filters as attributes, whose values are not text of the page, to find or to show in an excerpt.
    let mut filters = String::new();
    for (k, v) in &page.filters {
        let _ = write!(
            filters,
            "<span data-pagefind-filter=\"{}:{}\"></span>",
            escape(k),
            escape(v)
        );
    }
    format!(
        "<!doctype html><html lang=\"en\"><head><title>{title}</title></head><body><main data-pagefind-body data-pagefind-meta=\"title:{title}\">{filters}{}</main></body></html>",
        page.html,
        title = escape(&page.title)
    )
}

/// The search bundle of these pages.
///
/// # Errors
///
/// When Pagefind cannot index a page or build the index.
pub async fn bundle(pages: &[Page]) -> anyhow::Result<Bundle> {
    let config = PagefindServiceConfig::builder()
        .keep_index_url(true)
        .exclude_selectors(vec![".kb-stage".into(), "script".into(), "style".into()])
        .build();
    let mut index = PagefindIndex::new(Some(config)).context("search's settings")?;
    for page in pages {
        index
            .add_html_file(None, Some(page.url.clone()), html(page))
            .await
            .with_context(|| format!("indexing {}", page.url))?;
    }
    let files = index
        .get_files()
        .await
        .context("building the search index")?;
    Ok(Bundle {
        files: files
            .into_iter()
            .map(|f| (f.filename.to_string_lossy().replace('\\', "/"), f.contents))
            .collect(),
    })
}

/// The same, from code that is not async: in the runtime it runs in, if it does, from one of its blocking threads,
/// or else in one of its own.
///
/// # Errors
///
/// As `bundle`, or when no runtime can be made.
pub fn bundle_blocking(pages: &[Page]) -> anyhow::Result<Arc<Bundle>> {
    let bundle = match tokio::runtime::Handle::try_current() {
        Ok(handle) => handle.block_on(bundle(pages)),
        Err(_) => tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(bundle(pages)),
    };
    bundle.map(Arc::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_site_s_pages_make_a_bundle_with_pagefind_s_script_and_look() {
        let page = Page {
            url: "/docs/a.html".into(),
            title: "Tiling".into(),
            html: "<p>Tiles reuse their data.</p>".into(),
            filters: vec![("doctype".into(), "Design".into())],
        };
        let bundle = bundle_blocking(&[page]).unwrap();
        for name in [
            "pagefind.js",
            "pagefind-ui.js",
            "pagefind-ui.css",
            "pagefind-entry.json",
        ] {
            assert!(bundle.get(name).is_some(), "{name}");
        }
        assert!(bundle.files().any(|(n, _)| n.starts_with("fragment/")));
        assert!(bundle.files().any(|(n, _)| n.starts_with("filter/")));
    }
}
