//! Environment backend trait — the behavior-layer unification of local
//! (terminal-bootstrap) and remote (runtime) environments.
//!
//! # Architecture
//!
//! All user-facing "open tab" actions go through a single dispatch path:
//!
//! ```text
//! capability fn              ← only builds AgentTabEntry, no if/else
//!      │
//!      ▼
//! backend.deliver_entry(intent)      ← single dispatch point
//!      │
//!      ├── TerminalBootstrapEnvironmentBackend  → create terminal + apply_agent_tab_entry_immediately(entry)
//!      └── RuntimeEnvironmentBackend → queue AgentTabEntry → materialize → apply
//! ```
//!
//! The data layer (`EnvironmentSnapshot` / `authority`) was already unified.
//! This module unifies the behavior layer: entry delivery, indexed session
//! refresh, and alias/pin persistence all dispatch through the same backend.
//! The local/remote fork is isolated to the two backend impl bodies.
//!
//! **Invariants after B+:**
//! - Capability functions contain zero `if runtime { } else { }` branches.
//! - `AgentTabEntry` field omission = compile error (no silent behavioral drift).
//! - `apply_agent_tab_entry_immediately` is the single source of truth for all
//!   agent-tab side effects; remote defers to the same logic via bootstrap.

use std::path::Path;

use warpui::{EntityId, ViewContext};

use crate::ai::agent::conversation::AIConversation;
use crate::ai::blocklist::agent_view::AgentViewEntryOrigin;
use crate::app_state::PaneSessionBinding;
use crate::app_state::{EnvironmentSnapshot, WorkspaceSessionSnapshot};
use crate::environment_authority::ParsedEnvironmentAuthority;
use crate::terminal::view::inline_banner::ZeroStatePromptSuggestionType;
use crate::workspace::environment_runtime::{
    EnvironmentCliAgentSessionUserState, EnvironmentCliAgentSessionUserStateMutation,
};
use crate::workspace::view::Workspace;

// ---------------------------------------------------------------------------
// Shared entry types
// ---------------------------------------------------------------------------

/// Unified parameter bag for all "open agent tab" capabilities.
///
/// Both `TerminalBootstrapEnvironmentBackend::deliver_entry` and
/// `RuntimeEnvironmentBackend::deliver_entry` receive this struct. The local impl
/// applies all side effects immediately; the runtime impl queues the entry and
/// applies the same effects after the terminal bootstraps. Using one struct
/// means a missing field is a compile error — not a silent behavioral gap.
#[derive(Clone)]
pub(crate) struct AgentTabEntry {
    pub(crate) initial_prompt: Option<String>,
    pub(crate) origin: AgentViewEntryOrigin,
    pub(crate) codex_model_id: Option<String>,
    /// Open the code-review pane after entering agent view.
    pub(crate) open_code_review_pane: bool,
    /// Fallback conversation title set immediately after entering agent view
    /// (e.g. Linear deeplinks set "Linear Issue").
    pub(crate) fallback_display_title: Option<String>,
    /// Zero-state prompt suggestion inserted into the input after entering
    /// agent view. Previously only applied on the local path; now carried so
    /// the runtime path delivers the same UX after bootstrap.
    pub(crate) zero_state_prompt_suggestion_type: Option<ZeroStatePromptSuggestionType>,
    /// Restore the pane-group left panel open state after the terminal is created.
    pub(crate) restore_left_panel_open: bool,
}

impl AgentTabEntry {
    pub(crate) fn new(origin: AgentViewEntryOrigin) -> Self {
        Self {
            initial_prompt: None,
            origin,
            codex_model_id: None,
            open_code_review_pane: false,
            fallback_display_title: None,
            zero_state_prompt_suggestion_type: None,
            restore_left_panel_open: false,
        }
    }
}

/// Unified parameter bag for fork-to-new-tab. Carries everything both the local
/// and runtime delivery paths need to restore the forked conversation, copy
/// model/profile from the source, and run the summarize/initial prompts. Both
/// `TerminalBootstrapEnvironmentBackend::deliver_entry` and `RuntimeEnvironmentBackend::deliver_entry`
/// receive this struct; the runtime impl queues it and replays the same effects
/// after the terminal bootstraps.
#[derive(Clone)]
pub(crate) struct ForkEntry {
    pub(crate) conversation: AIConversation,
    pub(crate) source_terminal_view_id: Option<EntityId>,
    pub(crate) summarize_after_fork: bool,
    pub(crate) summarization_prompt: Option<String>,
    pub(crate) initial_prompt: Option<String>,
}

