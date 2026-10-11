use std::path::{Path, PathBuf};

use crate::error::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRef {
    pub id: String,
    pub label: String,
    /// The key stored on the session, if the backend stores keys and it has
    /// one.
    pub key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabRef {
    pub id: String,
    pub label: String,
    pub key: Option<String>,
}

/// A full-width tab to open in `dir`, running `argv` or, without one, whatever
/// the backend opens by default.
#[derive(Debug, Clone, Copy)]
pub struct NewTab<'a> {
    pub label: &'a str,
    pub dir: &'a Path,
    pub argv: Option<&'a [String]>,
    /// Stored on the tab by a backend that stores keys.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "Rex, the only backend, cannot store keys")
    )]
    pub key: Option<&'a str>,
}

#[derive(Debug)]
pub struct CreatedSession {
    pub session_id: String,
    pub tab_id: String,
}

/// A multiplexer. Sessions and tabs are addressed by the ids it hands out;
/// labels and keys are only for finding them. A key outlives a rename, but
/// not every backend can store one.
pub trait Backend {
    /// The session desk is running inside, if any.
    fn current_session(&self) -> Option<String>;

    /// Fails with `BackendUnavailable` when the multiplexer is not running.
    fn sessions(&self) -> Result<Vec<SessionRef>>;

    /// Creates a session whose first tab is `tab`, without showing it.
    fn create_session(
        &self,
        label: &str,
        key: Option<&str>,
        tab: NewTab<'_>,
    ) -> Result<CreatedSession>;

    fn kill_session(&self, session_id: &str) -> Result<()>;

    fn rename_session(&self, session_id: &str, label: &str) -> Result<()>;

    /// Returns whether the key was stored; a backend without keys stores
    /// nothing and returns false.
    fn set_session_key(&self, session_id: &str, key: &str) -> Result<bool>;

    fn tabs(&self, session_id: &str) -> Result<Vec<TabRef>>;

    /// Opens a tab without focusing it and returns its id.
    fn open_tab(&self, session_id: &str, tab: NewTab<'_>) -> Result<String>;

    fn rename_tab(&self, session_id: &str, tab_id: &str, label: &str) -> Result<()>;

    /// Returns whether the key was stored, as `set_session_key` does.
    fn set_tab_key(&self, session_id: &str, tab_id: &str, key: &str) -> Result<bool>;

    /// The working directory of each pane in the tab that reports one. Empty
    /// when the backend cannot know.
    fn tab_cwds(&self, session_id: &str, tab_id: &str) -> Result<Vec<PathBuf>>;

    /// Makes the tab the active one in its session.
    fn focus_tab(&self, session_id: &str, tab_id: &str) -> Result<()>;

    /// Shows the session to the user, on the given tab. Fails when the backend
    /// cannot change what the user sees; the tab is open either way.
    fn show_session(&self, session_id: &str, tab_id: &str) -> Result<()>;
}
