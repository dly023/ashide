//! Location-agnostic descriptor of a forkable session source.
//!
//! A session that can be forked / edited-and-forked / exported is identified by
//! two orthogonal facts:
//!
//! * WHERE it lives — [`SessionLocus`]: an AI conversation, or a CLI-agent
//!   transcript (codex / claude / …).
//! * WHICH backend owns it — `authority`: `None` for the local current-app
//!   store, `Some(authority)` for a connected remote environment runtime.
//!
//! Every UI entry point (a conversation id, a live pane locator, a
//! navigator-row target) resolves to a single [`SessionSourceRef`]. The
//! operations themselves (fork / edit / export) then act on that ref and never
//! branch on the entry point or on local-vs-remote: the local/remote decision
//! lives in exactly one place, [`SessionBackendKind::for_authority`].
//!
//! This module is intentionally pure data + pure classification so the
//! entry-point → (locus, backend) mapping can be pinned by unit tests without a
//! running `Workspace`.

use crate::ai::agent::conversation::AIConversationId;
use crate::terminal::CLIAgent;

/// A CLI-agent transcript source, parsed from a source id.
///
/// This unifies what used to be two structurally-identical twin types
/// (`EnvironmentCliAgentSessionSourceTarget` and
/// `CurrentAppCliAgentSessionSourceTarget`). They only ever differed by which
/// store owned them, which is now carried separately as
/// [`SessionSourceRef::authority`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliAgentSourceTarget {
    pub source: String,
    pub agent: Option<CLIAgent>,
    pub provider_session_id: Option<String>,
}

/// Where a forkable session physically lives (independent of which store owns
/// it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionLocus {
    /// An AI conversation tracked by the app's history model.
    Conversation(AIConversationId),
    /// A CLI-agent transcript addressed by its source target.
    CliAgent(CliAgentSourceTarget),
}
