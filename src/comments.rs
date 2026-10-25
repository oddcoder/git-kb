//! Review comments: threads on a page's lines, kept in the repository under refs of their own, one for each
//! person, `<namespace>/<their email>`, by default `refs/git-kb/comments/ada@example.com`. They are written
//! straight into git's object store, never into the working tree, the index or any branch, so the code and the
//! docs are untouched. A person writes only their own ref: a reply to someone else's thread goes in the replier's,
//! under the thread's id, and the threads shown are every ref's put together.
//!
//! Nothing in a ref is ever changed, only added, so that two histories of one can always be put together: every
//! file of each, as no two ever write the same file differently. A ref's tree holds
//!
//! ```text
//! threads/<id>/thread.json             what the thread is on: the page, commit, lines and words it began on
//! threads/<id>/comments/<stamp>.json   each comment
//! threads/<id>/state/<stamp>.json      each resolve or reopen; the last says whether the thread is open
//! texts/<blob>                         the page's source when a thread began, which may hold edits not committed
//! files/<blob>.<ext>                   images in comments, pasted or dropped into them
//! ```
//!
//! A stamp is when it was written, then a count and a random part, so that stamps sort in the order they were
//! written, and no two people's are the same.
//!
//! Sync fetches everyone's refs from each remote and pushes only one's own, with git itself, so that its
//! credentials are used. As each ref has one writer, pushes do not collide; only the same person writing from two
//! clones can, and a push so refused is fetched, merged and pushed again. A ref is only ever moved forward here:
//! one that has gone another way is merged with what is here, every file of each, so nothing seen is lost.

use std::collections::BTreeMap;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use anyhow::{Context, anyhow};
use gix::ObjectId;
use gix::object::tree::diff::ChangeDetached;
use gix::objs::tree::EntryKind;
use gix::refs::transaction::PreviousValue;
use gix::traverse::tree::Recorder;
use serde::{Deserialize, Serialize};
use similar::{Algorithm, DiffTag, capture_diff_slices};

use crate::diff::Diff;
use crate::git::{Repo, When};
use crate::render::Renderer;

/// The longest comment taken, in bytes.
pub const MAX_BODY: usize = 20_000;

/// The largest image taken, in bytes.
pub const MAX_IMAGE: usize = 5_000_000;

/// The kinds of image taken, by the ending of their names and their types. Not SVG, which can carry scripts.
pub const IMAGES: [(&str, &str); 4] = [
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
];

/// What a thread an AI begins raises, as it says: a concern, a question, a suggestion or a note. People's threads
/// have none.
pub const KINDS: [&str; 4] = ["concern", "question", "suggestion", "note"];

/// A thread: where it began, its comments, oldest first, and whether it is resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thread {
    pub id: String,
    /// The page it began on, from the site's top, as `arch/overview.md`.
    pub page: PathBuf,
    /// The commit it began on.
    pub commit: ObjectId,
    /// The page's source when it began, as a blob, which may hold edits not committed.
    pub text: ObjectId,
    /// The lines it began on, from 0, and their text.
    pub lines: Range<usize>,
    pub quote: String,
    /// For a thread on chosen words: the words, and those just before and after them, to find them again among
    /// others like them. Empty for a thread on whole lines.
    pub snippet: Snippet,
    /// One of `KINDS`, for a thread an AI began; empty for a person's.
    pub kind: String,
    pub resolved: bool,
    pub comments: Vec<Comment>,
}

/// Chosen words: the words themselves, and some of those just before and after them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snippet {
    #[serde(default)]
    pub exact: String,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub suffix: String,
}

/// A comment, as kept in `threads/<id>/comments/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    pub author: String,
    pub email: String,
    /// When it was written, in seconds since 1970.
    pub time: i64,
    pub body: String,
    /// Set when an AI wrote it, through `/_ai`; never otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai: Option<Ai>,
}

/// Who an AI comment is from: the agent, as it names itself, and the person, by their git name, whose server it
/// wrote through.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ai {
    pub agent: String,
    #[serde(default)]
    pub for_whom: String,
}

/// Where a thread begins: on lines of a page's source, as it is at a commit or with edits not committed yet.
#[derive(Debug, Clone)]
pub struct Begin<'a> {
    pub page: &'a Path,
    pub commit: ObjectId,
    pub text: &'a str,
    pub lines: Range<usize>,
    pub snippet: Snippet,
}

/// What a thread is on, as kept in `threads/<id>/thread.json`.
#[derive(Serialize, Deserialize)]
struct Anchor {
    id: String,
    page: String,
    commit: String,
    text: String,
    lines: [usize; 2],
    quote: String,
    #[serde(flatten)]
    snippet: Snippet,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    kind: String,
}

/// A resolve or a reopen, as kept in `threads/<id>/state/`.
#[derive(Serialize, Deserialize)]
struct Resolve {
    author: String,
    email: String,
    time: i64,
    resolved: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ai: Option<Ai>,
}

/// Where a thread is on a page as it is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// On the lines it began on, unchanged.
    Same,
    /// On those lines, which have changed since.
    Edited,
    /// On the paragraph most like the one it began on, its lines themselves gone.
    Moved,
}

/// A thread found on a page: on which of its lines now, and how.
#[derive(Debug, Clone)]
pub struct Located {
    pub thread: Thread,
    pub lines: Range<usize>,
    pub state: State,
}

/// The threads on a page as it is shown: those found on it, and those of its history whose text is gone from it.
#[derive(Debug, Default)]
pub struct OnPage {
    pub found: Vec<Located>,
    pub lost: Vec<Thread>,
}

/// What a sync did: when, how many comments it brought, and what it says it did, or why it could not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Synced {
    pub time: i64,
    pub brought: usize,
    pub said: String,
}

/// The comments kept in a repository.
pub struct Store {
    git: gix::ThreadSafeRepository,
    /// The site's folder in the repository, as `docs`, or empty when it is the repository's top.
    prefix: PathBuf,
    /// Where everyone's refs are, such as `refs/git-kb/comments`.
    namespace: String,
    /// Writes are one at a time, so that two at once both land on one's ref.
    writing: Mutex<()>,
    /// What the last sync did.
    synced: Mutex<Option<Synced>>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("namespace", &self.namespace)
            .finish_non_exhaustive()
    }
}

impl Store {
    /// The comments kept in a repository, under refs named from `namespace`.
    #[must_use]
    pub fn new(repo: &Repo, namespace: &str) -> Self {
        Self {
            git: repo.git().clone(),
            prefix: repo.prefix().to_path_buf(),
            namespace: namespace.trim_end_matches('/').to_string(),
            writing: Mutex::default(),
            synced: Mutex::default(),
        }
    }

    /// Who writes here: git's `user.name` and `user.email`, or else someone with no email.
    #[must_use]
    pub fn me(&self) -> (String, String) {
        let repo = self.git.to_thread_local();
        match repo.committer().and_then(Result::ok) {
            Some(who) => (who.name.to_string(), who.email.to_string()),
            None => ("Someone".into(), String::new()),
        }
    }

