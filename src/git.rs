//! The site's git history: its files at any commit, read with gix. A commit's files are its site's as a build
//! would have them then: what its own `.gitignore` and `.kbignore` files ignore is left out, as on disk. A commit
//! never changes, so its files, once listed, are kept.

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
    snapshots: Mutex<HashMap<ObjectId, Arc<Snapshot>>>,
}

/// A commit, by the name it was asked for by, as `main`, and its id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revision {
    pub name: String,
    pub id: ObjectId,
    /// Its id, as short as it can be and still be its own.
    pub short: String,
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
            snapshots: Mutex::default(),
        })
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
        let mut snapshots = self
            .snapshots
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(snapshot) = snapshots.get(&commit.id) {
            // Kept by its id; named as asked this time.
            return Ok(Arc::new(Snapshot {
                revision,
                files: Arc::clone(&snapshot.files),
            }));
        }
        let files = Arc::new(self.files(&repo, &commit.tree()?)?);
        let snapshot = Arc::new(Snapshot { revision, files });
        snapshots.insert(commit.id, Arc::clone(&snapshot));
        Ok(snapshot)
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
