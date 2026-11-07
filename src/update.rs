//! Updates: `git kb serve` keeping itself up to date. Installed by `cargo install`, from crates.io or from a branch
//! of a git repository, git-kb looks there as it begins to serve, and every hour after, for a version numbered higher
//! than its own; finding one, it installs it as it was installed itself, with cargo, into the same place, while it
//! serves on. Once its binary is replaced, by that or by hand, it stops, as at Ctrl-C, answering the requests begun,
//! and runs the new binary in its place, with the same arguments, as `restart` does; open pages connect to it again,
//! and load again.
//!
//! `git config kb.autoupdate false` turns this off, for a repository, or with `--global` for every one; `git -c
//! kb.autoupdate=false kb serve`, for one server. git-kb that cargo did not install, as run from `target/`, never
//! updates. Installed from a path, a tag, a commit or with `--version`, it is not looked for anew, but a binary
//! installed over it is run.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime};

use anyhow::{Context, anyhow, bail};
use percent_encoding::percent_decode_str;
use semver::Version;
use serde::Deserialize;
use tokio::process::Command;
use tokio::time::MissedTickBehavior;
use tracing::{debug, info, warn};

/// The package cargo installs, and its binary.
const NAME: &str = env!("CARGO_PKG_NAME");

/// How often it looks for a newer version where it came from.
const LOOK_EVERY: Duration = Duration::from_hours(1);
/// How often it looks whether its binary was replaced; and how long a new one is left alone before it is run, so
/// that whatever wrote it is done.
const WATCH_EVERY: Duration = Duration::from_secs(5);
/// How long asking crates.io or a repository may take, and installing.
const ASKING: Duration = Duration::from_mins(2);
const INSTALLING: Duration = Duration::from_hours(1);

/// crates.io's index, by git and by HTTP, as cargo names where a package came from.
const CRATES_IO: [&str; 2] = [
    "registry+https://github.com/rust-lang/crates.io-index",
    "sparse+https://index.crates.io/",
];

/// Where cargo installed git-kb from.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Source {
    CratesIo,
    /// A branch of a git repository, its default one if none is named; and the commit installed.
    Git {
        url: String,
        branch: Option<String>,
        commit: String,
    },
    /// Anywhere it is not looked for anew: a path, a tag, a commit, another registry, or a version asked for.
    Fixed,
}

impl Source {
    /// Where a package came from, as cargo names it: `registry+https://github.com/rust-lang/crates.io-index`,
    /// `git+https://github.com/oddcoder/git-kb?branch=dev#3f2a1c9…` or `path+file:///home/ada/git-kb`.
    fn parse(id: &str) -> Self {
        if CRATES_IO.contains(&id) {
            return Self::CratesIo;
        }
        let Some((place, commit)) = id.strip_prefix("git+").and_then(|git| git.rsplit_once('#'))
        else {
            return Self::Fixed;
        };
        let (url, query) = place.split_once('?').unwrap_or((place, ""));
        let mut branch = None;
        for pair in query.split('&').filter(|pair| !pair.is_empty()) {
            match pair.split_once('=') {
                Some(("branch", name)) => {
                    branch = Some(percent_decode_str(name).decode_utf8_lossy().into_owned());
                }
                // A tag or a commit, which does not move.
                _ => return Self::Fixed,
            }
        }
        Self::Git {
            url: url.to_owned(),
            branch,
            commit: commit.to_owned(),
        }
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CratesIo => f.write_str("crates.io"),
            Self::Git {
                url,
                branch: Some(branch),
                ..
            } => write!(f, "{url}, branch {branch}"),
            Self::Git { url, .. } => f.write_str(url),
            Self::Fixed => f.write_str("where it was installed from"),
        }
    }
}

/// git-kb as cargo installed it, as its record of what it installed says: `.crates2.json`, in the folder it installed
/// into.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Installed {
    version: Version,
    source: Source,
    features: Vec<String>,
    all_features: bool,
    no_default_features: bool,
    profile: String,
}

