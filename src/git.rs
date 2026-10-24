//! The site's git history: its files at any commit, and the commits that changed its pages, read with gix. A
//! commit's files are its site's as a build would have them then: what its own `.gitignore` and `.kbignore` files
//! ignore is left out, as on disk. A commit never changes, so what is read of one is kept.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use anyhow::{Context, anyhow};
use gix::ObjectId;
use gix::traverse::tree::Recorder;
use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::build::Builder;

/// The git repository a site's folder is in.
pub struct Repo {
    git: gix::ThreadSafeRepository,
    /// The site's folder in the repository, as `docs`, or empty when it is the repository's top.
    prefix: PathBuf,
    /// Each commit's files, by its id.
    files: Mutex<HashMap<ObjectId, Arc<BTreeMap<PathBuf, ObjectId>>>>,
    /// The history up to each commit, by its id.
    histories: Mutex<HashMap<ObjectId, Arc<History>>>,
    /// Who last changed each line of a page at a commit, by the commit's id and the page.
    blames: Mutex<HashMap<(ObjectId, PathBuf), Arc<Blame>>>,
}

/// The commit that last changed each line of a page, by line from 0; `None` for a line not committed yet.
pub type Blame = Vec<Option<Change>>;

/// A commit, by the name it was asked for by, as `main`, and its id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revision {
    pub name: String,
    pub id: ObjectId,
    /// Its id, as short as it can be and still be its own.
    pub short: String,
}

/// A commit that changed the site: who made it, when, and why, in its message's first line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub id: ObjectId,
    /// Its id, as short as it can be and still be its own.
    pub short: String,
    /// When it was made, in seconds since 1970.
    pub time: i64,
    pub author: String,
    pub summary: String,
    /// Whether it is a first commit, with none before it.
    pub first: bool,
}

/// A time, in seconds since 1970, as people say it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct When(pub i64);

/// The commits up to one that changed the site's pages, newest first: each with the pages it made, edited or
/// removed, against the commit before it; and, for each page, the commits that changed it.
#[derive(Debug, Default)]
pub struct History {
    pub log: Vec<(Change, Vec<PathBuf>)>,
    /// Each page's commits, as places in the log.
    pages: BTreeMap<PathBuf, Vec<usize>>,
}

/// The site's files at a commit, by their paths from its top, as `Builder::files` has them on disk.
#[derive(Debug)]
pub struct Snapshot {
    pub revision: Revision,
    files: Arc<BTreeMap<PathBuf, ObjectId>>,
}

impl std::fmt::Debug for Repo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Repo")
            .field("prefix", &self.prefix)
            .finish_non_exhaustive()
    }
}

impl Repo {
    /// The repository a folder is in, if it is in one with a working tree.
    #[must_use]
    pub fn discover(root: &Path) -> Option<Self> {
        let root = std::fs::canonicalize(root).ok()?;
        let repo = gix::ThreadSafeRepository::discover(&root).ok()?;
        let top = std::fs::canonicalize(repo.work_dir()?).ok()?;
        let prefix = root.strip_prefix(&top).ok()?.to_path_buf();
        Some(Self {
            git: repo,
            prefix,
            files: Mutex::default(),
            histories: Mutex::default(),
            blames: Mutex::default(),
        })
    }

    /// The site's folder in the repository, as `docs`, or empty when it is the repository's top.
    #[must_use]
    pub fn prefix(&self) -> &Path {
        &self.prefix
    }

    /// The repository, for what reads or writes it beyond the site's files, as the comments do.
    #[must_use]
    pub fn git(&self) -> &gix::ThreadSafeRepository {
        &self.git
    }

    /// The site's files at the commit a revision names: a branch, a tag, a commit's id, or anything else
    /// `git rev-parse` takes, as `main~2`.
    ///
    /// # Errors
    ///
    /// When the revision names no commit, or the commit cannot be read.
    pub fn snapshot(&self, name: &str) -> anyhow::Result<Arc<Snapshot>> {
        self.snapshot_as(name, name)
    }

