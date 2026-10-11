use std::io::Write;
use std::path::Path;

use crate::backend::{Backend, NewTab, SessionRef, TabRef};
use crate::config::{COMMAND_PLACEHOLDER, Config, Mode, PROMPT_PLACEHOLDER, Target};
use crate::error::{AppError, Result};

/// Plain `desk`.
pub fn build(config: &Config, backend: &dyn Backend) -> Result<()> {
    let startup: Vec<&Target> = config
        .targets
        .iter()
        .filter(|target| target.mode == Mode::Startup)
        .collect();
    for target in &startup {
        require_dir(&target.dir)?;
    }

    let mut sessions = backend.sessions()?;
    for target in startup {
        let label = base_label(target);
        let argv = argv(config, target, None);
        let tab = NewTab {
            label: &label,
            dir: &target.dir,
            argv: argv.as_deref(),
            key: None,
        };
        let session_label = config.session_label(target);
        match sessions
            .iter()
            .find(|session| session.label == session_label)
        {
            Some(session) => {
                if !backend
                    .tabs(&session.id)?
                    .iter()
                    .any(|tab| tab.label == label)
                {
                    backend.open_tab(&session.id, tab)?;
                }
            }
            None => {
                let created = backend.create_session(session_label, None, tab)?;
                sessions.push(SessionRef {
                    id: created.session_id,
                    label: session_label.to_owned(),
                    key: None,
                });
            }
        }
    }
    Ok(())
}

pub fn open(config: &Config, backend: &dyn Backend, name: &str, prompt: &[String]) -> Result<()> {
    let target = config
        .targets
        .iter()
        .find(|target| target.name == name)
        .ok_or_else(|| AppError::UnknownTarget(name.to_owned()))?;

    let prompt = prompt.join(" ");
    let prompt = (!prompt.is_empty()).then_some(prompt);
    if prompt.is_some() {
        let reason = match &target.command {
            None => Some("it has no `command`"),
            Some(command) if !command.contains(PROMPT_PLACEHOLDER) => {
                Some("its `command` has no `{prompt}`")
            }
            Some(_) => None,
        };
        if let Some(reason) = reason {
            return Err(AppError::PromptNotAccepted {
                target: name.to_owned(),
                reason,
            });
        }
    }

    let session_label = config.session_label(target);
    let session = backend
        .sessions()?
        .into_iter()
        .find(|session| session.label == session_label);
    let tabs = match &session {
        Some(session) => backend.tabs(&session.id)?,
        None => Vec::new(),
    };

    let base = base_label(target);
    let label = if target.mode == Mode::Multi {
        format!("{base} {}", next_number(&base, &tabs))
    } else if let Some(session) = &session
        && let Some(tab) = tabs.iter().find(|tab| tab.label == base)
    {
        if prompt.is_some() {
            return Err(AppError::TabAlreadyOpen(name.to_owned()));
        }
        return focus(backend, &session.id, &tab.id);
    } else {
        base
    };

    require_dir(&target.dir)?;
    let argv = argv(config, target, prompt.as_deref());
    let tab = NewTab {
        label: &label,
        dir: &target.dir,
        argv: argv.as_deref(),
        key: None,
    };
    let (session_id, tab_id) = match session {
        Some(session) => {
            let tab_id = backend.open_tab(&session.id, tab)?;
            (session.id, tab_id)
        }
        None => {
            let created = backend.create_session(session_label, None, tab)?;
            (created.session_id, created.tab_id)
        }
    };
    focus(backend, &session_id, &tab_id)
}

pub fn ls(config: &Config, backend: &dyn Backend, out: &mut dyn Write) -> Result<()> {
    let running = running_sessions(backend)?;
    for session in &config.sessions {
        let tabs = match running.iter().find(|other| other.label == session.label) {
            Some(running) => Some(backend.tabs(&running.id)?),
            None => None,
        };
        let state = if tabs.is_some() {
            ""
        } else {
            "  (not running)"
        };
        writeln!(out, "{}  {}{state}", session.key, session.label)
            .map_err(AppError::WriteOutput)?;

        let targets: Vec<&Target> = config
            .targets
            .iter()
            .filter(|target| target.session == session.key)
            .collect();
        let width = targets
            .iter()
            .map(|target| target.name.chars().count())
            .max()
            .unwrap_or(0);
        for target in targets {
            let open = open_tabs(target, tabs.as_deref().unwrap_or_default());
            let open = if open.is_empty() {
                "-".to_owned()
            } else {
                open.join(", ")
            };
            writeln!(
                out,
                "  {:<width$}  {:<7}  {open}",
                target.name,
                target.mode.as_str()
            )
            .map_err(AppError::WriteOutput)?;
        }
    }
    Ok(())
}

