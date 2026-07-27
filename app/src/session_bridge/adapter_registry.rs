use crate::terminal::CLIAgent;

use super::ir::SessionIr;
use super::SessionBridgeError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SessionBridgeForkTarget {
    Ashide,
    Agent(CLIAgent),
}

impl SessionBridgeForkTarget {
    pub fn display_label(self) -> &'static str {
        session_bridge_adapter_for_target(self)
            .map(|adapter| adapter.label)
            .unwrap_or("Unsupported Agent")
    }
}

#[cfg(feature = "local_fs")]
pub(crate) type CliSessionReader = fn(
    provider_session_id: &str,
    source_reference: &str,
    bytes: &[u8],
    title_override: Option<String>,
    cwd_override: Option<String>,
) -> Result<SessionIr, SessionBridgeError>;

#[derive(Debug, Clone, Copy)]
pub(crate) struct SessionBridgeAdapter {
    pub(crate) target: SessionBridgeForkTarget,
    pub(crate) agent: Option<CLIAgent>,
    pub(crate) label: &'static str,
    #[cfg(feature = "local_fs")]
    pub(crate) cli_reader: Option<CliSessionReader>,
}

#[cfg(feature = "local_fs")]
const SESSION_BRIDGE_ADAPTERS: &[SessionBridgeAdapter] = &[
    SessionBridgeAdapter {
        target: SessionBridgeForkTarget::Ashide,
        agent: None,
        label: "Ashide",
        cli_reader: None,
    },
    SessionBridgeAdapter {
        target: SessionBridgeForkTarget::Agent(CLIAgent::Codex),
        agent: Some(CLIAgent::Codex),
        label: "Codex",
        cli_reader: Some(super::cli_agent_reader::parse_codex_session_ir),
    },
    SessionBridgeAdapter {
        target: SessionBridgeForkTarget::Agent(CLIAgent::Claude),
        agent: Some(CLIAgent::Claude),
        label: "Claude",
        cli_reader: Some(super::cli_agent_reader::parse_claude_session_ir),
    },
];

#[cfg(not(feature = "local_fs"))]
const SESSION_BRIDGE_ADAPTERS: &[SessionBridgeAdapter] = &[SessionBridgeAdapter {
    target: SessionBridgeForkTarget::Ashide,
    agent: None,
    label: "Ashide",
}];

pub(crate) fn session_bridge_adapter_for_target(
    target: SessionBridgeForkTarget,
) -> Option<&'static SessionBridgeAdapter> {
    SESSION_BRIDGE_ADAPTERS
        .iter()
        .find(|adapter| adapter.target == target)
}

pub(crate) fn session_bridge_adapter_for_agent(
    agent: CLIAgent,
) -> Option<&'static SessionBridgeAdapter> {
    SESSION_BRIDGE_ADAPTERS
        .iter()
        .find(|adapter| adapter.agent == Some(agent))
}

pub(crate) fn session_bridge_fork_targets(
) -> impl Iterator<Item = SessionBridgeForkTarget> + 'static {
    SESSION_BRIDGE_ADAPTERS
        .iter()
        .filter(|adapter| {
            adapter
                .agent
                .is_none_or(|agent| agent.capabilities().can_fork)
        })
        .map(|adapter| adapter.target)
}