/// Unified parameter bag for a pending session restore on an environment
/// runtime. Carries the session metadata and optional startup command across
/// the async runtime connection boundary.
#[derive(Clone)]
pub(crate) struct SessionRestoreEntry {
    pub(crate) session: WorkspaceSessionSnapshot,
    /// Raw provider resume command. Each backend owns the final transport
    /// command shape: terminal-bootstrap prefixes cwd; runtime executes it in
    /// the already-rooted remote PTY.
    pub(crate) resume_command: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PlainTerminalEntry {
    pub(crate) hide_homepage: bool,
    pub(crate) show_welcome_if_enabled: bool,
    pub(crate) apply_default_session_mode: bool,
}

/// One semantic Environment entry intent shared by terminal-bootstrap and
/// runtime backends. Runtime stores the same value in `PendingMaterialization`;
/// local consumes it synchronously. Adding a new entry kind therefore cannot
/// create a remote-only payload model.
#[derive(Clone)]
pub(crate) enum EnvironmentEntryIntent {
    PlainTerminal(PlainTerminalEntry),
    StartupCommand(String),
    AgentView(AgentTabEntry),
    ForkedConversation(ForkEntry),
    SessionRestore(SessionRestoreEntry),
}

/// Product-level reason for activating an Environment navigation container.
///
/// Carrier differences remain backend-owned, while callers state whether the
/// target container must exist or whether activation follows a close that may
/// already have exposed a valid neighboring tab.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EnvironmentNavigationActivationIntent {
    EnsureTargetContainer,
    AfterContainerClosed,
}

/// Side-effect permission for refreshing indexed Environment sessions.
///
/// Passive projection is used by lifecycle/configuration reconciliation and
/// must never reconnect transport. Only an explicit user refresh owns a toast
/// generation; unavailable runtime transport is handled by the separate reconnect action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EnvironmentSessionRefreshIntent {
    PassiveProjection,
    UserInitiated { generation: u64 },
}

/// Environment header 与 backend 共享的会话刷新可用性。
///
/// 远端 discovery 是 helper-native host operation，不依赖 terminal execution
/// carrier；但必须证明 exact Environment owner 与 helper client 都仍 Connected。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EnvironmentSessionRefreshAvailability {
    Unavailable,
    Ready,
}

impl EnvironmentEntryIntent {
    pub(crate) fn pane_session_binding(&self) -> Option<PaneSessionBinding> {
        match self {
            Self::SessionRestore(restore) => {
                PaneSessionBinding::from_workspace_session(&restore.session)
            }
            Self::PlainTerminal(_)
            | Self::StartupCommand(_)
            | Self::AgentView(_)
            | Self::ForkedConversation(_) => None,
        }
    }
}

impl PlainTerminalEntry {
    pub(crate) fn default_tab(hide_homepage: bool) -> Self {
        Self {
            hide_homepage,
            show_welcome_if_enabled: false,
            apply_default_session_mode: true,
        }
    }
}

/// Result of persisting one optimistic alias/pin mutation. Local persistence
/// completes synchronously and returns the canonical sidecar state; runtime
/// persistence completes in its spawned callback.
pub(crate) enum SessionUserStateMutationDelivery {
    Applied(EnvironmentCliAgentSessionUserState),
    Pending,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SessionUserStateMutationFeedback {
    AliasSet,
    AliasCleared,
    Pinned,
    Unpinned,
    CleanupAlias,
    CleanupPinned,
}

impl SessionUserStateMutationFeedback {
    pub(crate) fn success_message(self) -> Option<&'static str> {
        match self {
            Self::AliasSet => Some("已更新会话别名"),
            Self::AliasCleared => Some("已清除会话别名"),
            Self::Pinned => Some("已置顶会话"),
            Self::Unpinned => Some("已取消置顶"),
            Self::CleanupAlias | Self::CleanupPinned => None,
        }
    }

