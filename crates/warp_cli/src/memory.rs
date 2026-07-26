use clap::{Args, Subcommand, ValueEnum};

/// Recall project memory from `.agents/memory`.
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct MemoryRecallArgs {
    /// Use summary-only recall.
    #[arg(long, conflicts_with_all = ["focused", "full"])]
    pub fast: bool,

    /// Use focused recall with FTS results. This is the default.
    #[arg(long, conflicts_with_all = ["fast", "full"])]
    pub focused: bool,

    /// Use full recall with future freshness checks.
    #[arg(long, conflicts_with_all = ["fast", "focused"])]
    pub full: bool,

    /// Maximum memories to return.
    #[arg(long = "top-k", default_value_t = 8)]
    pub top_k: usize,

    /// Emit machine-readable JSON for this command.
    #[arg(long = "json")]
    pub json: bool,

    /// Optional search query.
    #[arg(value_name = "QUERY", num_args = 0..)]
    pub query: Vec<String>,
}

/// Assemble a bounded agent context packet from project memory.
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct MemoryContextArgs {
    #[command(flatten)]
    pub recall: MemoryRecallArgs,

    /// Current user task to label in the assembled packet.
    #[arg(long = "task", allow_hyphen_values = true)]
    pub task: Option<String>,

    /// Approximate token budget for the whole context packet.
    #[arg(long = "token-budget", default_value_t = 1200)]
    pub token_budget: usize,
}

/// Build a read-only editor recall preview from memory and evidence.
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct MemoryPreviewArgs {
    #[command(flatten)]
    pub recall: MemoryRecallArgs,

    /// Current editor/user task to label in the preview.
    #[arg(long = "task", allow_hyphen_values = true)]
    pub task: Option<String>,

    /// Approximate token budget for the memory context part.
    #[arg(long = "token-budget", default_value_t = 1200)]
    pub token_budget: usize,

    /// Maximum non-memory evidence citations to include.
    #[arg(long = "evidence-limit", default_value_t = 8)]
    pub evidence_limit: usize,
}

/// List recent non-memory evidence citations.
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct MemoryEvidenceArgs {
    /// Maximum evidence citations to return.
    #[arg(long = "limit", default_value_t = 8)]
    pub limit: usize,

    /// Emit machine-readable JSON for this command.
    #[arg(long = "json")]
    pub json: bool,
}

impl MemoryRecallArgs {
    pub fn tier(&self) -> MemoryRecallTier {
        if self.fast {
            MemoryRecallTier::Fast
        } else if self.full {
            MemoryRecallTier::Full
        } else {
            MemoryRecallTier::Focused
        }
    }

    pub fn query_text(&self) -> Option<String> {
        let query = self.query.join(" ");
        let query = query.trim();
        if query.is_empty() {
            None
        } else {
            Some(query.to_owned())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryRecallTier {
    Fast,
    Focused,
    Full,
}

impl MemoryRecallTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Focused => "focused",
            Self::Full => "full",
        }
    }
}

/// Write a durable project memory.
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct MemoryWriteArgs {
    /// Memory kind.
    #[arg(long = "kind", value_enum, default_value_t = MemoryKind::Fact)]
    pub kind: MemoryKind,

    /// Memory text.
    #[arg(long = "text", allow_hyphen_values = true)]
    pub text: String,

    /// Source reference, for example `file:docs/foo.md` or `user`.
    #[arg(long = "source")]
    pub source: Option<String>,
}

/// Agent-friendly memory write surface.
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct RememberArgs {
    /// Optional kind followed by memory text. If the first word is not a known
    /// kind, the memory is recorded as a fact.
    #[arg(value_name = "TEXT", num_args = 1.., allow_hyphen_values = true)]
    pub words: Vec<String>,
}

impl RememberArgs {
    pub fn into_write_args(self) -> anyhow::Result<MemoryWriteArgs> {
        let mut words = self.words;
        if words.is_empty() {
            anyhow::bail!("remember requires memory text");
        }

        let kind = words
            .first()
            .and_then(|word| MemoryKind::from_token(word.as_str()));

        let text_words = if kind.is_some() {
            words.split_off(1)
        } else {
            words
        };
        let text = text_words.join(" ");
        let text = text.trim();
        if text.is_empty() {
            anyhow::bail!("remember requires memory text");
        }

        Ok(MemoryWriteArgs {
            kind: kind.unwrap_or(MemoryKind::Fact),
            text: text.to_owned(),
            source: None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum MemoryKind {
    Fact,
    Decision,
    Task,
    Failure,
    Preference,
}

impl MemoryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fact => "fact",
            Self::Decision => "decision",
            Self::Task => "task",
            Self::Failure => "failure",
            Self::Preference => "preference",
        }
    }

    pub fn from_token(token: &str) -> Option<Self> {
        match token.to_ascii_lowercase().as_str() {
            "fact" | "rule" => Some(Self::Fact),
            "decision" => Some(Self::Decision),
            "task" | "todo" => Some(Self::Task),
            "failure" | "lesson" => Some(Self::Failure),
            "preference" | "pref" => Some(Self::Preference),
            _ => None,
        }
    }
}

/// Structured project memory commands.
#[derive(Debug, Clone, Subcommand)]
pub enum MemoryCommand {
    /// Show memory store status.
    Status,

    /// Run a local project-memory MCP server over stdio.
    #[command(name = "mcp-server")]
    McpServer,

    /// Write a durable memory event.
    Write(MemoryWriteArgs),

    /// Recall durable project memories.
    Recall(MemoryRecallArgs),

    /// Assemble a bounded context packet from durable project memory.
    Context(MemoryContextArgs),

    /// Build a read-only editor recall preview with memory and evidence citations.
    Preview(MemoryPreviewArgs),

    /// List recent non-memory evidence citations.
    Evidence(MemoryEvidenceArgs),
}