/// cargo's record of what it installed, each package by its name, version and source.
#[derive(Deserialize)]
struct Tracker {
    #[serde(default)]
    installs: BTreeMap<String, Record>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Record {
    version_req: Option<String>,
    bins: Vec<String>,
    features: Vec<String>,
    all_features: bool,
    no_default_features: bool,
    profile: Option<String>,
}

impl Installed {
    /// git-kb's record in an install root, if cargo installed it there, as the binary named `bin`.
    fn read(root: &Path, bin: &str) -> anyhow::Result<Option<Self>> {
        let file = root.join(".crates2.json");
        match fs::read_to_string(&file) {
            Ok(text) => Self::parse(&text, bin).with_context(|| file.display().to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(anyhow!(e).context(file.display().to_string())),
        }
    }

    fn parse(text: &str, bin: &str) -> anyhow::Result<Option<Self>> {
        let tracker: Tracker = serde_json::from_str(text)?;
        for (key, record) in tracker.installs {
            // `git-kb 0.1.1 (registry+https://github.com/rust-lang/crates.io-index)`
            let mut parts = key.splitn(3, ' ');
            let (Some(NAME), Some(version), Some(source)) =
                (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            if !record.bins.iter().any(|b| b == bin) {
                continue;
            }
            let source = source.trim_start_matches('(').trim_end_matches(')');
            return Ok(Some(Self {
                version: Version::parse(version).with_context(|| key.clone())?,
                source: if record.version_req.is_some() {
                    Source::Fixed
                } else {
                    Source::parse(source)
                },
                features: record.features,
                all_features: record.all_features,
                no_default_features: record.no_default_features,
                profile: record.profile.unwrap_or_else(|| "release".into()),
            }));
        }
        Ok(None)
    }
}

/// A server's updates: the binary it runs, where cargo installed it from and into, and what it found of newer
/// versions.
#[derive(Debug)]
pub struct Updates {
    /// The folder served, whose repository's git config may turn updating off.
    folder: PathBuf,
    /// The binary, and when it was written: the time it is running, and a new time it was seen at, waited on.
    exe: PathBuf,
    written: SystemTime,
    waiting: Option<SystemTime>,
    /// Where cargo installed it, the folder above its `bin/`, and how.
    root: PathBuf,
    installed: Installed,
    /// What is run as cargo.
    cargo: OsString,
    /// The newest version found that installing did not bring, which is not tried again.
    tried: Option<Version>,
    /// A branch's last commit, as last looked at, and the version it has.
    seen: Option<(String, Version)>,
    /// Whether looking failed last time, so that it is said once, and not each hour.
    failing: bool,
}

impl Updates {
    /// The updates of the binary running, to serve a folder; none if cargo did not install it, or on Windows, which
    /// does not let a running binary be replaced.
    #[must_use]
    pub fn find(folder: &Path) -> Option<Self> {
        if !cfg!(unix) {
            return None;
        }
        let exe = std::env::current_exe().and_then(fs::canonicalize).ok()?;
        let bin = exe.file_name()?.to_str()?;
        let root = exe.parent().filter(|dir| dir.ends_with("bin"))?.parent()?;
        let installed = match Installed::read(root, bin) {
            Ok(Some(installed)) if installed.version.to_string() == env!("CARGO_PKG_VERSION") => {
                installed
            }
            Ok(_) => {
                debug!(exe = %exe.display(), "not installed by cargo: not updating");
                return None;
            }
            Err(e) => {
                warn!("not updating: {e:#}");
                return None;
            }
        };
        let written = fs::metadata(&exe).and_then(|m| m.modified()).ok()?;
        Some(Self {
            folder: folder.to_path_buf(),
            written,
            waiting: None,
            root: root.to_path_buf(),
            installed,
            cargo: "cargo".into(),
            tried: None,
            seen: None,
            failing: false,
            exe,
        })
    }

    /// Looks for newer versions, and installs them, until its binary is replaced; then says the version it is now.
    pub async fn next(mut self) -> String {
        if self.installed.source != Source::Fixed && enabled(&self.folder).await {
            info!(from = %self.installed.source, "updating itself, unless git config kb.autoupdate is false");
        }
        let mut look = tokio::time::interval(LOOK_EVERY);
        let mut watch = tokio::time::interval(WATCH_EVERY);
        look.set_missed_tick_behavior(MissedTickBehavior::Delay);
        watch.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = watch.tick() => {}
                _ = look.tick() => self.look().await,
            }
            if let Some(version) = self.replaced().await {
                return version;
            }
        }
    }

    /// Looks for a newer version where it came from, and installs it if there is one.
    async fn look(&mut self) {
        if self.installed.source == Source::Fixed || !enabled(&self.folder).await {
            return;
        }
        let newer = match self.newest().await {
            Ok(newest) => {
                self.failing = false;
                match newest {
                    Some(v) if v > self.installed.version && self.tried.as_ref() != Some(&v) => v,
                    _ => return,
                }
            }
            Err(e) if self.failing => return debug!("not looked for a newer version: {e:#}"),
            Err(e) => {
                self.failing = true;
                return warn!("not looked for a newer version: {e:#}");
            }
        };
        info!(version = %newer, from = %self.installed.source, "a newer version is out: installing it");
        if let Err(e) = self.install().await {
            warn!(version = %newer, "not installed: {e:#}");
        }
        self.tried = Some(newer);
    }