pub fn stop(config: &Config, backend: &dyn Backend, key: Option<&str>) -> Result<()> {
    let labels: Vec<&str> = match key {
        Some(key) => {
            let session = config
                .sessions
                .iter()
                .find(|session| session.key == key)
                .ok_or_else(|| AppError::UnknownSession(key.to_owned()))?;
            vec![&session.label]
        }
        None => config
            .sessions
            .iter()
            .map(|session| session.label.as_str())
            .collect(),
    };

    let mut doomed: Vec<SessionRef> = running_sessions(backend)?
        .into_iter()
        .filter(|session| labels.contains(&session.label.as_str()))
        .collect();
    // The session desk runs in goes last, so desk lives to kill the others.
    let current = backend.current_session();
    doomed.sort_by_key(|session| Some(&session.id) == current.as_ref());
    for session in doomed {
        backend.kill_session(&session.id)?;
    }
    Ok(())
}

fn running_sessions(backend: &dyn Backend) -> Result<Vec<SessionRef>> {
    match backend.sessions() {
        Err(AppError::BackendUnavailable(_)) => Ok(Vec::new()),
        sessions => sessions,
    }
}

/// Makes the tab active and shows its session to the user.
fn focus(backend: &dyn Backend, session_id: &str, tab_id: &str) -> Result<()> {
    backend.focus_tab(session_id, tab_id)?;
    // The tab is open and active either way.
    if let Err(error) = backend.show_session(session_id, tab_id) {
        eprintln!("warning: {error}");
    }
    Ok(())
}

pub fn require_dir(dir: &Path) -> Result<()> {
    if dir.is_dir() {
        Ok(())
    } else {
        Err(AppError::MissingDirectory(dir.to_owned()))
    }
}

fn base_label(target: &Target) -> String {
    format!("{} {}", target.icon, target.name)
}

/// The number in a multi target's tab label, if `label` is one of its tabs.
fn tab_number(base: &str, label: &str) -> Option<u32> {
    label
        .strip_prefix(base)?
        .strip_prefix(' ')?
        .parse()
        .ok()
        .filter(|number| *number > 0)
}

fn next_number(base: &str, tabs: &[TabRef]) -> u32 {
    let used: Vec<u32> = tabs
        .iter()
        .filter_map(|tab| tab_number(base, &tab.label))
        .collect();
    (1..)
        .find(|number| !used.contains(number))
        .expect("a number is free")
}

fn open_tabs<'a>(target: &Target, tabs: &'a [TabRef]) -> Vec<&'a str> {
    let base = base_label(target);
    tabs.iter()
        .map(|tab| tab.label.as_str())
        .filter(|label| match target.mode {
            Mode::Multi => tab_number(&base, label).is_some(),
            Mode::Startup | Mode::Demand => *label == base,
        })
        .collect()
}

/// The argv that runs the target's command and leaves a shell behind. A
/// target with no command gets the bare shell, or the backend's default when
/// no `shell` is configured.
fn argv(config: &Config, target: &Target, prompt: Option<&str>) -> Option<Vec<String>> {
    let prompt = prompt.map(fish_quote).unwrap_or_default();
    let command = target
        .command
        .as_deref()
        .unwrap_or_default()
        .replace(PROMPT_PLACEHOLDER, &prompt);
    shell_argv(config, &command)
}

/// The configured `shell` running `command`, which may be empty.
pub fn shell_argv(config: &Config, command: &str) -> Option<Vec<String>> {
    let shell = config.shell.as_ref()?;
    Some(
        shell
            .iter()
            .map(|argument| {
                if argument == COMMAND_PLACEHOLDER {
                    command.to_owned()
                } else {
                    argument.clone()
                }
            })
            .collect(),
    )
}

