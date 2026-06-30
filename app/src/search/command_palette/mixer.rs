use crate::ai::agent::conversation::AIConversationId;
use crate::drive::ObjectTypeAndId;
use crate::launch_configs::launch_config::LaunchConfig;
use crate::object_store::ids::ObjectStoreId;
use crate::search::command_palette::new_session::{NewSessionOption, NewSessionOptionId};
use crate::search::mixer::SearchMixer;
use crate::util::bindings::CommandBinding;
use crate::workspace::environment_provider::EnvironmentProviderTarget;
use crate::workspace::PaneViewLocator;
use std::sync::Arc;
use warp_core::HostId;
use warp_util::path::LineAndColumnArg;
use warpui::keymap::BindingId;
use warpui::{EntityId, WindowId};

pub type CommandPaletteMixer = SearchMixer<CommandPaletteItemAction>;

#[derive(Clone, Debug)]
pub enum CommandPaletteItemAction {
    /// A binding result was clicked.
    AcceptBinding {
        binding: Arc<CommandBinding>,
    },
    ExecuteWorkflow {
        id: ObjectStoreId,
    },
    OpenNotebook {
        id: ObjectStoreId,
    },
    ViewInLocalDrive {
        id: ObjectTypeAndId,
    },
    InvokeEnvironmentVariables {
        id: ObjectStoreId,
    },
    /// Navigate to the session identified by `pane_view`.
    NavigateToSession {
        pane_view_locator: PaneViewLocator,
        window_id: WindowId,
    },
    /// Activate a session that is not currently backed by a live terminal pane
    /// (restored / CLI-agent-indexed / historical Ashide conversation). Routes
    /// through `WorkspaceAction::ActivateRestoredWorkspaceSession`, which
    /// restores-or-focuses the session via the Session Navigator path.
    ActivateRestoredWorkspaceSession {
        target: crate::workspace::WorkspaceSessionActionTarget,
    },
    /// Navigate to a specific conversation.
    NavigateToConversation {
        pane_view_locator: Option<PaneViewLocator>,
        window_id: Option<WindowId>,
        conversation_id: AIConversationId,
        terminal_view_id: Option<EntityId>,
    },
    ForkConversation {
        conversation_id: AIConversationId,
    },
    OpenLaunchConfiguration {
        config: Arc<LaunchConfig>,
        /// See [`OpenLaunchConfigArg::open_in_active_window`].
        open_in_active_window: bool,
    },
    NewSession {
        source: Arc<NewSessionOption>,
    },
    OpenFile {
        path: String,
        project_directory: String,
        line_and_column_arg: Option<LineAndColumnArg>,
    },
    /// Open a file from an Environment Runtime host via buffer-sync instead of current-app FS.
    OpenEnvironmentFile {
        host_id: HostId,
        path: String,
        line_and_column_arg: Option<LineAndColumnArg>,
    },
    OpenDirectory {
        path: String,
        project_directory: String,
    },
    /// Change directory in an Environment Runtime terminal without persisting a current-app recent.
    OpenEnvironmentDirectory {
        host_id: HostId,
        path: String,
    },
    CreateFile {
        file_name: String,
        current_directory: String,
    },
    NewConversationInProject {
        path: String,
        project_name: String,
    },
    /// Start a new AI conversation
    NewConversation,
    /// 打开当前 provider target 对应的 Environment Runtime terminal。
    OpenEnvironmentProviderTerminal {
        target: EnvironmentProviderTarget,
    },
    /// No-op action (used for non-interactable separator items that don't do anything on click).
    NoOp,
}

