use std::process::ExitCode;

use clap::Parser;
use git_kb::cli::Cli;

fn main() -> ExitCode {
    let _app = Cli::parse();

    ExitCode::SUCCESS
}
