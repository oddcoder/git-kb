//! The API for AI reviewers, under `/_ai`, with its guide there and `/llms.txt` naming it: JSON in and out, to
//! read the pages and the threads on them, and to begin, reply to and resolve threads, each comment marked as an
//! AI's. See `ai.rs` for the guide, and how a quote is found on a page.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Server, View};
use crate::ai;
use crate::comments::{Begin, KINDS, MAX_BODY, Snippet, State as Placed, Thread};

/// A thread begun by an agent: on its quote, or else its lines.
#[derive(Deserialize)]
struct Start {
    #[serde(default)]
    agent: String,
    #[serde(default)]
    page: String,
    #[serde(default)]
    quote: Option<String>,
    #[serde(default)]
    lines: Option<Lines>,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    rev: Option<String>,
}

/// Lines of a page, from 1, as agents count them.
#[derive(Deserialize)]
struct Lines {
    first: usize,
    last: usize,
}

/// A reply, or a thread resolved, or reopened, by an agent.
#[derive(Deserialize)]
struct Reply {
    #[serde(default)]
    agent: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    resolved: Option<bool>,
}

impl Server {
    /// The ways agents write: a thread begun, and one replied to or resolved.
    pub(super) fn agent_routes() -> Router<Arc<Self>> {
        Router::new()
            .route(
                "/_ai/threads",
                post(
                    |State(server): State<Arc<Self>>,
                     headers: HeaderMap,
                     Json(start): Json<Start>| async move {
                        if !Self::agent_allowed(&headers) {
                            return StatusCode::FORBIDDEN.into_response();
                        }
                        tokio::task::spawn_blocking(move || server.agent_start(&start))
                            .await
                            .unwrap_or_else(|e| Self::broke(&e.into()))
                    },
                )
                .get(
                    |State(server): State<Arc<Self>>, uri: axum::http::Uri| async move {
                        let query = uri.query().unwrap_or("").to_owned();
                        tokio::task::spawn_blocking(move || {
                            server.agent_answer("/_ai/threads", &query)
                        })
                        .await
                        .unwrap_or_else(|e| Self::broke(&e.into()))
                    },
                ),
            )
            .route(
                "/_ai/threads/{id}/{what}",
                post(
                    |State(server): State<Arc<Self>>,
                     headers: HeaderMap,
                     UrlPath((id, what)): UrlPath<(String, String)>,
                     Json(reply): Json<Reply>| async move {
                        if !Self::agent_allowed(&headers) {
                            return StatusCode::FORBIDDEN.into_response();
                        }
                        tokio::task::spawn_blocking(move || server.agent_reply(&id, &what, &reply))
                            .await
                            .unwrap_or_else(|e| Self::broke(&e.into()))
                    },
                ),
            )
    }