    /// The same, of the commit `spec` names, as `refs/tags/v1`, named `name`, as `v1`.
    fn snapshot_as(&self, spec: &str, name: &str) -> anyhow::Result<Arc<Snapshot>> {
        let repo = self.git.to_thread_local();
        let commit = repo
            .rev_parse_single(spec)
            .with_context(|| format!("no revision {name}"))?
            .object()?
            .peel_to_commit()
            .with_context(|| format!("{name} is not a commit"))?;
        let revision = Revision {
            name: name.to_owned(),
            id: commit.id,
            short: commit.id().shorten_or_id().to_string(),
        };
        let files = self.files_at(&repo, commit.id)?;
        Ok(Arc::new(Snapshot { revision, files }))
    }

    /// A commit's files, read once.
    fn files_at(
        &self,
        repo: &gix::Repository,
        id: ObjectId,
    ) -> anyhow::Result<Arc<BTreeMap<PathBuf, ObjectId>>> {
        let kept = self
            .files
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id)
            .cloned();
        if let Some(files) = kept {
            return Ok(files);
        }
        let files = Arc::new(self.files(repo, &repo.find_commit(id)?.tree()?)?);
        self.files
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, Arc::clone(&files));
        Ok(files)
    }

    /// The commits up to a snapshot's that changed the site's pages, against each one's first parent, as
    /// `git log --first-parent` would compare them, newest first.
    ///
    /// # Errors
    ///
    /// When a commit cannot be read.
    pub fn history(&self, snapshot: &Snapshot) -> anyhow::Result<Arc<History>> {
        let id = snapshot.revision.id;
        let kept = self
            .histories
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id)
            .cloned();
        if let Some(history) = kept {
            return Ok(history);
        }
        let repo = self.git.to_thread_local();
        let walk = repo
            .rev_walk([id])
            .sorting(gix::revision::walk::Sorting::ByCommitTime(
                gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
            ))
            .all()?;
        let mut history = History::default();
        let none = Arc::new(BTreeMap::new());
        for info in walk {
            let info = info?;
            let now = self.files_at(&repo, info.id)?;
            let before = match info.parent_ids().next() {
                Some(parent) => self.files_at(&repo, parent.detach())?,
                None => Arc::clone(&none),
            };
            let markdown =
                |p: &&PathBuf| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("md"));
            let pages: Vec<PathBuf> = now
                .iter()
                .filter(|(path, blob)| before.get(*path) != Some(blob))
                .map(|(path, _)| path)
                .chain(before.keys().filter(|path| !now.contains_key(*path)))
                .filter(markdown)
                .cloned()
                .collect();
            if pages.is_empty() {
                continue;
            }
            let change = Self::change(&info.object()?)?;
            for page in &pages {
                history
                    .pages
                    .entry(page.clone())
                    .or_default()
                    .push(history.log.len());
            }
            history.log.push((change, pages));
        }
        let history = Arc::new(history);
        self.histories
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, Arc::clone(&history));
        Ok(history)
    }

    /// The site's files at each branch and tag of the repository, by its name, as `main` or `v1.0`: branches
    /// first, then tags, each by name. A tag of what is not a commit is none; nor is one named as a branch is.
    ///
    /// # Errors
    ///
    /// When the repository's branches or tags cannot be read.
    pub fn refs(&self) -> anyhow::Result<Vec<Arc<Snapshot>>> {
        let repo = self.git.to_thread_local();
        let references = repo.references()?;
        let mut names: Vec<(String, String)> = vec![];
        for reference in references.local_branches()?.chain(references.tags()?) {
            let reference = reference.map_err(|e| anyhow!(e))?;
            let name = reference.name();
            names.push((name.as_bstr().to_string(), name.shorten().to_string()));
        }
        let mut snapshots: Vec<Arc<Snapshot>> = vec![];
        for (spec, name) in names {
            if snapshots.iter().any(|s| s.revision.name == name) {
                tracing::warn!(
                    name,
                    "a branch and a tag share a name; only the branch is built"
                );
                continue;
            }
            match self.snapshot_as(&spec, &name) {
                Ok(snapshot) => snapshots.push(snapshot),
                Err(e) => tracing::debug!(name, "not a commit: {e:#}"),
            }
        }
        Ok(snapshots)
    }

    /// Who last changed each line of a page at a snapshot's commit; given `now`, the page's text on disk, of
    /// that text instead, its lines not at the commit not committed yet.
    ///
    /// # Errors
    ///
    /// When the page is not at the commit, or its history cannot be read.
    pub fn blame(
        &self,
        snapshot: &Snapshot,
        page: &Path,
        now: Option<&[u8]>,
    ) -> anyhow::Result<Arc<Blame>> {
        let blame = self.blame_at(snapshot, page)?;
        let Some(now) = now else {
            return Ok(blame);
        };
        let committed = self.read(snapshot, page)?;
        // The lines on disk as they were at the commit keep its blame; the rest are not committed yet.
        let (old, new) = (
            String::from_utf8_lossy(&committed),
            String::from_utf8_lossy(now),
        );
        let diff = similar::TextDiff::from_lines(old.as_ref(), new.as_ref());
        let mut lines: Blame = vec![None; new.split_inclusive('\n').count()];
        for op in diff.ops() {
            if let similar::DiffOp::Equal {
                old_index,
                new_index,
                len,
            } = *op
            {
                for i in 0..len {
                    lines[new_index + i] = blame.get(old_index + i).cloned().flatten();
                }
            }
        }
        Ok(Arc::new(lines))
    }

    /// Who last changed each line of a page at a snapshot's commit, worked out once.
    fn blame_at(&self, snapshot: &Snapshot, page: &Path) -> anyhow::Result<Arc<Blame>> {
        let key = (snapshot.revision.id, page.to_path_buf());
        let kept = self
            .blames
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .cloned();
        if let Some(blame) = kept {
            return Ok(blame);
        }
        let repo = self.git.to_thread_local();
        let path = gix::path::into_bstr(self.prefix.join(page))?.into_owned();
        let options = gix::repository::blame_file::Options::default();
        let outcome = repo.blame_file(path.as_ref(), snapshot.revision.id, options)?;
        let lines = outcome.blob.split_inclusive(|b| *b == b'\n').count();
        let mut blame: Blame = vec![None; lines];
        let mut changes: HashMap<ObjectId, Change> = HashMap::new();
        for entry in &outcome.entries {
            let change = match changes.entry(entry.commit_id) {
                std::collections::hash_map::Entry::Occupied(kept) => kept.into_mut(),
                std::collections::hash_map::Entry::Vacant(new) => {
                    new.insert(Self::change(&repo.find_commit(entry.commit_id)?)?)
                }
            };
            let start = entry.start_in_blamed_file as usize;
            for line in blame.iter_mut().skip(start).take(entry.len.get() as usize) {
                *line = Some(change.clone());
            }
        }
        let blame = Arc::new(blame);
        self.blames
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key, Arc::clone(&blame));
        Ok(blame)
    }

    /// What a commit says of itself.
    fn change(commit: &gix::Commit) -> anyhow::Result<Change> {
        Ok(Change {
            id: commit.id,
            short: commit.id().shorten_or_id().to_string(),
            time: commit.time()?.seconds,
            author: commit.author()?.name.to_string(),
            summary: commit.message()?.summary().to_string(),
            first: commit.parent_ids().next().is_none(),
        })
    }

    /// A file of a snapshot, as it was at its commit.
    ///
    /// # Errors
    ///
    /// When the snapshot has no such file, or it cannot be read.
    pub fn read(&self, snapshot: &Snapshot, path: &Path) -> anyhow::Result<Vec<u8>> {
        let id = snapshot
            .files
            .get(path)
            .ok_or_else(|| anyhow!("{} is not in {}", path.display(), snapshot.revision.name))?;
        Ok(self.git.to_thread_local().find_blob(*id)?.detach().data)
    }

    /// The site's files in a commit's tree, but what the tree's own ignore files ignore.
    fn files(
        &self,
        repo: &gix::Repository,
        tree: &gix::Tree,
    ) -> anyhow::Result<BTreeMap<PathBuf, ObjectId>> {
        let site = if self.prefix.as_os_str().is_empty() {
            tree.clone()
        } else {
            match tree.lookup_entry_by_path(&self.prefix)? {
                Some(entry) if entry.mode().is_tree() => entry.object()?.peel_to_tree()?,
                // The folder was not there yet.
                _ => return Ok(BTreeMap::new()),
            }
        };
        let mut recorder = Recorder::default();
        site.traverse().breadthfirst(&mut recorder)?;
        let mut ignores = Ignores::default();
        // The ignore files above the site's folder, as on disk, from the repository's top down.
        let mut above = PathBuf::new();
        for part in std::iter::once(None).chain(self.prefix.components().map(Some)) {
            if let Some(part) = part {
                above.push(part);
            }
            if above == self.prefix {
                break;
            }
            for name in Ignores::NAMES {
                if let Some(entry) = tree.lookup_entry_by_path(above.join(name))? {
                    ignores.add(&above, name, &entry.object()?.detach().data);
                }
            }
        }
        let mut blobs = BTreeMap::new();
        for entry in recorder.records {
            let path = self.prefix.join(gix::path::from_bstr(&entry.filepath)?);
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            // Links are neither followed nor copied, as on disk; nor are other repositories, submodules.
            if !entry.mode.is_blob() {
                continue;
            }
            if Ignores::NAMES.contains(&name) {
                let blob = repo.find_blob(entry.oid)?;
                ignores.add(path.parent().unwrap_or(Path::new("")), name, &blob.data);
            }
            blobs.insert(path, entry.oid);
        }
        Ok(blobs
            .into_iter()
            .filter(|(path, _)| !ignores.ignored(path))
            .filter_map(|(path, id)| {
                Some((path.strip_prefix(&self.prefix).ok()?.to_path_buf(), id))
            })
            .collect())
    }
}

