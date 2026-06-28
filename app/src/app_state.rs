use pathfinder_geometry::rect::RectF;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use warpui::platform::FullscreenState;

use warpui::AppContext;

use crate::ai::agent::conversation::AIConversationId;
use crate::ai::agent_conversations_model::AgentManagementFilters;
use crate::ai::ambient_agents::AmbientAgentTaskId;
use crate::ai::blocklist::InputConfig;
use crate::ai::blocklist::SerializedBlockListItem;
use crate::code::editor_management::CodeSource;
use crate::drive::LocalDriveObjectSettings;
use crate::object_store::ids::ObjectStoreId;
use crate::root_view::quake_mode_window_id;
use crate::settings_view::SettingsSection;
use crate::tab::SelectedTabColor;
use crate::terminal::ShellLaunchData;
use crate::themes::theme::AnsiColorIdentifier;
use crate::workspace::view::left_panel::ToolPanelView;
use crate::workspace::WorkspaceRegistry;
use warpui::SingletonEntity as _;

#[derive(Debug, Clone, PartialEq)]
pub struct AppState {
    pub windows: Vec<WindowSnapshot>,
    pub active_window_index: Option<usize>,
    pub block_lists: Arc<HashMap<PaneUuid, Vec<SerializedBlockListItem>>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PaneUuid(pub Vec<u8>);

/// Wrapper for persisting agent management filters to restore.
#[derive(Default, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedAgentManagementFilters {
    pub filters: AgentManagementFilters,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnvironmentKind {
    Local,
    Ssh,
    Container,
    Wsl,
    Custom,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnvironmentLifecycleState {
    Connected,
    Dormant,
    Connecting,
    Installing,
    Reconnecting,
    Error,
}

/// Minimal Ashide environment metadata carried by persisted window snapshots.
///
/// This is intentionally small: the first milestone is to make the authority
/// boundary explicit without splitting Ashide's existing `Workspace.tabs` and
/// pane-group persistence model. Full `environments` / `workspace_sessions`
/// tables can backfill from this skeleton later.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentSnapshot {
    pub label: String,
    pub kind: EnvironmentKind,
    pub authority_key: String,
    /// Stable reference to the provider profile that owns this environment.
    ///
    /// Runtime-backed environments keep the provider connection id here.
    /// Terminal-bootstrap environments keep this empty; future container / WSL /
    /// custom providers can point at their own profile IDs without changing the
    /// snapshot shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_ref: Option<String>,
    pub active_workspace_root: Option<String>,
    pub lifecycle_state: EnvironmentLifecycleState,
}

impl EnvironmentSnapshot {
    pub fn local(active_workspace_root: Option<String>) -> Self {
        let authority_key = active_workspace_root
            .as_deref()
            .map(|root| format!("local:{root}"))
            .unwrap_or_else(|| "local".to_string());

        Self {
            label: "Local".to_string(),
            kind: EnvironmentKind::Local,
            authority_key,
            connection_ref: None,
            active_workspace_root,
            lifecycle_state: EnvironmentLifecycleState::Connected,
        }
    }

    pub fn local_from_tabs(tabs: &[TabSnapshot], active_tab_index: usize) -> Self {
        Self::terminal_bootstrap_from_tabs(tabs, active_tab_index)
    }

    pub fn terminal_bootstrap(active_workspace_root: Option<String>) -> Self {
        Self::local(active_workspace_root)
    }

    pub fn terminal_bootstrap_from_tabs(tabs: &[TabSnapshot], active_tab_index: usize) -> Self {
        Self::terminal_bootstrap(infer_active_workspace_root(tabs, active_tab_index))
    }

    pub fn runtime_transport(
        kind: EnvironmentKind,
        label: String,
        authority_key: String,
        connection_ref: Option<String>,
        active_workspace_root: Option<String>,
        lifecycle_state: EnvironmentLifecycleState,
    ) -> Self {
        Self {
            label,
            kind,
            authority_key,
            connection_ref,
            active_workspace_root,
            lifecycle_state,
        }
    }

