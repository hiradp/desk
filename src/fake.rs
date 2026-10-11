//! An in-memory backend for tests. Unlike Rex it stores keys, unless told to
//! behave as a backend that cannot.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use crate::backend::{Backend, CreatedSession, NewTab, SessionRef, TabRef};
use crate::error::{AppError, Result};

#[derive(Default)]
pub struct Fake {
    pub state: RefCell<State>,
}

#[derive(Default)]
pub struct State {
    pub down: bool,
    /// Drop keys, as a backend with nowhere to store them does.
    pub keyless: bool,
    /// Fail to show sessions, as a backend with no app connected does.
    pub headless: bool,
    next_id: u32,
    pub current: Option<String>,
    pub sessions: Vec<FakeSession>,
    /// How many times the sessions were listed.
    pub listings: u32,
    /// Tab labels in the order they were focused.
    pub focused: Vec<String>,
    /// Session labels in the order they were shown.
    pub shown: Vec<String>,
    /// Session labels in the order they were killed.
    pub killed: Vec<String>,
}

pub struct FakeSession {
    pub id: String,
    pub label: String,
    pub key: Option<String>,
    pub tabs: Vec<FakeTab>,
}

pub struct FakeTab {
    pub id: String,
    pub label: String,
    pub key: Option<String>,
    pub dir: PathBuf,
    /// What its panes report; a new tab has one pane, in `dir`.
    pub cwds: Vec<PathBuf>,
    pub argv: Option<Vec<String>>,
}

impl State {
    fn id(&mut self) -> String {
        self.next_id += 1;
        format!("id-{}", self.next_id)
    }

    fn key(&self, key: Option<&str>) -> Option<String> {
        key.filter(|_| !self.keyless).map(str::to_owned)
    }

    fn tab(&mut self, tab: NewTab<'_>) -> FakeTab {
        FakeTab {
            id: self.id(),
            label: tab.label.to_owned(),
            key: self.key(tab.key),
            dir: tab.dir.to_owned(),
            cwds: vec![tab.dir.to_owned()],
            argv: tab.argv.map(<[String]>::to_vec),
        }
    }

    fn session(&mut self, session_id: &str) -> &mut FakeSession {
        self.sessions
            .iter_mut()
            .find(|session| session.id == session_id)
            .unwrap()
    }

    fn existing_tab(&mut self, session_id: &str, tab_id: &str) -> &mut FakeTab {
        self.session(session_id)
            .tabs
            .iter_mut()
            .find(|tab| tab.id == tab_id)
            .unwrap()
    }
}

impl Fake {
    /// Adds a session as if it had been created outside desk.
    pub fn seed(&self, label: &str, tabs: &[&str]) -> String {
        let mut state = self.state.borrow_mut();
        let tabs = tabs
            .iter()
            .map(|label| {
                state.tab(NewTab {
                    label,
                    dir: Path::new("/"),
                    argv: None,
                    key: None,
                })
            })
            .collect();
        let id = state.id();
        state.sessions.push(FakeSession {
            id: id.clone(),
            label: label.to_owned(),
            key: None,
            tabs,
        });
        id
    }

    pub fn layout(&self) -> Vec<(String, Vec<String>)> {
        let state = self.state.borrow();
        state
            .sessions
            .iter()
            .map(|session| {
                let tabs = session.tabs.iter().map(|tab| tab.label.clone()).collect();
                (session.label.clone(), tabs)
            })
            .collect()
    }

    /// Runs `inspect` on the one tab with this label.
    pub fn tab<T>(&self, label: &str, inspect: impl FnOnce(&mut FakeTab) -> T) -> T {
        let mut state = self.state.borrow_mut();
        let mut tabs = state
            .sessions
            .iter_mut()
            .flat_map(|session| &mut session.tabs)
            .filter(|tab| tab.label == label);
        let tab = tabs.next().unwrap();
        assert!(
            tabs.next().is_none(),
            "more than one tab is labeled {label}"
        );
        inspect(tab)
    }

    pub fn argv(&self, label: &str) -> Option<Vec<String>> {
        self.tab(label, |tab| tab.argv.clone())
    }

