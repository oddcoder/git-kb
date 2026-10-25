use std::io::IsTerminal;
use std::net::SocketAddr;
use std::path::Path;
use std::process::ExitCode;

use clap::Parser;
use git_kb::build::Builder;
use git_kb::cli::{Cli, Command};
use git_kb::config::Config;
use git_kb::git::{Repo, When};
use git_kb::new::{New, new_page};
use git_kb::plugins::{self, Plugins};
use git_kb::serve::Server;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    // What is logged: GIT_KB_LOG, as RUST_LOG is read, or else git-kb's news and other crates' warnings.
    let filter = EnvFilter::try_from_env("GIT_KB_LOG")
        .unwrap_or_else(|_| EnvFilter::new("git_kb=info,warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .without_time()
        .with_target(false)
        .init();

    let command = Cli::parse().command;
    let folder = match &command {
        Command::Serve { folder }
        | Command::Build { folder }
        | Command::Plugins { folder }
        | Command::New { folder, .. } => folder.clone(),
    };
    if !folder.is_dir() {
        error!(folder = %folder.display(), "not a folder");
        return ExitCode::FAILURE;
    }
    let (config, config_path) = match Config::find(&folder) {
        Ok(found) => found,
        Err(e) => {
            error!("{e:#}");
            return ExitCode::FAILURE;
        }
    };
    if let Some(file) = &config_path {
        info!(file = %file.display(), "settings");
    } else {
        info!("no {}: every setting is its default", Config::FILE);
    }

    // The plugins git-kb.toml names, their commands' outputs kept in the repository's git folder.
    let cache =
        Repo::discover(&config.root).map(|repo| repo.git().git_dir().join("git-kb").join("cache"));
    match Plugins::load(&config, config_path.as_deref(), cache) {
        Ok(loaded) => plugins::set(loaded),
        Err(e) => {
            error!("{e:#}");
            return ExitCode::FAILURE;
        }
    }

    match command {
        Command::Serve { .. } => {
            let address = SocketAddr::new(config.serve.host, config.serve.port);
            let served = tokio::runtime::Runtime::new()
                .map_err(anyhow::Error::from)
                .and_then(|runtime| {
                    // Until Ctrl-C, after which the requests begun are answered, and no more; or, at a
                    // second, at once.
                    runtime.block_on(
                        Server::new(config.root.clone(), config.title.clone())
                            .looking(config.look.clone())
                            .typing(config.doctypes.clone())
                            .commenting(&config.comments)
                            .syncing(config.serve.sync_every)
                            .run(address, async {
                                tokio::signal::ctrl_c().await.unwrap_or(());
                                tokio::spawn(async {
                                    tokio::signal::ctrl_c().await.unwrap_or(());
                                    std::process::exit(130);
                                });
                            }),
                    )
                });
            if let Err(e) = served {
                error!("{e:#}");
                return ExitCode::FAILURE;
            }
        }
        Command::Build { .. } => {
            let out = &config.build.out;
            let builder = Builder::new(config.title.clone(), config.url.clone())
                .looking(config.look.clone())
                .typing(config.doctypes.clone())
                .commenting(config.comments.clone());
            match builder.build(&config.root, out) {
                Ok(built) => {
                    info!(root = %config.root.display(), refs = built.refs, pages = built.pages, files = built.files,
                        folders = built.folders, out = %out.display(), "built");
                }
                Err(e) => {
                    error!("{e:#}");
                    return ExitCode::FAILURE;
                }
            }
        }
        Command::Plugins { .. } => list_plugins(),
        Command::New {
            doctype,
            title,
            folder,
        } => return write_new(&config, &doctype, &title, &folder),
    }
    ExitCode::SUCCESS
}

/// A new page of a doctype, from its template, in a folder of the site; and says where.
fn write_new(config: &Config, doctype: &str, title: &str, folder: &Path) -> ExitCode {
    // Who writes it, as git says, and today.
    let author = Repo::discover(folder)
        .and_then(|repo| {
            let repo = repo.git().to_thread_local();
            repo.committer()
                .and_then(Result::ok)
                .map(|who| who.name.to_string())
        })
        .unwrap_or_default();
    let new = New {
        doctype,
        title,
        folder,
        author: &author,
        date: &When::now().date(),
    };
    match new_page(config, &new) {
        Ok(file) => {
            println!("{}", file.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            error!("{e:#}");
            ExitCode::FAILURE
        }
    }
}

/// The built-in plugins, marked when git-kb.toml uses them, and the repository's own.
fn list_plugins() {
    let used = plugins::get();
    println!("Built in, each used by naming it in git-kb.toml, as [plugins.tikz]:");
    for name in plugins::built_in_names() {
        let mark = if used.by_name(name).is_some() {
            "used"
        } else {
            "    "
        };
        println!("  {mark}  {name:<10} {}", plugins::describe(name));
    }
    let own: Vec<_> = used
        .all()
        .iter()
        .filter(|p| !plugins::built_in_names().contains(&p.name.as_str()))
        .collect();
    if !own.is_empty() {
        println!("The repository's own:");
        for p in own {
            println!("  used  {:<10} {}", p.name, p.description);
        }
    }
}
