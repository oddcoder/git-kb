//! `git-kb.toml`: which folder of Markdown is shown, and how it is served and built. It sits in the folder
//! or any folder above it, up to the top of its git repository, and the nearest one is used. Without one,
//! the folder itself is shown, and every setting is its default. Paths in it are relative to the folder it
//! is in.
//!
//! ```toml
//! root = "docs"          # the folder of Markdown; the file's own by default. Before any [table], as
//!                        # TOML has it.
//! title = "pentasil"     # the site's name, in its bar and its pages' titles; the root folder's by default
//! url = "https://foobar.xyz/docs/"  # where the built site lives, for its pages' canonical addresses;
//!                        # none by default
//!
//! [serve]
//! host = "127.0.0.1"     # the address to listen on; only this machine by default
//! port = 8000
//!
//! [build]
//! out = "target/git-kb"  # where the site is written
//! ```

use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::Deserialize;
use tracing::{debug, trace};

/// The settings for a folder of Markdown.
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// The folder of Markdown shown, as a full path once read.
    pub root: PathBuf,
    /// The site's name; the root folder's when none is given.
    pub title: Option<String>,
    /// Where the built site lives, ending in `/` once read, as `https://foobar.xyz/docs/`.
    pub url: Option<String>,
    pub serve: ServeConfig,
    pub build: BuildConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            root: ".".into(),
            title: None,
            url: None,
            serve: ServeConfig::default(),
            build: BuildConfig::default(),
        }
    }
}

/// How the folder is served.
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ServeConfig {
    pub host: IpAddr,
    pub port: u16,
}

impl Default for ServeConfig {
    fn default() -> Self {
        Self {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 8000,
        }
    }
}

/// How the folder is built.
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct BuildConfig {
    pub out: PathBuf,
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            out: "target/git-kb".into(),
        }
    }
}

impl Config {
    /// The file the settings are in.
    pub const FILE: &str = "git-kb.toml";

    /// The settings for a folder, and the file they are from: the nearest `git-kb.toml` in it or above
    /// it, up to the top of its repository, the first folder holding `.git`; or every default, and no
    /// file, if there is none. Paths are taken against the folder the file is in, or the folder itself
    /// when there is none.
    ///
    /// # Errors
    ///
    /// When the folder cannot be found, or a file found cannot be read or is not valid settings.
    pub fn find(folder: &Path) -> anyhow::Result<(Self, Option<PathBuf>)> {
        // As a full path, so that the search goes on above it: "docs" has no folders above it.
        let folder =
            &std::fs::canonicalize(folder).with_context(|| folder.display().to_string())?;
        for dir in folder.ancestors() {
            let file = dir.join(Self::FILE);
            trace!(file = %file.display(), "looking for settings");
            if file.is_file() {
                return Ok((Self::load(&file)?, Some(file)));
            }
            if dir.join(".git").exists() {
                trace!(repository = %dir.display(), "no settings up to the top of the repository");
                break;
            }
        }
        let mut config = Self::default();
        config.resolve(folder)?;
        Ok((config, None))
    }

    /// The settings in a file, with its paths taken against the folder it is in.
    ///
    /// # Errors
    ///
    /// When the file cannot be read, or is not valid settings.
    pub fn load(file: &Path) -> anyhow::Result<Self> {
        let in_file = || file.display().to_string();
        let text = std::fs::read_to_string(file).with_context(in_file)?;
        let mut config: Self = toml::from_str(&text).with_context(in_file)?;
        config
            .resolve(file.parent().unwrap_or(Path::new("")))
            .with_context(in_file)?;
        debug!(file = %file.display(), ?config, "read settings");
        Ok(config)
    }

    /// Its paths taken against a folder.
    fn resolve(&mut self, base: &Path) -> anyhow::Result<()> {
        if let Some(url) = &mut self.url {
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                anyhow::bail!("url {url}: an address starts with https:// or http://");
            }
            if !url.ends_with('/') {
                url.push('/');
            }
        }
        let root = base.join(&self.root);
        self.root = std::fs::canonicalize(&root)
            .with_context(|| format!("the root folder, {}", root.display()))?;
        self.build.out = base.join(&self.build.out);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// A folder for a test, made in a temporary one, with these files, which is removed after.
    struct Tree(tempfile::TempDir);

    impl Tree {
        fn with(files: &[(&str, &str)]) -> Self {
            let dir = tempfile::tempdir().unwrap();
            for (path, text) in files {
                let path = dir.path().join(path);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, text).unwrap();
            }
            Self(dir)
        }

