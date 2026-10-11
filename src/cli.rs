use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum, ValueHint};
use clap_complete::engine::ArgValueCompleter;

use crate::completion;

#[derive(Debug, Parser)]
#[command(name = "desk", version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Open a target, or focus it if it is already open
    // No help flag, so a prompt may start with `--help`; `desk help open` works.
    #[command(disable_help_flag = true)]
    Open(OpenArgs),

    /// List sessions, targets and open tabs
    Ls,

    /// Kill the sessions desk manages, or just one
    Stop(StopArgs),

    /// Open or focus the workspaces a plan describes, and report them as JSON
    Attach(AttachArgs),

    /// Print shell completion setup
    Completions(CompletionsArgs),
}

#[derive(Debug, Args)]
pub struct OpenArgs {
    /// Target name
    #[arg(add = ArgValueCompleter::new(completion::targets))]
    pub target: String,

    /// Prompt for the target's command; joined with spaces
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub prompt: Vec<String>,
}

#[derive(Debug, Args)]
pub struct StopArgs {
    /// Session key to kill; all managed sessions when omitted
    #[arg(add = ArgValueCompleter::new(completion::sessions))]
    pub session: Option<String>,
}

#[derive(Debug, Args)]
pub struct AttachArgs {
    /// JSON plan to read; `-` reads standard input
    #[arg(long, value_name = "FILE", value_hint = ValueHint::FilePath)]
    pub plan: PathBuf,
}

#[derive(Debug, Args)]
pub struct CompletionsArgs {
    /// Shell to print completion setup for
    #[arg(value_enum)]
    pub shell: CompletionShell,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum CompletionShell {
    Bash,
    Elvish,
    Fish,
    #[value(name = "powershell", alias = "pwsh")]
    PowerShell,
    Zsh,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("desk").chain(args.iter().copied())).unwrap()
    }

    #[test]
    fn open_takes_target_and_hyphenated_prompt() {
        let Some(Command::Open(args)) =
            parse(&["open", "notes", "why", "--is", "-it", "slow"]).command
        else {
            panic!("expected Open");
        };
        assert_eq!(args.target, "notes");
        assert_eq!(args.prompt, ["why", "--is", "-it", "slow"]);
    }

    #[test]
    fn attach_reads_a_plan_from_a_file_or_standard_input() {
        for plan in ["plan.json", "-"] {
            let Some(Command::Attach(args)) = parse(&["attach", "--plan", plan]).command else {
                panic!("expected Attach");
            };
            assert_eq!(args.plan, PathBuf::from(plan));
        }
    }

    #[test]
    fn open_prompt_may_start_with_a_help_flag() {
        for flag in ["--help", "-h"] {
            let Some(Command::Open(args)) = parse(&["open", "notes", flag, "me"]).command else {
                panic!("expected Open");
            };
            assert_eq!(args.prompt, [flag, "me"]);
        }
    }
}
