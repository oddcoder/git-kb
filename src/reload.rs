//! Reloading: what git-kb reads once, as it begins, its settings and its plugins; and, for `git kb serve`, those
//! watched, so that it begins again with them when they change. The settings are the `git-kb.toml` found for the
//! folder served, or one put where it would be found first; the plugins, the repository's own, each its folder and
//! the files its options name.
//!
//! What changed is read before the server stops for it: only settings and plugins that load, and that differ from
//! those it runs with, have it begin again, so that an editor's own files beside them, or a comment added to the
//! settings, do not. Those that do not load are said, once, and read again every few seconds, as for a plugin's
//! folder made after the settings that name it, while the server goes on as it was.

use std::ffi::OsStr;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::config::Config;
use crate::git::Repo;
use crate::plugins::Plugins;

/// How long changes are let settle, as an editor saving writes a file more than once.
const SETTLE: Duration = Duration::from_millis(300);
/// How often what does not load is read again.
const RETRY: Duration = Duration::from_secs(3);

/// The settings and the plugins they name, loaded.
#[derive(Debug)]
pub struct Loaded {
    pub config: Config,
    /// The settings' file, if one was found.
    pub file: Option<PathBuf>,
    pub plugins: Plugins,
}

impl Loaded {
    /// The settings found for a folder, and the plugins they name, their commands' outputs kept in the repository's
    /// git folder.
    ///
    /// # Errors
    ///
    /// When the folder is not there, its settings cannot be read, or a plugin cannot be loaded.
    pub fn load(folder: &Path) -> anyhow::Result<Self> {
        let (config, file) = Config::find(folder)?;
        let cache = Repo::discover(&config.root)
            .map(|repo| repo.git().git_dir().join("git-kb").join("cache"));
        let plugins = Plugins::load(&config, file.as_deref(), cache)?;
        Ok(Self {
            config,
            file,
            plugins,
        })
    }

    /// What they are, to tell whether they changed: the settings as read, not as written, and the plugins'
    /// fingerprints.
    fn fingerprint(&self) -> String {
        format!(
            "{:?}\n{:?}\n{}",
            self.file,
            self.config,
            self.plugins.fingerprint()
        )
    }

    /// Where a server with these settings listens.
    fn address(&self) -> SocketAddr {
        SocketAddr::new(self.config.serve.host, self.config.serve.port)
    }
}

/// The settings and plugins a server began with, watched.
#[derive(Debug)]
pub struct Watch {
    folder: PathBuf,
    fingerprint: String,
    address: SocketAddr,
    changes: mpsc::UnboundedReceiver<()>,
    _watcher: RecommendedWatcher,
}