    /// The ref this person writes: by their email, or their name when they have none.
    #[must_use]
    pub fn mine(&self) -> String {
        let (name, email) = self.me();
        let who = if email.is_empty() { name } else { email };
        format!("{}/{}", self.namespace, ref_name(&who))
    }

    /// Every thread, by when it began, oldest first: everyone's refs put together.
    ///
    /// # Errors
    ///
    /// When the refs or what they hold cannot be read.
    pub fn threads(&self) -> anyhow::Result<Vec<Thread>> {
        let repo = self.git.to_thread_local();
        let mut parts: BTreeMap<String, Parts> = BTreeMap::new();
        for tip in self.tips(&repo)? {
            let tree = repo.find_commit(tip)?.tree()?;
            let Some(threads) = subtree(&tree, "threads")? else {
                continue;
            };
            for entry in threads.iter() {
                let entry = entry?;
                let Ok(thread) = entry.object()?.try_into_tree() else {
                    continue;
                };
                parts
                    .entry(entry.filename().to_string())
                    .or_default()
                    .add(&thread)?;
            }
        }
        let mut threads: Vec<Thread> = parts.into_values().filter_map(Parts::thread).collect();
        threads.sort_by_key(|t| t.comments.first().map_or(0, |c| c.time));
        Ok(threads)
    }

    /// A thread, by its id.
    ///
    /// # Errors
    ///
    /// When the threads cannot be read.
    pub fn thread(&self, id: &str) -> anyhow::Result<Option<Thread>> {
        Ok(self.threads()?.into_iter().find(|t| t.id == id))
    }

    /// Begins a thread, with its first comment, and gives its id.
    ///
    /// # Errors
    ///
    /// When the comment is empty or too long, or the repository cannot be written.
    pub fn begin(&self, on: &Begin, body: &str) -> anyhow::Result<String> {
        self.begin_as(on, body, None)
    }

    /// Begins a thread, by whoever writes here, or, given `(agent, kind)`, by that AI, for them, raising that kind
    /// of point; and gives its id.
    ///
    /// # Errors
    ///
    /// When the comment is empty or too long, the kind is not one of `KINDS`, or the repository cannot be written.
    pub fn begin_as(
        &self,
        on: &Begin,
        body: &str,
        ai: Option<(&str, &str)>,
    ) -> anyhow::Result<String> {
        let body = Self::body(body)?;
        if let Some((_, kind)) = ai {
            anyhow::ensure!(
                KINDS.contains(&kind),
                "a kind is one of {}",
                KINDS.join(", ")
            );
        }
        let repo = self.git.to_thread_local();
        let lines: Vec<&str> = on.text.lines().collect();
        let start = on.lines.start.min(lines.len());
        let end = on.lines.end.clamp(start, lines.len());
        let text = repo.write_blob(on.text)?.detach();
        let id = stamp();
        let anchor = Anchor {
            id: id.clone(),
            page: on.page.to_string_lossy().into_owned(),
            commit: on.commit.to_string(),
            text: text.to_string(),
            lines: [start, end],
            quote: lines[start..end].join("\n"),
            snippet: on.snippet.clone(),
            kind: ai.map(|(_, kind)| kind.to_string()).unwrap_or_default(),
        };
        let files = [
            (format!("texts/{text}"), text),
            (format!("threads/{id}/thread.json"), json(&repo, &anchor)?),
            (
                format!("threads/{id}/comments/{}.json", stamp()),
                json(&repo, &self.comment(body, ai.map(|(agent, _)| agent)))?,
            ),
        ];
        let by = ai
            .map(|(agent, kind)| format!(" (AI: {agent}, {kind})"))
            .unwrap_or_default();
        let message = format!(
            "Comment on {}{by}: {}",
            on.page.display(),
            first_words(body)
        );
        self.commit(&repo, &files, &message)?;
        Ok(id)
    }

    /// Replies to a thread.
    ///
    /// # Errors
    ///
    /// When there is no such thread, the reply is empty or too long, or the repository cannot be written.
    pub fn reply(&self, id: &str, body: &str) -> anyhow::Result<()> {
        self.reply_as(id, body, None)
    }

    /// Replies to a thread, as whoever writes here, or, given an agent, as that AI, for them.
    ///
    /// # Errors
    ///
    /// When there is no such thread, the reply is empty or too long, or the repository cannot be written.
    pub fn reply_as(&self, id: &str, body: &str, agent: Option<&str>) -> anyhow::Result<()> {
        let body = Self::body(body)?;
        let thread = self.thread(id)?.ok_or_else(|| anyhow!("no thread {id}"))?;
        let repo = self.git.to_thread_local();
        let file = (
            format!("threads/{id}/comments/{}.json", stamp()),
            json(&repo, &self.comment(body, agent))?,
        );
        let by = agent
            .map(|agent| format!(" (AI: {agent})"))
            .unwrap_or_default();
        let message = format!(
            "Reply on {}{by}: {}",
            thread.page.display(),
            first_words(body)
        );
        self.commit(&repo, &[file], &message)
    }

    /// Resolves a thread, or reopens it.
    ///
    /// # Errors
    ///
    /// When there is no such thread, or the repository cannot be written.
    pub fn resolve(&self, id: &str, resolved: bool) -> anyhow::Result<()> {
        self.resolve_as(id, resolved, None)
    }

    /// Resolves or reopens a thread, as whoever writes here, or, given an agent, as that AI: an AI only a thread an
    /// AI began, never a person's.
    ///
    /// # Errors
    ///
    /// When there is no such thread, an AI's would be a person's, or the repository cannot be written.
    pub fn resolve_as(&self, id: &str, resolved: bool, agent: Option<&str>) -> anyhow::Result<()> {
        let thread = self.thread(id)?.ok_or_else(|| anyhow!("no thread {id}"))?;
        anyhow::ensure!(
            agent.is_none() || thread.comments.first().is_some_and(|c| c.ai.is_some()),
            "an AI may resolve or reopen only threads an AI began; this one a person began"
        );
        let repo = self.git.to_thread_local();
        let (name, email) = self.me();
        let event = match agent {
            None => Resolve {
                author: name,
                email,
                time: When::now().0,
                resolved,
                ai: None,
            },
            Some(agent) => Resolve {
                author: agent.to_string(),
                email: String::new(),
                time: When::now().0,
                resolved,
                ai: Some(Ai {
                    agent: agent.to_string(),
                    for_whom: name,
                }),
            },
        };
        let file = (
            format!("threads/{id}/state/{}.json", stamp()),
            json(&repo, &event)?,
        );
        let what = if resolved { "Resolve" } else { "Reopen" };
        let by = agent
            .map(|agent| format!(" (AI: {agent})"))
            .unwrap_or_default();
        let message = format!("{what} a thread on {}{by}", thread.page.display());
        self.commit(&repo, &[file], &message)
    }

