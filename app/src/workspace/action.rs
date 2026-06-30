use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use warp_util::path::LineAndColumnArg;

use crate::ai::agent::api::ServerConversationToken;
use crate::ai::agent::conversation::AIConversationId;
use crate::ai::agent::AIAgentExchangeId;
use crate::ai::ambient_agents::AmbientAgentTaskId;
use crate::ai::document::ai_document_model::{AIDocumentId, AIDocumentVersion};
use crate::app_interaction::PaletteSource;
use crate::auth::LoginGatedFeature;
use crate::drive::items::LocalDriveItemId;
use crate::drive::ObjectTypeAndId;
use crate::object_store::ids::ObjectStoreId;
use crate::palette::PaletteMode;
use crate::search;
use crate::settings_view::{SettingsAction as SettingsTabAction, SettingsSection};
use crate::tab::{NewSessionMenuItem, SelectedTabColor};
use crate::tab_configs::TabConfig;
use crate::terminal::available_shells::AvailableShell;
use crate::terminal::view::inline_banner::ZeroStatePromptSuggestionType;
use crate::terminal::CLIAgent;
use crate::themes::theme::AnsiColorIdentifier;
use crate::themes::theme_chooser::ThemeChooserMode;
use crate::workflows::{WorkflowSelectionSource, WorkflowSource, WorkflowType};
use crate::workspace::environment_provider::EnvironmentProviderTarget;
use crate::workspace::PaneViewLocator;

use ui_components::lightbox;
use warpui::accessibility::AccessibilityVerbosity;
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::Vector2F;
use warpui::platform::Cursor;
use warpui::{EntityId, WindowId};

use super::global_actions::{ForkFromExchange, ForkedConversationDestination};
use super::view::{OnboardingTutorial, WorkspaceBanner};

pub use crate::session_bridge::adapter_registry::SessionBridgeForkTarget;

/// This enum determines how the search query is initialized when opening command search.
#[derive(Clone, Default, Debug)]
pub enum InitContent {
    /// Read the content of the active terminal input, and make that the initial search query.
    #[default]
    FromInputBuffer,
    /// Specify an exact string to initialize the query to.
    Custom(String),
}

/// To initialize command search, we may want to specify a search filter, or the content of the
/// query itself.
#[derive(Clone, Default, Debug)]
pub struct CommandSearchOptions {
    pub filter: Option<search::QueryFilter>,
    pub init_content: InitContent,
}

/// Specifies how to restore a conversation when it's not already open in a pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum RestoreConversationLayout {
    /// Restore the conversation into the currently active pane.
    ActivePane,
    /// Restore the conversation in a new split pane.
    SplitPane,
    /// Restore the conversation in a new tab.
    #[default]
    NewTab,
}

#[derive(Debug, Clone, Copy)]
pub enum TabContextMenuAnchor {
    Pointer(Vector2F),
    VerticalTabsKebab,
}

/// Session Navigator actions must be scoped to the environment that rendered
/// the row. Live rows still use volatile `tab:x:leaf:y` ids, so the id alone is
/// not globally unique across local / remote environments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSessionActionTarget {
    pub session_id: String,
    pub environment_authority_key: Option<String>,
}

