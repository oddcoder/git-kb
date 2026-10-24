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
//! ```
//!
//! A stamp is when it was written, then a count and a random part, so that stamps sort in the order they were
//! written, and no two people's are the same.

use std::collections::BTreeMap;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use anyhow::{Context, anyhow};
use gix::ObjectId;
use gix::objs::tree::EntryKind;
use serde::{Deserialize, Serialize};

use crate::git::{Repo, When};

/// The longest comment taken, in bytes.
pub const MAX_BODY: usize = 20_000;

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
}

/// A resolve or a reopen, as kept in `threads/<id>/state/`.
#[derive(Serialize, Deserialize)]
struct Resolve {
    author: String,
    email: String,
    time: i64,
    resolved: bool,
}

/// The comments kept in a repository.
pub struct Store {
    git: gix::ThreadSafeRepository,
    /// Where everyone's refs are, such as `refs/git-kb/comments`.
    namespace: String,
    /// Writes are one at a time, so that two at once both land on one's ref.
    writing: Mutex<()>,
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
            namespace: namespace.trim_end_matches('/').to_string(),
            writing: Mutex::default(),
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
        let body = Self::body(body)?;
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
        };
        let files = [
            (format!("texts/{text}"), text),
            (format!("threads/{id}/thread.json"), json(&repo, &anchor)?),
            (
                format!("threads/{id}/comments/{}.json", stamp()),
                json(&repo, &self.comment(body))?,
            ),
        ];
        let message = format!("Comment on {}: {}", on.page.display(), first_words(body));
        self.commit(&repo, &files, &message)?;
        Ok(id)
    }

    /// Replies to a thread.
    ///
    /// # Errors
    ///
    /// When there is no such thread, the reply is empty or too long, or the repository cannot be written.
    pub fn reply(&self, id: &str, body: &str) -> anyhow::Result<()> {
        let body = Self::body(body)?;
        let thread = self.thread(id)?.ok_or_else(|| anyhow!("no thread {id}"))?;
        let repo = self.git.to_thread_local();
        let file = (
            format!("threads/{id}/comments/{}.json", stamp()),
            json(&repo, &self.comment(body))?,
        );
        let message = format!("Reply on {}: {}", thread.page.display(), first_words(body));
        self.commit(&repo, &[file], &message)
    }

    /// Resolves a thread, or reopens it.
    ///
    /// # Errors
    ///
    /// When there is no such thread, or the repository cannot be written.
    pub fn resolve(&self, id: &str, resolved: bool) -> anyhow::Result<()> {
        let thread = self.thread(id)?.ok_or_else(|| anyhow!("no thread {id}"))?;
        let repo = self.git.to_thread_local();
        let (author, email) = self.me();
        let event = Resolve {
            author,
            email,
            time: When::now().0,
            resolved,
        };
        let file = (
            format!("threads/{id}/state/{}.json", stamp()),
            json(&repo, &event)?,
        );
        let what = if resolved { "Resolve" } else { "Reopen" };
        let message = format!("{what} a thread on {}", thread.page.display());
        self.commit(&repo, &[file], &message)
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

    /// A comment by whoever writes here, now.
    fn comment(&self, body: &str) -> Comment {
        let (author, email) = self.me();
        Comment {
            author,
            email,
            time: When::now().0,
            body: body.to_string(),
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
        let parent = match repo.try_find_reference(mine.as_str())? {
            Some(mut reference) => Some(reference.peel_to_id()?.detach()),
            None => None,
        };
        let base = match parent {
            Some(parent) => repo.find_commit(parent)?.tree_id()?.detach(),
            None => ObjectId::empty_tree(repo.object_hash()),
        };
        let mut tree = repo.edit_tree(base)?;
        for (path, blob) in files {
            tree.upsert(path.as_str(), EntryKind::Blob, *blob)?;
        }
        let tree = tree.write()?.detach();
        let (name, email) = self.me();
        let signature = gix::actor::Signature {
            name: name.into(),
            email: email.into(),
            time: gix::date::Time::now_local_or_utc(),
        };
        let mut time = gix::date::parse::TimeBuf::default();
        let signature = signature.to_ref(&mut time);
        repo.commit_as(signature, signature, mine.as_str(), message, tree, parent)
            .with_context(|| format!("writing {mine}"))?;
        Ok(())
    }
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
}
