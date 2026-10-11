use std::ffi::OsStr;
use std::io;

use clap::CommandFactory;
use clap_complete::CompleteEnv;
use clap_complete::engine::CompletionCandidate;
use clap_complete::env::{Bash, Elvish, EnvCompleter, Fish, Powershell, Zsh};

use crate::cli::{Cli, CompletionShell};
use crate::config::Config;
use crate::error::{AppError, Result};

const COMPLETE_ENV: &str = "DESK_COMPLETE";
const BINARY: &str = "desk";

/// Answers the shell and exits when it is asking for completions.
pub fn handle_environment() {
    CompleteEnv::with_factory(Cli::command)
        .var(COMPLETE_ENV)
        .bin(BINARY)
        .completer(BINARY)
        .complete();
}

pub fn write_registration(shell: CompletionShell) -> Result<()> {
    let completer: &dyn EnvCompleter = match shell {
        CompletionShell::Bash => &Bash,
        CompletionShell::Elvish => &Elvish,
        CompletionShell::Fish => &Fish,
        CompletionShell::PowerShell => &Powershell,
        CompletionShell::Zsh => &Zsh,
    };
    completer
        .write_registration(
            COMPLETE_ENV,
            BINARY,
            BINARY,
            BINARY,
            &mut io::stdout().lock(),
        )
        .map_err(AppError::WriteOutput)
}

pub fn targets(current: &OsStr) -> Vec<CompletionCandidate> {
    let (Ok(config), Some(current)) = (Config::load(), current.to_str()) else {
        return Vec::new();
    };
    config
        .targets
        .iter()
        .filter(|target| target.name.starts_with(current))
        .map(|target| {
            let help = format!(
                "{} · {}",
                config.session_label(target),
                target.mode.as_str()
            );
            CompletionCandidate::new(&target.name).help(Some(help.into()))
        })
        .collect()
}

pub fn sessions(current: &OsStr) -> Vec<CompletionCandidate> {
    let (Ok(config), Some(current)) = (Config::load(), current.to_str()) else {
        return Vec::new();
    };
    config
        .sessions
        .iter()
        .filter(|session| session.key.starts_with(current))
        .map(|session| {
            CompletionCandidate::new(&session.key).help(Some(session.label.clone().into()))
        })
        .collect()
}
