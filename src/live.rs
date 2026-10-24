//! Live: what tells open pages that something they show has changed, so that they load again. It watches the
//! folder, and says so when a file of the site is made, changed or removed; not when anything else in the folder
//! is, such as what `.gitignore` ignores, so a build filling `target/` does not reload every page. Given the
//! folder's repository, it watches its refs too, as a commit, a checkout, a rebase or a pull moves them, or a
//! comment is written or synced.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use anyhow::Context;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::broadcast;
use tracing::{debug, warn};

use crate::build::Builder;

/// What changed: `docs` for the site's files, `git` for the repository's refs, and `pull` for its remotes'
/// branches, as a fetch or a pull moves them, after which the comments are synced too.
pub type Change = &'static str;

/// Watches a folder's site, and its repository's refs, telling whoever listens what changes; watching stops when
/// it is dropped.
#[derive(Debug)]
pub struct Live {
    _watcher: RecommendedWatcher,
}

impl Live {
    /// Watches the site of a folder, and, given its repository's git folder, its HEAD and refs, telling `changes`
    /// what changes.
    ///
    /// # Errors
    ///
    /// When the folder cannot be watched, or its files cannot be listed.
    pub fn watch(
        root: &Path,
        git: Option<&Path>,
        changes: broadcast::Sender<Change>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            _watcher: Self::watcher(root, git, changes)?,
        })
    }

    fn watcher(
        root: &Path,
        git: Option<&Path>,
        told: broadcast::Sender<Change>,
    ) -> anyhow::Result<RecommendedWatcher> {
        let files = Mutex::new(Self::files(root)?);
        let (folder, repository) = (root.to_path_buf(), git.map(Path::to_path_buf));
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let event = match event {
                    Ok(event) if !matches!(event.kind, EventKind::Access(_)) => event,
                    Ok(_) => return,
                    Err(e) => return warn!("watching: {e}"),
                };
                // A ref of the repository moved.
                if let Some(git) = &repository
                    && let Some(change) =
                        event.paths.iter().find_map(|path| Self::of_git(path, git))
                {
                    debug!(change, "refs moved");
                    let _ = told.send(change);
                    return;
                }
                // The site's files as they were and as they are, to tell whether the change is to one of them.
                let Ok(now) = Self::files(&folder) else {
                    return;
                };
                let before = std::mem::replace(
                    &mut *files
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner),
                    now.clone(),
                );
                let touched = event.paths.iter().find(|path| {
                    path.strip_prefix(&folder)
                        .is_ok_and(|path| Self::of_site(path, &before) || Self::of_site(path, &now))
                });
                if let Some(path) = touched {
                    debug!(path = %path.display(), "changed");
                    // No one may be listening, which is no matter.
                    let _ = told.send("docs");
                }
            })?;
        watcher
            .watch(root, RecursiveMode::Recursive)
            .with_context(|| root.display().to_string())?;
        if let Some(git) = git {
            watcher
                .watch(git, RecursiveMode::NonRecursive)
                .with_context(|| git.display().to_string())?;
            let refs = git.join("refs");
            watcher
                .watch(&refs, RecursiveMode::Recursive)
                .with_context(|| refs.display().to_string())?;
        }
        Ok(watcher)
    }

    fn files(root: &Path) -> anyhow::Result<BTreeSet<PathBuf>> {
        Ok(Builder::files(root, None)?.into_iter().collect())
    }

    /// Whether a path in the folder is the site's: one of its files, or a folder holding one, as when a
    /// folder is renamed.
    fn of_site(path: &Path, files: &BTreeSet<PathBuf>) -> bool {
        files.contains(path) || files.iter().any(|f| f.starts_with(path))
    }

    /// What a change in the repository's git folder is: `pull` for its remotes' branches, `git` for its HEAD,
    /// packed refs and other refs, and nothing for the rest, such as its objects or index.
    fn of_git(path: &Path, git: &Path) -> Option<Change> {
        let inside = path.strip_prefix(git).ok()?;
        if inside.starts_with("refs/remotes") {
            return Some("pull");
        }
        let first = inside.components().next()?;
        matches!(first, Component::Normal(n) if n == "HEAD" || n == "packed-refs" || n == "refs")
            .then_some("git")
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_path_is_the_site_s_if_it_is_a_file_of_it_or_holds_one() {
        let files = BTreeSet::from([PathBuf::from("arch/dma.md"), PathBuf::from("index.md")]);
        assert!(Live::of_site(Path::new("index.md"), &files));
        assert!(Live::of_site(Path::new("arch"), &files));
        assert!(!Live::of_site(Path::new("target/debug/x"), &files));
        assert!(!Live::of_site(Path::new("arch/dm"), &files));
    }

    #[test]
    fn moved_refs_are_told_apart_from_the_rest_of_git() {
        let git = Path::new("/repo/.git");
        assert_eq!(Live::of_git(Path::new("/repo/.git/HEAD"), git), Some("git"));
        assert_eq!(
            Live::of_git(Path::new("/repo/.git/refs/heads/main"), git),
            Some("git")
        );
        assert_eq!(
            Live::of_git(Path::new("/repo/.git/refs/remotes/origin/main"), git),
            Some("pull")
        );
        assert_eq!(
            Live::of_git(Path::new("/repo/.git/objects/ab/cd"), git),
            None
        );
        assert_eq!(Live::of_git(Path::new("/repo/docs/a.md"), git), None);
    }

    /// Whether a change is told within a while.
    async fn told(changes: &mut broadcast::Receiver<Change>) -> bool {
        tokio::time::timeout(Duration::from_secs(5), changes.recv())
            .await
            .is_ok()
    }

    #[tokio::test]
    async fn a_change_to_a_page_is_told_and_one_to_what_is_ignored_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        fs::write(root.join(".gitignore"), "target/\n").unwrap();
        fs::write(root.join("a.md"), "# A").unwrap();
        fs::create_dir(root.join("target")).unwrap();
        let told_to = broadcast::channel(16).0;
        let mut changes = told_to.subscribe();
        let _live = Live::watch(&root, None, told_to).unwrap();

        fs::write(root.join("target/x.md"), "# Ignored").unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(500), changes.recv())
                .await
                .is_err(),
            "an ignored file is not the site's"
        );
        fs::write(root.join("a.md"), "# A, changed").unwrap();
        assert!(told(&mut changes).await, "a change to a page");
        fs::write(root.join("b.md"), "# B").unwrap();
        assert!(told(&mut changes).await, "a new page");
    }
}