    /// The threads on a page as it is shown at a commit, with its text now, which may hold edits not committed.
    ///
    /// A thread is on the page its page has become since it began, through renames. The commit it began on is a
    /// hint, not a must: a rebase or a squash leaves it out of the history shown, and a colleague's may never have
    /// been here. So a thread from the history shown is found wherever its text has gone, and is lost when its
    /// text is gone; one from outside it, as on a branch not merged, only where its text is plainly here.
    ///
    /// # Errors
    ///
    /// When the threads, or the commit shown, cannot be read.
    pub fn on_page(
        &self,
        page: &Path,
        at: ObjectId,
        text: &str,
        renderer: &Renderer,
    ) -> anyhow::Result<OnPage> {
        let repo = self.git.to_thread_local();
        let mut on = OnPage::default();
        for thread in self.threads()? {
            let known = repo.find_commit(thread.commit).is_ok();
            let ours = known && is_ancestor(&repo, thread.commit, at);
            let now_at = if known {
                self.path_at(&repo, &thread, at)?
            } else {
                Some(thread.page.clone())
            };
            if now_at.as_deref() != Some(page) {
                continue;
            }
            // Its copy of the text it began on may not have been fetched.
            let Ok(then) = repo.find_blob(thread.text) else {
                continue;
            };
            let then = String::from_utf8_lossy(&then.data).into_owned();
            match place(&thread, &then, text, renderer) {
                Some((lines, state)) if ours || plainly_here(&thread, text, &lines, state) => {
                    on.found.push(Located {
                        thread,
                        lines,
                        state,
                    });
                }
                None if ours => on.lost.push(thread),
                _ => {}
            }
        }
        Ok(on)
    }

    /// Where a thread's page is at a commit, from the site's top: where it began, if it is still there; or else
    /// where git finds it was renamed to; or none, when it is gone.
    fn path_at(
        &self,
        repo: &gix::Repository,
        thread: &Thread,
        at: ObjectId,
    ) -> anyhow::Result<Option<PathBuf>> {
        let full = self.prefix.join(&thread.page);
        let now = repo.find_commit(at)?.tree()?;
        if thread.commit == at || now.lookup_entry_by_path(&full)?.is_some() {
            return Ok(Some(thread.page.clone()));
        }
        let then = repo.find_commit(thread.commit)?.tree()?;
        let mut options = gix::diff::Options::default();
        options
            .track_path()
            .track_rewrites(Some(gix::diff::Rewrites::default()));
        let renamed = repo
            .diff_tree_to_tree(&then, &now, options)?
            .into_iter()
            .find_map(|change| match change {
                ChangeDetached::Rewrite {
                    source_location,
                    location,
                    copy: false,
                    ..
                } if Path::new(&source_location.to_string()) == full => {
                    Some(PathBuf::from(location.to_string()))
                }
                _ => None,
            });
        Ok(renamed.and_then(|path| path.strip_prefix(&self.prefix).ok().map(Path::to_path_buf)))
    }

    /// The tips of everyone's refs.
    fn tips(&self, repo: &gix::Repository) -> anyhow::Result<Vec<ObjectId>> {
        let references = repo.references()?;
        let mut tips = vec![];
        for reference in references.prefixed(format!("{}/", self.namespace).as_str())? {
            let mut reference = reference.map_err(|e| anyhow!(e))?;
            tips.push(reference.peel_to_id()?.detach());
        }
        Ok(tips)
    }

    /// Keeps an image with the comments, by its content, so that the same image is kept once, and gives its name
    /// under `files/`, as `<blob>.png`, for a comment to show it by.
    ///
    /// # Errors
    ///
    /// When it is empty, too large or not of a kind taken, or the repository cannot be written.
    pub fn image(&self, bytes: &[u8], kind: &str) -> anyhow::Result<String> {
        let ending = IMAGES
            .iter()
            .find(|(_, k)| *k == kind)
            .map(|(e, _)| *e)
            .ok_or_else(|| anyhow!("an image is one of {}", IMAGES.map(|(_, k)| k).join(", ")))?;
        anyhow::ensure!(!bytes.is_empty(), "the image is empty");
        anyhow::ensure!(
            bytes.len() <= MAX_IMAGE,
            "an image is at most {} MB",
            MAX_IMAGE / 1_000_000
        );
        let repo = self.git.to_thread_local();
        let blob = repo.write_blob(bytes)?.detach();
        let name = format!("{blob}.{ending}");
        self.commit(&repo, &[(format!("files/{name}"), blob)], "Keep an image")?;
        Ok(name)
    }

    /// An image kept with the comments, by its name under `files/`, from whoever's ref has it.
    ///
    /// # Errors
    ///
    /// When the refs cannot be read.
    pub fn file(&self, name: &str) -> anyhow::Result<Option<Vec<u8>>> {
        if name.contains('/') {
            return Ok(None);
        }
        let repo = self.git.to_thread_local();
        for tip in self.tips(&repo)? {
            let tree = repo.find_commit(tip)?.tree()?;
            if let Some(entry) = tree.lookup_entry_by_path(format!("files/{name}"))? {
                return Ok(Some(entry.object()?.detach().data));
            }
        }
        Ok(None)
    }

    /// A comment's text, trimmed, if it is one to take.
    fn body(body: &str) -> anyhow::Result<&str> {
        let body = body.trim();
        anyhow::ensure!(!body.is_empty(), "a comment needs some text");
        anyhow::ensure!(
            body.len() <= MAX_BODY,
            "a comment can be at most {MAX_BODY} bytes"
        );
        Ok(body)
    }

    /// A comment by whoever writes here, now; or, given an agent, by that AI, for them.
    fn comment(&self, body: &str, agent: Option<&str>) -> Comment {
        let (name, email) = self.me();
        match agent {
            None => Comment {
                author: name,
                email,
                time: When::now().0,
                body: body.to_string(),
                ai: None,
            },
            Some(agent) => Comment {
                author: agent.to_string(),
                email: String::new(),
                time: When::now().0,
                body: body.to_string(),
                ai: Some(Ai {
                    agent: agent.to_string(),
                    for_whom: name,
                }),
            },
        }
    }

    /// One commit on one's own ref: its tree as it was, with these files added. It is refused if the ref has
    /// moved since it was read.
    fn commit(
        &self,
        repo: &gix::Repository,
        files: &[(String, ObjectId)],
        message: &str,
    ) -> anyhow::Result<()> {
        let _writing = self.writing.lock().unwrap_or_else(PoisonError::into_inner);
        let mine = self.mine();
        let parent = tip(repo, &mine)?;
        let base = match parent {
            Some(parent) => repo.find_commit(parent)?.tree_id()?.detach(),
            None => ObjectId::empty_tree(repo.object_hash()),
        };
        let mut tree = repo.edit_tree(base)?;
        for (path, blob) in files {
            tree.upsert(path.as_str(), EntryKind::Blob, *blob)?;
        }
        let tree = tree.write()?.detach();
        self.write(repo, &mine, message, tree, parent)
    }

    /// A commit of a tree on a ref, by whoever writes here, refused if the ref is not at the first parent.
    fn write(
        &self,
        repo: &gix::Repository,
        name: &str,
        message: &str,
        tree: ObjectId,
        parents: impl IntoIterator<Item = ObjectId>,
    ) -> anyhow::Result<()> {
        let (who, email) = self.me();
        let signature = gix::actor::Signature {
            name: who.into(),
            email: email.into(),
            time: gix::date::Time::now_local_or_utc(),
        };
        let mut time = gix::date::parse::TimeBuf::default();
        let signature = signature.to_ref(&mut time);
        repo.commit_as(signature, signature, name, message, tree, parents)
            .with_context(|| format!("writing {name}"))?;
        Ok(())
    }