    /// What an agent reads: the guide, the pages, a page, or the threads on one.
    pub(super) fn agent_answer(&self, path: &str, query: &str) -> Response {
        let text = |body: &'static str| {
            (
                [(header::CONTENT_TYPE, "text/markdown; charset=utf-8")],
                body,
            )
                .into_response()
        };
        match path {
            "/llms.txt" => text(concat!(
                "# Docs\n\n> A folder of Markdown docs, kept in git, with review comments.\n\n",
                "AI agents can read the pages and comment on them through a JSON API; every comment it takes is ",
                "marked as an AI's.\n\n- [The API for AI reviewers](/_ai): read this first.\n"
            )),
            "/_ai" => text(ai::GUIDE),
            "/_ai/pages" => self.agent_pages(),
            "/_ai/page" => self.agent_page(
                &Self::param(query, "path").unwrap_or_default(),
                Self::param(query, "rev").as_deref(),
            ),
            "/_ai/threads" => self.agent_threads(&Self::param(query, "page").unwrap_or_default()),
            _ => Self::agent_error(
                StatusCode::NOT_FOUND,
                json!({ "error": format!("nothing at {path}"), "hint": "see /_ai" }),
            ),
        }
    }

    /// Every page, with its title and when it last changed.
    fn agent_pages(&self) -> Response {
        let view = match self.view(None) {
            Ok(view) => view,
            Err(e) => return self.failed(&e),
        };
        let pages = view.pages();
        let listed: Vec<Value> = view
            .files
            .iter()
            .filter(|f| Self::is_markdown(f))
            .map(|f| {
                let last = view.history.as_ref().and_then(|h| h.of(f).next());
                json!({ "path": Self::agent_path(f), "title": pages.title(f),
                    "last_changed": last.map(|c| json!({ "commit": c.short, "author": c.author, "time": c.time,
                        "summary": c.summary })) })
            })
            .collect();
        Json(json!({ "pages": listed })).into_response()
    }

    /// A page for an agent: its source, its numbered lines and blocks, and the open threads on it.
    fn agent_page(&self, path: &str, rev: Option<&str>) -> Response {
        let snapshot = match rev.map(|name| self.snapshot(name)).transpose() {
            Ok(snapshot) => snapshot,
            Err(e) => {
                return Self::agent_error(
                    StatusCode::NOT_FOUND,
                    json!({ "error": format!("{e:#}") }),
                );
            }
        };
        let view = match self.view(snapshot) {
            Ok(view) => view,
            Err(e) => return self.failed(&e),
        };
        let Some((page, text)) = Self::agent_text(&view, path) else {
            return Self::no_page(path);
        };
        let lines: Vec<Value> = text
            .lines()
            .enumerate()
            .map(|(i, l)| json!({ "n": i + 1, "text": l }))
            .collect();
        let shown = view.snapshot.as_ref().or(view.head.as_ref());
        Json(json!({
            "path": Self::agent_path(&page), "title": view.pages().title(&page),
            "revision": view.snapshot.as_ref().map_or("working copy".to_string(), |s| s.revision.name.clone()),
            "commit": shown.map(|s| s.revision.id.to_string()),
            "source": text, "lines": lines, "blocks": ai::blocks(&text),
            "threads": self.agent_threads_on(&view, &page, &text, false),
        }))
        .into_response()
    }

    /// The threads on a page, open and resolved, with every comment.
    fn agent_threads(&self, path: &str) -> Response {
        let view = match self.view(None) {
            Ok(view) => view,
            Err(e) => return self.failed(&e),
        };
        let Some((page, text)) = Self::agent_text(&view, path) else {
            return Self::no_page(path);
        };
        Json(json!({ "page": Self::agent_path(&page),
            "threads": self.agent_threads_on(&view, &page, &text, true) }))
        .into_response()
    }

    /// The threads on a page as shown, with where each is now; resolved ones too, if asked.
    fn agent_threads_on(
        &self,
        view: &View,
        page: &Path,
        text: &str,
        resolved_too: bool,
    ) -> Vec<Value> {
        let Some(store) = &self.comments else {
            return vec![];
        };
        let Some(shown) = view.snapshot.as_ref().or(view.head.as_ref()) else {
            return vec![];
        };
        let on = match store.on_page(page, shown.revision.id, text, &self.renderer) {
            Ok(on) => on,
            Err(e) => {
                tracing::warn!(page = %page.display(), "comments not read: {e:#}");
                return vec![];
            }
        };
        let (found, lost) = (on.found, on.lost);
        let thread = |t: &Thread, at: Option<(&std::ops::Range<usize>, Placed)>| {
            json!({
                "id": t.id, "kind": if t.kind.is_empty() { Value::Null } else { t.kind.clone().into() },
                "resolved": t.resolved,
                "where": match at.map(|(_, s)| s) {
                    Some(Placed::Same) => "on the text it began on",
                    Some(Placed::Edited) => "on its text, which has changed since",
                    Some(Placed::Moved) => "on the paragraph most like its text, which is gone",
                    None => "nowhere: its text has gone from the page",
                },
                "lines": at.map(|(l, _)| json!({ "first": l.start + 1, "last": l.end })),
                "quote": if t.snippet.exact.is_empty() { &t.quote } else { &t.snippet.exact },
                "comments": t.comments.iter().map(|c| json!({
                    "by": if c.ai.is_some() { "ai" } else { "person" }, "author": c.author,
                    "agent": c.ai.as_ref().map(|a| a.agent.clone()), "for": c.ai.as_ref().map(|a| a.for_whom.clone()),
                    "time": c.time, "body": c.body })).collect::<Vec<_>>(),
            })
        };
        found
            .iter()
            .filter(|l| resolved_too || !l.thread.resolved)
            .map(|l| thread(&l.thread, Some((&l.lines, l.state))))
            .chain(
                lost.iter()
                    .filter(|t| resolved_too || !t.resolved)
                    .map(|t| thread(t, None)),
            )
            .collect()
    }

    /// Begins a thread for an agent, on its quote or its lines.
    fn agent_start(&self, start: &Start) -> Response {
        let agent = match Self::agent_name(&start.agent) {
            Ok(agent) => agent,
            Err(answer) => return *answer,
        };
        let kind = start.kind.trim().to_lowercase();
        if !KINDS.contains(&kind.as_str()) {
            return Self::agent_error(
                StatusCode::BAD_REQUEST,
                json!({ "error": format!("kind must be one of {}", KINDS.join(", ")) }),
            );
        }
        if let Err(answer) = Self::agent_body(&start.body) {
            return *answer;
        }
        let Some(store) = &self.comments else {
            return Self::agent_error(
                StatusCode::NOT_FOUND,
                json!({ "error": "not in a git repository" }),
            );
        };
        let rev = start.rev.as_deref().filter(|r| !r.is_empty());
        let view = match rev.map(|name| self.snapshot(name)).transpose() {
            Ok(snapshot) => match self.view(snapshot) {
                Ok(view) => view,
                Err(e) => return self.failed(&e),
            },
            Err(_) => {
                return Self::agent_error(
                    StatusCode::NOT_FOUND,
                    json!({ "error": "no such revision" }),
                );
            }
        };
        let Some(commit) = view
            .snapshot
            .as_ref()
            .or(view.head.as_ref())
            .map(|s| s.revision.id)
        else {
            return Self::agent_error(
                StatusCode::CONFLICT,
                json!({ "error": "nothing is committed yet" }),
            );
        };
        let Some((page, text)) = Self::agent_text(&view, &start.page) else {
            return Self::no_page(&start.page);
        };
        let (lines, [exact, prefix, suffix]) = match (&start.quote, &start.lines) {
            (Some(quote), _) if !quote.trim().is_empty() => match ai::find(&text, quote) {
                Ok(found) => (found.lines, [found.exact, found.prefix, found.suffix]),
                Err(why) => return Self::agent_error(StatusCode::UNPROCESSABLE_ENTITY, why),
            },
            (_, Some(l)) if l.first >= 1 && l.first <= l.last && l.last <= text.lines().count() => {
                (l.first - 1..l.last, Default::default())
            }
            _ => {
                return Self::agent_error(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "give a quote of the words, or lines on the page",
                        "hint": "\"quote\": \"exact words\", or \"lines\": {\"first\": 3, \"last\": 5}" }),
                );
            }
        };
        let shown = json!({ "first": lines.start + 1, "last": lines.end });
        let on = Begin {
            page: &page,
            commit,
            text: &text,
            lines,
            snippet: Snippet {
                exact: exact.clone(),
                prefix,
                suffix,
            },
        };
        let begun = store.begin_as(&on, start.body.trim(), Some((&agent, &kind)));
        match begun {
            Ok(id) => {
                let at = page.parent().unwrap_or(Path::new(""));
                let url = format!("{}#t-{id}", view.shell.link(at, &page));
                (
                    StatusCode::CREATED,
                    Json(json!({ "id": id, "page": Self::agent_path(&page), "kind": kind, "lines": shown,
                        "quote": if exact.is_empty() { Value::Null } else { exact.into() }, "url": url })),
                )
                    .into_response()
            }
            Err(e) => Self::agent_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("{e:#}") }),
            ),
        }
    }

    /// Replies to a thread for an agent, or resolves or reopens one an AI began.
    fn agent_reply(&self, id: &str, what: &str, reply: &Reply) -> Response {
        let agent = match Self::agent_name(&reply.agent) {
            Ok(agent) => agent,
            Err(answer) => return *answer,
        };
        let Some(store) = &self.comments else {
            return Self::agent_error(
                StatusCode::NOT_FOUND,
                json!({ "error": "not in a git repository" }),
            );
        };
        match what {
            "replies" => {
                if let Err(answer) = Self::agent_body(&reply.body) {
                    return *answer;
                }
                match store.reply_as(id, reply.body.trim(), Some(&agent)) {
                    Ok(()) => Json(json!({ "id": id, "replied": true })).into_response(),
                    Err(e) => Self::agent_error(
                        StatusCode::NOT_FOUND,
                        json!({ "error": format!("{e:#}") }),
                    ),
                }
            }
            "resolve" => {
                let resolved = reply.resolved.unwrap_or(true);
                match store.resolve_as(id, resolved, Some(&agent)) {
                    Ok(()) => Json(json!({ "id": id, "resolved": resolved })).into_response(),
                    Err(e) => Self::agent_error(
                        StatusCode::FORBIDDEN,
                        json!({ "error": format!("{e:#}") }),
                    ),
                }
            }
            _ => Self::agent_error(
                StatusCode::NOT_FOUND,
                json!({ "error": format!("nothing at /_ai/threads/{id}/{what}"), "hint": "see /_ai" }),
            ),
        }
    }

    /// A page of a view, by its path as an agent gives it, from the site's top, with or without a slash before
    /// it, and its text.
    fn agent_text(view: &View, path: &str) -> Option<(PathBuf, String)> {
        let page = PathBuf::from(path.trim_start_matches('/'));
        if !Self::is_markdown(&page) || !view.files.contains(&page) {
            return None;
        }
        let text = String::from_utf8_lossy(&view.read(&page).ok()?).into_owned();
        Some((page, text))
    }

    /// A page's path as agents are given it: from the site's top, after a slash.
    fn agent_path(page: &Path) -> String {
        format!("/{}", page.display())
    }

    fn no_page(path: &str) -> Response {
        Self::agent_error(
            StatusCode::NOT_FOUND,
            json!({ "error": format!("no page {path}"), "hint": "see /_ai/pages" }),
        )
    }

    /// An error for an agent: a status, and JSON saying what was wrong and what to do.
    fn agent_error(status: StatusCode, body: Value) -> Response {
        (status, Json(body)).into_response()
    }

    /// A browser on another site must not write through this API: it says so in Sec-Fetch-Site, as agents' clients
    /// do not. A JSON body also needs a preflight, which is never answered.
    fn agent_allowed(headers: &HeaderMap) -> bool {
        headers
            .get("sec-fetch-site")
            .and_then(|v| v.to_str().ok())
            .is_none_or(|site| site == "same-origin" || site == "none")
    }

    /// An agent's name, as people will see it: given, and of a sensible length.
    fn agent_name(agent: &str) -> Result<String, Box<Response>> {
        let agent = agent.trim();
        if agent.is_empty() || agent.chars().count() > 80 {
            return Err(Box::new(Self::agent_error(
                StatusCode::BAD_REQUEST,
                json!({ "error": "agent is required: who you are, up to 80 characters",
                    "hint": "such as \"Claude (claude-opus-5-5)\"" }),
            )));
        }
        Ok(agent.to_string())
    }

    /// A comment's body, given, and not too long.
    fn agent_body(body: &str) -> Result<(), Box<Response>> {
        if body.trim().is_empty() || body.len() > MAX_BODY {
            return Err(Box::new(Self::agent_error(
                StatusCode::BAD_REQUEST,
                json!({ "error": format!("body is required, up to {MAX_BODY} bytes") }),
            )));
        }
        Ok(())
    }
}
