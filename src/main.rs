use std::io::IsTerminal;
use std::net::SocketAddr;
use std::process::ExitCode;

use clap::Parser;
use git_kb::cli::{Cli, Command};
use git_kb::config::Config;
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
        Command::Serve { folder } | Command::Build { folder } => folder.clone(),
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

    // Serving and building come in later steps; until then, what would be done.
    match command {
        Command::Serve { .. } => {
            let address = SocketAddr::new(config.serve.host, config.serve.port);
            info!(folder = %folder.display(), %address, "would serve");
        }
        Command::Build { .. } => {
            let out = &config.build.out;
            info!(folder = %folder.display(), out = %out.display(), "would build");
        }
    }
    ExitCode::SUCCESS
}