    /// Pulls everyone's comments from each remote and shares one's own, and says what it did; what it did is
    /// kept, to show, failures too.
    ///
    /// # Errors
    ///
    /// When the repository has no remotes, or git cannot be run or fails.
    pub fn sync(&self) -> anyhow::Result<Synced> {
        let result = self.sync_remotes();
        let synced = match &result {
            Ok(synced) => synced.clone(),
            Err(e) => Synced {
                time: When::now().0,
                brought: 0,
                said: format!("{e:#}"),
            },
        };
        *self.synced.lock().unwrap_or_else(PoisonError::into_inner) = Some(synced);
        result
    }

    /// What the last sync did, if there has been one.
    #[must_use]
    pub fn last_sync(&self) -> Option<Synced> {
        self.synced
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn sync_remotes(&self) -> anyhow::Result<Synced> {
        let place = self
            .git
            .work_dir()
            .unwrap_or_else(|| self.git.git_dir())
            .to_path_buf();
        // Never waiting on a prompt: no password asked on a terminal, and ssh in batch mode, unless set otherwise.
        let ssh =
            std::env::var("GIT_SSH_COMMAND").unwrap_or_else(|_| "ssh -o BatchMode=yes".into());
        let git = |args: &[&str]| -> anyhow::Result<std::process::Output> {
            Command::new("git")
                .arg("-C")
                .arg(&place)
                .args(args)
                .env("GIT_TERMINAL_PROMPT", "0")
                .env("GIT_SSH_COMMAND", &ssh)
                .output()
                .context("running git")
        };
        let remotes = String::from_utf8(git(&["remote"])?.stdout)?;
        let remotes: Vec<&str> = remotes.lines().filter(|r| !r.is_empty()).collect();
        anyhow::ensure!(
            !remotes.is_empty(),
            "this repository has no remotes to sync with"
        );
        let mine = self.mine();
        let (mut brought, mut said) = (0, vec![]);
        for remote in remotes {
            let (got, what) = self.sync_remote(remote, &mine, &git)?;
            brought += got;
            said.push(what);
        }
        Ok(Synced {
            time: When::now().0,
            brought,
            said: said.join("; "),
        })
    }

    /// Syncs with one remote: fetches everyone's refs, takes each in, and pushes one's own; again, if the push is
    /// refused because the same person pushed from elsewhere meanwhile. Gives how many comments it brought, and
    /// what it did.
    fn sync_remote(
        &self,
        remote: &str,
        mine: &str,
        git: &dyn Fn(&[&str]) -> anyhow::Result<std::process::Output>,
    ) -> anyhow::Result<(usize, String)> {
        let last_line = |bytes: &[u8]| {
            String::from_utf8_lossy(bytes)
                .trim()
                .lines()
                .last()
                .unwrap_or("")
                .to_string()
        };
        // The remote's refs are kept apart, then each taken into the one here.
        let tracking = format!("refs/git-kb/remotes/{remote}");
        let spec = format!("+{}/*:{tracking}/*", self.namespace);
        let mut brought = 0;
        for _ in 0..3 {
            let fetched = git(&["fetch", "--quiet", "--prune", remote, &spec])?;
            anyhow::ensure!(
                fetched.status.success(),
                "couldn't fetch from {remote}: {}",
                last_line(&fetched.stderr)
            );
            let repo = self.git.to_thread_local();
            let theirs: Vec<(String, ObjectId)> = {
                let references = repo.references()?;
                let mut out = vec![];
                for reference in references.prefixed(format!("{tracking}/").as_str())? {
                    let mut reference = reference.map_err(|e| anyhow!(e))?;
                    let name = reference.name().as_bstr().to_string();
                    out.push((name, reference.peel_to_id()?.detach()));
                }
                out
            };
            for (name, id) in theirs {
                let here = format!("{}/{}", self.namespace, &name[tracking.len() + 1..]);
                brought += self.merge(&repo, &here, id, remote)?;
            }
            let Some(ours) = tip(&repo, mine)? else {
                return Ok((brought, format!("{}from {remote}", got(brought))));
            };
            let theirs = tip(
                &repo,
                &format!("{tracking}/{}", &mine[self.namespace.len() + 1..]),
            )?;
            if theirs == Some(ours) {
                return Ok((brought, format!("{}up to date with {remote}", got(brought))));
            }
            let pushed = git(&["push", "--quiet", remote, &format!("{mine}:{mine}")])?;
            if pushed.status.success() {
                return Ok((
                    brought,
                    format!("{}shared ours with {remote}", got(brought)),
                ));
            }
            let why = String::from_utf8_lossy(&pushed.stderr);
            let raced = ["fetch first", "non-fast-forward", "rejected"]
                .iter()
                .any(|r| why.contains(r));
            anyhow::ensure!(
                raced,
                "couldn't push to {remote}: {}",
                last_line(&pushed.stderr)
            );
        }
        anyhow::bail!("couldn't push to {remote}: it kept moving")
    }

    /// Takes another history of a ref into the one here: by moving it forward, when it is behind, or else by a
    /// commit of both, with every file of each. Gives how many comments it brought.
    fn merge(
        &self,
        repo: &gix::Repository,
        name: &str,
        theirs: ObjectId,
        from: &str,
    ) -> anyhow::Result<usize> {
        let _writing = self.writing.lock().unwrap_or_else(PoisonError::into_inner);
        let ours = tip(repo, name)?;
        let before = ours.map_or(Ok(0), |ours| comments_at(repo, ours))?;
        match ours {
            Some(ours) if ours == theirs || is_ancestor(repo, theirs, ours) => return Ok(0),
            Some(ours) if is_ancestor(repo, ours, theirs) => {
                let expected = PreviousValue::MustExistAndMatch(gix::refs::Target::Object(ours));
                repo.reference(name, theirs, expected, format!("comments: from {from}"))?;
            }
            Some(ours) => {
                let tree = union(
                    repo,
                    repo.find_commit(ours)?.tree_id()?.detach(),
                    repo.find_commit(theirs)?.tree_id()?.detach(),
                )?;
                self.write(
                    repo,
                    name,
                    &format!("Merge comments from {from}"),
                    tree,
                    [ours, theirs],
                )?;
            }
            None => {
                repo.reference(
                    name,
                    theirs,
                    PreviousValue::MustNotExist,
                    format!("comments: from {from}"),
                )?;
            }
        }
        let after = comments_at(repo, tip(repo, name)?.unwrap_or(theirs))?;
        Ok(after.saturating_sub(before))
    }
}

/// Where a ref is, if it is.
fn tip(repo: &gix::Repository, name: &str) -> anyhow::Result<Option<ObjectId>> {
    Ok(match repo.try_find_reference(name)? {
        Some(mut reference) => Some(reference.peel_to_id()?.detach()),
        None => None,
    })
}

/// How many comments a ref's commit holds, its own and replies to others'.
fn comments_at(repo: &gix::Repository, commit: ObjectId) -> anyhow::Result<usize> {
    let mut files = Recorder::default();
    repo.find_commit(commit)?
        .tree()?
        .traverse()
        .breadthfirst(&mut files)?;
    Ok(files
        .records
        .iter()
        .filter(|f| f.mode.is_blob() && f.filepath.to_string().contains("/comments/"))
        .count())
}

/// Two trees as one, with every file of each. Files are only ever added, so a file in both is the same; were it
/// ever not so, theirs is kept beside ours rather than lost.
fn union(repo: &gix::Repository, ours: ObjectId, theirs: ObjectId) -> anyhow::Result<ObjectId> {
    let mut files = Recorder::default();
    repo.find_tree(theirs)?
        .traverse()
        .breadthfirst(&mut files)?;
    let mine = repo.find_tree(ours)?;
    let mut tree = repo.edit_tree(ours)?;
    for file in files.records.iter().filter(|f| f.mode.is_blob()) {
        let path = file.filepath.to_string();
        match mine.lookup_entry_by_path(&path)? {
            None => {
                tree.upsert(path.as_str(), EntryKind::Blob, file.oid)?;
            }
            Some(entry) if entry.oid() == file.oid => {}
            Some(_) => {
                let beside = format!("{path}.theirs-{}", file.oid.to_hex_with_len(8));
                tree.upsert(beside.as_str(), EntryKind::Blob, file.oid)?;
            }
        }
    }
    Ok(tree.write()?.detach())
}

/// What a sync brought, as the start of what it says.
fn got(brought: usize) -> String {
    match brought {
        0 => String::new(),
        1 => "1 new comment ".into(),
        n => format!("{n} new comments "),
    }
}

/// Whether a commit is in the history of another, or is it.
fn is_ancestor(repo: &gix::Repository, ancestor: ObjectId, of: ObjectId) -> bool {
    ancestor == of
        || repo
            .merge_base(ancestor, of)
            .is_ok_and(|base| base.is_some_and(|base| base.detach() == ancestor))
}

/// Whether a thread from outside the history shown is plainly on this page: its lines unchanged, or still most of
/// what they were.
fn plainly_here(thread: &Thread, text: &str, lines: &Range<usize>, state: State) -> bool {
    let now: Vec<&str> = text.lines().collect();
    let here = now
        .get(lines.clone())
        .map(|l| l.join("\n"))
        .unwrap_or_default();
    state == State::Same || Diff::similarity(&thread.quote, &here) >= 0.6
}

/// Where a thread's lines are in a page's text now, from the text it began on: followed through the edits since,
/// or, when they are gone, the block most like the one it began on, if any is like enough.
fn place(
    thread: &Thread,
    then: &str,
    now: &str,
    renderer: &Renderer,
) -> Option<(Range<usize>, State)> {
    let (a, b): (Vec<&str>, Vec<&str>) = (then.lines().collect(), now.lines().collect());
    let (start, end) = (thread.lines.start, thread.lines.end);
    let (mut new_start, mut new_end, mut changed) = (None, None, false);
    for op in capture_diff_slices(Algorithm::Patience, &a, &b) {
        let (tag, old, new) = op.as_tag_tuple();
        if tag != DiffTag::Equal && old.start < end && start < old.end.max(old.start + 1) {
            changed = true;
        }
        // Only the change that holds a line moves it: lines put in just before it are not where it went.
        if old.contains(&start) {
            new_start = Some(if tag == DiffTag::Equal {
                new.start + (start - old.start)
            } else {
                new.start
            });
        }
        let last = end.saturating_sub(1);
        if old.contains(&last) {
            new_end = Some(if tag == DiffTag::Equal {
                new.start + (last - old.start) + 1
            } else {
                new.end
            });
        }
    }
    // Each block's lines now, and how like the thread's lines each is.
    let starts: Vec<usize> = std::iter::once(0)
        .chain(now.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let line = |offset: usize| starts.partition_point(|&s| s <= offset).saturating_sub(1);
    let blocks: Vec<(f64, Range<usize>)> = renderer
        .block_ranges(now)
        .into_iter()
        .map(|r| {
            let lines = line(r.start)..line(r.end.saturating_sub(1).max(r.start)) + 1;
            (Diff::similarity(&thread.quote, &now[r]), lines)
        })
        .collect();
    let best = |within: &dyn Fn(&Range<usize>) -> bool, enough: f64| {
        blocks
            .iter()
            .filter(|(likeness, lines)| *likeness >= enough && within(lines))
            .max_by(|x, y| x.0.total_cmp(&y.0))
            .map(|(_, lines)| lines.clone())
    };
    if let (Some(s), Some(e)) = (new_start, new_end)
        && s < e
    {
        if !changed {
            return Some((s..e, State::Same));
        }
        // An edit can take in new paragraphs beside the lines: of the blocks they went to, the one most like them.
        if let Some(lines) = best(&|l| l.start < e && s < l.end, 0.4) {
            return Some((lines, State::Edited));
        }
    }
    best(&|_| true, 0.5).map(|lines| (lines, State::Moved))
}

/// A thread's files, from every ref that holds them, put together: its anchor from wherever it is, and its
/// comments and resolves from all, each once, by its file's name.
#[derive(Default)]
struct Parts {
    anchor: Option<Anchor>,
    comments: BTreeMap<String, Comment>,
    resolves: BTreeMap<String, Resolve>,
}

impl Parts {
    /// What one ref's tree of a thread adds.
    fn add(&mut self, thread: &gix::Tree) -> anyhow::Result<()> {
        if self.anchor.is_none()
            && let Some(entry) = thread.find_entry("thread.json")
        {
            self.anchor = serde_json::from_slice(&entry.object()?.data).ok();
        }
        for (folder, comments) in [("comments", true), ("state", false)] {
            let Some(files) = subtree(thread, folder)? else {
                continue;
            };
            for file in files.iter() {
                let file = file?;
                let (name, data) = (file.filename().to_string(), file.object()?.detach().data);
                // A file that does not read is someone else's newer kind, left for them.
                if comments {
                    if let Ok(comment) = serde_json::from_slice(&data) {
                        self.comments.entry(name).or_insert(comment);
                    }
                } else if let Ok(resolve) = serde_json::from_slice(&data) {
                    self.resolves.entry(name).or_insert(resolve);
                }
            }
        }
        Ok(())
    }

    /// The thread, if its anchor is here: its comments, and whether its last resolve resolved it, each in the
    /// order its stamp says it was written.
    fn thread(self) -> Option<Thread> {
        let anchor = self.anchor?;
        let [start, end] = anchor.lines;
        Some(Thread {
            id: anchor.id,
            page: PathBuf::from(anchor.page),
            commit: ObjectId::from_hex(anchor.commit.as_bytes()).ok()?,
            text: ObjectId::from_hex(anchor.text.as_bytes()).ok()?,
            lines: start..end.max(start),
            quote: anchor.quote,
            snippet: anchor.snippet,
            kind: anchor.kind,
            resolved: self
                .resolves
                .into_values()
                .last()
                .is_some_and(|r| r.resolved),
            comments: self.comments.into_values().collect(),
        })
    }
}

/// A tree's subtree by name, if it has one.
fn subtree<'repo>(tree: &gix::Tree<'repo>, name: &str) -> anyhow::Result<Option<gix::Tree<'repo>>> {
    match tree.find_entry(name) {
        Some(entry) => Ok(entry.object()?.try_into_tree().ok()),
        None => Ok(None),
    }
}

