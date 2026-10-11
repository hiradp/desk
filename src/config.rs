use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use toml::{Table, Value};

use crate::error::{AppError, Result};

const DEFAULT_PATH: &str = ".config/desk/config.toml";
pub const COMMAND_PLACEHOLDER: &str = "{command}";
pub const PROMPT_PLACEHOLDER: &str = "{prompt}";

#[derive(Debug)]
pub struct Config {
    pub backend: BackendKind,
    /// Argv that runs a target's command; exactly one element is `{command}`.
    pub shell: Option<Vec<String>>,
    pub sessions: Vec<Session>,
    /// Every target in build order: the main file first, then each included
    /// file as listed.
    pub targets: Vec<Target>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Rex,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Session {
    pub key: String,
    pub label: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Target {
    pub name: String,
    pub session: String,
    pub icon: String,
    pub dir: PathBuf,
    pub command: Option<String>,
    pub mode: Mode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Startup,
    Demand,
    Multi,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::Demand => "demand",
            Self::Multi => "multi",
        }
    }
}

impl Config {
    pub fn session_label(&self, target: &Target) -> &str {
        self.sessions
            .iter()
            .find(|session| session.key == target.session)
            .map(|session| session.label.as_str())
            .expect("every target's session is configured")
    }

    pub fn load() -> Result<Self> {
        let home = env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .ok_or(AppError::HomeNotSet)?;
        let path = env::var_os("DESK_CONFIG")
            .filter(|value| !value.is_empty())
            .map_or_else(|| home.join(DEFAULT_PATH), PathBuf::from);
        Self::from_path(&path, &home)
    }

    fn from_path(path: &Path, home: &Path) -> Result<Self> {
        let mut loader = Loader::new(home);
        loader.add_file(path, true)?;

        let base = path.parent().unwrap_or(Path::new(""));
        for include in std::mem::take(&mut loader.include) {
            loader.add_file(&base.join(expand_tilde(&include, home)), false)?;
        }
        loader.finish(path)
    }
}

struct Loader<'a> {
    home: &'a Path,
    backend: Option<BackendKind>,
    shell: Option<Vec<String>>,
    include: Vec<String>,
    sessions: Vec<PendingSession>,
    targets: Vec<Target>,
    /// The file each target name was defined in.
    target_files: HashMap<String, PathBuf>,
}

struct PendingSession {
    key: String,
    /// The label and the file that set it.
    label: Option<(String, PathBuf)>,
    first_file: PathBuf,
}

impl<'a> Loader<'a> {
    fn new(home: &'a Path) -> Self {
        Self {
            home,
            backend: None,
            shell: None,
            include: Vec::new(),
            sessions: Vec::new(),
            targets: Vec::new(),
            target_files: HashMap::new(),
        }
    }