impl Snapshot {
    /// Its files, by their paths from the site's top.
    #[must_use]
    pub fn files(&self) -> Vec<PathBuf> {
        self.files.keys().cloned().collect()
    }

    /// Whether a file has these bytes at its commit: `None` when the commit has no such file.
    #[must_use]
    pub fn holds(&self, path: &Path, bytes: &[u8]) -> Option<bool> {
        let blob = self.files.get(path)?;
        let hash = gix::objs::compute_hash(blob.kind(), gix::objs::Kind::Blob, bytes).ok()?;
        Some(hash == *blob)
    }
}

impl Change {
    /// How long before `now`, both in seconds since 1970, it was made, as people say it: `3 days ago`.
    #[must_use]
    pub fn ago(&self, now: i64) -> String {
        When(self.time).ago(now)
    }

    /// The day it was made, in UTC: `2026-10-15`.
    #[must_use]
    pub fn date(&self) -> String {
        When(self.time).date()
    }
}

impl When {
    /// How long before `now` it was: `3 days ago`.
    #[must_use]
    pub fn ago(self, now: i64) -> String {
        let s = (now - self.0).max(0);
        let (n, unit) = match s {
            0..60 => return "just now".into(),
            60..3600 => (s / 60, "minute"),
            3600..86_400 => (s / 3600, "hour"),
            86_400..2_592_000 => (s / 86_400, "day"),
            2_592_000..31_536_000 => (s / 2_592_000, "month"),
            _ => (s / 31_536_000, "year"),
        };
        format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
    }