    /// The newest version where it came from: crates.io's, or the version the branch's last commit has, unless that
    /// is the commit installed.
    async fn newest(&mut self) -> anyhow::Result<Option<Version>> {
        match &self.installed.source {
            Source::CratesIo => {
                let mut cargo = self.cargo();
                cargo.args(["search", NAME, "--limit", "1"]);
                let found = run(&mut cargo, ASKING, "cargo search").await?;
                Ok(searched(&found))
            }
            Source::Git {
                url,
                branch,
                commit,
            } => {
                let reference = branch
                    .as_ref()
                    .map_or_else(|| "HEAD".to_owned(), |b| format!("refs/heads/{b}"));
                let mut git = self.git();
                git.args(["ls-remote", "--", url, &reference]);
                let heads = run(&mut git, ASKING, "git ls-remote").await?;
                let Some(head) = heads.split_whitespace().next() else {
                    bail!("{url} has no {reference}");
                };
                if head == commit {
                    return Ok(None);
                }
                if let Some((seen, version)) = &self.seen
                    && seen == head
                {
                    return Ok(Some(version.clone()));
                }
                let version = self.version_at(&url.clone(), &reference).await?;
                self.seen = Some((head.to_owned(), version.clone()));
                Ok(Some(version))
            }
            Source::Fixed => Ok(None),
        }
    }

    /// The version a branch's last commit has, by its `Cargo.toml`: fetched alone, into a repository made for it
    /// beside the binaries, and removed after.
    async fn version_at(&self, url: &str, reference: &str) -> anyhow::Result<Version> {
        let scratch = self
            .root
            .join(format!(".{NAME}-update-{}", std::process::id()));
        let _ = fs::remove_dir_all(&scratch);
        let version = async {
            let mut git = self.git();
            git.args(["init", "-q", "--bare"]).arg(&scratch);
            run(&mut git, ASKING, "git init").await?;
            let mut git = self.git();
            git.arg("-C").arg(&scratch).args([
                "fetch",
                "-q",
                "--depth",
                "1",
                "--no-tags",
                "--",
                url,
                reference,
            ]);
            run(&mut git, ASKING, "git fetch").await?;
            let mut git = self.git();
            git.arg("-C")
                .arg(&scratch)
                .args(["show", "FETCH_HEAD:Cargo.toml"]);
            manifest_version(&run(&mut git, ASKING, "git show").await?)
        }
        .await;
        let _ = fs::remove_dir_all(&scratch);
        version
    }

    /// Installs the newest version as this one was installed: from the same place, with the same features and
    /// profile, into the same folder.
    async fn install(&self) -> anyhow::Result<()> {
        let mut cargo = self.cargo();
        cargo
            .args(["install", "--locked", "--root"])
            .arg(&self.root);
        cargo.args(self.install_args());
        run(&mut cargo, INSTALLING, "cargo install").await.map(drop)
    }

    /// What `cargo install` is told, after where to install, to install as this one was.
    fn install_args(&self) -> Vec<String> {
        let installed = &self.installed;
        let mut args = vec![];
        if let Source::Git { url, branch, .. } = &installed.source {
            args.extend(["--git".into(), url.clone()]);
            if let Some(branch) = branch {
                args.extend(["--branch".into(), branch.clone()]);
            }
        }
        if !installed.features.is_empty() {
            args.extend(["--features".into(), installed.features.join(",")]);
        }
        if installed.all_features {
            args.push("--all-features".into());
        }
        if installed.no_default_features {
            args.push("--no-default-features".into());
        }
        if installed.profile != "release" {
            args.extend(["--profile".into(), installed.profile.clone()]);
        }
        args.push(NAME.into());
        args
    }

    /// The version of the binary installed in place of this one, by updating or by hand, once it has been left alone
    /// a while, so that whatever wrote it is done, and it runs.
    async fn replaced(&mut self) -> Option<String> {
        let written = fs::metadata(&self.exe).and_then(|m| m.modified()).ok()?;
        if written == self.written {
            return None;
        }
        if self.waiting.replace(written) != Some(written) {
            return None;
        }
        self.written = written;
        let mut asked = Command::new(&self.exe);
        asked.arg("-V");
        let said = match run(&mut asked, ASKING, "the new binary").await {
            Ok(said) => said,
            Err(e) => {
                warn!("a new binary is installed, but does not run: {e:#}");
                return None;
            }
        };
        let version = said.trim().trim_start_matches(NAME).trim().to_owned();
        if !enabled(&self.folder).await {
            info!(
                %version,
                "a new binary is installed, not run: git config kb.autoupdate is false"
            );
            return None;
        }
        Some(version)
    }

