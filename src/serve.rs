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
//! with what its own ignore files ignore left out, as a build of it would have them. Each page says when it last
//! changed, and `?history` after it, as `/docs/arch/overview.md?history`, lists the commits that changed it;
//! `/ref/` lists the branches and tags, and every commit that changed a page.
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
use tokio::sync::{broadcast, watch};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::{BroadcastStream, WatchStream};
use tracing::{debug, error, info, warn};

use crate::build::{Builder, Folder};
use crate::comments::Store;
use crate::config::{CommentsConfig, LookConfig};
use crate::diff::Diff;
use crate::git::{History, Repo, Snapshot};
use crate::graph::Graph;
use crate::live::Live;
use crate::nav::{Nav, Pages};
use crate::render::Renderer;
use crate::shell::{self, Comparison, Links, Margin, Notes, PageHistory, Shell, Version};

/// Serves a folder of Markdown.
#[derive(Debug)]
pub struct Server {
    root: PathBuf,
    renderer: Renderer,
    shell: Shell,
    /// The repository the folder is in, if it is in one, for its commits.
    repo: Option<Repo>,
    /// The review comments kept in it.
    comments: Option<Store>,
    /// The site's changes, told to open pages.
    changes: broadcast::Sender<()>,
    /// Whether it is stopping, told to open pages' event streams, which would otherwise never end.
    stopping: watch::Sender<bool>,
}

/// What a request is answered from: the site's files, on disk or at a commit, and the shell of its pages.
struct View<'a> {
    server: &'a Server,
    files: Vec<PathBuf>,
    /// The commit shown, when it is not the folder as it is.
    snapshot: Option<Arc<Snapshot>>,
    /// The last commit, when the folder as it is is shown, to tell which pages have changed since.
    head: Option<Arc<Snapshot>>,
    /// The commits up to the one shown, or to the last, that changed the site's pages.
    history: Option<Arc<History>>,
    shell: Shell,
}

impl Server {
    /// A server of a folder of Markdown, named `title`, or else by its folder.
    #[must_use]
    pub fn new(root: PathBuf, title: Option<String>) -> Self {
        let shell = Shell::new(Builder::new(title, None).title(&root), Links::Served);
        let repo = Repo::discover(&root);
        let comments = repo
            .as_ref()
            .map(|repo| Store::new(repo, &CommentsConfig::default().namespace));
        Self {
            repo,
            comments,
            root,
            renderer: Renderer::new(),
            shell,
            changes: broadcast::channel(16).0,
            stopping: watch::channel(false).0,
        }
    }