    /// Its day, in UTC: `2026-10-15`.
    #[must_use]
    pub fn date(self) -> String {
        // Days to a civil date, after Howard Hinnant's civil_from_days.
        let z = self.0.div_euclid(86_400) + 719_468;
        let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + era * 400 + i64::from(month <= 2);
        format!("{year:04}-{month:02}-{day:02}")
    }

    /// Now.
    #[must_use]
    pub fn now() -> Self {
        Self(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX)),
        )
    }
}

impl History {
    /// The commits that changed a page, newest first.
    pub fn of(&self, page: &Path) -> impl Iterator<Item = &Change> {
        self.pages
            .get(page)
            .into_iter()
            .flatten()
            .map(|&i| &self.log[i].0)
    }
}

/// The ignore files of a commit's folders, by folder, from the repository's top.
#[derive(Default)]
struct Ignores {
    by_folder: BTreeMap<PathBuf, Vec<(usize, Gitignore)>>,
}

impl Ignores {
    /// The ignore files, by the order a folder's are asked in: `.kbignore` first, which says more of the
    /// site, then `.gitignore`.
    const NAMES: [&str; 2] = [Builder::IGNORE, ".gitignore"];

    fn add(&mut self, folder: &Path, name: &str, text: &[u8]) {
        let mut builder = GitignoreBuilder::new(folder);
        for line in String::from_utf8_lossy(text).lines() {
            // A line that is not a pattern is skipped, as git skips it.
            let _ = builder.add_line(None, line);
        }
        let Ok(ignore) = builder.build() else {
            return;
        };
        let order = Self::NAMES.iter().position(|n| *n == name).unwrap_or(0);
        let ignores = self.by_folder.entry(folder.to_path_buf()).or_default();
        ignores.push((order, ignore));
        ignores.sort_by_key(|(order, _)| *order);
    }

