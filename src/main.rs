mod backend;
mod cli;
mod commands;
mod completion;
mod config;
mod error;
mod rex;

use std::process::ExitCode;

use clap::Parser;

use crate::cli::{Cli, Command};
use crate::config::{BackendKind, Config};
use crate::error::Result;
use crate::rex::Rex;

fn main() -> ExitCode {
    completion::handle_environment();

    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(error.exit_code())
        }
    }
}

fn run(cli: &Cli) -> Result<()> {
    if let Some(Command::Completions(args)) = &cli.command {
        return completion::write_registration(args.shell);
    }

    let config = Config::load()?;
    let backend = match config.backend {
        BackendKind::Rex => Rex::from_environment(),
    };
    match &cli.command {
        None => commands::build(&config, &backend),
        Some(Command::Open(args)) => commands::open(&config, &backend, &args.target, &args.prompt),
        Some(Command::Ls) => commands::ls(&config, &backend, &mut std::io::stdout().lock()),
        Some(Command::Stop(args)) => commands::stop(&config, &backend, args.session.as_deref()),
        Some(Command::Completions(_)) => unreachable!("handled before the config is loaded"),
    }
}
