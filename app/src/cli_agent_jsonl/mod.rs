//! Shared persisted-session discovery primitives for CLI agents.
//!
//! Consumers parse the same on-disk transcript/index formats:
//!
//! - [`crate::session_bridge::cli_agent_reader`] runs locally and parses a
//!   transcript into a full `SessionIr` (every user/assistant message).
//! - [`crate::environment_runtime_transport::cli_agent_sessions`] runs natively
//!   inside the daemon on the remote host and scans / reads / mutates the stores.
//!
//! JSONL discovery, decoding and lightweight session metadata extraction are
//! shared here because local and remote scans must either observe the same
//! complete store snapshot or fail without replacing their previous cache.
//! An agent whose native store has not been provisioned yet contributes a
//! successful empty set; a missing store is not a transient scan failure.
//! Path allow-listing stays at the mutation/read boundaries because it encodes
//! a different security responsibility.

#[cfg(feature = "local_fs")]
mod discovery;
#[cfg(feature = "local_fs")]
mod error;
mod parse;
#[cfg(feature = "local_fs")]
mod policy;
mod roots;

#[allow(unused_imports)]
#[cfg(feature = "local_fs")]
pub(crate) use discovery::{
    recent_jsonl_files, AgentSessionDiscoveryPlan, AgentSessionDiscoveryProvider,
    AgentSessionDiscoveryRecord, AgentSessionDiscoveryResult, AgentSessionDiscoverySource,
    AgentSessionDiscoveryTransition,
};
#[cfg(feature = "local_fs")]
pub(crate) use error::CliAgentSessionScanError;
#[allow(unused_imports)]
#[cfg(feature = "local_fs")]
pub(crate) use parse::read_jsonl_values_from_path;
#[allow(unused_imports)]
pub(crate) use parse::{
    canonical_codex_session_id, codex_session_index_record, CodexSessionIndexRecord,
};
#[allow(unused_imports)]
pub use parse::{
    claude_session_metadata, claude_user_message_from_item, codex_session_metadata,
    codex_title_from_item, codex_user_message_from_item, first_message_excerpt, nested_string,
    parse_jsonl_values, sha256_hex, CliAgentSessionMetadata,
};
#[allow(unused_imports)]
#[cfg(feature = "local_fs")]
pub(crate) use policy::{limit_cli_agent_session_sources, CliAgentSessionSource, RecentJsonlFile};
#[allow(unused_imports)]
#[cfg(feature = "local_fs")]
pub(crate) use roots::{
    current_cli_agent_home, normalize_cli_agent_session_cwd, require_cli_agent_home,
    resolve_current_process_cli_agent_store_roots,
};
pub(crate) use roots::{is_omp_session_source, CliAgentStoreRoots};

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
