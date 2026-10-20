//! `git kb serve`: a folder of Markdown served as a site to read, rendered as each page is asked for, so an
//! edit shows on the next load. It serves the files `git kb build` would build, and no others: what
//! `.gitignore` and `.kbignore` ignore, `.git`, and anything outside the folder are not there.
//!
//! A page is at its own path, `/arch/overview.md`, so the links between pages work as they are written; and
//! at its built site's, `/arch/overview.html`, so that a built site's links work here too. A folder, `/arch/`,
//! shows its `index.md`, or else its `README.md`, or else a list of its folders and pages, as a built site's
//! does. Any other file is served as it is.
//!
//! Open pages load again when the site changes: the server tells them, as events from `/_events`, and a line of
//! script in each page, the shell's, reloads it.

use std::convert::Infallible;
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use axum::Router;
use axum::extract::State;
use axum::http::{StatusCode, Uri, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use percent_encoding::percent_decode_str;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use tracing::{debug, error, info, warn};

use crate::build::{Builder, Folder};
use crate::live::Live;
use crate::render::Renderer;
use crate::shell::{self, Links, Shell};

/// Serves a folder of Markdown.
#[derive(Debug)]
pub struct Server {
    root: PathBuf,
    renderer: Renderer,
    shell: Shell,
    /// The site's changes, told to open pages.
    changes: broadcast::Sender<()>,
}

impl Server {
    /// A server of a folder of Markdown.
    #[must_use]
    pub fn new(root: PathBuf, title: Option<String>) -> Self {
        let shell = Shell::new(Builder::new(title, None).title(&root), Links::Served);
        Self {
            root,
            renderer: Renderer::new(),
            shell,
            changes: broadcast::channel(16).0,
        }
    }

    /// Listens at an address and answers requests until `shutdown` is done, and then the ones it has
    /// begun.
    ///
    /// # Errors
    ///
    /// When the address cannot be listened at, as when another program has its port, or serving fails.
    pub async fn run(
        self,
        address: SocketAddr,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> anyhow::Result<()> {
        let listener = TcpListener::bind(address)
            .await
            .with_context(|| address.to_string())?;
        info!(%address, root = %self.root.display(), "serving");
        // Without watching, pages are served still, but do not load again by themselves.
        let _live = Live::watch(&self.root, self.changes.clone())
            .inspect_err(|e| warn!("not watching for changes: {e:#}"))
            .ok();
        axum::serve(listener, Arc::new(self).router())
            .with_graceful_shutdown(shutdown)
            .await?;
        info!("stopped");
        Ok(())
    }

    /// What it answers each request with: the answer, worked out away from the threads that serve, as it
    /// reads the disk.
    fn router(self: Arc<Self>) -> Router {
        Router::new()
            .route(
                "/_events",
                get(|State(server): State<Arc<Self>>| async move { server.events() }),
            )
            .fallback(|State(server): State<Arc<Self>>, uri: Uri| async move {
                let path = uri.path().to_owned();
                tokio::task::spawn_blocking(move || server.answer(&path))
                    .await
                    .unwrap_or_else(|e| Self::broke(&e.into()))
            })
            .with_state(self)
    }

    /// The site's changes, as events, one each, for as long as the page listens.
    fn events(&self) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>> + use<>> {
        let changes = BroadcastStream::new(self.changes.subscribe())
            .filter_map(|change| change.ok().map(|()| Ok(Event::default().data("changed"))));
        Sse::new(changes).keep_alive(KeepAlive::default())
    }

    /// The answer to a request for a path of the site.
    fn answer(&self, path: &str) -> Response {
        debug!(path, "request");
        let Ok(decoded) = percent_decode_str(path).decode_utf8() else {
            return self.not_found(path);
        };
        let wanted = decoded.trim_start_matches('/');
        if wanted == shell::STYLE_PATH {
            return (
                [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                shell::STYLE,
            )
                .into_response();
        }
        let files = match Builder::files(&self.root, None) {
            Ok(files) => files,
            Err(e) => return self.failed(&e),
        };
        let at = Path::new(wanted);
        // A folder is a path with pages below it, or the top.
        if wanted.is_empty() || files.iter().any(|f| f.starts_with(at) && f != at) {
            // Without its slash, a folder's relative links would be taken against the folder above it.
            if !wanted.ends_with('/') && !wanted.is_empty() {
                return Redirect::permanent(&format!("{path}/")).into_response();
            }
            return self.folder(at, &files);
        }
        if let Some(file) = files.iter().find(|f| f.as_path() == at) {
            return self.file(file);
        }
        // A built site's page, a.html, is its Markdown's, a.md, whatever the case of the letters; and a
        // folder's index.html is the folder.
        if let Some(stem) = wanted.strip_suffix(".html") {
            let (folder, name) = stem.rsplit_once('/').unwrap_or(("", stem));
            let folder = Path::new(folder);
            if name.eq_ignore_ascii_case("index") && files.iter().any(|f| f.starts_with(folder)) {
                return self.folder(folder, &files);
            }
            let page = files.iter().find(|f| {
                f.parent() == Some(folder)
                    && Self::is_markdown(f)
                    && f.file_stem().is_some_and(|s| s.eq_ignore_ascii_case(name))
            });
            if let Some(file) = page {
                return self.file(file);
            }
        }
        self.not_found(path)
    }

    /// A folder's page: its `index.md`, or else its `README.md`, or else a list of its folders and pages.
    fn folder(&self, at: &Path, files: &[PathBuf]) -> Response {
        let here: Vec<&PathBuf> = files
            .iter()
            .filter(|f| f.parent().unwrap_or(Path::new("")) == at)
            .collect();
        for name in ["index.md", "README.md"] {
            if let Some(file) = here.iter().find(|f| Builder::named(f, name)) {
                return self.file(file);
            }
        }
        let mut folder = Folder::default();
        for file in here.iter().filter(|f| Self::is_markdown(f)) {
            let Ok(text) = std::fs::read_to_string(self.root.join(file)) else {
                continue;
            };
            let title = self
                .renderer
                .render(&text)
                .title
                .unwrap_or_else(|| Self::stem(file));
            let html = file.with_extension("html");
            let html = html.file_name().unwrap_or_default().to_string_lossy();
            folder.pages.insert(html.into_owned(), title);
        }
        for file in files.iter().filter(|f| Self::is_markdown(f)) {
            let Ok(below) = file.strip_prefix(at) else {
                continue;
            };
            let mut parts = below.components();
            if let (Some(Component::Normal(sub)), Some(_)) = (parts.next(), parts.next()) {
                folder.folders.insert(sub.to_string_lossy().into_owned());
            }
        }
        Html(Builder::listing(&self.shell, at, &folder)).into_response()
    }

    /// A file of the site: a page, rendered, or any other file, as it is.
    fn file(&self, file: &Path) -> Response {
        let from = self.root.join(file);
        let read = |e: std::io::Error| {
            self.failed(&anyhow::Error::from(e).context(from.display().to_string()))
        };
        if Self::is_markdown(file) {
            return match std::fs::read_to_string(&from) {
                Ok(text) => {
                    let page = self.renderer.render(&text);
                    let title = page.title.unwrap_or_else(|| Self::stem(file));
                    let at = file.parent().unwrap_or(Path::new(""));
                    Html(self.shell.page(at, "", &title, &page.html)).into_response()
                }
                Err(e) => read(e),
            };
        }
        match std::fs::read(&from) {
            Ok(bytes) => {
                let kind = mime_guess::from_path(file).first_or_octet_stream();
                ([(header::CONTENT_TYPE, kind.to_string())], bytes).into_response()
            }
            Err(e) => read(e),
        }
    }

    fn is_markdown(file: &Path) -> bool {
        file.extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("md"))
    }

    /// A file's name without its extension, for a page without a heading to be titled by.
    fn stem(file: &Path) -> String {
        file.file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    }

    fn not_found(&self, path: &str) -> Response {
        let body = format!(
            "<h1>Not found</h1>\n<p>There is no page at {}.</p>\n",
            Builder::escape(path)
        );
        (
            StatusCode::NOT_FOUND,
            Html(self.shell.page(Path::new(""), "", "Not found", &body)),
        )
            .into_response()
    }

    /// The answer when working one out failed, before there was a server to shape it.
    fn broke(e: &anyhow::Error) -> Response {
        error!("{e:#}");
        (StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong").into_response()
    }

    fn failed(&self, e: &anyhow::Error) -> Response {
        error!("{e:#}");
        let body = self.shell.page(
            Path::new(""),
            "",
            "Error",
            "<h1>Something went wrong</h1>
",
        );
        (StatusCode::INTERNAL_SERVER_ERROR, Html(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use tower::ServiceExt;

    use super::*;

    /// A server of a temporary folder's docs/, with these files, which is removed after.
    struct Site(tempfile::TempDir);

    impl Site {
        fn with(files: &[(&str, &[u8])]) -> Self {
            let dir = tempfile::tempdir().unwrap();
            for (path, bytes) in files {
                let path = dir.path().join(path);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, bytes).unwrap();
            }
            fs::create_dir_all(dir.path().join("docs")).unwrap();
            Self(dir)
        }

        /// The status, content type, and body or else where it redirects, of the answer to a request.
        async fn get(&self, path: &str) -> (StatusCode, String, String) {
            let router = Arc::new(Server::new(self.0.path().join("docs"), None)).router();
            let response = router
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            let header = |name| {
                response
                    .headers()
                    .get(name)
                    .map(|v: &header::HeaderValue| v.to_str().unwrap().to_owned())
            };
            let (status, kind, location) = (
                response.status(),
                header(header::CONTENT_TYPE).unwrap_or_default(),
                header(header::LOCATION),
            );
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let body = location.unwrap_or_else(|| String::from_utf8_lossy(&body).into_owned());
            (status, kind, body)
        }
    }

    #[tokio::test]
    async fn a_page_is_rendered_at_its_path_and_at_its_built_site_s() {
        let site = Site::with(&[(
            "docs/arch/overview.md",
            b"# Overview\n\nSee [the DMA](dma.md).",
        )]);
        for path in [
            "/arch/overview.md",
            "/arch/overview.html",
            "/arch/OVERVIEW.html",
        ] {
            let (status, kind, body) = site.get(path).await;
            assert_eq!(status, StatusCode::OK, "{path}");
            assert!(kind.starts_with("text/html"), "{path}: {kind}");
            assert!(
                body.contains("<title>Overview · docs</title>"),
                "{path}: {body}"
            );
            assert!(body.contains(r#"<a href="dma.md">"#), "{path}: {body}");
        }
    }

    #[tokio::test]
    async fn a_folder_shows_its_index_md_or_readme_or_a_list() {
        let site = Site::with(&[
            ("docs/index.md", b"# Home"),
            ("docs/arch/README.md", b"# Architecture"),
            ("docs/arch/dma.md", b"# DMA"),
            ("docs/algebra/gemm.md", b"# GEMM"),
            ("docs/algebra/deep/x.md", b"# X"),
        ]);
        assert!(site.get("/").await.2.contains("<title>Home · docs</title>"));
        assert!(
            site.get("/index.html")
                .await
                .2
                .contains("<title>Home · docs</title>")
        );
        assert!(
            site.get("/arch/")
                .await
                .2
                .contains("<title>Architecture · docs</title>")
        );
        assert!(
            site.get("/arch/readme.html")
                .await
                .2
                .contains("<title>Architecture · docs</title>")
        );
        let list = site.get("/algebra/").await.2;
        assert!(
            list.contains(r#"<a href="deep/index.html">deep/</a>"#),
            "{list}"
        );
        assert!(list.contains(r#"<a href="gemm.html">GEMM</a>"#), "{list}");
        assert!(
            site.get("/algebra/deep/index.html")
                .await
                .2
                .contains(r#"<a href="x.html">X</a>"#)
        );
    }

    #[tokio::test]
    async fn a_folder_without_its_slash_is_sent_to_it() {
        let site = Site::with(&[("docs/arch/dma.md", b"# DMA")]);
        let (status, _, location) = site.get("/arch").await;
        assert_eq!(
            (status, location.as_str()),
            (StatusCode::PERMANENT_REDIRECT, "/arch/")
        );
    }

    #[tokio::test]
    async fn other_files_are_served_as_they_are() {
        let site = Site::with(&[("docs/img/chip.png", &[0x89, b'P', b'N', b'G'])]);
        let (status, kind, _) = site.get("/img/chip.png").await;
        assert_eq!((status, kind.as_str()), (StatusCode::OK, "image/png"));
    }

    #[tokio::test]
    async fn names_with_spaces_are_found() {
        let site = Site::with(&[("docs/a b.md", b"# Spaced")]);
        assert!(
            site.get("/a%20b.md")
                .await
                .2
                .contains("<title>Spaced · docs</title>")
        );
    }

    #[tokio::test]
    async fn only_what_a_build_would_hold_is_served() {
        let site = Site::with(&[
            ("secret.md", b"# Outside"),
            ("docs/.git/config", b"[core]"),
            ("docs/.gitignore", b"target/\n"),
            ("docs/target/a.md", b"# Built"),
            ("docs/.kbignore", b"drafts/\n"),
            ("docs/drafts/b.md", b"# Draft"),
        ]);
        for path in [
            "/../secret.md",
            "/%2E%2E/secret.md",
            "/.git/config",
            "/target/a.md",
            "/drafts/b.md",
            "/nope.md",
        ] {
            let (status, _, body) = site.get(path).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {body}");
        }
    }

    #[tokio::test]
    async fn an_address_in_use_is_an_error_naming_it() {
        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = taken.local_addr().unwrap();
        let error = Server::new("/docs".into(), None)
            .run(address, async {})
            .await
            .unwrap_err();
        assert!(
            format!("{error:#}").contains(&address.to_string()),
            "{error:#}"
        );
    }

    #[tokio::test]
    async fn pages_are_in_the_shell_and_the_style_is_served() {
        let site = Site::with(&[("docs/arch/dma.md", b"# DMA")]);
        let page = site.get("/arch/dma.md").await.2;
        assert!(
            page.contains(r#"<link rel="stylesheet" href="/_assets/style.css">"#),
            "{page}"
        );
        assert!(page.contains(r#"<a href="/arch/">arch</a>"#), "{page}");
        let (status, kind, style) = site.get("/_assets/style.css").await;
        assert_eq!(
            (status, kind.as_str()),
            (StatusCode::OK, "text/css; charset=utf-8")
        );
        assert_eq!(style, shell::STYLE);
    }

    #[tokio::test]
    async fn open_pages_are_told_of_changes() {
        let site = Site::with(&[("docs/a.md", b"# A")]);
        let server = Arc::new(Server::new(site.0.path().join("docs"), None));
        let response = Arc::clone(&server)
            .router()
            .oneshot(Request::get("/_events").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let kind = response
            .headers()
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        assert_eq!(kind, "text/event-stream");
        server.changes.send(()).unwrap();
        let mut events = response.into_body().into_data_stream();
        let event = tokio::time::timeout(std::time::Duration::from_secs(5), events.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(&event[..], b"data: changed\n\n");
    }

    #[tokio::test]
    async fn the_site_s_title_names_it() {
        let site = Site::with(&[("docs/arch/dma.md", b"# DMA")]);
        let router = Arc::new(Server::new(
            site.0.path().join("docs"),
            Some("pentasil".into()),
        ))
        .router();
        let response = router
            .oneshot(Request::get("/arch/dma.md").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let page = String::from_utf8_lossy(&body);
        assert!(page.contains("<title>DMA · pentasil</title>"), "{page}");
    }
}