    fn add_file(&mut self, path: &Path, main: bool) -> Result<()> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            // An included file may live in a repo this machine does not have.
            Err(error) if !main && error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(source) => {
                return Err(AppError::ReadConfig {
                    path: path.to_owned(),
                    source,
                });
            }
        };
        let table = text
            .parse::<Table>()
            .map_err(|source| AppError::ParseConfig {
                path: path.to_owned(),
                source: Box::new(source),
            })?;

        for (key, value) in table {
            match (key.as_str(), value) {
                ("backend" | "shell" | "include", _) if !main => {
                    return Err(invalid(
                        path,
                        format!("`{key}` can only be set in the main config file"),
                    ));
                }
                ("backend", value) => self.backend = Some(backend(path, value)?),
                ("shell", value) => self.shell = Some(shell(path, value)?),
                ("include", value) => self.include = string_list(path, "include", value)?,
                (_, Value::Table(session)) => self.add_session(path, &key, session)?,
                _ => return Err(invalid(path, format!("unknown key `{key}`"))),
            }
        }
        Ok(())
    }

    fn add_session(&mut self, path: &Path, key: &str, table: Table) -> Result<()> {
        let index = match self.sessions.iter().position(|session| session.key == key) {
            Some(index) => index,
            None => {
                self.sessions.push(PendingSession {
                    key: key.to_owned(),
                    label: None,
                    first_file: path.to_owned(),
                });
                self.sessions.len() - 1
            }
        };

        for (name, value) in table {
            match (name.as_str(), value) {
                ("label", value) => {
                    let label = string(path, &format!("{key}.label"), value)?;
                    if let Some((_, earlier)) = &self.sessions[index].label {
                        return Err(invalid(
                            path,
                            format!(
                                "session `{key}` already has a `label` in {}",
                                earlier.display()
                            ),
                        ));
                    }
                    if let Some(other) = self.sessions.iter().find(
                        |session| matches!(&session.label, Some((taken, _)) if *taken == label),
                    ) {
                        return Err(invalid(
                            path,
                            format!(
                                "sessions `{}` and `{key}` have the same `label` {label:?}",
                                other.key
                            ),
                        ));
                    }
                    self.sessions[index].label = Some((label, path.to_owned()));
                }
                (_, Value::Table(target)) => self.add_target(path, key, name, target)?,
                _ => return Err(invalid(path, format!("unknown key `{key}.{name}`"))),
            }
        }
        Ok(())
    }

    fn add_target(&mut self, path: &Path, session: &str, name: String, table: Table) -> Result<()> {
        if let Some(earlier) = self.target_files.get(&name) {
            return Err(invalid(
                path,
                format!(
                    "target `{name}` is already defined in {}",
                    earlier.display()
                ),
            ));
        }

        let mut icon = None;
        let mut dir = None;
        let mut command = None;
        let mut mode = None;
        for (field, value) in table {
            let key = format!("{session}.{name}.{field}");
            match field.as_str() {
                "icon" => icon = Some(string(path, &key, value)?),
                "dir" => dir = Some(string(path, &key, value)?),
                "command" => command = Some(string(path, &key, value)?),
                "mode" => mode = Some(parse_mode(path, &key, value)?),
                _ => return Err(invalid(path, format!("unknown key `{key}`"))),
            }
        }

        let missing =
            |field: &str| invalid(path, format!("`{session}.{name}` is missing `{field}`"));
        let icon = icon.ok_or_else(|| missing("icon"))?;
        let dir = dir.ok_or_else(|| missing("dir"))?;
        let expanded = expand_tilde(&dir, self.home);
        if !expanded.is_absolute() {
            return Err(invalid(
                path,
                format!("`{session}.{name}.dir` must be absolute or start with `~/`, not {dir:?}"),
            ));
        }

        self.target_files.insert(name.clone(), path.to_owned());
        self.targets.push(Target {
            name,
            session: session.to_owned(),
            icon,
            dir: expanded,
            command,
            mode: mode.unwrap_or(Mode::Demand),
        });
        Ok(())
    }

    fn finish(self, main: &Path) -> Result<Config> {
        let backend = self
            .backend
            .ok_or_else(|| invalid(main, "`backend` is required"))?;

        if self.shell.is_none()
            && let Some(target) = self.targets.iter().find(|target| target.command.is_some())
        {
            return Err(invalid(
                main,
                format!(
                    "`shell` is required because target `{}` has a `command`",
                    target.name
                ),
            ));
        }

        let sessions = self
            .sessions
            .into_iter()
            .map(|session| match session.label {
                Some((label, _)) => Ok(Session {
                    key: session.key,
                    label,
                }),
                None => Err(invalid(
                    &session.first_file,
                    format!("session `{}` has no `label`", session.key),
                )),
            })
            .collect::<Result<_>>()?;

        Ok(Config {
            backend,
            shell: self.shell,
            sessions,
            targets: self.targets,
        })
    }
}

