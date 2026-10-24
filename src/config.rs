//! `git-kb.toml`: which folder of Markdown is shown, and how it is served and built. It sits in the folder
//! or any folder above it, up to the top of its git repository, and the nearest one is used. Without one,
//! the folder itself is shown, and every setting is its default. Paths in it are relative to the folder it
//! is in.
//!
//! ```toml
//! root = "docs"          # the folder of Markdown; the file's own by default. Before any [table], as
//!                        # TOML has it.
//! title = "pentasil"     # the site's name, in its bar and its pages' titles; the root folder's by default
//! url = "https://foobar.xyz/kb/"  # where the built site's top lives, for its pages' canonical addresses;
//!                        # none by default
//!
//! [serve]
//! host = "127.0.0.1"     # the address to listen on; only this machine by default
//! port = 8000
//! sync-every = 5         # minutes between syncs of the comments with every remote; 0 for never
//!
//! [build]
//! out = "target/git-kb"  # where the site is written
//!
//! [comments]
//! namespace = "refs/git-kb/comments"  # each person's comments are <namespace>/<their email>
//!
//! [plugins.graphviz]     # a plugin, by name: a built-in one, or, by path, a folder with a plugin.toml;
//! path = "tools/kb-graphviz"  # its options go in its table. See plugins.rs for what a plugin is.
//!
//!
//! [look]                 # how the pages look until a reader chooses in the Aa menu; web/src/look.ts's
//! scheme = "auto"        # light, dark, or auto, as the reader's system is
//! palette = "github"     # github, paper, solarized, nord or gruvbox
//! size = 16              # the text's size, in pixels: 14, 15, 16, 18 or 20
//! font = "sans"          # the text's font: sans, serif or mono
//! code = "system"        # code's font: system, menlo or courier
//! ```

use std::collections::BTreeMap;
use std::fmt::Write;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::Deserialize;
use tracing::{debug, trace};

use crate::build::Builder;

/// The settings for a folder of Markdown.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// The folder of Markdown shown, as a full path once read.
    pub root: PathBuf,
    /// The site's name; the root folder's when none is given.
    pub title: Option<String>,
    /// Where the built site's top lives, ending in `/` once read, as `https://foobar.xyz/kb/`.
    pub url: Option<String>,
    pub serve: ServeConfig,
    pub build: BuildConfig,
    pub comments: CommentsConfig,
    pub look: LookConfig,
    /// Each plugin by name, with its options; a `path` names a plugin of the repository's own.
    pub plugins: BTreeMap<String, toml::Table>,
    /// The folder the settings' paths are taken against: the file's, or the folder's when there is none.
    #[serde(skip)]
    pub base: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            root: ".".into(),
            title: None,
            url: None,
            serve: ServeConfig::default(),
            build: BuildConfig::default(),
            comments: CommentsConfig::default(),
            look: LookConfig::default(),
            plugins: BTreeMap::new(),
            base: PathBuf::new(),
        }
    }
}

/// Where the review comments are kept.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct CommentsConfig {
    /// The refs each person's comments are under, by their email.
    pub namespace: String,
}

impl Default for CommentsConfig {
    fn default() -> Self {
        Self {
            namespace: "refs/git-kb/comments".into(),
        }
    }
}

/// How the folder is served.
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ServeConfig {
    pub host: IpAddr,
    pub port: u16,
    /// Minutes between syncs of the review comments with every remote; 0 for never.
    #[serde(rename = "sync-every")]
    pub sync_every: u64,
}

impl Default for ServeConfig {
    fn default() -> Self {
        Self {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 8000,
            sync_every: 5,
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

/// How the pages look until a reader chooses. git-kb only hands it to them, each part given as an attribute of
/// their `<html>`; what each may be, and is when not given, is the browser's: `web/src/look.ts` and
/// `assets/style.css`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct LookConfig {
    pub scheme: Option<String>,
    pub palette: Option<String>,
    pub size: Option<u16>,
    pub font: Option<String>,
    pub code: Option<String>,
}

impl LookConfig {
    /// The parts given, as attributes of `<html>`: ` data-palette="nord"`.
    #[must_use]
    pub fn attributes(&self) -> String {
        let size = self.size.map(|size| size.to_string());
        let parts = [
            ("scheme", &self.scheme),
            ("palette", &self.palette),
            ("size", &size),
            ("font", &self.font),
            ("code", &self.code),
        ];
        let mut attributes = String::new();
        for (key, value) in parts {
            if let Some(value) = value {
                let _ = write!(attributes, " data-{key}=\"{}\"", Builder::escape(value));
            }
        }
        attributes
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
        self.base = base.to_path_buf();
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
                port: 8000,
                ..ServeConfig::default()
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
                port: 9000,
                ..ServeConfig::default()
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
                port: 9000,
                ..ServeConfig::default()
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

    #[test]
    fn the_look_is_read_and_handed_to_the_pages() {
        let tree = Tree::with(&[(
            "git-kb.toml",
            "[look]\npalette = \"nord\"\nsize = 18\nfont = \"a\\\"b\"\n",
        )]);
        let (config, _) = Config::find(&tree.path("")).unwrap();
        assert_eq!(
            config.look.attributes(),
            r#" data-palette="nord" data-size="18" data-font="a&quot;b""#
        );
        assert_eq!(LookConfig::default().attributes(), "");
    }
}