impl CommandPaletteItemAction {
    pub fn to_summary(&self) -> ItemSummary {
        match self {
            CommandPaletteItemAction::AcceptBinding { binding } => ItemSummary::Action {
                binding_id: binding.id,
            },
            CommandPaletteItemAction::OpenNotebook { id } => ItemSummary::Notebook { id: *id },
            CommandPaletteItemAction::ExecuteWorkflow { id } => ItemSummary::Workflow { id: *id },
            CommandPaletteItemAction::InvokeEnvironmentVariables { id } => {
                ItemSummary::EnvVarCollection { id: *id }
            }
            CommandPaletteItemAction::NavigateToSession {
                pane_view_locator, ..
            } => ItemSummary::Session {
                pane_view_locator: *pane_view_locator,
            },
            CommandPaletteItemAction::ActivateRestoredWorkspaceSession { .. } => {
                // Restored sessions have no stable pane locator, so they cannot
                // be reconstructed from a recent-items summary. Treat as
                // no-op for the recent-items path; activation itself is
                // handled in the action handler.
                ItemSummary::NoOp
            }
            CommandPaletteItemAction::NavigateToConversation {
                conversation_id, ..
            } => ItemSummary::Conversation {
                id: *conversation_id,
            },
            CommandPaletteItemAction::ForkConversation { .. } => ItemSummary::ForkConversation,
            CommandPaletteItemAction::NewSession { source } => ItemSummary::NewSession {
                id: source.id().clone(),
            },
            CommandPaletteItemAction::OpenLaunchConfiguration { .. } => {
                ItemSummary::LaunchConfiguration
            }
            CommandPaletteItemAction::ViewInLocalDrive { id } => match id {
                ObjectTypeAndId::Notebook(_)
                | ObjectTypeAndId::Folder(_)
                | ObjectTypeAndId::GenericStringObject { .. } => ItemSummary::StoredObject,
                ObjectTypeAndId::Workflow(id) => ItemSummary::Workflow { id: *id },
            },
            CommandPaletteItemAction::OpenFile {
                path,
                project_directory,
                line_and_column_arg,
            } => ItemSummary::File {
                path: path.clone(),
                project_directory: project_directory.clone(),
                line_and_column_arg: *line_and_column_arg,
            },
            CommandPaletteItemAction::OpenEnvironmentFile { .. } => {
                // Environment files are host-scoped and must not be reconstructed from
                // current-app recent items, otherwise a remote palette selection can
                // silently turn back into a local file open.
                ItemSummary::NoOp
            }
            CommandPaletteItemAction::OpenDirectory {
                path,
                project_directory,
            } => ItemSummary::Directory {
                path: path.clone(),
                project_directory: project_directory.clone(),
            },
            CommandPaletteItemAction::OpenEnvironmentDirectory { .. } => ItemSummary::NoOp,
            CommandPaletteItemAction::CreateFile { .. } => {
                // CreateFile actions should not show up in recent items
                ItemSummary::NoOp
            }
            CommandPaletteItemAction::NewConversationInProject { path, .. } => {
                ItemSummary::Project { path: path.clone() }
            }
            CommandPaletteItemAction::NewConversation => ItemSummary::NewConversation,
            CommandPaletteItemAction::OpenEnvironmentProviderTerminal { target } => {
                ItemSummary::EnvironmentProviderTarget {
                    connection_ref: target.connection_ref().to_owned(),
                }
            }
            CommandPaletteItemAction::NoOp => ItemSummary::NoOp,
        }
    }
}

/// Summary of items that were selected via the command palette. This is needed so that we have a
/// unique way to identify a selected item  so  we can show it in the "recent" section of the
/// palette. We choose to not use the entire [`CommandPaletteItemAction`] since we only need a
/// unique identifier to store. Additionally, parts of the `CommandPaletteItemAction` could change
/// in between invocations of the command palette (such as the content or title of a workflow or the
/// trigger for a keybinding) that should not be factored in when determining whether to show it in
/// the recent section of the palette.
#[derive(Clone, Debug, PartialEq)]
pub enum ItemSummary {
    Action {
        binding_id: BindingId,
    },
    Workflow {
        id: ObjectStoreId,
    },
    EnvVarCollection {
        id: ObjectStoreId,
    },
    Notebook {
        id: ObjectStoreId,
    },
    Session {
        pane_view_locator: PaneViewLocator,
    },
    NewSession {
        id: NewSessionOptionId,
    },
    /// Dummy enum variant for launch configurations until we support showing them in recent section
    /// of the zero state
    LaunchConfiguration,
    /// Dummy enum variant for object-store objects that are not supported yet in command palette
    StoredObject,
    File {
        path: String,
        project_directory: String,
        line_and_column_arg: Option<LineAndColumnArg>,
    },
    Directory {
        path: String,
        project_directory: String,
    },
    Project {
        path: String,
    },
    Conversation {
        id: AIConversationId,
    },
    ForkConversation,
    NewConversation,
    /// Environment provider target selected from command palette.
    EnvironmentProviderTarget {
        connection_ref: String,
    },
    /// No-op action (used for non-interactable separator items that don't do anything on click).
    NoOp,
}