fn backend(path: &Path, value: Value) -> Result<BackendKind> {
    match string(path, "backend", value)?.as_str() {
        "rex" => Ok(BackendKind::Rex),
        other => Err(invalid(path, format!("unsupported `backend` {other:?}"))),
    }
}

fn shell(path: &Path, value: Value) -> Result<Vec<String>> {
    let argv = string_list(path, "shell", value)?;
    let placeholders = argv
        .iter()
        .filter(|argument| *argument == COMMAND_PLACEHOLDER)
        .count();
    if placeholders != 1 {
        return Err(invalid(
            path,
            format!("`shell` must have exactly one `{COMMAND_PLACEHOLDER}` element"),
        ));
    }
    Ok(argv)
}

fn parse_mode(path: &Path, key: &str, value: Value) -> Result<Mode> {
    match string(path, key, value)?.as_str() {
        "startup" => Ok(Mode::Startup),
        "demand" => Ok(Mode::Demand),
        "multi" => Ok(Mode::Multi),
        other => Err(invalid(
            path,
            format!("`{key}` must be `startup`, `demand` or `multi`, not {other:?}"),
        )),
    }
}

fn string(path: &Path, key: &str, value: Value) -> Result<String> {
    match value {
        Value::String(value) => Ok(value),
        _ => Err(invalid(path, format!("`{key}` must be a string"))),
    }
}

fn string_list(path: &Path, key: &str, value: Value) -> Result<Vec<String>> {
    let error = || invalid(path, format!("`{key}` must be a list of strings"));
    let Value::Array(values) = value else {
        return Err(error());
    };
    values
        .into_iter()
        .map(|value| match value {
            Value::String(value) => Ok(value),
            _ => Err(error()),
        })
        .collect()
}

/// Expands a leading `~` or `~/`; `~user` is left alone.
fn expand_tilde(value: &str, home: &Path) -> PathBuf {
    match value.strip_prefix('~') {
        Some("") => home.to_owned(),
        Some(rest) if rest.starts_with('/') => home.join(rest.trim_start_matches('/')),
        _ => PathBuf::from(value),
    }
}

