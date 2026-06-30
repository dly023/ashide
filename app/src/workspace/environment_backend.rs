//! Environment entry backend trait — the behavior-layer unification of the
//! local (terminal-bootstrap) and remote (runtime) environments.
//!
//! # Architecture
//!
//! All user-facing "open tab" actions go through a single dispatch path:
//!
//! ```text
//! capability fn              ← only builds AgentTabEntry, no if/else
//!      │
//!      ▼
//! backend.deliver_agent_tab(entry)   ← single dispatch point
//!      │
//!      ├── LocalEntryBackend  → create terminal + apply_agent_tab_entry_immediately(entry)
//!      └── RuntimeEntryBackend → queue AgentTabEntry → materialize → apply
//! ```
//!
//! The data layer (`EnvironmentSnapshot` / `authority`) was already unified.
//! This module unifies the behavior layer: every capability builds the same
//! `AgentTabEntry` struct and dispatches through `deliver_agent_tab`. The
//! local/remote fork is isolated to the two `deliver_*` impl bodies.
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
use crate::app_state::EnvironmentSnapshot;
use crate::terminal::view::inline_banner::ZeroStatePromptSuggestionType;
use crate::workspace::environment_runtime::authority_uses_terminal_bootstrap;
use crate::workspace::view::Workspace;

// ---------------------------------------------------------------------------
// Shared entry types
// ---------------------------------------------------------------------------

/// Unified parameter bag for all "open agent tab" capabilities.
///
/// Both `LocalEntryBackend::deliver_agent_tab` and
/// `RuntimeEntryBackend::deliver_agent_tab` receive this struct. The local impl
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
/// `LocalEntryBackend::deliver_fork` and `RuntimeEntryBackend::deliver_fork`
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

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

pub(crate) trait EnvironmentEntryBackend {
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

    /// Deliver an agent-tab intent: create a terminal and apply all side
    /// effects in `entry`. Local applies immediately; runtime queues and
    /// applies after bootstrap. This is the single dispatch point for all
    /// "open agent tab" capabilities.
    fn deliver_agent_tab(
        &self,
        ws: &mut Workspace,
        env: &EnvironmentSnapshot,
        entry: AgentTabEntry,
        ctx: &mut ViewContext<Workspace>,
    );

    /// Deliver a startup-command tab: create a terminal and execute `command`.
    fn deliver_startup_command(
        &self,
        ws: &mut Workspace,
        env: &EnvironmentSnapshot,
        command: String,
        ctx: &mut ViewContext<Workspace>,
    );

    /// Deliver a fork-to-new-tab intent: create a terminal and restore the
    /// forked conversation + copy model/profile + handle prompts. Local applies
    /// immediately; runtime queues and applies after bootstrap. This is the
    /// single dispatch point for fork-to-new-tab. (Fork-to-split-pane is a
    /// separate capability with no remote path today — see capability matrix #15.)
    fn deliver_fork(
        &self,
        ws: &mut Workspace,
        env: &EnvironmentSnapshot,
        entry: ForkEntry,
        ctx: &mut ViewContext<Workspace>,
    );
}

// ---------------------------------------------------------------------------
// Backend kinds & dispatch
// ---------------------------------------------------------------------------

pub(crate) struct LocalEntryBackend;
pub(crate) struct RuntimeEntryBackend;

pub(crate) enum EnvironmentBackendKind {
    TerminalBootstrap,
    Runtime,
}

impl EnvironmentBackendKind {
    pub(crate) fn for_environment(env: &EnvironmentSnapshot) -> Self {
        if authority_uses_terminal_bootstrap(&env.authority_key) {
            Self::TerminalBootstrap
        } else {
            Self::Runtime
        }
    }

    pub(crate) fn backend(self) -> &'static dyn EnvironmentEntryBackend {
        static LOCAL: LocalEntryBackend = LocalEntryBackend;
        static RUNTIME: RuntimeEntryBackend = RuntimeEntryBackend;
        match self {
            Self::TerminalBootstrap => &LOCAL,
            Self::Runtime => &RUNTIME,
        }
    }
}
