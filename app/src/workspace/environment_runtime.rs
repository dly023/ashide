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
use warpui::{AppContext, Entity, ModelContext, SingletonEntity, View, ViewContext};

use crate::app_state::{
    EnvironmentKind, EnvironmentLifecycleState, EnvironmentSnapshot, TabSnapshot,
};
use crate::auth::AuthStateProvider;
#[cfg(feature = "local_tty")]
pub(crate) use crate::environment_runtime_transport::setup::RemoteServerSetupState as EnvironmentRuntimeSetupState;
use crate::pane_group::{EnvironmentRuntimePtyProcess, NewTerminalOptions, PanesLayout};
use crate::terminal::available_shells::AvailableShell;
use crate::terminal::view::load_ai_conversation::ConversationRestorationInNewPaneType;
use crate::terminal::CLIAgent;

pub(crate) use crate::environment_runtime_transport::auth::RemoteServerAuthContext as EnvironmentRuntimeAuthContext;
pub(crate) use crate::environment_runtime_transport::auth_context::server_api_auth_context as environment_runtime_auth_context;
pub(crate) use crate::environment_runtime_transport::client::ClientError as EnvironmentRuntimeClientError;
pub(crate) use crate::environment_runtime_transport::client::RemoteServerClient as EnvironmentRuntimeClient;
pub(crate) use crate::environment_runtime_transport::manager::RemoteServerErrorKind as EnvironmentRuntimeErrorKind;
pub(crate) use crate::environment_runtime_transport::manager::RemoteServerInitPhase as EnvironmentRuntimeInitPhase;
pub(crate) use crate::environment_runtime_transport::manager::RemoteServerManager as EnvironmentRuntimeTransportManager;
#[cfg(feature = "local_fs")]
pub(crate) use crate::environment_runtime_transport::manager::RemoteServerManagerEvent as EnvironmentRuntimeTransportEvent;
pub(crate) use crate::environment_runtime_transport::manager::RemoteServerOperation as EnvironmentRuntimeOperation;
pub(crate) use crate::environment_runtime_transport::setup::PreinstallCheckResult as EnvironmentRuntimePreinstallCheckResult;
pub(crate) use crate::environment_runtime_transport::setup::PreinstallStatus as EnvironmentRuntimePreinstallStatus;
pub(crate) use crate::environment_runtime_transport::setup::RemotePlatform as EnvironmentRuntimePlatform;
#[cfg(not(target_family = "wasm"))]
pub(crate) use crate::environment_runtime_transport::ssh_transport::SshTransport as EnvironmentRuntimeTransport;