/// Quotes `value` as one fish argument. Inside single quotes fish only
/// treats `\` and `'` specially.
fn fish_quote(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BackendKind, Session};
    use crate::fake::Fake;

    const MAIN: &str = "🏠 main";
    const WORK: &str = "🔎 work";

    fn target(dir: &Path, session: &str, name: &str, mode: Mode, command: Option<&str>) -> Target {
        Target {
            name: name.to_owned(),
            session: session.to_owned(),
            icon: "•".to_owned(),
            dir: dir.to_owned(),
            command: command.map(str::to_owned),
            mode,
        }
    }

    /// Two sessions whose startup targets interleave: notes, logs, mail.
    fn config(dir: &Path) -> Config {
        let session = |key: &str, label: &str| Session {
            key: key.to_owned(),
            label: label.to_owned(),
        };
        Config {
            backend: BackendKind::Rex,
            shell: Some(["fish", "-C", "{command}"].map(String::from).to_vec()),
            sessions: vec![session("main", MAIN), session("work", WORK)],
            targets: vec![
                target(dir, "main", "notes", Mode::Startup, None),
                target(dir, "main", "app", Mode::Demand, None),
                target(dir, "main", "chat", Mode::Demand, Some("chat {prompt}")),
                target(dir, "work", "logs", Mode::Startup, Some("tail -f log")),
                target(dir, "work", "agent", Mode::Multi, Some("agent {prompt}")),
                target(dir, "main", "mail", Mode::Startup, None),
            ],
        }
    }

    fn words(prompt: &[&str]) -> Vec<String> {
        prompt.iter().map(|word| (*word).to_owned()).collect()
    }

    fn layout(sessions: &[(&str, &[&str])]) -> Vec<(String, Vec<String>)> {
        sessions
            .iter()
            .map(|(label, tabs)| ((*label).to_owned(), words(tabs)))
            .collect()
    }

    #[test]
    fn build_opens_startup_tabs_in_config_order_without_focusing() {
        let dir = tempfile::tempdir().unwrap();
        let config = config(dir.path());
        let fake = Fake::default();

        build(&config, &fake).unwrap();

        assert_eq!(
            fake.layout(),
            layout(&[(MAIN, &["• notes", "• mail"]), (WORK, &["• logs"])])
        );
        assert_eq!(fake.argv("• notes"), Some(words(&["fish", "-C", ""])));
        assert_eq!(
            fake.argv("• logs"),
            Some(words(&["fish", "-C", "tail -f log"]))
        );
        assert_eq!(fake.state.borrow().sessions[0].tabs[0].dir, dir.path());
        assert!(fake.focused().is_empty());
    }

    #[test]
    fn without_a_shell_a_tab_opens_the_backend_default() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = config(dir.path());
        config.shell = None;
        config.targets.retain(|target| target.command.is_none());
        let fake = Fake::default();

        open(&config, &fake, "app", &[]).unwrap();

        assert_eq!(fake.argv("• app"), None);
    }

    #[test]
    fn build_adds_only_the_missing_startup_tabs() {
        let dir = tempfile::tempdir().unwrap();
        let config = config(dir.path());
        let fake = Fake::default();
        fake.seed(MAIN, &["mine", "• mail"]);

        build(&config, &fake).unwrap();
        build(&config, &fake).unwrap();

        assert_eq!(
            fake.layout(),
            layout(&[(MAIN, &["mine", "• mail", "• notes"]), (WORK, &["• logs"])])
        );
    }

    #[test]
    fn a_missing_directory_is_named_and_nothing_is_created() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = config(dir.path());
        let gone = dir.path().join("gone");
        for name in ["mail", "app"] {
            let target = config
                .targets
                .iter_mut()
                .find(|target| target.name == name)
                .unwrap();
            target.dir = gone.clone();
        }
        let fake = Fake::default();

        let built = build(&config, &fake).unwrap_err().to_string();
        let opened = open(&config, &fake, "app", &[]).unwrap_err().to_string();

        assert!(built.contains(gone.to_str().unwrap()), "{built}");
        assert!(opened.contains(gone.to_str().unwrap()), "{opened}");
        assert!(fake.layout().is_empty());
    }

    #[test]
    fn open_focuses_an_existing_single_tab_instead_of_opening_another() {
        let dir = tempfile::tempdir().unwrap();
        let config = config(dir.path());
        let fake = Fake::default();

        open(&config, &fake, "app", &[]).unwrap();
        open(&config, &fake, "app", &[]).unwrap();
        open(&config, &fake, "notes", &[]).unwrap();

        assert_eq!(fake.layout(), layout(&[(MAIN, &["• app", "• notes"])]));
        assert_eq!(fake.focused(), ["• app", "• app", "• notes"]);
    }

    #[test]
    fn open_numbers_multi_tabs_from_the_smallest_free_number() {
        let dir = tempfile::tempdir().unwrap();
        let config = config(dir.path());
        let fake = Fake::default();
        fake.seed(WORK, &["• agent 1", "• agent 3", "• agent x", "• agent 0"]);

        open(&config, &fake, "agent", &[]).unwrap();
        open(&config, &fake, "agent", &[]).unwrap();

        assert_eq!(fake.focused(), ["• agent 2", "• agent 4"]);
    }

    #[test]
    fn open_passes_the_prompt_as_one_fish_quoted_argument() {
        let dir = tempfile::tempdir().unwrap();
        let config = config(dir.path());
        let fake = Fake::default();

        open(
            &config,
            &fake,
            "agent",
            &words(&["why", "it's", "--slow\\"]),
        )
        .unwrap();
        open(&config, &fake, "agent", &[]).unwrap();

        assert_eq!(
            fake.argv("• agent 1"),
            Some(words(&["fish", "-C", r"agent 'why it\'s --slow\\'"]))
        );
        assert_eq!(
            fake.argv("• agent 2"),
            Some(words(&["fish", "-C", "agent "]))
        );
    }

    #[test]
    fn open_rejects_what_it_cannot_do_and_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let config = config(dir.path());
        let fake = Fake::default();
        open(&config, &fake, "chat", &[]).unwrap();
        let cases = [
            ("nope", vec![], "unknown target `nope`"),
            ("app", words(&["hi"]), "it has no `command`"),
            ("logs", words(&["hi"]), "its `command` has no `{prompt}`"),
            ("chat", words(&["hi"]), "target `chat` is already open"),
        ];

        for (name, prompt, expected) in cases {
            let message = open(&config, &fake, name, &prompt).unwrap_err().to_string();
            assert!(message.contains(expected), "{message}");
        }

        assert_eq!(fake.layout(), layout(&[(MAIN, &["• chat"])]));
        assert_eq!(fake.focused(), ["• chat"]);
    }

    #[test]
    fn stop_kills_only_configured_sessions_and_its_own_last() {
        let dir = tempfile::tempdir().unwrap();
        let config = config(dir.path());
        let fake = Fake::default();
        let main = fake.seed(MAIN, &[]);
        fake.seed("other", &[]);
        fake.seed(WORK, &[]);
        fake.state.borrow_mut().current = Some(main);

        stop(&config, &fake, None).unwrap();

        assert_eq!(fake.state.borrow().killed, [WORK, MAIN]);
        assert_eq!(fake.layout(), layout(&[("other", &[])]));
    }

    #[test]
    fn stop_with_a_key_kills_just_that_session() {
        let dir = tempfile::tempdir().unwrap();
        let config = config(dir.path());
        let fake = Fake::default();
        fake.seed(MAIN, &[]);
        fake.seed(WORK, &[]);

        stop(&config, &fake, Some("work")).unwrap();
        let unknown = stop(&config, &fake, Some("other")).unwrap_err().to_string();

        assert_eq!(fake.layout(), layout(&[(MAIN, &[])]));
        assert!(unknown.contains("unknown session `other`"), "{unknown}");
    }

    #[test]
    fn ls_shows_modes_and_open_tabs() {
        let dir = tempfile::tempdir().unwrap();
        let config = config(dir.path());
        let fake = Fake::default();
        fake.seed(WORK, &["• agent 1", "mine", "• agent 2"]);
        let mut out = Vec::new();

        ls(&config, &fake, &mut out).unwrap();

        assert_eq!(
            String::from_utf8(out).unwrap(),
            "main  🏠 main  (not running)\n\
             \x20 notes  startup  -\n\
             \x20 app    demand   -\n\
             \x20 chat   demand   -\n\
             \x20 mail   startup  -\n\
             work  🔎 work\n\
             \x20 logs   startup  -\n\
             \x20 agent  multi    • agent 1, • agent 2\n"
        );
    }

    #[test]
    fn without_a_running_backend_only_ls_and_stop_succeed() {
        let dir = tempfile::tempdir().unwrap();
        let config = config(dir.path());
        let fake = Fake::default();
        fake.state.borrow_mut().down = true;
        let mut out = Vec::new();

        ls(&config, &fake, &mut out).unwrap();
        stop(&config, &fake, None).unwrap();

        assert!(String::from_utf8(out).unwrap().contains("(not running)"));
        assert!(matches!(
            build(&config, &fake),
            Err(AppError::BackendUnavailable(_))
        ));
        assert!(matches!(
            open(&config, &fake, "app", &[]),
            Err(AppError::BackendUnavailable(_))
        ));
    }
}