        /// A path in it, by its full path, as the search gives them: on macOS, /var is /private/var.
        fn path(&self, rest: &str) -> PathBuf {
            fs::canonicalize(self.0.path()).unwrap().join(rest)
        }
    }

    #[test]
    fn without_a_file_every_setting_is_its_default() {
        let tree = Tree::with(&[(".git/HEAD", ""), ("docs/a.md", "")]);
        let (config, file) = Config::find(&tree.path("docs")).unwrap();
        assert_eq!(file, None);
        assert_eq!(
            config.serve,
            ServeConfig {
                host: [127, 0, 0, 1].into(),
                port: 8000
            }
        );
        assert_eq!(config.build.out, tree.path("docs/target/git-kb"));
    }

    #[test]
    fn settings_are_read_from_the_file() {
        let tree = Tree::with(&[(
            "git-kb.toml",
            "[serve]\nhost = \"0.0.0.0\"\nport = 9000\n[build]\nout = \"site\"\n",
        )]);
        let (config, file) = Config::find(&tree.path("")).unwrap();
        assert_eq!(file, Some(tree.path("git-kb.toml")));
        assert_eq!(
            config.serve,
            ServeConfig {
                host: [0, 0, 0, 0].into(),
                port: 9000
            }
        );
        assert_eq!(config.build.out, tree.path("site"));
    }

    #[test]
    fn settings_left_out_are_their_defaults() {
        let tree = Tree::with(&[("git-kb.toml", "[serve]\nport = 9000\n")]);
        let (config, _) = Config::find(&tree.path("")).unwrap();
        assert_eq!(
            config.serve,
            ServeConfig {
                host: [127, 0, 0, 1].into(),
                port: 9000
            }
        );
        assert_eq!(config.build.out, tree.path("target/git-kb"));
    }

    #[test]
    fn the_nearest_file_above_the_folder_is_used_and_its_paths_are_its_own() {
        let tree = Tree::with(&[
            ("git-kb.toml", "[build]\nout = \"site\"\n"),
            ("docs/arch/a.md", ""),
        ]);
        let (config, file) = Config::find(&tree.path("docs/arch")).unwrap();
        assert_eq!(file, Some(tree.path("git-kb.toml")));
        assert_eq!(config.build.out, tree.path("site"));
    }

    #[test]
    fn a_folder_that_is_not_there_is_an_error() {
        let tree = Tree::with(&[]);
        assert!(Config::find(&tree.path("missing")).is_err());
    }

    #[test]
    fn the_search_stops_at_the_top_of_the_repository() {
        let tree = Tree::with(&[
            ("git-kb.toml", "[serve]\nport = 9000\n"),
            ("repo/.git/HEAD", ""),
            ("repo/docs/a.md", ""),
        ]);
        assert_eq!(Config::find(&tree.path("repo/docs")).unwrap().1, None);
    }

    #[test]
    fn an_unknown_setting_is_refused_by_name() {
        let tree = Tree::with(&[("git-kb.toml", "[serve]\nprot = 9000\n")]);
        let error = format!("{:#}", Config::find(&tree.path("")).unwrap_err());
        assert!(
            error.contains("git-kb.toml") && error.contains("prot"),
            "{error}"
        );
    }

    #[test]
    fn a_bad_value_is_refused() {
        for text in ["[serve]\nport = 70000\n", "[serve]\nhost = \"localhost\"\n"] {
            let tree = Tree::with(&[("git-kb.toml", text)]);
            assert!(Config::find(&tree.path("")).is_err(), "{text}");
        }
    }

    #[test]
    fn without_a_file_the_folder_is_the_root() {
        let tree = Tree::with(&[(".git/HEAD", ""), ("docs/a.md", "")]);
        let (config, _) = Config::find(&tree.path("docs")).unwrap();
        assert_eq!(config.root, tree.path("docs"));
    }

    #[test]
    fn the_root_is_the_file_s_folder_or_the_one_it_names() {
        let tree = Tree::with(&[("git-kb.toml", ""), ("docs/arch/a.md", "")]);
        assert_eq!(
            Config::find(&tree.path("docs/arch")).unwrap().0.root,
            tree.path("")
        );
        let tree = Tree::with(&[("git-kb.toml", "root = \"docs\"\n"), ("docs/arch/a.md", "")]);
        assert_eq!(
            Config::find(&tree.path("docs/arch")).unwrap().0.root,
            tree.path("docs")
        );
    }

    #[test]
    fn a_root_that_is_not_there_is_an_error() {
        let tree = Tree::with(&[("git-kb.toml", "root = \"missing\"\n")]);
        let error = format!("{:#}", Config::find(&tree.path("")).unwrap_err());
        assert!(
            error.contains("the root folder") && error.contains("missing"),
            "{error}"
        );
    }

    #[test]
    fn the_site_s_title_is_read() {
        let tree = Tree::with(&[(
            "git-kb.toml",
            "title = \"pentasil\"
",
        )]);
        let (config, _) = Config::find(&tree.path("")).unwrap();
        assert_eq!(config.title.as_deref(), Some("pentasil"));
        let tree = Tree::with(&[(
            "git-kb.toml",
            "[site]
title = \"pentasil\"
",
        )]);
        assert!(Config::find(&tree.path("")).is_err(), "there is no [site]");
    }

    #[test]
    fn the_site_s_url_is_read_and_ends_with_a_slash() {
        let tree = Tree::with(&[("git-kb.toml", "url = \"https://foobar.xyz/docs\"\n")]);
        let (config, _) = Config::find(&tree.path("")).unwrap();
        assert_eq!(config.url.as_deref(), Some("https://foobar.xyz/docs/"));
        let tree = Tree::with(&[("git-kb.toml", "url = \"foobar.xyz\"\n")]);
        let error = format!("{:#}", Config::find(&tree.path("")).unwrap_err());
        assert!(error.contains("https://"), "{error}");
    }
}