impl WorkspaceSessionActionTarget {
    pub fn new(session_id: String, environment_authority_key: Option<String>) -> Self {
        Self {
            session_id,
            environment_authority_key,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum VerticalTabsPaneContextMenuTarget {
    ClickedPane(PaneViewLocator),
    ActivePane(PaneViewLocator),
}

impl VerticalTabsPaneContextMenuTarget {
    pub fn locator(self) -> PaneViewLocator {
        match self {
            Self::ClickedPane(locator) | Self::ActivePane(locator) => locator,
        }
    }
}

/// Identifies the source of a session-bridge operation (fork / edit-and-fork /
/// export), independent of the operation itself. Collapses the former
/// per-entry-point action families (conversation / active-pane / workspace
/// target) into a single token so each operation is one action variant.
#[derive(Debug, Clone)]
pub enum SessionBridgeActionSource {
    /// An AI conversation, optionally associated with a remote environment.
    Conversation {
        conversation_id: AIConversationId,
        source_environment_authority_key: Option<String>,
    },
    /// The active AI conversation in a concrete live pane.
    ActivePane { locator: PaneViewLocator },
    /// An indexed workspace / CLI-agent session row.
    WorkspaceTarget {
        target: WorkspaceSessionActionTarget,
    },
}

#[derive(Debug, Clone)]
pub enum WorkspaceAction {
    ActivateTab(usize),
    ActivatePrevTab,
    ActivateNextTab,
    ActivateLastTab,
    CyclePrevSession,
    CycleNextSession,
    MoveActiveTabLeft,
    MoveActiveTabRight,
    MoveTabLeft(usize),
    MoveTabRight(usize),
    RenameTab(usize),
    ResetTabName(usize),
    RenamePane(PaneViewLocator),
    ResetPaneName(PaneViewLocator),
    RenameActiveTab,
    SetActiveTabName(String),
    /// Sets the manual color override for the active tab.
    ///
    /// - `Color(_)` — apply that color.
    /// - `Cleared` — explicitly clear (suppresses any directory default).
    /// - `Unset` — remove the manual override (lets the directory default apply, if any).
    SetActiveTabColor(SelectedTabColor),
    ToggleTabRightClickMenu {
        tab_index: usize,
        anchor: TabContextMenuAnchor,
    },
    ToggleVerticalTabsPaneContextMenu {
        tab_index: usize,
        target: VerticalTabsPaneContextMenuTarget,
        position: Vector2F,
    },
    ShowWorkspaceSessionContextMenu {
        target: WorkspaceSessionActionTarget,
        position: Vector2F,
    },
    TabHoverWidthStart {
        width: f32,
    },
    TabHoverWidthEnd,
    ToggleTabBarOverflowMenu,
    ToggleWelcomeTips,
    CloseTab(usize),
    CloseActiveTab,
    CloseOtherTabs(usize),
    CloseNonActiveTabs,
    CloseTabsRight(usize),
    CloseTabsRightActiveTab,
    AddDefaultTab,
    AddTerminalTab {
        hide_homepage: bool,
    },
    /// 打开当前 provider 对应的 Environment Runtime terminal。
    /// 由 provider source 的 Connect 按钮 / 管理面板右键“连接”触发；
    /// 不应创建一个可见的 terminal-bootstrap wrapper pane。
    OpenEnvironmentRuntimeTerminal {
        target: EnvironmentProviderTarget,
    },
    /// 将 provider target 打开为 Ashide Environment；不立即拨号，先进入 dormant
    /// 环境状态，等待用户显式 reconnect / connect。
    OpenEnvironmentRuntime {
        target: EnvironmentProviderTarget,
    },
    /// 对当前 Environment 执行显式 reconnect。底层 transport 由 provider 决定。
    ReconnectCurrentEnvironment,
    /// 切换到当前窗口内已打开的 Environment。
    SwitchEnvironment {
        authority_key: String,
    },
    /// 显式断开并释放当前窗口内的 Environment 标签。
    DisconnectEnvironment {
        authority_key: String,
    },
    /// Deterministically show or hide restored workspace session metadata.
    SetWorkspaceSessionRestorePopoverOpen {
        open: bool,
    },
    /// User explicitly selected a restored workspace session from the Workspace Navigator.
    ActivateRestoredWorkspaceSession {
        target: WorkspaceSessionActionTarget,
    },
    /// User requested a custom display alias for a workspace/CLI-agent session.
    RequestRenameWorkspaceSession {
        target: WorkspaceSessionActionTarget,
    },
    /// User explicitly cleared the custom display alias for a workspace/CLI-agent session.
    ClearWorkspaceSessionAlias {
        target: WorkspaceSessionActionTarget,
    },
    /// User requested to copy the session's stable identifier to the clipboard
    /// (SSTAB-007 discoverability: short id / cwd / authority cues).
    CopyWorkspaceSessionId {
        target: WorkspaceSessionActionTarget,
    },
    /// User requested permanent deletion; must show confirmation before deleting files.
    RequestDeleteWorkspaceSession {
        target: WorkspaceSessionActionTarget,
    },
    /// User explicitly confirmed permanent deletion of a persisted workspace/CLI-agent session.
    DeleteWorkspaceSession {
        target: WorkspaceSessionActionTarget,
    },
    /// 设置 Environment Strip provider 快速选择器的显示状态。
    SetEnvironmentProviderPickerOpen {
        open: bool,
    },
    /// 直接从 provider 候选打开 Environment Runtime。
    OpenEnvironmentProviderCandidate {
        alias: String,
    },
    /// 聚焦左侧 panel 的 provider 管理视图；当前实现委托默认 provider 管理 UI。
    OpenEnvironmentProviderManager,
    /// 打开/关闭左侧 panel 的 Environment provider 管理视图。
    ToggleEnvironmentProviderManager,
    /// 打开/关闭左侧 panel 的 Skill 管理器视图(Ashide 独有)。
    ToggleSkillManager,
    AddTabWithShell {
        shell: AvailableShell,
    },
    AddGetStartedTab,
    /// Add a new tab that immediately enters agent view with a new conversation.
    AddAgentTab,
    /// Add a new terminal tab and launch a specific CLI agent.
    AddSpecificAgentTab(CLIAgent),
    /// Add a new tab running a local Docker sandbox via `sbx`.
    AddDockerSandboxTab,
    OpenNewSessionMenu {
        position: Vector2F,
    },
    ToggleTabConfigsMenu,
    ToggleNewSessionMenu {
        position: Vector2F,
        is_vertical_tabs: bool,
    },
    SelectNewSessionMenuItem(NewSessionMenuItem),
    AutoupdateFailureLink,
    ApplyUpdate,
    // 本地/去中心化构建无云端账号,故无 `LogOut` 动作。
    CopyVersion(&'static str),
    DownloadNewVersion,
    ConfigureKeybindingSettings {
        keybinding_name: Option<String>,
    },
    ShowSettings,
    ShowSettingsPage(SettingsSection),
    ShowSettingsPageWithSearch {
        search_query: String,
        section: Option<SettingsSection>,
    },
    ShowThemeChooser(ThemeChooserMode),
    ShowThemeChooserForActiveTheme,
    IncreaseFontSize,
    DecreaseFontSize,
    ResetFontSize,
    IncreaseZoom,
    DecreaseZoom,
    ResetZoom,
    ActivateTabByNumber(usize),
    OpenPalette {
        mode: PaletteMode,
        source: PaletteSource,
        query: Option<String>,
    },
    TogglePalette {
        mode: PaletteMode,
        source: PaletteSource,
    },
    // 本地/去中心化构建无云端账号,故无 `ShowUpgrade` / `ShowReferralSettingsPage` 动作。
    JoinSlack,
    ViewUserDocs,
    ViewLatestChangelog,
    ViewPrivacyPolicy,
    SendFeedback,
    /// Open the log directory in the system file explorer with the current log file selected.
    #[cfg(not(target_family = "wasm"))]
    ViewLogs,
    /// Prompt the user with a native save-file dialog and write the log
    /// bundle (recent logs + MCP / update logs + diagnostic manifest) to
    /// the chosen path. Used by the "Export logs" link on the About page.
    #[cfg(not(target_family = "wasm"))]
    ExportLogsToPath,
    ChangeCursor(Cursor),
    ToggleBlockSnackbar,
    ToggleErrorUnderlining,
    ToggleSyntaxHighlighting,
    CheckForUpdate,
    ExportAllLocalDriveObjects,
    SetA11yVerbosityLevel(AccessibilityVerbosity),
    ToggleNotifications,
    ToggleTabColor {
        color: AnsiColorIdentifier,
        tab_index: usize,
    },
    OpenLaunchConfigSaveModal,
    SelectTabConfig(TabConfig),
    DispatchToSettingsTab(SettingsTabAction),
    ToggleResourceCenter,
    ToggleUserMenu,
    ToggleAIAssistant,
    ClickedAIAssistantIcon,
    ToggleKeybindingsPage,
    ShowCommandSearch(CommandSearchOptions),
    CreatePersonalNotebook,
    ImportToPersonalDrive,
    CreatePersonalWorkflow,
    CreatePersonalFolder,
    CreatePersonalEnvVarCollection,
    CreatePersonalAIPrompt,
    ToggleMouseReporting,
    ToggleScrollReporting,
    ToggleFocusReporting,
    StartTabDrag,
    DragTab {
        tab_index: usize,
        tab_position: RectF,
    },
    DropTab,
    /// Toggles the left panel. In Code Mode V1 this toggles Ashide Drive.
    /// In Code Mode V2 this toggles the left panel which contains both the project explorer and
    /// Ashide Drive. This happens as explicit action from the user.
    ToggleLeftPanel,
    /// Toggles directly to the Ashide Drive tab of the left panel in Code Mode V2
    ToggleLocalDrive,
    /// Unconditionally opens Ashide Drive. This is used in the case of user lifecycle
    /// events like new user onboarding or when the user joins a team.
    LocalDrive,
    /// Toggles the right panel. This happens as an explicit action from the user.
    ToggleRightPanel,
    /// Opens the code review panel (right panel) without toggling. If already open,
    /// switches to the target pane's repo. Used by vertical tabs diff stats chip.
    OpenCodeReviewPanel(PaneViewLocator),
    /// Toggles the vertical tabs panel. This happens as an explicit action from the user.
    ToggleVerticalTabsPanel,
    /// Re-scan current-app and Environment CLI-agent session history and refresh the Session Navigator.
    RefreshWorkspaceSessions,
    ToggleWorkspaceSessionPinned {
        target: WorkspaceSessionActionTarget,
        pinned: bool,
    },
    /// Closes the focused panel. This happens as an explicit action from the user.
    ClosePanel,
    CopyTextToClipboard(String),
    /// An action only registered in dev and local builds, which writes the user's current access
    /// token to the system clipboard to aid debugging and development.
    CopyAccessTokenToClipboard,
    DismissWorkspaceBanner(WorkspaceBanner),
    /// An action only registered in dev and local builds, which crashes the
    /// 调用后立即触发 app crash。
    Crash,
    /// An action only registered in dev and local builds, which triggers a
    /// panic immediately when called.
    Panic,
    /// Stops the heap profiler (if one is running) and writes the profiling
    /// data to disk.
    DumpHeapProfile,
    ShowAIAssistantWarmWelcome,
    ClickedAIAssistantWarmWelcome,
    /// An action to open a new window with a view hierarchy debugger.
    OpenViewTreeDebugWindow,
    DismissAIAssistantWarmWelcome,
    /// An action to either upgrade syncing status from none or just in one tab
    /// to syncing all tabs, or downgrade from syncing all tabs to no syncing
    ToggleSyncAllTerminalInputsInAllTabs,
    /// An action to either cancel syncing
    /// or switch from no syncing/syncing all tabs to syncing within one tab
    ToggleSyncTerminalInputsInTab,
    /// An action to force terminal input syncing off
    DisableTerminalInputSync,
    HandleConflictingWorkflow(ObjectStoreId),
    HandleConflictingEnvVarCollection(ObjectStoreId),
    OpenPromptEditor,
    OpenAgentToolbarEditor,
    OpenCLIAgentToolbarEditor,
    OpenHeaderToolbarEditor,
    ShowHeaderToolbarContextMenu {
        position: Vector2F,
    },
    // 本地/去中心化构建无云端账号,故无 `Reauth` / `SignupAnonymousUser` / `SignInAnonymousWebUser` 动作。
    OpenLink(String),
    /// On WASM, opens a given URL in the desktop Ashide app (if installed) or redirects to download page.
    #[cfg(target_family = "wasm")]
    OpenLinkOnDesktop(url::Url),
    ReopenClosedSession,
    AddWindow,
    AddWindowWithShell {
        shell: AvailableShell,
    },
    /// Moves focus to the panel on the left
    FocusLeftPanel,
    /// Moves focus to the panel on the right
    FocusRightPanel,
    /// An action to view a newly created/edited workflow in WD from the toast
    ViewObjectInLocalDrive(LocalDriveItemId),
    UndoTrash(ObjectTypeAndId),
    /// Open a current-app path in the file explorer.
    OpenInExplorer {
        path: PathBuf,
    },
    /// Open a current-app file with the system's default application.
    OpenFilePath {
        path: PathBuf,
    },
    TerminateApp,
    CloseWindow,
    /// Help the user call the Ashide executable with the [`crate::args::DEBUG_DUMP_FLAG`].
    DumpDebugInfo,
    /// Log review comment send eligibility for panes in the active tab.
    LogReviewCommentSendStatusForActiveTab,
    ToggleRecordingMode,
    ToggleInBandGenerators,
    ToggleDebugNetworkStatus,
    ToggleShowMemoryStats,
    RunAISuggestedCommand(String),
    RunCommand(String),
    InsertInInput {
        content: String,
        replace_buffer: bool,
        /// Whether to ensure agent mode is enabled when inserting content
        ensure_agent_mode: bool,
    },
    /// Open a new tab with its input in AI mode.
    NewTabInAgentMode {
        /// The type of zero state prompt suggestion to start with (optional).
        zero_state_prompt_suggestion_type: Option<ZeroStatePromptSuggestionType>,
    },
    /// Open a new pane with its input in AI mode.
    NewPaneInAgentMode {
        /// The type of zero state prompt suggestion to start with (optional).
        zero_state_prompt_suggestion_type: Option<ZeroStatePromptSuggestionType>,
    },
    // 本地/去中心化构建无云端账号,故无 `AttemptLoginGatedAIUpgrade` 动作。
    /// Dismisses the Wayland crash recovery banner and opens a link to our docs page with more
    /// information.
    #[cfg(target_os = "linux")]
    DismissWaylandCrashRecoveryBannerAndOpenLink,
    /// Open a new pane with its input in AI mode
    /// with query "Fix this" with error name and details from AI summary.
    FixInAgentMode {
        query: String,
    },
    OpenAIFactCollection,
    OpenMCPServerCollection,
    ToggleAIDocumentPane {
        document_id: AIDocumentId,
        document_version: AIDocumentVersion,
    },
    /// Closes all visible AI document panes in the active pane group.
    HideAIDocumentPanes,
    /// Closes any other ai document panes in the active pane group, and opens the specified document_id.
    OpenAIDocumentPane {
        document_id: AIDocumentId,
        document_version: AIDocumentVersion,
    },
    FocusTerminalViewInWorkspace {
        terminal_view_id: EntityId,
    },
    /// Focus a specific pane by its locator (pane_group_id and pane_id).
    FocusPane(PaneViewLocator),
    /// Start a new AI conversation in a terminal view. This sets the pending query state
    /// to default and focuses the terminal view.
    StartNewConversation {
        terminal_view_id: EntityId,
    },
    /// Jump to the terminal pane of the most recent agent toast
    JumpToLatestToast,
    /// Open a file in a new tab with a code pane
    OpenFileInNewTab {
        full_path: PathBuf,
        line_and_column: Option<LineAndColumnArg>,
    },
    OpenNotebook {
        id: ObjectStoreId,
    },
    RunWorkflow {
        workflow: Arc<WorkflowType>,
        workflow_source: WorkflowSource,
        workflow_selection_source: WorkflowSelectionSource,
        argument_override: Option<HashMap<String, String>>,
    },
    ScrollToSettingsWidget {
        page: SettingsSection,
        widget_id: &'static str,
    },
    /// Navigate to an existing AI conversation, focusing on its terminal view.
    ///
    /// If the conversation is not in an open pane, restore it based on the layout setting or override.
    RestoreOrNavigateToConversation {
        pane_view_locator: Option<PaneViewLocator>,
        window_id: Option<WindowId>,
        conversation_id: AIConversationId,
        terminal_view_id: Option<EntityId>,
        /// If provided, use this layout to restore the conversation.
        /// Otherwise, fall back to the user's setting.
        restore_layout: Option<RestoreConversationLayout>,
    },
    /// Fork an existing AI conversation.
    /// Optionally summarizes the conversation after forking and/or sends an initial prompt.
    ForkAIConversation {
        conversation_id: AIConversationId,
        /// When Some, fork from the given response (or exchange if `fork_from_exact_exchange`
        /// is true). When None, fork from the last exchange.
        fork_from_exchange: Option<ForkFromExchange>,
        /// Whether to summarize the conversation after forking.
        summarize_after_fork: bool,
        /// Prompt to use for summarization when `summarize_after_fork` is true.
        summarization_prompt: Option<String>,
        /// Initial prompt to send in the forked conversation (sent after summarization if enabled).
        initial_prompt: Option<String>,
        /// Where to open the forked conversation.
        destination: ForkedConversationDestination,
    },
    /// Fork a session (conversation / active pane / workspace row) into a new
    /// native session-history entry.
    ForkSessionBridge {
        source: SessionBridgeActionSource,
        fork_target: SessionBridgeForkTarget,
    },
    /// Open the edit-and-fork dialog for a session.
    ShowSessionBridgeEditDialog {
        source: SessionBridgeActionSource,
    },
    /// Export a session to a portable SessionBridge bundle.
    ExportSessionBridgeBundle {
        source: SessionBridgeActionSource,
    },
    /// Fork an existing AI conversation into a new pane and prefill the input with a current-app
    /// continuation command (selecting all text).
    #[cfg(not(target_family = "wasm"))]
    ContinueConversationInCurrentApp {
        conversation_id: AIConversationId,
    },
    /// Insert the /fork slash command into the active terminal's input.
    InsertForkSlashCommand,
    /// Summarize the active AI conversation in the focused pane.
    SummarizeAIConversation {
        prompt: Option<String>,
        /// Optional prompt to send after summarization completes successfully.
        initial_prompt: Option<String>,
    },
    /// Queue a prompt to be sent after the current conversation finishes.
    QueuePromptForConversation {
        prompt: String,
    },
    /// Install the Ashide CLI command to /usr/local/bin
    #[cfg(target_os = "macos")]
    InstallCLI,
    /// Uninstall the Ashide CLI command from /usr/local/bin
    #[cfg(target_os = "macos")]
    UninstallCLI,
    UndoRevertInCodeReviewPane {
        window_id: WindowId,
        view_id: EntityId,
    },
    /// Handle a file being renamed in the file tree
    #[cfg(feature = "local_fs")]
    FileRenamed {
        old_path: PathBuf,
        new_path: PathBuf,
    },
    /// Handle a file being deleted in the file tree
    #[cfg(feature = "local_fs")]
    FileDeleted {
        path: PathBuf,
    },
    /// Open a repository directory via file picker. The `path` is an `Option` because some
    /// dispatchers don't know the path to open yet (so the Workspace must open the file picker)
    /// and some do, e.g. the GetStartedView. The GetStartedView needs to handle the file picker
    /// because it needs to determine whether or not to close itself based on whether the user
    /// actually selects a file in the file picker or cancels it.
    OpenRepository {
        path: Option<String>,
    },
    /// Open the native folder picker for a repo param in the tab-config modal after the
    /// current interaction cycle finishes.
    OpenTabConfigRepoPicker {
        param_index: usize,
    },
    /// Open a new blank code file in the current tab
    NewCodeFile,
    NavigatePrevPaneOrPanel,
    NavigateNextPaneOrPanel,
    ToggleProjectExplorer,
    ToggleGlobalSearch,
    OpenGlobalSearch,
    /// Reset the AWS Bedrock login banner dismissed state (for debugging).
    #[cfg(debug_assertions)]
    DebugResetAwsBedrockLoginBannerDismissed,
    /// Open the Ashide Launch Modal (for debugging)
    #[cfg(debug_assertions)]
    OpenAshideLaunchModal,
    /// Reset the Ashide launch modal dismissed state (for debugging)
    #[cfg(debug_assertions)]
    ResetAshideLaunchModalState,
    /// Install the opencode-warp plugin from GitHub into the global opencode config.
    #[cfg(debug_assertions)]
    InstallOpenCodeWarpPlugin,
    /// Use a development checkout of the opencode-warp plugin (for testing/development).
    #[cfg(debug_assertions)]
    UseDevCheckoutOpenCodeWarpPlugin,
    /// Take a process sample of the app (equivalent to Activity Monitor > Sample Process).
    #[cfg(target_os = "macos")]
    SampleProcess,
    ToggleNotificationMailbox {
        select_first: bool,
    },
    /// Show the rewind confirmation dialog before rewinding an AI conversation
    ShowRewindConfirmationDialog {
        ai_block_view_id: EntityId,
        exchange_id: AIAgentExchangeId,
        conversation_id: AIConversationId,
    },
    /// Execute the actual rewind after confirmation
    ExecuteRewindAIConversation {
        ai_block_view_id: EntityId,
        exchange_id: AIAgentExchangeId,
        conversation_id: AIConversationId,
    },
    /// Execute the actual deletion of a conversation after confirmation
    ExecuteDeleteConversation {
        conversation_id: AIConversationId,
        terminal_view_id: Option<EntityId>,
    },
    /// Open an ambient agent session by joining its shared session.
    /// Used when the sandbox is running or when we need to view a live session.
    OpenAmbientAgentSession {
        task_id: AmbientAgentTaskId,
    },
    /// Load conversation data into a transcript viewer.
    /// Used for persisted view-only conversations.
    OpenConversationTranscriptViewer {
        conversation_id: ServerConversationToken,
        ambient_agent_task_id: Option<AmbientAgentTaskId>,
    },
    /// Toggle the conversation transcript details panel (WASM-only).
    #[cfg(target_family = "wasm")]
    ToggleConversationTranscriptDetailsPanel,
    /// Open a full-window lightbox displaying the given images.
    OpenLightbox {
        images: Vec<lightbox::LightboxImage>,
        /// The index of the image to display initially.
        initial_index: usize,
    },
    /// Update a single image in the currently open lightbox.
    UpdateLightboxImage {
        index: usize,
        image: lightbox::LightboxImage,
    },
    StartAgentOnboardingTutorial(OnboardingTutorial),
    ShowSessionConfigModal,
    DismissSessionConfigTabConfigChip,
    /// Start the HOA onboarding flow (for debugging)
    #[cfg(debug_assertions)]
    ShowHoaOnboardingFlow,
    /// Open the "New worktree" modal for creating a reusable worktree tab config.
    OpenNewWorktreeModal,
    /// Open the native folder picker for the repo field in the new-worktree modal.
    OpenNewWorktreeRepoPicker,
    /// Create a new worktree in the given repo using the default worktree tab config.
    /// The branch name is auto-generated.
    OpenWorktreeInRepo {
        repo_path: String,
    },
    SaveCurrentTabAsNewConfig(usize),
    SyncTrafficLights,
    /// Opens a tab config file in the editor and dismisses the associated error toast.
    OpenTabConfigErrorFile {
        path: PathBuf,
        toast_object_id: String,
    },
    /// Sidecar action: set the hovered item as the Cmd+T default.
    TabConfigSidecarMakeDefault {
        mode: crate::settings::ai::DefaultSessionMode,
        tab_config_path: Option<PathBuf>,
        shell: Option<AvailableShell>,
    },
    /// Sidecar action: open the tab config TOML in the user's editor.
    TabConfigSidecarEditConfig {
        path: PathBuf,
    },
    /// Sidecar action: show the remove confirmation dialog for a tab config.
    TabConfigSidecarRemoveConfig {
        name: String,
        path: PathBuf,
    },
    /// Opens the settings.toml file in a code editor pane.
    OpenSettingsFile,
    /// Opens a new agent session to fix settings.toml errors using the modify-settings skill.
    FixSettingsWithOz {
        error_description: String,
    },
}

impl From<&WorkspaceAction> for LoginGatedFeature {
    fn from(val: &WorkspaceAction) -> LoginGatedFeature {
        let _ = val;
        "Unknown reason"
    }
}

impl WorkspaceAction {
    pub fn blocked_for_anonymous_user(&self) -> bool {
        false
    }

    /// Matches what actions require the app state to be saved, and which don't. We match all
    /// actions directly, rather than using _, so we're forced to make a conscious decision for each
    /// of them, rather than following some default.
    pub fn should_save_app_state_on_action(&self) -> bool {
        use WorkspaceAction::*;
        match self {
            #[cfg(not(target_family = "wasm"))]
            ContinueConversationInCurrentApp { .. } => true,
            ActivateTab(_)
            | ActivateTabByNumber(_)
            | ActivatePrevTab
            | ActivateNextTab
            | ActivateLastTab
            | CyclePrevSession
            | CycleNextSession
            | MoveActiveTabLeft
            | MoveActiveTabRight
            | MoveTabLeft(_)
            | MoveTabRight(_)
            | DropTab
            | RenameTab(_)
            | ResetTabName(_)
            | RenamePane(_)
            | ResetPaneName(_)
            | RenameActiveTab
            | SetActiveTabName(_)
            | SetActiveTabColor(_)
            | CloseTab(_)
            | CloseActiveTab
            | CloseOtherTabs(_)
            | CloseNonActiveTabs
            | CloseTabsRight(_)
            | CloseTabsRightActiveTab
            | ToggleTabColor { .. }
            | AddDefaultTab
            | AddTerminalTab { .. }
            | OpenEnvironmentRuntimeTerminal { .. }
            | OpenEnvironmentRuntime { .. }
            | ReconnectCurrentEnvironment
            | SwitchEnvironment { .. }
            | DisconnectEnvironment { .. }
            | SetEnvironmentProviderPickerOpen { .. }
            | OpenEnvironmentProviderCandidate { .. }
            | OpenEnvironmentProviderManager
            | ToggleEnvironmentProviderManager
            | ToggleSkillManager
            | AddTabWithShell { .. }
            | AddGetStartedTab
            | AddAgentTab
            | AddSpecificAgentTab(_)
            | AddDockerSandboxTab
            | AddWindow
            | AddWindowWithShell { .. }
            | CloseWindow
            | ScrollToSettingsWidget { .. }
            | NewTabInAgentMode { .. }
            | NewPaneInAgentMode { .. }
            | FixInAgentMode { .. }
            | OpenNotebook { .. }
            | RunWorkflow { .. }
            | OpenFileInNewTab { .. }
            | RestoreOrNavigateToConversation { .. }
            | NewCodeFile
            | ForkAIConversation { .. }
            | ForkSessionBridge { .. }
            | ShowSessionBridgeEditDialog { .. }
            | ExportSessionBridgeBundle { .. }
            | SummarizeAIConversation { .. }
            | OpenRepository { .. }
            | SelectTabConfig(_)
            | ToggleVerticalTabsPanel => true, // actions that actually change a state of the state of user's
            // workspace would most likely require a save, so that if the app gets
            // restarted, the user can continue working
            AutoupdateFailureLink
            | ApplyUpdate
            | CopyVersion(_)
            | DownloadNewVersion
            | ConfigureKeybindingSettings { .. }
            | ExportAllLocalDriveObjects
            | ShowSettings
            | ShowSettingsPage(_)
            | ShowSettingsPageWithSearch { .. }
            | ShowThemeChooser(_)
            | ShowThemeChooserForActiveTheme
            | IncreaseFontSize
            | DecreaseFontSize
            | ResetFontSize
            | IncreaseZoom
            | DecreaseZoom
            | ResetZoom
            | OpenPalette { .. }
            | TogglePalette { mode: _, source: _ }
            | JoinSlack
            | ViewUserDocs
            | ViewLatestChangelog
            | ViewPrivacyPolicy
            | SendFeedback
            | ChangeCursor(_)
            | ToggleBlockSnackbar
            | ToggleErrorUnderlining
            | ToggleSyntaxHighlighting
            | OpenLaunchConfigSaveModal
            | ToggleTabRightClickMenu { .. }
            | ToggleVerticalTabsPaneContextMenu { .. }
            | ShowWorkspaceSessionContextMenu { .. }
            | OpenNewSessionMenu { .. }
            | ToggleTabConfigsMenu
            | ToggleNewSessionMenu { .. }
            | SelectNewSessionMenuItem(_)
            | ToggleTabBarOverflowMenu
            | CheckForUpdate
            | SetA11yVerbosityLevel(_)
            | ToggleNotifications
            | DispatchToSettingsTab { .. }
            | ToggleResourceCenter
            | ToggleUserMenu
            | ClickedAIAssistantIcon
            | ToggleAIAssistant
            | ToggleKeybindingsPage
            | ShowCommandSearch(_)
            | ToggleMouseReporting
            | ToggleScrollReporting
            | ToggleFocusReporting
            | ImportToPersonalDrive
            | CreatePersonalNotebook
            | CreatePersonalWorkflow
            | CreatePersonalFolder
            | CreatePersonalEnvVarCollection
            | CreatePersonalAIPrompt
            | OpenInExplorer { .. }
            | DragTab { .. }
            | StartTabDrag
            | ToggleLeftPanel
            | ToggleLocalDrive
            | LocalDrive
            | ClosePanel
            | ToggleRightPanel
            | OpenCodeReviewPanel(..)
            | ToggleWelcomeTips
            | CopyTextToClipboard(_)
            | CopyAccessTokenToClipboard
            | OpenTabConfigRepoPicker { .. }
            | OpenNewWorktreeModal
            | OpenNewWorktreeRepoPicker
            | OpenWorktreeInRepo { .. }
            | Crash
            | Panic
            | DumpHeapProfile
            | OpenViewTreeDebugWindow
            | ShowAIAssistantWarmWelcome
            | ClickedAIAssistantWarmWelcome
            | DismissAIAssistantWarmWelcome
            | DismissWorkspaceBanner(..)
            | ToggleSyncAllTerminalInputsInAllTabs
            | ToggleSyncTerminalInputsInTab
            | DisableTerminalInputSync
            | HandleConflictingWorkflow(_)
            | HandleConflictingEnvVarCollection(_)
            | OpenPromptEditor
            | OpenAgentToolbarEditor
            | OpenCLIAgentToolbarEditor
            | OpenHeaderToolbarEditor
            | ShowHeaderToolbarContextMenu { .. }
            | OpenLink(_)
            | ReopenClosedSession
            | FocusLeftPanel
            | FocusRightPanel
            | DumpDebugInfo
            | LogReviewCommentSendStatusForActiveTab
            | ToggleRecordingMode
            | ToggleInBandGenerators
            | ToggleDebugNetworkStatus
            | ToggleShowMemoryStats
            | RunAISuggestedCommand { .. }
            | RunCommand { .. }
            | InsertInInput { .. }
            | InsertForkSlashCommand
            | QueuePromptForConversation { .. }
            | UndoTrash(_)
            | OpenFilePath { .. }
            | ViewObjectInLocalDrive(_)
            | TerminateApp
            | TabHoverWidthStart { .. }
            | TabHoverWidthEnd
            | OpenAIFactCollection
            | OpenMCPServerCollection
            | FocusTerminalViewInWorkspace { .. }
            | FocusPane(..)
            | StartNewConversation { .. }
            | UndoRevertInCodeReviewPane { .. }
            | JumpToLatestToast
            | NavigatePrevPaneOrPanel
            | NavigateNextPaneOrPanel
            | ToggleProjectExplorer
            | ToggleGlobalSearch
            | OpenGlobalSearch
            | ToggleNotificationMailbox { .. }
            | ToggleAIDocumentPane { .. }
            | HideAIDocumentPanes
            | OpenAIDocumentPane { .. }
            | ShowRewindConfirmationDialog { .. }
            | ExecuteRewindAIConversation { .. }
            | ExecuteDeleteConversation { .. }
            | OpenAmbientAgentSession { .. }
            | OpenConversationTranscriptViewer { .. }
            | OpenLightbox { .. }
            | UpdateLightboxImage { .. }
            | StartAgentOnboardingTutorial(_)
            | ShowSessionConfigModal
            | DismissSessionConfigTabConfigChip
            | SetWorkspaceSessionRestorePopoverOpen { .. }
            | RefreshWorkspaceSessions
            | ToggleWorkspaceSessionPinned { .. }
            | ActivateRestoredWorkspaceSession { .. }
            | RequestRenameWorkspaceSession { .. }
            | ClearWorkspaceSessionAlias { .. }
            | CopyWorkspaceSessionId { .. }
            | RequestDeleteWorkspaceSession { .. }
            | DeleteWorkspaceSession { .. }
            | SaveCurrentTabAsNewConfig(_)
            | SyncTrafficLights
            | OpenTabConfigErrorFile { .. }
            | TabConfigSidecarMakeDefault { .. }
            | TabConfigSidecarEditConfig { .. }
            | TabConfigSidecarRemoveConfig { .. }
            | OpenSettingsFile
            | FixSettingsWithOz { .. } => false,
            #[cfg(debug_assertions)]
            ShowHoaOnboardingFlow => false,
            #[cfg(target_family = "wasm")]
            ToggleConversationTranscriptDetailsPanel => false,
            #[cfg(debug_assertions)]
            DebugResetAwsBedrockLoginBannerDismissed
            | OpenAshideLaunchModal
            | ResetAshideLaunchModalState
            | InstallOpenCodeWarpPlugin
            | UseDevCheckoutOpenCodeWarpPlugin => false,
            #[cfg(not(target_family = "wasm"))]
            ViewLogs => false,
            #[cfg(not(target_family = "wasm"))]
            ExportLogsToPath => false,
            #[cfg(target_os = "macos")]
            SampleProcess => false,
            #[cfg(target_os = "macos")]
            InstallCLI | UninstallCLI => false,
            #[cfg(feature = "local_fs")]
            FileRenamed { .. } => false, // File rename doesn't change workspace state
            #[cfg(feature = "local_fs")]
            FileDeleted { .. } => false, // File deletion doesn't change workspace state
            #[cfg(target_os = "linux")]
            DismissWaylandCrashRecoveryBannerAndOpenLink => false,
            #[cfg(target_family = "wasm")]
            OpenLinkOnDesktop(_) => false,
            // actions that are related to updating user settings or
            // managing some ui elements (like closing/opening modals)
            // that don't reflect on actual workspace and don't need to
            // be preserved between restarts.
        }
    }
}

#[cfg(test)]
#[path = "action_tests.rs"]
mod tests;
