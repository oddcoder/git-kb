//! `git kb serve`: a folder of Markdown served as a site to read, rendered as each page is asked for, so an
//! edit shows on the next load. It serves the files `git kb build` would build, and no others: what
//! `.gitignore` and `.kbignore` ignore, `.git`, and anything outside the folder are not there.
//!
//! The site is under `/docs/`. A page is at its own path, `/docs/arch/overview.md`, so the links between pages
//! work as they are written; and at its built site's, `/docs/arch/overview.html`, so that a built site's links
//! work here too. A folder, `/docs/arch/`, shows its `index.md`, or else its `README.md`, or else a list of its
//! folders and pages, as a built site's does. Any other file is served as it is.
//!
//! When the folder is in a git repository, `/ref/rev/` is the site at a commit, `rev` being anything git takes, as
//! `/ref/main/arch/` or `/ref/dced2d8/arch/overview.md`, with a slash in it as `%2F`. Its files are the commit's,
//! with what its own ignore files ignore left out, as a build of it would have them.
//!
//! git-kb's own files are beside the site, not in it, so that no page of the docs can be taken for one: the
//! pages' style sheet and scripts under `/_assets/`, and `/_events`. Open pages load again when the site changes:
//! the server tells them, as events from `/_events`, and a line of script in each page, the shell's, reloads it.

use std::convert::Infallible;
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, anyhow};
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
use crate::config::LookConfig;
use crate::git::{Repo, Snapshot};
use crate::live::Live;
use crate::nav::{Nav, Pages};
use crate::render::Renderer;
use crate::shell::{self, Links, Shell};

/// Serves a folder of Markdown.
#[derive(Debug)]
pub struct Server {
    root: PathBuf,
    renderer: Renderer,
    shell: Shell,
    /// The repository the folder is in, if it is in one, for its commits.
    repo: Option<Repo>,
    /// The site's changes, told to open pages.
    changes: broadcast::Sender<()>,
}

/// What a request is answered from: the site's files, on disk or at a commit, and the shell of its pages.
struct View<'a> {
    server: &'a Server,
    files: Vec<PathBuf>,
    /// The commit shown, when it is not the folder as it is.
    snapshot: Option<Arc<Snapshot>>,
    shell: Shell,
}

impl Server {
    /// A server of a folder of Markdown, named `title`, or else by its folder.
    #[must_use]
    pub fn new(root: PathBuf, title: Option<String>) -> Self {
        let shell = Shell::new(Builder::new(title, None).title(&root), Links::Served);
        Self {
            repo: Repo::discover(&root),
            root,
            renderer: Renderer::new(),
            shell,
            changes: broadcast::channel(16).0,
        }
    }

