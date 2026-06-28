use std::path::PathBuf;

use clap::{Args, Subcommand};

/// Inspect and export local Ashide sessions through the SessionBridge format.
#[derive(Debug, Clone, Subcommand)]
pub enum SessionBridgeCommand {
    /// List exportable local sessions.
    List,

    /// Export a local session to a portable SessionBridge bundle.
    Export(SessionBridgeExportArgs),

    /// Fork an existing local session directly into native Ashide session history.
    Fork(SessionBridgeForkArgs),

    /// Write an edited copy of an existing local session into native Ashide session history.
    Edit(SessionBridgeEditArgs),

    /// Import a SessionBridge bundle into native Ashide session history.
    Import(SessionBridgeImportArgs),
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct SessionBridgeExportArgs {
    /// Persisted Ashide session/conversation id to export.
    #[arg(long = "session")]
    pub session: String,

    /// Output file or directory for the exported bundle.
    #[arg(long = "out")]
    pub out: Option<PathBuf>,

    /// Print the export preview without writing a bundle.
    #[arg(long = "dry-run")]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct SessionBridgeForkArgs {
    /// Persisted Ashide session/conversation id to fork.
    #[arg(long = "session")]
    pub session: String,

    /// Explicit id for the derived fork session. Defaults to a fresh UUID.
    #[arg(long = "new-session")]
    pub new_session: Option<String>,

    /// Print the fork plan without writing to native Ashide persistence.
    #[arg(long = "dry-run")]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct SessionBridgeEditArgs {
    /// Persisted Ashide session/conversation id to edit.
    #[arg(long = "session")]
    pub session: String,

    /// Explicit id for the derived edited session. Defaults to a fresh UUID.
    #[arg(long = "new-session")]
    pub new_session: Option<String>,

    /// Literal text to redact from messages and artifacts. Can be repeated.
    #[arg(long = "redact")]
    pub redact: Vec<String>,

    /// Keep only the first N messages in the derived session.
    #[arg(long = "trim-after")]
    pub trim_after: Option<usize>,

    /// Print the edit plan without writing to native Ashide persistence.
    #[arg(long = "dry-run")]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct SessionBridgeImportArgs {
    /// SessionBridge bundle JSON file to import.
    #[arg(long = "bundle")]
    pub bundle: PathBuf,

    /// Explicit native Ashide session id. Defaults to the bundle session id.
    #[arg(long = "new-session")]
    pub new_session: Option<String>,

    /// Print the import plan without writing to native Ashide persistence.
    #[arg(long = "dry-run")]
    pub dry_run: bool,
}
