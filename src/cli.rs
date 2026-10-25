//! The command line: what git-kb is asked to do, parsed; `main` does it. Installed on the PATH, git-kb
//! is also git's own command, so `git kb serve` runs `git-kb serve`, and `git kb build` runs
//! `git-kb build`.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// A knowledge base of Markdown in git.
///
/// Serve a folder of Markdown as a site to read, or build it as a static site.
#[derive(Debug, Parser, PartialEq, Eq)]
#[command(name = "git-kb", version, propagate_version = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

/// What git-kb is asked to do. Each says it is git-kb, not git-kb-serve, when asked its version.
#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum Command {
    /// Serve a folder of Markdown as a site to read
    #[command(display_name = "git-kb")]
    Serve {
        /// A folder to find git-kb.toml from, which says what to serve; or, without one, the folder to serve
        #[arg(default_value = ".")]
        folder: PathBuf,
    },
    /// Build a folder of Markdown as a static site, to open from disk or put on any host. It is
    /// written where git-kb.toml says.
    #[command(display_name = "git-kb")]
    Build {
        /// A folder to find git-kb.toml from, which says what to build; or, without one, the folder to build
        #[arg(default_value = ".")]
        folder: PathBuf,
    },
    /// Write a new page of a doctype from its template, with its next number; as `git kb new design "Cache tiling"`
    #[command(display_name = "git-kb")]
    New {
        /// The doctype, by its id in git-kb.toml
        doctype: String,
        /// The page's title; its file is named from it
        title: String,
        /// The folder of the site to write it in; the one git-kb is run in by default
        #[arg(long = "in", default_value = ".")]
        folder: PathBuf,
    },
    /// Say what in the pages' front matter cannot be made sense of: unknown doctypes or statuses, shared numbers,
    /// replaced pages and cited sources that are not there. Fails when it finds any
    #[command(display_name = "git-kb")]
    Check {
        /// A folder to find git-kb.toml from, which says what to check; or, without one, the folder to check
        #[arg(default_value = ".")]
        folder: PathBuf,
    },
    /// List the built-in plugins, and the ones git-kb.toml uses
    #[command(display_name = "git-kb")]
    Plugins {
        /// A folder to find git-kb.toml from, which says which are used
        #[arg(default_value = ".")]
        folder: PathBuf,
    },
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    impl Cli {
        fn parse_args(args: &[&str]) -> Result<Self, clap::Error> {
            Self::try_parse_from(std::iter::once("git-kb").chain(args.iter().copied()))
        }
    }

    #[test]
    fn the_command_line_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn serve_takes_a_folder() {
        assert_eq!(
            Cli::parse_args(&["serve", "docs"]).unwrap().command,
            Command::Serve {
                folder: "docs".into()
            }
        );
    }

    #[test]
    fn serve_defaults_to_the_folder_it_runs_in() {
        assert_eq!(
            Cli::parse_args(&["serve"]).unwrap().command,
            Command::Serve { folder: ".".into() }
        );
    }

    #[test]
    fn help_and_version_are_asked_for_with_h_and_capital_v() {
        for (args, kind) in [
            (&["-h"][..], clap::error::ErrorKind::DisplayHelp),
            (&["--help"], clap::error::ErrorKind::DisplayHelp),
            (&["serve", "-h"], clap::error::ErrorKind::DisplayHelp),
            (&["-V"], clap::error::ErrorKind::DisplayVersion),
            (&["--version"], clap::error::ErrorKind::DisplayVersion),
            (&["serve", "-V"], clap::error::ErrorKind::DisplayVersion),
        ] {
            assert_eq!(Cli::parse_args(args).unwrap_err().kind(), kind, "{args:?}");
        }
    }

    #[test]
    fn run_alone_it_says_how_it_is_used() {
        assert_eq!(
            Cli::parse_args(&[]).unwrap_err().kind(),
            clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
    }

    #[test]
    fn an_unknown_command_is_refused() {
        assert_eq!(
            Cli::parse_args(&["no-such-command"]).unwrap_err().kind(),
            clap::error::ErrorKind::InvalidSubcommand
        );
    }

    #[test]
    fn build_takes_a_folder() {
        assert_eq!(
            Cli::parse_args(&["build", "docs"]).unwrap().command,
            Command::Build {
                folder: "docs".into()
            }
        );
    }

    #[test]
    fn build_defaults_to_the_folder_it_runs_in() {
        assert_eq!(
            Cli::parse_args(&["build"]).unwrap().command,
            Command::Build { folder: ".".into() }
        );
    }

    #[test]
    fn where_to_write_is_only_set_in_git_kb_toml() {
        assert_eq!(
            Cli::parse_args(&["build", "--out", "site"])
                .unwrap_err()
                .kind(),
            clap::error::ErrorKind::UnknownArgument
        );
    }
}