/// A value as a JSON blob.
fn json(repo: &gix::Repository, value: &impl Serialize) -> anyhow::Result<ObjectId> {
    Ok(repo.write_blob(serde_json::to_vec_pretty(value)?)?.detach())
}

/// A name no one else will choose, that sorts in the order it was made: the time, a count, and a random part.
fn stamp() -> String {
    static MADE: AtomicU64 = AtomicU64::new(0);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let count = MADE.fetch_add(1, Ordering::Relaxed);
    let mut random = RandomState::new().build_hasher();
    random.write_u64(count);
    format!("{millis:013x}-{count:08x}-{:016x}", random.finish())
}

/// A person's email, or name, as the last part of a ref's name: only letters, digits and `. _ - @ +`, never `..`,
/// and not ending as a lock file does.
fn ref_name(who: &str) -> String {
    let mut name: String = who
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-@+".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect();
    while name.contains("..") {
        name = name.replace("..", ".");
    }
    let name = name.trim_end_matches(".lock").trim_matches(['.', '-']);
    if name.is_empty() {
        "someone".into()
    } else {
        name.to_string()
    }
}

/// A comment's first line, short, for a commit's message.
fn first_words(body: &str) -> String {
    let line = body.lines().next().unwrap_or("").trim();
    if line.chars().count() > 60 {
        format!("{}…", line.chars().take(59).collect::<String>())
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;
    use crate::git::tests::Git;

    const NAMESPACE: &str = "refs/git-kb/comments";

    /// A repository with a page committed, written to by T, and its comments.
    fn site() -> (Git, Store) {
        let git = Git::new();
        git.run(&["config", "user.name", "T"]);
        git.run(&["config", "user.email", "t@t"]);
        git.write("docs/a.md", "# A\n\nOne.\n\nTwo.\n");
        git.commit("A page");
        let repo = Repo::discover(&git.top().join("docs")).unwrap();
        let store = Store::new(&repo, NAMESPACE);
        (git, store)
    }

    fn head(git: &Git) -> ObjectId {
        let out = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(git.top())
            .output()
            .unwrap();
        ObjectId::from_hex(String::from_utf8(out.stdout).unwrap().trim().as_bytes()).unwrap()
    }

    fn git_out(git: &Git, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(git.top())
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap()
    }

    fn begin_on_two(git: &Git, store: &Store) -> String {
        let begin = Begin {
            page: Path::new("a.md"),
            commit: head(git),
            text: "# A\n\nOne.\n\nTwo.\n",
            lines: 4..5,
            snippet: Snippet::default(),
        };
        store.begin(&begin, "  Why two?  ").unwrap()
    }

    #[test]
    fn a_thread_begins_on_its_lines_with_its_first_comment() {
        let (git, store) = site();
        let id = begin_on_two(&git, &store);
        let threads = store.threads().unwrap();
        assert_eq!(threads.len(), 1);
        let thread = &threads[0];
        assert_eq!(thread.id, id);
        assert_eq!(thread.page, Path::new("a.md"));
        assert_eq!(thread.commit, head(&git));
        assert_eq!(
            (thread.lines.clone(), thread.quote.as_str()),
            (4..5, "Two.")
        );
        assert!(!thread.resolved);
        let said: Vec<(&str, &str, &str)> = thread
            .comments
            .iter()
            .map(|c| (c.author.as_str(), c.email.as_str(), c.body.as_str()))
            .collect();
        assert_eq!(said, [("T", "t@t", "Why two?")]);
    }

    #[test]
    fn the_page_s_text_is_kept_as_it_was() {
        let (git, store) = site();
        begin_on_two(&git, &store);
        let thread = &store.threads().unwrap()[0];
        assert_eq!(
            git_out(&git, &["cat-file", "-p", &thread.text.to_string()]),
            "# A\n\nOne.\n\nTwo.\n"
        );
    }

    #[test]
    fn an_image_is_kept_once_by_its_content_and_read_back_by_its_name() {
        let (_git, store) = site();
        let name = store.image(b"\x89PNG fake", "image/png").unwrap();
        assert_eq!(name.rsplit_once('.').map(|(_, e)| e), Some("png"), "{name}");
        assert_eq!(store.image(b"\x89PNG fake", "image/png").unwrap(), name);
        assert_eq!(
            store.file(&name).unwrap().as_deref(),
            Some(&b"\x89PNG fake"[..])
        );
        assert_eq!(store.file("nope.png").unwrap(), None);
        assert_eq!(store.file("../threads").unwrap(), None);
        assert!(
            store.image(b"<svg/>", "image/svg+xml").is_err(),
            "not SVG, which can carry scripts"
        );
        assert!(store.image(b"", "image/png").is_err());
    }

    #[test]
    fn lines_past_the_page_s_end_are_its_last() {
        let (git, store) = site();
        let begin = Begin {
            page: Path::new("a.md"),
            commit: head(&git),
            text: "One.\n",
            lines: 3..9,
            snippet: Snippet::default(),
        };
        store.begin(&begin, "Past the end").unwrap();
        let thread = &store.threads().unwrap()[0];
        assert_eq!((thread.lines.clone(), thread.quote.as_str()), (1..1, ""));
    }

    #[test]
    fn replies_and_resolves_add_to_a_thread() {
        let (git, store) = site();
        let id = begin_on_two(&git, &store);
        store.reply(&id, "Because.").unwrap();
        store.resolve(&id, true).unwrap();
        let thread = store.thread(&id).unwrap().unwrap();
        let bodies: Vec<&str> = thread.comments.iter().map(|c| c.body.as_str()).collect();
        assert_eq!(bodies, ["Why two?", "Because."]);
        assert!(thread.resolved);
        store.resolve(&id, false).unwrap();
        assert!(!store.thread(&id).unwrap().unwrap().resolved, "reopened");
    }

    #[test]
    fn an_empty_or_unknown_comment_is_refused() {
        let (git, store) = site();
        let id = begin_on_two(&git, &store);
        assert!(store.reply(&id, " \n ").is_err());
        assert!(store.reply(&id, &"x".repeat(MAX_BODY + 1)).is_err());
        assert!(store.reply("no-such-thread", "Hello").is_err());
        assert!(store.resolve("no-such-thread", true).is_err());
        assert_eq!(store.thread(&id).unwrap().unwrap().comments.len(), 1);
    }

    #[test]
    fn comments_are_kept_off_every_branch_on_one_s_own_ref() {
        let (git, store) = site();
        let before = git_out(&git, &["rev-parse", "HEAD"]);
        begin_on_two(&git, &store);
        assert_eq!(git_out(&git, &["rev-parse", "HEAD"]), before);
        assert_eq!(git_out(&git, &["status", "--porcelain"]), "");
        assert_eq!(
            git_out(&git, &["branch", "--format=%(refname)"]),
            "refs/heads/main\n"
        );
        assert_eq!(store.mine(), "refs/git-kb/comments/t@t");
        let files = git_out(&git, &["ls-tree", "-r", "--name-only", &store.mine()]);
        assert_eq!(files.lines().filter(|f| f.starts_with("texts/")).count(), 1);
        assert_eq!(
            files.lines().filter(|f| f.ends_with("thread.json")).count(),
            1
        );
        assert_eq!(
            files.lines().filter(|f| f.contains("/comments/")).count(),
            1
        );
    }

    #[test]
    fn each_write_is_a_commit_on_the_last() {
        let (git, store) = site();
        let id = begin_on_two(&git, &store);
        store.reply(&id, "Again").unwrap();
        let log = git_out(&git, &["log", "--format=%an <%ae> %s", &store.mine()]);
        assert_eq!(
            log,
            "T <t@t> Reply on a.md: Again\nT <t@t> Comment on a.md: Why two?\n"
        );
    }

    #[test]
    fn everyone_s_refs_are_read_together() {
        let (git, store) = site();
        let id = begin_on_two(&git, &store);
        // Someone else, with a server of their own on the same repository.
        git.run(&["config", "user.name", "U"]);
        git.run(&["config", "user.email", "u@u"]);
        let theirs = Store::new(&Repo::discover(&git.top()).unwrap(), NAMESPACE);
        theirs.reply(&id, "From U.").unwrap();
        assert_eq!(theirs.mine(), "refs/git-kb/comments/u@u");
        let thread = store.thread(&id).unwrap().unwrap();
        let said: Vec<(&str, &str)> = thread
            .comments
            .iter()
            .map(|c| (c.author.as_str(), c.body.as_str()))
            .collect();
        assert_eq!(said, [("T", "Why two?"), ("U", "From U.")]);
        let mine = git_out(&git, &["ls-tree", "-r", "--name-only", &theirs.mine()]);
        assert!(
            !mine.contains("thread.json"),
            "U's ref has only U's reply: {mine}"
        );
    }

    #[test]
    fn refs_elsewhere_are_not_comments() {
        let (git, store) = site();
        begin_on_two(&git, &store);
        git.run(&["update-ref", "refs/git-kb/other/x", &store.mine()]);
        git.run(&["update-ref", "refs/git-kb/commentsx/y", &store.mine()]);
        assert_eq!(store.threads().unwrap().len(), 1);
    }

    #[test]
    fn a_ref_name_is_safe_for_git() {
        assert_eq!(ref_name("ada@example.com"), "ada@example.com");
        assert_eq!(ref_name("Ada Lovelace"), "Ada-Lovelace");
        assert_eq!(ref_name("a..b"), "a.b");
        assert_eq!(ref_name("x.lock"), "x");
        assert_eq!(ref_name(" .- "), "someone");
    }

    #[test]
    fn stamps_sort_in_the_order_they_were_made() {
        let stamps: Vec<String> = (0..100).map(|_| stamp()).collect();
        let mut sorted = stamps.clone();
        sorted.sort();
        assert_eq!(stamps, sorted);
        sorted.dedup();
        assert_eq!(sorted.len(), 100);
    }

    /// What a page shows: each thread found, as (its first comment, lines, state), and each lost, as its first
    /// comment.
    type Shown = (Vec<(String, Range<usize>, State)>, Vec<String>);

    /// The page as shown at HEAD with this text.
    fn shown(git: &Git, store: &Store, page: &str, text: &str) -> Shown {
        let on = store
            .on_page(Path::new(page), head(git), text, &Renderer::new())
            .unwrap();
        let first = |t: &Thread| t.comments[0].body.clone();
        (
            on.found
                .iter()
                .map(|l| (first(&l.thread), l.lines.clone(), l.state))
                .collect(),
            on.lost.iter().map(first).collect(),
        )
    }

    const PAGE: &str = "# A\n\nThe dispatcher feeds tasklets to the units.\n\nThe scratchpads hold the tiles of each product.\n";

    /// A site with PAGE committed, and a thread on its last paragraph.
    fn commented() -> (Git, Store) {
        let (git, store) = site();
        git.write("docs/a.md", PAGE);
        git.commit("Paragraphs");
        let begin = Begin {
            page: Path::new("a.md"),
            commit: head(&git),
            text: PAGE,
            lines: 4..5,
            snippet: Snippet::default(),
        };
        store.begin(&begin, "On the scratchpads").unwrap();
        (git, store)
    }

    #[test]
    fn a_thread_is_on_its_lines_while_they_are_as_they_were() {
        let (git, store) = commented();
        let (found, lost) = shown(&git, &store, "a.md", PAGE);
        assert_eq!(found, [("On the scratchpads".into(), 4..5, State::Same)]);
        assert_eq!(lost, Vec::<String>::new());
    }

    #[test]
    fn a_thread_follows_its_lines_as_text_is_added_above_them() {
        let (git, store) = commented();
        let now = PAGE.replace(
            "# A\n\n",
            "# A\n\nA new first paragraph.\n\nAnd another.\n\n",
        );
        let (found, _) = shown(&git, &store, "a.md", &now);
        assert_eq!(found, [("On the scratchpads".into(), 8..9, State::Same)]);
    }

    #[test]
    fn a_thread_stays_on_its_lines_as_they_are_edited() {
        let (git, store) = commented();
        let now = PAGE.replace("tiles of each product", "tiles of every product");
        let (found, _) = shown(&git, &store, "a.md", &now);
        assert_eq!(found, [("On the scratchpads".into(), 4..5, State::Edited)]);
    }

    #[test]
    fn a_thread_finds_its_paragraph_moved_elsewhere() {
        let (git, store) = commented();
        let now = "# A\n\nThe scratchpads hold the tiles of each product, as before.\n\nThe dispatcher feeds tasklets to the units.\n";
        let (found, lost) = shown(&git, &store, "a.md", now);
        assert_eq!(found, [("On the scratchpads".into(), 2..3, State::Moved)]);
        assert_eq!(lost, Vec::<String>::new());
    }

    #[test]
    fn a_thread_whose_text_is_gone_is_lost() {
        let (git, store) = commented();
        let now = "# A\n\nThe dispatcher feeds tasklets to the units.\n";
        let (found, lost) = shown(&git, &store, "a.md", now);
        assert_eq!(found, []);
        assert_eq!(lost, ["On the scratchpads"]);
    }

    #[test]
    fn a_thread_is_only_on_its_own_page() {
        let (git, store) = commented();
        git.write("docs/b.md", PAGE);
        git.commit("Another page, the same text");
        let (found, lost) = shown(&git, &store, "b.md", PAGE);
        assert!(found.is_empty() && lost.is_empty());
    }

    #[test]
    fn a_thread_follows_its_page_as_it_is_renamed() {
        let (git, store) = commented();
        git.run(&["mv", "docs/a.md", "docs/moved.md"]);
        git.commit("Rename");
        let (found, _) = shown(&git, &store, "moved.md", PAGE);
        assert_eq!(found, [("On the scratchpads".into(), 4..5, State::Same)]);
        let (found, lost) = shown(&git, &store, "a.md", PAGE);
        assert!(
            found.is_empty() && lost.is_empty(),
            "a.md is no longer its page"
        );
    }

    #[test]
    fn a_thread_from_another_branch_shows_only_where_its_text_plainly_is() {
        let (git, store) = commented();
        git.run(&["checkout", "-q", "-b", "side", "HEAD~1"]);
        git.write("docs/a.md", PAGE);
        git.commit("The same text, on a branch without the thread's commit");
        let (found, _) = shown(&git, &store, "a.md", PAGE);
        assert_eq!(found, [("On the scratchpads".into(), 4..5, State::Same)]);
        let elsewhere = "# A\n\nThe dispatcher feeds tasklets to the units.\n\nSomething else entirely is here now.\n";
        let (found, lost) = shown(&git, &store, "a.md", elsewhere);
        assert_eq!(found, []);
        assert_eq!(
            lost,
            Vec::<String>::new(),
            "not lost either: it is not this history's"
        );
    }

    /// A bare repository, as a team's shared remote, with a page pushed to it, and two people's clones of it, Ada's
    /// and Ben's, each with a store; or, when `same`, two clones of Ada's.
    fn team(same: bool) -> (tempfile::TempDir, [(Git, Store); 2]) {
        let remote = tempfile::tempdir().unwrap();
        let bare = remote.path().to_str().unwrap().to_owned();
        assert!(
            Command::new("git")
                .args(["init", "-q", "--bare", "-b", "main", &bare])
                .status()
                .unwrap()
                .success()
        );
        let clone = |name: &str, email: &str| {
            let git = Git::new();
            git.run(&["config", "user.name", name]);
            git.run(&["config", "user.email", email]);
            git.run(&["remote", "add", "origin", &bare]);
            git
        };
        let ada = clone("Ada", "ada@x");
        ada.write("docs/a.md", PAGE);
        ada.commit("A page");
        ada.run(&["push", "-q", "origin", "main"]);
        let other = if same {
            clone("Ada", "ada@x")
        } else {
            clone("Ben", "ben@x")
        };
        other.run(&["pull", "-q", "origin", "main"]);
        let store =
            |git: &Git| Store::new(&Repo::discover(&git.top().join("docs")).unwrap(), NAMESPACE);
        let (a, b) = (store(&ada), store(&other));
        (remote, [(ada, a), (other, b)])
    }

    fn begin_as(git: &Git, store: &Store, body: &str) -> String {
        let begin = Begin {
            page: Path::new("a.md"),
            commit: head(git),
            text: PAGE,
            lines: 4..5,
            snippet: Snippet::default(),
        };
        store.begin(&begin, body).unwrap()
    }

    fn bodies(store: &Store) -> Vec<String> {
        let mut all: Vec<String> = store
            .threads()
            .unwrap()
            .iter()
            .flat_map(|t| {
                t.comments
                    .iter()
                    .map(|c| format!("{}: {}", c.author, c.body))
            })
            .collect();
        all.sort();
        all
    }

    #[test]
    fn sync_shares_each_person_s_comments_through_a_remote() {
        let (_remote, [(ada, a), (_ben, b)]) = team(false);
        let id = begin_as(&ada, &a, "From Ada");
        let synced = a.sync().unwrap();
        assert_eq!(synced.said, "shared ours with origin");
        let synced = b.sync().unwrap();
        assert_eq!(
            (synced.brought, synced.said.as_str()),
            (1, "1 new comment from origin")
        );
        assert_eq!(bodies(&b), ["Ada: From Ada"]);
        b.reply(&id, "From Ben").unwrap();
        b.sync().unwrap();
        a.sync().unwrap();
        assert_eq!(bodies(&a), ["Ada: From Ada", "Ben: From Ben"]);
        assert_eq!(a.sync().unwrap().said, "up to date with origin");
        assert_eq!(a.last_sync().unwrap().said, "up to date with origin");
    }

    #[test]
    fn the_same_person_writing_from_two_clones_loses_nothing() {
        let (_remote, [(one, a), (two, b)]) = team(true);
        begin_as(&one, &a, "From the laptop");
        begin_as(&two, &b, "From the desktop");
        a.sync().unwrap();
        // The desktop's own ref has gone another way from the remote's: merged, then pushed.
        assert_eq!(
            b.sync().unwrap().said,
            "1 new comment shared ours with origin"
        );
        a.sync().unwrap();
        let both = ["Ada: From the desktop", "Ada: From the laptop"];
        assert_eq!(bodies(&a), both);
        assert_eq!(bodies(&b), both);
        let log = git_out(&one, &["log", "--format=%s", "-1", &a.mine()]);
        assert_eq!(
            log, "Merge comments from origin\n",
            "the laptop moved forward to the merge"
        );
    }

    #[test]
    fn a_sync_without_remotes_says_so_and_is_kept() {
        let (_git, store) = site();
        let e = store.sync().unwrap_err();
        assert!(format!("{e:#}").contains("no remotes"), "{e:#}");
        assert!(store.last_sync().unwrap().said.contains("no remotes"));
    }

    #[test]
    fn a_remote_s_other_refs_are_not_taken_as_comments() {
        let (_remote, [(ada, a), (_ben, b)]) = team(false);
        begin_as(&ada, &a, "From Ada");
        a.sync().unwrap();
        b.sync().unwrap();
        a.sync().unwrap();
        let refs = git_out(
            &ada,
            &["for-each-ref", "--format=%(refname)", "refs/git-kb/"],
        );
        assert_eq!(
            refs, "refs/git-kb/comments/ada@x\nrefs/git-kb/remotes/origin/ada@x\n",
            "only comments are fetched, beside one's own"
        );
    }

    #[test]
    fn an_ai_s_comments_say_whose_they_are_and_it_resolves_only_its_own() {
        let (git, store) = site();
        let person = begin_on_two(&git, &store);
        let begin = Begin {
            page: Path::new("a.md"),
            commit: head(&git),
            text: "# A\n\nOne.\n\nTwo.\n",
            lines: 2..3,
            snippet: Snippet::default(),
        };
        assert!(
            store
                .begin_as(&begin, "Hmm", Some(("Claude", "rant")))
                .is_err(),
            "no such kind"
        );
        let ai = store
            .begin_as(&begin, "Unclear", Some(("Claude", "question")))
            .unwrap();
        let thread = store.thread(&ai).unwrap().unwrap();
        assert_eq!(thread.kind, "question");
        let by = thread.comments[0].ai.clone().unwrap();
        assert_eq!(
            (
                thread.comments[0].author.as_str(),
                by.agent.as_str(),
                by.for_whom.as_str()
            ),
            ("Claude", "Claude", "T")
        );
        store.reply_as(&person, "A note", Some("Claude")).unwrap();
        assert!(
            store.thread(&person).unwrap().unwrap().comments[1]
                .ai
                .is_some()
        );
        assert!(
            store.resolve_as(&person, true, Some("Claude")).is_err(),
            "a person's thread is theirs to resolve"
        );
        store.resolve_as(&ai, true, Some("Claude")).unwrap();
        assert!(store.thread(&ai).unwrap().unwrap().resolved);
        assert_eq!(
            store.thread(&person).unwrap().unwrap().kind,
            "",
            "a person's thread has no kind"
        );
    }
}
