//! The Rex backend, driving the `rex` CLI.
//!
//! What desk relies on, checked against Rex Beta 0.1.0 (1069):
//!
//! - Ids never change. Labels match case-insensitively and need not be
//!   unique, so every call here passes ids.
//! - Creating a session or focusing a window never moves the app to another
//!   session. Only the app's `session.select` action does.
//! - A terminal started without a command runs the login shell. A command
//!   gets the login shell's environment but is not parsed as shell code.
//! - Nothing can be stored on a session or window, so desk's keys are
//!   dropped and never reported.
//! - A window's panes are the blocks in its layers. A terminal block answers
//!   `process` with its foreground process, which may report a cwd.
//! - Exit status 3 means the target was not found or is ambiguous, and 4
//!   means the server is unreachable.
//!
//! CLI reference: <https://www.superlogical.com/rex/docs/automate/cli>

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::backend::{Backend, CreatedSession, NewTab, SessionRef, TabRef};
use crate::error::{AppError, Result};

const CURRENT_SESSION_VARIABLE: &str = "REX_SESSION";
/// Rex's exit status when it cannot reach the server.
const SERVER_UNREACHABLE: i32 = 4;

pub struct Output {
    pub status: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub trait Runner {
    fn run(&self, arguments: &[OsString]) -> std::io::Result<Output>;
}

pub struct RexCli;

impl Runner for RexCli {
    fn run(&self, arguments: &[OsString]) -> std::io::Result<Output> {
        let output = Command::new("rex").args(arguments).output()?;
        Ok(Output {
            status: output.status.code(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

pub struct Rex<R = RexCli> {
    runner: R,
    current_session: Option<String>,
}

impl Rex {
    pub fn from_environment() -> Self {
        Self {
            runner: RexCli,
            current_session: std::env::var(CURRENT_SESSION_VARIABLE)
                .ok()
                .filter(|id| !id.is_empty()),
        }
    }
}

#[derive(Deserialize)]
struct SessionList {
    sessions: Option<Vec<SessionEntry>>,
}

#[derive(Deserialize)]
struct SessionEntry {
    session_id: String,
    label: String,
}

#[derive(Deserialize)]
struct WindowList {
    windows: Option<Vec<WindowEntry>>,
}

#[derive(Deserialize)]
struct WindowEntry {
    window_id: String,
    label: String,
    layers: Option<Vec<LayerEntry>>,
}

#[derive(Deserialize)]
struct LayerEntry {
    blocks: Option<Vec<BlockEntry>>,
}

#[derive(Deserialize)]
struct BlockEntry {
    block_id: Option<String>,
}

#[derive(Deserialize)]
struct ProcessResult {
    foreground: Option<ProcessEntry>,
}

#[derive(Deserialize)]
struct ProcessEntry {
    cwd: Option<PathBuf>,
}

#[derive(Deserialize)]
struct SessionCreated {
    session_id: String,
    initial_windows: Vec<WindowCreated>,
}

#[derive(Deserialize)]
struct WindowCreated {
    window_id: String,
}

impl<R: Runner> Rex<R> {
    fn run(&self, context: &str, arguments: Vec<OsString>) -> Result<Vec<u8>> {
        // Rex starts a missing server unless told otherwise.
        let mut full = vec![OsString::from("--autostart=false")];
        full.extend(arguments);
        let output = self.runner.run(&full).map_err(AppError::StartRex)?;
        match output.status {
            Some(0) => Ok(output.stdout),
            Some(SERVER_UNREACHABLE) => Err(AppError::BackendUnavailable(
                "the Rex server is not running".into(),
            )),
            status => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let message = match (stderr.trim(), status) {
                    ("", Some(status)) => format!("rex exited with status {status}"),
                    ("", None) => "rex was killed by a signal".to_owned(),
                    (stderr, _) => stderr.to_owned(),
                };
                Err(AppError::Rex {
                    context: context.to_owned(),
                    message,
                })
            }
        }
    }

    fn request<T: DeserializeOwned>(&self, context: &str, arguments: Vec<OsString>) -> Result<T> {
        let stdout = self.run(context, arguments)?;
        serde_json::from_slice(&stdout).map_err(|source| AppError::ParseRex {
            context: context.to_owned(),
            source,
        })
    }

    fn windows(&self, session_id: &str) -> Result<Vec<WindowEntry>> {
        let list: WindowList = self.request(
            "could not list Rex windows",
            vec![
                "window".into(),
                "ls".into(),
                "--session".into(),
                session_id.into(),
                "--json".into(),
            ],
        )?;
        Ok(list.windows.unwrap_or_default())
    }
}

/// The arguments shared by `rex new` and `rex window new`.
fn terminal(tab: NewTab<'_>) -> Vec<OsString> {
    let mut arguments = vec!["--cwd".into(), tab.dir.into(), "--json".into()];
    if let Some(argv) = tab.argv {
        arguments.push("--".into());
        arguments.extend(argv.iter().map(OsString::from));
    }
    arguments
}

impl<R: Runner> Backend for Rex<R> {
    fn current_session(&self) -> Option<String> {
        self.current_session.clone()
    }

    fn sessions(&self) -> Result<Vec<SessionRef>> {
        let list: SessionList = self.request(
            "could not list Rex sessions",
            vec!["ls".into(), "--json".into()],
        )?;
        Ok(list
            .sessions
            .unwrap_or_default()
            .into_iter()
            .map(|session| SessionRef {
                id: session.session_id,
                label: session.label,
                key: None,
            })
            .collect())
    }

    fn create_session(
        &self,
        label: &str,
        _key: Option<&str>,
        tab: NewTab<'_>,
    ) -> Result<CreatedSession> {
        let context = "could not create a Rex session";
        let mut arguments = vec!["new".into(), label.into()];
        if let Some(window) = tab.label {
            arguments.extend(["--window".into(), window.into()]);
        }
        arguments.extend(terminal(tab));
        let created: SessionCreated = self.request(context, arguments)?;
        let window = created
            .initial_windows
            .into_iter()
            .next()
            .ok_or_else(|| AppError::Rex {
                context: context.to_owned(),
                message: "Rex did not report the session's first window".into(),
            })?;
        Ok(CreatedSession {
            session_id: created.session_id,
            tab_id: window.window_id,
        })
    }

    fn kill_session(&self, session_id: &str) -> Result<()> {
        self.run(
            "could not kill a Rex session",
            vec!["kill".into(), session_id.into()],
        )
        .map(drop)
    }

    fn rename_session(&self, session_id: &str, label: &str) -> Result<()> {
        self.run(
            "could not rename a Rex session",
            vec![
                "session".into(),
                "rename".into(),
                session_id.into(),
                label.into(),
            ],
        )
        .map(drop)
    }

    fn set_session_key(&self, _session_id: &str, _key: &str) -> Result<bool> {
        Ok(false)
    }

    fn tabs(&self, session_id: &str) -> Result<Vec<TabRef>> {
        Ok(self
            .windows(session_id)?
            .into_iter()
            .map(|window| TabRef {
                id: window.window_id,
                label: window.label,
                key: None,
            })
            .collect())
    }

    fn open_tab(&self, session_id: &str, tab: NewTab<'_>) -> Result<String> {
        let mut arguments = vec![
            "window".into(),
            "new".into(),
            "--session".into(),
            session_id.into(),
        ];
        arguments.extend(tab.label.map(OsString::from));
        arguments.push("--focus=false".into());
        arguments.extend(terminal(tab));
        let created: WindowCreated = self.request("could not create a Rex window", arguments)?;
        Ok(created.window_id)
    }

    fn rename_tab(&self, session_id: &str, tab_id: &str, label: &str) -> Result<()> {
        self.run(
            "could not rename a Rex window",
            vec![
                "window".into(),
                "rename".into(),
                "--session".into(),
                session_id.into(),
                tab_id.into(),
                label.into(),
            ],
        )
        .map(drop)
    }

    fn set_tab_key(&self, _session_id: &str, _tab_id: &str, _key: &str) -> Result<bool> {
        Ok(false)
    }

    fn tab_cwds(&self, session_id: &str, tab_id: &str) -> Result<Vec<PathBuf>> {
        let window = self
            .windows(session_id)?
            .into_iter()
            .find(|window| window.window_id == tab_id);
        let blocks = window
            .and_then(|window| window.layers)
            .into_iter()
            .flatten()
            .flat_map(|layer| layer.blocks.unwrap_or_default())
            .filter_map(|block| block.block_id);
        // A block that is not a terminal has no process to ask about.
        Ok(blocks
            .filter_map(|block_id| {
                let process: ProcessResult = self
                    .request(
                        "could not inspect a Rex block",
                        vec![
                            "block".into(),
                            "call".into(),
                            "--session".into(),
                            session_id.into(),
                            "--block".into(),
                            block_id.into(),
                            "process".into(),
                        ],
                    )
                    .ok()?;
                process.foreground?.cwd
            })
            .collect())
    }

    fn focus_tab(&self, session_id: &str, tab_id: &str) -> Result<()> {
        self.run(
            "could not focus a Rex window",
            vec![
                "window".into(),
                "focus".into(),
                "--session".into(),
                session_id.into(),
                tab_id.into(),
            ],
        )
        .map(drop)
    }

    fn show_session(&self, session_id: &str, tab_id: &str) -> Result<()> {
        // Only the app can show a session, and none may be connected.
        self.run(
            "could not show the session in the Rex app",
            vec![
                "do".into(),
                "session.select".into(),
                format!("session_id={session_id}").into(),
                format!("window_id={tab_id}").into(),
            ],
        )
        .map(drop)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::path::Path;

    use super::*;

    /// Replays canned results and records the arguments of each call.
    #[derive(Default)]
    struct Script {
        results: RefCell<VecDeque<(i32, &'static str)>>,
        calls: RefCell<Vec<String>>,
    }

    impl Runner for Script {
        fn run(&self, arguments: &[OsString]) -> std::io::Result<Output> {
            let arguments: Vec<_> = arguments
                .iter()
                .map(|argument| argument.to_str().unwrap())
                .collect();
            self.calls.borrow_mut().push(arguments.join(" "));
            let (status, text) = self.results.borrow_mut().pop_front().unwrap();
            let (stdout, stderr) = if status == 0 { (text, "") } else { ("", text) };
            Ok(Output {
                status: Some(status),
                stdout: stdout.into(),
                stderr: stderr.into(),
            })
        }
    }

    fn rex(results: &[(i32, &'static str)]) -> Rex<Script> {
        Rex {
            runner: Script {
                results: RefCell::new(results.iter().copied().collect()),
                calls: RefCell::default(),
            },
            current_session: None,
        }
    }

    fn calls(rex: &Rex<Script>) -> Vec<String> {
        rex.runner.calls.borrow().clone()
    }

    fn tab<'a>(argv: Option<&'a [String]>) -> NewTab<'a> {
        NewTab {
            label: Some("📝 notes"),
            dir: Path::new("/src/app"),
            argv,
            key: None,
        }
    }

    // The JSON below was captured from Rex Beta 0.1.0 (1069).

    #[test]
    fn lists_sessions_and_tabs_by_id() {
        let rex = rex(&[
            (
                0,
                r#"{"sessions":[{"session_id":"session:1","label":"🏠 main"}]}"#,
            ),
            (
                0,
                r#"{"revision":55,"session_id":"session:1","label":"🏠 main",
                    "active_window_id":"window:2","focused_window":{"window_id":"window:2"},
                    "windows":[{"window_id":"window:1","label":"📝 notes"},
                               {"window_id":"window:2","label":"📦 app","active":true}]}"#,
            ),
        ]);

        let sessions = rex.sessions().unwrap();
        let tabs = rex.tabs("session:1").unwrap();

        assert_eq!(
            sessions,
            [SessionRef {
                id: "session:1".into(),
                label: "🏠 main".into(),
                key: None
            }]
        );
        let tabs: Vec<_> = tabs
            .iter()
            .map(|tab| (tab.id.as_str(), tab.label.as_str()))
            .collect();
        assert_eq!(tabs, [("window:1", "📝 notes"), ("window:2", "📦 app")]);
        assert_eq!(
            calls(&rex),
            [
                "--autostart=false ls --json",
                "--autostart=false window ls --session session:1 --json"
            ]
        );
    }

    #[test]
    fn creates_a_session_running_the_argv_as_given() {
        let rex = rex(&[(
            0,
            r#"{"session_id":"session:9","initial_windows":[{"window_id":"window:7",
                "block_ids":["block:3"],"revision":1}]}"#,
        )]);
        let argv = ["fish", "-C", "agent 'a b'"].map(String::from);

        let created = rex
            .create_session("🏠 main", None, tab(Some(&argv)))
            .unwrap();

        assert_eq!(created.session_id, "session:9");
        assert_eq!(created.tab_id, "window:7");
        assert_eq!(
            rex.runner.calls.borrow()[0],
            "--autostart=false new 🏠 main --window 📝 notes --cwd /src/app --json \
             -- fish -C agent 'a b'"
        );
    }

    #[test]
    fn opens_a_tab_unfocused_and_without_a_command() {
        let rex = rex(&[(
            0,
            r#"{"window_id":"window:8","block_ids":["block:4"],"revision":2}"#,
        )]);

        let id = rex.open_tab("session:9", tab(None)).unwrap();

        assert_eq!(id, "window:8");
        assert_eq!(
            calls(&rex),
            [
                "--autostart=false window new --session session:9 📝 notes --focus=false \
              --cwd /src/app --json"
            ]
        );
    }

    #[test]
    fn an_unlabelled_tab_leaves_the_label_to_rex() {
        let rex = rex(&[
            (
                0,
                r#"{"session_id":"session:9","initial_windows":[{"window_id":"window:7",
                    "block_ids":["block:3"],"revision":1}]}"#,
            ),
            (
                0,
                r#"{"window_id":"window:8","block_ids":["block:4"],"revision":2}"#,
            ),
        ]);
        let tab = NewTab {
            label: None,
            ..tab(None)
        };