impl Watch {
    /// Watches what was loaded for a folder: each folder its settings are looked for in, from it up to where they
    /// were found, or to the repository's top; and its plugins' folders and files.
    ///
    /// # Errors
    ///
    /// When the folder is not there, or one of those cannot be watched.
    pub fn new(folder: &Path, loaded: &Loaded) -> anyhow::Result<Self> {
        let folder = std::fs::canonicalize(folder).with_context(|| folder.display().to_string())?;
        let found = loaded.file.as_deref().and_then(Path::parent);
        let mut looked = vec![];
        for dir in folder.ancestors() {
            looked.push(dir.to_path_buf());
            if found == Some(dir) || dir.join(".git").exists() {
                break;
            }
        }
        let sources: Vec<PathBuf> = loaded
            .plugins
            .sources()
            .filter_map(|source| std::fs::canonicalize(source).ok())
            .collect();
        let (tell, changes) = mpsc::unbounded_channel();
        let of_plugins = sources.clone();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let Ok(event) = event else { return };
                let ours = |path: &PathBuf| {
                    path.file_name() == Some(OsStr::new(Config::FILE))
                        || of_plugins.iter().any(|source| path.starts_with(source))
                };
                if !matches!(event.kind, EventKind::Access(_)) && event.paths.iter().any(ours) {
                    let _ = tell.send(());
                }
            })?;
        for dir in &looked {
            watcher
                .watch(dir, RecursiveMode::NonRecursive)
                .with_context(|| dir.display().to_string())?;
        }
        for source in &sources {
            let (path, mode) = if source.is_dir() {
                (source.as_path(), RecursiveMode::Recursive)
            } else {
                (
                    source.parent().unwrap_or(source),
                    RecursiveMode::NonRecursive,
                )
            };
            watcher
                .watch(path, mode)
                .with_context(|| path.display().to_string())?;
        }
        Ok(Self {
            folder,
            fingerprint: loaded.fingerprint(),
            address: loaded.address(),
            changes,
            _watcher: watcher,
        })
    }

    /// Once what it watches has changed, and loads, and differs from what the server began with.
    pub async fn changed(mut self) {
        let mut said: Option<String> = None;
        loop {
            if said.is_some() {
                let _ = tokio::time::timeout(RETRY, self.changes.recv()).await;
            } else if self.changes.recv().await.is_none() {
                return std::future::pending().await;
            }
            tokio::time::sleep(SETTLE).await;
            while self.changes.try_recv().is_ok() {}
            let folder = self.folder.clone();
            let now = tokio::task::spawn_blocking(move || Loaded::load(&folder))
                .await
                .map_err(anyhow::Error::from)
                .and_then(|loaded| self.ready(&loaded?));
            match now {
                Ok(true) => return,
                Ok(false) => {
                    if said.take().is_some() {
                        info!("settings and plugins load again, unchanged");
                    }
                }
                Err(e) => {
                    let e = format!("{e:#}");
                    if said.as_ref() != Some(&e) {
                        warn!("not reloaded, serving on as before: {e}");
                        said = Some(e);
                    }
                }
            }
        }
    }

    /// Whether settings and plugins loaded differ from the server's, and it can begin again with them: when they move
    /// it to another address, that that one is free.
    fn ready(&self, now: &Loaded) -> anyhow::Result<bool> {
        if now.fingerprint() == self.fingerprint {
            return Ok(false);
        }
        let address = now.address();
        if address != self.address {
            drop(std::net::TcpListener::bind(address).with_context(|| address.to_string())?);
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tokio::task::JoinHandle;

    use super::*;
    use crate::git::tests::Git;

    /// Whether watching has seen a change it would begin again for, within a while.
    async fn told(changed: &mut JoinHandle<()>) -> bool {
        tokio::time::timeout(Duration::from_secs(5), changed)
            .await
            .is_ok()
    }

    /// Whether it has not, after a while for it to settle and load.
    async fn not_told(changed: &JoinHandle<()>) -> bool {
        tokio::time::sleep(Duration::from_millis(1200)).await;
        !changed.is_finished()
    }

    #[tokio::test]
    async fn settings_changed_are_taken_once_they_load_and_differ() {
        let git = Git::new();
        let settings = git.top().join(Config::FILE);
        fs::write(&settings, "title = \"One\"\n").unwrap();
        let loaded = Loaded::load(&git.top()).unwrap();
        let mut changed = tokio::spawn(Watch::new(&git.top(), &loaded).unwrap().changed());

        fs::write(&settings, "# The site's name.\ntitle = \"One\"\n").unwrap();
        assert!(not_told(&changed).await, "a comment changes nothing");
        fs::write(&settings, "title = [\n").unwrap();
        assert!(not_told(&changed).await, "settings that do not load");
        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = taken.local_addr().unwrap().port();
        fs::write(&settings, format!("[serve]\nport = {port}\n")).unwrap();
        assert!(not_told(&changed).await, "an address in use");
        fs::write(&settings, "title = \"Two\"\n").unwrap();
        assert!(told(&mut changed).await, "settings that load and differ");
    }

    #[tokio::test]
    async fn settings_put_nearer_the_folder_or_a_plugin_changed_are_taken() {
        let git = Git::new();
        git.write("docs/a.md", "# A");
        git.write("kb-plugin/plugin.toml", "name = \"p\"\nblocks = [\"p\"]\n");
        git.write(
            Config::FILE,
            "root = \"docs\"\n[plugins.p]\npath = \"kb-plugin\"\n",
        );
        let docs = git.top().join("docs");
        let loaded = Loaded::load(&docs).unwrap();
        assert_eq!(loaded.file, Some(git.top().join(Config::FILE)));

        let mut changed = tokio::spawn(Watch::new(&docs, &loaded).unwrap().changed());
        git.write("kb-plugin/.plugin.toml.swp", "an editor's own");
        assert!(not_told(&changed).await, "a file that is not the plugin's");
        git.write("kb-plugin/draw.js", "draw();");
        assert!(told(&mut changed).await, "a file of the plugin");

        let mut changed = tokio::spawn(Watch::new(&docs, &loaded).unwrap().changed());
        git.write("docs/git-kb.toml", "title = \"Docs\"\n");
        assert!(told(&mut changed).await, "settings nearer the folder");
    }
}