fn invalid(path: &Path, message: impl Into<String>) -> AppError {
    AppError::InvalidConfig {
        path: path.to_owned(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/home/u";

    /// Writes the files into a temporary directory and loads the first one.
    fn load(files: &[(&str, &str)]) -> Result<Config> {
        let dir = tempfile::tempdir().unwrap();
        for (name, text) in files {
            fs::write(dir.path().join(name), text).unwrap();
        }
        Config::from_path(&dir.path().join(files[0].0), Path::new(HOME))
    }

    fn error(files: &[(&str, &str)]) -> String {
        load(files).unwrap_err().to_string()
    }

    fn names(config: &Config) -> Vec<(&str, &str)> {
        config
            .targets
            .iter()
            .map(|target| (target.session.as_str(), target.name.as_str()))
            .collect()
    }

    #[test]
    fn loads_targets_with_defaults_and_expanded_dirs() {
        let config = load(&[(
            "config.toml",
            r#"
            backend = "rex"
            shell = ["/usr/bin/fish", "-C", "{command}"]

            [main]
            label = "🏠 main"

            [main.notes]
            icon = "📝"
            dir = "~/notes"
            mode = "startup"

            [main.app]
            icon = "📦"
            dir = "/src/app"
            command = "agent {prompt}"
            "#,
        )])
        .unwrap();

        assert_eq!(config.backend, BackendKind::Rex);
        assert_eq!(
            config.shell.as_deref(),
            Some(
                ["/usr/bin/fish", "-C", "{command}"]
                    .map(String::from)
                    .as_slice()
            )
        );
        assert_eq!(
            config.sessions,
            [Session {
                key: "main".into(),
                label: "🏠 main".into()
            }]
        );
        assert_eq!(
            config.targets,
            [
                Target {
                    name: "notes".into(),
                    session: "main".into(),
                    icon: "📝".into(),
                    dir: "/home/u/notes".into(),
                    command: None,
                    mode: Mode::Startup,
                },
                Target {
                    name: "app".into(),
                    session: "main".into(),
                    icon: "📦".into(),
                    dir: "/src/app".into(),
                    command: Some("agent {prompt}".into()),
                    mode: Mode::Demand,
                },
            ]
        );
    }

    #[test]
    fn orders_targets_by_file_then_position() {
        let config = load(&[
            (
                "config.toml",
                r#"
                backend = "rex"
                include = ["second.toml", "third.toml"]

                [zeta]
                label = "z"
                [zeta.b]
                icon = "b"
                dir = "/b"

                [alpha.a]
                icon = "a"
                dir = "/a"
                "#,
            ),
            (
                "second.toml",
                r#"
                [zeta.c]
                icon = "c"
                dir = "/c"

                [alpha]
                label = "a"
                "#,
            ),
            (
                "third.toml",
                r#"
                [late]
                label = "l"
                [late.e]
                icon = "e"
                dir = "/e"

                [zeta.d]
                icon = "d"
                dir = "/d"
                "#,
            ),
        ])
        .unwrap();

        assert_eq!(
            names(&config),
            [
                ("zeta", "b"),
                ("alpha", "a"),
                ("zeta", "c"),
                ("late", "e"),
                ("zeta", "d")
            ]
        );
        let sessions: Vec<_> = config
            .sessions
            .iter()
            .map(|session| (session.key.as_str(), session.label.as_str()))
            .collect();
        assert_eq!(sessions, [("zeta", "z"), ("alpha", "a"), ("late", "l")]);
    }

    #[test]
    fn resolves_includes_against_home_and_the_main_file() {
        let home = tempfile::tempdir().unwrap();
        let config_dir = home.path().join("config");
        fs::create_dir_all(config_dir.join("sub")).unwrap();
        fs::write(
            config_dir.join("config.toml"),
            "backend = \"rex\"\ninclude = [\"~/work.toml\", \"sub/more.toml\"]\n[main]\nlabel = \"m\"\n",
        )
        .unwrap();
        fs::write(
            home.path().join("work.toml"),
            "[main.work]\nicon = \"w\"\ndir = \"/w\"\n",
        )
        .unwrap();
        fs::write(
            config_dir.join("sub/more.toml"),
            "[main.more]\nicon = \"m\"\ndir = \"/m\"\n",
        )
        .unwrap();

        let config = Config::from_path(&config_dir.join("config.toml"), home.path()).unwrap();

        assert_eq!(names(&config), [("main", "work"), ("main", "more")]);
    }

    #[test]
    fn skips_an_include_that_does_not_exist() {
        let config = load(&[(
            "config.toml",
            "backend = \"rex\"\ninclude = [\"gone.toml\"]\n\
             [main]\nlabel = \"m\"\n[main.notes]\nicon = \"i\"\ndir = \"/d\"\n",
        )])
        .unwrap();

        assert_eq!(names(&config), [("main", "notes")]);
    }

    #[test]
    fn a_missing_main_file_names_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gone.toml");

        let message = Config::from_path(&path, Path::new(HOME))
            .unwrap_err()
            .to_string();

        assert!(message.contains("gone.toml"), "{message}");
    }

    #[test]
    fn rejects_invalid_main_files() {
        let target = |fields: &str| {
            format!("backend = \"rex\"\n[main]\nlabel = \"m\"\n[main.notes]\n{fields}\n")
        };
        let cases = [
            ("[main]\nlabel = \"m\"".to_owned(), "`backend` is required"),
            (
                "backend = \"tmux\"".to_owned(),
                "unsupported `backend` \"tmux\"",
            ),
            (
                "backend = \"rex\"\nshel = []".to_owned(),
                "unknown key `shel`",
            ),
            (
                "backend = \"rex\"\n[main]\nlabel = \"m\"\nlable = \"x\"".to_owned(),
                "unknown key `main.lable`",
            ),
            (
                target("icon = \"i\"\ndir = \"/d\"\ncomand = \"x\""),
                "unknown key `main.notes.comand`",
            ),
            (
                target("icon = \"i\"\ndir = \"/d\"\n[main.notes.nested]\nx = 1"),
                "unknown key `main.notes.nested`",
            ),
            (target("dir = \"/d\""), "`main.notes` is missing `icon`"),
            (target("icon = \"i\""), "`main.notes` is missing `dir`"),
            (
                target("icon = \"i\"\ndir = \"/d\"\nmode = \"always\""),
                "`main.notes.mode` must be `startup`, `demand` or `multi`",
            ),
            (
                target("icon = \"i\"\ndir = \"src/app\""),
                "`main.notes.dir` must be absolute",
            ),
            (
                target("icon = \"i\"\ndir = \"~other/app\""),
                "`main.notes.dir` must be absolute",
            ),
            (
                target("icon = \"i\"\ndir = \"/d\"\ncommand = \"agent\""),
                "`shell` is required because target `notes` has a `command`",
            ),
            (
                "backend = \"rex\"\nshell = [\"fish\", \"-C\"]".to_owned(),
                "exactly one `{command}` element",
            ),
            (
                "backend = \"rex\"\nshell = [\"fish\", \"-C{command}\"]".to_owned(),
                "exactly one `{command}` element",
            ),
            (
                "backend = \"rex\"\nshell = [\"{command}\", \"{command}\"]".to_owned(),
                "exactly one `{command}` element",
            ),
            (
                "backend = \"rex\"\n[main.notes]\nicon = \"i\"\ndir = \"/d\"".to_owned(),
                "session `main` has no `label`",
            ),
            (
                "backend = \"rex\"\n[a]\nlabel = \"a\"\n[a.x]\nicon = \"i\"\ndir = \"/d\"\n\
                 [b]\nlabel = \"b\"\n[b.x]\nicon = \"i\"\ndir = \"/d\""
                    .to_owned(),
                "target `x` is already defined",
            ),
            (
                "backend = \"rex\"\n[a]\nlabel = \"same\"\n[b]\nlabel = \"same\"".to_owned(),
                "sessions `a` and `b` have the same `label` \"same\"",
            ),
        ];

        for (text, expected) in cases {
            let message = error(&[("config.toml", &text)]);
            assert!(message.contains(expected), "{text}\n→ {message}");
        }
    }

    #[test]
    fn included_files_cannot_set_main_only_keys() {
        for line in [
            "backend = \"rex\"",
            "shell = [\"{command}\"]",
            "include = []",
        ] {
            let message = error(&[
                (
                    "config.toml",
                    "backend = \"rex\"\ninclude = [\"more.toml\"]\n",
                ),
                ("more.toml", line),
            ]);

            assert!(message.contains("more.toml"), "{message}");
            assert!(
                message.contains("can only be set in the main config file"),
                "{message}"
            );
        }
    }

    #[test]
    fn rejects_a_label_or_target_repeated_in_an_included_file() {
        let main = "backend = \"rex\"\ninclude = [\"more.toml\"]\n\
                    [main]\nlabel = \"m\"\n[main.notes]\nicon = \"i\"\ndir = \"/d\"\n";
        let cases = [
            (
                "[main]\nlabel = \"again\"",
                "session `main` already has a `label`",
            ),
            (
                "[other]\nlabel = \"o\"\n[other.notes]\nicon = \"i\"\ndir = \"/d\"",
                "target `notes` is already defined",
            ),
        ];

        for (included, expected) in cases {
            let message = error(&[("config.toml", main), ("more.toml", included)]);
            assert!(message.contains(expected), "{message}");
            assert!(message.contains("config.toml"), "{message}");
        }
    }
}
