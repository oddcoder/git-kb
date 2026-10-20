//! Live: what tells open pages that the site has changed, so that they load again. It watches the folder, and
//! says so when a file of the site is made, changed or removed; not when anything else in the folder is, such
//! as what `.gitignore` ignores, so a build filling `target/` does not reload every page.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Context;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::broadcast;
use tracing::{debug, warn};

use crate::build::Builder;

/// Watches a folder's site, and tells whoever listens when it changes.
#[derive(Debug)]
pub struct Live {
    changes: broadcast::Sender<()>,
    /// Kept, as watching stops when it is dropped.
    _watcher: RecommendedWatcher,
}

impl Live {
    /// Watches the site of a folder, telling `changes` when it changes.
    ///
    /// # Errors
    ///
    /// When the folder cannot be watched, or its files cannot be listed.
    pub fn watch(root: &Path, changes: broadcast::Sender<()>) -> anyhow::Result<Self> {
        let files = Mutex::new(Self::files(root)?);
        let (folder, told) = (root.to_path_buf(), changes.clone());
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let event = match event {
                    Ok(event) if !matches!(event.kind, EventKind::Access(_)) => event,
                    Ok(_) => return,
                    Err(e) => return warn!("watching: {e}"),
                };
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
                    let _ = told.send(());
                }
            })?;
        watcher
            .watch(root, RecursiveMode::Recursive)
            .with_context(|| root.display().to_string())?;
        Ok(Self {
            changes,
            _watcher: watcher,
        })
    }

    /// Listens for the site's changes.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.changes.subscribe()
    }

    fn files(root: &Path) -> anyhow::Result<BTreeSet<PathBuf>> {
        Ok(Builder::files(root, None)?.into_iter().collect())
    }

    /// Whether a path in the folder is the site's: one of its files, or a folder holding one, as when a
    /// folder is renamed.
    fn of_site(path: &Path, files: &BTreeSet<PathBuf>) -> bool {
        files.contains(path) || files.iter().any(|f| f.starts_with(path))
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

    /// Whether a change is told within a while.
    async fn told(changes: &mut broadcast::Receiver<()>) -> bool {
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
        let live = Live::watch(&root, broadcast::channel(16).0).unwrap();
        let mut changes = live.subscribe();

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
