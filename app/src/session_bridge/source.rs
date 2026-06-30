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

/// Which backend owns a session — the single local-vs-remote axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionBackendKind {
    /// The current-app local filesystem history.
    Local,
    /// A connected remote environment runtime, reached over daemon RPC.
    Environment,
}

impl SessionBackendKind {
    /// The sole local-vs-remote decision in the session-bridge stack: a session
    /// is owned by a remote environment iff its `authority` names a runtime
    /// environment (i.e. anything other than the local terminal-bootstrap
    /// authority). `None` is always local.
    pub fn for_authority(authority: Option<&str>) -> Self {
        if crate::workspace::environment_runtime::session_authority_uses_runtime_environment(
            authority,
        ) {
            SessionBackendKind::Environment
        } else {
            SessionBackendKind::Local
        }
    }
}

/// Fully resolved, entry-point-agnostic handle to a forkable session source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSourceRef {
    pub locus: SessionLocus,
    /// `None` => local / current-app; `Some(authority)` => remote environment.
    pub authority: Option<String>,
    /// Best-effort display title hint (alias / label / source title).
    pub title: Option<String>,
    /// Best-effort working-directory hint.
    pub cwd: Option<String>,
}

impl SessionSourceRef {
    /// Which backend should service operations on this source.
    pub fn backend_kind(&self) -> SessionBackendKind {
        SessionBackendKind::for_authority(self.authority.as_deref())
    }
}

/// The single source-classification rule that every entry point funnels
/// through.
///
/// Given the ingredients an entry point can resolve — an optional in-context
/// conversation id, an optional CLI-agent backing source, and the owning
/// authority — produce the location-agnostic [`SessionSourceRef`], or `None`
/// when the session is not forkable.
///
/// Precedence mirrors the historical resolution: a conversation id always wins
/// over a CLI-agent backing.
pub fn classify_session_source(
    conversation_id: Option<AIConversationId>,
    cli_agent_source: Option<CliAgentSourceTarget>,
    authority: Option<String>,
    title: Option<String>,
    cwd: Option<String>,
) -> Option<SessionSourceRef> {
    let locus = match conversation_id {
        Some(conversation_id) => SessionLocus::Conversation(conversation_id),
        None => SessionLocus::CliAgent(cli_agent_source?),
    };
    Some(SessionSourceRef {
        locus,
        authority,
        title,
        cwd,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cli_source() -> CliAgentSourceTarget {
        CliAgentSourceTarget {
            source: "session.jsonl".to_owned(),
            agent: Some(CLIAgent::Codex),
            provider_session_id: Some("psid-1".to_owned()),
        }
    }

    // --- backend axis: the single local-vs-remote decision -------------------

    #[test]
    fn backend_is_local_without_authority() {
        assert_eq!(
            SessionBackendKind::for_authority(None),
            SessionBackendKind::Local
        );
    }

    #[test]
    fn backend_is_environment_for_runtime_authority() {
        assert_eq!(
            SessionBackendKind::for_authority(Some("root@dnyx216")),
            SessionBackendKind::Environment
        );
    }

    // --- truth table: entry ingredients -> (locus, authority) ----------------

    #[test]
    fn conversation_id_wins_over_cli_backing() {
        // A conversation id present alongside a CLI backing resolves to the
        // conversation locus, never the CLI one.
        let id = AIConversationId::new();
        let resolved =
            classify_session_source(Some(id), Some(cli_source()), None, None, None).unwrap();
        assert_eq!(resolved.locus, SessionLocus::Conversation(id));
        assert_eq!(resolved.backend_kind(), SessionBackendKind::Local);
    }

    #[test]
    fn remote_cli_backing_resolves_to_environment() {
        // No conversation, CLI backing under a remote authority => CliAgent
        // locus serviced by the Environment backend.
        let resolved = classify_session_source(
            None,
            Some(cli_source()),
            Some("root@dnyx216".to_owned()),
            None,
            None,
        )
        .unwrap();
        assert_eq!(resolved.locus, SessionLocus::CliAgent(cli_source()));
        assert_eq!(resolved.backend_kind(), SessionBackendKind::Environment);
    }

    #[test]
    fn local_cli_backing_resolves_to_local() {
        let resolved = classify_session_source(None, Some(cli_source()), None, None, None).unwrap();
        assert_eq!(resolved.locus, SessionLocus::CliAgent(cli_source()));
        assert_eq!(resolved.backend_kind(), SessionBackendKind::Local);
    }

    #[test]
    fn no_conversation_and_no_cli_backing_is_not_forkable() {
        assert!(classify_session_source(None, None, None, None, None).is_none());
    }

    #[test]
    fn conversation_carries_remote_authority() {
        // A conversation owned by a remote environment is serviced by the
        // Environment backend, just like a remote CLI source.
        let id = AIConversationId::new();
        let resolved =
            classify_session_source(Some(id), None, Some("root@dnyx216".to_owned()), None, None)
                .unwrap();
        assert_eq!(resolved.locus, SessionLocus::Conversation(id));
        assert_eq!(resolved.backend_kind(), SessionBackendKind::Environment);
    }
}
