use std::collections::HashSet;

use chrono::NaiveDateTime;

use warpui::{AppContext, Entity, EntityId, WindowId};

use crate::context_chips::prompt_snapshot::PromptSnapshot;
use crate::pane_group::PaneGroup;
use crate::terminal::model::blockgrid::BlockGrid;
use crate::terminal::shared_session::SharedSessionStatus;
use crate::{
    pane_group::PaneId,
    workspace::{PaneViewLocator, Workspace},
};

/// Contains session metadata, including a prompt and running command (if there is one).
#[derive(Clone)]
pub struct SessionNavigationData {
    /// The prompt of the session.
    prompt: String,
    /// The various parts of the prompt, like virtual environment and working directory.
    prompt_elements: SessionNavigationPromptElements,
    /// A running command, if there is one.
    command_context: CommandContext,
    /// A `PaneViewLocator` to navigate to the session.
    pane_view_locator: PaneViewLocator,
    /// The id of the window the session is located in.
    window_id: WindowId,
    /// The timestamp of the last interaction.
    last_focus_ts: Option<NaiveDateTime>,
    /// Whether or not the session is in a read-only state.
    is_read_only: bool,
    /// The sharing status of the session.
    shared_session_status: SharedSessionStatus,
    /// When set, the session is not currently backed by a live terminal pane
    /// (restored / CLI-agent-indexed / historical Ashide conversation). Selecting
    /// it in the command palette must dispatch `ActivateRestoredWorkspaceSession`
    /// via this target rather than `NavigateToSession` (which only works for
    /// live panes). `None` for live terminal panes.
    restore_target: Option<crate::workspace::WorkspaceSessionActionTarget>,
}

impl SessionNavigationData {
    /// Returns whether the session is the session identified by `session_id`.
    pub fn is_for_session(&self, session_id: PaneId) -> bool {
        session_id == self.pane_view_locator().pane_id
    }
}

/// Contains prompt data for rendering in the command palette.
#[derive(Clone)]
pub struct SessionNavigationPromptElements {
    /// The raw terminal grid of the PS1 prompt, populated when `honor_ps1` is
    /// active. When present, the command palette renders this grid directly.
    pub ps1_prompt_grid: Option<BlockGrid>,
    /// A snapshot of the user's configured prompt chips and their current
    /// values. Used as the default prompt representation in the command palette.
    pub prompt_chip_snapshot: Option<PromptSnapshot>,
    /// Plain-text fallback label used when neither `ps1_prompt_grid` nor
    /// `prompt_chip_snapshot` is available — e.g. for restored / indexed /
    /// historical sessions surfaced into the command-palette search that have
    /// no live terminal pane to snapshot a prompt from. Renders as a simple
    /// text label so the row is still legible and searchable.
    pub display_label: Option<String>,
}

impl SessionNavigationPromptElements {
    /// Constructs prompt elements for a live terminal pane (no text fallback).
    pub fn from_live_pane(
        ps1_prompt_grid: Option<BlockGrid>,
        prompt_chip_snapshot: Option<PromptSnapshot>,
    ) -> Self {
        Self {
            ps1_prompt_grid,
            prompt_chip_snapshot,
            display_label: None,
        }
    }

    /// Constructs prompt elements for a non-live session (restored / indexed /
    /// historical) where only a plain display label is available.
    pub fn from_display_label(display_label: String) -> Self {
        Self {
            ps1_prompt_grid: None,
            prompt_chip_snapshot: None,
            display_label: Some(display_label),
        }
    }
}

/// Represents the execution context of a session - what command or AI interaction
/// was last run or is currently running.
#[derive(Clone, Debug)]
pub enum CommandContext {
    /// The last executed terminal command
    LastRunCommand {
        last_run_command: String,
        mins_since_completion: Option<i64>,
    },
    /// The last completed AI interaction
    LastRunAIBlock {
        prompt: String, // The prompt that initiated the AI interaction
    },
    /// Currently running terminal command
    RunningCommand { running_command: String },
    /// Currently running AI interaction
    RunningAIBlock {
        prompt: String, // The prompt for the active AI conversation
    },
    /// No command context (e.g. just launched terminal)
    None,
}

impl CommandContext {
    pub fn a11y_description(&self) -> Option<String> {
        match self {
            Self::None => None,
            Self::LastRunCommand {
                last_run_command, ..
            } => Some(format!("Last run command {}", last_run_command.clone())),
            Self::LastRunAIBlock { prompt } => Some(format!("Last AI interaction: {prompt}")),
            Self::RunningCommand { running_command } => {
                Some(format!("Currently running {running_command}"))
            }
            Self::RunningAIBlock { prompt } => {
                Some(format!("Currently running AI interaction: {prompt}"))
            }
        }
    }
}