    /// The same, serving pages in a look of their own.
    #[must_use]
    pub fn looking(self, look: LookConfig) -> Self {
        Self {
            shell: self.shell.looking(look),
            ..self
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
        info!(url = %format!("http://{address}/docs/"), root = %self.root.display(), "serving");
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

    /// The answer to a request: for a path of the site, as it is or at a commit, or for one of git-kb's files.
    fn answer(&self, path: &str) -> Response {
        debug!(path, "request");
        let (snapshot, rest) = if let Some(rest) = path.strip_prefix("/docs") {
            if rest.is_empty() {
                return Redirect::permanent("/docs/").into_response();
            }
            (None, rest)
        } else if let Some(rest) = path.strip_prefix("/ref/") {
            // The commit's name is taken apart from the path, as it may hold a slash, as %2F.
            let Some((name, rest)) = rest.find('/').map(|i| rest.split_at(i)) else {
                return Redirect::permanent(&format!("{path}/")).into_response();
            };
            let name = percent_decode_str(name).decode_utf8_lossy();
            match self.snapshot(&name) {
                Ok(snapshot) => (Some(snapshot), rest),
                Err(e) => return self.missing(&format!("{e:#}")),
            }
        } else if path == "/" {
            return Redirect::temporary("/docs/").into_response();
        } else {
            return self.asset(path);
        };
        if !rest.starts_with('/') {
            return self.missing(&format!("There is no page at {path}."));
        }
        let Ok(decoded) = percent_decode_str(rest).decode_utf8() else {
            return self.missing(&format!("There is no page at {path}."));
        };
        match self.view(snapshot) {
            Ok(view) => view.answer(decoded.trim_start_matches('/'), path),
            Err(e) => self.failed(&e),
        }
    }

    /// One of git-kb's own files: the pages' style sheet and scripts.
    fn asset(&self, path: &str) -> Response {
        let wanted = path.trim_start_matches('/');
        let Some((_, text)) = shell::ASSETS.iter().find(|(asset, _)| *asset == wanted) else {
            return self.missing(&format!("There is no page at {path}."));
        };
        let kind = mime_guess::from_path(wanted).first_or_octet_stream();
        (
            [(header::CONTENT_TYPE, format!("{kind}; charset=utf-8"))],
            *text,
        )
            .into_response()
    }

    /// The site's files at the commit `name` names.
    fn snapshot(&self, name: &str) -> anyhow::Result<Arc<Snapshot>> {
        self.repo
            .as_ref()
            .ok_or_else(|| anyhow!("{} is not in a git repository", self.root.display()))?
            .snapshot(name)
    }

    /// What a request is answered from: the folder as it is, or a commit of it.
    fn view(&self, snapshot: Option<Arc<Snapshot>>) -> anyhow::Result<View<'_>> {
        let files = match &snapshot {
            Some(snapshot) => snapshot.files(),
            None => Builder::files(&self.root, None)?,
        };
        let shell = self
            .shell
            .clone()
            .showing(snapshot.as_ref().map(|s| s.revision.clone()));
        Ok(View {
            server: self,
            files,
            snapshot,
            shell,
        })
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

    /// The answer for what is not there, before there is a site to look in, as for a commit that is not.
    fn missing(&self, said: &str) -> Response {
        let body = format!("<h1>Not found</h1>\n<p>{}</p>\n", Builder::escape(said));
        let page = self
            .shell
            .page(Path::new(""), "", "Not found", &body, &Nav::none());
        (StatusCode::NOT_FOUND, Html(page)).into_response()
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
            "<h1>Something went wrong</h1>\n",
            &Nav::none(),
        );
        (StatusCode::INTERNAL_SERVER_ERROR, Html(body)).into_response()
    }
}

impl View<'_> {
    /// The answer for a path of the site, `wanted`, from its top, decoded; `path` as it was asked for.
    fn answer(&self, wanted: &str, path: &str) -> Response {
        let files = &self.files;
        let at = Path::new(wanted);
        // A folder is a path with pages below it, or the top.
        if wanted.is_empty() || files.iter().any(|f| f.starts_with(at) && f != at) {
            // Without its slash, a folder's relative links would be taken against the folder above it.
            if !wanted.ends_with('/') && !wanted.is_empty() {
                return Redirect::permanent(&format!("{path}/")).into_response();
            }
            return self.folder(at);
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
                return self.folder(folder);
            }
            let page = files.iter().find(|f| {
                f.parent() == Some(folder)
                    && Server::is_markdown(f)
                    && f.file_stem().is_some_and(|s| s.eq_ignore_ascii_case(name))
            });
            if let Some(file) = page {
                return self.file(file);
            }
        }
        self.not_found(path)
    }

    /// A file of the site, as it is on disk or was at the commit.
    fn read(&self, file: &Path) -> anyhow::Result<Vec<u8>> {
        if let (Some(snapshot), Some(repo)) = (&self.snapshot, &self.server.repo) {
            return repo.read(snapshot, file);
        }
        let from = self.server.root.join(file);
        std::fs::read(&from).with_context(|| from.display().to_string())
    }

    /// The site's pages, for a page's sidebar.
    fn pages(&self) -> Pages {
        Pages::new(&self.files, &self.server.renderer, |f| {
            self.read(f)
                .ok()
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        })
    }

    /// A folder's page: its `index.md`, or else its `README.md`, or else a list of its folders and pages.
    fn folder(&self, at: &Path) -> Response {
        let pages = self.pages();
        if let Some(home) = pages.home(at) {
            return self.file(home);
        }
        let (folders, here) = pages.entries(at);
        let mut folder = Folder {
            folders,
            ..Folder::default()
        };
        for (file, title) in here {
            let html = file.with_extension("html");
            let html = html.file_name().unwrap_or_default().to_string_lossy();
            folder.pages.insert(html.into_owned(), title.to_owned());
        }
        Html(Builder::listing(&self.shell, &pages, at, &folder)).into_response()
    }

    /// A file of the site: a page, rendered, or any other file, as it is.
    fn file(&self, file: &Path) -> Response {
        let bytes = match self.read(file) {
            Ok(bytes) => bytes,
            Err(e) => return self.server.failed(&e),
        };
        if !Server::is_markdown(file) {
            let kind = mime_guess::from_path(file).first_or_octet_stream();
            return ([(header::CONTENT_TYPE, kind.to_string())], bytes).into_response();
        }
        let page = self
            .server
            .renderer
            .render(&String::from_utf8_lossy(&bytes));
        let title = page.title.unwrap_or_else(|| Server::stem(file));
        let at = file.parent().unwrap_or(Path::new(""));
        let pages = self.pages();
        let nav = Nav {
            pages: &pages,
            current: file,
            headings: &page.headings,
        };
        Html(self.shell.page(at, "", &title, &page.html, &nav)).into_response()
    }

    /// The answer for a path the site has nothing at, with its pages to go to instead.
    fn not_found(&self, path: &str) -> Response {
        let body = format!(
            "<h1>Not found</h1>\n<p>There is no page at {}.</p>\n",
            Builder::escape(path)
        );
        let pages = self.pages();
        let nav = Nav {
            pages: &pages,
            current: Path::new(""),
            headings: &[],
        };
        (
            StatusCode::NOT_FOUND,
            Html(self.shell.page(Path::new(""), "", "Not found", &body, &nav)),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use tower::ServiceExt;

    use super::*;

    /// A server of a folder: a temporary folder's docs/, with these files, which is removed after; or another.
    struct Site {
        docs: PathBuf,
        _made: Option<tempfile::TempDir>,
    }

    impl Site {
        /// A server of a folder made elsewhere, which outlives it.
        fn of(docs: PathBuf) -> Self {
            Self { docs, _made: None }
        }

        fn with(files: &[(&str, &[u8])]) -> Self {
            let dir = tempfile::tempdir().unwrap();
            for (path, bytes) in files {
                let path = dir.path().join(path);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, bytes).unwrap();
            }
            fs::create_dir_all(dir.path().join("docs")).unwrap();
            Self {
                docs: dir.path().join("docs"),
                _made: Some(dir),
            }
        }

        /// The status, content type, and body or else where it redirects, of the answer to a request.
        async fn get(&self, path: &str) -> (StatusCode, String, String) {
            let router = Arc::new(Server::new(self.docs.clone(), None)).router();
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
            "/docs/arch/overview.md",
            "/docs/arch/overview.html",
            "/docs/arch/OVERVIEW.html",
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
        assert!(
            site.get("/docs/")
                .await
                .2
                .contains("<title>Home · docs</title>")
        );
        assert!(
            site.get("/docs/index.html")
                .await
                .2
                .contains("<title>Home · docs</title>")
        );
        assert!(
            site.get("/docs/arch/")
                .await
                .2
                .contains("<title>Architecture · docs</title>")
        );
        assert!(
            site.get("/docs/arch/readme.html")
                .await
                .2
                .contains("<title>Architecture · docs</title>")
        );
        let list = site.get("/docs/algebra/").await.2;
        assert!(
            list.contains(r#"<a href="deep/index.html">deep/</a>"#),
            "{list}"
        );
        assert!(list.contains(r#"<a href="gemm.html">GEMM</a>"#), "{list}");
        assert!(
            site.get("/docs/algebra/deep/index.html")
                .await
                .2
                .contains(r#"<a href="x.html">X</a>"#)
        );
    }

    #[tokio::test]
    async fn pages_and_folders_have_the_site_s_pages_beside_them() {
        let site = Site::with(&[
            ("docs/arch/README.md", b"# Architecture"),
            ("docs/arch/dma.md", b"# DMA"),
            ("docs/x.md", b"# X"),
        ]);
        let dma = site.get("/docs/arch/dma.md").await.2;
        assert!(
            dma.contains(r#"<a href="/docs/arch/dma.html" aria-current="page">DMA</a>"#),
            "{dma}"
        );
        let arch = site.get("/docs/arch/").await.2;
        assert!(
            arch.contains(r#"<a href="/docs/arch/" aria-current="page">arch</a>"#),
            "{arch}"
        );
        let missing = site.get("/docs/nope.md").await.2;
        assert!(
            missing.contains(r#"<a href="/docs/x.html">X</a>"#),
            "{missing}"
        );
    }

    #[tokio::test]
    async fn the_site_is_shown_at_any_commit() {
        let git = crate::git::tests::Git::new();
        git.write("docs/arch/dma.md", "# DMA, first");
        git.commit("first");
        git.run(&["branch", "topic/x"]);
        git.write("docs/arch/dma.md", "# DMA, second");
        git.write("docs/new.md", "# New");
        git.commit("second");
        git.write("docs/arch/dma.md", "# DMA, not committed");
        let site = Site::of(git.top().join("docs"));

        assert!(
            site.get("/docs/arch/dma.md")
                .await
                .2
                .contains("<title>DMA, not committed")
        );
        assert!(
            site.get("/ref/main/arch/dma.md")
                .await
                .2
                .contains("<title>DMA, second")
        );
        let first = site.get("/ref/main~1/arch/dma.html").await.2;
        assert!(first.contains("<title>DMA, first"), "{first}");
        assert!(
            first.contains(r#"class="revision""#) && first.contains("@ main~1 · "),
            "{first}"
        );
        assert!(
            first.contains(r#"<a href="/ref/main~1/arch/">arch</a>"#),
            "its links stay at the commit: {first}"
        );
        assert!(
            !first.contains("EventSource"),
            "a commit does not change: {first}"
        );
        assert_eq!(
            site.get("/ref/main~1/new.md").await.0,
            StatusCode::NOT_FOUND
        );
        let branch = site.get("/ref/topic%2Fx/arch/").await.2;
        assert!(branch.contains(">DMA, first</a>"), "{branch}");
        assert!(
            branch.contains(r#"href="/ref/topic%2Fx/arch/""#),
            "{branch}"
        );
        let (status, _, location) = site.get("/ref/main").await;
        assert_eq!(
            (status, location.as_str()),
            (StatusCode::PERMANENT_REDIRECT, "/ref/main/")
        );
        let (status, _, body) = site.get("/ref/nope/").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("no revision nope"), "{body}");
        let style = site.get("/_assets/style.css").await;
        assert_eq!(style.2, shell::STYLE);
    }

    #[tokio::test]
    async fn the_site_is_under_docs_and_git_kb_s_own_files_beside_it() {
        let site = Site::with(&[
            ("docs/a.md", b"# A"),
            ("docs/_assets/style.css", b"the docs' own"),
        ]);
        let (status, _, location) = site.get("/").await;
        assert_eq!(
            (status, location.as_str()),
            (StatusCode::TEMPORARY_REDIRECT, "/docs/")
        );
        let (status, _, location) = site.get("/docs").await;
        assert_eq!(
            (status, location.as_str()),
            (StatusCode::PERMANENT_REDIRECT, "/docs/")
        );
        assert_eq!(site.get("/docs/_assets/style.css").await.2, "the docs' own");
        assert_eq!(site.get("/_assets/style.css").await.2, shell::STYLE);
        for path in ["/a.md", "/docsa.md", "/_assets/nope.css"] {
            assert_eq!(site.get(path).await.0, StatusCode::NOT_FOUND, "{path}");
        }
    }

    #[tokio::test]
    async fn outside_git_there_are_no_commits() {
        let site = Site::with(&[("docs/a.md", b"# A")]);
        let (status, _, body) = site.get("/ref/main/a.md").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("not in a git repository"), "{body}");
    }

    #[tokio::test]
    async fn a_folder_without_its_slash_is_sent_to_it() {
        let site = Site::with(&[("docs/arch/dma.md", b"# DMA")]);
        let (status, _, location) = site.get("/docs/arch").await;
        assert_eq!(
            (status, location.as_str()),
            (StatusCode::PERMANENT_REDIRECT, "/docs/arch/")
        );
    }

    #[tokio::test]
    async fn other_files_are_served_as_they_are() {
        let site = Site::with(&[("docs/img/chip.png", &[0x89, b'P', b'N', b'G'])]);
        let (status, kind, _) = site.get("/docs/img/chip.png").await;
        assert_eq!((status, kind.as_str()), (StatusCode::OK, "image/png"));
    }

    #[tokio::test]
    async fn names_with_spaces_are_found() {
        let site = Site::with(&[("docs/a b.md", b"# Spaced")]);
        assert!(
            site.get("/docs/a%20b.md")
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
            "/docs/../secret.md",
            "/docs/%2E%2E/secret.md",
            "/docs/.git/config",
            "/docs/target/a.md",
            "/docs/drafts/b.md",
            "/docs/nope.md",
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
    async fn pages_are_in_the_shell_and_its_files_are_served() {
        let site = Site::with(&[("docs/arch/dma.md", b"# DMA")]);
        let page = site.get("/docs/arch/dma.md").await.2;
        assert!(
            page.contains(r#"<link rel="stylesheet" href="/_assets/style.css">"#),
            "{page}"
        );
        assert!(page.contains(r#"<a href="/docs/arch/">arch</a>"#), "{page}");
        let (status, kind, style) = site.get("/_assets/style.css").await;
        assert_eq!(
            (status, kind.as_str()),
            (StatusCode::OK, "text/css; charset=utf-8")
        );
        assert_eq!(style, shell::STYLE);
        let (status, kind, script) = site.get("/_assets/look.js").await;
        assert_eq!(
            (status, kind.as_str()),
            (StatusCode::OK, "text/javascript; charset=utf-8")
        );
        assert_eq!(script, shell::LOOK);
    }

    #[tokio::test]
    async fn open_pages_are_told_of_changes() {
        let site = Site::with(&[("docs/a.md", b"# A")]);
        let server = Arc::new(Server::new(site.docs.clone(), None));
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
        let router = Arc::new(Server::new(site.docs.clone(), Some("pentasil".into()))).router();
        let response = router
            .oneshot(
                Request::get("/docs/arch/dma.md")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let page = String::from_utf8_lossy(&body);
        assert!(page.contains("<title>DMA · pentasil</title>"), "{page}");
    }
}
