/// Client/helper wire and semantic protocol revision.
///
/// Bump this whenever protobuf compatibility would preserve decoding while changing
/// request meaning or required fields. Initialize rejects every mismatch.
///
/// Revision 7 extends `CliAgentSessionStoreRoots` and discovery semantics for
/// Droid/OpenCode/Copilot/Pi/Cursor/Antigravity persisted sessions.
pub const REMOTE_SERVER_PROTOCOL_REVISION: u32 = 7;

pub mod auth;
pub mod client;
pub mod host_id;
pub mod manager;
pub mod protocol;
pub mod repo_metadata_proto;
pub mod runtime_paths;
pub mod session_execution_context;
pub mod setup;
#[cfg(not(target_family = "wasm"))]
pub mod ssh;
pub mod transport;

pub use host_id::HostId;

pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/remote_server.rs"));
}