    pub fn focused(&self) -> Vec<String> {
        self.state.borrow().focused.clone()
    }
}

impl Backend for Fake {
    fn current_session(&self) -> Option<String> {
        self.state.borrow().current.clone()
    }

    fn sessions(&self) -> Result<Vec<SessionRef>> {
        let mut state = self.state.borrow_mut();
        if state.down {
            return Err(AppError::BackendUnavailable("fake is down".into()));
        }
        state.listings += 1;
        Ok(state
            .sessions
            .iter()
            .map(|session| SessionRef {
                id: session.id.clone(),
                label: session.label.clone(),
                key: session.key.clone(),
            })
            .collect())
    }

    fn create_session(
        &self,
        label: &str,
        key: Option<&str>,
        tab: NewTab<'_>,
    ) -> Result<CreatedSession> {
        let mut state = self.state.borrow_mut();
        let tab = state.tab(tab);
        let created = CreatedSession {
            session_id: state.id(),
            tab_id: tab.id.clone(),
        };
        let key = state.key(key);
        state.sessions.push(FakeSession {
            id: created.session_id.clone(),
            label: label.to_owned(),
            key,
            tabs: vec![tab],
        });
        Ok(created)
    }

    fn kill_session(&self, session_id: &str) -> Result<()> {
        let mut state = self.state.borrow_mut();
        let index = state
            .sessions
            .iter()
            .position(|session| session.id == session_id)
            .unwrap();
        let label = state.sessions.remove(index).label;
        state.killed.push(label);
        Ok(())
    }

    fn rename_session(&self, session_id: &str, label: &str) -> Result<()> {
        self.state.borrow_mut().session(session_id).label = label.to_owned();
        Ok(())
    }

    fn set_session_key(&self, session_id: &str, key: &str) -> Result<bool> {
        let mut state = self.state.borrow_mut();
        let key = state.key(Some(key));
        let stored = key.is_some();
        if stored {
            state.session(session_id).key = key;
        }
        Ok(stored)
    }

    fn tabs(&self, session_id: &str) -> Result<Vec<TabRef>> {
        let mut state = self.state.borrow_mut();
        Ok(state
            .session(session_id)
            .tabs
            .iter()
            .map(|tab| TabRef {
                id: tab.id.clone(),
                label: tab.label.clone(),
                key: tab.key.clone(),
            })
            .collect())
    }

    fn open_tab(&self, session_id: &str, tab: NewTab<'_>) -> Result<String> {
        let mut state = self.state.borrow_mut();
        let tab = state.tab(tab);
        let id = tab.id.clone();
        state.session(session_id).tabs.push(tab);
        Ok(id)
    }

    fn rename_tab(&self, session_id: &str, tab_id: &str, label: &str) -> Result<()> {
        let mut state = self.state.borrow_mut();
        state.existing_tab(session_id, tab_id).label = label.to_owned();
        Ok(())
    }

    fn set_tab_key(&self, session_id: &str, tab_id: &str, key: &str) -> Result<bool> {
        let mut state = self.state.borrow_mut();
        let key = state.key(Some(key));
        let stored = key.is_some();
        if stored {
            state.existing_tab(session_id, tab_id).key = key;
        }
        Ok(stored)
    }

    fn tab_cwds(&self, session_id: &str, tab_id: &str) -> Result<Vec<PathBuf>> {
        let mut state = self.state.borrow_mut();
        Ok(state.existing_tab(session_id, tab_id).cwds.clone())
    }

    fn focus_tab(&self, session_id: &str, tab_id: &str) -> Result<()> {
        let mut state = self.state.borrow_mut();
        let label = state.existing_tab(session_id, tab_id).label.clone();
        state.focused.push(label);
        Ok(())
    }

    fn show_session(&self, session_id: &str, _tab_id: &str) -> Result<()> {
        let mut state = self.state.borrow_mut();
        if state.headless {
            return Err(AppError::BackendUnavailable("no app is connected".into()));
        }
        let label = state.session(session_id).label.clone();
        state.shown.push(label);
        Ok(())
    }
}