    /// The same, keeping the review comments as `config` says.
    #[must_use]
    pub fn commenting(self, config: &CommentsConfig) -> Self {
        let comments = self
            .repo
            .as_ref()
            .map(|repo| Store::new(repo, &config.namespace));
        Self { comments, ..self }
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
        let stopping = self.stopping.clone();
        axum::serve(listener, Arc::new(self).router())
            .with_graceful_shutdown(async move {
                shutdown.await;
                info!("stopping");
                // The pages' event streams end, so that the requests begun do.
                stopping.send_replace(true);
            })
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
                let (path, query) = (uri.path().to_owned(), uri.query().map(str::to_owned));
                tokio::task::spawn_blocking(move || server.answer(&path, query.as_deref()))
                    .await
                    .unwrap_or_else(|e| Self::broke(&e.into()))
            })
            .with_state(self)
    }

    /// The site's changes, as events, one each, for as long as the page listens, or until the server stops.
    fn events(&self) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>> + use<>> {
        let changes = BroadcastStream::new(self.changes.subscribe()).filter_map(|change| {
            change
                .ok()
                .map(|()| Some(Ok(Event::default().data("changed"))))
        });
        let stopped = WatchStream::new(self.stopping.subscribe())
            .filter_map(|stopping| stopping.then_some(None));
        let events = changes
            .merge(stopped)
            .take_while(Option::is_some)
            .filter_map(|event| event);
        Sse::new(events).keep_alive(KeepAlive::default())
    }

    /// The answer to a request: for a path of the site, as it is or at a commit, or for one of git-kb's files.
    fn answer(&self, path: &str, query: Option<&str>) -> Response {
        debug!(path, query, "request");
        if path == "/ref/" {
            return self.revisions();
        }
        if path == "/_search" {
            return self.search(query.unwrap_or(""));
        }
        if path == "/_graph" {
            return self.graph_page(query.unwrap_or(""));
        }
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
            Ok(view) => view.answer(decoded.trim_start_matches('/'), path, query),
            Err(e) => self.failed(&e),
        }
    }

    /// A query's value, by its name, decoded.
    fn param(query: &str, name: &str) -> Option<String> {
        query.split('&').find_map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (k == name).then(|| {
                percent_decode_str(&v.replace('+', " "))
                    .decode_utf8_lossy()
                    .into_owned()
            })
        })
    }

    /// What a request of the site's own, as `/_search`, is answered from: the folder as it is, or the commit its
    /// query's `ref` names.
    fn view_of(&self, query: &str) -> Result<View<'_>, Box<Response>> {
        let snapshot = Self::param(query, "ref")
            .map(|name| self.snapshot(&name))
            .transpose()
            .map_err(|e| Box::new(self.missing(&format!("{e:#}"))))?;
        self.view(snapshot).map_err(|e| Box::new(self.failed(&e)))
    }

    /// The graph of every page and link, as a page.
    fn graph_page(&self, query: &str) -> Response {
        let view = match self.view_of(query) {
            Ok(view) => view,
            Err(answer) => return *answer,
        };
        let (pages, graph) = (view.pages(), view.graph());
        let body = view.shell.graph_page(Path::new(""), &graph, &pages);
        let nav = Nav {
            pages: &pages,
            current: Path::new(""),
            headings: &[],
            graph: None,
        };
        Html(view.shell.page(Path::new(""), "", "Graph", &body, &nav)).into_response()
    }

    /// What a search finds: in the folder as it is, or at the commit `ref` names.
    fn search(&self, query: &str) -> Response {
        let asked = Self::param(query, "q").unwrap_or_default();
        let view = match self.view_of(query) {
            Ok(view) => view,
            Err(answer) => return *answer,
        };
        let index = crate::search::Index::new(&view.files, &self.renderer, |f| {
            view.read(f)
                .ok()
                .map(|b| String::from_utf8_lossy(&b).into_owned())
        });
        let shell = view.shell.clone().asking(&asked);
        let body = shell.results(&asked, &index.search(&asked));
        let pages = view.pages();
        let nav = Nav {
            pages: &pages,
            current: Path::new(""),
            headings: &[],
            graph: None,
        };
        Html(shell.page(Path::new(""), "", "Search", &body, &nav)).into_response()
    }

    /// The list of the branches and tags, and of every commit up to the last that changed a page.
    fn revisions(&self) -> Response {
        let listed = self.repo.as_ref().map(|repo| -> anyhow::Result<String> {
            let head = repo.snapshot("HEAD")?;
            let refs: Vec<_> = repo.refs()?.iter().map(|s| s.revision.clone()).collect();
            Ok(self.shell.revisions(&refs, &*repo.history(&head)?))
        });
        match listed {
            Some(Ok(body)) => {
                let shell = self.shell.clone().in_git(true);
                Html(shell.page(Path::new(""), "", "Revisions", &body, &Nav::none()))
                    .into_response()
            }
            Some(Err(e)) => self.missing(&format!("{e:#}")),
            None => self.missing(&format!(
                "{} is not in a git repository",
                self.root.display()
            )),
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
        // A folder in a repository without commits yet has no history.
        let head = match (&snapshot, &self.repo) {
            (None, Some(repo)) => repo.snapshot("HEAD").ok(),
            _ => None,
        };
        let history = match (&self.repo, snapshot.as_ref().or(head.as_ref())) {
            (Some(repo), Some(at)) => Some(repo.history(at)?),
            _ => None,
        };
        let shell = self
            .shell
            .clone()
            .in_git(self.repo.is_some())
            .showing(snapshot.as_ref().map(|s| s.revision.clone()));
        Ok(View {
            server: self,
            files,
            snapshot,
            head,
            history,
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
    fn answer(&self, wanted: &str, path: &str, query: Option<&str>) -> Response {
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
            return self.asked(file, query);
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
                return self.asked(file, query);
            }
        }
        self.not_found(path)
    }

    /// A file of the site, as its query asks, at its own path or a built site's: a page's Markdown, as it is, for
    /// `?raw`; its history, for `?history`; it compared, for `?diff=rev`, with itself as shown, or at `to=rev`;
    /// or else the file itself.
    fn asked(&self, file: &Path, query: Option<&str>) -> Response {
        let query = query.unwrap_or("");
        if !Server::is_markdown(file) {
            return self.file(file);
        }
        if query == "raw" {
            return match self.read(file) {
                Ok(bytes) => {
                    ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], bytes).into_response()
                }
                Err(e) => self.server.failed(&e),
            };
        }
        if query == "history" {
            return self.history(file);
        }
        if let Some(from) = Server::param(query, "diff") {
            return self.compare(file, &from, Server::param(query, "to").as_deref());
        }
        self.file(file)
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

    /// The links between the site's pages.
    fn graph(&self) -> Graph {
        Graph::new(&self.files, &self.server.renderer, |f| {
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
        let text = String::from_utf8_lossy(&bytes);
        let render = |body: &str| self.server.renderer.render_safe(body, &|_| None).html;
        let notes = self.notes(file, &text, &render);
        let page = self.render(file, &text, &bytes, notes.as_ref());
        let title = page.title.unwrap_or_else(|| Server::stem(file));
        let at = file.parent().unwrap_or(Path::new(""));
        let pages = self.pages();
        let graph = self.graph();
        let nav = Nav {
            pages: &pages,
            current: file,
            headings: &page.headings,
            graph: Some(&graph),
        };
        let body = self
            .shell
            .noted(at, file, &page.html, &self.page_history(file, &bytes));
        let body = Shell::with_source(&body, &self.server.renderer.source(&text));
        let rail = notes
            .map(|notes| self.shell.rail(&notes))
            .unwrap_or_default();
        Html(
            self.shell
                .page_with_rail(at, "", &title, &body, &nav, &rail),
        )
        .into_response()
    }

    /// The comments on a page as shown, when there are any: at the commit shown, or, for the folder as it is,
    /// the last.
    fn notes<'r>(
        &self,
        file: &Path,
        text: &str,
        render: &'r dyn Fn(&str) -> String,
    ) -> Option<Notes<'r>> {
        let store = self.server.comments.as_ref()?;
        let at = self.snapshot.as_ref().or(self.head.as_ref())?;
        let on = store
            .on_page(file, at.revision.id, text, &self.server.renderer)
            .inspect_err(|e| warn!(page = %file.display(), "comments not read: {e:#}"))
            .ok()?;
        Some(Notes::new(on, text, render)).filter(|notes| !notes.is_empty())
    }

    /// A page rendered, each block marked in its margin with when it last changed, when git knows, and with its
    /// comments.
    fn render(
        &self,
        file: &Path,
        text: &str,
        bytes: &[u8],
        notes: Option<&Notes>,
    ) -> crate::render::Page {
        let renderer = &self.server.renderer;
        let (Some(repo), Some(at)) = (
            &self.server.repo,
            self.snapshot.as_ref().or(self.head.as_ref()),
        ) else {
            return renderer.render(text);
        };
        // The folder as it is is blamed as it is on disk, its lines not committed yet as such.
        let now = self.snapshot.is_none().then_some(bytes);
        let Ok(blame) = repo.blame(at, file, now) else {
            return renderer.render(text);
        };
        let margin = Margin::new(&self.shell, file, text, &blame, repo, renderer, &|_| None);
        renderer.render_marked(text, &|_| None, &|range| {
            self.shell.marked(margin.mark(range.clone()), notes, &range)
        })
    }

    /// A page's history, as a page: the commits that changed it.
    fn history(&self, file: &Path) -> Response {
        let bytes = match self.read(file) {
            Ok(bytes) => bytes,
            Err(e) => return self.server.failed(&e),
        };
        let title = self
            .server
            .renderer
            .title(&String::from_utf8_lossy(&bytes))
            .unwrap_or_else(|| Server::stem(file));
        let at = file.parent().unwrap_or(Path::new(""));
        let body = self
            .shell
            .history(at, file, &title, &self.page_history(file, &bytes));
        let pages = self.pages();
        let nav = Nav {
            pages: &pages,
            current: file,
            headings: &[],
            graph: None,
        };
        let title = format!("History of {title}");
        Html(self.shell.page(at, "", &title, &body, &nav)).into_response()
    }

    /// A page compared: as it was at the commit `from` names, with it at the one `to` names, or as shown.
    fn compare(&self, file: &Path, from: &str, to: Option<&str>) -> Response {
        let Some(repo) = &self.server.repo else {
            return self.server.missing(&format!(
                "{} is not in a git repository",
                self.server.root.display()
            ));
        };
        let text = |bytes: Vec<u8>| String::from_utf8_lossy(&bytes).into_owned();
        // Before the first commit, there is no page to link to.
        let before = repo
            .snapshot(from)
            .is_ok()
            .then(|| self.shell.at_commit(from, file))
            .flatten();
        let old = match repo.snapshot(from) {
            // A page not at the commit was added since.
            Ok(at) => repo.read(&at, file).map(text).unwrap_or_default(),
            // The commit before the first is nothing.
            Err(_)
                if from
                    .strip_suffix('^')
                    .is_some_and(|c| repo.snapshot(c).is_ok()) =>
            {
                String::new()
            }
            Err(e) => return self.server.missing(&format!("{e:#}")),
        };
        let new = match to {
            // A page not at the commit was removed by then.
            Some(to) => match repo.snapshot(to) {
                Ok(at) => repo.read(&at, file).map(text).unwrap_or_default(),
                Err(e) => return self.server.missing(&format!("{e:#}")),
            },
            None => match self.read(file) {
                Ok(bytes) => text(bytes),
                Err(e) => return self.server.failed(&e),
            },
        };
        let renderer = &self.server.renderer;
        let title = renderer.title(&new).unwrap_or_else(|| Server::stem(file));
        let at = file.parent().unwrap_or(Path::new(""));
        let diff = Diff::pages(&old, &new, renderer, &|_| None);
        let then = match (to, &self.snapshot) {
            (Some(to), _) => Version {
                label: to.to_owned(),
                href: self.shell.at_commit(to, file),
            },
            (None, Some(snapshot)) => Version {
                label: snapshot.revision.name.clone(),
                href: Some(self.shell.link(at, file)),
            },
            (None, None) => Version {
                label: "the working copy".into(),
                href: Some(self.shell.link(at, file)),
            },
        };
        let comparison = Comparison {
            title: &title,
            from: Version {
                label: from.to_owned(),
                href: before,
            },
            to: then,
            diff: &diff,
            before: renderer.render(&old).html,
            after: renderer.render(&new).html,
            lines: Diff::lines(&old, &new),
        };
        let pages = self.pages();
        let nav = Nav {
            pages: &pages,
            current: file,
            headings: &[],
            graph: None,
        };
        let body = self.shell.compare(&comparison);
        let title = format!("Changes to {title}");
        Html(self.shell.page(at, "", &title, &body, &nav)).into_response()
    }

    /// Where a page, of these bytes, stands against the last commit, and the commits that changed it.
    fn page_history(&self, file: &Path, bytes: &[u8]) -> PageHistory<'_> {
        PageHistory::of(self.head.as_deref(), self.history.as_deref(), file, bytes)
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
            graph: None,
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
            !first.contains("live.js"),
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
    async fn a_page_says_when_it_last_changed_and_lists_its_commits() {
        let git = crate::git::tests::Git::new();
        git.write("docs/a.md", "# A\n\nFirst.");
        git.commit("write a");
        git.write("docs/a.md", "# A\n\nSecond.");
        git.commit("edit a");
        git.run(&["tag", "v1"]);
        let site = Site::of(git.top().join("docs"));

        let a = site.get("/docs/a.md").await.2;
        assert!(a.contains("<p class=\"meta\">Last changed <span"), "{a}");
        assert!(a.contains(" by T in <a href=\"/ref/"), "{a}");
        assert!(
            a.contains(r#"<a href="/docs/a.md?history">History</a>"#),
            "{a}"
        );
        assert!(
            a.contains(r#"<a class="revisions" href="/ref/">Revisions</a>"#),
            "{a}"
        );
        let history = site.get("/docs/a.md?history").await.2;
        assert!(
            history.contains("<title>History of A · docs</title>"),
            "{history}"
        );
        let edit = history.find("edit a").unwrap();
        assert!(
            edit < history.find("write a").unwrap(),
            "newest first: {history}"
        );

        git.write("docs/a.md", "# A\n\nThird, not committed.");
        git.write("docs/b.md", "# B");
        assert!(
            site.get("/docs/a.md")
                .await
                .2
                .contains("Changed since the last commit")
        );
        assert!(
            site.get("/docs/b.md")
                .await
                .2
                .contains("New page, not committed yet")
        );
        assert!(
            !site.get("/ref/v1/a.md").await.2.contains("Changed since"),
            "a commit is as it is"
        );

        let revisions = site.get("/ref/").await.2;
        assert!(
            revisions.contains(r#"<a href="/ref/main/">main</a>"#),
            "{revisions}"
        );
        assert!(
            revisions.contains(r#"<a href="/ref/v1/">v1</a>"#),
            "{revisions}"
        );
        assert!(
            revisions.contains("edit a") && revisions.contains("write a"),
            "{revisions}"
        );
    }

    #[tokio::test]
    async fn a_page_is_compared_with_itself_at_a_commit() {
        let git = crate::git::tests::Git::new();
        git.write("docs/a.md", "# A\n\nThe quick brown fox.\n");
        git.commit("write a");
        git.write("docs/a.md", "# A\n\nThe quick red fox.\n\nMore.\n");
        git.commit("edit a");
        git.write("docs/a.md", "# A\n\nThe quick red fox.\n");
        let site = Site::of(git.top().join("docs"));

        let uncommitted = site.get("/docs/a.md?diff=HEAD").await.2;
        assert!(
            uncommitted.contains("<title>Changes to A · docs</title>"),
            "{uncommitted}"
        );
        assert!(uncommitted.contains("1 change from"), "{uncommitted}");
        assert!(
            uncommitted.contains(r#"<div class="diff-del" id="change-1"><p>More.</p>"#),
            "{uncommitted}"
        );
        let page = site.get("/docs/a.md").await.2;
        assert!(
            page.contains(r#"<a href="/docs/a.md?diff=HEAD">Changed since the last commit</a>"#),
            "{page}"
        );
        assert!(page.contains("?diff="), "{page}");

        let commit = site.get("/ref/main/a.md?diff=main%5E").await.2;
        assert!(
            commit.contains("<del>brown</del><ins>red</ins>"),
            "{commit}"
        );
        let first = site.get("/ref/main~1/a.md?diff=main~1%5E").await.2;
        assert!(
            first.contains(r#"<div class="diff-ins" id="change-1">"#),
            "before the first commit, the page was not: {first}"
        );
        assert_eq!(
            site.get("/docs/a.md?diff=nope").await.0,
            StatusCode::NOT_FOUND
        );
        let html = site.get("/ref/main/a.html?diff=main^").await.2;
        assert!(
            html.contains("<del>brown</del><ins>red</ins>"),
            "a built page's address asks too: {html}"
        );
        assert!(
            site.get("/docs/a.html?history")
                .await
                .2
                .contains("History of A")
        );
        assert_eq!(
            site.get("/docs/a.html?raw").await.1,
            "text/plain; charset=utf-8"
        );
        let page = site.get("/docs/a.md").await.2;
        let link = page
            .split("(<a href=\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .unwrap()
            .replace("&amp;", "&");
        assert!(page.contains("%5E&amp;to="), "{page}");
        assert!(
            link.starts_with("/docs/a.html?diff="),
            "the site stays as it is: {link}"
        );
        let changes = site.get(&link).await.2;
        assert!(
            changes.contains("<del>brown</del><ins>red</ins>"),
            "{changes}"
        );
        assert!(
            changes.contains(r#"<div class="diff-ins" id="change-2"><p>More.</p>"#),
            "the commit's changes, which added More., not the working copy's, which took it out: {changes}"
        );
        assert!(!changes.contains(r#"class="revision""#), "{changes}");
    }

    #[tokio::test]
    async fn each_block_says_when_it_last_changed() {
        let git = crate::git::tests::Git::new();
        git.write("docs/a.md", "# A\n\nThe quick brown fox.\n");
        git.commit("write a");
        git.write("docs/a.md", "# A\n\nThe quick red fox.\n");
        git.commit("edit a");
        git.write("docs/a.md", "# A\n\nThe quick red fox.\n\nNot committed.\n");
        let site = Site::of(git.top().join("docs"));
        let page = site.get("/docs/a.md").await.2;
        assert!(
            page.contains(r#"<div class="blk ">"#) && page.contains("· T · "),
            "{page}"
        );
        assert!(
            page.contains("<b>T</b>") && page.contains("edit a</p>"),
            "{page}"
        );
        assert!(
            page.contains("<del>brown</del><ins>red</ins>"),
            "the card shows how it changed: {page}"
        );
        assert!(
            page.contains(r#"<div class="blk new"><button class="blame""#)
                && page.contains(">not committed yet</button>"),
            "{page}"
        );
        assert!(
            page.contains("?diff=HEAD\">Every uncommitted change"),
            "{page}"
        );
        let then = site.get("/ref/main~1/a.md").await.2;
        assert!(!then.contains("not committed yet"), "{then}");
    }

    #[tokio::test]
    async fn outside_git_there_are_no_commits() {
        let site = Site::with(&[("docs/a.md", b"# A")]);
        let (status, _, body) = site.get("/ref/main/a.md").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("not in a git repository"), "{body}");
    }

    #[tokio::test]
    async fn a_page_shows_its_source_too_and_gives_it_raw() {
        let site = Site::with(&[("docs/a.md", b"# A\n\n*Some* text.\n")]);
        let page = site.get("/docs/a.md").await.2;
        assert!(
            page.contains(r#"<input type="checkbox" id="v-source" class="toggle">"#),
            "{page}"
        );
        assert!(
            page.contains(r#"<div class="p-source">"#) && page.contains(r#"<pre class="source">"#),
            "{page}"
        );
        let (status, kind, raw) = site.get("/docs/a.md?raw").await;
        assert_eq!(
            (status, kind.as_str(), raw.as_str()),
            (
                StatusCode::OK,
                "text/plain; charset=utf-8",
                "# A\n\n*Some* text.\n"
            )
        );
    }

    #[tokio::test]
    async fn the_site_is_searched_as_it_is_or_at_a_commit() {
        let git = crate::git::tests::Git::new();
        git.write("docs/a.md", "# DMA\n\nMoves data.\n");
        git.commit("first");
        git.write("docs/a.md", "# DMA\n\nMoves bytes.\n");
        let site = Site::of(git.top().join("docs"));
        let found = site.get("/_search?q=moves+bytes").await.2;
        assert!(found.contains("1 page matches “moves bytes”."), "{found}");
        assert!(
            found.contains(r#"<a href="/docs/a.md#dma"><strong>DMA</strong>"#),
            "{found}"
        );
        assert!(
            found.contains("<mark>Moves</mark> <mark>bytes</mark>"),
            "{found}"
        );
        assert!(
            found.contains(r#"value="moves bytes""#),
            "the box says what was searched for: {found}"
        );
        let then = site.get("/_search?q=bytes&ref=main").await.2;
        assert!(then.contains("Nothing matches"), "{then}");
        let page = site.get("/ref/main/a.md").await.2;
        assert!(
            page.contains(r#"<input type="hidden" name="ref" value="main">"#),
            "{page}"
        );
    }

    #[tokio::test]
    async fn a_page_shows_its_links_and_the_graph_of_them() {
        let site = Site::with(&[
            (
                "docs/a.md",
                b"# A\n\n## Part\n\nSee [B](b.md) and [gone](gone.md).\n",
            ),
            ("docs/b.md", b"# B\n\nBack to [A](a.md).\n"),
            ("docs/c.md", b"# C\n"),
        ]);
        let a = site.get("/docs/a.md").await.2;
        assert!(
            a.contains(r#"<a class="ref" href="/docs/b.html"><span class="dir">↔</span> B</a>"#),
            "{a}"
        );
        assert!(
            a.contains(r##"<a class="spot" href="#part">→ Part</a>"##),
            "{a}"
        );
        assert!(
            a.contains(r#"<a class="spot" href="/docs/b.html">← top</a>"#),
            "{a}"
        );
        assert!(a.contains(r#"<span class="ref missing" title="No such page"><span class="dir">→</span> gone.md</span>"#), "{a}");
        assert!(a.contains(r#"<div class="kb-graph" data-graph=""#), "{a}");
        assert!(
            !site.get("/docs/c.md").await.2.contains("refs-list"),
            "a page with no links has none"
        );
        let graph = site.get("/_graph").await.2;
        assert!(
            graph.contains(r#"<div class="kb-graph full""#) && graph.contains("gone.md (missing)"),
            "{graph}"
        );
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
    async fn open_pages_stop_listening_when_the_server_stops() {
        let site = Site::with(&[("docs/a.md", b"# A")]);
        let server = Arc::new(Server::new(site.docs.clone(), None));
        let response = Arc::clone(&server)
            .router()
            .oneshot(Request::get("/_events").body(Body::empty()).unwrap())
            .await
            .unwrap();
        server.stopping.send_replace(true);
        let mut events = response.into_body().into_data_stream();
        let end = tokio::time::timeout(std::time::Duration::from_secs(5), events.next())
            .await
            .expect("the stream ends, as the server could not stop while it is open");
        assert!(end.is_none(), "{end:?}");
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

    /// A repository whose docs/a.md has a thread on its last paragraph, begun by Ada with this comment.
    fn commented(comment: &str) -> crate::git::tests::Git {
        let git = crate::git::tests::Git::new();
        git.run(&["config", "user.name", "Ada"]);
        git.run(&["config", "user.email", "ada@x"]);
        let page = "# A\n\nOne.\n\nTwo.\n";
        git.write("docs/a.md", page);
        git.commit("A page");
        let repo = Repo::discover(&git.top().join("docs")).unwrap();
        let begin = crate::comments::Begin {
            page: Path::new("a.md"),
            commit: repo.snapshot("HEAD").unwrap().revision.id,
            text: page,
            lines: 4..5,
            snippet: crate::comments::Snippet::default(),
        };
        Store::new(&repo, &CommentsConfig::default().namespace)
            .begin(&begin, comment)
            .unwrap();
        git
    }

    #[tokio::test]
    async fn a_page_shows_its_comments_beside_it() {
        let git = commented("Why **two**? <script>alert(1)</script>");
        let body = Site::of(git.top().join("docs")).get("/docs/a.html").await.2;
        assert!(body.contains("<div class=\"layout has-rail\">"), "{body}");
        assert!(
            body.contains("<aside class=\"rail\" aria-label=\"Comments\">"),
            "{body}"
        );
        assert!(body.contains("Ada · 1 comment"), "{body}");
        assert!(body.contains("Why <strong>two</strong>?"), "{body}");
        assert!(
            !body.contains("<script>alert"),
            "a comment's HTML is text: {body}"
        );
        let marker = body
            .find(r##"<a class="cmt" href="#t-"##)
            .expect("a marker");
        assert!(
            body[marker..].starts_with(r##"<a class="cmt" href="#t-"##)
                && body[marker..].contains("title=\"1 open thread\">💬 1</a>\n<p>Two.</p>"),
            "the marker is on the paragraph the thread is on: {body}"
        );
    }

    #[tokio::test]
    async fn a_page_without_comments_has_no_column() {
        let git = commented("On a.md");
        git.write("docs/b.md", "# B\n");
        git.commit("Another page");
        let body = Site::of(git.top().join("docs")).get("/docs/b.html").await.2;
        assert!(
            !body.contains("has-rail") && !body.contains("class=\"rail\""),
            "{body}"
        );
    }
}
