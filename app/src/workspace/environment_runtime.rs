//! Environment seam: resolves *where* a workspace session or terminal runs.
//!
//! An "environment" is either the user's local machine or a remote host. This
//! module is the single place that discriminates between the two, keyed off the
//! `authority` string carried on `EnvironmentSnapshot`:
//!
//! - `"local"` / `"local:<root>"` -> **terminal bootstrap** (local). Backed by the
//!   `TerminalBootstrap*` types in this module; execution is a locally spawned
//!   shell with no transport. Constructed via `terminal_bootstrap_environment` and
//!   friends; tested with `uses_terminal_bootstrap` / `authority_uses_terminal_bootstrap`.
//! - any other authority (e.g. `"ssh:..."`) -> **environment runtime** (remote).
//!   Backed by the `EnvironmentRuntime*` types, which are `pub(crate)` aliases over
//!   `environment_runtime_transport`'s `RemoteServer*` types (SSH / RPC transport).
//!   Tested with `uses_environment_runtime` / `session_authority_uses_runtime_environment`.
//!
//! Naming caveat: `EnvironmentRuntime` is **only the remote half**, not an umbrella
//! over both backends -- it shares a prefix with the module name for historical
//! reasons (the remote runtime landed first). The local half is `TerminalBootstrap*`.
//! New call sites should branch through the `uses_*` predicates above rather than
//! re-deriving local-vs-remote inline.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use warp_core::features::FeatureFlag;
use warp_core::{HostId, SessionId};
#[cfg(feature = "local_fs")]
use warp_util::standardized_path::StandardizedPath;
use warpui::{AppContext, Entity, ModelContext, SingletonEntity, View, ViewContext};

use crate::app_state::{
    EnvironmentKind, EnvironmentLifecycleState, EnvironmentSnapshot, TabSnapshot,
};
use crate::auth::AuthStateProvider;
use crate::cli_agent_jsonl::CliAgentStoreRoots;
use crate::environment_authority::ParsedEnvironmentAuthority;
#[cfg(feature = "local_tty")]
pub(crate) use crate::environment_runtime_transport::setup::RemoteServerSetupState as EnvironmentRuntimeSetupState;
use crate::pane_group::{EnvironmentRuntimePtyProcess, NewTerminalOptions, PanesLayout};
use crate::terminal::available_shells::AvailableShell;
use crate::terminal::capability_environment::terminal_capability_environment_variables;
use crate::terminal::view::load_ai_conversation::ConversationRestorationInNewPaneType;
use crate::terminal::CLIAgent;

