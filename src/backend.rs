use std::path::Path;

use crate::error::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRef {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabRef {
    pub id: String,
    pub label: String,
}

/// A full-width tab to open in `dir`, running `argv` or, without one, whatever
/// the backend opens by default.
#[derive(Debug, Clone, Copy)]
pub struct NewTab<'a> {
    pub label: &'a str,
    pub dir: &'a Path,
    pub argv: Option<&'a [String]>,
}

#[derive(Debug)]
pub struct CreatedSession {
    pub session_id: String,
    pub tab_id: String,
}

/// A multiplexer. Sessions and tabs are addressed by the ids it hands out;
/// labels are only for finding them.
pub trait Backend {
    /// The session desk is running inside, if any.
    fn current_session(&self) -> Option<String>;

    /// Fails with `BackendUnavailable` when the multiplexer is not running.
    fn sessions(&self) -> Result<Vec<SessionRef>>;

    /// Creates a session whose first tab is `tab`, without showing it.
    fn create_session(&self, label: &str, tab: NewTab<'_>) -> Result<CreatedSession>;

    fn kill_session(&self, session_id: &str) -> Result<()>;

    fn tabs(&self, session_id: &str) -> Result<Vec<TabRef>>;

    /// Opens a tab without focusing it and returns its id.
    fn open_tab(&self, session_id: &str, tab: NewTab<'_>) -> Result<String>;

    /// Makes the tab active and shows its session to the user.
    fn focus_tab(&self, session_id: &str, tab_id: &str) -> Result<()>;
}