        rex.create_session("🏠 main", None, tab).unwrap();
        rex.open_tab("session:9", tab).unwrap();

        assert_eq!(
            calls(&rex),
            [
                "--autostart=false new 🏠 main --cwd /src/app --json",
                "--autostart=false window new --session session:9 --focus=false \
              --cwd /src/app --json"
            ]
        );
    }

    #[test]
    fn focusing_and_showing_are_separate_and_a_missing_app_fails_only_the_show() {
        let rex = rex(&[(0, ""), (1, "no client offers session.select")]);

        rex.focus_tab("session:9", "window:8").unwrap();
        let shown = rex.show_session("session:9", "window:8").unwrap_err();

        assert_eq!(
            shown.to_string(),
            "could not show the session in the Rex app: no client offers session.select"
        );
        assert_eq!(
            calls(&rex),
            [
                "--autostart=false window focus --session session:9 window:8",
                "--autostart=false do session.select session_id=session:9 window_id=window:8"
            ]
        );
    }

    #[test]
    fn renames_by_id_and_stores_no_keys() {
        let rex = rex(&[(0, ""), (0, "")]);

        rex.rename_session("session:9", "🌲 q4").unwrap();
        rex.rename_tab("session:9", "window:8", "🌲 main").unwrap();
        let stored = [
            rex.set_session_key("session:9", "/src/app").unwrap(),
            rex.set_tab_key("session:9", "window:8", "/src/app")
                .unwrap(),
        ];

        assert_eq!(stored, [false, false]);
        assert_eq!(
            calls(&rex),
            [
                "--autostart=false session rename session:9 🌲 q4",
                "--autostart=false window rename --session session:9 window:8 🌲 main"
            ]
        );
    }