pub(crate) use crate::environment_runtime_transport::auth::RemoteServerAuthContext as EnvironmentRuntimeAuthContext;
pub(crate) use crate::environment_runtime_transport::auth_context::server_api_auth_context as environment_runtime_auth_context;
pub(crate) use crate::environment_runtime_transport::client::ClientError as EnvironmentRuntimeClientError;
pub(crate) use crate::environment_runtime_transport::client::RemoteServerClient as EnvironmentRuntimeClient;
pub(crate) use crate::environment_runtime_transport::manager::RemoteServerErrorKind as EnvironmentRuntimeErrorKind;
pub(crate) use crate::environment_runtime_transport::manager::RemoteServerManager as EnvironmentRuntimeTransportManager;
#[cfg(feature = "local_fs")]
pub(crate) use crate::environment_runtime_transport::manager::RemoteServerManagerEvent as EnvironmentRuntimeTransportEvent;
pub(crate) use crate::environment_runtime_transport::manager::RemoteServerOperation as EnvironmentRuntimeOperation;
pub(crate) use crate::environment_runtime_transport::manager::RemoteSessionDisconnectCause as EnvironmentRuntimeDisconnectCause;
pub(crate) use crate::environment_runtime_transport::setup::PreinstallCheckResult as EnvironmentRuntimePreinstallCheckResult;
pub(crate) use crate::environment_runtime_transport::setup::PreinstallStatus as EnvironmentRuntimePreinstallStatus;
#[cfg(not(target_family = "wasm"))]
pub(crate) use crate::environment_runtime_transport::ssh_transport::SshTransport as EnvironmentRuntimeTransport;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EnvironmentRuntimeStatus {
    Dormant,
    Connecting,
    Installing,
    Connected,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EnvironmentRuntimeHeartbeatResult {
    Alive,
    TransportFailure(String),
    ExecutionFailure(String),
}

impl EnvironmentRuntimeStatus {
    pub(crate) fn lifecycle_state(&self) -> EnvironmentLifecycleState {
        match self {
            Self::Dormant => EnvironmentLifecycleState::Dormant,
            Self::Connecting => EnvironmentLifecycleState::Connecting,
            Self::Installing => EnvironmentLifecycleState::Installing,
            Self::Connected => EnvironmentLifecycleState::Connected,
            Self::Error => EnvironmentLifecycleState::Error,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct EnvironmentRuntimeRoots {
    pub(crate) workspace_root: String,
    pub(crate) home_root: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EnvironmentRuntimeExecutionCarrierGate {
    MissingRuntimeOwner,
    StaleRuntimeOwner,
    WaitingForExecutionCarrier,
    Ready,
}

pub(crate) fn environment_runtime_execution_carrier_gate(
    runtime_owner_session_id: Option<SessionId>,
    session_id: SessionId,
    has_execution_context: bool,
) -> EnvironmentRuntimeExecutionCarrierGate {
    let Some(runtime_owner_session_id) = runtime_owner_session_id else {
        return EnvironmentRuntimeExecutionCarrierGate::MissingRuntimeOwner;
    };
    if runtime_owner_session_id != session_id {
        return EnvironmentRuntimeExecutionCarrierGate::StaleRuntimeOwner;
    }
    if !has_execution_context {
        return EnvironmentRuntimeExecutionCarrierGate::WaitingForExecutionCarrier;
    }
    EnvironmentRuntimeExecutionCarrierGate::Ready
}

#[cfg(feature = "local_tty")]
#[derive(Clone, Debug)]
pub(crate) struct EnvironmentRuntimeBufferEdit {
    pub(crate) start_offset: u64,
    pub(crate) end_offset: u64,
    pub(crate) text: String,
}

#[cfg(feature = "local_tty")]
#[derive(Clone, Debug)]
pub(crate) struct EnvironmentRuntimeBufferUpdate {
    pub(crate) session_id: SessionId,
    pub(crate) host_id: HostId,
    pub(crate) path: String,
    pub(crate) new_server_version: u64,
    pub(crate) expected_client_version: u64,
    pub(crate) edits: Vec<EnvironmentRuntimeBufferEdit>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EnvironmentRuntimeFileKind {
    Unspecified,
    File,
    Directory,
    Symlink,
    Other,
    Missing,
}

fn file_kind_from_proto(kind: i32) -> EnvironmentRuntimeFileKind {
    match crate::environment_runtime_transport::proto::FileSystemEntryKind::try_from(kind) {
        Ok(crate::environment_runtime_transport::proto::FileSystemEntryKind::Unspecified) => {
            EnvironmentRuntimeFileKind::Unspecified
        }
        Ok(crate::environment_runtime_transport::proto::FileSystemEntryKind::File) => {
            EnvironmentRuntimeFileKind::File
        }
        Ok(crate::environment_runtime_transport::proto::FileSystemEntryKind::Directory) => {
            EnvironmentRuntimeFileKind::Directory
        }
        Ok(crate::environment_runtime_transport::proto::FileSystemEntryKind::Symlink) => {
            EnvironmentRuntimeFileKind::Symlink
        }
        Ok(crate::environment_runtime_transport::proto::FileSystemEntryKind::Missing) => {
            EnvironmentRuntimeFileKind::Missing
        }
        Ok(crate::environment_runtime_transport::proto::FileSystemEntryKind::Other) | Err(_) => {
            EnvironmentRuntimeFileKind::Other
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct EnvironmentRuntimeResolvedPath {
    /// 展开后的请求路径，但不跟随末级符号链接；浏览器操作和 UI 身份只使用它。
    pub(crate) path: String,
    /// 解析成功时的规范目标路径，仅供 realpath 等目标查询使用；不得替换 `path`。
    #[cfg_attr(any(test, feature = "integration_tests"), allow(dead_code))]
    pub(crate) resolved_path: Option<String>,
    pub(crate) kind: EnvironmentRuntimeFileKind,
    /// For symlink entries, the kind of the resolved target.
    /// `Unspecified` for non-symlink entries.
    pub(crate) target_kind: EnvironmentRuntimeFileKind,
    pub(crate) size_bytes: Option<u64>,
}

#[derive(Clone, Debug)]
pub(crate) struct EnvironmentRuntimeDirectoryEntry {
    pub(crate) name: String,
    pub(crate) is_dir: bool,
    pub(crate) kind: EnvironmentRuntimeFileKind,
    /// For symlink entries, the kind of the resolved target.
    /// `Unspecified` for non-symlink entries.
    pub(crate) target_kind: EnvironmentRuntimeFileKind,
    pub(crate) size_bytes: Option<u64>,
    pub(crate) modified_epoch_millis: Option<u64>,
    pub(crate) directory_identity:
        Option<crate::environment_runtime_transport::proto::DeleteDirectoryIdentity>,
    pub(crate) platform_hidden: bool,
    pub(crate) ignored: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct EnvironmentRuntimeDirectoryListing {
    /// 展开后的请求路径，但不跟随末级符号链接。
    pub(crate) path: String,
    pub(crate) entries: Vec<EnvironmentRuntimeDirectoryEntry>,
}

#[derive(Clone, Debug)]
pub(crate) struct EnvironmentRuntimeFileChunk {
    pub(crate) bytes: Vec<u8>,
    pub(crate) next_offset: u64,
    pub(crate) total_size: u64,
    pub(crate) eof: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct EnvironmentRuntimeWriteChunkSuccess {
    pub(crate) next_offset: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct EnvironmentRuntimeReadTransfer {
    pub(crate) handle: crate::environment_runtime_transport::proto::FileTransferHandle,
    total_size: u64,
    next_offset: u64,
}

impl EnvironmentRuntimeReadTransfer {
    pub(crate) fn total_size(&self) -> u64 {
        self.total_size
    }

    pub(crate) fn next_offset(&self) -> u64 {
        self.next_offset
    }

    pub(crate) fn accept_chunk(
        &mut self,
        chunk: &EnvironmentRuntimeFileChunk,
    ) -> Result<(), String> {
        if chunk.total_size != self.total_size {
            return Err(format!(
                "environment read transfer size changed: begin={}, chunk={}",
                self.total_size, chunk.total_size
            ));
        }
        let byte_count = u64::try_from(chunk.bytes.len())
            .map_err(|_| "environment read chunk length exceeds u64".to_owned())?;
        let expected_next_offset = self
            .next_offset
            .checked_add(byte_count)
            .ok_or_else(|| "environment read offset overflow".to_owned())?;
        if chunk.next_offset != expected_next_offset {
            return Err(format!(
                "environment read cursor mismatch: previous={}, bytes={}, expected={}, actual={}",
                self.next_offset, byte_count, expected_next_offset, chunk.next_offset
            ));
        }
        if chunk.next_offset > self.total_size {
            return Err(format!(
                "environment read exceeded captured size: offset={}, total={}",
                chunk.next_offset, self.total_size
            ));
        }
        let expected_eof = chunk.next_offset == self.total_size;
        if chunk.eof != expected_eof {
            return Err(format!(
                "environment read EOF mismatch: offset={}, total={}, eof={}",
                chunk.next_offset, self.total_size, chunk.eof
            ));
        }
        if byte_count == 0 && !chunk.eof {
            return Err(format!(
                "environment read made no progress at offset {}",
                self.next_offset
            ));
        }
        self.next_offset = chunk.next_offset;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct EnvironmentRuntimeWriteTransfer {
    pub(crate) handle: crate::environment_runtime_transport::proto::FileTransferHandle,
    next_offset: u64,
}

impl EnvironmentRuntimeWriteTransfer {
    pub(crate) fn next_offset(&self) -> u64 {
        self.next_offset
    }

    pub(crate) fn accept_chunk(
        &mut self,
        submitted_bytes: usize,
        success: &EnvironmentRuntimeWriteChunkSuccess,
    ) -> Result<(), String> {
        let submitted_bytes = u64::try_from(submitted_bytes)
            .map_err(|_| "environment write chunk length exceeds u64".to_owned())?;
        let expected_next_offset = self
            .next_offset
            .checked_add(submitted_bytes)
            .ok_or_else(|| "environment write offset overflow".to_owned())?;
        if success.next_offset != expected_next_offset {
            return Err(format!(
                "environment write cursor mismatch: previous={}, bytes={}, expected={}, actual={}",
                self.next_offset, submitted_bytes, expected_next_offset, success.next_offset
            ));
        }
        self.next_offset = success.next_offset;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct EnvironmentRuntimeCommandOutput {
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) exit_code: Option<i32>,
}

#[cfg(feature = "local_tty")]
#[derive(Clone, Debug)]
pub(crate) enum EnvironmentRuntimePtyCreateResult {
    Created { pty_id: u64, shell_type: String },
    Failed(String),
}

#[cfg(feature = "local_tty")]
pub(crate) enum EnvironmentRuntimeSessionEvent {
    Connected {
        session_id: SessionId,
        host_id: HostId,
    },
    Disconnected {
        session_id: SessionId,
    },
    SetupStateChanged {
        session_id: SessionId,
        state: EnvironmentRuntimeSetupState,
    },
    Reconnected {
        session_id: SessionId,
        client: Arc<EnvironmentRuntimeClient>,
    },
}

#[cfg(feature = "local_tty")]
pub(crate) enum EnvironmentRuntimePtyEvent {
    Output {
        session_id: SessionId,
        pty_id: u64,
        bytes: Vec<u8>,
    },
    Exited {
        session_id: SessionId,
        pty_id: u64,
    },
}

#[cfg(feature = "local_tty")]
pub(crate) enum EnvironmentRuntimeSetupEvent {
    BinaryCheckComplete {
        session_id: SessionId,
        result: Result<bool, String>,
        preinstall_check: Option<EnvironmentRuntimePreinstallCheckResult>,
        has_old_binary: bool,
    },
    BinaryInstallComplete {
        session_id: SessionId,
        result: Result<(), String>,
    },
    Connected {
        session_id: SessionId,
    },
    ConnectionFailed {
        session_id: SessionId,
    },
}

#[cfg(feature = "local_tty")]
pub(crate) enum EnvironmentRuntimeTerminalEvent {
    SetupStateChanged {
        session_id: SessionId,
    },
    SessionConnected {
        session_id: SessionId,
    },
    SessionConnectionFailed {
        session_id: SessionId,
        error: String,
    },
    SessionDisconnected {
        session_id: SessionId,
    },
    SessionDeregistered {
        session_id: SessionId,
    },
    BinaryInstallComplete {
        session_id: SessionId,
        result: Result<(), String>,
    },
    BinaryCheckComplete {
        session_id: SessionId,
        result: Result<bool, String>,
    },
    ClientRequestFailed {
        session_id: SessionId,
    },
    ServerMessageDecodingError {
        session_id: SessionId,
    },
    NavigatedToDirectory {
        session_id: SessionId,
        host_id: HostId,
        requested_path: String,
        indexed_path: String,
    },
}

#[cfg(feature = "local_tty")]
impl EnvironmentRuntimeTerminalEvent {
    pub(crate) fn session_id(&self) -> SessionId {
        match self {
            Self::SetupStateChanged { session_id }
            | Self::SessionConnected { session_id, .. }
            | Self::SessionConnectionFailed { session_id, .. }
            | Self::SessionDisconnected { session_id, .. }
            | Self::SessionDeregistered { session_id }
            | Self::BinaryInstallComplete { session_id, .. }
            | Self::BinaryCheckComplete { session_id, .. }
            | Self::ClientRequestFailed { session_id, .. }
            | Self::ServerMessageDecodingError { session_id, .. }
            | Self::NavigatedToDirectory { session_id, .. } => *session_id,
        }
    }
}

pub(crate) fn environment_runtime_feature_enabled() -> bool {
    FeatureFlag::EnvironmentRuntime.is_enabled()
}

pub(crate) fn install_debug_runtime_feature_flags(flags: &mut HashSet<FeatureFlag>) {
    // Environment Runtime:release bundle 走 RELEASE_FLAGS 启用,但 dev 源码构建
    // (`cargo run`)不是 release bundle,该 flag 会一直关闭 —— 于是环境 runtime
    // transport 不激活,dev 模式自动构建并上传 helper 二进制也就没有机会触发。
    // 这里在 debug 构建里显式开启,保证开发时能联调环境文件打开 / buffer-sync。
    // Windows 暂不支持该 runtime helper 二进制,与 RELEASE_FLAGS 的 cfg 保持一致排除掉。
    #[cfg(all(debug_assertions, not(windows)))]
    {
        flags.insert(FeatureFlag::EnvironmentRuntime);
        flags.insert(FeatureFlag::ServerFileBrowser);
    }
    #[cfg(not(all(debug_assertions, not(windows))))]
    {
        let _ = flags;
    }
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn run_worker_proxy(identity_key: String) -> anyhow::Result<()> {
    crate::environment_runtime_transport::run_proxy(identity_key)
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn run_worker_daemon(identity_key: String) -> anyhow::Result<()> {
    crate::environment_runtime_transport::run_daemon(identity_key)
}

pub(crate) fn new_transport_manager(
    ctx: &mut ModelContext<EnvironmentRuntimeTransportManager>,
) -> EnvironmentRuntimeTransportManager {
    EnvironmentRuntimeTransportManager::new(ctx)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EnvironmentRuntimeCapabilities {
    uses_terminal_bootstrap: bool,
    uses_runtime_entry: bool,
    uses_runtime_transport: bool,
    display_icon_kind: EnvironmentDisplayIconKind,
}

fn capabilities_for_kind(kind: &EnvironmentKind) -> EnvironmentRuntimeCapabilities {
    match kind {
        EnvironmentKind::Local => EnvironmentRuntimeCapabilities {
            uses_terminal_bootstrap: true,
            uses_runtime_entry: false,
            uses_runtime_transport: false,
            display_icon_kind: EnvironmentDisplayIconKind::Laptop,
        },
        EnvironmentKind::Ssh => EnvironmentRuntimeCapabilities {
            uses_terminal_bootstrap: false,
            uses_runtime_entry: true,
            uses_runtime_transport: true,
            display_icon_kind: EnvironmentDisplayIconKind::Server,
        },
        EnvironmentKind::Container | EnvironmentKind::Wsl | EnvironmentKind::Custom => {
            EnvironmentRuntimeCapabilities {
                uses_terminal_bootstrap: false,
                uses_runtime_entry: true,
                uses_runtime_transport: false,
                display_icon_kind: EnvironmentDisplayIconKind::Terminal,
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EnvironmentToolPanelCapability {
    RuntimeFileBrowser,
    RuntimeProjectExplorer,
    CurrentAppProjectExplorer,
    CurrentAppFileBrowser,
    CurrentAppGlobalSearch,
    CurrentAppSkillManager,
    RuntimeSkillManager,
}

fn capabilities_for_environment(
    environment: &EnvironmentSnapshot,
) -> EnvironmentRuntimeCapabilities {
    capabilities_for_kind(&environment.kind)
}

pub(crate) fn uses_terminal_bootstrap(environment: &EnvironmentSnapshot) -> bool {
    capabilities_for_environment(environment).uses_terminal_bootstrap
}

fn uses_environment_runtime(environment: &EnvironmentSnapshot) -> bool {
    capabilities_for_environment(environment).uses_runtime_entry
}

pub(crate) fn supports_runtime_entry(environment: &EnvironmentSnapshot) -> bool {
    uses_environment_runtime(environment)
}

pub(crate) fn should_preserve_current_environment_for_strip(
    environment: &EnvironmentSnapshot,
) -> bool {
    supports_runtime_entry(environment)
}

pub(crate) fn should_seed_strip_with_current_environment(
    environment: &EnvironmentSnapshot,
) -> bool {
    uses_terminal_bootstrap(environment)
}

pub(crate) fn should_sync_connected_left_panel_roots(environment: &EnvironmentSnapshot) -> bool {
    uses_environment_runtime(environment)
}

pub(crate) fn environment_strip_dedupe_key(environment: &EnvironmentSnapshot) -> String {
    ParsedEnvironmentAuthority::parse(&environment.authority_key)
        .navigation_key()
        .to_owned()
}

pub(crate) fn workspace_root_candidate_for_authority(
    authority: &str,
    root: String,
) -> Option<String> {
    let trimmed = root.trim();
    if trimmed.is_empty() {
        return None;
    }
    if ParsedEnvironmentAuthority::parse(authority).uses_runtime_environment()
        && path_looks_like_current_app_local_path(trimmed)
    {
        return None;
    }

    Some(root)
}

pub(crate) fn path_looks_like_current_app_local_path(path: &str) -> bool {
    let normalized_path = normalize_local_path_for_compare(path);
    if normalized_path.is_empty() {
        return false;
    }

    local_path_leak_candidates().iter().any(|candidate| {
        let candidate = normalize_local_path_for_compare(candidate);
        if candidate.is_empty() {
            return false;
        }
        normalized_path == candidate
            || normalized_path
                .strip_prefix(candidate.as_str())
                .is_some_and(|suffix| suffix.starts_with('/'))
    })
}

fn normalize_local_path_for_compare(path: &str) -> String {
    let mut normalized = path.trim().replace('\\', "/");
    while normalized.len() > 1 && normalized.ends_with('/') {
        normalized.pop();
    }
    normalized
}

fn local_path_leak_candidates() -> Vec<String> {
    let mut candidates = Vec::new();
    if let Ok(current_dir) = std::env::current_dir() {
        candidates.push(current_dir.to_string_lossy().into_owned());
    }
    for key in ["HOME", "USERPROFILE"] {
        if let Some(path) = std::env::var_os(key) {
            candidates.push(path.to_string_lossy().into_owned());
        }
    }
    candidates
}

pub(crate) fn terminal_bootstrap_environment(
    active_workspace_root: Option<String>,
) -> EnvironmentSnapshot {
    EnvironmentSnapshot::terminal_bootstrap(active_workspace_root)
}

pub(crate) fn terminal_bootstrap_environment_with_authority(
    active_workspace_root: Option<String>,
    authority: String,
) -> EnvironmentSnapshot {
    let mut environment = terminal_bootstrap_environment(active_workspace_root);
    environment.authority_key = authority;
    environment
}

pub(crate) fn terminal_bootstrap_environment_from_tabs(
    tabs: &[TabSnapshot],
    active_tab_index: usize,
) -> EnvironmentSnapshot {
    EnvironmentSnapshot::terminal_bootstrap_from_tabs(tabs, active_tab_index)
}

pub(crate) fn terminal_bootstrap_environment_for_authority(
    authority_key: &str,
) -> Option<EnvironmentSnapshot> {
    match ParsedEnvironmentAuthority::parse(authority_key) {
        ParsedEnvironmentAuthority::TerminalBootstrap { root, .. } => {
            Some(terminal_bootstrap_environment(root.map(str::to_owned)))
        }
        ParsedEnvironmentAuthority::SavedSsh { .. }
        | ParsedEnvironmentAuthority::Runtime { .. } => None,
    }
}

pub(crate) fn supports_tool_panel_capability(
    environment: &EnvironmentSnapshot,
    capability: EnvironmentToolPanelCapability,
) -> bool {
    match capability {
        EnvironmentToolPanelCapability::RuntimeFileBrowser
        | EnvironmentToolPanelCapability::RuntimeProjectExplorer
        | EnvironmentToolPanelCapability::RuntimeSkillManager => {
            uses_environment_runtime(environment)
        }
        EnvironmentToolPanelCapability::CurrentAppProjectExplorer
        | EnvironmentToolPanelCapability::CurrentAppFileBrowser
        | EnvironmentToolPanelCapability::CurrentAppGlobalSearch
        | EnvironmentToolPanelCapability::CurrentAppSkillManager => {
            uses_terminal_bootstrap(environment)
        }
    }
}

pub(crate) fn should_show_runtime_file_browsers(environment: &EnvironmentSnapshot) -> bool {
    supports_tool_panel_capability(
        environment,
        EnvironmentToolPanelCapability::RuntimeFileBrowser,
    )
}

pub(crate) fn should_show_runtime_project_explorer(environment: &EnvironmentSnapshot) -> bool {
    supports_tool_panel_capability(
        environment,
        EnvironmentToolPanelCapability::RuntimeProjectExplorer,
    )
}

pub(crate) fn should_show_terminal_project_explorer(environment: &EnvironmentSnapshot) -> bool {
    supports_tool_panel_capability(
        environment,
        EnvironmentToolPanelCapability::CurrentAppProjectExplorer,
    )
}

pub(crate) fn should_show_terminal_file_browser(environment: &EnvironmentSnapshot) -> bool {
    supports_tool_panel_capability(
        environment,
        EnvironmentToolPanelCapability::CurrentAppFileBrowser,
    )
}

pub(crate) fn should_seed_terminal_file_browser_home(environment: &EnvironmentSnapshot) -> bool {
    supports_tool_panel_capability(
        environment,
        EnvironmentToolPanelCapability::CurrentAppFileBrowser,
    )
}

pub(crate) fn should_show_current_app_global_search(environment: &EnvironmentSnapshot) -> bool {
    supports_tool_panel_capability(
        environment,
        EnvironmentToolPanelCapability::CurrentAppGlobalSearch,
    )
}

pub(crate) fn should_show_current_app_skill_manager(environment: &EnvironmentSnapshot) -> bool {
    supports_tool_panel_capability(
        environment,
        EnvironmentToolPanelCapability::CurrentAppSkillManager,
    )
}

pub(crate) fn should_show_skill_manager_panel(environment: &EnvironmentSnapshot) -> bool {
    supports_tool_panel_capability(
        environment,
        EnvironmentToolPanelCapability::CurrentAppSkillManager,
    ) || supports_tool_panel_capability(
        environment,
        EnvironmentToolPanelCapability::RuntimeSkillManager,
    )
}

pub(crate) fn should_ensure_runtime_transport(environment: &EnvironmentSnapshot) -> bool {
    capabilities_for_environment(environment).uses_runtime_transport
}

pub(crate) fn runtime_transport_error_snapshot(
    label: String,
    authority_key: String,
    connection_ref: Option<String>,
    active_workspace_root: Option<String>,
) -> EnvironmentSnapshot {
    EnvironmentSnapshot {
        label,
        kind: EnvironmentKind::Ssh,
        authority_key,
        connection_ref,
        active_workspace_root,
        lifecycle_state: EnvironmentLifecycleState::Error,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EnvironmentDisplayIconKind {
    Laptop,
    Server,
    Terminal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EnvironmentDisplayInfo {
    pub(crate) kind_label: &'static str,
    pub(crate) tooltip_label: String,
    pub(crate) chip_label: Option<String>,
    pub(crate) icon_kind: EnvironmentDisplayIconKind,
    pub(crate) supports_disconnect: bool,
    pub(crate) supports_reconnect: bool,
}

pub(crate) fn environment_kind_label(kind: &EnvironmentKind) -> &'static str {
    match kind {
        EnvironmentKind::Local => crate::t_static!("workspace-environment-kind-local"),
        EnvironmentKind::Ssh => crate::t_static!("workspace-environment-kind-ssh"),
        EnvironmentKind::Container => crate::t_static!("workspace-environment-kind-container"),
        EnvironmentKind::Wsl => crate::t_static!("workspace-environment-kind-wsl"),
        EnvironmentKind::Custom => crate::t_static!("workspace-environment-kind-custom"),
    }
}

pub(crate) fn environment_tooltip_label(environment: &EnvironmentSnapshot) -> String {
    let capabilities = capabilities_for_environment(environment);
    if capabilities.uses_terminal_bootstrap {
        return crate::t!("workspace-environment-tooltip-local");
    }

    if capabilities.uses_runtime_transport {
        return match environment.lifecycle_state {
            EnvironmentLifecycleState::Connected => {
                crate::t!("workspace-environment-tooltip-runtime-connected")
            }
            EnvironmentLifecycleState::Dormant => {
                crate::t!("workspace-environment-tooltip-runtime-dormant")
            }
            EnvironmentLifecycleState::Connecting => {
                crate::t!("workspace-environment-tooltip-runtime-connecting")
            }
            EnvironmentLifecycleState::Installing => {
                crate::t!("workspace-environment-tooltip-runtime-installing")
            }
            EnvironmentLifecycleState::Error => {
                crate::t!("workspace-environment-tooltip-runtime-error")
            }
        };
    }

    crate::t!("workspace-environment-tooltip-generic")
}

pub(crate) fn environment_display_info_for_environment(
    environment: &EnvironmentSnapshot,
) -> EnvironmentDisplayInfo {
    let kind_label = environment_kind_label(&environment.kind);
    let capabilities = capabilities_for_environment(environment);
    let chip_label = if capabilities.uses_terminal_bootstrap {
        Some(kind_label.to_string())
    } else if capabilities.uses_runtime_transport {
        if environment.label.is_empty() {
            Some(kind_label.to_string())
        } else {
            Some(environment.label.clone())
        }
    } else if environment.label == kind_label {
        Some(kind_label.to_string())
    } else {
        Some(format!("{kind_label} · {}", environment.label))
    };
    let icon_kind = capabilities.display_icon_kind;
    let supports_disconnect = capabilities.uses_runtime_transport;
    let supports_reconnect =
        supports_disconnect && environment.lifecycle_state != EnvironmentLifecycleState::Connected;

    EnvironmentDisplayInfo {
        kind_label,
        tooltip_label: environment_tooltip_label(environment),
        chip_label,
        icon_kind,
        supports_disconnect,
        supports_reconnect,
    }
}

pub(crate) async fn resolve_environment_runtime_roots(
    client: Arc<EnvironmentRuntimeClient>,
    session_id: SessionId,
    requested_root: Option<String>,
) -> Result<EnvironmentRuntimeRoots, String> {
    let workspace_command = if let Some(root) = requested_root
        .as_deref()
        .filter(|root| !root.trim().is_empty())
    {
        format!("cd {} && pwd -P || exit $?", shell_words::quote(root))
    } else {
        r#"pwd -P 2>/dev/null || printf '%s\n' "$HOME""#.to_owned()
    };
    let command = format!(
        r#"{workspace_command}
printf '%s\n' "$HOME""#
    );
    let response = client
        .run_command(session_id, command, None, HashMap::new())
        .await
        .map_err(|error| error.to_string())?;
    match response.result {
        Some(
            crate::environment_runtime_transport::proto::run_command_response::Result::Success(
                success,
            ),
        ) => {
            if success.exit_code != Some(0) {
                let stderr = String::from_utf8_lossy(&success.stderr).trim().to_owned();
                return Err(if stderr.is_empty() {
                    format!("environment root probe exited with {:?}", success.exit_code)
                } else {
                    stderr
                });
            }
            environment_runtime_roots_from_probe_stdout(success.stdout)
        }
        Some(crate::environment_runtime_transport::proto::run_command_response::Result::Error(
            error,
        )) => Err(format!("environment root probe failed: {error:?}")),
        None => Err("environment root probe returned no result".to_owned()),
    }
}

#[derive(Clone, Debug)]
pub(crate) struct EnvironmentRuntimeReadFile {
    pub(crate) path: String,
    pub(crate) line_ranges: Vec<Range<u32>>,
}

#[derive(Clone, Debug)]
pub(crate) struct EnvironmentRuntimeReadFileContextRequest {
    pub(crate) files: Vec<EnvironmentRuntimeReadFile>,
    pub(crate) max_file_bytes: Option<u32>,
    pub(crate) max_batch_bytes: Option<u32>,
}

#[derive(Clone, Debug)]
pub(crate) enum EnvironmentRuntimeFileContent {
    Text(String),
    Binary(Vec<u8>),
}

#[derive(Clone, Debug)]
pub(crate) struct EnvironmentRuntimeFileContext {
    pub(crate) file_name: String,
    pub(crate) content: Option<EnvironmentRuntimeFileContent>,
    pub(crate) line_range: Option<Range<usize>>,
    pub(crate) last_modified: Option<SystemTime>,
    pub(crate) line_count: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct EnvironmentRuntimeFailedFileRead {
    pub(crate) path: String,
    pub(crate) message: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct EnvironmentRuntimeReadFileContextResponse {
    pub(crate) file_contexts: Vec<EnvironmentRuntimeFileContext>,
    pub(crate) failed_files: Vec<EnvironmentRuntimeFailedFileRead>,
}

#[cfg(feature = "local_tty")]
#[derive(Clone, Debug)]
pub(crate) struct EnvironmentRuntimeOpenBufferResponse {
    pub(crate) content: String,
    pub(crate) server_version: u64,
}

#[cfg(feature = "local_tty")]
#[derive(Clone, Debug)]
pub(crate) enum EnvironmentRuntimeSaveBufferResponse {
    Saved,
    Failed(String),
}

pub(crate) async fn read_file_context(
    client: &EnvironmentRuntimeClient,
    request: EnvironmentRuntimeReadFileContextRequest,
) -> Result<EnvironmentRuntimeReadFileContextResponse, EnvironmentRuntimeClientError> {
    let request = crate::environment_runtime_transport::proto::ReadFileContextRequest {
        files: request
            .files
            .into_iter()
            .map(
                |file| crate::environment_runtime_transport::proto::ReadFileContextFile {
                    path: file.path,
                    line_ranges: file
                        .line_ranges
                        .into_iter()
                        .map(
                            |range| crate::environment_runtime_transport::proto::LineRange {
                                start: range.start,
                                end: range.end,
                            },
                        )
                        .collect(),
                },
            )
            .collect(),
        max_file_bytes: request.max_file_bytes,
        max_batch_bytes: request.max_batch_bytes,
    };
    let response = client.read_file_context(request).await?;
    Ok(EnvironmentRuntimeReadFileContextResponse {
        file_contexts: response
            .file_contexts
            .into_iter()
            .map(|file| {
                let content = file.content.map(|content| match content {
                    crate::environment_runtime_transport::proto::file_context_proto::Content::TextContent(text) => {
                        EnvironmentRuntimeFileContent::Text(text)
                    }
                    crate::environment_runtime_transport::proto::file_context_proto::Content::BinaryContent(
                        bytes,
                    ) => EnvironmentRuntimeFileContent::Binary(bytes),
                });
                EnvironmentRuntimeFileContext {
                    file_name: file.file_name,
                    content,
                    line_range: match (file.line_range_start, file.line_range_end) {
                        (Some(start), Some(end)) => Some(start as usize..end as usize),
                        _ => None,
                    },
                    last_modified: file
                        .last_modified_epoch_millis
                        .map(|ms| SystemTime::UNIX_EPOCH + Duration::from_millis(ms)),
                    line_count: file.line_count as usize,
                }
            })
            .collect(),
        failed_files: response
            .failed_files
            .into_iter()
            .map(|file| EnvironmentRuntimeFailedFileRead {
                path: file.path,
                message: file.error.map(|error| error.message),
            })
            .collect(),
    })
}

pub(crate) async fn resolve_path(
    client: &EnvironmentRuntimeClient,
    path: String,
) -> Result<EnvironmentRuntimeResolvedPath, String> {
    let requested_path = path.clone();
    let response = client
        .resolve_path(path)
        .await
        .map_err(|error| error.to_string())?;
    match response.result {
        Some(
            crate::environment_runtime_transport::proto::resolve_path_response::Result::Success(
                success,
            ),
        ) => Ok(EnvironmentRuntimeResolvedPath {
            path: success.path,
            resolved_path: success.resolved_path,
            kind: file_kind_from_proto(success.kind),
            target_kind: file_kind_from_proto(success.target_kind),
            size_bytes: success.size_bytes,
        }),
        Some(
            crate::environment_runtime_transport::proto::resolve_path_response::Result::Error(
                error,
            ),
        ) => Err(error.message),
        Some(
            crate::environment_runtime_transport::proto::resolve_path_response::Result::NotFound(_),
        ) => Err(format!("Path not found: {requested_path}")),
        None => Err(crate::t!("server-file-browser-empty-response")),
    }
}

pub(crate) async fn try_resolve_path(
    client: &EnvironmentRuntimeClient,
    path: String,
) -> Result<Option<EnvironmentRuntimeResolvedPath>, String> {
    let response = client
        .resolve_path(path)
        .await
        .map_err(|error| error.to_string())?;
    match response.result {
        Some(
            crate::environment_runtime_transport::proto::resolve_path_response::Result::Success(
                success,
            ),
        ) => Ok(Some(EnvironmentRuntimeResolvedPath {
            path: success.path,
            resolved_path: success.resolved_path,
            kind: file_kind_from_proto(success.kind),
            target_kind: file_kind_from_proto(success.target_kind),
            size_bytes: success.size_bytes,
        })),
        Some(
            crate::environment_runtime_transport::proto::resolve_path_response::Result::NotFound(_),
        ) => Ok(None),
        Some(
            crate::environment_runtime_transport::proto::resolve_path_response::Result::Error(
                error,
            ),
        ) => Err(error.message),
        None => Err(crate::t!("server-file-browser-empty-response")),
    }
}

pub(crate) async fn list_directory(
    client: &EnvironmentRuntimeClient,
    path: String,
) -> Result<EnvironmentRuntimeDirectoryListing, String> {
    let response = client
        .list_directory(path)
        .await
        .map_err(|error| error.to_string())?;
    match response.result {
        Some(
            crate::environment_runtime_transport::proto::list_directory_response::Result::Success(
                success,
            ),
        ) => {
            let entries = success
                .entries
                .into_iter()
                .map(|entry| EnvironmentRuntimeDirectoryEntry {
                    name: entry.name,
                    is_dir: entry.is_dir,
                    kind: file_kind_from_proto(entry.kind),
                    target_kind: file_kind_from_proto(entry.target_kind),
                    size_bytes: entry.size_bytes,
                    modified_epoch_millis: entry.modified_epoch_millis,
                    directory_identity: entry.directory_identity,
                    platform_hidden: entry.platform_hidden,
                    ignored: entry.ignored,
                })
                .collect();
            Ok(EnvironmentRuntimeDirectoryListing {
                path: success.path,
                entries,
            })
        }
        Some(
            crate::environment_runtime_transport::proto::list_directory_response::Result::Error(
                error,
            ),
        ) => Err(error.message),
        None => Err(crate::t!("server-file-browser-empty-response")),
    }
}

pub(crate) async fn create_directory(
    client: &EnvironmentRuntimeClient,
    path: String,
) -> Result<(), String> {
    let response = client
        .create_directory(path)
        .await
        .map_err(|error| error.to_string())?;
    match response.result {
        Some(
            crate::environment_runtime_transport::proto::create_directory_response::Result::Success(
                _,
            ),
        ) => Ok(()),
        Some(
            crate::environment_runtime_transport::proto::create_directory_response::Result::Error(
                error,
            ),
        ) => Err(error.message),
        None => Err(crate::t!("server-file-browser-empty-response")),
    }
}

pub(crate) async fn delete_directory(
    client: &EnvironmentRuntimeClient,
    path: String,
    identity: crate::environment_runtime_transport::proto::DeleteDirectoryIdentity,
) -> Result<(), String> {
    client
        .delete_directory(path, identity)
        .await
        .map_err(|error| error.to_string())
}

pub(crate) async fn exact_rename(
    client: &EnvironmentRuntimeClient,
    from: String,
    to: String,
) -> Result<String, String> {
    client
        .exact_rename(from, to)
        .await
        .map_err(|error| error.to_string())
}

async fn begin_file_transfer_success(
    client: &EnvironmentRuntimeClient,
    path: String,
    direction: crate::environment_runtime_transport::proto::FileTransferDirection,
    executable: Option<bool>,
) -> Result<crate::environment_runtime_transport::proto::BeginFileTransferSuccess, String> {
    let response = client
        .begin_file_transfer(path, direction, executable)
        .await
        .map_err(|error| error.to_string())?;
    match response.result {
        Some(
            crate::environment_runtime_transport::proto::begin_file_transfer_response::Result::Success(
                success,
            ),
        ) => Ok(success),
        Some(
            crate::environment_runtime_transport::proto::begin_file_transfer_response::Result::Error(
                error,
            ),
        ) => Err(error.message),
        None => Err(crate::t!("server-file-browser-empty-response")),
    }
}

pub(crate) async fn begin_read_file_transfer(
    client: &EnvironmentRuntimeClient,
    path: String,
) -> Result<EnvironmentRuntimeReadTransfer, String> {
    let success = begin_file_transfer_success(
        client,
        path,
        crate::environment_runtime_transport::proto::FileTransferDirection::Read,
        None,
    )
    .await?;
    Ok(EnvironmentRuntimeReadTransfer {
        handle: success
            .handle
            .ok_or_else(|| crate::t!("server-file-browser-empty-response"))?,
        total_size: success
            .total_size
            .ok_or_else(|| "read transfer response is missing total_size".to_owned())?,
        next_offset: 0,
    })
}

pub(crate) async fn begin_write_file_transfer(
    client: &EnvironmentRuntimeClient,
    path: String,
    executable: Option<bool>,
) -> Result<EnvironmentRuntimeWriteTransfer, String> {
    let success = begin_file_transfer_success(
        client,
        path,
        crate::environment_runtime_transport::proto::FileTransferDirection::Write,
        executable,
    )
    .await?;
    if success.total_size.is_some() {
        return Err("write transfer response unexpectedly included total_size".to_owned());
    }
    Ok(EnvironmentRuntimeWriteTransfer {
        handle: success
            .handle
            .ok_or_else(|| crate::t!("server-file-browser-empty-response"))?,
        next_offset: 0,
    })
}

pub(crate) async fn read_file_chunk(
    client: &EnvironmentRuntimeClient,
    handle: crate::environment_runtime_transport::proto::FileTransferHandle,
    max_bytes: u64,
) -> Result<EnvironmentRuntimeFileChunk, String> {
    let response = client
        .read_file_chunk(handle, max_bytes)
        .await
        .map_err(|error| error.to_string())?;
    match response.result {
        Some(
            crate::environment_runtime_transport::proto::read_file_chunk_response::Result::Success(
                success,
            ),
        ) => Ok(EnvironmentRuntimeFileChunk {
            bytes: success.bytes,
            next_offset: success.next_offset,
            total_size: success
                .total_size
                .ok_or_else(|| "read chunk response is missing total_size".to_owned())?,
            eof: success.eof,
        }),
        Some(
            crate::environment_runtime_transport::proto::read_file_chunk_response::Result::Error(
                error,
            ),
        ) => Err(error.message),
        None => Err(crate::t!("server-file-browser-empty-response")),
    }
}

pub(crate) async fn write_file_chunk(
    client: &EnvironmentRuntimeClient,
    handle: crate::environment_runtime_transport::proto::FileTransferHandle,
    bytes: Vec<u8>,
) -> Result<EnvironmentRuntimeWriteChunkSuccess, String> {
    let response = client
        .write_file_chunk(handle, bytes)
        .await
        .map_err(|error| error.to_string())?;
    match response.result {
        Some(
            crate::environment_runtime_transport::proto::write_file_chunk_response::Result::Success(
                success,
            ),
        ) => Ok(EnvironmentRuntimeWriteChunkSuccess {
            next_offset: success.next_offset,
        }),
        Some(
            crate::environment_runtime_transport::proto::write_file_chunk_response::Result::Error(
                error,
            ),
        ) => Err(error.message),
        None => Err(crate::t!("server-file-browser-empty-response")),
    }
}

pub(crate) async fn finish_file_transfer(
    client: &EnvironmentRuntimeClient,
    handle: crate::environment_runtime_transport::proto::FileTransferHandle,
) -> Result<Option<String>, String> {
    let response = client
        .finish_file_transfer(handle)
        .await
        .map_err(|error| error.to_string())?;
    match response.result {
        Some(
            crate::environment_runtime_transport::proto::finish_file_transfer_response::Result::Success(
                success,
            ),
        ) => Ok(success.committed_path),
        Some(
            crate::environment_runtime_transport::proto::finish_file_transfer_response::Result::Error(
                error,
            ),
        ) => Err(error.message),
        None => Err(crate::t!("server-file-browser-empty-response")),
    }
}

pub(crate) async fn abort_file_transfer(
    client: &EnvironmentRuntimeClient,
    handle: crate::environment_runtime_transport::proto::FileTransferHandle,
) -> Result<(), String> {
    let response = client
        .abort_file_transfer(handle)
        .await
        .map_err(|error| error.to_string())?;
    match response.result {
        Some(
            crate::environment_runtime_transport::proto::abort_file_transfer_response::Result::Success(_),
        ) => Ok(()),
        Some(
            crate::environment_runtime_transport::proto::abort_file_transfer_response::Result::Error(
                error,
            ),
        ) => Err(error.message),
        None => Err(crate::t!("server-file-browser-empty-response")),
    }
}

pub(crate) async fn run_command_success(
    client: &EnvironmentRuntimeClient,
    session_id: SessionId,
    command: String,
) -> Result<Vec<u8>, String> {
    let response = client
        .run_command(session_id, command, None, HashMap::new())
        .await
        .map_err(|error| format!("{error:#}"))?;
    match response.result {
        Some(
            crate::environment_runtime_transport::proto::run_command_response::Result::Success(
                success,
            ),
        ) if success.exit_code.unwrap_or(1) == 0 => Ok(success.stdout),
        Some(
            crate::environment_runtime_transport::proto::run_command_response::Result::Success(
                success,
            ),
        ) => {
            let stderr = String::from_utf8_lossy(&success.stderr);
            Err(stderr.trim().to_string())
        }
        Some(crate::environment_runtime_transport::proto::run_command_response::Result::Error(
            error,
        )) => Err(error.message),
        None => Err(crate::t!(
            "server-file-browser-operation-failed",
            error = "empty response"
        )),
    }
}

pub(crate) async fn check_environment_runtime_heartbeat(
    client: &EnvironmentRuntimeClient,
    session_id: SessionId,
    command: String,
) -> EnvironmentRuntimeHeartbeatResult {
    let response = match client
        .run_command(session_id, command, None, HashMap::new())
        .await
    {
        Ok(response) => response,
        Err(error) => {
            let message = format!("{error:#}");
            return match EnvironmentRuntimeErrorKind::from_client_error(&error) {
                EnvironmentRuntimeErrorKind::Timeout
                | EnvironmentRuntimeErrorKind::Disconnected => {
                    EnvironmentRuntimeHeartbeatResult::TransportFailure(message)
                }
                EnvironmentRuntimeErrorKind::ServerError | EnvironmentRuntimeErrorKind::Other => {
                    EnvironmentRuntimeHeartbeatResult::ExecutionFailure(message)
                }
            };
        }
    };

    match response.result {
        Some(
            crate::environment_runtime_transport::proto::run_command_response::Result::Success(
                success,
            ),
        ) if success.exit_code.unwrap_or(1) == 0 => EnvironmentRuntimeHeartbeatResult::Alive,
        Some(
            crate::environment_runtime_transport::proto::run_command_response::Result::Success(
                success,
            ),
        ) => EnvironmentRuntimeHeartbeatResult::ExecutionFailure(
            String::from_utf8_lossy(&success.stderr).trim().to_owned(),
        ),
        Some(crate::environment_runtime_transport::proto::run_command_response::Result::Error(
            error,
        )) => EnvironmentRuntimeHeartbeatResult::ExecutionFailure(error.message),
        None => EnvironmentRuntimeHeartbeatResult::ExecutionFailure(crate::t!(
            "server-file-browser-operation-failed",
            error = "empty response"
        )),
    }
}

pub(crate) async fn run_command_output(
    client: &EnvironmentRuntimeClient,
    session_id: SessionId,
    command: String,
    working_directory: Option<String>,
    environment_variables: HashMap<String, String>,
) -> Result<EnvironmentRuntimeCommandOutput, String> {
    let response = client
        .run_command(
            session_id,
            command,
            working_directory,
            environment_variables,
        )
        .await
        .map_err(|error| format!("{error:#}"))?;
    match response.result {
        Some(
            crate::environment_runtime_transport::proto::run_command_response::Result::Success(
                success,
            ),
        ) => Ok(EnvironmentRuntimeCommandOutput {
            stdout: success.stdout,
            stderr: success.stderr,
            exit_code: success.exit_code,
        }),
        Some(crate::environment_runtime_transport::proto::run_command_response::Result::Error(
            error,
        )) => Err(format!(
            "environment command error (code={:?}): {}",
            error.code(),
            error.message
        )),
        None => Err("environment command returned empty response".to_owned()),
    }
}

#[cfg(feature = "local_tty")]
pub(crate) async fn create_pty(
    client: &EnvironmentRuntimeClient,
    working_directory: String,
    shell: String,
    rows: u32,
    columns: u32,
    environment_variables: HashMap<String, String>,
) -> Result<EnvironmentRuntimePtyCreateResult, EnvironmentRuntimeClientError> {
    let response = client
        .create_pty(
            working_directory,
            shell,
            rows,
            columns,
            environment_variables,
        )
        .await?;
    let result = match response.result {
        Some(
            crate::environment_runtime_transport::proto::create_pty_response::Result::Success(
                success,
            ),
        ) => EnvironmentRuntimePtyCreateResult::Created {
            pty_id: success.pty_id,
            shell_type: success.shell_type,
        },
        Some(crate::environment_runtime_transport::proto::create_pty_response::Result::Error(
            error,
        )) => EnvironmentRuntimePtyCreateResult::Failed(error.message),
        None => return Err(EnvironmentRuntimeClientError::UnexpectedResponse),
    };
    Ok(result)
}

#[cfg(feature = "local_tty")]
pub(crate) async fn open_buffer(
    client: &EnvironmentRuntimeClient,
    path: String,
) -> Result<EnvironmentRuntimeOpenBufferResponse, EnvironmentRuntimeClientError> {
    let response = client.open_buffer(path).await?;
    Ok(EnvironmentRuntimeOpenBufferResponse {
        content: response.content,
        server_version: response.server_version,
    })
}

#[cfg(feature = "local_tty")]
pub(crate) fn send_buffer_edit(
    client: &EnvironmentRuntimeClient,
    path: String,
    expected_server_version: u64,
    new_client_version: u64,
    edits: Vec<EnvironmentRuntimeBufferEdit>,
) -> Result<(), EnvironmentRuntimeClientError> {
    client.send_buffer_edit(
        path,
        expected_server_version,
        new_client_version,
        edits
            .into_iter()
            .map(
                |edit| crate::environment_runtime_transport::proto::TextEdit {
                    start_offset: edit.start_offset,
                    end_offset: edit.end_offset,
                    text: edit.text,
                },
            )
            .collect(),
    )
}

#[cfg(feature = "local_tty")]
pub(crate) async fn save_buffer(
    client: &EnvironmentRuntimeClient,
    path: String,
) -> Result<EnvironmentRuntimeSaveBufferResponse, EnvironmentRuntimeClientError> {
    let response = client.save_buffer(path).await?;
    let result = match response.result {
        Some(
            crate::environment_runtime_transport::proto::save_buffer_response::Result::Success(_),
        ) => EnvironmentRuntimeSaveBufferResponse::Saved,
        Some(crate::environment_runtime_transport::proto::save_buffer_response::Result::Error(
            error,
        )) => EnvironmentRuntimeSaveBufferResponse::Failed(error.message),
        None => return Err(EnvironmentRuntimeClientError::UnexpectedResponse),
    };
    Ok(result)
}

pub(crate) fn environment_runtime_roots_from_probe_stdout(
    stdout: Vec<u8>,
) -> Result<EnvironmentRuntimeRoots, String> {
    let stdout = String::from_utf8(stdout).map_err(|error| error.to_string())?;
    let mut lines = stdout
        .lines()
        .map(str::trim)
        .filter(|path| !path.is_empty());
    let workspace_root = lines
        .next()
        .map(str::to_owned)
        .ok_or_else(|| "environment root probe returned an empty path".to_owned())?;
    let home_root = lines
        .next()
        .map(str::to_owned)
        .ok_or_else(|| "environment root probe omitted target HOME".to_owned())?;
    Ok(EnvironmentRuntimeRoots {
        workspace_root,
        home_root,
    })
}

fn environment_cli_agent_store_roots_from_probe_stdout(
    stdout: Vec<u8>,
) -> Result<CliAgentStoreRoots, String> {
    let mut fields = stdout.split(|byte| *byte == 0);
    let mut next_path = |label: &str| -> Result<String, String> {
        let bytes = fields
            .next()
            .ok_or_else(|| format!("environment CLI-agent root probe omitted {label}"))?;
        let path = std::str::from_utf8(bytes)
            .map_err(|error| format!("environment CLI-agent root {label} is not UTF-8: {error}"))?;
        if path.is_empty() {
            return Err(format!("environment CLI-agent root {label} is empty"));
        }
        if !path.starts_with('/') {
            return Err(format!(
                "environment CLI-agent root {label} is not absolute: {path}"
            ));
        }
        Ok(path.to_owned())
    };
    let home_dir = next_path("home_dir")?;
    let claude_config_dir = next_path("claude_config_dir")?;
    let codex_home = next_path("codex_home")?;
    if fields.any(|field| !field.is_empty()) {
        return Err("environment CLI-agent root probe returned unexpected extra fields".to_owned());
    }
    CliAgentStoreRoots::from_explicit_target_paths(
        PathBuf::from(home_dir),
        PathBuf::from(claude_config_dir),
        PathBuf::from(codex_home),
    )
}

fn environment_cli_agent_store_roots_to_proto(
    roots: &CliAgentStoreRoots,
) -> crate::environment_runtime_transport::proto::CliAgentSessionStoreRoots {
    crate::environment_runtime_transport::proto::CliAgentSessionStoreRoots {
        home_dir: roots.home_dir.to_string_lossy().into_owned(),
        claude_config_dir: roots.claude_config_dir.to_string_lossy().into_owned(),
        codex_home: roots.codex_home.to_string_lossy().into_owned(),
    }
}

fn environment_cli_agent_store_roots_probe_command() -> &'static str {
    "test \"${ASHIDE_SESSION_EXECUTION_CONTEXT:-}\" = 1 || { echo 'target session execution context is unavailable' >&2; exit 1; }; \
     home=${HOME:?target session HOME is unavailable}; pwd_root=$(pwd -P) || exit $?; \
     case \"$home\" in /*) ;; *) home=\"$pwd_root/$home\";; esac; \
     claude=${CLAUDE_CONFIG_DIR:-$home/.claude}; \
     case \"$claude\" in /*) ;; *) claude=\"$pwd_root/$claude\";; esac; \
     codex=${CODEX_HOME:-$home/.codex}; \
     case \"$codex\" in /*) ;; *) codex=\"$pwd_root/$codex\";; esac; \
     printf '%s\\0%s\\0%s\\0' \"$home\" \"$claude\" \"$codex\""
}

#[cfg(not(target_family = "wasm"))]
pub(crate) async fn resolve_environment_cli_agent_store_roots(
    client: &EnvironmentRuntimeClient,
    session_id: SessionId,
) -> Result<CliAgentStoreRoots, String> {
    let stdout = run_command_success(
        client,
        session_id,
        environment_cli_agent_store_roots_probe_command().to_owned(),
    )
    .await?;
    environment_cli_agent_store_roots_from_probe_stdout(stdout)
}

#[cfg(not(target_family = "wasm"))]
pub(crate) async fn run_environment_cli_agent_session_source_action(
    client: Arc<EnvironmentRuntimeClient>,
    roots: CliAgentStoreRoots,
    target: EnvironmentCliAgentSourceLocator,
    action: EnvironmentCliAgentSessionSourceAction,
) -> Result<(), String> {
    use crate::environment_runtime_transport::proto::CliAgentSessionMutation;

    let mutation = match action {
        EnvironmentCliAgentSessionSourceAction::Delete => CliAgentSessionMutation::Delete,
    };
    let roots = environment_cli_agent_store_roots_to_proto(&roots);
    client
        .mutate_cli_agent_session(target.source.clone(), mutation as i32, roots)
        .await
        .map_err(|error| format!("{error:#}"))
        .and_then(|response| match response.result {
            Some(
                crate::environment_runtime_transport::proto::mutate_cli_agent_session_response::Result::Success(
                    _,
                ),
            ) => Ok(()),
            None => Err("environment session source action returned no result".to_owned()),
            Some(
                crate::environment_runtime_transport::proto::mutate_cli_agent_session_response::Result::Error(
                    error,
                ),
            ) => {
                let message = error.message;
                if environment_file_missing_error(&message) {
                    Ok(())
                } else {
                    Err(format!(
                        "environment session source action failed: {}",
                        message
                    ))
                }
            }
        })
}

#[derive(Clone, Debug, Default)]
pub(crate) struct EnvironmentCliAgentSessionUserState {
    pub(crate) aliases: HashMap<String, String>,
    pub(crate) pinned: HashSet<String>,
}

#[derive(Clone, Debug)]
pub(crate) enum EnvironmentCliAgentSessionUserStateMutation {
    SetAlias(String),
    ClearAlias,
    SetPinned,
    ClearPinned,
}

#[cfg(not(target_family = "wasm"))]
pub(crate) async fn read_environment_cli_agent_session_user_state(
    client: Arc<EnvironmentRuntimeClient>,
) -> Result<EnvironmentCliAgentSessionUserState, String> {
    let response = client
        .get_cli_agent_session_user_state()
        .await
        .map_err(|error| format!("{error:#}"))?;
    match response.result {
        Some(
            crate::environment_runtime_transport::proto::get_cli_agent_session_user_state_response::Result::Success(
                success,
            ),
        ) => Ok(success
            .state
            .map(environment_cli_agent_session_user_state_from_proto)
            .unwrap_or_default()),
        Some(
            crate::environment_runtime_transport::proto::get_cli_agent_session_user_state_response::Result::Error(
                error,
            ),
        ) => Err(format!(
            "environment session user-state read failed: {}",
            error.message
        )),
        None => Err("environment session user-state read returned no result".to_owned()),
    }
}

#[cfg(not(target_family = "wasm"))]
pub(crate) async fn mutate_environment_cli_agent_session_user_state(
    client: Arc<EnvironmentRuntimeClient>,
    keys: Vec<String>,
    mutation: EnvironmentCliAgentSessionUserStateMutation,
) -> Result<EnvironmentCliAgentSessionUserState, String> {
    use crate::environment_runtime_transport::proto::CliAgentSessionUserStateMutation;

    let (mutation, alias) = match mutation {
        EnvironmentCliAgentSessionUserStateMutation::SetAlias(alias) => {
            (CliAgentSessionUserStateMutation::SetAlias, Some(alias))
        }
        EnvironmentCliAgentSessionUserStateMutation::ClearAlias => {
            (CliAgentSessionUserStateMutation::ClearAlias, None)
        }
        EnvironmentCliAgentSessionUserStateMutation::SetPinned => {
            (CliAgentSessionUserStateMutation::SetPinned, None)
        }
        EnvironmentCliAgentSessionUserStateMutation::ClearPinned => {
            (CliAgentSessionUserStateMutation::ClearPinned, None)
        }
    };
    let response = client
        .mutate_cli_agent_session_user_state(keys, mutation as i32, alias)
        .await
        .map_err(|error| format!("{error:#}"))?;
    match response.result {
        Some(
            crate::environment_runtime_transport::proto::mutate_cli_agent_session_user_state_response::Result::Success(
                success,
            ),
        ) => Ok(success
            .state
            .map(environment_cli_agent_session_user_state_from_proto)
            .unwrap_or_default()),
        Some(
            crate::environment_runtime_transport::proto::mutate_cli_agent_session_user_state_response::Result::Error(
                error,
            ),
        ) => Err(format!(
            "environment session user-state mutation failed: {}",
            error.message
        )),
        None => Err("environment session user-state mutation returned no result".to_owned()),
    }
}

#[cfg(not(target_family = "wasm"))]
fn environment_cli_agent_session_user_state_from_proto(
    state: crate::environment_runtime_transport::proto::CliAgentSessionUserState,
) -> EnvironmentCliAgentSessionUserState {
    EnvironmentCliAgentSessionUserState {
        aliases: state
            .aliases
            .into_iter()
            .filter_map(|(key, alias)| {
                let key = key.trim().to_owned();
                let alias = alias.trim().to_owned();
                (!key.is_empty() && !alias.is_empty()).then_some((key, alias))
            })
            .collect(),
        pinned: state
            .pinned
            .into_iter()
            .map(|key| key.trim().to_owned())
            .filter(|key| !key.is_empty())
            .collect(),
    }
}

#[derive(Clone, Debug)]
pub(crate) struct EnvironmentCliAgentSessionSourceBytes {
    pub(crate) reference: String,
    pub(crate) bytes: Vec<u8>,
}

#[cfg(not(target_family = "wasm"))]
pub(crate) async fn read_environment_cli_agent_session_source(
    client: Arc<EnvironmentRuntimeClient>,
    roots: CliAgentStoreRoots,
    target: EnvironmentCliAgentSourceLocator,
) -> Result<EnvironmentCliAgentSessionSourceBytes, String> {
    let roots = environment_cli_agent_store_roots_to_proto(&roots);
    let response = client
        .read_cli_agent_session(
            target.source.clone(),
            Some(target.provider_session_id.clone()),
            roots,
        )
        .await
        .map_err(|error| format!("{error:#}"))?;
    match response.result {
        Some(
            crate::environment_runtime_transport::proto::read_cli_agent_session_response::Result::Success(
                success,
            ),
        ) => Ok(EnvironmentCliAgentSessionSourceBytes {
            reference: success.reference,
            bytes: success.content,
        }),
        Some(
            crate::environment_runtime_transport::proto::read_cli_agent_session_response::Result::Error(
                error,
            ),
        ) => Err(error.message),
        None => Err("environment session read returned no result".to_owned()),
    }
}

pub(crate) async fn read_environment_file_all(
    client: &EnvironmentRuntimeClient,
    path: String,
) -> Result<Vec<u8>, String> {
    const CHUNK_BYTES: u64 = 512 * 1024;
    const MAX_BYTES: u64 = 64 * 1024 * 1024;

    let mut transfer = begin_read_file_transfer(client, path.clone()).await?;
    let handle = transfer.handle.clone();
    if transfer.total_size() > MAX_BYTES {
        let _ = abort_file_transfer(client, handle).await;
        return Err(format!(
            "refusing to read oversized environment file: {path}"
        ));
    }
    let mut bytes = Vec::with_capacity(transfer.total_size() as usize);
    loop {
        let chunk = match read_file_chunk(client, handle.clone(), CHUNK_BYTES).await {
            Ok(chunk) => chunk,
            Err(error) => {
                let _ = abort_file_transfer(client, handle.clone()).await;
                return Err(error);
            }
        };
        if let Err(error) = transfer.accept_chunk(&chunk) {
            let _ = abort_file_transfer(client, handle.clone()).await;
            return Err(format!("{error}: {path}"));
        }
        bytes.extend(chunk.bytes);
        if chunk.eof {
            let committed_path = finish_file_transfer(client, handle).await?;
            if committed_path.is_some() {
                return Err(format!(
                    "environment read transfer unexpectedly committed a path: {committed_path:?}"
                ));
            }
            return Ok(bytes);
        }
    }
}

pub(crate) async fn write_environment_file_all(
    client: &EnvironmentRuntimeClient,
    path: String,
    contents: Vec<u8>,
) -> Result<(), String> {
    const CHUNK_BYTES: usize = 512 * 1024;

    if let Some(parent) = environment_parent_path(&path) {
        create_directory(client, parent).await?;
    }
    let mut transfer = begin_write_file_transfer(client, path.clone(), None).await?;
    let handle = transfer.handle.clone();

    for chunk in contents.chunks(CHUNK_BYTES) {
        let success = match write_file_chunk(client, handle.clone(), chunk.to_vec()).await {
            Ok(success) => success,
            Err(error) => {
                let _ = abort_file_transfer(client, handle.clone()).await;
                return Err(error);
            }
        };
        if let Err(error) = transfer.accept_chunk(chunk.len(), &success) {
            let _ = abort_file_transfer(client, handle.clone()).await;
            return Err(format!("{error}: {path}"));
        }
    }
    let committed_path = finish_file_transfer(client, handle).await?;
    if committed_path.as_deref() != Some(path.as_str()) {
        return Err(format!(
            "environment file write committed unexpected path: requested={path}, committed={committed_path:?}"
        ));
    }
    Ok(())
}

pub(crate) async fn append_environment_file(
    client: &EnvironmentRuntimeClient,
    path: String,
    contents: Vec<u8>,
) -> Result<(), String> {
    let response = client
        .append_file(path, contents)
        .await
        .map_err(|error| error.to_string())?;
    match response.result {
        Some(
            crate::environment_runtime_transport::proto::append_file_response::Result::Success(_),
        ) => Ok(()),
        Some(crate::environment_runtime_transport::proto::append_file_response::Result::Error(
            error,
        )) => Err(error.message),
        None => Err(crate::t!("server-file-browser-empty-response")),
    }
}

pub(crate) fn environment_parent_path(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches(['/', '\\']);
    let index = trimmed.rfind(['/', '\\'])?;
    if index == 0 {
        return Some(trimmed[..=0].to_owned());
    }
    Some(trimmed[..index].to_owned()).filter(|parent| !parent.is_empty())
}

fn environment_file_missing_error(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("no such file")
        || error.contains("not found")
        || error.contains("does not exist")
        || error.contains("cannot find")
}

#[derive(Clone, Debug)]
pub(crate) struct EnvironmentCliAgentSessionRecord {
    pub(crate) agent: CLIAgent,
    pub(crate) provider_session_id: String,
    pub(crate) source: String,
    pub(crate) label: Option<String>,
    pub(crate) cwd: Option<String>,
    pub(crate) modified_ms: Option<i64>,
}

/// Remote delivery preserves the shared discovery plan outcome instead of
/// flattening a previously-observed missing provider into an empty record set.
#[derive(Clone, Debug)]
pub(crate) enum EnvironmentCliAgentSessionDiscovery {
    Complete {
        observed_agents: HashSet<CLIAgent>,
        records: Vec<EnvironmentCliAgentSessionRecord>,
    },
    SourceMissing(CLIAgent),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EnvironmentCliAgentSourceLocator {
    pub(crate) authority: String,
    pub(crate) source: String,
    pub(crate) agent: CLIAgent,
    pub(crate) provider_session_id: String,
}

pub(crate) fn environment_cli_agent_session_source_id(
    authority: &str,
    agent: &CLIAgent,
    source: &str,
) -> String {
    format!(
        "remote:{}:{}:{}",
        authority,
        agent.to_serialized_name(),
        hex_encode_for_session_id(source.as_bytes())
    )
}

pub(crate) fn is_environment_cli_agent_session_source_id(session_id: &str) -> bool {
    session_id.starts_with("remote:")
}

pub(crate) fn environment_cli_agent_session_source_target_from_id(
    session_id: &str,
    cli_agent: Option<&str>,
    provider_session_id: Option<String>,
) -> Result<Option<EnvironmentCliAgentSourceLocator>, String> {
    let Some(encoded_payload) = session_id.strip_prefix("remote:") else {
        return Ok(None);
    };
    let mut parts = encoded_payload.rsplitn(3, ':');
    let hex_source = parts
        .next()
        .ok_or_else(|| "remote CLI session source id is missing source".to_owned())?;
    let encoded_agent = parts
        .next()
        .ok_or_else(|| "remote CLI session source id is missing agent".to_owned())?;
    let authority = parts
        .next()
        .filter(|authority| !authority.trim().is_empty())
        .ok_or_else(|| "remote CLI session source id is missing authority".to_owned())?;
    let source = hex_decode_session_id_component(hex_source)
        .filter(|source| !source.trim().is_empty())
        .ok_or_else(|| "remote CLI session source id has invalid source".to_owned())?;
    let encoded_agent = CLIAgent::from_serialized_name(encoded_agent);
    if matches!(encoded_agent, CLIAgent::Unknown) {
        return Err("remote CLI session source id has unknown agent".to_owned());
    }
    let metadata_agent = match cli_agent {
        Some(name) => {
            let agent = CLIAgent::from_serialized_name(name);
            if matches!(agent, CLIAgent::Unknown) {
                return Err("remote CLI session snapshot has unknown agent".to_owned());
            }
            Some(agent)
        }
        None => None,
    };
    if metadata_agent.is_some_and(|agent| agent != encoded_agent) {
        return Err(
            "remote CLI session source id agent does not match snapshot metadata".to_owned(),
        );
    }
    let provider_session_id = provider_session_id
        .filter(|provider_session_id| !provider_session_id.trim().is_empty())
        .ok_or_else(|| "remote CLI session source is missing provider session id".to_owned())?;
    Ok(Some(EnvironmentCliAgentSourceLocator {
        authority: authority.to_owned(),
        source,
        agent: encoded_agent,
        provider_session_id,
    }))
}

#[cfg(not(target_family = "wasm"))]
fn decode_environment_cli_agent_scan_records(
    records: Vec<crate::environment_runtime_transport::proto::CliAgentSessionRecord>,
) -> Result<Vec<EnvironmentCliAgentSessionRecord>, String> {
    let mut records = records
        .into_iter()
        .enumerate()
        .map(|(index, record)| {
            let agent = match record.agent.as_str() {
                "claude" => CLIAgent::Claude,
                "codex" => CLIAgent::Codex,
                name => CLIAgent::from_serialized_name(name),
            };
            if matches!(agent, CLIAgent::Unknown) {
                return Err(format!(
                    "environment session scan record {index} has unknown agent {:?}",
                    record.agent
                ));
            }
            let provider_session_id = record.id.trim();
            if provider_session_id.is_empty() {
                return Err(format!(
                    "environment session scan record {index} is missing provider session id"
                ));
            }
            if record.source.trim().is_empty() {
                return Err(format!(
                    "environment session scan record {index} is missing source"
                ));
            }
            Ok(EnvironmentCliAgentSessionRecord {
                agent,
                provider_session_id: provider_session_id.to_owned(),
                source: record.source,
                label: record.label.filter(|label| !label.trim().is_empty()),
                cwd: record.cwd.filter(|cwd| !cwd.trim().is_empty()),
                modified_ms: record.modified_epoch_millis,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    records.sort_by(|left, right| right.modified_ms.cmp(&left.modified_ms));
    Ok(records)
}

#[cfg(not(target_family = "wasm"))]
pub(crate) async fn scan_environment_cli_agent_sessions(
    client: Arc<EnvironmentRuntimeClient>,
    session_id: SessionId,
    enabled_agents: &[crate::terminal::CLIAgent],
    previously_observed_agents: &HashSet<CLIAgent>,
) -> Result<EnvironmentCliAgentSessionDiscovery, String> {
    let roots = resolve_environment_cli_agent_store_roots(&client, session_id).await?;
    scan_environment_cli_agent_sessions_with_roots(
        client,
        roots,
        enabled_agents,
        previously_observed_agents,
    )
    .await
}

#[cfg(not(target_family = "wasm"))]
pub(crate) async fn scan_environment_cli_agent_sessions_with_roots(
    client: Arc<EnvironmentRuntimeClient>,
    roots: CliAgentStoreRoots,
    enabled_agents: &[crate::terminal::CLIAgent],
    previously_observed_agents: &HashSet<CLIAgent>,
) -> Result<EnvironmentCliAgentSessionDiscovery, String> {
    let roots = environment_cli_agent_store_roots_to_proto(&roots);
    let response = client
        .scan_cli_agent_sessions(
            crate::app_state::WORKSPACE_SESSION_NAVIGATOR_LOGICAL_LIMIT as u32,
            roots,
            enabled_agents
                .iter()
                .map(crate::terminal::CLIAgent::to_serialized_name)
                .collect(),
            previously_observed_agents
                .iter()
                .map(crate::terminal::CLIAgent::to_serialized_name)
                .collect(),
        )
        .await
        .map_err(|error| format!("{error:#}"))?;

    let success = match response.result {
        Some(
            crate::environment_runtime_transport::proto::scan_cli_agent_sessions_response::Result::Success(
                success,
            ),
        ) => success,
        Some(
            crate::environment_runtime_transport::proto::scan_cli_agent_sessions_response::Result::Error(
                error,
            ),
        ) => return Err(format!("environment session scan failed: {}", error.message)),
        None => return Err("environment session scan returned no result".to_owned()),
    };

    if let Some(agent) = success.source_missing_agent {
        let agent = CLIAgent::from_serialized_name(&agent);
        if matches!(agent, CLIAgent::Unknown) {
            return Err(
                "environment session scan returned unknown source-missing agent".to_owned(),
            );
        }
        return Ok(EnvironmentCliAgentSessionDiscovery::SourceMissing(agent));
    }

    let observed_agents = success
        .observed_agents
        .into_iter()
        .map(|name| CLIAgent::from_serialized_name(&name))
        .map(|agent| {
            if matches!(agent, CLIAgent::Unknown) {
                Err("environment session scan returned unknown observed agent".to_owned())
            } else {
                Ok(agent)
            }
        })
        .collect::<Result<HashSet<_>, _>>()?;
    let records = decode_environment_cli_agent_scan_records(success.records)?;
    Ok(EnvironmentCliAgentSessionDiscovery::Complete {
        observed_agents,
        records,
    })
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn hex_decode_session_id_component(hex: &str) -> Option<String> {
    if hex.len() % 2 != 0 {
        return None;
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    let mut chars = hex.as_bytes().chunks_exact(2);
    for chunk in &mut chars {
        let hi = (chunk[0] as char).to_digit(16)?;
        let lo = (chunk[1] as char).to_digit(16)?;
        bytes.push(((hi << 4) | lo) as u8);
    }
    String::from_utf8(bytes).ok()
}

#[cfg(target_family = "wasm")]
pub(crate) fn hex_decode_session_id_component(_hex: &str) -> Option<String> {
    None
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn hex_encode_for_session_id(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(target_family = "wasm")]
pub(crate) fn hex_encode_for_session_id(_bytes: &[u8]) -> String {
    String::new()
}

#[derive(Clone, Debug)]
pub(crate) struct EnvironmentRuntimeTarget {
    pub(crate) authority: String,
    pub(crate) session_id: SessionId,
    pub(crate) host_id: HostId,
    pub(crate) root: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct EnvironmentRuntimeTerminalSpawn {
    pub(crate) target: EnvironmentRuntimeTarget,
    pub(crate) root: String,
}

#[derive(Clone, Debug)]
pub(crate) struct TerminalBootstrapTarget {
    pub(crate) authority: String,
    pub(crate) root: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct TerminalBootstrapSpawn {
    pub(crate) target: TerminalBootstrapTarget,
    pub(crate) initial_directory: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub(crate) struct EnvironmentTerminalBootstrap {
    pub(crate) options: NewTerminalOptions,
    pub(crate) enter_agent_view: bool,
}

#[derive(Clone, Debug)]
pub(crate) enum EnvironmentRuntimeSpawnPlan {
    TerminalBootstrap(TerminalBootstrapTarget),
    RuntimeTarget(EnvironmentRuntimeTarget),
    RuntimeBootstrap,
}

pub(crate) trait EnvironmentRuntimeSpawnPlanHandler {
    type Output;

    fn open_terminal_bootstrap_target(&mut self, target: TerminalBootstrapTarget) -> Self::Output;

    fn open_runtime_target(&mut self, target: EnvironmentRuntimeTarget) -> Self::Output;

    fn bootstrap_runtime_target(&mut self) -> Self::Output;
}

impl EnvironmentRuntimeSpawnPlan {
    pub(crate) fn open_with<H: EnvironmentRuntimeSpawnPlanHandler>(
        self,
        handler: &mut H,
    ) -> H::Output {
        match self {
            Self::TerminalBootstrap(target) => handler.open_terminal_bootstrap_target(target),
            Self::RuntimeTarget(target) => handler.open_runtime_target(target),
            Self::RuntimeBootstrap => handler.bootstrap_runtime_target(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum EnvironmentSessionTabPlan {
    TerminalBootstrap {
        environment: EnvironmentSnapshot,
        spawn: EnvironmentTerminalBootstrap,
    },
    RuntimeEntry {
        environment: EnvironmentSnapshot,
        hide_homepage: bool,
    },
}

pub(crate) trait EnvironmentSessionTabPlanHandler {
    fn open_terminal_bootstrap(
        &mut self,
        environment: EnvironmentSnapshot,
        spawn: EnvironmentTerminalBootstrap,
    );

    fn open_runtime_entry(&mut self, environment: EnvironmentSnapshot, hide_homepage: bool);
}

impl EnvironmentSessionTabPlan {
    pub(crate) fn open_with(self, handler: &mut impl EnvironmentSessionTabPlanHandler) {
        match self {
            Self::TerminalBootstrap { environment, spawn } => {
                handler.open_terminal_bootstrap(environment, spawn);
            }
            Self::RuntimeEntry {
                environment,
                hide_homepage,
            } => handler.open_runtime_entry(environment, hide_homepage),
        }
    }
}

pub(crate) fn session_tab_plan_for_environment(
    environment: EnvironmentSnapshot,
    requires_current_app_terminal_capabilities: bool,
    current_app_spawn: EnvironmentTerminalBootstrap,
    hide_homepage: bool,
) -> EnvironmentSessionTabPlan {
    if uses_environment_runtime(&environment) && !requires_current_app_terminal_capabilities {
        EnvironmentSessionTabPlan::RuntimeEntry {
            environment,
            hide_homepage,
        }
    } else {
        EnvironmentSessionTabPlan::TerminalBootstrap {
            environment,
            spawn: current_app_spawn,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum EnvironmentCliAgentSessionSourceAction {
    Delete,
}

#[cfg(all(test, not(target_family = "wasm")))]
impl EnvironmentCliAgentSessionSourceAction {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Delete => "delete",
        }
    }

    pub(crate) fn localized_verb(self) -> &'static str {
        match self {
            Self::Delete => "删除",
        }
    }
}

pub(crate) fn environment_runtime_pty_options_for_spawn(
    client: Arc<EnvironmentRuntimeClient>,
    spawn: &EnvironmentRuntimeTerminalSpawn,
) -> NewTerminalOptions {
    NewTerminalOptions::default().with_environment_runtime_pty(EnvironmentRuntimePtyProcess {
        client,
        session_id: spawn.target.session_id,
        working_directory: spawn.root.clone(),
        shell: String::new(),
        // Native environment-runtime PTYs must not receive the restored agent command
        // as raw bytes during bootstrap. Keep it in TerminalView's pending-command
        // path so it runs after shell integration finishes bootstrapping.
        startup_command: None,
        environment_variables: terminal_capability_environment_variables(),
    })
}

pub(crate) fn terminal_bootstrap_options(
    initial_directory: Option<PathBuf>,
    shell: Option<AvailableShell>,
    conversation_restoration: Option<ConversationRestorationInNewPaneType>,
    hide_homepage: bool,
) -> NewTerminalOptions {
    NewTerminalOptions {
        shell,
        initial_directory,
        conversation_restoration,
        hide_homepage,
        ..Default::default()
    }
}

pub(crate) fn terminal_session_tab_bootstrap(
    initial_directory: Option<PathBuf>,
    shell: Option<AvailableShell>,
    conversation_restoration: Option<ConversationRestorationInNewPaneType>,
    hide_homepage: bool,
    enter_agent_view: bool,
) -> EnvironmentTerminalBootstrap {
    terminal_session_tab_bootstrap_from_options(
        terminal_bootstrap_options(
            initial_directory,
            shell,
            conversation_restoration,
            hide_homepage,
        ),
        enter_agent_view,
    )
}

pub(crate) fn terminal_session_tab_bootstrap_from_options(
    options: NewTerminalOptions,
    enter_agent_view: bool,
) -> EnvironmentTerminalBootstrap {
    EnvironmentTerminalBootstrap {
        options,
        enter_agent_view,
    }
}

pub(crate) fn terminal_bootstrap_panes_layout(options: Box<NewTerminalOptions>) -> PanesLayout {
    PanesLayout::SingleTerminal(options)
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn transport_for_target(
    control_path: PathBuf,
    target: String,
    auth_context: Arc<EnvironmentRuntimeAuthContext>,
) -> EnvironmentRuntimeTransport {
    EnvironmentRuntimeTransport::new_with_target(control_path, target, auth_context)
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn transport_for_control_path(
    control_path: PathBuf,
    auth_context: Arc<EnvironmentRuntimeAuthContext>,
) -> EnvironmentRuntimeTransport {
    EnvironmentRuntimeTransport::new(control_path, auth_context)
}

pub(crate) fn auth_context(ctx: &AppContext) -> Arc<EnvironmentRuntimeAuthContext> {
    Arc::new(environment_runtime_auth_context(
        AuthStateProvider::as_ref(ctx).get().clone(),
    ))
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn check_target_binary<T: View>(
    session_id: SessionId,
    control_path: PathBuf,
    target: String,
    ctx: &mut ViewContext<T>,
) {
    let transport = transport_for_target(control_path, target, auth_context(ctx));
    check_binary(session_id, transport, ctx);
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn install_target_binary<T: View>(
    session_id: SessionId,
    control_path: PathBuf,
    target: String,
    has_old_binary: bool,
    ctx: &mut ViewContext<T>,
) {
    let transport = transport_for_target(control_path, target, auth_context(ctx));
    install_binary(session_id, transport, has_old_binary, ctx);
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn connect_target_transport<T: View>(
    session_id: SessionId,
    control_path: PathBuf,
    target: String,
    ctx: &mut ViewContext<T>,
) {
    let auth_context = auth_context(ctx);
    let transport = transport_for_target(control_path, target, auth_context.clone());
    connect_transport(session_id, transport, auth_context, ctx);
}

#[cfg(feature = "local_fs")]
pub(crate) fn subscribe_to_transport_events<T, F>(ctx: &mut ViewContext<T>, mut callback: F)
where
    T: View,
    F: 'static + FnMut(&mut T, &EnvironmentRuntimeTransportEvent, &mut ViewContext<T>),
{
    ctx.subscribe_to_model(
        &EnvironmentRuntimeTransportManager::handle(ctx),
        move |view, _handle, event, ctx| callback(view, event, ctx),
    );
}

#[cfg(feature = "local_fs")]
pub(crate) fn subscribe_to_repo_metadata_updates(
    ctx: &mut ModelContext<repo_metadata::RepoMetadataModel>,
) {
    let manager = EnvironmentRuntimeTransportManager::handle(ctx);
    ctx.subscribe_to_model(&manager, |model, event, ctx| match event {
        EnvironmentRuntimeTransportEvent::RepoMetadataSnapshot { host_id, update } => {
            model.insert_remote_snapshot(host_id.clone(), update, ctx);
        }
        EnvironmentRuntimeTransportEvent::RepoMetadataUpdated { host_id, update } => {
            model.apply_remote_incremental_update(host_id, update, ctx);
        }
        EnvironmentRuntimeTransportEvent::RepoMetadataDirectoryLoaded {
            host_id,
            dir_path,
            update,
        } => {
            model.apply_remote_directory_load(host_id, dir_path, update, ctx);
        }
        EnvironmentRuntimeTransportEvent::RepoMetadataDirectoryLoadFailed {
            host_id,
            repo_path,
            dir_path,
            ..
        } => {
            model.finish_remote_directory_load(host_id, repo_path, dir_path, ctx);
        }
        EnvironmentRuntimeTransportEvent::HostDisconnected { host_id } => {
            model.remove_remote_repositories_for_host(host_id, ctx);
        }
        EnvironmentRuntimeTransportEvent::SessionConnecting { .. }
        | EnvironmentRuntimeTransportEvent::SessionConnected { .. }
        | EnvironmentRuntimeTransportEvent::SessionConnectionFailed { .. }
        | EnvironmentRuntimeTransportEvent::SessionDisconnected { .. }
        | EnvironmentRuntimeTransportEvent::SessionReconnected { .. }
        | EnvironmentRuntimeTransportEvent::SessionDeregistered { .. }
        | EnvironmentRuntimeTransportEvent::SessionExecutionContextEstablished { .. }
        | EnvironmentRuntimeTransportEvent::HostConnected { .. }
        | EnvironmentRuntimeTransportEvent::NavigatedToDirectory { .. }
        | EnvironmentRuntimeTransportEvent::BufferUpdated { .. }
        | EnvironmentRuntimeTransportEvent::SetupStateChanged { .. }
        | EnvironmentRuntimeTransportEvent::ClientRequestFailed { .. }
        | EnvironmentRuntimeTransportEvent::ServerMessageDecodingError { .. }
        | EnvironmentRuntimeTransportEvent::PtyOutput { .. }
        | EnvironmentRuntimeTransportEvent::PtyExited { .. }
        | EnvironmentRuntimeTransportEvent::BinaryCheckComplete { .. }
        | EnvironmentRuntimeTransportEvent::BinaryInstallComplete { .. } => {}
    });
}

#[cfg(feature = "local_tty")]
pub(crate) fn subscribe_to_buffer_updates<T, F>(ctx: &mut ModelContext<T>, mut callback: F)
where
    T: Entity,
    F: 'static + FnMut(&mut T, EnvironmentRuntimeBufferUpdate, &mut ModelContext<T>),
{
    let manager = EnvironmentRuntimeTransportManager::handle(ctx);
    ctx.subscribe_to_model(&manager, move |model, event, ctx| {
        let EnvironmentRuntimeTransportEvent::BufferUpdated {
            session_id,
            host_id,
            path,
            new_server_version,
            expected_client_version,
            edits,
        } = event
        else {
            return;
        };
        callback(
            model,
            EnvironmentRuntimeBufferUpdate {
                session_id: *session_id,
                host_id: host_id.clone(),
                path: path.clone(),
                new_server_version: *new_server_version,
                expected_client_version: *expected_client_version,
                edits: edits
                    .iter()
                    .map(|edit| EnvironmentRuntimeBufferEdit {
                        start_offset: edit.start_offset,
                        end_offset: edit.end_offset,
                        text: edit.text.clone(),
                    })
                    .collect(),
            },
            ctx,
        );
    });
}

#[cfg(feature = "local_tty")]
pub(crate) fn subscribe_to_session_events<T, F>(ctx: &mut ModelContext<T>, mut callback: F)
where
    T: Entity,
    F: 'static + FnMut(&mut T, EnvironmentRuntimeSessionEvent, &mut ModelContext<T>),
{
    let manager = EnvironmentRuntimeTransportManager::handle(ctx);
    ctx.subscribe_to_model(&manager, move |model, event, ctx| {
        let event = match event {
            EnvironmentRuntimeTransportEvent::SessionConnected {
                session_id,
                host_id,
            } => EnvironmentRuntimeSessionEvent::Connected {
                session_id: *session_id,
                host_id: host_id.clone(),
            },
            EnvironmentRuntimeTransportEvent::SessionDisconnected { session_id, .. } => {
                EnvironmentRuntimeSessionEvent::Disconnected {
                    session_id: *session_id,
                }
            }
            EnvironmentRuntimeTransportEvent::SetupStateChanged { session_id, state } => {
                EnvironmentRuntimeSessionEvent::SetupStateChanged {
                    session_id: *session_id,
                    state: state.clone(),
                }
            }
            EnvironmentRuntimeTransportEvent::SessionReconnected {
                session_id, client, ..
            } => EnvironmentRuntimeSessionEvent::Reconnected {
                session_id: *session_id,
                client: client.clone(),
            },
            EnvironmentRuntimeTransportEvent::SessionConnecting { .. }
            | EnvironmentRuntimeTransportEvent::SessionDeregistered { .. }
            | EnvironmentRuntimeTransportEvent::SessionExecutionContextEstablished { .. }
            | EnvironmentRuntimeTransportEvent::SessionConnectionFailed { .. }
            | EnvironmentRuntimeTransportEvent::HostConnected { .. }
            | EnvironmentRuntimeTransportEvent::HostDisconnected { .. }
            | EnvironmentRuntimeTransportEvent::NavigatedToDirectory { .. }
            | EnvironmentRuntimeTransportEvent::RepoMetadataSnapshot { .. }
            | EnvironmentRuntimeTransportEvent::RepoMetadataUpdated { .. }
            | EnvironmentRuntimeTransportEvent::RepoMetadataDirectoryLoaded { .. }
            | EnvironmentRuntimeTransportEvent::RepoMetadataDirectoryLoadFailed { .. }
            | EnvironmentRuntimeTransportEvent::BufferUpdated { .. }
            | EnvironmentRuntimeTransportEvent::PtyOutput { .. }
            | EnvironmentRuntimeTransportEvent::PtyExited { .. }
            | EnvironmentRuntimeTransportEvent::BinaryCheckComplete { .. }
            | EnvironmentRuntimeTransportEvent::BinaryInstallComplete { .. }
            | EnvironmentRuntimeTransportEvent::ClientRequestFailed { .. }
            | EnvironmentRuntimeTransportEvent::ServerMessageDecodingError { .. } => return,
        };
        callback(model, event, ctx);
    });
}

#[cfg(feature = "local_tty")]
pub(crate) fn subscribe_to_pty_events<T, F>(ctx: &mut ModelContext<T>, mut callback: F)
where
    T: Entity,
    F: 'static + FnMut(&mut T, EnvironmentRuntimePtyEvent, &mut ModelContext<T>),
{
    let manager = EnvironmentRuntimeTransportManager::handle(ctx);
    ctx.subscribe_to_model(&manager, move |model, event, ctx| {
        let event = match event {
            EnvironmentRuntimeTransportEvent::PtyOutput {
                session_id,
                pty_id,
                bytes,
                ..
            } => EnvironmentRuntimePtyEvent::Output {
                session_id: *session_id,
                pty_id: *pty_id,
                bytes: bytes.clone(),
            },
            EnvironmentRuntimeTransportEvent::PtyExited {
                session_id, pty_id, ..
            } => EnvironmentRuntimePtyEvent::Exited {
                session_id: *session_id,
                pty_id: *pty_id,
            },
            EnvironmentRuntimeTransportEvent::SessionConnecting { .. }
            | EnvironmentRuntimeTransportEvent::SessionConnected { .. }
            | EnvironmentRuntimeTransportEvent::SessionConnectionFailed { .. }
            | EnvironmentRuntimeTransportEvent::SessionDisconnected { .. }
            | EnvironmentRuntimeTransportEvent::SessionReconnected { .. }
            | EnvironmentRuntimeTransportEvent::SessionDeregistered { .. }
            | EnvironmentRuntimeTransportEvent::SessionExecutionContextEstablished { .. }
            | EnvironmentRuntimeTransportEvent::HostConnected { .. }
            | EnvironmentRuntimeTransportEvent::HostDisconnected { .. }
            | EnvironmentRuntimeTransportEvent::NavigatedToDirectory { .. }
            | EnvironmentRuntimeTransportEvent::RepoMetadataSnapshot { .. }
            | EnvironmentRuntimeTransportEvent::RepoMetadataUpdated { .. }
            | EnvironmentRuntimeTransportEvent::RepoMetadataDirectoryLoaded { .. }
            | EnvironmentRuntimeTransportEvent::RepoMetadataDirectoryLoadFailed { .. }
            | EnvironmentRuntimeTransportEvent::BufferUpdated { .. }
            | EnvironmentRuntimeTransportEvent::SetupStateChanged { .. }
            | EnvironmentRuntimeTransportEvent::BinaryCheckComplete { .. }
            | EnvironmentRuntimeTransportEvent::BinaryInstallComplete { .. }
            | EnvironmentRuntimeTransportEvent::ClientRequestFailed { .. }
            | EnvironmentRuntimeTransportEvent::ServerMessageDecodingError { .. } => return,
        };
        callback(model, event, ctx);
    });
}

#[cfg(feature = "local_tty")]
pub(crate) fn subscribe_to_setup_events<T, F>(ctx: &mut ModelContext<T>, mut callback: F)
where
    T: Entity,
    F: 'static + FnMut(&mut T, EnvironmentRuntimeSetupEvent, &mut ModelContext<T>),
{
    let manager = EnvironmentRuntimeTransportManager::handle(ctx);
    ctx.subscribe_to_model(&manager, move |model, event, ctx| {
        let event = match event {
            EnvironmentRuntimeTransportEvent::BinaryCheckComplete {
                session_id,
                result,
                preinstall_check,
                has_old_binary,
                ..
            } => EnvironmentRuntimeSetupEvent::BinaryCheckComplete {
                session_id: *session_id,
                result: result.clone(),
                preinstall_check: preinstall_check.clone(),
                has_old_binary: *has_old_binary,
            },
            EnvironmentRuntimeTransportEvent::BinaryInstallComplete { session_id, result } => {
                EnvironmentRuntimeSetupEvent::BinaryInstallComplete {
                    session_id: *session_id,
                    result: result.clone(),
                }
            }
            EnvironmentRuntimeTransportEvent::SessionConnected { session_id, .. } => {
                EnvironmentRuntimeSetupEvent::Connected {
                    session_id: *session_id,
                }
            }
            EnvironmentRuntimeTransportEvent::SessionConnectionFailed { session_id, .. } => {
                EnvironmentRuntimeSetupEvent::ConnectionFailed {
                    session_id: *session_id,
                }
            }
            EnvironmentRuntimeTransportEvent::SessionConnecting { .. }
            | EnvironmentRuntimeTransportEvent::SessionDisconnected { .. }
            | EnvironmentRuntimeTransportEvent::SessionReconnected { .. }
            | EnvironmentRuntimeTransportEvent::SessionDeregistered { .. }
            | EnvironmentRuntimeTransportEvent::SessionExecutionContextEstablished { .. }
            | EnvironmentRuntimeTransportEvent::HostConnected { .. }
            | EnvironmentRuntimeTransportEvent::HostDisconnected { .. }
            | EnvironmentRuntimeTransportEvent::NavigatedToDirectory { .. }
            | EnvironmentRuntimeTransportEvent::RepoMetadataSnapshot { .. }
            | EnvironmentRuntimeTransportEvent::RepoMetadataUpdated { .. }
            | EnvironmentRuntimeTransportEvent::RepoMetadataDirectoryLoaded { .. }
            | EnvironmentRuntimeTransportEvent::RepoMetadataDirectoryLoadFailed { .. }
            | EnvironmentRuntimeTransportEvent::BufferUpdated { .. }
            | EnvironmentRuntimeTransportEvent::SetupStateChanged { .. }
            | EnvironmentRuntimeTransportEvent::ClientRequestFailed { .. }
            | EnvironmentRuntimeTransportEvent::ServerMessageDecodingError { .. }
            | EnvironmentRuntimeTransportEvent::PtyOutput { .. }
            | EnvironmentRuntimeTransportEvent::PtyExited { .. } => return,
        };
        callback(model, event, ctx);
    });
}

#[cfg(feature = "local_tty")]
pub(crate) fn subscribe_to_terminal_events<T, F>(ctx: &mut ViewContext<T>, mut callback: F)
where
    T: View,
    F: 'static + FnMut(&mut T, EnvironmentRuntimeTerminalEvent, &mut ViewContext<T>),
{
    let manager = EnvironmentRuntimeTransportManager::handle(ctx);
    ctx.subscribe_to_model(&manager, move |view, _handle, event, ctx| {
        let event = match event {
            EnvironmentRuntimeTransportEvent::SetupStateChanged { session_id, .. } => {
                EnvironmentRuntimeTerminalEvent::SetupStateChanged {
                    session_id: *session_id,
                }
            }
            EnvironmentRuntimeTransportEvent::SessionConnected { session_id, .. } => {
                EnvironmentRuntimeTerminalEvent::SessionConnected {
                    session_id: *session_id,
                }
            }
            EnvironmentRuntimeTransportEvent::SessionConnectionFailed {
                session_id, error, ..
            } => EnvironmentRuntimeTerminalEvent::SessionConnectionFailed {
                session_id: *session_id,
                error: error.clone(),
            },
            EnvironmentRuntimeTransportEvent::SessionDisconnected { session_id, .. } => {
                EnvironmentRuntimeTerminalEvent::SessionDisconnected {
                    session_id: *session_id,
                }
            }
            EnvironmentRuntimeTransportEvent::SessionDeregistered { session_id } => {
                EnvironmentRuntimeTerminalEvent::SessionDeregistered {
                    session_id: *session_id,
                }
            }
            EnvironmentRuntimeTransportEvent::BinaryInstallComplete { session_id, result } => {
                EnvironmentRuntimeTerminalEvent::BinaryInstallComplete {
                    session_id: *session_id,
                    result: result.clone(),
                }
            }
            EnvironmentRuntimeTransportEvent::BinaryCheckComplete {
                session_id, result, ..
            } => EnvironmentRuntimeTerminalEvent::BinaryCheckComplete {
                session_id: *session_id,
                result: result.clone(),
            },
            EnvironmentRuntimeTransportEvent::ClientRequestFailed { session_id, .. } => {
                EnvironmentRuntimeTerminalEvent::ClientRequestFailed {
                    session_id: *session_id,
                }
            }
            EnvironmentRuntimeTransportEvent::ServerMessageDecodingError { session_id } => {
                EnvironmentRuntimeTerminalEvent::ServerMessageDecodingError {
                    session_id: *session_id,
                }
            }
            EnvironmentRuntimeTransportEvent::NavigatedToDirectory {
                session_id,
                host_id,
                requested_path,
                indexed_path,
                ..
            } => EnvironmentRuntimeTerminalEvent::NavigatedToDirectory {
                session_id: *session_id,
                host_id: host_id.clone(),
                requested_path: requested_path.clone(),
                indexed_path: indexed_path.clone(),
            },
            EnvironmentRuntimeTransportEvent::SessionConnecting { .. }
            | EnvironmentRuntimeTransportEvent::SessionReconnected { .. }
            | EnvironmentRuntimeTransportEvent::SessionExecutionContextEstablished { .. }
            | EnvironmentRuntimeTransportEvent::HostConnected { .. }
            | EnvironmentRuntimeTransportEvent::HostDisconnected { .. }
            | EnvironmentRuntimeTransportEvent::RepoMetadataSnapshot { .. }
            | EnvironmentRuntimeTransportEvent::RepoMetadataUpdated { .. }
            | EnvironmentRuntimeTransportEvent::RepoMetadataDirectoryLoaded { .. }
            | EnvironmentRuntimeTransportEvent::RepoMetadataDirectoryLoadFailed { .. }
            | EnvironmentRuntimeTransportEvent::BufferUpdated { .. }
            | EnvironmentRuntimeTransportEvent::PtyOutput { .. }
            | EnvironmentRuntimeTransportEvent::PtyExited { .. } => return,
        };
        callback(view, event, ctx);
    });
}

pub(crate) fn client_for_session(
    session_id: SessionId,
    ctx: &AppContext,
) -> Option<Arc<EnvironmentRuntimeClient>> {
    EnvironmentRuntimeTransportManager::as_ref(ctx)
        .client_for_session(session_id)
        .cloned()
}

pub(crate) fn sessions_share_transport_binding(
    left: SessionId,
    right: SessionId,
    ctx: &AppContext,
) -> bool {
    EnvironmentRuntimeTransportManager::as_ref(ctx).sessions_share_transport_binding(left, right)
}

pub(crate) fn has_session_execution_context(session_id: SessionId, ctx: &AppContext) -> bool {
    EnvironmentRuntimeTransportManager::as_ref(ctx).has_session_execution_context(session_id)
}

#[cfg(feature = "local_fs")]
pub(crate) fn environment_file_runtime() -> warp_files::EnvironmentFileRuntime {
    warp_files::EnvironmentFileRuntime::new(
        |host_id, session_id, path, content, ctx| {
            let client = client_for_session(session_id, ctx).ok_or_else(|| {
                warp_util::file::FileSaveError::RemoteError(format!(
                    "Environment session {session_id:?} for host {host_id} is not connected"
                ))
            })?;
            Ok(Box::pin(async move {
                client
                    .write_file(path, content)
                    .await
                    .map_err(|error| error.to_string())
            }))
        },
        |host_id, session_id, path, ctx| {
            let client = client_for_session(session_id, ctx).ok_or_else(|| {
                warp_util::file::FileSaveError::RemoteError(format!(
                    "Environment session {session_id:?} for host {host_id} is not connected"
                ))
            })?;
            Ok(Box::pin(async move {
                client
                    .delete_file(path)
                    .await
                    .map_err(|error| error.to_string())
            }))
        },
    )
}

pub(crate) fn host_id_for_session(session_id: SessionId, ctx: &AppContext) -> Option<HostId> {
    EnvironmentRuntimeTransportManager::as_ref(ctx)
        .host_id_for_session(session_id)
        .cloned()
}

pub(crate) fn is_session_potentially_active(session_id: SessionId, ctx: &AppContext) -> bool {
    EnvironmentRuntimeTransportManager::as_ref(ctx).is_session_potentially_active(session_id)
}

pub(crate) fn navigate_session_to_directory<T: View>(
    session_id: SessionId,
    cwd: String,
    ctx: &mut ViewContext<T>,
) {
    EnvironmentRuntimeTransportManager::handle(ctx).update(ctx, |manager, ctx| {
        manager.navigate_to_directory(session_id, cwd, ctx);
    });
}

#[cfg(feature = "local_fs")]
pub(crate) fn load_repo_metadata_directory_for_host<T: View>(
    host_id: &HostId,
    repo_root: StandardizedPath,
    dir_path: StandardizedPath,
    ctx: &mut ViewContext<T>,
) -> Result<(), String> {
    let session_id = {
        let manager = EnvironmentRuntimeTransportManager::as_ref(ctx);
        let sessions = manager
            .sessions_for_host(host_id)
            .ok_or_else(|| format!("no sessions for host {host_id}"))?;
        sessions
            .iter()
            .next()
            .copied()
            .ok_or_else(|| format!("no active sessions for host {host_id}"))?
    };
    EnvironmentRuntimeTransportManager::handle(ctx)
        .update(ctx, |manager, ctx| {
            manager.load_remote_repo_metadata_directory(session_id, repo_root, dir_path, ctx)
        })
        .map_err(|error_kind| format!("failed to schedule remote directory load: {error_kind:?}"))
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn check_binary<T: View>(
    session_id: SessionId,
    transport: EnvironmentRuntimeTransport,
    ctx: &mut ViewContext<T>,
) {
    EnvironmentRuntimeTransportManager::handle(ctx).update(ctx, |manager, ctx| {
        manager.check_binary(session_id, transport, ctx);
    });
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn install_binary<T: View>(
    session_id: SessionId,
    transport: EnvironmentRuntimeTransport,
    has_old_binary: bool,
    ctx: &mut ViewContext<T>,
) {
    EnvironmentRuntimeTransportManager::handle(ctx).update(ctx, |manager, ctx| {
        manager.install_binary(session_id, transport, has_old_binary, ctx);
    });
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn connect_transport<T: View>(
    session_id: SessionId,
    transport: EnvironmentRuntimeTransport,
    auth_context: Arc<EnvironmentRuntimeAuthContext>,
    ctx: &mut ViewContext<T>,
) {
    EnvironmentRuntimeTransportManager::handle(ctx).update(ctx, |manager, ctx| {
        manager.set_environment_owned_session(session_id, true);
        manager.connect_session(session_id, transport, auth_context, ctx);
    });
}

pub(crate) fn notify_bootstrapped_session<T: Entity>(
    session_id: SessionId,
    runtime_session_id: Option<SessionId>,
    shell_type_name: &str,
    shell_path: Option<&str>,
    execution_context: &crate::terminal::model::session::SessionExecutionContext,
    ctx: &mut ModelContext<T>,
) {
    EnvironmentRuntimeTransportManager::handle(ctx).update(ctx, |manager, manager_ctx| {
        let context_registered = manager.register_session_execution_context(
            session_id,
            runtime_session_id,
            shell_type_name,
            shell_path,
            execution_context.working_directory.as_deref(),
            &execution_context.environment_variables,
            manager_ctx,
        );
        if !context_registered {
            log::error!(
                "refusing incomplete terminal execution context for session {session_id:?}"
            );
        }
    });
}

pub(crate) fn deregister_session<T: View>(session_id: SessionId, ctx: &mut ViewContext<T>) {
    EnvironmentRuntimeTransportManager::handle(ctx).update(ctx, |manager, ctx| {
        manager.deregister_session(session_id, ctx);
    });
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn restart_session_transport<T: View>(session_id: SessionId, ctx: &mut ViewContext<T>) {
    EnvironmentRuntimeTransportManager::handle(ctx).update(ctx, |manager, ctx| {
        manager.restart_session_transport(session_id, ctx);
    });
}

pub(crate) fn allocate_environment_owned_session_id<T: View>(
    ctx: &mut ViewContext<T>,
) -> SessionId {
    EnvironmentRuntimeTransportManager::handle(ctx).update(ctx, |manager, _| {
        manager.allocate_environment_owned_session_id()
    })
}

pub(crate) fn deregister_terminal_session_if_unowned<T: View>(
    session_id: SessionId,
    ctx: &mut ViewContext<T>,
) {
    EnvironmentRuntimeTransportManager::handle(ctx).update(ctx, |manager, ctx| {
        if manager.is_environment_owned_session(session_id) {
            log::info!(
                "Skipping environment runtime deregistration for environment-owned session {session_id:?}"
            );
            return;
        }
        manager.deregister_session(session_id, ctx);
    });
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn check_session_binary<T: Entity>(
    session_id: SessionId,
    transport: EnvironmentRuntimeTransport,
    ctx: &mut ModelContext<T>,
) {
    EnvironmentRuntimeTransportManager::handle(ctx).update(ctx, |manager, ctx| {
        manager.check_binary(session_id, transport, ctx);
    });
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn install_session_binary<T: Entity>(
    session_id: SessionId,
    transport: EnvironmentRuntimeTransport,
    has_old_binary: bool,
    ctx: &mut ModelContext<T>,
) {
    EnvironmentRuntimeTransportManager::handle(ctx).update(ctx, |manager, ctx| {
        manager.install_binary(session_id, transport, has_old_binary, ctx);
    });
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn mark_session_setup_unsupported<T: Entity>(
    session_id: SessionId,
    reason: crate::environment_runtime_transport::setup::UnsupportedReason,
    ctx: &mut ModelContext<T>,
) {
    EnvironmentRuntimeTransportManager::handle(ctx).update(ctx, |manager, ctx| {
        manager.mark_setup_unsupported(session_id, reason, ctx);
    });
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn connect_session_transport<T: Entity>(
    session_id: SessionId,
    transport: EnvironmentRuntimeTransport,
    auth_context: Arc<EnvironmentRuntimeAuthContext>,
    ctx: &mut ModelContext<T>,
) {
    EnvironmentRuntimeTransportManager::handle(ctx).update(ctx, |manager, ctx| {
        manager.connect_session(session_id, transport, auth_context, ctx);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_read_transfer(total_size: u64) -> EnvironmentRuntimeReadTransfer {
        EnvironmentRuntimeReadTransfer {
            handle: crate::environment_runtime_transport::proto::FileTransferHandle {
                id: "read-test".to_owned(),
            },
            total_size,
            next_offset: 0,
        }
    }

    fn test_read_chunk(
        bytes: &[u8],
        next_offset: u64,
        total_size: u64,
        eof: bool,
    ) -> EnvironmentRuntimeFileChunk {
        EnvironmentRuntimeFileChunk {
            bytes: bytes.to_vec(),
            next_offset,
            total_size,
            eof,
        }
    }

    fn test_write_transfer() -> EnvironmentRuntimeWriteTransfer {
        EnvironmentRuntimeWriteTransfer {
            handle: crate::environment_runtime_transport::proto::FileTransferHandle {
                id: "write-test".to_owned(),
            },
            next_offset: 0,
        }
    }

    #[test]
    fn environment_read_cursor_requires_exact_progress_and_terminal_size() {
        let mut transfer = test_read_transfer(4);
        transfer
            .accept_chunk(&test_read_chunk(b"ab", 2, 4, false))
            .expect("exact non-terminal progress must be accepted");
        assert_eq!(transfer.next_offset(), 2);

        for invalid in [
            test_read_chunk(b"c", 4, 4, true),
            test_read_chunk(b"cd", 4, 5, true),
            test_read_chunk(b"c", 3, 4, true),
            test_read_chunk(b"cd", 4, 4, false),
            test_read_chunk(b"", 2, 4, false),
        ] {
            transfer
                .accept_chunk(&invalid)
                .expect_err("invalid cursor/size/EOF state must fail closed");
            assert_eq!(
                transfer.next_offset(),
                2,
                "a rejected chunk must not mutate the canonical cursor"
            );
        }

        transfer
            .accept_chunk(&test_read_chunk(b"cd", 4, 4, true))
            .expect("exact terminal progress must be accepted");
        assert_eq!(transfer.next_offset(), 4);

        let mut empty = test_read_transfer(0);
        empty
            .accept_chunk(&test_read_chunk(b"", 0, 0, true))
            .expect("zero-size transfer must terminate with an empty EOF chunk");
    }

    #[test]
    fn environment_write_cursor_requires_exact_progress() {
        let mut transfer = test_write_transfer();
        transfer
            .accept_chunk(2, &EnvironmentRuntimeWriteChunkSuccess { next_offset: 2 })
            .expect("exact write progress must be accepted");
        assert_eq!(transfer.next_offset(), 2);

        transfer
            .accept_chunk(2, &EnvironmentRuntimeWriteChunkSuccess { next_offset: 5 })
            .expect_err("write cursor jumps must fail closed");
        assert_eq!(
            transfer.next_offset(),
            2,
            "a rejected write acknowledgement must not mutate the canonical cursor"
        );

        transfer
            .accept_chunk(2, &EnvironmentRuntimeWriteChunkSuccess { next_offset: 4 })
            .expect("the next exact acknowledgement must still be accepted");
        assert_eq!(transfer.next_offset(), 4);
    }

    #[test]
    fn terminal_bootstrap_environment_display_has_label_like_runtime_environments() {
        let environment = terminal_bootstrap_environment(None);
        let display = environment_display_info_for_environment(&environment);

        assert_eq!(
            display.chip_label.as_deref(),
            Some(environment_kind_label(&EnvironmentKind::Local)),
            "local/current-app must have the same visible Environment identity shape as runtime-backed environments"
        );
    }

    #[test]
    fn environment_navigation_key_collapses_current_app_authority_aliases() {
        assert_eq!(
            ParsedEnvironmentAuthority::parse("local").navigation_key(),
            "local"
        );
        assert_eq!(
            ParsedEnvironmentAuthority::parse("local:/tmp/project").navigation_key(),
            "local"
        );
        assert_eq!(
            ParsedEnvironmentAuthority::parse("ssh:ssh-config:remote-fixture-primary")
                .navigation_key(),
            "ssh:ssh-config:remote-fixture-primary"
        );
    }

    #[test]
    fn environment_runtime_pty_advertises_terminal_capabilities_like_local_pty() {
        let environment_variables = terminal_capability_environment_variables();

        assert_eq!(
            environment_variables.get("TERM").map(String::as_str),
            Some("xterm-256color")
        );
        assert_eq!(
            environment_variables
                .get("TERM_PROGRAM")
                .map(String::as_str),
            Some("WarpTerminal")
        );
        assert_eq!(
            environment_variables.get("COLORTERM").map(String::as_str),
            Some("truecolor")
        );
        assert!(
            environment_variables.contains_key("WARP_CLIENT_VERSION"),
            "runtime PTYs must expose the same client-version marker as local PTYs"
        );
    }

    #[test]
    fn environment_runtime_root_probe_rejects_missing_home_snapshot() {
        let error = environment_runtime_roots_from_probe_stdout(b"/workspace/target\n".to_vec())
            .expect_err("target HOME is required and must never fall back to workspace root");
        assert!(error.contains("HOME"));
    }

    #[test]
    fn root_resolution_waits_for_bootstrapped_execution_carrier() {
        let owner = SessionId::from(7301);
        let unrelated = SessionId::from(7302);

        assert_eq!(
            environment_runtime_execution_carrier_gate(Some(owner), owner, false),
            EnvironmentRuntimeExecutionCarrierGate::WaitingForExecutionCarrier,
            "transport connection alone must not start a target root probe"
        );
        assert_eq!(
            environment_runtime_execution_carrier_gate(None, owner, true),
            EnvironmentRuntimeExecutionCarrierGate::MissingRuntimeOwner,
            "an execution context without a canonical runtime owner must remain inert"
        );
        assert_eq!(
            environment_runtime_execution_carrier_gate(Some(owner), owner, true),
            EnvironmentRuntimeExecutionCarrierGate::Ready,
            "the first validated owner execution context releases root materialization"
        );
        assert_eq!(
            environment_runtime_execution_carrier_gate(Some(owner), unrelated, true),
            EnvironmentRuntimeExecutionCarrierGate::StaleRuntimeOwner,
            "an unrelated terminal context must not release another runtime owner"
        );
    }

    #[test]
    fn environment_cli_agent_session_source_id_parses_colon_authority() {
        let id = environment_cli_agent_session_source_id(
            "ssh:ssh-config:remote-fixture-primary",
            &CLIAgent::Codex,
            "/root/.codex/sessions/session.jsonl",
        );
        let agent_name = CLIAgent::Codex.to_serialized_name();
        let target = environment_cli_agent_session_source_target_from_id(
            &id,
            Some(agent_name.as_str()),
            Some("codex-session".to_owned()),
        )
        .expect("environment session source id must be valid")
        .expect("environment session source ids must allow provider authorities containing ':'");

        assert_eq!(target.authority, "ssh:ssh-config:remote-fixture-primary");
        assert_eq!(target.source, "/root/.codex/sessions/session.jsonl");
        assert_eq!(target.agent, CLIAgent::Codex);
        assert_eq!(target.provider_session_id, "codex-session");
    }

    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn environment_cli_agent_scan_rejects_malformed_record() {
        use crate::environment_runtime_transport::proto::CliAgentSessionRecord;

        let records = vec![
            CliAgentSessionRecord {
                agent: "codex".to_owned(),
                id: "valid-session".to_owned(),
                source: "/root/.codex/sessions/valid.jsonl".to_owned(),
                label: None,
                cwd: None,
                modified_epoch_millis: Some(2),
            },
            CliAgentSessionRecord {
                agent: "codex".to_owned(),
                id: "malformed-session".to_owned(),
                source: String::new(),
                label: None,
                cwd: None,
                modified_epoch_millis: Some(1),
            },
        ];

        let error = decode_environment_cli_agent_scan_records(records)
            .expect_err("one malformed required record must reject the complete scan");
        assert!(error.contains("record 1"));
        assert!(error.contains("missing source"));
    }

    #[test]
    fn environment_cli_agent_store_roots_are_shared_by_scan_read_mutate_and_fork() {
        let roots = environment_cli_agent_store_roots_from_probe_stdout(
            b"/home/target\0/srv/claude-config\0/srv/codex-home\0".to_vec(),
        )
        .expect("parse target Environment roots");

        assert_eq!(roots.home_dir, PathBuf::from("/home/target"));
        assert_eq!(roots.claude_config_dir, PathBuf::from("/srv/claude-config"));
        assert_eq!(roots.codex_home, PathBuf::from("/srv/codex-home"));

        let proto = environment_cli_agent_store_roots_to_proto(&roots);
        assert_eq!(proto.home_dir, "/home/target");
        assert_eq!(proto.claude_config_dir, "/srv/claude-config");
        assert_eq!(proto.codex_home, "/srv/codex-home");
    }

    #[test]
    fn environment_cli_agent_store_root_probe_rejects_relative_or_missing_paths() {
        let relative = environment_cli_agent_store_roots_from_probe_stdout(
            b"/home/target\0relative-claude\0/srv/codex\0".to_vec(),
        )
        .expect_err("relative target roots must fail before RPC");
        assert!(relative.contains("claude_config_dir is not absolute"));

        let missing = environment_cli_agent_store_roots_from_probe_stdout(
            b"/home/target\0/srv/claude\0".to_vec(),
        )
        .expect_err("incomplete root snapshots must fail before RPC");
        assert!(missing.contains("codex_home"));
    }

    #[cfg(not(target_family = "wasm"))]
    #[tokio::test]
    async fn environment_cli_agent_roots_use_session_owned_execution_context() {
        use crate::terminal::model::session::command_executor::{
            ExecuteCommandOptions, LocalCommandExecutionContext, LocalCommandExecutor,
        };
        use crate::terminal::shell::ShellType;

        let cwd = tempfile::tempdir().expect("create target session cwd");
        let canonical_cwd = std::fs::canonicalize(cwd.path()).expect("canonical target cwd");
        let authoritative_names = [
            "ASHIDE_SESSION_EXECUTION_CONTEXT",
            "HOME",
            "PATH",
            "CODEX_HOME",
            "CLAUDE_CONFIG_DIR",
        ];
        let executor = LocalCommandExecutor::new(
            Some(PathBuf::from("/bin/bash")),
            ShellType::Bash,
            LocalCommandExecutionContext {
                working_directory: Some(cwd.path().to_path_buf()),
                environment_variables: HashMap::from([
                    (
                        "ASHIDE_SESSION_EXECUTION_CONTEXT".to_owned(),
                        "1".to_owned(),
                    ),
                    ("HOME".to_owned(), "relative-home".to_owned()),
                    ("PATH".to_owned(), "/target/bin".to_owned()),
                    ("CODEX_HOME".to_owned(), "relative-codex".to_owned()),
                    ("CLAUDE_CONFIG_DIR".to_owned(), "relative-claude".to_owned()),
                ]),
                authoritative_environment_variable_names: authoritative_names
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
            },
        );

        let output = executor
            .execute_local_command(
                environment_cli_agent_store_roots_probe_command(),
                None,
                None,
                ExecuteCommandOptions::default(),
            )
            .await
            .expect("target session root probe must execute");
        let roots = environment_cli_agent_store_roots_from_probe_stdout(output.stdout)
            .expect("target session root probe must return complete roots");

        assert_eq!(roots.home_dir, canonical_cwd.join("relative-home"));
        assert_eq!(
            roots.claude_config_dir,
            canonical_cwd.join("relative-claude")
        );
        assert_eq!(roots.codex_home, canonical_cwd.join("relative-codex"));
    }

    #[test]
    fn runtime_session_source_delete_treats_missing_source_as_success() {
        let error = "session source file not found";
        assert!(
            environment_file_missing_error(error),
            "delete should classify missing sources as already gone"
        );
    }

    #[test]
    fn workspace_root_candidate_rejects_current_app_leaks_for_runtime_authority() {
        let current_app_path = std::env::current_dir()
            .expect("test process should have a current directory")
            .to_string_lossy()
            .to_string();

        assert_eq!(
            workspace_root_candidate_for_authority("ssh:ssh-config:test", current_app_path.clone()),
            None,
            "native runtime roots must not accept current-app paths leaked from terminal metadata"
        );
        assert_eq!(
            workspace_root_candidate_for_authority(
                crate::environment_authority::TERMINAL_BOOTSTRAP_AUTHORITY,
                current_app_path.clone()
            ),
            Some(current_app_path),
            "terminal-bootstrap roots remain current-app paths"
        );
        if let Some(home) = std::env::var_os("HOME") {
            let home = home.to_string_lossy().into_owned();
            assert_eq!(
                workspace_root_candidate_for_authority("ssh:ssh-config:test", home),
                None,
                "runtime authority must reject current-app home leaked as cwd"
            );
        }
        assert_eq!(
            workspace_root_candidate_for_authority("ssh:ssh-config:test", "   ".to_owned()),
            None,
            "empty roots are still ignored"
        );
    }
}