    /// cargo, run in the folder it installs into, so that only its own settings apply, not a repository's.
    fn cargo(&self) -> Command {
        let mut cargo = Command::new(&self.cargo);
        cargo
            .current_dir(&self.root)
            .env("CARGO_TERM_COLOR", "never");
        cargo
    }

    /// git, run on no repository but those named, and never waiting on a prompt, as the comments' sync runs it.
    fn git(&self) -> Command {
        let ssh =
            std::env::var("GIT_SSH_COMMAND").unwrap_or_else(|_| "ssh -o BatchMode=yes".into());
        let mut git = Command::new("git");
        git.current_dir(&self.root)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_SSH_COMMAND", ssh);
        git
    }
}

/// Whether git config, of the folder's repository or of everyone's, lets git-kb update: unless `kb.autoupdate` is
/// false.
async fn enabled(folder: &Path) -> bool {
    let said = Command::new("git")
        .args(["config", "--type=bool", "--get", "kb.autoupdate"])
        .current_dir(folder)
        .stdin(Stdio::null())
        .output()
        .await;
    !matches!(said, Ok(out) if out.stdout.trim_ascii() == b"false")
}

/// What a command writes on its output, if it succeeds in time; or else why not, with the last of what it wrote on
/// its errors.
async fn run(command: &mut Command, within: Duration, what: &str) -> anyhow::Result<String> {
    command.stdin(Stdio::null()).kill_on_drop(true);
    let out = tokio::time::timeout(within, command.output())
        .await
        .map_err(|_| anyhow!("{what} took over {} minutes", within.as_secs() / 60))?
        .with_context(|| format!("running {what}"))?;
    if !out.status.success() {
        let said = String::from_utf8_lossy(&out.stderr);
        let lines: Vec<&str> = said.lines().filter(|l| !l.trim().is_empty()).collect();
        let last = lines[lines.len().saturating_sub(5)..].join("\n");
        bail!("{what} failed, {}:\n{last}", out.status);
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// git-kb's version, as `cargo search` finds it: `git-kb = "0.1.1"    # A knowledge base…`.
fn searched(found: &str) -> Option<Version> {
    found.lines().find_map(|line| {
        let (name, rest) = line.split_once(" = \"")?;
        let (version, _) = rest.split_once('"')?;
        if name == NAME {
            Version::parse(version).ok()
        } else {
            None
        }
    })
}

/// git-kb's version, as its `Cargo.toml` says.
fn manifest_version(manifest: &str) -> anyhow::Result<Version> {
    let manifest: toml::Table = toml::from_str(manifest).context("Cargo.toml")?;
    let package = manifest
        .get("package")
        .and_then(toml::Value::as_table)
        .filter(|package| package.get("name").and_then(toml::Value::as_str) == Some(NAME))
        .ok_or_else(|| anyhow!("Cargo.toml is not {NAME}'s"))?;
    let version = package
        .get("version")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| anyhow!("Cargo.toml has no version"))?;
    Ok(Version::parse(version)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracker(key: &str, record: &str) -> String {
        format!(
            r#"{{"installs":{{"ripgrep 15.2.0 (registry+https://github.com/rust-lang/crates.io-index)":{{"bins":["rg"]}},"{key}":{record}}}}}"#
        )
    }

    const PLAIN: &str = r#"{"version_req":null,"bins":["git-kb"],"features":[],"all_features":false,"no_default_features":false,"profile":"release","target":"x86_64-unknown-linux-gnu","rustc":"rustc 1.99.1"}"#;

    fn source(of: &str) -> Source {
        Installed::parse(&tracker(&format!("git-kb 0.1.1 ({of})"), PLAIN), "git-kb")
            .unwrap()
            .unwrap()
            .source
    }

    #[test]
    fn cargo_s_record_says_what_it_installed() {
        let installed = Installed::parse(
            &tracker(
                "git-kb 0.1.1 (registry+https://github.com/rust-lang/crates.io-index)",
                PLAIN,
            ),
            "git-kb",
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            installed,
            Installed {
                version: Version::new(0, 1, 1),
                source: Source::CratesIo,
                features: vec![],
                all_features: false,
                no_default_features: false,
                profile: "release".into(),
            }
        );
    }

    #[test]
    fn crates_io_is_known_by_either_index() {
        assert_eq!(
            source("registry+https://github.com/rust-lang/crates.io-index"),
            Source::CratesIo
        );
        assert_eq!(source("sparse+https://index.crates.io/"), Source::CratesIo);
        assert_eq!(
            source("sparse+https://my.registry.example/index/"),
            Source::Fixed
        );
    }

    #[test]
    fn a_branch_is_looked_at_but_a_tag_or_a_commit_is_not() {
        assert_eq!(
            source(
                "git+https://github.com/oddcoder/git-kb#0336238c12aaab57782f08ce924a17a50f7e6195"
            ),
            Source::Git {
                url: "https://github.com/oddcoder/git-kb".into(),
                branch: None,
                commit: "0336238c12aaab57782f08ce924a17a50f7e6195".into(),
            }
        );
        assert_eq!(
            source("git+https://github.com/oddcoder/git-kb?branch=ada%2Fdev#0336238"),
            Source::Git {
                url: "https://github.com/oddcoder/git-kb".into(),
                branch: Some("ada/dev".into()),
                commit: "0336238".into(),
            }
        );
        assert_eq!(
            source("git+https://github.com/oddcoder/git-kb?tag=v0.1.1#0336238"),
            Source::Fixed
        );
        assert_eq!(
            source("git+https://github.com/oddcoder/git-kb?rev=0336238#0336238"),
            Source::Fixed
        );
        assert_eq!(source("path+file:///home/ada/git-kb"), Source::Fixed);
    }

    #[test]
    fn a_version_asked_for_is_kept() {
        let record = PLAIN.replace(r#""version_req":null"#, r#""version_req":"=0.1.1""#);
        let installed = Installed::parse(
            &tracker(
                "git-kb 0.1.1 (registry+https://github.com/rust-lang/crates.io-index)",
                &record,
            ),
            "git-kb",
        )
        .unwrap()
        .unwrap();
        assert_eq!(installed.source, Source::Fixed);
    }

    #[test]
    fn only_git_kb_s_own_binary_is_found() {
        let text = tracker(
            "git-kb 0.1.1 (registry+https://github.com/rust-lang/crates.io-index)",
            PLAIN,
        );
        assert_eq!(Installed::parse(&text, "rg").unwrap(), None);
        assert_eq!(Installed::parse(&text, "git-kb-dev").unwrap(), None);
        assert_eq!(
            Installed::parse(r#"{"installs":{}}"#, "git-kb").unwrap(),
            None
        );
        assert!(Installed::parse("not json", "git-kb").is_err());
    }

    #[test]
    fn it_installs_as_it_was_installed() {
        let mut updates = Updates {
            folder: ".".into(),
            exe: "/home/ada/.cargo/bin/git-kb".into(),
            written: SystemTime::UNIX_EPOCH,
            waiting: None,
            root: "/home/ada/.cargo".into(),
            installed: Installed {
                version: Version::new(0, 1, 1),
                source: Source::CratesIo,
                features: vec![],
                all_features: false,
                no_default_features: false,
                profile: "release".into(),
            },
            cargo: "cargo".into(),
            tried: None,
            seen: None,
            failing: false,
        };
        assert_eq!(updates.install_args(), ["git-kb"]);
        updates.installed = Installed {
            version: Version::new(0, 1, 1),
            source: Source::Git {
                url: "https://github.com/oddcoder/git-kb".into(),
                branch: Some("dev".into()),
                commit: "0336238".into(),
            },
            features: vec!["a".into(), "b".into()],
            all_features: false,
            no_default_features: true,
            profile: "dev".into(),
        };
        assert_eq!(
            updates.install_args(),
            [
                "--git",
                "https://github.com/oddcoder/git-kb",
                "--branch",
                "dev",
                "--features",
                "a,b",
                "--no-default-features",
                "--profile",
                "dev",
                "git-kb"
            ]
        );
    }

    #[test]
    fn cargo_search_finds_its_version() {
        let found = "git-kb = \"0.2.0\"    # A knowledge base of Markdown in git\n\
                     note: to learn more about a package, run `cargo info <name>`\n";
        assert_eq!(searched(found), Some(Version::new(0, 2, 0)));
        assert_eq!(searched("git-kbx = \"9.0.0\"    # Another\n"), None);
        assert_eq!(searched(""), None);
    }

    #[test]
    fn a_manifest_says_its_version() {
        let manifest = "[package]\nname = \"git-kb\"\nversion = \"0.3.0-rc.1\"\n";
        assert_eq!(
            manifest_version(manifest).unwrap(),
            Version::parse("0.3.0-rc.1").unwrap()
        );
        assert!(manifest_version("[package]\nname = \"other\"\nversion = \"1.0.0\"\n").is_err());
        assert!(manifest_version("[workspace]\nmembers = []\n").is_err());
    }
}