    // The shapes below follow git-forest's Rex client; they were not captured
    // from a running Rex here.
    #[test]
    fn tab_cwds_asks_each_block_of_the_tab_and_skips_those_without_one() {
        let rex = rex(&[
            (
                0,
                r#"{"windows":[
                    {"window_id":"window:1","label":"a",
                     "layers":[{"blocks":[{"block_id":"block:9"}]}]},
                    {"window_id":"window:2","label":"b",
                     "layers":[{"blocks":[{"block_id":"block:3"},{"block_id":"block:4"}]},
                               {"blocks":[{"block_id":"block:5"}]}]}]}"#,
            ),
            (0, r#"{"foreground":{"cwd":"/src/app"}}"#),
            (1, "Error: block has no process"),
            (0, r#"{"foreground":null}"#),
        ]);

        let cwds = rex.tab_cwds("session:1", "window:2").unwrap();

        assert_eq!(cwds, [PathBuf::from("/src/app")]);
        assert_eq!(
            calls(&rex)[1..],
            [
                "--autostart=false block call --session session:1 --block block:3 process",
                "--autostart=false block call --session session:1 --block block:4 process",
                "--autostart=false block call --session session:1 --block block:5 process"
            ]
        );
    }

    #[test]
    fn reports_an_unreachable_server_and_other_failures_differently() {
        let rex = rex(&[(4, "cannot connect"), (3, "Error: no session matches")]);

        let unreachable = rex.sessions().unwrap_err();
        let failed = rex.kill_session("session:1").unwrap_err();

        assert!(matches!(unreachable, AppError::BackendUnavailable(_)));
        assert_eq!(
            failed.to_string(),
            "could not kill a Rex session: Error: no session matches"
        );
        assert_eq!(calls(&rex)[1], "--autostart=false kill session:1");
    }
}