    pub fn runtime_connection_ref(&self) -> Option<&str> {
        self.connection_ref.as_deref().or_else(|| {
            self.authority_key.strip_prefix("ssh:").or_else(|| {
                self.authority_key
                    .strip_prefix("ssh-config:")
                    .map(|_| self.authority_key.as_str())
            })
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkspaceSessionKind {
    Terminal,
    AgentTerminal,
    Welcome,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CliAgentSessionOrigin {
    /// The terminal command looked like a known CLI agent. This is an
    /// auto-detected terminal annotation, not explicit Ashide ownership.
    CommandDetected,
    /// A CLI-agent plugin/listener produced structured events for the session.
    PluginObserved,
}

/// Minimal Ashide session metadata carried beside Ashide's pane tree snapshot.
///
/// This does not attempt to persist PTY/runtime state. It is a stable restore
/// and recall scaffold for Ashide's workspace model: which environment owns a
/// session, what root/cwd it was associated with, and which agent
/// conversations can be resumed by higher-level CLI-agent integrations later.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSessionSnapshot {
    pub id: String,
    pub kind: WorkspaceSessionKind,
    pub label: Option<String>,
    pub environment_authority_key: Option<String>,
    pub cwd: Option<String>,
    pub startup_directory: Option<String>,
    /// Stable serialized [`CLIAgent`](crate::terminal::CLIAgent) name captured
    /// from the live session. This is normalized agent metadata; `cli_command`
    /// may still carry a user alias or custom command prefix.
    #[serde(default)]
    pub cli_agent: Option<String>,
    pub cli_command: Option<String>,
    #[serde(default)]
    pub cli_agent_origin: Option<CliAgentSessionOrigin>,
    pub conversation_ids: Vec<String>,
    pub active_conversation_id: Option<String>,
    /// CLI-native session identifier captured from plugin events, distinct from
    /// Ashide/Ashide AI conversation IDs. Used by explicit warm-restore adapters.
    #[serde(default)]
    pub cli_agent_session_id: Option<String>,
    pub is_active: bool,
    #[serde(default)]
    pub is_pinned: bool,
    /// Last-known update time used only for Session Navigator ordering. Live
    /// terminal snapshots may leave this empty; provider indexes should fill it.
    #[serde(default)]
    pub updated_at_unix_ms: Option<i64>,
}

impl WorkspaceSessionSnapshot {
    pub fn stable_pin_keys(&self) -> Vec<String> {
        let environment_key = self
            .environment_authority_key
            .as_deref()
            .filter(|key| !key.trim().is_empty())
            .unwrap_or("local");
        let mut keys = Vec::new();

        if let Some(cli_agent_session_id) = self
            .cli_agent_session_id
            .as_deref()
            .filter(|id| !id.trim().is_empty())
        {
            keys.push(format!(
                "{environment_key}::agent:{}:{}",
                self.cli_agent
                    .as_deref()
                    .or(self.cli_command.as_deref())
                    .unwrap_or_default(),
                cli_agent_session_id
            ));
        }

        for conversation_id in self
            .active_conversation_id
            .iter()
            .chain(self.conversation_ids.iter())
            .filter(|id| !id.trim().is_empty())
        {
            keys.push(format!("{environment_key}::conversation:{conversation_id}"));
        }

        if Self::is_stable_source_id(&self.id) {
            keys.push(self.id.clone());
            keys.push(self.logical_key());
        }

        keys.sort();
        keys.dedup();
        keys
    }

    pub fn is_pinned_by(&self, pinned_session_ids: &HashSet<String>) -> bool {
        self.stable_pin_keys()
            .iter()
            .any(|key| pinned_session_ids.contains(key))
    }

    fn is_stable_source_id(id: &str) -> bool {
        !id.trim().is_empty() && !id.starts_with("tab:")
    }

    pub fn logical_key(&self) -> String {
        let environment_key = self
            .environment_authority_key
            .as_deref()
            .filter(|key| !key.trim().is_empty())
            .unwrap_or("local");

        if let Some(cli_agent_session_id) = self
            .cli_agent_session_id
            .as_deref()
            .filter(|id| !id.trim().is_empty())
        {
            return format!(
                "{environment_key}::agent:{}:{}",
                self.cli_agent
                    .as_deref()
                    .or(self.cli_command.as_deref())
                    .unwrap_or_default(),
                cli_agent_session_id
            );
        }

        format!("{environment_key}::source:{}", self.id)
    }

    pub fn merge_for_session_navigator(
        sources: impl IntoIterator<Item = WorkspaceSessionSnapshot>,
        pinned_session_ids: &HashSet<String>,
    ) -> Vec<WorkspaceSessionSnapshot> {
        let mut sessions: Vec<WorkspaceSessionSnapshot> = Vec::new();
        let mut keys = HashMap::<String, usize>::new();

        for mut source in sources {
            let logical_key = source.logical_key();
            let source_is_live = source.id.starts_with("tab:");
            source.is_active = source_is_live && source.is_active;
            source.is_pinned = source.is_pinned || source.is_pinned_by(pinned_session_ids);

            if let Some(index) = keys.get(&logical_key).copied() {
                let existing = &mut sessions[index];
                existing.is_active |= source.is_active;
                existing.is_pinned |= source.is_pinned;
                existing.updated_at_unix_ms =
                    existing.updated_at_unix_ms.max(source.updated_at_unix_ms);
                if existing.label.is_none() || source.is_active || source_is_live {
                    existing.label = source.label.clone().or_else(|| existing.label.clone());
                }
                if existing.cwd.is_none() || source.is_active || source_is_live {
                    existing.cwd = source.cwd.clone().or_else(|| existing.cwd.clone());
                }
                if existing.startup_directory.is_none() || source.is_active || source_is_live {
                    existing.startup_directory = source
                        .startup_directory
                        .clone()
                        .or_else(|| existing.startup_directory.clone());
                }
                if existing.environment_authority_key.is_none()
                    || source.is_active
                    || source_is_live
                {
                    existing.environment_authority_key = source
                        .environment_authority_key
                        .clone()
                        .or_else(|| existing.environment_authority_key.clone());
                }
                if source_is_live && !existing.id.starts_with("tab:") {
                    existing.id = source.id;
                }
                continue;
            }

            keys.insert(logical_key, sessions.len());
            sessions.push(source);
        }

        sessions.sort_by(|left, right| {
            right
                .is_pinned
                .cmp(&left.is_pinned)
                .then_with(|| left.updated_at_unix_ms.cmp(&right.updated_at_unix_ms))
                .then_with(|| {
                    left.label
                        .as_deref()
                        .unwrap_or_default()
                        .cmp(right.label.as_deref().unwrap_or_default())
                })
                .then_with(|| left.id.cmp(&right.id))
        });

        sessions
    }

    pub fn from_tabs(
        tabs: &[TabSnapshot],
        fallback_environment: Option<&EnvironmentSnapshot>,
    ) -> Vec<WorkspaceSessionSnapshot> {
        let mut sessions = Vec::new();
        for (tab_index, tab) in tabs.iter().enumerate() {
            let mut leaf_index = 0;
            let environment = tab.environment.as_ref().or(fallback_environment);
            collect_workspace_sessions_from_node(
                &tab.root,
                tab_index,
                &mut leaf_index,
                tab.custom_title.as_deref(),
                environment,
                &mut sessions,
            );
        }
        sessions
    }
}

fn collect_workspace_sessions_from_node(
    node: &PaneNodeSnapshot,
    tab_index: usize,
    leaf_index: &mut usize,
    tab_title: Option<&str>,
    environment: Option<&EnvironmentSnapshot>,
    sessions: &mut Vec<WorkspaceSessionSnapshot>,
) {
    match node {
        PaneNodeSnapshot::Branch(BranchSnapshot { children, .. }) => {
            for (_, child) in children {
                collect_workspace_sessions_from_node(
                    child,
                    tab_index,
                    leaf_index,
                    tab_title,
                    environment,
                    sessions,
                );
            }
        }
        PaneNodeSnapshot::Leaf(LeafSnapshot { contents, .. }) => {
            let id = format!("tab:{tab_index}:leaf:{leaf_index}");
            *leaf_index += 1;

            if let Some(session) = workspace_session_from_leaf(id, contents, tab_title, environment)
            {
                sessions.push(session);
            }
        }
    }
}

fn workspace_session_from_leaf(
    id: String,
    contents: &LeafContents,
    tab_title: Option<&str>,
    environment: Option<&EnvironmentSnapshot>,
) -> Option<WorkspaceSessionSnapshot> {
    let environment_authority_key =
        environment.map(|environment| environment.authority_key.clone());

    match contents {
        LeafContents::Terminal(terminal) => {
            let conversation_ids: Vec<String> = terminal
                .conversation_ids_to_restore
                .iter()
                .map(|id| id.to_string())
                .collect();
            let has_conversation = !conversation_ids.is_empty()
                || terminal.active_conversation_id.is_some()
                || terminal.cli_agent.is_some()
                || terminal.cli_command.is_some();
            Some(WorkspaceSessionSnapshot {
                id,
                kind: if has_conversation {
                    WorkspaceSessionKind::AgentTerminal
                } else {
                    WorkspaceSessionKind::Terminal
                },
                label: tab_title.map(str::to_string),
                environment_authority_key,
                cwd: terminal.cwd.clone(),
                startup_directory: None,
                cli_agent: terminal.cli_agent.clone(),
                cli_command: terminal.cli_command.clone(),
                cli_agent_origin: terminal.cli_agent_origin.clone(),
                conversation_ids,
                active_conversation_id: terminal
                    .active_conversation_id
                    .as_ref()
                    .map(|id| id.to_string()),
                cli_agent_session_id: terminal.cli_agent_session_id.clone(),
                is_active: terminal.is_active,
                is_pinned: false,
                updated_at_unix_ms: None,
            })
        }
        LeafContents::Welcome { startup_directory } => Some(WorkspaceSessionSnapshot {
            id,
            kind: WorkspaceSessionKind::Welcome,
            label: tab_title.map(str::to_string),
            environment_authority_key,
            cwd: None,
            startup_directory: startup_directory
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            cli_agent: None,
            cli_command: None,
            cli_agent_origin: None,
            conversation_ids: Vec::new(),
            active_conversation_id: None,
            cli_agent_session_id: None,
            is_active: false,
            is_pinned: false,
            updated_at_unix_ms: None,
        }),
        _ => None,
    }
}

fn infer_active_workspace_root(tabs: &[TabSnapshot], active_tab_index: usize) -> Option<String> {
    tabs.get(active_tab_index)
        .and_then(|tab| infer_root_from_node(&tab.root))
        .or_else(|| tabs.iter().find_map(|tab| infer_root_from_node(&tab.root)))
}

fn infer_root_from_node(node: &PaneNodeSnapshot) -> Option<String> {
    match node {
        PaneNodeSnapshot::Leaf(LeafSnapshot { contents, .. }) => infer_root_from_leaf(contents),
        PaneNodeSnapshot::Branch(BranchSnapshot { children, .. }) => children
            .iter()
            .find_map(|(_, child)| infer_root_from_node(child)),
    }
}

fn infer_root_from_leaf(contents: &LeafContents) -> Option<String> {
    match contents {
        LeafContents::Terminal(TerminalPaneSnapshot { cwd: Some(cwd), .. }) => Some(cwd.clone()),
        LeafContents::Welcome {
            startup_directory: Some(path),
        } => Some(path.to_string_lossy().into_owned()),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct WindowSnapshot {
    pub environment: Option<EnvironmentSnapshot>,
    pub workspace_sessions: Vec<WorkspaceSessionSnapshot>,
    pub tabs: Vec<TabSnapshot>,
    pub active_tab_index: usize,
    pub bounds: Option<RectF>,
    pub fullscreen_state: FullscreenState,
    pub quake_mode: bool,
    pub universal_search_width: Option<f32>,
    pub warp_ai_width: Option<f32>,
    pub voltron_width: Option<f32>,
    pub local_drive_index_width: Option<f32>,
    pub left_panel_open: bool,
    pub vertical_tabs_panel_open: bool,
    pub left_panel_width: Option<f32>,
    pub right_panel_width: Option<f32>,
    pub agent_management_filters: Option<PersistedAgentManagementFilters>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TabSnapshot {
    pub environment: Option<EnvironmentSnapshot>,
    pub custom_title: Option<String>,
    pub root: PaneNodeSnapshot,
    pub default_directory_color: Option<AnsiColorIdentifier>,
    pub selected_color: SelectedTabColor,
    pub left_panel: Option<LeftPanelSnapshot>,
    pub right_panel: Option<RightPanelSnapshot>,
}

impl TabSnapshot {
    pub(crate) fn color(&self) -> Option<AnsiColorIdentifier> {
        self.selected_color.resolve(self.default_directory_color)
    }
}

#[derive(Clone, Debug, PartialEq)]
#[allow(
    clippy::large_enum_variant,
    reason = "LeafSnapshot is significantly larger than BranchSnapshot due to nested snapshot types."
)]
pub enum PaneNodeSnapshot {
    Branch(BranchSnapshot),
    Leaf(LeafSnapshot),
}

impl PaneNodeSnapshot {
    pub fn has_horizontal_split(&self) -> bool {
        match self {
            PaneNodeSnapshot::Leaf(_) => false,
            PaneNodeSnapshot::Branch(BranchSnapshot {
                direction,
                children,
            }) => {
                let self_has_split = *direction == SplitDirection::Horizontal && children.len() > 1;
                self_has_split
                    || children
                        .iter()
                        .any(|(_, child)| child.has_horizontal_split())
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BranchSnapshot {
    pub direction: SplitDirection,
    pub children: Vec<(PaneFlex, PaneNodeSnapshot)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LeafSnapshot {
    pub is_focused: bool,
    pub custom_vertical_tabs_title: Option<String>,
    pub contents: LeafContents,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LeafContents {
    Terminal(TerminalPaneSnapshot),
    Notebook(NotebookPaneSnapshot),
    AIDocument(AIDocumentPaneSnapshot),
    Code(CodePaneSnapShot),
    EnvVarCollection(EnvVarCollectionPaneSnapshot),
    Workflow(WorkflowPaneSnapshot),
    Settings(SettingsPaneSnapshot),
    AIFact(AIFactPaneSnapshot),
    ExecutionProfileEditor,
    CodeReview(CodeReviewPaneSnapshot),
    AmbientAgent(AmbientAgentPaneSnapshot),
    /// An entrypoint pane type to launch other pane types from a search palette. The default view
    /// when creating a tab.
    Welcome {
        startup_directory: Option<PathBuf>,
    },
    /// A new first-time user experience which prioritizes choosing a coding repository.
    GetStarted,
    /// Stateless placeholder for a runtime-backed Environment tab before a real
    /// runtime PTY/session has been materialized.
    EnvironmentRuntimePlaceholder,
    /// Provider connection editor pane(Ashide 独有)。引用 provider connection 主键
    /// 加载/保存。**不持久化** — 重启后用户从左侧 Environment provider manager 重新打开。
    ProviderConnection {
        node_id: String,
    },
    /// Provider file browser pane。引用 provider connection 主键关联环境文件系统。
    /// **不持久化** — 重启后用户从左侧 Environment provider manager 重新打开。
    ProviderFileBrowser {
        node_id: String,
    },
}

#[cfg(feature = "local_fs")]
impl LeafContents {
    /// Whether this pane content should be written to (and later restored
    /// from) the SQLite app-state database.
    ///
    /// Non-persisted pane types are skipped entirely during the pane tree
    /// traversal in `save_app_state`, so no `pane_nodes` row is inserted for
    /// them. This is important: inserting a `pane_nodes` row with
    /// `is_leaf = true` but no matching `pane_leaves` row leaves an orphan
    /// that `read_node` cannot resolve, which causes the surrounding tab's
    /// restoration to fail and the whole tab to disappear on restart.
    pub(crate) fn is_persisted(&self) -> bool {
        match self {
            // Provider connection editor:连接数据持久化在 provider store 里,
            // pane 本身只是 view,关掉再打开没差别。
            LeafContents::ProviderConnection { .. } => false,
            // Provider file browser:环境文件系统依赖活跃 provider 连接,pane 不可恢复。
            LeafContents::ProviderFileBrowser { .. } => false,
            // 环境文件代码 pane:environment buffer 依赖活跃 Environment Runtime,`EnvironmentFileTree`
            // source 不可恢复(`is_restorable() == false`)。若写入持久化会留下
            // 一条 restore 阶段被跳过的孤儿 `Code` 行,导致整个 tab 丢失 ——
            // 因此带 Environment Runtime source 的代码 pane 整体不持久化。
            LeafContents::Code(CodePaneSnapShot::Local { source, .. }) => {
                source.as_ref().map(|s| s.is_restorable()).unwrap_or(true)
            }
            LeafContents::Terminal(_)
            | LeafContents::Notebook(_)
            | LeafContents::AIDocument(_)
            | LeafContents::EnvVarCollection(_)
            | LeafContents::Workflow(_)
            | LeafContents::Settings(_)
            | LeafContents::AIFact(_)
            | LeafContents::ExecutionProfileEditor
            | LeafContents::CodeReview(_)
            | LeafContents::AmbientAgent(_)
            | LeafContents::Welcome { .. }
            | LeafContents::GetStarted
            | LeafContents::EnvironmentRuntimePlaceholder => true,
        }
    }
}

/// Snapshot of an ambient agent pane.
#[derive(Clone, Debug, PartialEq)]
pub struct AmbientAgentPaneSnapshot {
    pub uuid: Vec<u8>,
    // `task_id` is purposefully optional,
    // as you can have a valid state (i.e. an empty ambient-agent pane) where it is None.
    pub task_id: Option<AmbientAgentTaskId>,
}

/// Snapshot of the contents of a terminal pane.
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalPaneSnapshot {
    pub uuid: Vec<u8>,
    pub cwd: Option<String>,
    pub shell_launch_data: Option<ShellLaunchData>,
    pub is_active: bool,
    pub is_read_only: bool,
    pub input_config: Option<InputConfig>,
    pub llm_model_override: Option<String>,
    pub active_profile_id: Option<ObjectStoreId>,
    pub conversation_ids_to_restore: Vec<AIConversationId>,
    /// The active conversation ID if the agent view was open in fullscreen mode.
    /// When `Some`, the agent view should be restored to fullscreen for this conversation.
    pub active_conversation_id: Option<AIConversationId>,
    /// Stable serialized [`CLIAgent`](crate::terminal::CLIAgent) name captured
    /// from the live session.
    pub cli_agent: Option<String>,
    /// The command prefix associated with this terminal. May be a custom alias.
    pub cli_command: Option<String>,
    /// Where the CLI-agent association came from. Command detection is only a
    /// lightweight annotation; plugin observation is stronger evidence for a
    /// future explicit resume adapter.
    pub cli_agent_origin: Option<CliAgentSessionOrigin>,
    /// CLI-native session identifier from structured agent events, if available.
    pub cli_agent_session_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NotebookPaneSnapshot {
    NotebookObject {
        /// The ID of the notebook that was open in this pane. There are 3 possibilities:
        /// 1. The pane contains a newly-created notebook that has not been edited yet. It might not
        ///    have an ID yet (client or server), so this will be `None`.
        /// 2. The pane contains a notebook that has not been persisted in the local object store yet, so this will
        ///    contain a client ID that should exist in SQLite.
        /// 3. The pane contains a notebook that's known to the server, so this will contain the
        ///    server ID.
        notebook_id: Option<ObjectStoreId>,
        // Settings for the notebook pane when it's opened (such as a folder to focus upon opening)
        settings: LocalDriveObjectSettings,
    },
    CurrentAppFileNotebook {
        /// The path to the current app filesystem file that was open in this pane. This may be `None` if
        /// the pane contained an unreadable file.
        path: Option<PathBuf>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum AIDocumentPaneSnapshot {
    Local {
        document_id: String,
        version: i32,
        content: Option<String>,
        title: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct CodePaneTabSnapshot {
    pub path: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CodePaneSnapShot {
    Local {
        tabs: Vec<CodePaneTabSnapshot>,
        active_tab_index: usize,
        /// The full `CodeSource` for this pane, serialized as JSON in the DB.
        source: Option<CodeSource>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum WorkflowPaneSnapshot {
    WorkflowObject {
        workflow_id: Option<ObjectStoreId>,
        // Settings for the workflow pane when it's opened (such as a folder to focus upon opening)
        settings: LocalDriveObjectSettings,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum EnvVarCollectionPaneSnapshot {
    // EnvVarCollectionObject snapshots operate under the same heuristics
    // as NotebookPaneSnapshot::NotebookObject
    EnvVarCollectionObject {
        env_var_collection_id: Option<ObjectStoreId>,
    },
}

// Legacy environment-management pane snapshot was removed with the ambient-agent UI subsystem.

#[derive(Clone, Debug, PartialEq)]
pub enum SettingsPaneSnapshot {
    Local {
        current_page: SettingsSection,
        search_query: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum AIFactPaneSnapshot {
    Personal,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CodeReviewPaneSnapshot {
    Local {
        terminal_uuid: Vec<u8>,
        repo_path: PathBuf,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum LeftPanelDisplayedTab {
    FileTree,
    GlobalSearch,
    LocalDrive,
    EnvironmentProviderManager,
    ServerFileBrowser,
    SkillManager,
}

impl From<ToolPanelView> for LeftPanelDisplayedTab {
    fn from(view: ToolPanelView) -> Self {
        match view {
            ToolPanelView::ProjectExplorer => LeftPanelDisplayedTab::FileTree,
            ToolPanelView::EnvironmentProjectExplorer => LeftPanelDisplayedTab::FileTree,
            ToolPanelView::GlobalSearch { .. } => LeftPanelDisplayedTab::GlobalSearch,
            ToolPanelView::LocalDrive => LeftPanelDisplayedTab::LocalDrive,
            ToolPanelView::EnvironmentProviderManager => {
                LeftPanelDisplayedTab::EnvironmentProviderManager
            }
            ToolPanelView::ServerFileBrowser => LeftPanelDisplayedTab::ServerFileBrowser,
            ToolPanelView::SkillManager => LeftPanelDisplayedTab::SkillManager,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LeftPanelSnapshot {
    pub left_panel_displayed_tab: LeftPanelDisplayedTab,
    pub pane_group_id: String,
    pub width: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RightPanelSnapshot {
    pub pane_group_id: String,
    pub width: usize,
    pub is_maximized: bool,
}

/// Copied from pane group model, which should be private to pane group.
#[derive(Clone, Debug, PartialEq)]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PaneFlex(pub f32);

pub fn get_app_state(app: &AppContext) -> AppState {
    let active_window_id = app.windows().active_window();
    let quake_mode_id = quake_mode_window_id();

    let mut active_window_index = None;

    let mut windows = vec![];

    for (index, window_id) in app.window_ids().enumerate() {
        // Determine index of active window
        if let Some(active_window_id) = active_window_id {
            if active_window_id == window_id {
                active_window_index = Some(index);
            }
        }

        if let Some(workspace) = WorkspaceRegistry::as_ref(app).get(window_id, app) {
            let ws = workspace.as_ref(app);
            // Transient drag-preview windows are not real user-visible
            // workspaces; skip them so they never end up in the persisted
            // session. (Persistence is also short-circuited entirely while a
            // cross-window drag is active; see `save_app` in
            // `workspace/global_actions.rs`.)
            if ws.is_tab_drag_preview() {
                continue;
            }
            let snapshot = ws.snapshot(
                window_id,
                quake_mode_id.map(|id| id == window_id).unwrap_or(false),
                app,
            );
            if !snapshot.tabs.is_empty() {
                windows.push(snapshot);
            }
        }
    }

    AppState {
        windows,
        active_window_index,
        block_lists: Default::default(),
    }
}

#[cfg(test)]
#[path = "app_state_tests.rs"]
mod tests;