    /// Whether a file, by its path from the repository's top, is ignored: by the nearest folder's ignore file
    /// that says, of the file or a folder it is in.
    fn ignored(&self, path: &Path) -> bool {
        for folder in path.ancestors().skip(1) {
            let Some(ignores) = self.by_folder.get(folder) else {
                continue;
            };
            let Ok(within) = path.strip_prefix(folder) else {
                continue;
            };
            for (_, ignore) in ignores {
                let matched = ignore.matched_path_or_any_parents(within, false);
                if matched.is_ignore() {
                    return true;
                }
                if matched.is_whitelist() {
                    return false;
                }
            }
        }
        false
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::fs;
    use std::process::Command;

    use super::*;

    /// A repository in a temporary folder, made and committed to with git itself.
    pub(crate) struct Git(tempfile::TempDir);

    impl Git {
        pub(crate) fn new() -> Self {
            let git = Self(tempfile::tempdir().unwrap());
            git.run(&["init", "-q", "-b", "main"]);
            git
        }

        pub(crate) fn top(&self) -> PathBuf {
            fs::canonicalize(self.0.path()).unwrap()
        }

        pub(crate) fn run(&self, args: &[&str]) {
            let status = Command::new("git")
                .args([
                    "-c",
                    "user.name=T",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .current_dir(self.0.path())
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?}");
        }

        pub(crate) fn write(&self, path: &str, text: &str) {
            let path = self.0.path().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        pub(crate) fn commit(&self, message: &str) {
            self.run(&["add", "-A", "-f"]);
            self.run(&["commit", "-q", "-m", message]);
        }
    }

    fn names(snapshot: &Snapshot) -> Vec<String> {
        snapshot
            .files()
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn a_commit_s_files_are_the_site_s_then() {
        let git = Git::new();
        git.write("docs/a.md", "# A, first");
        git.commit("first");
        git.write("docs/a.md", "# A, second");
        git.write("docs/b.md", "# B");
        git.write("notes.md", "# Outside the site");
        git.commit("second");
        let repo = Repo::discover(&git.top().join("docs")).unwrap();
        let first = repo.snapshot("main~1").unwrap();
        assert_eq!(names(&first), ["a.md"]);
        assert_eq!(repo.read(&first, Path::new("a.md")).unwrap(), b"# A, first");
        let now = repo.snapshot("main").unwrap();
        assert_eq!(names(&now), ["a.md", "b.md"]);
        assert_eq!(now.revision.name, "main");
        assert!(now.revision.id.to_string().starts_with(&now.revision.short));
        assert!(repo.read(&now, Path::new("../notes.md")).is_err());
    }

    #[test]
    fn what_a_commit_s_ignore_files_ignore_is_left_out() {
        let git = Git::new();
        git.write(".gitignore", "*.log\n");
        git.write("docs/.kbignore", "drafts/\n");
        git.write("docs/a.md", "# A");
        git.write("docs/run.log", "ignored by the top's .gitignore");
        git.write("docs/drafts/b.md", "# Draft");
        git.write("docs/deep/.gitignore", "*.tmp\n!keep.tmp\n");
        git.write("docs/deep/x.tmp", "");
        git.write("docs/deep/keep.tmp", "");
        git.commit("all");
        let repo = Repo::discover(&git.top().join("docs")).unwrap();
        assert_eq!(
            names(&repo.snapshot("HEAD").unwrap()),
            [".kbignore", "a.md", "deep/.gitignore", "deep/keep.tmp"]
        );
    }

    #[test]
    fn a_page_s_history_is_the_commits_that_changed_it() {
        let git = Git::new();
        git.write("docs/a.md", "# A");
        git.write("docs/b.md", "# B");
        git.commit("both");
        git.write("docs/a.md", "# A, again");
        git.write("notes.md", "outside the site");
        git.commit("edit a");
        git.write("docs/c.png", "not a page");
        git.commit("an image");
        git.run(&["rm", "-q", "docs/b.md"]);
        git.commit("remove b");
        let repo = Repo::discover(&git.top().join("docs")).unwrap();
        let history = repo.history(&repo.snapshot("HEAD").unwrap()).unwrap();
        let log: Vec<(&str, Vec<&str>)> = history
            .log
            .iter()
            .map(|(c, pages)| {
                (
                    c.summary.as_str(),
                    pages.iter().map(|p| p.to_str().unwrap()).collect(),
                )
            })
            .collect();
        assert_eq!(
            log,
            [
                ("remove b", vec!["b.md"]),
                ("edit a", vec!["a.md"]),
                ("both", vec!["a.md", "b.md"])
            ]
        );
        let a: Vec<&str> = history
            .of(Path::new("a.md"))
            .map(|c| c.summary.as_str())
            .collect();
        assert_eq!(a, ["edit a", "both"]);
        let (last, _) = &history.log[0];
        assert_eq!(last.author, "T");
        assert!(last.id.to_string().starts_with(&last.short));
        let head = repo.snapshot("HEAD").unwrap();
        assert_eq!(head.holds(Path::new("a.md"), b"# A, again"), Some(true));
        assert_eq!(head.holds(Path::new("a.md"), b"# A, edited"), Some(false));
        assert_eq!(head.holds(Path::new("new.md"), b""), None);
    }

    #[test]
    fn each_line_is_blamed_on_the_commit_that_last_changed_it() {
        let git = Git::new();
        git.write("docs/a.md", "one\ntwo\n");
        git.commit("first");
        git.write("docs/a.md", "one\nTWO\n");
        git.commit("second");
        let repo = Repo::discover(&git.top().join("docs")).unwrap();
        let head = repo.snapshot("HEAD").unwrap();
        let blame = repo.blame(&head, Path::new("a.md"), None).unwrap();
        let said: Vec<Option<&str>> = blame
            .iter()
            .map(|c| c.as_ref().map(|c| c.summary.as_str()))
            .collect();
        assert_eq!(said, [Some("first"), Some("second")]);
        assert!(blame[0].as_ref().unwrap().first);
        let now = repo
            .blame(&head, Path::new("a.md"), Some(b"zero\none\nTWO\n"))
            .unwrap();
        let said: Vec<Option<&str>> = now
            .iter()
            .map(|c| c.as_ref().map(|c| c.summary.as_str()))
            .collect();
        assert_eq!(
            said,
            [None, Some("first"), Some("second")],
            "a line not committed yet is no one's"
        );
    }

    #[test]
    fn a_change_says_when_it_was_made() {
        let change = Change {
            id: ObjectId::null(gix::hash::Kind::Sha1),
            short: "0000000".into(),
            time: 1_760_486_400,
            author: "T".into(),
            summary: String::new(),
            first: true,
        };
        assert_eq!(change.date(), "2025-10-15");
        assert_eq!(change.ago(change.time + 30), "just now");
        assert_eq!(change.ago(change.time + 3600), "1 hour ago");
        assert_eq!(change.ago(change.time + 3 * 86_400), "3 days ago");
        assert_eq!(Change { time: 0, ..change }.date(), "1970-01-01");
    }

    #[test]
    fn a_revision_that_names_nothing_is_an_error_naming_it() {
        let git = Git::new();
        git.write("a.md", "# A");
        git.commit("first");
        let repo = Repo::discover(&git.top()).unwrap();
        let error = format!("{:#}", repo.snapshot("nope").unwrap_err());
        assert!(error.contains("no revision nope"), "{error}");
        assert!(
            Repo::discover(&fs::canonicalize(tempfile::tempdir().unwrap().path()).unwrap())
                .is_none()
        );
    }
}