    pub(crate) fn error_message(self, error: &str) -> String {
        match self {
            Self::AliasSet => format!("更新会话别名失败：{error}"),
            Self::AliasCleared => format!("清除会话别名失败：{error}"),
            Self::Pinned | Self::Unpinned => format!("置顶状态更新失败：{error}"),
            Self::CleanupAlias => format!("清理会话别名失败：{error}"),
            Self::CleanupPinned => format!("清理会话置顶状态失败：{error}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

pub(crate) trait EnvironmentBackend {
    // --- Directory capabilities (unchanged from A) ---

    /// cd into a directory from the file browser.
    /// Local fills the input box (user confirms); runtime executes directly.
    fn cd_to_directory(
        &self,
        ws: &mut Workspace,
        env: &EnvironmentSnapshot,
        path: &Path,
        ctx: &mut ViewContext<Workspace>,
    );

    /// Open a directory in a new tab.
    /// Returns `true` if the terminal materialized synchronously (local).
    fn open_directory_tab(
        &self,
        ws: &mut Workspace,
        env: &EnvironmentSnapshot,
        path: &Path,
        hide_homepage: bool,
        ctx: &mut ViewContext<Workspace>,
    ) -> bool;

    /// Open a directory tab and enter agent mode.
    /// Returns `true` if the terminal materialized synchronously (local).
    fn open_agent_directory_tab(
        &self,
        ws: &mut Workspace,
        env: &EnvironmentSnapshot,
        path: &Path,
        hide_homepage: bool,
        open_code_review_pane: bool,
        fallback_display_title: Option<String>,
        ctx: &mut ViewContext<Workspace>,
    ) -> bool;

    // --- Delivery methods (B+) ---

    /// Allocate and deliver one semantic Environment entry. Local consumes the
    /// intent synchronously; runtime queues the same value and consumes it after
    /// PTY bootstrap. SessionRestore deliberately uses this same entrypoint so
    /// Navigator never owns a local/remote branch.
    fn deliver_entry(
        &self,
        ws: &mut Workspace,
        env: &EnvironmentSnapshot,
        intent: EnvironmentEntryIntent,
        ctx: &mut ViewContext<Workspace>,
    );

    /// Activate the semantic navigation container for an Environment without
    /// creating an entry intent. TerminalBootstrap and Runtime may use different
    /// carrier panes, but product actions never select that backend behavior.
    fn activate_navigation_container(
        &self,
        ws: &mut Workspace,
        env: &EnvironmentSnapshot,
        intent: EnvironmentNavigationActivationIntent,
        ctx: &mut ViewContext<Workspace>,
    ) -> Result<(), String>;

    /// Read the backend-independent cache owned by EnvironmentTable. Backends
    /// populate the same cache from current-app sidecars or runtime RPC.
    fn session_user_state(
        &self,
        ws: &Workspace,
        authority: &str,
    ) -> EnvironmentCliAgentSessionUserState;

    /// Persist an optimistic alias/pin mutation. The caller performs the common
    /// cache transition before dispatch; synchronous errors are rolled back by
    /// the caller and runtime callbacks use the supplied previous state.
    fn mutate_session_user_state(
        &self,
        ws: &mut Workspace,
        authority: &str,
        generation: u64,
        feedback: SessionUserStateMutationFeedback,
        keys: Vec<String>,
        mutation: EnvironmentCliAgentSessionUserStateMutation,
        previous_state: EnvironmentCliAgentSessionUserState,
        ctx: &mut ViewContext<Workspace>,
    ) -> Result<SessionUserStateMutationDelivery, String>;

    /// Refresh indexed sessions for one authority. Returns true when completion
    /// is asynchronous and will finish the optional refresh generation later.
    fn refresh_indexed_sessions(
        &self,
        ws: &mut Workspace,
        authority: &str,
        intent: EnvironmentSessionRefreshIntent,
        ctx: &mut ViewContext<Workspace>,
    ) -> Result<bool, String>;
}

// ---------------------------------------------------------------------------
// Backend kinds & dispatch
// ---------------------------------------------------------------------------

pub(crate) struct TerminalBootstrapEnvironmentBackend;
pub(crate) struct RuntimeEnvironmentBackend;

pub(crate) enum EnvironmentBackendKind {
    TerminalBootstrap,
    Runtime,
}

impl EnvironmentBackendKind {
    pub(crate) fn for_authority(authority: &str) -> Self {
        if ParsedEnvironmentAuthority::parse(authority).uses_terminal_bootstrap() {
            Self::TerminalBootstrap
        } else {
            Self::Runtime
        }
    }

    pub(crate) fn for_environment(env: &EnvironmentSnapshot) -> Self {
        Self::for_authority(&env.authority_key)
    }

    pub(crate) fn backend(self) -> &'static dyn EnvironmentBackend {
        static LOCAL: TerminalBootstrapEnvironmentBackend = TerminalBootstrapEnvironmentBackend;
        static RUNTIME: RuntimeEnvironmentBackend = RuntimeEnvironmentBackend;
        match self {
            Self::TerminalBootstrap => &LOCAL,
            Self::Runtime => &RUNTIME,
        }
    }
}
