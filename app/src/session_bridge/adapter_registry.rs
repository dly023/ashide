use std::path::Path;

use crate::terminal::cli_agent_session_index::IndexedSession;
use crate::terminal::CLIAgent;

use super::ir::SessionIr;
use super::SessionBridgeError;

#[cfg(feature = "local_fs")]
use super::native_writer::NativeSessionWriteReceipt;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SessionBridgeAdapterCapabilities {
    pub(crate) can_receive_fork: bool,
    pub(crate) can_read_cli_history: bool,
    pub(crate) can_write_native_history: bool,
    pub(crate) can_scan_current_app_history: bool,
}

impl SessionBridgeAdapterCapabilities {
    const ASHIDE: Self = Self {
        can_receive_fork: true,
        can_read_cli_history: false,
        can_write_native_history: false,
        can_scan_current_app_history: false,
    };

    const NATIVE_HISTORY_AGENT: Self = Self {
        can_receive_fork: true,
        can_read_cli_history: true,
        can_write_native_history: true,
        can_scan_current_app_history: true,
    };
}

#[cfg(feature = "local_fs")]
pub(crate) type NativeSessionWriter =
    fn(&SessionIr, &Path) -> Result<NativeSessionWriteReceipt, SessionBridgeError>;

#[cfg(feature = "local_fs")]
pub(crate) type CliSessionReader = fn(
    provider_session_id: &str,
    source_reference: &str,
    bytes: &[u8],
    title_override: Option<String>,
    cwd_override: Option<String>,
) -> Result<SessionIr, SessionBridgeError>;

pub(crate) type CurrentAppSessionScanner = fn(&Path, usize) -> Vec<IndexedSession>;

#[derive(Debug, Clone, Copy)]
pub(crate) struct SessionBridgeAdapter {
    pub(crate) target: SessionBridgeForkTarget,
    pub(crate) agent: Option<CLIAgent>,
    pub(crate) label: &'static str,
    pub(crate) capabilities: SessionBridgeAdapterCapabilities,
    #[cfg(feature = "local_fs")]
    pub(crate) native_writer: Option<NativeSessionWriter>,
    #[cfg(feature = "local_fs")]
    pub(crate) cli_reader: Option<CliSessionReader>,
    pub(crate) current_app_scanner: Option<CurrentAppSessionScanner>,
}

#[cfg(feature = "local_fs")]
const SESSION_BRIDGE_ADAPTERS: &[SessionBridgeAdapter] = &[
    SessionBridgeAdapter {
        target: SessionBridgeForkTarget::Ashide,
        agent: None,
        label: "Ashide",
        capabilities: SessionBridgeAdapterCapabilities::ASHIDE,
        native_writer: None,
        cli_reader: None,
        current_app_scanner: None,
    },
    SessionBridgeAdapter {
        target: SessionBridgeForkTarget::Agent(CLIAgent::Codex),
        agent: Some(CLIAgent::Codex),
        label: "Codex",
        capabilities: SessionBridgeAdapterCapabilities::NATIVE_HISTORY_AGENT,
        native_writer: Some(super::native_writer::write_codex_session),
        cli_reader: Some(super::cli_agent_reader::parse_codex_session_ir),
        current_app_scanner: Some(crate::terminal::cli_agent_session_index::scan_codex_sessions),
    },
    SessionBridgeAdapter {
        target: SessionBridgeForkTarget::Agent(CLIAgent::Claude),
        agent: Some(CLIAgent::Claude),
        label: "Claude",
        capabilities: SessionBridgeAdapterCapabilities::NATIVE_HISTORY_AGENT,
        native_writer: Some(super::native_writer::write_claude_session),
        cli_reader: Some(super::cli_agent_reader::parse_claude_session_ir),
        current_app_scanner: Some(crate::terminal::cli_agent_session_index::scan_claude_sessions),
    },
];

#[cfg(not(feature = "local_fs"))]
const SESSION_BRIDGE_ADAPTERS: &[SessionBridgeAdapter] = &[SessionBridgeAdapter {
    target: SessionBridgeForkTarget::Ashide,
    agent: None,
    label: "Ashide",
    capabilities: SessionBridgeAdapterCapabilities::ASHIDE,
    current_app_scanner: None,
}];

pub(crate) fn session_bridge_adapters() -> &'static [SessionBridgeAdapter] {
    SESSION_BRIDGE_ADAPTERS
}

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
        .filter(|adapter| adapter.capabilities.can_receive_fork)
        .map(|adapter| adapter.target)
}