const TERMINAL_BOOTSTRAP_AUTHORITY: &str = "local";
const TERMINAL_BOOTSTRAP_AUTHORITY_PREFIX: &str = "local:";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EnvironmentRuntimeStatus {
    Dormant,
    Connecting,
    Installing,
    Connected,
    Error,
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
        Ok(crate::environment_runtime_transport::proto::FileSystemEntryKind::Other) | Err(_) => {
            EnvironmentRuntimeFileKind::Other
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct EnvironmentRuntimeResolvedPath {
    pub(crate) canonical_path: String,
    pub(crate) kind: EnvironmentRuntimeFileKind,
    pub(crate) size_bytes: Option<u64>,
}

#[derive(Clone, Debug)]
pub(crate) struct EnvironmentRuntimeDirectoryEntry {
    pub(crate) name: String,
    pub(crate) is_dir: bool,
    pub(crate) kind: EnvironmentRuntimeFileKind,
    pub(crate) size_bytes: Option<u64>,
    pub(crate) modified_epoch_millis: Option<u64>,
}

#[derive(Clone, Debug)]
pub(crate) struct EnvironmentRuntimeDirectoryListing {
    pub(crate) canonical_path: String,
    pub(crate) entries: Vec<EnvironmentRuntimeDirectoryEntry>,
}

#[derive(Clone, Debug)]
pub(crate) struct EnvironmentRuntimeFileChunk {
    pub(crate) bytes: Vec<u8>,
    pub(crate) next_offset: u64,
    pub(crate) eof: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct EnvironmentRuntimeWriteChunkSuccess {
    pub(crate) next_offset: u64,
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
    Empty,
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
        remote_platform: Option<EnvironmentRuntimePlatform>,
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
#[derive(Clone, Debug)]
pub(crate) struct EnvironmentRuntimePlatformInfo {
    pub(crate) environment_os: Option<String>,
    pub(crate) environment_arch: Option<String>,
}

#[cfg(feature = "local_tty")]
impl EnvironmentRuntimePlatformInfo {
    pub(crate) fn empty() -> Self {
        Self {
            environment_os: None,
            environment_arch: None,
        }
    }
}

#[cfg(feature = "local_tty")]
pub(crate) enum EnvironmentRuntimeTerminalEvent {
    SetupStateChanged {
        session_id: SessionId,
    },
    SessionConnected {
        session_id: SessionId,
        platform: EnvironmentRuntimePlatformInfo,
    },
    SessionConnectionFailed {
        session_id: SessionId,
        phase: EnvironmentRuntimeInitPhase,
        error: String,
        platform: EnvironmentRuntimePlatformInfo,
    },
    SessionDisconnected {
        session_id: SessionId,
        platform: EnvironmentRuntimePlatformInfo,
    },
    SessionDeregistered {
        session_id: SessionId,
    },
    BinaryInstallComplete {
        session_id: SessionId,
        result: Result<(), String>,
        platform: EnvironmentRuntimePlatformInfo,
    },
    BinaryCheckComplete {
        session_id: SessionId,
        result: Result<bool, String>,
        platform: EnvironmentRuntimePlatformInfo,
    },
    ClientRequestFailed {
        session_id: SessionId,
        operation: EnvironmentRuntimeOperation,
        error_kind: EnvironmentRuntimeErrorKind,
        platform: EnvironmentRuntimePlatformInfo,
    },
    ServerMessageDecodingError {
        session_id: SessionId,
        platform: EnvironmentRuntimePlatformInfo,
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

fn uses_terminal_bootstrap(environment: &EnvironmentSnapshot) -> bool {
    capabilities_for_environment(environment).uses_terminal_bootstrap
}

fn uses_environment_runtime(environment: &EnvironmentSnapshot) -> bool {
    capabilities_for_environment(environment).uses_runtime_entry
}

fn kind_uses_environment_runtime(kind: &EnvironmentKind) -> bool {
    capabilities_for_kind(kind).uses_runtime_entry
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

pub(crate) fn should_sync_environment_terminal_options(environment: &EnvironmentSnapshot) -> bool {
    uses_environment_runtime(environment)
}

pub(crate) fn environment_strip_dedupe_key(environment: &EnvironmentSnapshot) -> String {
    if uses_terminal_bootstrap(environment) {
        TERMINAL_BOOTSTRAP_AUTHORITY.to_owned()
    } else {
        environment.authority_key.clone()
    }
}

pub(crate) fn authority_uses_terminal_bootstrap(authority: &str) -> bool {
    authority == TERMINAL_BOOTSTRAP_AUTHORITY
        || authority.starts_with(TERMINAL_BOOTSTRAP_AUTHORITY_PREFIX)
}

pub(crate) fn workspace_root_candidate_for_authority(
    authority: &str,
    root: String,
) -> Option<String> {
    let trimmed = root.trim();
    if trimmed.is_empty() {
        return None;
    }
    if !authority_uses_terminal_bootstrap(authority)
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

pub(crate) fn session_authority_matches(
    session_authority: Option<&str>,
    current_authority: &str,
) -> bool {
    match session_authority {
        Some(authority) if authority_uses_terminal_bootstrap(authority) => {
            authority_uses_terminal_bootstrap(current_authority)
        }
        Some(authority) => authority == current_authority,
        None => authority_uses_terminal_bootstrap(current_authority),
    }
}

pub(crate) fn session_authority_or_terminal_bootstrap(session_authority: Option<&str>) -> &str {
    session_authority.unwrap_or(TERMINAL_BOOTSTRAP_AUTHORITY)
}

pub(crate) fn session_authority_uses_runtime_environment(session_authority: Option<&str>) -> bool {
    session_authority.is_some_and(|authority| !authority_uses_terminal_bootstrap(authority))
}

pub(crate) fn session_environment_display_label(authority: &str) -> Option<String> {
    let authority = authority.trim();
    if authority.is_empty() || authority_uses_terminal_bootstrap(authority) {
        return None;
    }
    Some(
        authority
            .strip_prefix("ssh:ssh-config:")
            .or_else(|| authority.strip_prefix("ssh:"))
            .unwrap_or(authority)
            .to_owned(),
    )
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
    if authority_key == TERMINAL_BOOTSTRAP_AUTHORITY {
        return Some(terminal_bootstrap_environment(None));
    }

    authority_key
        .strip_prefix(TERMINAL_BOOTSTRAP_AUTHORITY_PREFIX)
        .map(|root| terminal_bootstrap_environment(Some(root.to_owned())))
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
            EnvironmentLifecycleState::Reconnecting => {
                crate::t!("workspace-environment-tooltip-runtime-reconnecting")
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
        None
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
            canonical_path: success.canonical_path,
            kind: file_kind_from_proto(success.kind),
            size_bytes: success.size_bytes,
        }),
        Some(
            crate::environment_runtime_transport::proto::resolve_path_response::Result::Error(
                error,
            ),
        ) => Err(error.message),
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
            canonical_path: success.canonical_path,
            kind: file_kind_from_proto(success.kind),
            size_bytes: success.size_bytes,
        })),
        Some(
            crate::environment_runtime_transport::proto::resolve_path_response::Result::Error(_),
        )
        | None => Ok(None),
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
                    size_bytes: entry.size_bytes,
                    modified_epoch_millis: entry.modified_epoch_millis,
                })
                .collect();
            Ok(EnvironmentRuntimeDirectoryListing {
                canonical_path: success.canonical_path,
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
        )
        | None => Ok(()),
        Some(
            crate::environment_runtime_transport::proto::create_directory_response::Result::Error(
                error,
            ),
        ) => Err(error.message),
    }
}

pub(crate) async fn rename_file(
    client: &EnvironmentRuntimeClient,
    from: String,
    to: String,
) -> Result<(), String> {
    client
        .rename_file(from, to)
        .await
        .map_err(|error| error.to_string())
}

pub(crate) async fn write_file_chunk(
    client: &EnvironmentRuntimeClient,
    path: String,
    offset: u64,
    bytes: Vec<u8>,
    truncate: bool,
    executable: Option<bool>,
) -> Result<EnvironmentRuntimeWriteChunkSuccess, String> {
    let response = client
        .write_file_chunk(path, offset, bytes, truncate, executable)
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

pub(crate) async fn read_file_chunk(
    client: &EnvironmentRuntimeClient,
    path: String,
    offset: u64,
    max_bytes: u64,
) -> Result<EnvironmentRuntimeFileChunk, String> {
    let response = client
        .read_file_chunk(path, offset, max_bytes)
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
        None => EnvironmentRuntimePtyCreateResult::Empty,
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
        )
        | None => EnvironmentRuntimeSaveBufferResponse::Saved,
        Some(crate::environment_runtime_transport::proto::save_buffer_response::Result::Error(
            error,
        )) => EnvironmentRuntimeSaveBufferResponse::Failed(error.message),
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
        .unwrap_or_else(|| workspace_root.clone());
    Ok(EnvironmentRuntimeRoots {
        workspace_root,
        home_root,
    })
}

#[cfg(not(target_family = "wasm"))]
pub(crate) async fn run_environment_cli_agent_session_source_action(
    client: Arc<EnvironmentRuntimeClient>,
    _session_id: SessionId,
    target: EnvironmentCliAgentSessionSourceTarget,
    action: EnvironmentCliAgentSessionSourceAction,
) -> Result<(), String> {
    use crate::environment_runtime_transport::proto::CliAgentSessionMutation;

    let mutation = match action {
        EnvironmentCliAgentSessionSourceAction::Delete => CliAgentSessionMutation::Delete,
    };
    client
        .mutate_cli_agent_session(target.source.clone(), mutation as i32)
        .await
        .map_err(|error| format!("{error:#}"))
        .and_then(|response| match response.result {
            Some(
                crate::environment_runtime_transport::proto::mutate_cli_agent_session_response::Result::Success(
                    _,
                ),
            )
            | None => Ok(()),
            Some(
                crate::environment_runtime_transport::proto::mutate_cli_agent_session_response::Result::Error(
                    error,
                ),
            ) => Err(format!(
                "environment session source action failed: {}",
                error.message
            )),
        })
}

#[derive(Clone, Debug)]
pub(crate) struct EnvironmentCliAgentSessionSourceBytes {
    pub(crate) reference: String,
    pub(crate) sha256: String,
    pub(crate) bytes: Vec<u8>,
}

#[cfg(not(target_family = "wasm"))]
pub(crate) async fn read_environment_cli_agent_session_source(
    client: Arc<EnvironmentRuntimeClient>,
    _session_id: SessionId,
    _home_root: String,
    target: EnvironmentCliAgentSessionSourceTarget,
) -> Result<EnvironmentCliAgentSessionSourceBytes, String> {
    let response = client
        .read_cli_agent_session(target.source.clone(), target.provider_session_id.clone())
        .await
        .map_err(|error| format!("{error:#}"))?;
    match response.result {
        Some(
            crate::environment_runtime_transport::proto::read_cli_agent_session_response::Result::Success(
                success,
            ),
        ) => Ok(EnvironmentCliAgentSessionSourceBytes {
            reference: success.reference,
            sha256: success.sha256,
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

    let mut offset = 0;
    let mut bytes = Vec::new();
    loop {
        if offset > MAX_BYTES {
            return Err(format!(
                "refusing to read oversized environment file: {path}"
            ));
        }
        let chunk = read_file_chunk(client, path.clone(), offset, CHUNK_BYTES).await?;
        bytes.extend(chunk.bytes);
        if chunk.eof {
            return Ok(bytes);
        }
        if chunk.next_offset <= offset {
            return Err(format!(
                "environment file read made no progress at offset {offset}: {path}"
            ));
        }
        offset = chunk.next_offset;
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
    if contents.is_empty() {
        write_file_chunk(client, path, 0, Vec::new(), true, None).await?;
        return Ok(());
    }

    let mut offset = 0u64;
    for (index, chunk) in contents.chunks(CHUNK_BYTES).enumerate() {
        let success = write_file_chunk(
            client,
            path.clone(),
            offset,
            chunk.to_vec(),
            index == 0,
            None,
        )
        .await?;
        if success.next_offset <= offset {
            return Err(format!(
                "environment file write made no progress at offset {offset}: {path}"
            ));
        }
        offset = success.next_offset;
    }
    Ok(())
}

pub(crate) async fn append_environment_file(
    client: &EnvironmentRuntimeClient,
    path: String,
    contents: Vec<u8>,
) -> Result<(), String> {
    let mut existing = match read_environment_file_all(client, path.clone()).await {
        Ok(bytes) => bytes,
        Err(error) if environment_file_missing_error(&error) => Vec::new(),
        Err(error) => return Err(error),
    };
    existing.extend(contents);
    write_environment_file_all(client, path, existing).await
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

#[derive(Clone, Debug)]
pub(crate) struct EnvironmentCliAgentSessionSourceTarget {
    pub(crate) source: String,
    pub(crate) agent: Option<CLIAgent>,
    pub(crate) provider_session_id: Option<String>,
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
) -> Option<EnvironmentCliAgentSessionSourceTarget> {
    let encoded_payload = session_id.strip_prefix("remote:")?;
    let mut parts = encoded_payload.rsplitn(3, ':');
    let hex_source = parts.next()?;
    let encoded_agent = parts.next()?;
    let _authority = parts.next()?;
    let source = hex_decode_session_id_component(hex_source)?;
    let agent = cli_agent
        .map(CLIAgent::from_serialized_name)
        .filter(|agent| !matches!(agent, CLIAgent::Unknown))
        .or_else(|| {
            let agent = CLIAgent::from_serialized_name(encoded_agent);
            (!matches!(agent, CLIAgent::Unknown)).then_some(agent)
        });
    Some(EnvironmentCliAgentSessionSourceTarget {
        source,
        agent,
        provider_session_id,
    })
}

#[cfg(not(target_family = "wasm"))]
pub(crate) async fn scan_environment_cli_agent_sessions(
    client: Arc<EnvironmentRuntimeClient>,
    _session_id: SessionId,
) -> Result<Vec<EnvironmentCliAgentSessionRecord>, String> {
    const SCAN_LIMIT: u32 = 40;

    let response = client
        .scan_cli_agent_sessions(SCAN_LIMIT)
        .await
        .map_err(|error| format!("{error:#}"))?;

    let records = match response.result {
        Some(
            crate::environment_runtime_transport::proto::scan_cli_agent_sessions_response::Result::Success(
                success,
            ),
        ) => success.records,
        Some(
            crate::environment_runtime_transport::proto::scan_cli_agent_sessions_response::Result::Error(
                error,
            ),
        ) => return Err(format!("environment session scan failed: {}", error.message)),
        None => return Err("environment session scan returned no result".to_owned()),
    };

    let mut records = records
        .into_iter()
        .filter_map(|record| {
            let agent = match record.agent.as_str() {
                "claude" => CLIAgent::Claude,
                "codex" => CLIAgent::Codex,
                name => CLIAgent::from_serialized_name(name),
            };
            if matches!(agent, CLIAgent::Unknown) {
                return None;
            }
            let provider_session_id = record.id.trim();
            if provider_session_id.is_empty() {
                return None;
            }
            let source = if record.source.trim().is_empty() {
                provider_session_id.to_owned()
            } else {
                record.source.clone()
            };
            Some(EnvironmentCliAgentSessionRecord {
                agent,
                provider_session_id: provider_session_id.to_owned(),
                source,
                label: record
                    .label
                    .filter(|label| !label.trim().is_empty()),
                cwd: record.cwd.filter(|cwd| !cwd.trim().is_empty()),
                modified_ms: record.modified_epoch_millis,
            })
        })
        .collect::<Vec<_>>();
    records.sort_by(|left, right| left.modified_ms.cmp(&right.modified_ms));
    Ok(records)
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
pub(crate) struct EnvironmentRuntime {
    pub(crate) environment: EnvironmentSnapshot,
    pub(crate) status: EnvironmentRuntimeStatus,
    pub(crate) synthetic_session_id: Option<SessionId>,
    pub(crate) host_id: Option<HostId>,
    pub(crate) control_path: Option<PathBuf>,
    pub(crate) last_error: Option<String>,
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
        environment_variables: HashMap::new(),
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

pub(crate) fn terminal_bootstrap_options_for_spawn(
    spawn: TerminalBootstrapSpawn,
    shell: Option<AvailableShell>,
    conversation_restoration: Option<ConversationRestorationInNewPaneType>,
    hide_homepage: bool,
) -> NewTerminalOptions {
    terminal_bootstrap_options(
        spawn.initial_directory,
        shell,
        conversation_restoration,
        hide_homepage,
    )
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
        EnvironmentRuntimeTransportEvent::RepoMetadataUpdated { host_id, update }
        | EnvironmentRuntimeTransportEvent::RepoMetadataDirectoryLoaded { host_id, update } => {
            model.apply_remote_incremental_update(host_id, update, ctx);
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
            | EnvironmentRuntimeTransportEvent::SessionConnectionFailed { .. }
            | EnvironmentRuntimeTransportEvent::HostConnected { .. }
            | EnvironmentRuntimeTransportEvent::HostDisconnected { .. }
            | EnvironmentRuntimeTransportEvent::NavigatedToDirectory { .. }
            | EnvironmentRuntimeTransportEvent::RepoMetadataSnapshot { .. }
            | EnvironmentRuntimeTransportEvent::RepoMetadataUpdated { .. }
            | EnvironmentRuntimeTransportEvent::RepoMetadataDirectoryLoaded { .. }
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
            | EnvironmentRuntimeTransportEvent::HostConnected { .. }
            | EnvironmentRuntimeTransportEvent::HostDisconnected { .. }
            | EnvironmentRuntimeTransportEvent::NavigatedToDirectory { .. }
            | EnvironmentRuntimeTransportEvent::RepoMetadataSnapshot { .. }
            | EnvironmentRuntimeTransportEvent::RepoMetadataUpdated { .. }
            | EnvironmentRuntimeTransportEvent::RepoMetadataDirectoryLoaded { .. }
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
                remote_platform,
                preinstall_check,
                has_old_binary,
            } => EnvironmentRuntimeSetupEvent::BinaryCheckComplete {
                session_id: *session_id,
                result: result.clone(),
                remote_platform: remote_platform.clone(),
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
            | EnvironmentRuntimeTransportEvent::HostConnected { .. }
            | EnvironmentRuntimeTransportEvent::HostDisconnected { .. }
            | EnvironmentRuntimeTransportEvent::NavigatedToDirectory { .. }
            | EnvironmentRuntimeTransportEvent::RepoMetadataSnapshot { .. }
            | EnvironmentRuntimeTransportEvent::RepoMetadataUpdated { .. }
            | EnvironmentRuntimeTransportEvent::RepoMetadataDirectoryLoaded { .. }
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
fn platform_info_from_platform(
    platform: Option<&EnvironmentRuntimePlatform>,
) -> EnvironmentRuntimePlatformInfo {
    platform
        .map(|platform| EnvironmentRuntimePlatformInfo {
            environment_os: Some(platform.os.as_str().to_owned()),
            environment_arch: Some(platform.arch.as_str().to_owned()),
        })
        .unwrap_or_else(EnvironmentRuntimePlatformInfo::empty)
}

#[cfg(feature = "local_tty")]
fn platform_info_for_session<T: View>(
    session_id: SessionId,
    ctx: &ViewContext<T>,
) -> EnvironmentRuntimePlatformInfo {
    let manager = EnvironmentRuntimeTransportManager::handle(ctx);
    platform_info_from_platform(manager.as_ref(ctx).platform_for_session(session_id))
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
                    platform: platform_info_for_session(*session_id, ctx),
                }
            }
            EnvironmentRuntimeTransportEvent::SessionConnectionFailed {
                session_id,
                phase,
                error,
            } => EnvironmentRuntimeTerminalEvent::SessionConnectionFailed {
                session_id: *session_id,
                phase: *phase,
                error: error.clone(),
                platform: platform_info_for_session(*session_id, ctx),
            },
            EnvironmentRuntimeTransportEvent::SessionDisconnected { session_id, .. } => {
                EnvironmentRuntimeTerminalEvent::SessionDisconnected {
                    session_id: *session_id,
                    platform: platform_info_for_session(*session_id, ctx),
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
                    platform: platform_info_for_session(*session_id, ctx),
                }
            }
            EnvironmentRuntimeTransportEvent::BinaryCheckComplete {
                session_id,
                result,
                remote_platform,
                ..
            } => EnvironmentRuntimeTerminalEvent::BinaryCheckComplete {
                session_id: *session_id,
                result: result.clone(),
                platform: platform_info_from_platform(remote_platform.as_ref()),
            },
            EnvironmentRuntimeTransportEvent::ClientRequestFailed {
                session_id,
                operation,
                error_kind,
            } => EnvironmentRuntimeTerminalEvent::ClientRequestFailed {
                session_id: *session_id,
                operation: *operation,
                error_kind: *error_kind,
                platform: platform_info_for_session(*session_id, ctx),
            },
            EnvironmentRuntimeTransportEvent::ServerMessageDecodingError { session_id } => {
                EnvironmentRuntimeTerminalEvent::ServerMessageDecodingError {
                    session_id: *session_id,
                    platform: platform_info_for_session(*session_id, ctx),
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
            | EnvironmentRuntimeTransportEvent::HostConnected { .. }
            | EnvironmentRuntimeTransportEvent::HostDisconnected { .. }
            | EnvironmentRuntimeTransportEvent::RepoMetadataSnapshot { .. }
            | EnvironmentRuntimeTransportEvent::RepoMetadataUpdated { .. }
            | EnvironmentRuntimeTransportEvent::RepoMetadataDirectoryLoaded { .. }
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

pub(crate) fn client_for_host(
    host_id: &HostId,
    ctx: &AppContext,
) -> Option<Arc<EnvironmentRuntimeClient>> {
    EnvironmentRuntimeTransportManager::as_ref(ctx)
        .client_for_host(host_id)
        .cloned()
}

#[cfg(feature = "local_fs")]
pub(crate) fn environment_file_runtime() -> warp_files::EnvironmentFileRuntime {
    warp_files::EnvironmentFileRuntime::new(
        |host_id, path, content, ctx| {
            let client = client_for_host(host_id, ctx).ok_or_else(|| {
                warp_util::file::FileSaveError::RemoteError(format!(
                    "Environment host {host_id} is not connected"
                ))
            })?;
            Ok(Box::pin(async move {
                client
                    .write_file(path, content)
                    .await
                    .map_err(|error| error.to_string())
            }))
        },
        |host_id, path, ctx| {
            let client = client_for_host(host_id, ctx).ok_or_else(|| {
                warp_util::file::FileSaveError::RemoteError(format!(
                    "Environment host {host_id} is not connected"
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
    repo_root: String,
    dir_path: String,
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
    EnvironmentRuntimeTransportManager::handle(ctx).update(ctx, |manager, ctx| {
        manager.load_remote_repo_metadata_directory(session_id, repo_root, dir_path, ctx);
    });
    Ok(())
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

pub(crate) fn notify_session_bootstrapped<T: View>(
    session_id: SessionId,
    ctx: &mut ViewContext<T>,
) {
    EnvironmentRuntimeTransportManager::handle(ctx).update(ctx, |manager, _| {
        // Environment Runtime owns a control session that is not backed by a
        // visible terminal bootstrap DCS. Register a conservative shell
        // executor so runtime-level commands (agent scan, cwd/root probes,
        // future file/project operations) go through the connected runtime
        // instead of opening per-operation transport wrapper commands.
        manager.notify_session_bootstrapped(session_id, "bash", None);
    });
}

pub(crate) fn notify_bootstrapped_session<T: Entity>(
    session_id: SessionId,
    shell_type_name: &str,
    shell_path: Option<&str>,
    ctx: &mut ModelContext<T>,
) {
    EnvironmentRuntimeTransportManager::handle(ctx).update(ctx, |manager, _| {
        manager.notify_session_bootstrapped(session_id, shell_type_name, shell_path);
    });
}

pub(crate) fn register_terminal_bootstrap_session_alias<T: Entity>(
    terminal_session_id: SessionId,
    runtime_session_id: SessionId,
    ctx: &mut ModelContext<T>,
) {
    EnvironmentRuntimeTransportManager::handle(ctx).update(ctx, |manager, _| {
        manager.register_session_alias(terminal_session_id, runtime_session_id);
    });
}

pub(crate) fn deregister_session<T: View>(session_id: SessionId, ctx: &mut ViewContext<T>) {
    EnvironmentRuntimeTransportManager::handle(ctx).update(ctx, |manager, ctx| {
        manager.deregister_session(session_id, ctx);
    });
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

impl EnvironmentRuntime {
    fn dormant(environment: EnvironmentSnapshot) -> Self {
        Self {
            environment,
            status: EnvironmentRuntimeStatus::Dormant,
            synthetic_session_id: None,
            host_id: None,
            control_path: None,
            last_error: None,
        }
    }
}

#[derive(Default)]
pub(crate) struct EnvironmentRuntimeRegistry {
    runtimes: HashMap<String, EnvironmentRuntime>,
    session_to_authority: HashMap<SessionId, String>,
}

impl EnvironmentRuntimeRegistry {
    pub(crate) fn upsert_environment(&mut self, environment: EnvironmentSnapshot) {
        let authority = environment.authority_key.clone();
        self.runtimes
            .entry(authority)
            .and_modify(|runtime| runtime.environment = environment.clone())
            .or_insert_with(|| EnvironmentRuntime::dormant(environment));
    }

    pub(crate) fn mark_connecting(
        &mut self,
        mut environment: EnvironmentSnapshot,
        session_id: SessionId,
        control_path: PathBuf,
    ) {
        environment.lifecycle_state = EnvironmentLifecycleState::Connecting;
        let authority = environment.authority_key.clone();
        if let Some(previous_session_id) = self
            .runtimes
            .get(&authority)
            .and_then(|runtime| runtime.synthetic_session_id)
        {
            self.session_to_authority.remove(&previous_session_id);
        }
        self.session_to_authority
            .insert(session_id, authority.clone());
        self.runtimes.insert(
            authority,
            EnvironmentRuntime {
                environment,
                status: EnvironmentRuntimeStatus::Connecting,
                synthetic_session_id: Some(session_id),
                host_id: None,
                control_path: Some(control_path),
                last_error: None,
            },
        );
    }

    pub(crate) fn mark_installing_session(&mut self, session_id: SessionId) -> Option<String> {
        let authority = self.current_authority_for_session(session_id)?.to_owned();
        let runtime = self.runtimes.get_mut(&authority)?;
        runtime.status = EnvironmentRuntimeStatus::Installing;
        runtime.environment.lifecycle_state = EnvironmentLifecycleState::Installing;
        self.session_to_authority
            .insert(session_id, authority.clone());
        Some(authority)
    }

    pub(crate) fn mark_connected_session(
        &mut self,
        session_id: SessionId,
        host_id: HostId,
    ) -> Option<String> {
        let authority = self.current_authority_for_session(session_id)?.to_owned();
        let runtime = self.runtimes.get_mut(&authority)?;
        runtime.status = EnvironmentRuntimeStatus::Connected;
        runtime.host_id = Some(host_id);
        runtime.last_error = None;
        runtime.environment.lifecycle_state = EnvironmentLifecycleState::Connected;
        self.session_to_authority
            .insert(session_id, authority.clone());
        Some(authority)
    }

    pub(crate) fn mark_error_for_session(
        &mut self,
        session_id: SessionId,
        error: String,
    ) -> Option<String> {
        let authority = self.current_authority_for_session(session_id)?.to_owned();
        self.mark_error_for_authority(&authority, error);
        self.session_to_authority
            .insert(session_id, authority.clone());
        Some(authority)
    }

    pub(crate) fn mark_error_for_authority(&mut self, authority: &str, error: String) {
        if let Some(runtime) = self.runtimes.get_mut(authority) {
            runtime.status = EnvironmentRuntimeStatus::Error;
            runtime.last_error = Some(error);
            runtime.environment.lifecycle_state = EnvironmentLifecycleState::Error;
        }
    }

    pub(crate) fn authority_for_session(&self, session_id: SessionId) -> Option<&str> {
        self.session_to_authority
            .get(&session_id)
            .map(String::as_str)
    }

    pub(crate) fn authority_for_session_or_synthetic(&self, session_id: SessionId) -> Option<&str> {
        self.authority_for_session(session_id).or_else(|| {
            self.runtimes
                .iter()
                .find_map(|(authority, runtime)| {
                    (runtime.synthetic_session_id == Some(session_id)).then_some(authority)
                })
                .map(String::as_str)
        })
    }

    pub(crate) fn current_authority_for_session(&self, session_id: SessionId) -> Option<&str> {
        let authority = self.authority_for_session_or_synthetic(session_id)?;
        let runtime = self.runtimes.get(authority)?;
        (runtime.synthetic_session_id == Some(session_id)).then_some(authority)
    }

    pub(crate) fn runtime_for_authority(&self, authority: &str) -> Option<&EnvironmentRuntime> {
        self.runtimes.get(authority)
    }

    pub(crate) fn snapshot_for_authority(&self, authority: &str) -> Option<EnvironmentSnapshot> {
        self.runtimes
            .get(authority)
            .map(|runtime| runtime.environment.clone())
    }

    pub(crate) fn environment_snapshots(&self) -> Vec<EnvironmentSnapshot> {
        let mut environments = self
            .runtimes
            .values()
            .map(|runtime| runtime.environment.clone())
            .collect::<Vec<_>>();
        environments.sort_by(|left, right| {
            left.label
                .to_lowercase()
                .cmp(&right.label.to_lowercase())
                .then_with(|| left.authority_key.cmp(&right.authority_key))
        });
        environments
    }

    pub(crate) fn session_for_authority(&self, authority: &str) -> Option<SessionId> {
        self.runtimes.get(authority)?.synthetic_session_id
    }

    pub(crate) fn has_bootstrap_session(&self, authority: &str) -> bool {
        let Some(runtime) = self.runtimes.get(authority) else {
            return false;
        };
        match runtime.status {
            EnvironmentRuntimeStatus::Connecting | EnvironmentRuntimeStatus::Installing => {
                runtime.synthetic_session_id.is_some()
            }
            EnvironmentRuntimeStatus::Dormant
            | EnvironmentRuntimeStatus::Connected
            | EnvironmentRuntimeStatus::Error => false,
        }
    }

    pub(crate) fn connected_session_for_authority(
        &self,
        authority: &str,
    ) -> Option<(SessionId, HostId)> {
        let runtime = self.runtimes.get(authority)?;
        if runtime.status != EnvironmentRuntimeStatus::Connected {
            return None;
        }
        Some((runtime.synthetic_session_id?, runtime.host_id.clone()?))
    }

    pub(crate) fn connected_target_for_authority(
        &self,
        authority: &str,
    ) -> Option<EnvironmentRuntimeTarget> {
        let runtime = self.runtimes.get(authority)?;
        if runtime.status != EnvironmentRuntimeStatus::Connected {
            return None;
        }
        Some(EnvironmentRuntimeTarget {
            authority: authority.to_owned(),
            session_id: runtime.synthetic_session_id?,
            host_id: runtime.host_id.clone()?,
            root: runtime.environment.active_workspace_root.clone(),
        })
    }

    pub(crate) fn connected_session_for_host(
        &self,
        host_id: &HostId,
    ) -> Option<(String, SessionId)> {
        self.runtimes.iter().find_map(|(authority, runtime)| {
            if runtime.status == EnvironmentRuntimeStatus::Connected
                && runtime.host_id.as_ref() == Some(host_id)
            {
                Some((authority.clone(), runtime.synthetic_session_id?))
            } else {
                None
            }
        })
    }

    pub(crate) fn remove_authority(&mut self, authority: &str) -> Option<EnvironmentRuntime> {
        let runtime = self.runtimes.remove(authority)?;
        self.session_to_authority
            .retain(|_, session_authority| session_authority != authority);
        Some(runtime)
    }

    pub(crate) fn control_path_for_session(&self, session_id: SessionId) -> Option<PathBuf> {
        let authority = self.current_authority_for_session(session_id)?;
        self.runtimes.get(authority)?.control_path.clone()
    }

    pub(crate) fn lifecycle_for_authority(
        &self,
        authority: &str,
    ) -> Option<EnvironmentLifecycleState> {
        self.runtimes
            .get(authority)
            .map(|runtime| runtime.status.lifecycle_state())
    }

    pub(crate) fn terminal_spawn_for_target(
        &self,
        target: EnvironmentRuntimeTarget,
        root: impl Into<String>,
    ) -> EnvironmentRuntimeTerminalSpawn {
        EnvironmentRuntimeTerminalSpawn {
            target,
            root: root.into(),
        }
    }

    pub(crate) fn terminal_bootstrap_target_for_environment(
        &self,
        environment: &EnvironmentSnapshot,
    ) -> Option<TerminalBootstrapTarget> {
        if !uses_terminal_bootstrap(environment) {
            return None;
        }
        Some(TerminalBootstrapTarget {
            authority: environment.authority_key.clone(),
            root: environment.active_workspace_root.clone(),
        })
    }

    pub(crate) fn terminal_bootstrap_spawn_for_target(
        &self,
        target: TerminalBootstrapTarget,
    ) -> TerminalBootstrapSpawn {
        let initial_directory = target
            .root
            .as_ref()
            .filter(|root| !root.trim().is_empty())
            .map(PathBuf::from);
        TerminalBootstrapSpawn {
            target,
            initial_directory,
        }
    }

    pub(crate) fn spawn_plan_for_environment(
        &self,
        environment: &EnvironmentSnapshot,
    ) -> EnvironmentRuntimeSpawnPlan {
        if let Some(target) = self.terminal_bootstrap_target_for_environment(environment) {
            return EnvironmentRuntimeSpawnPlan::TerminalBootstrap(target);
        }

        self.connected_target_for_authority(&environment.authority_key)
            .map(|mut target| {
                target.root = environment.active_workspace_root.clone();
                EnvironmentRuntimeSpawnPlan::RuntimeTarget(target)
            })
            .unwrap_or(EnvironmentRuntimeSpawnPlan::RuntimeBootstrap)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use warp_core::{HostId, SessionId};

    use super::*;

    fn ssh_environment(authority_key: &str) -> EnvironmentSnapshot {
        runtime_transport_error_snapshot(
            authority_key.to_owned(),
            authority_key.to_owned(),
            Some(authority_key.to_owned()),
            Some("/".to_owned()),
        )
    }

    #[test]
    fn environment_cli_agent_session_source_id_parses_colon_authority() {
        let id = environment_cli_agent_session_source_id(
            "ssh:ssh-config:dnyx216",
            &CLIAgent::Codex,
            "/root/.codex/sessions/session.jsonl",
        );
        let agent_name = CLIAgent::Codex.to_serialized_name();
        let target = environment_cli_agent_session_source_target_from_id(
            &id,
            Some(agent_name.as_str()),
            Some("codex-session".to_owned()),
        )
        .expect("environment session source ids must allow provider authorities containing ':'");

        assert_eq!(target.source, "/root/.codex/sessions/session.jsonl");
        assert_eq!(target.agent, Some(CLIAgent::Codex));
        assert_eq!(target.provider_session_id, Some("codex-session".to_owned()));
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
                TERMINAL_BOOTSTRAP_AUTHORITY,
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

    #[test]
    fn registry_rejects_stale_session_transitions_after_reconnect() {
        let mut registry = EnvironmentRuntimeRegistry::default();
        let first_session = SessionId::from(1);
        let second_session = SessionId::from(2);

        registry.mark_connecting(
            ssh_environment("ssh:example"),
            first_session,
            PathBuf::from("/tmp/first.sock"),
        );
        registry.mark_connecting(
            ssh_environment("ssh:example"),
            second_session,
            PathBuf::from("/tmp/second.sock"),
        );

        assert_eq!(
            registry.authority_for_session(first_session),
            None,
            "reconnect should remove the previous session mapping for this authority"
        );
        assert_eq!(
            registry.current_authority_for_session(first_session),
            None,
            "old bootstrap events must not be able to mutate the replacement runtime"
        );
        assert_eq!(
            registry.control_path_for_session(first_session),
            None,
            "old bootstrap events must not reuse the replacement runtime socket"
        );
        assert_eq!(
            registry.mark_installing_session(first_session),
            None,
            "old binary-check completion must not move the new runtime to installing"
        );
        assert_eq!(
            registry.mark_connected_session(first_session, HostId::new("old-host".to_owned())),
            None,
            "old connected event must not mark the new runtime connected"
        );
        assert_eq!(
            registry.mark_error_for_session(first_session, "old failure".to_owned()),
            None,
            "old failure must not put the new runtime in error state"
        );

        assert_eq!(
            registry.current_authority_for_session(second_session),
            Some("ssh:example")
        );
        assert_eq!(
            registry.control_path_for_session(second_session),
            Some(PathBuf::from("/tmp/second.sock"))
        );
        assert_eq!(
            registry.lifecycle_for_authority("ssh:example"),
            Some(EnvironmentLifecycleState::Connecting)
        );

        registry.remove_authority("ssh:example");
        assert_eq!(
            registry.authority_for_session(second_session),
            None,
            "disconnect should remove all session mappings for this authority"
        );
    }
}