impl SessionNavigationData {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        prompt: String,
        prompt_elements: SessionNavigationPromptElements,
        command_context: CommandContext,
        pane_view_locator: PaneViewLocator,
        last_focus_ts: Option<NaiveDateTime>,
        is_read_only: bool,
        window_id: WindowId,
        shared_session_status: SharedSessionStatus,
    ) -> Self {
        SessionNavigationData {
            prompt,
            prompt_elements,
            command_context,
            pane_view_locator,
            last_focus_ts,
            is_read_only,
            window_id,
            shared_session_status,
            restore_target: None,
        }
    }

    /// Constructs `SessionNavigationData` for a non-live session surfaced from
    /// the Session Navigator's merged set (restored / CLI-agent-indexed /
    /// historical Ashide conversation). The searchable `prompt` is built from
    /// the navigator's search fragments so the title-bar fuzzy matcher can hit
    /// any of them; the `display_label` is the row's primary label; the
    /// `restore_target` drives activation when the row is selected (routing
    /// through `ActivateRestoredWorkspaceSession` instead of pane focus).
    pub fn from_workspace_session_snapshot(
        snapshot: &crate::app_state::WorkspaceSessionSnapshot,
        window_id: WindowId,
    ) -> Self {
        use crate::workspace::view::vertical_tabs::session_display::{
            restored_session_label, restored_session_search_fragments,
        };

        let display_label = restored_session_label(snapshot);
        let fragments = restored_session_search_fragments(snapshot);
        let prompt = fragments.join(" ");

        let command_context = if let Some(command) = snapshot
            .cli_command
            .as_deref()
            .filter(|c| !c.trim().is_empty())
        {
            CommandContext::LastRunCommand {
                last_run_command: command.to_string(),
                mins_since_completion: None,
            }
        } else if let Some(prompt_str) = snapshot
            .active_conversation_id
            .as_deref()
            .filter(|id| !id.trim().is_empty())
        {
            CommandContext::LastRunAIBlock {
                prompt: prompt_str.to_string(),
            }
        } else {
            CommandContext::None
        };

        let last_focus_ts = snapshot
            .updated_at_unix_ms
            .and_then(|ms| {
                chrono::DateTime::from_timestamp_millis(ms).map(|dt| dt.naive_utc())
            });

        let restore_target = crate::workspace::WorkspaceSessionActionTarget::new(
            snapshot.id.clone(),
            snapshot.environment_authority_key.clone(),
        );

        SessionNavigationData {
            prompt,
            prompt_elements: SessionNavigationPromptElements::from_display_label(display_label),
            command_context,
            pane_view_locator: PaneViewLocator::placeholder(),
            last_focus_ts,
            is_read_only: false,
            window_id,
            shared_session_status: SharedSessionStatus::default(),
            restore_target: Some(restore_target),
        }
    }

    /// Returns the restore target, if this session is not backed by a live
    /// terminal pane.
    pub fn restore_target(&self) -> Option<&crate::workspace::WorkspaceSessionActionTarget> {
        self.restore_target.as_ref()
    }

    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    pub fn prompt_elements(&self) -> &SessionNavigationPromptElements {
        &self.prompt_elements
    }

    pub fn command_context(&self) -> CommandContext {
        self.command_context.clone()
    }

    pub fn pane_view_locator(&self) -> PaneViewLocator {
        self.pane_view_locator
    }

    pub fn window_id(&self) -> WindowId {
        self.window_id
    }

    pub fn last_focus_ts(&self) -> Option<NaiveDateTime> {
        self.last_focus_ts
    }

    pub fn is_read_only(&self) -> bool {
        self.is_read_only
    }

    pub fn shared_session_status(&self) -> SharedSessionStatus {
        self.shared_session_status.clone()
    }

    /// Fetches all sessions currently open in the app.
    pub fn all_sessions(app: &AppContext) -> impl Iterator<Item = SessionNavigationData> + '_ {
        app.window_ids()
            .filter_map(move |window_id| {
                let workspaces = app.views_of_type::<Workspace>(window_id)?;

                Some(workspaces.into_iter().flat_map(move |workspace| {
                    workspace.as_ref(app).workspace_sessions(window_id, app)
                }))
            })
            .flatten()
    }
}

pub struct RunningSessionSummary<'a> {
    /// Does not include long running blocks for viewer of a shared session.
    pub long_running_cmds: Vec<&'a SessionNavigationData>,
}

impl<'a> RunningSessionSummary<'a> {
    pub fn new(sessions: &'a [SessionNavigationData]) -> Self {
        let long_running_cmds: Vec<_> = sessions
            .iter()
            .filter(|session| {
                matches!(
                    session.command_context(),
                    CommandContext::RunningCommand { .. } | CommandContext::RunningAIBlock { .. }
                ) && !session.shared_session_status().is_viewer()
                    && !session.is_read_only()
            })
            .collect();
        Self { long_running_cmds }
    }

    pub fn windows_running(&self) -> HashSet<WindowId> {
        self.long_running_cmds
            .iter()
            .map(|session| session.window_id())
            .collect()
    }

    pub fn tabs_running(&self) -> HashSet<EntityId> {
        self.long_running_cmds
            .iter()
            .map(|session| session.pane_view_locator().pane_group_id)
            .collect()
    }

    pub fn processes_in_window(&self, window_id: &WindowId) -> Vec<&SessionNavigationData> {
        self.long_running_cmds
            .iter()
            .filter(|&session| session.window_id() == *window_id)
            .cloned()
            .collect()
    }
}

pub enum SessionSource {
    None,
    Set {
        active_pane_id: PaneId,
        active_tab_id: EntityId,
        active_window_id: WindowId,
    },
}

impl Entity for SessionSource {
    type Event = ();
}

pub fn num_shared_sessions(ctx: &AppContext) -> usize {
    let mut num_shared_sessions = 0;
    let window_ids: Vec<WindowId> = ctx.window_ids().collect();
    for window_id in window_ids {
        let Some(pane_group_views) = ctx.views_of_type::<PaneGroup>(window_id) else {
            continue;
        };
        for pane_group_view in pane_group_views {
            pane_group_view.read(ctx, |pane_group, ctx| {
                num_shared_sessions += pane_group.number_of_shared_sessions(ctx);
            })
        }
    }
    num_shared_sessions
}
