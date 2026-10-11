use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("could not determine the home directory: HOME is not set")]
    HomeNotSet,

    #[error("could not read configuration {path}: {source}")]
    ReadConfig {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("could not parse configuration {path}: {source}")]
    ParseConfig {
        path: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },

    #[error("invalid configuration {path}: {message}")]
    InvalidConfig { path: PathBuf, message: String },

    #[error("unknown target `{0}`")]
    UnknownTarget(String),

    #[error("unknown session `{0}`")]
    UnknownSession(String),

    #[error("target `{target}` does not take a prompt: {reason}")]
    PromptNotAccepted {
        target: String,
        reason: &'static str,
    },

    #[error("target `{0}` is already open; a prompt can only start a new tab")]
    TabAlreadyOpen(String),

    #[error("could not read the plan from {origin}: {source}")]
    ReadPlan {
        origin: String,
        #[source]
        source: std::io::Error,
    },

    #[error("could not parse the plan: {0}")]
    ParsePlan(#[source] serde_json::Error),

    #[error("invalid plan: {0}")]
    InvalidPlan(String),

    #[error("multiple sessions or tabs match workspace {name:?}: {candidates}")]
    AmbiguousWorkspace { name: String, candidates: String },

    #[error("multiple tabs in session {session} match the workspace's main tab: {candidates}")]
    AmbiguousMainTab { session: String, candidates: String },

    #[error("directory {} does not exist", .0.display())]
    MissingDirectory(PathBuf),

    #[error("{0}")]
    BackendUnavailable(String),

    #[error("Rex command failed to start: {0}")]
    StartRex(#[source] std::io::Error),

    #[error("{context}: {message}")]
    Rex { context: String, message: String },

    #[error("{context}: could not parse Rex response: {source}")]
    ParseRex {
        context: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("could not write output: {0}")]
    WriteOutput(#[source] std::io::Error),
}

impl AppError {
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::HomeNotSet
            | Self::ReadConfig { .. }
            | Self::ParseConfig { .. }
            | Self::InvalidConfig { .. }
            | Self::UnknownTarget(_)
            | Self::UnknownSession(_)
            | Self::PromptNotAccepted { .. }
            | Self::ReadPlan { .. }
            | Self::ParsePlan(_)
            | Self::InvalidPlan(_) => 2,
            Self::TabAlreadyOpen(_)
            | Self::AmbiguousWorkspace { .. }
            | Self::AmbiguousMainTab { .. }
            | Self::MissingDirectory(_)
            | Self::BackendUnavailable(_)
            | Self::StartRex(_)
            | Self::Rex { .. }
            | Self::ParseRex { .. }
            | Self::WriteOutput(_) => 1,
        }
    }
}

pub type Result<T> = std::result::Result<T, AppError>;
