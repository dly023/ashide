use super::*;
use crate::ai::blocklist::{BlocklistAIHistoryModel, BlocklistAIPermissions};
use crate::ai::document::ai_document_model::AIDocumentModel;
use crate::ai::execution_profiles::profiles::AIExecutionProfilesModel;
use crate::ai::facts::manager::AIFactManager;
use crate::ai::llms::LLMPreferences;
use crate::ai::restored_conversations::RestoredAgentConversations;
use crate::ai::skills::environment_skill_inventory::EnvironmentSkillInventoryCache;
use crate::ai::skills::SkillManager;
use crate::ai::AIRequestUsageModel;
use crate::app_state::EnvironmentKind;
use crate::auth::UserUid;
use crate::context_chips::prompt::Prompt;
use crate::editor::Event;
use crate::editor::ReplicaId;
use crate::gpu_state::GPUState;
use crate::network::NetworkStatus;
use crate::notebooks::editor::keys::NotebookKeybindings;
use crate::notebooks::notebook::NotebookView;
use crate::object_store::model::persistence::ObjectStoreModel;
use crate::object_store::model::view::ObjectStoreViewModel;
use crate::pane_group::{Direction, PaneGroupAction, PaneId};
use crate::suggestions::ignored_suggestions_model::IgnoredSuggestionsModel;
use crate::terminal::shared_session::protocol::SessionSourceType;
use crate::terminal::shared_session::protocol::{ParticipantId, ParticipantList};
#[cfg(feature = "local_fs")]
use crate::user_config::tab_configs_dir;
use repo_metadata::repositories::DetectedRepositories;
use repo_metadata::watcher::DirectoryWatcher;
#[cfg(feature = "local_fs")]
use repo_metadata::RepoMetadataModel;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use watcher::HomeDirectoryWatcher;

use crate::object_store::update_manager::UpdateManager;
use crate::server::experiments::ServerExperiments;

use crate::settings::{AutoupdateSettings, PrivacySettings};
use crate::settings_view::keybindings::KeybindingChangedNotifier;
use crate::settings_view::DisplayCount;
use crate::system::SystemStats;
use crate::tab_configs::tab_config::{TabConfigPaneNode, TabConfigPaneType};
use crate::terminal::history::History;
use crate::terminal::keys::TerminalKeybindings;
#[cfg(windows)]
use crate::util::traffic_lights::windows::RendererState;
use crate::workspaces::user_profiles::UserProfiles;
use crate::workspaces::user_workspaces::UserWorkspaces;

use crate::terminal::local_tty::spawner::PtySpawner;
use crate::terminal::shared_session::SharedSessionStatus;

use crate::ai::agent_conversations_model::AgentConversationsModel;
use crate::ai::ambient_agents::github_auth_notifier::GitHubAuthNotifier;
use crate::ai::mcp::{
    gallery::MCPGalleryManager, templatable_manager::TemplatableMCPServerManager,
    FileBasedMCPManager, FileMCPWatcher,
};
use crate::resource_center::Tip;
use crate::terminal::cli_agent_sessions::CLIAgentSessionsModel;
use crate::test_util::settings::initialize_settings_for_tests;
use crate::undo_close::UndoCloseSettings;
use crate::warp_managed_paths_watcher::WarpManagedPathsWatcher;
use crate::workflows::local_workflows::LocalWorkflows;
use crate::ObjectActions;
use crate::{experiments, workspace, GlobalResourceHandlesProvider};

// Ashide(本地化,Phase 5):`PreferencesSyncer` 已物理删除。

use crate::terminal::shared_session::protocol::SessionId;
use crate::test_util::ai_agent_tasks::{create_api_task, create_message};
use ai::project_context::model::ProjectContextModel;
use pane_group::{
    EnvironmentRuntimePtyProcess, NotebookPane, PaneState, SplitPaneState, TerminalPaneId,
};
use persistence::model::AgentConversationData;
use terminal::view::ActiveSessionState;
use warp_core::{HostId, SessionId as CoreSessionId};
use warp_multi_agent_api as api;
use warpui::AddSingletonModel;
use warpui::{platform::WindowStyle, App, ViewHandle};

fn initialize_app(app: &mut App) {
    // Several workspace tests assert resolved i18n labels (menu items, primary-line
    // text). The loader lives in a global `OnceLock`, so without an explicit init
    // here a test only sees resolved strings when some *other* test happened to call
    // `i18n::init` first in the same process — passing under the full parallel run but
    // failing in isolation. `init` is idempotent, so pin it to English for determinism.
    crate::i18n::init(Some("en"));

    initialize_settings_for_tests(app);

    // Add the necessary singleton models to the App
    app.add_singleton_model(|_| AuthStateProvider::new_for_test());
    app.add_singleton_model(AuthManager::new_for_test);
    app.add_singleton_model(|_ctx| PtySpawner::new_for_test());
    app.add_singleton_model(|_| Prompt::mock());
    app.add_singleton_model(|_| AutoupdateState::new(Arc::new(http_client::Client::new())));
    app.add_singleton_model(|_| NetworkStatus::new());
    app.add_singleton_model(|_| SystemStats::new());
    app.add_singleton_model(ObjectStoreModel::mock);
    app.add_singleton_model(UserWorkspaces::default_mock);
    app.add_singleton_model(|_ctx| UserProfiles::new(Vec::new()));
    app.add_singleton_model(UpdateManager::mock);
    app.add_singleton_model(MCPGalleryManager::new);
    app.add_singleton_model(ObjectStoreViewModel::mock);
    app.add_singleton_model(|_| Appearance::mock());
    app.add_singleton_model(AppearanceManager::new);
    app.add_singleton_model(|_| DisplayCount::mock());
    app.add_singleton_model(PrivacySettings::mock);
    app.add_singleton_model(|_| KeybindingChangedNotifier::new());
    app.add_singleton_model(|_ctx| RelaunchModel::new());
    app.add_singleton_model(|_| ChangelogModel::new(Arc::new(http_client::Client::new())));
    app.add_singleton_model(|_| GitHubAuthNotifier::new());
    app.add_singleton_model(|_| crate::ssh_manager::SshTreeChangedNotifier::new());
    app.add_singleton_model(|_ctx| SyncedInputState::mock());
    app.add_singleton_model(|_| ResizableData::default());
    app.add_singleton_model(LocalWorkflows::new);
    app.add_singleton_model(UndoCloseStack::new);
    app.add_singleton_model(|_| ActiveSession::default());
    app.add_singleton_model(|_| WorkspaceToastStack);
    app.add_singleton_model(|_| ObjectActions::new(Vec::new()));
    app.add_singleton_model(NotebookKeybindings::new);
    app.add_singleton_model(TerminalKeybindings::new);
    app.add_singleton_model(NotebookManager::mock);
    // Ashide(本地化,Phase 5):`PreferencesSyncer` 已物理删除,test singleton 不再需要。
    app.add_singleton_model(|_| BlocklistAIHistoryModel::new_for_test());
    app.add_singleton_model(|_| CLIAgentSessionsModel::new());
    app.add_singleton_model(crate::terminal::cli_agent::CLIAgentInstallModel::new);
    app.add_singleton_model(crate::ai::agent_providers::AgentProviderSecrets::new);
    app.add_singleton_model(AgentConversationsModel::new);
    app.add_singleton_model(LLMPreferences::new);
    app.add_singleton_model(|_| SettingsPaneManager::new());
    app.add_singleton_model(|_| AIFactManager::new());

    // Initialize file-based MCP dependencies.
    app.add_singleton_model(|_| DetectedRepositories::default());
    app.add_singleton_model(HomeDirectoryWatcher::new_for_test);
    app.add_singleton_model(DirectoryWatcher::new);
    app.add_singleton_model(WarpManagedPathsWatcher::new_for_testing);
    app.add_singleton_model(FileMCPWatcher::new);
    app.add_singleton_model(|_| FileBasedMCPManager::default());

    app.add_singleton_model(|_| TemplatableMCPServerManager::default());
    app.add_singleton_model(|ctx| {
        AIExecutionProfilesModel::new(&crate::LaunchMode::new_for_unit_test(), ctx)
    });
    #[cfg(feature = "voice_input")]
    app.add_singleton_model(voice_input::VoiceInput::new);
    app.add_singleton_model(BlocklistAIPermissions::new);
    app.add_singleton_model(|_| GPUState::new());
    app.add_singleton_model(|_| RestoredAgentConversations::default());
    app.add_singleton_model(OneTimeModalModel::new);
    // Register GlobalResourceHandlesProvider before ServerExperiments which depends on it
    let global_resource_handles = GlobalResourceHandles::mock(app);
    app.add_singleton_model(|_| GlobalResourceHandlesProvider::new(global_resource_handles));
    app.add_singleton_model(|ctx| ServerExperiments::new_from_cache(vec![], ctx));
    app.add_singleton_model(DefaultTerminal::new);
    app.add_singleton_model(|_| IgnoredSuggestionsModel::new(vec![]));
    app.add_singleton_model(|_| crate::code_review::git_status_update::GitStatusUpdateModel::new());
    app.add_singleton_model(crate::workspace::environment_runtime::new_transport_manager);

    #[cfg(feature = "local_fs")]
    app.add_singleton_model(RepoMetadataModel::new);
    app.add_singleton_model(search::files::model::FileSearchModel::new);

    #[cfg(windows)]
    {
        app.add_singleton_model(RendererState::new);
    }

    #[cfg(feature = "local_tty")]
    terminal::available_shells::register(app);
    AltScreenReporting::register(app);

    #[cfg(enable_crash_recovery)]
    crate::crash_recovery::CrashRecovery::register_for_test(app);

    app.update(experiments::init);

    app.add_singleton_model(AIRequestUsageModel::new_for_test);

    app.add_singleton_model(|_| ProjectContextModel::default());
    app.add_singleton_model(AIDocumentModel::new);
    app.add_singleton_model(|_| History::new(vec![]));

    // SkillManager must be registered because the command palette materializes
    // binding descriptions eagerly, and `workspace:send_feedback`'s dynamic
    // label calls `is_feedback_skill_available`, which reads `SkillManager`.
    // Registered after `HomeDirectoryWatcher`, `DirectoryWatcher`,
    // `WarpManagedPathsWatcher`, `DetectedRepositories`, and `RepoMetadataModel`
    // because `SkillWatcher::new` subscribes to all of them.
    app.add_singleton_model(SkillManager::new);
    app.add_singleton_model(EnvironmentSkillInventoryCache::new);

    // SSH manager tests use an isolated throwaway DB path so workspace UI tests
    // don't depend on app-level persistence bootstrap. Run migrations so
    // EnvironmentProviderManager / SshManager panel queries (ssh_nodes) don't
    // fail when workspace mutations trigger left-panel refresh.
    ensure_test_ssh_manager_database_migrated();
    AutoupdateSettings::register(app);

    // Make sure to initialize the keybindings so that they are available for subviews
    app.update(workspace::init);
}

fn ensure_test_ssh_manager_database_migrated() {
    use diesel::connection::SimpleConnection;
    use diesel_migrations::MigrationHarness;

    let temp_db = std::env::temp_dir().join("ashide_workspace_view_test_ssh_manager.sqlite");
    let _ = warp_ssh_manager::set_database_path(temp_db);
    let _ = warp_ssh_manager::with_conn(|conn| {
        conn.batch_execute("PRAGMA foreign_keys = ON;")?;
        conn.run_pending_migrations(::persistence::MIGRATIONS)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(())
    });
}

fn insert_historical_ashide_conversation(
    app: &mut App,
    conversation_id: AIConversationId,
    title: &str,
    cwd: &str,
) {
    insert_historical_ashide_conversation_with_run_id(app, conversation_id, title, cwd, None);
}

fn insert_historical_ashide_conversation_with_run_id(
    app: &mut App,
    conversation_id: AIConversationId,
    title: &str,
    cwd: &str,
    run_id: Option<&str>,
) {
    let task_id = format!("historical-session-{conversation_id}");
    let user_message = api::Message {
        id: format!("{task_id}-user"),
        task_id: task_id.clone(),
        server_message_data: String::new(),
        citations: Vec::new(),
        message: Some(api::message::Message::UserQuery(api::message::UserQuery {
            query: title.to_string(),
            context: Some(api::InputContext {
                directory: Some(api::input_context::Directory {
                    pwd: cwd.to_string(),
                    home: "/Users/admin".to_string(),
                    pwd_file_symbols_indexed: false,
                }),
                ..Default::default()
            }),
            referenced_attachments: HashMap::new(),
            mode: None,
            intended_agent: api::AgentType::Unknown as i32,
        })),
        request_id: String::new(),
        timestamp: None,
    };
    let mut task = create_api_task(
        &task_id,
        vec![
            user_message,
            create_message(&format!("{task_id}-assistant"), &task_id),
        ],
    );
    task.description = title.to_string();

    let conversation_data = AgentConversationData {
        server_conversation_token: None,
        conversation_usage_metadata: None,
        reverted_action_ids: None,
        forked_from_server_conversation_token: None,
        artifacts_json: None,
        parent_agent_id: None,
        agent_name: None,
        parent_conversation_id: None,
        run_id: run_id.map(str::to_owned),
        autoexecute_override: None,
        last_event_sequence: None,
        compaction_state_json: None,
        byop_repair_state_json: None,
        session_bridge_import: None,
    };

    BlocklistAIHistoryModel::handle(app).update(app, |history, ctx| {
        history
            .insert_historical_conversation_from_tasks(
                conversation_id,
                vec![task],
                conversation_data,
                ctx,
            )
            .expect("historical conversation should be inserted");
    });
}

fn mock_workspace(app: &mut App) -> ViewHandle<Workspace> {
    let global_resource_handles = GlobalResourceHandles::mock(app);
    let active_window_id = app.read(|ctx| ctx.windows().active_window());
    let (_, workspace) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
        Workspace::new(
            global_resource_handles,
            None,
            NewWorkspaceSource::Empty {
                previous_active_window: active_window_id,
                shell: None,
            },
            ctx,
        )
    });
    workspace
}

fn restored_workspace(
    app: &mut App,
    window_snapshot: crate::app_state::WindowSnapshot,
) -> ViewHandle<Workspace> {
    let global_resource_handles = GlobalResourceHandles::mock(app);
    let (_, workspace) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
        Workspace::new(
            global_resource_handles,
            None,
            NewWorkspaceSource::Restored {
                window_snapshot,
                block_lists: Arc::new(HashMap::new()),
            },
            ctx,
        )
    });
    workspace
}

fn transferred_tab_workspace(
    app: &mut App,
    vertical_tabs_panel_open: bool,
) -> ViewHandle<Workspace> {
    let global_resource_handles = GlobalResourceHandles::mock(app);
    let (_, workspace) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
        Workspace::new(
            global_resource_handles,
            None,
            NewWorkspaceSource::TransferredTab {
                tab_color: None,
                custom_title: None,
                left_panel_open: false,
                vertical_tabs_panel_open,
                right_panel_open: false,
                is_right_panel_maximized: false,
                is_tab_drag_preview: false,
            },
            ctx,
        )
    });
    workspace
}

#[cfg(feature = "local_fs")]
fn open_worktree_sidecar(workspace: &ViewHandle<Workspace>, app: &mut App) {
    workspace.update(app, |workspace, ctx| {
        workspace.open_new_session_dropdown_menu(Vector2F::zero(), ctx);

        let worktree_index = workspace
            .new_session_dropdown_menu
            .read(ctx, |menu, _| {
                menu.items().iter().position(|item| {
                    matches!(
                        item,
                        MenuItem::Item(fields) if fields.label() == "New worktree config"
                    )
                })
            })
            .expect("expected new worktree config item in new-session menu");

        workspace
            .new_session_dropdown_menu
            .update(ctx, |menu, view_ctx| {
                menu.set_selected_by_index(worktree_index, view_ctx);
            });
    });
}

/// RAII guard that removes tab config TOML files whose name starts with
/// `prefix` from `~/.warp/tab_configs/` on drop. Because `Drop` runs even
/// when a test panics, this prevents stale worktree configs from leaking
/// into Ashide dev.
#[cfg(feature = "local_fs")]
struct TabConfigCleanupGuard {
    prefix: &'static str,
}

#[cfg(feature = "local_fs")]
impl TabConfigCleanupGuard {
    fn new(prefix: &'static str) -> Self {
        // Eagerly clean up leftovers from any previously-crashed run.
        Self::clean(prefix);
        Self { prefix }
    }

    fn clean(prefix: &str) {
        let dir = tab_configs_dir();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with(prefix) && name.ends_with(".toml") {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }
}

#[cfg(feature = "local_fs")]
impl Drop for TabConfigCleanupGuard {
    fn drop(&mut self) {
        Self::clean(self.prefix);
    }
}

/// Creates a workspace with a single, shared session.
fn mock_workspace_with_shared_session(app: &mut App) -> ViewHandle<Workspace> {
    // Create the workspace as a session-sharing sharer.
    let global_resource_handles = GlobalResourceHandles::mock(app);
    let (_, workspace) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
        Workspace::new(
            global_resource_handles,
            None,
            NewWorkspaceSource::Empty {
                previous_active_window: None,
                shell: None,
            },
            ctx,
        )
    });

    // Get the single terminal view in the workspace.
    let terminal_view = workspace.read(app, |workspace, ctx| {
        assert_eq!(workspace.tabs.len(), 1);
        workspace
            .active_tab_pane_group()
            .as_ref(ctx)
            .focused_session_view(ctx)
            .unwrap()
    });

    terminal_view.update(app, |view, ctx| {
        view.model.lock().block_list_mut().set_bootstrapped();
        view.model
            .lock()
            .set_shared_session_status(SharedSessionStatus::ActiveSharer);
        ctx.notify();
    });

    workspace
}

// Creates a workspace as a viewer of a shared session.
fn mock_workspace_viewing_shared_session(app: &mut App) -> ViewHandle<Workspace> {
    // Create the workspace as a session-sharing sharer.
    let global_resource_handles = GlobalResourceHandles::mock(app);

    let (_, workspace) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
        Workspace::new(
            global_resource_handles,
            None,
            NewWorkspaceSource::Empty {
                previous_active_window: None,
                shell: None,
            },
            ctx,
        )
    });

    // Get the single terminal view in the workspace.
    let terminal_view = workspace.read(app, |workspace, ctx| {
        assert_eq!(workspace.tabs.len(), 1);
        workspace
            .active_tab_pane_group()
            .as_ref(ctx)
            .focused_session_view(ctx)
            .unwrap()
    });

    terminal_view.update(app, |view, ctx| {
        view.on_session_share_joined(
            ParticipantId::new(),
            UserUid::new("mock_user_uid"),
            ReplicaId::random(),
            Box::new(ParticipantList::default()),
            SessionId::new(),
            SessionSourceType::default(),
            ctx,
        );
    });

    workspace
}

/// Disable the warn-before-quit setting. Because we don't fully bootstrap the shell in tests, this
/// is generally needed in tests that close tabs.
fn disable_quit_warning(app: &mut AppContext) {
    GeneralSettings::handle(app).update(app, |settings, ctx| {
        settings
            .show_warning_before_quitting
            .set_value(false, ctx)
            .expect("Failed to disable quit warning");
    });
}

fn get_newly_created_pane_id(panes: &PaneGroup, existing_ids: &[PaneId]) -> PaneId {
    panes
        .pane_ids()
        .find(|id| !existing_ids.contains(id))
        .unwrap()
}

fn split_pane_state(
    panes: &PaneGroup,
    pane_id: impl Into<PaneId>,
    ctx: &AppContext,
) -> SplitPaneState {
    // Split pane state is now inferred from the pane group's focus state
    panes
        .focus_state_handle()
        .as_ref(ctx)
        .split_pane_state_for(pane_id.into())
}

fn active_session_state(
    panes: &PaneGroup,
    pane_id: TerminalPaneId,
    ctx: &AppContext,
) -> ActiveSessionState {
    if panes
        .terminal_view_from_pane_id(pane_id, ctx)
        .expect("Not a terminal pane")
        .as_ref(ctx)
        .is_active_session(ctx)
    {
        ActiveSessionState::Active
    } else {
        ActiveSessionState::Inactive
    }
}

fn new_session_menu_label(item: &MenuItem<WorkspaceAction>) -> String {
    match item {
        MenuItem::Item(fields) => fields.label().to_string(),
        MenuItem::Separator => "---".to_string(),
        MenuItem::ItemsRow { items } => items
            .iter()
            .map(|fields| fields.label().to_string())
            .collect::<Vec<_>>()
            .join(" | "),
        MenuItem::Submenu { fields, .. } => fields.label().to_string(),
        MenuItem::Header { fields, .. } => fields.label().to_string(),
    }
}

fn reopen_closed_session_menu_item(
    menu_items: &[MenuItem<WorkspaceAction>],
) -> &MenuItemFields<WorkspaceAction> {
    match menu_items.last() {
        Some(MenuItem::Item(fields)) if fields.label() == "Reopen closed session" => fields,
        _ => panic!("expected Reopen closed session to be the last new-session menu item"),
    }
}

#[test]
fn test_tab_renaming_editor_selections() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        app.update(|ctx| {
            TabSettings::handle(ctx).update(ctx, |settings, ctx| {
                report_if_error!(settings.use_vertical_tabs.set_value(false, ctx));
            });
        });

        let workspace = mock_workspace(&mut app);

        // Add second tab and rename both of them to prepare for the test
        workspace.update(&mut app, |workspace, ctx| {
            workspace.add_terminal_tab(false, ctx);
            workspace.rename_tab_internal(0, "short_title", ctx);
            let selected_text = workspace
                .tab_rename_editor
                .read(ctx, |editor, ctx| editor.selected_text(ctx));
            assert_eq!("short_title", selected_text);

            // Ensure that whatever is selected, is the full title and not the leftover from
            // the previous, shorter one.
            workspace.rename_tab_internal(1, "very_long_title_this_is", ctx);
            let selected_text = workspace
                .tab_rename_editor
                .read(ctx, |editor, ctx| editor.selected_text(ctx));
            assert_eq!("very_long_title_this_is", selected_text);

            // Ensure that if we escape, the current editor's contents is going to be cleared
            // as well.
            workspace.handle_tab_rename_editor_event(&Event::Escape, ctx);
            let selected_text = workspace
                .tab_rename_editor
                .read(ctx, |editor, ctx| editor.selected_text(ctx));
            assert_eq!("", selected_text);
        });
    });
}

#[test]
fn test_tab_renaming_editor_reset() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        let _welcome_guard = FeatureFlag::WelcomeTab.override_enabled(true);

        workspace.update(&mut app, |workspace, ctx| {
            workspace.add_terminal_tab(false, ctx);
            workspace.rename_tab_internal(0, "short_title", ctx);
            workspace.rename_tab_internal(1, "very_long_title_this_is", ctx);

            // Ensure that when the editor is initially not empty, it will be cleared before a user renames a tab
            workspace.tab_rename_editor.update(ctx, |editor, ctx| {
                editor.insert_selected_text("some-text", ctx);
            });
            workspace.rename_tab_internal(1, "new_very_long_title", ctx);
            let selected_text: String = workspace
                .tab_rename_editor
                .read(ctx, |editor, ctx| editor.selected_text(ctx));
            assert_eq!("new_very_long_title", selected_text);
        });
    });
}

#[test]
fn test_set_active_tab_name() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            workspace.add_terminal_tab(false, ctx);

            workspace.handle_action(
                &WorkspaceAction::SetActiveTabName("  Backend API  ".to_string()),
                ctx,
            );
            assert_eq!(
                workspace
                    .active_tab_pane_group()
                    .as_ref(ctx)
                    .display_title(ctx),
                "Backend API"
            );
            assert_eq!(
                workspace
                    .active_tab_pane_group()
                    .as_ref(ctx)
                    .custom_title(ctx)
                    .as_deref(),
                Some("Backend API")
            );

            workspace.handle_action(&WorkspaceAction::ActivateTab(0), ctx);
            assert_ne!(
                workspace
                    .active_tab_pane_group()
                    .as_ref(ctx)
                    .custom_title(ctx)
                    .as_deref(),
                Some("Backend API")
            );

            workspace.handle_action(&WorkspaceAction::ActivateTab(1), ctx);
            workspace.handle_action(&WorkspaceAction::SetActiveTabName("   ".to_string()), ctx);
            assert_eq!(
                workspace
                    .active_tab_pane_group()
                    .as_ref(ctx)
                    .custom_title(ctx)
                    .as_deref(),
                Some("Backend API")
            );
        });
    });
}

#[test]
fn test_set_active_tab_name_clears_active_rename_editor_state() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            workspace.rename_tab_internal(0, "old title", ctx);
            assert!(workspace.current_workspace_state.is_tab_being_renamed());

            workspace.handle_action(
                &WorkspaceAction::SetActiveTabName("new title".to_string()),
                ctx,
            );

            assert!(!workspace.current_workspace_state.is_tab_being_renamed());
            assert_eq!(
                workspace
                    .active_tab_pane_group()
                    .as_ref(ctx)
                    .display_title(ctx),
                "new title"
            );
        });
    });
}

#[test]
fn test_set_active_tab_color() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            workspace.add_terminal_tab(false, ctx);
            let active = workspace.active_tab_index;

            // Setting a color stores it as the manual selection and resolves to it.
            workspace.handle_action(
                &WorkspaceAction::SetActiveTabColor(SelectedTabColor::Color(
                    AnsiColorIdentifier::Magenta,
                )),
                ctx,
            );
            assert_eq!(
                workspace.tabs[active].selected_color,
                SelectedTabColor::Color(AnsiColorIdentifier::Magenta),
            );
            assert_eq!(
                workspace.tabs[active].color(),
                Some(AnsiColorIdentifier::Magenta),
            );

            // Replacing with a different color overwrites the previous selection.
            workspace.handle_action(
                &WorkspaceAction::SetActiveTabColor(SelectedTabColor::Color(
                    AnsiColorIdentifier::Green,
                )),
                ctx,
            );
            assert_eq!(
                workspace.tabs[active].selected_color,
                SelectedTabColor::Color(AnsiColorIdentifier::Green),
            );

            // `Cleared` explicitly suppresses any color (including a directory default).
            workspace.handle_action(
                &WorkspaceAction::SetActiveTabColor(SelectedTabColor::Cleared),
                ctx,
            );
            assert_eq!(
                workspace.tabs[active].selected_color,
                SelectedTabColor::Cleared,
            );
            assert_eq!(workspace.tabs[active].color(), None);

            // `Unset` removes the manual override so a directory default could apply.
            // With no directory default configured, the resolved color is still `None`.
            workspace.handle_action(
                &WorkspaceAction::SetActiveTabColor(SelectedTabColor::Unset),
                ctx,
            );
            assert_eq!(
                workspace.tabs[active].selected_color,
                SelectedTabColor::Unset,
            );
            assert_eq!(workspace.tabs[active].color(), None);

            // Action targets the active tab — switching to tab 0 leaves the second tab
            // unaffected.
            workspace.handle_action(&WorkspaceAction::ActivateTab(0), ctx);
            workspace.handle_action(
                &WorkspaceAction::SetActiveTabColor(SelectedTabColor::Color(
                    AnsiColorIdentifier::Blue,
                )),
                ctx,
            );
            assert_eq!(
                workspace.tabs[0].selected_color,
                SelectedTabColor::Color(AnsiColorIdentifier::Blue),
            );
            assert_eq!(
                workspace.tabs[active].selected_color,
                SelectedTabColor::Unset,
            );
        });
    });
}

#[test]
fn test_workspace_sessions_retrieves_tabs() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let pane_id = workspace
                .get_pane_group_view(0)
                .map(|tab| tab.read(ctx, |tab, _ctx| tab.pane_id_by_index(0).unwrap()))
                .expect("WindowId was not retrieved.");

            assert!(workspace
                .workspace_sessions(ctx.window_id(), ctx)
                .any(|x| { x.pane_view_locator().pane_id == pane_id }));

            // Add a tab and check if workspace_sessions finds the second session from the new tab.
            workspace.add_terminal_tab(false, ctx);
            let new_pane_id = workspace
                .get_pane_group_view(1)
                .map(|tab| tab.read(ctx, |tab, _ctx| tab.pane_id_by_index(0).unwrap()))
                .expect("WindowId was not retrieved.");

            assert!(workspace
                .workspace_sessions(ctx.window_id(), ctx)
                .any(|x| { x.pane_view_locator().pane_id == new_pane_id }));
        });
    });
}

#[test]
fn test_workspace_sessions_retrieves_panes() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            // Add a new split pane to the right.
            if let Some(tab_view) = workspace.get_pane_group_view(0) {
                tab_view.update(ctx, |view, ctx| {
                    view.handle_action(&PaneGroupAction::Add(Direction::Right), ctx);
                })
            }

            // Get the EntityId of the new pane added to the current tab.
            let new_pane_id = workspace
                .get_pane_group_view(0)
                .map(|tab| tab.read(ctx, |tab, _ctx| tab.pane_id_by_index(1).unwrap()))
                .expect("WindowId was not retrieved.");
            assert!(workspace
                .workspace_sessions(ctx.window_id(), ctx)
                .any(|x| { x.pane_view_locator().pane_id == new_pane_id }));
        });
    });
}

fn number_of_shared_sessions_in_tab(
    workspace: &Workspace,
    index: usize,
    ctx: &AppContext,
) -> usize {
    workspace
        .get_pane_group_view(index)
        .map_or(0, |view| view.as_ref(ctx).number_of_shared_sessions(ctx))
}

/// Sets up the workspace with three tabs. The middle tab has two panes, where one is shared.
fn setup_session_sharing_test(workspace: &ViewHandle<Workspace>, app: &mut App) -> PaneId {
    let shared_pane_id = workspace.update(app, |workspace, ctx| {
        workspace.add_terminal_tab(false, ctx);
        workspace.add_terminal_tab(false, ctx);

        let tab_view = workspace.get_pane_group_view(1).unwrap();

        tab_view.update(ctx, |view, ctx| {
            assert_eq!(view.pane_count(), 1);
            view.focused_session_view(ctx)
                .unwrap()
                .update(ctx, |terminal, ctx| {
                    terminal
                        .model
                        .lock()
                        .set_shared_session_status(SharedSessionStatus::ActiveSharer);
                    ctx.notify();
                });

            view.handle_action(&PaneGroupAction::Add(Direction::Right), ctx);
            assert_eq!(view.pane_count(), 2);

            view.pane_id_by_index(0).unwrap()
        })
    });

    workspace.read(app, |workspace, ctx| {
        assert_eq!(number_of_shared_sessions_in_tab(workspace, 1, ctx), 1);

        // Confirmation dialog starts not open.
        assert!(
            !workspace
                .current_workspace_state
                .is_close_session_confirmation_dialog_open
        );
    });

    shared_pane_id
}

#[test]
fn test_close_tab_confirmation_dialog() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        app.update(disable_quit_warning);

        let workspace = mock_workspace(&mut app);
        setup_session_sharing_test(&workspace, &mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let first_tab_id = workspace.get_pane_group_view(0).unwrap().id();

            // Trying to close tab with a shared pane opens dialog.
            workspace.handle_action(&WorkspaceAction::CloseTab(1), ctx);
            assert!(
                workspace
                    .current_workspace_state
                    .is_close_session_confirmation_dialog_open
            );

            // User clicking cancel closes dialog.
            workspace.handle_close_session_confirmation_dialog_event(
                &CloseSessionConfirmationEvent::Cancel,
                ctx,
            );
            assert!(
                !workspace
                    .current_workspace_state
                    .is_close_session_confirmation_dialog_open
            );

            // Trying to close tab without a shared pane goes through without dialog.
            workspace.handle_action(&WorkspaceAction::CloseTab(2), ctx);
            assert_eq!(workspace.tab_count(), 2);
            assert!(
                !workspace
                    .current_workspace_state
                    .is_close_session_confirmation_dialog_open
            );

            // Close the tab with the shared pane.
            workspace.handle_action(&WorkspaceAction::CloseTab(1), ctx);
            assert!(
                workspace
                    .current_workspace_state
                    .is_close_session_confirmation_dialog_open
            );
            workspace.handle_close_session_confirmation_dialog_event(
                &CloseSessionConfirmationEvent::CloseSession {
                    dont_show_again: false,
                    open_confirmation_source: OpenDialogSource::CloseTab { tab_index: 1 },
                },
                ctx,
            );
            assert!(
                !workspace
                    .current_workspace_state
                    .is_close_session_confirmation_dialog_open
            );
            assert_eq!(workspace.tab_count(), 1);
            assert_eq!(workspace.get_pane_group_view(0).unwrap().id(), first_tab_id);
        });
    });
}

#[test]
fn test_close_pane_confirmation_dialog() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        let shared_pane_id = setup_session_sharing_test(&workspace, &mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let shared_pane_group_id = workspace.get_pane_group_view(1).unwrap().id();

            // User tries to close shared pane, dialog comes up.
            workspace.handle_file_tree_event(
                workspace.get_pane_group_view(1).unwrap().clone(),
                &pane_group::Event::CloseSharedSessionPaneRequested {
                    pane_id: shared_pane_id,
                },
                ctx,
            );
            assert!(
                workspace
                    .current_workspace_state
                    .is_close_session_confirmation_dialog_open
            );

            // User confirms.
            workspace.handle_close_session_confirmation_dialog_event(
                &CloseSessionConfirmationEvent::CloseSession {
                    dont_show_again: false,
                    open_confirmation_source: OpenDialogSource::ClosePane {
                        pane_group_id: shared_pane_group_id,
                        pane_id: shared_pane_id,
                    },
                },
                ctx,
            );
            assert!(
                !workspace
                    .current_workspace_state
                    .is_close_session_confirmation_dialog_open
            );
            assert_eq!(number_of_shared_sessions_in_tab(workspace, 1, ctx), 0);
            let remaining_pane_id = workspace
                .get_pane_group_view_with_id(shared_pane_group_id)
                .unwrap()
                .as_ref(ctx)
                .pane_id_by_index(0)
                .unwrap();
            assert_ne!(remaining_pane_id, shared_pane_id);
            assert_eq!(workspace.tab_count(), 3);
        });
    });
}

#[test]
fn test_reopen_closed_shared_tab() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        setup_session_sharing_test(&workspace, &mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let shared_pane_group = workspace.get_pane_group_view(1).unwrap().clone();

            // Close the tab with the shared pane.
            workspace.close_tab(1, true, true, ctx);
            assert_eq!(workspace.tab_count(), 2);

            // Restore the shared tab.
            workspace.restore_closed_tab(1, TabData::new(shared_pane_group.to_owned()), ctx);
        });
        // Restored tab should no longer be shared.
        workspace.read(&app, |workspace, ctx| {
            let pane_group = workspace.get_pane_group_view(1).unwrap();
            assert!(!pane_group.as_ref(ctx).is_terminal_pane_being_shared(ctx));
            assert_eq!(workspace.tab_count(), 3);
        })
    });
}

#[test]
fn test_close_other_tabs_confirmation_dialog() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        setup_session_sharing_test(&workspace, &mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let last_tab_id = workspace.get_pane_group_view(2).unwrap().id();

            // User tries to close other tabs choosing non-shared tab, dialog comes up.
            workspace.handle_action(&WorkspaceAction::CloseOtherTabs(2), ctx);
            assert!(
                workspace
                    .current_workspace_state
                    .is_close_session_confirmation_dialog_open
            );

            // User confirms.
            workspace.handle_close_session_confirmation_dialog_event(
                &CloseSessionConfirmationEvent::CloseSession {
                    dont_show_again: false,
                    open_confirmation_source: OpenDialogSource::CloseOtherTabs { tab_index: 2 },
                },
                ctx,
            );
            assert!(
                !workspace
                    .current_workspace_state
                    .is_close_session_confirmation_dialog_open
            );
            assert_eq!(workspace.tab_count(), 1);
            assert_eq!(workspace.get_pane_group_view(0).unwrap().id(), last_tab_id);
        });
    });
}

#[test]
fn test_close_tabs_right_confirmation_dialog() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        setup_session_sharing_test(&workspace, &mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let first_tab_id = workspace.get_pane_group_view(0).unwrap().id();

            // User tries to close all tabs right of the left-most tab, dialog comes up.
            workspace.handle_action(&WorkspaceAction::CloseTabsRight(0), ctx);
            assert!(
                workspace
                    .current_workspace_state
                    .is_close_session_confirmation_dialog_open
            );

            // User confirms.
            workspace.handle_close_session_confirmation_dialog_event(
                &CloseSessionConfirmationEvent::CloseSession {
                    dont_show_again: false,
                    open_confirmation_source: OpenDialogSource::CloseTabsDirection {
                        tab_index: 0,
                        direction: TabMovement::Right,
                    },
                },
                ctx,
            );
            assert!(
                !workspace
                    .current_workspace_state
                    .is_close_session_confirmation_dialog_open
            );
            assert_eq!(workspace.tab_count(), 1);
            assert_eq!(workspace.get_pane_group_view(0).unwrap().id(), first_tab_id);
        });
    });
}

#[test]
fn test_confirmation_dialog_dont_show_again() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        app.update(disable_quit_warning);

        let workspace = mock_workspace(&mut app);
        setup_session_sharing_test(&workspace, &mut app);

        workspace.update(&mut app, |workspace, ctx| {
            // Close the tab with the shared pane, dialog comes up
            workspace.handle_action(&WorkspaceAction::CloseTab(1), ctx);
            assert!(
                workspace
                    .current_workspace_state
                    .is_close_session_confirmation_dialog_open
            );

            // User confirms, checking "Don't show again".
            workspace.handle_close_session_confirmation_dialog_event(
                &CloseSessionConfirmationEvent::CloseSession {
                    dont_show_again: true,
                    open_confirmation_source: OpenDialogSource::CloseTab { tab_index: 1 },
                },
                ctx,
            );
            assert!(
                !workspace
                    .current_workspace_state
                    .is_close_session_confirmation_dialog_open
            );
            assert_eq!(workspace.tab_count(), 2);

            // Share the first tab
            let tab_view = workspace.get_pane_group_view(0).unwrap();
            tab_view.update(ctx, |view, ctx| {
                view.terminal_manager(0, ctx)
                    .unwrap()
                    .as_ref(ctx)
                    .model()
                    .lock()
                    .set_shared_session_status(SharedSessionStatus::ActiveSharer);
            });

            // Close the shared tab. No dialog should come up and action should go through.
            workspace.handle_action(&WorkspaceAction::CloseActiveTab, ctx);
            assert!(
                !workspace
                    .current_workspace_state
                    .is_close_session_confirmation_dialog_open
            );
            assert_eq!(workspace.tab_count(), 1);
        });
    });
}

#[test]
fn test_close_last_tab_skip_confirmation() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        app.update(disable_quit_warning);

        let workspace = mock_workspace(&mut app);
        setup_session_sharing_test(&workspace, &mut app);

        workspace.update(&mut app, |workspace, ctx| {
            // Close the non-shared tabs so there's just one shared tab left.
            workspace.handle_action(&WorkspaceAction::CloseTab(2), ctx);
            workspace.handle_action(&WorkspaceAction::CloseTab(0), ctx);
            assert_eq!(workspace.tab_count(), 1);
            // Close the last remaining tab with the shared pane, no dialog should come up because
            // we're going to close the window and there's already a confirmation on window close.
            workspace.handle_action(&WorkspaceAction::CloseActiveTab, ctx);
            assert!(
                !workspace
                    .current_workspace_state
                    .is_close_session_confirmation_dialog_open
            );
        });
    });
}

#[test]
fn test_notebook_pane_tracking() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            // Add a new notebook pane.
            workspace.open_notebook(
                &NotebookSource::New {
                    title: None,
                    owner: Owner::mock_current_user(),
                    initial_folder_id: None,
                },
                &LocalDriveObjectSettings::default(),
                ctx,
                true,
            );

            // Get the ID of the new notebook.
            let pane_group = workspace
                .get_pane_group_view(0)
                .expect("Pane group does not exist")
                .clone();
            let notebook_view = pane_group
                .as_ref(ctx)
                .notebook_view_at_pane_index(0, ctx)
                .expect("Notebook view was not created")
                .clone();
            let notebook_pane_id = pane_group
                .as_ref(ctx)
                .pane_id_from_index(0)
                .expect("Notebook view should have been created");
            let notebook_id = notebook_view
                .as_ref(ctx)
                .notebook_id(ctx)
                .expect("Notebook should have an ID");

            // The notebook should be registered with the NotebookManager.
            let (window, locator) = NotebookManager::as_ref(ctx)
                .find_pane(&NotebookSource::Existing(notebook_id))
                .expect("Notebook pane should be registered");
            assert_eq!(window, ctx.window_id());
            assert_eq!(
                locator,
                PaneViewLocator {
                    pane_group_id: pane_group.id(),
                    pane_id: notebook_pane_id,
                }
            );

            // Re-opening the notebook should not create a new view.
            workspace.open_notebook(
                &NotebookSource::Existing(notebook_id),
                &LocalDriveObjectSettings::default(),
                ctx,
                true,
            );
            assert_eq!(
                ctx.views_of_type::<NotebookView>(ctx.window_id()),
                Some(vec![notebook_view])
            );

            // Finally, closing the notebook pane should de-register it.
            pane_group.update(ctx, |pane_group, ctx| {
                pane_group.handle_action(&PaneGroupAction::RemoveActive, ctx)
            });
            assert_eq!(
                NotebookManager::handle(ctx)
                    .as_ref(ctx)
                    .find_pane(&NotebookSource::Existing(notebook_id)),
                None
            );
        });
    });
}

#[test]
fn test_set_active_terminal_input_contents_and_focus_app() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let initial_buffer_contents = workspace
                .get_active_input_view_handle(ctx)
                .map(|input_view_handle| input_view_handle.as_ref(ctx).buffer_text(ctx))
                .expect("There should be an active input view");
            assert_eq!(
                "", initial_buffer_contents,
                "initial active input should be empty"
            );

            workspace.set_active_terminal_input_contents_and_focus_app("foobar", ctx);

            assert_eq!(
                "foobar",
                workspace
                    .get_active_input_view_handle(ctx)
                    .map(|input_view_handle| input_view_handle.as_ref(ctx).buffer_text(ctx))
                    .expect("There should be an active input view")
            );
            assert!(ctx.windows().app_is_active());
        });
    });
}

/// Ensures that the terminal model is destroyed when it is no longer needed.
/// This is only a "workspace" test because we want to mimic what a normal
/// user would do and expect (e.g. close a tab and expect that its backing
/// data is correctly deallocated).
///
/// TODO(suraj): we may also want to investigate a more "real" integration test
/// that inspects the application process's overall memory consumption
/// instead of just the terminal model, but this is not easy because
/// 1. we want to measure non-shared memory (i.e. the "memory" value in Activity Monitor)
///    which is not easy; it's easier to measure "real memory" or RSS, but that includes
///    shared memory across processes.
/// 2. the test might be flaky depending on how much memory is actually allocated vs
///    freed up (not something easily controlled).
///
/// For now, this test is still useful because the terminal model is one of the largest data structures
/// maintained by our app, so we want to ensure we're not introducing regressions that cause it to not
/// be deallocated correctly.
#[test]
fn test_terminal_model_isnt_leaked() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        // Turn off undo-close so that we don't need to wait for deallocation.
        UndoCloseSettings::handle(&app).update(&mut app, |settings, ctx| {
            settings
                .enabled
                .set_value(false, ctx)
                .expect("Can turn off undo-close via settings.")
        });

        let workspace = mock_workspace(&mut app);

        let terminal_model = workspace.update(&mut app, |workspace, ctx| {
            // Add another tab so that the workspace isn't destroyed when we close the tab.
            workspace.add_terminal_tab(false, ctx);

            // Get a weak reference to the model.
            let model = workspace.get_active_session_terminal_model(ctx).unwrap();
            Arc::downgrade(&model)
        });

        workspace.update(&mut app, |workspace, ctx| {
            // Remove the tab. This should destroy the corresponding terminal view.
            workspace.remove_tab(workspace.active_tab_index(), true, true, ctx);
        });
        // For some reason, the update call above results in more pending effects, one of which
        // contains the actual logic that drops the `TerminalModel`.
        app.update(|_| ());

        // If we can't upgrade the weak reference, that means it was in fact destructed.
        assert!(
            terminal_model.upgrade().is_none(),
            "The terminal model should not exist once the tab is closed."
        )
    });
}

#[test]
fn test_open_or_toggle_local_drive() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            // First, unconditionally open Ashide Drive as a system action. WD should be open and welcome tips should not have opening Ashide Drive.
            workspace.open_or_toggle_local_drive(
                false, /* toggle */
                false, /* explicit_user_action */
                ctx,
            );
            assert!(
                workspace.current_workspace_state.is_local_drive_open,
                "Ashide Drive should be open"
            );
            assert!(
                !workspace
                    .tips_completed
                    .as_ref(ctx)
                    .features_used
                    .contains(&Tip::Action(TipAction::LocalDrive)),
                "Ashide Drive welcome tip should not be completed"
            );

            // Next, toggle Ashide Drive as a user action. WD should be closed and tip should not be filled out.
            workspace.open_or_toggle_local_drive(
                true, /* toggle */
                true, /* explicit_user_action */
                ctx,
            );
            assert!(
                !workspace.current_workspace_state.is_local_drive_open,
                "Ashide Drive should be closed"
            );
            assert!(
                !workspace
                    .tips_completed
                    .as_ref(ctx)
                    .features_used
                    .contains(&Tip::Action(TipAction::LocalDrive)),
                "Ashide Drive welcome tip should not be completed"
            );

            // Finally, toggle Ashide Drive again as a user action. WD should be open and tip filled out.
            workspace.open_or_toggle_local_drive(
                true, /* toggle */
                true, /* explicit_user_action */
                ctx,
            );
            assert!(
                workspace.current_workspace_state.is_local_drive_open,
                "Ashide Drive should be open"
            );
            assert!(
                workspace
                    .tips_completed
                    .as_ref(ctx)
                    .features_used
                    .contains(&Tip::Action(TipAction::LocalDrive)),
                "Ashide Drive welcome tip should not be completed"
            );
        });
    });
}

#[test]
fn test_view_only_session() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        // Trying to open command search
        let workspace = mock_workspace_viewing_shared_session(&mut app);
        workspace.update(&mut app, |workspace: &mut Workspace, ctx| {
            workspace.handle_action(&WorkspaceAction::ShowCommandSearch(Default::default()), ctx);
        });

        // Ensure command search doesn't work for read-only shared sessions
        workspace.read(&app, |workspace, _ctx| {
            assert!(!workspace.current_workspace_state.is_command_search_open);
        });
    });
}

#[test]
fn test_server_token_compatibility_finds_restored_persisted_conversation() {
    use crate::ai::agent::conversation::AIConversation;

    App::test((), |mut app| async move {
        let history_model = app.add_singleton_model(|_| BlocklistAIHistoryModel::new_for_test());
        let token = ServerConversationToken::new("restored-token".to_string());
        let conversation_id = history_model.update(&mut app, |model, ctx| {
            let mut conversation = AIConversation::new(false);
            conversation.set_server_conversation_token(token.as_str().to_string());
            let conversation_id = conversation.id();
            model.restore_conversations(EntityId::new(), vec![conversation], ctx);
            conversation_id
        });

        app.read(|ctx| {
            assert_eq!(
                Workspace::find_persisted_conversation_id_by_server_token(&token, ctx),
                Some(conversation_id),
            );
        });
    });
}

#[test]
fn test_server_token_compatibility_ignores_unknown_token() {
    App::test((), |app| async move {
        app.add_singleton_model(|_| BlocklistAIHistoryModel::new_for_test());
        let token = ServerConversationToken::new("missing-token".to_string());

        app.read(|ctx| {
            assert_eq!(
                Workspace::find_persisted_conversation_id_by_server_token(&token, ctx),
                None,
            );
        });
    });
}

#[test]
// This tests the end-to-end behavior to correctly switch focus among panels.
// (The only panels that can be focused currently are WD, workspace, & AI assistant.)
fn test_switch_focus_panels() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |view, ctx| {
            view.focus_active_tab(ctx);
        });
        workspace.update(&mut app, |view, ctx| {
            assert!(
                view.active_tab_pane_group().is_self_or_child_focused(ctx),
                "Expected terminal to be focused"
            );
        });

        // Shift focus from terminal to left panel when WD is open
        workspace.update(&mut app, |view, ctx| {
            view.current_workspace_state.is_local_drive_open = true;
            view.focus_left_panel(ctx);
        });
        workspace.update(&mut app, |view, ctx| {
            assert!(
                view.left_panel_view.is_self_or_child_focused(ctx),
                "Expected Ashide Drive panel to be focused"
            );
        });

        // Shift focus from WD to left panel when AI panel is open
        workspace.update(&mut app, |view, ctx| {
            view.current_workspace_state.is_ai_assistant_panel_open = true;
            view.focus_left_panel(ctx);
        });
        workspace.update(&mut app, |view, ctx| {
            assert!(
                view.ai_assistant_panel.is_self_or_child_focused(ctx),
                "Expected AI panel to be focused"
            );
        });

        // Shift focus from AI panel to left panel (terminal)
        workspace.update(&mut app, |view, ctx| {
            view.focus_left_panel(ctx);
        });
        workspace.update(&mut app, |_view, ctx| {
            assert!(
                workspace.is_self_or_child_focused(ctx),
                "Expected terminal to be focused"
            );
        });

        // Shift focus from workspace to right panel when AI assistant is open
        workspace.update(&mut app, |view, ctx| {
            view.current_workspace_state.is_ai_assistant_panel_open = true;
            view.focus_right_panel(ctx);
        });
        workspace.update(&mut app, |view, ctx| {
            assert!(
                view.ai_assistant_panel.is_self_or_child_focused(ctx),
                "Expected AI panel to be focused"
            );
        });

        // Shift focus from WD to right panel (terminal)
        workspace.update(&mut app, |view, ctx| {
            view.focus_right_panel(ctx);
        });
        workspace.update(&mut app, |_view, ctx| {
            assert!(
                workspace.is_self_or_child_focused(ctx),
                "Expected terminal to be focused"
            );
        });
    });
}

#[test]
fn test_left_panel_tool_actions_focus_opened_panel_entry() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);

        for action in [
            LeftPanelAction::ProjectExplorer,
            LeftPanelAction::SkillManager,
        ] {
            workspace.update(&mut app, |workspace, ctx| {
                workspace.open_left_panel(ctx);
                workspace.focus_active_tab(ctx);
            });
            workspace.update(&mut app, |workspace, ctx| {
                assert!(
                    workspace
                        .active_tab_pane_group()
                        .is_self_or_child_focused(ctx),
                    "test setup should start from the already-focused terminal before {action:?}"
                );
                assert!(
                    !workspace.left_panel_view.is_self_or_child_focused(ctx),
                    "test setup should start outside the tools panel before {action:?}"
                );
            });

            workspace.update(&mut app, |workspace, ctx| {
                workspace.left_panel_view.update(ctx, |left_panel, ctx| {
                    left_panel.apply_action(&action, ctx);
                });
            });

            workspace.update(&mut app, |workspace, ctx| {
                assert!(
                    workspace.left_panel_view.is_focused(ctx)
                        || workspace.left_panel_view.is_self_or_child_focused(ctx),
                    "opening {action:?} from the tools panel must move focus into that panel"
                );
            });
        }
    });
}

fn assert_focus_left_panel_enters_unified_tool_panel_and_cycles_back_to_terminal(
    action: LeftPanelAction,
) {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            workspace.open_left_panel(ctx);
        });
        workspace.update(&mut app, |workspace, ctx| {
            workspace.left_panel_view.update(ctx, |left_panel, ctx| {
                left_panel.apply_action(&action, ctx);
            });
        });
        workspace.update(&mut app, |workspace, ctx| {
            workspace.focus_active_tab(ctx);
        });
        workspace.update(&mut app, |workspace, ctx| {
            assert!(
                workspace
                    .active_tab_pane_group()
                    .as_ref(ctx)
                    .left_panel_open,
                "left panel must stay open before FocusLeftPanel for {action:?}"
            );
            assert!(
                workspace
                    .active_tab_pane_group()
                    .is_self_or_child_focused(ctx),
                "test setup should start from terminal focus before FocusLeftPanel for {action:?}"
            );
            assert!(
                !workspace.left_panel_view.is_self_or_child_focused(ctx),
                "test setup should start outside the unified left panel before {action:?}"
            );
        });

        workspace.update(&mut app, |workspace, ctx| {
            workspace.focus_left_panel(ctx);
        });
        workspace.update(&mut app, |workspace, ctx| {
            assert!(
                workspace.left_panel_view.is_focused(ctx)
                    || workspace.left_panel_view.is_self_or_child_focused(ctx),
                "FocusLeftPanel from terminal must enter the active unified left panel for {action:?}"
            );
        });

        workspace.update(&mut app, |workspace, ctx| {
            workspace.focus_left_panel(ctx);
        });
        workspace.update(&mut app, |workspace, ctx| {
            assert!(
                workspace.active_tab_pane_group().is_self_or_child_focused(ctx),
                "FocusLeftPanel from unified left panel should cycle back to terminal when no right panel is open for {action:?}"
            );
        });
    });
}

#[test]
fn test_focus_left_panel_enters_project_explorer_and_cycles_back_to_terminal() {
    assert_focus_left_panel_enters_unified_tool_panel_and_cycles_back_to_terminal(
        LeftPanelAction::ProjectExplorer,
    );
}

#[test]
fn test_focus_left_panel_enters_skill_manager_and_cycles_back_to_terminal() {
    assert_focus_left_panel_enters_unified_tool_panel_and_cycles_back_to_terminal(
        LeftPanelAction::SkillManager,
    );
}

#[test]
fn test_focus_notebook() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        let pane_group = workspace.read(&app, |workspace, _ctx| {
            workspace
                .get_pane_group_view(0)
                .expect("should have pane group for tab 0")
                .clone()
        });

        let first_terminal_id = pane_group.read(&app, |panes, _ctx| {
            get_newly_created_pane_id(panes, &[])
                .as_terminal_pane_id()
                .expect("should be a terminal pane")
        });

        let notebook_id = pane_group.update(&mut app, |panes, ctx| {
            // Add a notebook to the left.
            let notebook_view = ctx.add_typed_action_view(NotebookView::new);
            panes.add_pane_with_direction(
                Direction::Left,
                NotebookPane::new(notebook_view, ctx),
                true, /* focus_new_pane */
                ctx,
            );
            get_newly_created_pane_id(panes, &[first_terminal_id.into()])
        });

        // The new pane should be focused, but the terminal is still the active session.
        pane_group.read(&app, |panes, ctx| {
            assert_eq!(panes.focused_pane_id(ctx), notebook_id);
            assert_eq!(panes.active_session_id(ctx), Some(first_terminal_id));
            assert_eq!(
                split_pane_state(panes, first_terminal_id, ctx),
                SplitPaneState::InSplitPane(PaneState::Unfocused)
            );
            assert_eq!(
                active_session_state(panes, first_terminal_id, ctx),
                ActiveSessionState::Active
            );
            assert_eq!(
                split_pane_state(panes, notebook_id, ctx),
                SplitPaneState::InSplitPane(PaneState::Focused)
            );
        });

        // Add a terminal below.
        let second_terminal_id = pane_group.update(&mut app, |panes, ctx| {
            panes.add_terminal_pane_with_options(
                Direction::Down,
                NewTerminalOptions::default(),
                ctx,
            );
            get_newly_created_pane_id(panes, &[first_terminal_id.into(), notebook_id])
                .as_terminal_pane_id()
                .expect("should be a terminal pane")
        });

        // The new terminal should be both focused and the active session.
        pane_group.read(&app, |panes, ctx| {
            assert_eq!(panes.focused_pane_id(ctx), second_terminal_id.into());
            assert_eq!(panes.active_session_id(ctx), Some(second_terminal_id));
            assert_eq!(
                split_pane_state(panes, first_terminal_id, ctx),
                SplitPaneState::InSplitPane(PaneState::Unfocused)
            );
            assert_eq!(
                active_session_state(panes, first_terminal_id, ctx),
                ActiveSessionState::Inactive
            );
            assert_eq!(
                split_pane_state(panes, second_terminal_id, ctx),
                SplitPaneState::InSplitPane(PaneState::Focused)
            );
            assert_eq!(
                active_session_state(panes, second_terminal_id, ctx),
                ActiveSessionState::Active
            );
            assert_eq!(
                split_pane_state(panes, notebook_id, ctx),
                SplitPaneState::InSplitPane(PaneState::Unfocused)
            );
        });

        // Close the new terminal.
        pane_group.update(&mut app, |panes, ctx| {
            panes.close_pane(second_terminal_id.into(), ctx);
        });

        // Focus should switch to the notebook, and the first terminal session
        // will activate.
        pane_group.read(&app, |panes, ctx| {
            assert_eq!(panes.focused_pane_id(ctx), notebook_id);
            assert_eq!(panes.active_session_id(ctx), Some(first_terminal_id));
            assert_eq!(
                split_pane_state(panes, first_terminal_id, ctx),
                SplitPaneState::InSplitPane(PaneState::Unfocused)
            );
            assert_eq!(
                split_pane_state(panes, notebook_id, ctx),
                SplitPaneState::InSplitPane(PaneState::Focused)
            );
            assert_eq!(
                active_session_state(panes, first_terminal_id, ctx),
                ActiveSessionState::Active
            );
        });
    })
}

#[test]
fn test_close_active_session() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        let pane_group = workspace.read(&app, |workspace, _ctx| {
            workspace
                .get_pane_group_view(0)
                .expect("should have pane group for tab 0")
                .clone()
        });

        let first_terminal_id = pane_group.read(&app, |panes, _ctx| {
            get_newly_created_pane_id(panes, &[])
                .as_terminal_pane_id()
                .expect("should be a terminal pane")
        });

        // Add a terminal above.
        let second_terminal_id = pane_group.update(&mut app, |panes, ctx| {
            panes.add_terminal_pane_with_options(Direction::Up, NewTerminalOptions::default(), ctx);
            get_newly_created_pane_id(panes, &[first_terminal_id.into()])
                .as_terminal_pane_id()
                .expect("should be a terminal pane")
        });

        let notebook_id = pane_group.update(&mut app, |panes, ctx| {
            // Add a notebook to the left.
            let notebook_view = ctx.add_typed_action_view(NotebookView::new);
            panes.add_pane_with_direction(
                Direction::Left,
                NotebookPane::new(notebook_view, ctx),
                true, /* focus_new_pane */
                ctx,
            );
            get_newly_created_pane_id(
                panes,
                &[first_terminal_id.into(), second_terminal_id.into()],
            )
        });

        pane_group.read(&app, |panes, ctx| {
            assert_eq!(panes.focused_pane_id(ctx), notebook_id);
            assert_eq!(panes.active_session_id(ctx), Some(second_terminal_id));
        });

        pane_group.update(&mut app, |panes, ctx| {
            // Close the active session, which should leave the notebook focused and activate the
            // remaining session.
            panes.close_pane(second_terminal_id.into(), ctx);
        });

        pane_group.read(&app, |panes, ctx| {
            assert_eq!(panes.focused_pane_id(ctx), notebook_id);
            assert_eq!(panes.active_session_id(ctx), Some(first_terminal_id));
            assert_eq!(
                split_pane_state(panes, first_terminal_id, ctx),
                SplitPaneState::InSplitPane(PaneState::Unfocused)
            );
            assert_eq!(
                active_session_state(panes, first_terminal_id, ctx),
                ActiveSessionState::Active
            );
        });

        pane_group.update(&mut app, |panes, ctx| {
            // Now, focus the remaining session, which should keep it activated.
            panes.focus_pane_by_id(first_terminal_id.into(), ctx);
        });

        pane_group.read(&app, |panes, ctx| {
            assert_eq!(panes.focused_pane_id(ctx), first_terminal_id.into());
            assert_eq!(panes.active_session_id(ctx), Some(first_terminal_id));
            assert_eq!(
                split_pane_state(panes, first_terminal_id, ctx),
                SplitPaneState::InSplitPane(PaneState::Focused)
            );
            assert_eq!(
                split_pane_state(panes, notebook_id, ctx),
                SplitPaneState::InSplitPane(PaneState::Unfocused)
            );
            assert_eq!(
                active_session_state(panes, first_terminal_id, ctx),
                ActiveSessionState::Active
            );
        });
    });
}

fn set_left_panel_visibility_across_tabs(is_enabled: bool, ctx: &mut ViewContext<Workspace>) {
    WindowSettings::handle(ctx).update(ctx, |window_settings, ctx| {
        window_settings
            .left_panel_visibility_across_tabs
            .set_value(is_enabled, ctx)
            .expect("Failed to update left_panel_visibility_across_tabs setting");
    });
}

fn add_get_started_tab(workspace: &mut Workspace, ctx: &mut ViewContext<Workspace>) {
    workspace.add_tab_with_pane_layout(
        PanesLayout::Snapshot(Box::new(PaneNodeSnapshot::Leaf(LeafSnapshot {
            is_focused: true,
            custom_vertical_tabs_title: None,
            contents: LeafContents::GetStarted,
        }))),
        Arc::new(HashMap::<PaneUuid, Vec<SerializedBlockListItem>>::new()),
        None,
        ctx,
    );
}

fn find_terminal_tab_index(workspace: &Workspace, ctx: &AppContext) -> usize {
    workspace
        .tabs
        .iter()
        .position(|tab| tab.pane_group.as_ref(ctx).has_terminal_panes())
        .expect("Expected a terminal tab")
}

fn find_non_following_tab_index(workspace: &Workspace, ctx: &AppContext) -> usize {
    workspace
        .tabs
        .iter()
        .position(|tab| {
            !Workspace::should_enable_file_tree_and_global_search_for_pane_group(
                tab.pane_group.as_ref(ctx),
            )
        })
        .expect("Expected a non-following tab")
}

#[test]
fn test_left_panel_window_scoped_reconciles_between_terminal_tabs_when_enabled() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            set_left_panel_visibility_across_tabs(true, ctx);

            workspace.add_terminal_tab(false, ctx);

            workspace.activate_tab(0, ctx);
            assert!(
                !workspace
                    .active_tab_pane_group()
                    .as_ref(ctx)
                    .left_panel_open
            );
            assert!(!workspace.left_panel_open);

            workspace.open_left_panel(ctx);
            assert!(
                workspace
                    .active_tab_pane_group()
                    .as_ref(ctx)
                    .left_panel_open
            );
            assert!(workspace.left_panel_open);

            workspace.activate_tab(1, ctx);
            assert!(
                workspace
                    .active_tab_pane_group()
                    .as_ref(ctx)
                    .left_panel_open
            );

            workspace.close_left_panel(ctx);
            assert!(
                !workspace
                    .active_tab_pane_group()
                    .as_ref(ctx)
                    .left_panel_open
            );
            assert!(!workspace.left_panel_open);

            workspace.activate_tab(0, ctx);
            assert!(
                !workspace
                    .active_tab_pane_group()
                    .as_ref(ctx)
                    .left_panel_open
            );
        });
    });
}

#[test]
fn test_left_panel_window_scoped_non_following_tab_does_not_reconcile_but_updates_window_state() {
    let _get_started_guard = FeatureFlag::GetStartedTab.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            set_left_panel_visibility_across_tabs(true, ctx);

            // Establish window-scoped desired state = open on a terminal tab.
            workspace.open_left_panel(ctx);
            assert!(workspace.left_panel_open);

            // Create a non-following tab (e.g. Get Started), which should not auto-open even though
            // the window state is open.
            add_get_started_tab(workspace, ctx);
            let non_following_tab_index = find_non_following_tab_index(workspace, ctx);
            workspace.activate_tab(non_following_tab_index, ctx);

            assert!(
                !workspace
                    .active_tab_pane_group()
                    .as_ref(ctx)
                    .left_panel_open
            );
            assert!(workspace.left_panel_open);

            // User actions in the non-following tab still update window state.
            workspace.open_left_panel(ctx);
            assert!(
                workspace
                    .active_tab_pane_group()
                    .as_ref(ctx)
                    .left_panel_open
            );
            assert!(workspace.left_panel_open);

            workspace.close_left_panel(ctx);
            assert!(
                !workspace
                    .active_tab_pane_group()
                    .as_ref(ctx)
                    .left_panel_open
            );
            assert!(!workspace.left_panel_open);

            // The window state should reconcile back onto following tabs.
            let terminal_tab_index = find_terminal_tab_index(workspace, ctx);
            workspace.activate_tab(terminal_tab_index, ctx);
            assert!(
                !workspace
                    .active_tab_pane_group()
                    .as_ref(ctx)
                    .left_panel_open
            );

            // But toggling the window state from a following tab should not auto-open the
            // non-following tab.
            workspace.open_left_panel(ctx);
            assert!(workspace.left_panel_open);

            workspace.activate_tab(non_following_tab_index, ctx);
            assert!(
                !workspace
                    .active_tab_pane_group()
                    .as_ref(ctx)
                    .left_panel_open
            );
            assert!(workspace.left_panel_open);
        });
    });
}

#[test]
fn test_left_panel_window_scoped_disabled_keeps_per_tab_state() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            set_left_panel_visibility_across_tabs(false, ctx);

            workspace.add_terminal_tab(false, ctx);

            // Open left panel on tab 0.
            workspace.activate_tab(0, ctx);
            workspace.open_left_panel(ctx);
            assert!(
                workspace
                    .active_tab_pane_group()
                    .as_ref(ctx)
                    .left_panel_open
            );

            // With window scoping disabled, switching tabs should not reconcile the open state.
            workspace.activate_tab(1, ctx);
            assert!(
                !workspace
                    .active_tab_pane_group()
                    .as_ref(ctx)
                    .left_panel_open
            );

            // Each tab can be toggled independently.
            workspace.open_left_panel(ctx);
            assert!(
                workspace
                    .active_tab_pane_group()
                    .as_ref(ctx)
                    .left_panel_open
            );

            workspace.activate_tab(0, ctx);
            assert!(
                workspace
                    .active_tab_pane_group()
                    .as_ref(ctx)
                    .left_panel_open
            );
        });
    });
}

#[test]
fn test_vertical_tabs_panel_visibility_restores_from_window_snapshot() {
    let _vertical_tabs_guard = FeatureFlag::VerticalTabs.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        app.update(|ctx| {
            TabSettings::handle(ctx).update(ctx, |settings, ctx| {
                report_if_error!(settings.use_vertical_tabs.set_value(true, ctx));
                report_if_error!(settings
                    .show_vertical_tab_panel_in_restored_windows
                    .set_value(false, ctx));
            });
        });

        let workspace = mock_workspace(&mut app);

        let closed_snapshot = workspace.update(&mut app, |workspace, ctx| {
            workspace.vertical_tabs_panel_open = false;
            workspace.snapshot(ctx.window_id(), false, ctx)
        });
        let open_snapshot = workspace.update(&mut app, |workspace, ctx| {
            workspace.vertical_tabs_panel_open = true;
            workspace.snapshot(ctx.window_id(), false, ctx)
        });

        let restored_closed = restored_workspace(&mut app, closed_snapshot);
        let restored_open = restored_workspace(&mut app, open_snapshot);

        restored_closed.read(&app, |workspace, _| {
            assert!(!workspace.vertical_tabs_panel_open);
        });
        restored_open.read(&app, |workspace, _| {
            assert!(workspace.vertical_tabs_panel_open);
        });
    });
}

#[test]
fn test_vertical_tabs_panel_restored_open_when_show_in_restored_windows_enabled() {
    let _vertical_tabs_guard = FeatureFlag::VerticalTabs.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        app.update(|ctx| {
            TabSettings::handle(ctx).update(ctx, |settings, ctx| {
                report_if_error!(settings.use_vertical_tabs.set_value(true, ctx));
                report_if_error!(settings
                    .show_vertical_tab_panel_in_restored_windows
                    .set_value(true, ctx));
            });
        });

        let workspace = mock_workspace(&mut app);

        let closed_snapshot = workspace.update(&mut app, |workspace, ctx| {
            workspace.vertical_tabs_panel_open = false;
            workspace.snapshot(ctx.window_id(), false, ctx)
        });

        let restored = restored_workspace(&mut app, closed_snapshot);
        restored.read(&app, |workspace, _| {
            assert!(workspace.vertical_tabs_panel_open);
        });
    });
}

#[test]
fn test_vertical_tabs_panel_defaults_open_for_new_window_when_vertical_tabs_enabled() {
    let _vertical_tabs_guard = FeatureFlag::VerticalTabs.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        app.update(|ctx| {
            TabSettings::handle(ctx).update(ctx, |settings, ctx| {
                report_if_error!(settings.use_vertical_tabs.set_value(true, ctx));
            });
        });

        let workspace = mock_workspace(&mut app);

        workspace.read(&app, |workspace, _| {
            assert!(workspace.vertical_tabs_panel_open);
        });
    });
}

#[test]
fn test_vertical_tabs_panel_inherits_transferred_tab_source_window_state() {
    let _vertical_tabs_guard = FeatureFlag::VerticalTabs.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        app.update(|ctx| {
            TabSettings::handle(ctx).update(ctx, |settings, ctx| {
                report_if_error!(settings.use_vertical_tabs.set_value(true, ctx));
            });
        });

        let transferred_closed = transferred_tab_workspace(&mut app, false);
        let transferred_open = transferred_tab_workspace(&mut app, true);

        transferred_closed.read(&app, |workspace, _| {
            assert!(!workspace.vertical_tabs_panel_open);
        });
        transferred_open.read(&app, |workspace, _| {
            assert!(workspace.vertical_tabs_panel_open);
        });
    });
}

#[test]
fn test_vertical_tabs_panel_auto_shows_when_setting_enabled() {
    let _vertical_tabs_guard = FeatureFlag::VerticalTabs.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        app.update(|ctx| {
            TabSettings::handle(ctx).update(ctx, |settings, ctx| {
                report_if_error!(settings.use_vertical_tabs.set_value(false, ctx));
            });
        });

        let workspace = mock_workspace(&mut app);

        workspace.read(&app, |workspace, _| {
            assert!(!workspace.vertical_tabs_panel_open);
        });

        // Enabling vertical tabs should auto-open the panel.
        workspace.update(&mut app, |_, ctx| {
            TabSettings::handle(ctx).update(ctx, |settings, ctx| {
                report_if_error!(settings.use_vertical_tabs.set_value(true, ctx));
            });
        });
        workspace.read(&app, |workspace, _| {
            assert!(workspace.vertical_tabs_panel_open);
        });

        // Disabling vertical tabs should auto-close the panel.
        workspace.update(&mut app, |_, ctx| {
            TabSettings::handle(ctx).update(ctx, |settings, ctx| {
                report_if_error!(settings.use_vertical_tabs.set_value(false, ctx));
            });
        });
        workspace.read(&app, |workspace, _| {
            assert!(!workspace.vertical_tabs_panel_open);
        });
    });
}

#[test]
fn test_toggle_tab_configs_menu_opens_vertical_tabs_panel_and_menu() {
    let _vertical_tabs_guard = FeatureFlag::VerticalTabs.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            TabSettings::handle(ctx).update(ctx, |settings, ctx| {
                report_if_error!(settings.use_vertical_tabs.set_value(true, ctx));
            });
            workspace.vertical_tabs_panel_open = true;
        });
        workspace.update(&mut app, |workspace, ctx| {
            workspace.vertical_tabs_panel_open = false;
            workspace.show_new_session_dropdown_menu = None;

            workspace.handle_action(&WorkspaceAction::ToggleTabConfigsMenu, ctx);

            assert!(workspace.vertical_tabs_panel_open);
            assert!(workspace.show_new_session_dropdown_menu.is_some());
        });
    });
}

#[test]
fn test_toggle_tab_configs_menu_keyboard_shortcut_selects_top_item() {
    let _tab_configs_guard = FeatureFlag::TabConfigs.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            workspace.show_new_session_dropdown_menu = None;

            workspace.handle_action(&WorkspaceAction::ToggleTabConfigsMenu, ctx);

            assert!(workspace.show_new_session_dropdown_menu.is_some());
            assert_eq!(
                workspace
                    .new_session_dropdown_menu
                    .read(ctx, |menu, _| menu.selected_index()),
                Some(0)
            );
        });
    });
}

#[test]
fn test_pointer_opened_tab_configs_menu_does_not_select_top_item() {
    let _tab_configs_guard = FeatureFlag::TabConfigs.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            workspace.toggle_new_session_dropdown_menu(Vector2F::zero(), false, ctx);

            assert!(workspace.show_new_session_dropdown_menu.is_some());
            assert_eq!(
                workspace
                    .new_session_dropdown_menu
                    .read(ctx, |menu, _| menu.selected_index()),
                None
            );
        });
    });
}

#[test]
fn test_open_tab_config_with_params_does_not_use_worktree_branch_as_implicit_title() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        let tab_config = crate::tab_configs::TabConfig {
            name: "Untitled worktree".to_string(),
            title: None,
            color: None,
            panes: vec![TabConfigPaneNode {
                id: "main".to_string(),
                pane_type: Some(TabConfigPaneType::Terminal),
                split: None,
                children: None,
                is_focused: Some(true),
                directory: None,
                commands: Some(vec!["echo {{autogenerated_branch_name}}".to_string()]),
                shell: None,
            }],
            params: HashMap::new(),
            source_path: None,
        };

        workspace.update(&mut app, |workspace, ctx| {
            workspace.open_tab_config_with_params(
                tab_config.clone(),
                HashMap::new(),
                Some("mesa-coyote"),
                ctx,
            );
        });

        workspace.read(&app, |workspace, ctx| {
            assert_eq!(workspace.tab_count(), 2);
            assert_eq!(
                workspace
                    .active_tab_pane_group()
                    .as_ref(ctx)
                    .custom_title(ctx),
                None
            );
        });
    });
}

#[test]
fn test_open_tab_config_with_params_uses_explicit_title_template() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        let tab_config = crate::tab_configs::TabConfig {
            name: "Titled worktree".to_string(),
            title: Some("{{autogenerated_branch_name}}".to_string()),
            color: None,
            panes: vec![TabConfigPaneNode {
                id: "main".to_string(),
                pane_type: Some(TabConfigPaneType::Terminal),
                split: None,
                children: None,
                is_focused: Some(true),
                directory: None,
                commands: Some(vec!["echo {{autogenerated_branch_name}}".to_string()]),
                shell: None,
            }],
            params: HashMap::new(),
            source_path: None,
        };

        workspace.update(&mut app, |workspace, ctx| {
            workspace.open_tab_config_with_params(
                tab_config.clone(),
                HashMap::new(),
                Some("mesa-coyote"),
                ctx,
            );
        });

        workspace.read(&app, |workspace, ctx| {
            assert_eq!(workspace.tab_count(), 2);
            assert_eq!(
                workspace
                    .active_tab_pane_group()
                    .as_ref(ctx)
                    .custom_title(ctx),
                Some("mesa-coyote".to_string())
            );
        });
    });
}
#[test]
fn test_toggle_tab_configs_menu_does_not_change_vertical_tabs_panel_in_horizontal_mode() {
    let _vertical_tabs_guard = FeatureFlag::VerticalTabs.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            TabSettings::handle(ctx).update(ctx, |settings, ctx| {
                report_if_error!(settings.use_vertical_tabs.set_value(false, ctx));
            });
            workspace.vertical_tabs_panel_open = true;
            workspace.show_new_session_dropdown_menu = None;

            workspace.handle_action(&WorkspaceAction::ToggleTabConfigsMenu, ctx);

            assert!(workspace.vertical_tabs_panel_open);
            assert!(workspace.show_new_session_dropdown_menu.is_some());
        });
    });
}

#[test]
fn test_unified_new_session_menu_uses_new_worktree_config_label_and_order() {
    let _tab_configs_guard = FeatureFlag::TabConfigs.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let labels = workspace
                .unified_new_session_menu_items(ctx)
                .iter()
                .map(new_session_menu_label)
                .collect::<Vec<_>>();

            assert!(!labels.iter().any(|label| label == "Worktree in"));

            // The worktree-config entry is grouped under its own separator with the
            // "New tab config" entry immediately after it. Anchor on the entry itself
            // rather than the first "---" in the menu, since AI-enabled mocks insert an
            // earlier separator before the Agent item.
            let worktree_config_index = labels
                .iter()
                .position(|label| label == "New worktree config")
                .expect("expected a 'New worktree config' entry in the new-session menu");

            assert_eq!(
                labels.get(worktree_config_index - 1),
                Some(&"---".to_string()),
                "expected the worktree-config entry to start its own separated group"
            );
            assert_eq!(
                labels.get(worktree_config_index + 1),
                Some(&"New tab config".to_string())
            );
        });
    });
}

#[test]
fn test_unified_new_session_menu_includes_reopen_closed_session() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let menu_items = workspace.unified_new_session_menu_items(ctx);
            assert!(matches!(
                menu_items.get(menu_items.len() - 2),
                Some(MenuItem::Separator)
            ));

            let reopen_item = reopen_closed_session_menu_item(&menu_items);
            assert!(reopen_item.is_disabled());
            assert!(matches!(
                reopen_item.on_select_action(),
                Some(action) if matches!(action, WorkspaceAction::ReopenClosedSession)
            ));

            workspace.add_terminal_tab(false, ctx);
            workspace.remove_tab(workspace.active_tab_index(), true, true, ctx);

            let menu_items = workspace.unified_new_session_menu_items(ctx);
            let reopen_item = reopen_closed_session_menu_item(&menu_items);
            assert!(!reopen_item.is_disabled());
        });
    });
}

#[test]
fn test_vertical_tabs_context_menu_does_not_show_hover_only_tab_bar() {
    let _full_screen_zen_mode_guard = FeatureFlag::FullScreenZenMode.override_enabled(true);
    let _vertical_tabs_guard = FeatureFlag::VerticalTabs.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            TabSettings::handle(ctx).update(ctx, |settings, ctx| {
                report_if_error!(settings
                    .workspace_decoration_visibility
                    .set_value(WorkspaceDecorationVisibility::OnHover, ctx));
                report_if_error!(settings.use_vertical_tabs.set_value(true, ctx));
            });
            workspace.should_show_ai_assistant_warm_welcome = false;
            workspace.vertical_tabs_panel_open = true;

            workspace.show_tab_right_click_menu =
                Some((0, TabContextMenuAnchor::Pointer(Vector2F::zero())));

            assert_eq!(workspace.tab_bar_mode(ctx), ShowTabBar::Hidden);
        });
    });
}

#[test]
fn test_standard_tab_context_menu_shows_hover_only_tab_bar() {
    let _full_screen_zen_mode_guard = FeatureFlag::FullScreenZenMode.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            TabSettings::handle(ctx).update(ctx, |settings, ctx| {
                report_if_error!(settings
                    .workspace_decoration_visibility
                    .set_value(WorkspaceDecorationVisibility::OnHover, ctx));
            });
            workspace.should_show_ai_assistant_warm_welcome = false;

            workspace.show_tab_right_click_menu =
                Some((0, TabContextMenuAnchor::Pointer(Vector2F::zero())));

            assert_eq!(workspace.tab_bar_mode(ctx), ShowTabBar::Stacked);
        });
    });
}

#[test]
fn test_left_panel_default_views_drop_session_navigator_and_demote_ssh_manager() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        app.read(|ctx| {
            let views = Workspace::compute_left_panel_views(ctx);
            assert!(views.contains(&ToolPanelView::ProjectExplorer));
            assert!(!views.contains(&ToolPanelView::EnvironmentProviderManager));
        });
    });
}

#[test]
fn test_left_panel_snapshot_restore_ignores_unavailable_advanced_view() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            workspace.left_panel_view.update(ctx, |left_panel, ctx| {
                assert_eq!(left_panel.active_view(), ToolPanelView::ProjectExplorer);

                left_panel.restore_active_view_from_snapshot(
                    ToolPanelView::EnvironmentProviderManager,
                    ctx,
                );
                assert_eq!(left_panel.active_view(), ToolPanelView::ProjectExplorer);

                left_panel.apply_action(&LeftPanelAction::EnvironmentProviderManager, ctx);
                assert_eq!(
                    left_panel.active_view(),
                    ToolPanelView::EnvironmentProviderManager
                );
            });
        });
    });
}

#[test]
fn test_workspace_session_context_menu_exposes_session_bridge_actions_for_ai_session() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let active_conversation_id = AIConversationId::new();
            let older_conversation_id = AIConversationId::new();
            let session = WorkspaceSessionSnapshot {
                id: "session-bridge-ai-session".to_string(),
                kind: WorkspaceSessionKind::AgentTerminal,
                label: Some("SessionBridge AI session".to_string()),
                environment_authority_key: None,
                cwd: Some("/Users/admin/ashide".to_string()),
                startup_directory: None,
                cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
                cli_command: Some("codex".to_string()),
                cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                conversation_ids: vec![older_conversation_id.to_string()],
                active_conversation_id: Some(active_conversation_id.to_string()),
                cli_agent_session_id: None,
                is_active: false,
                is_pinned: false,
                updated_at_unix_ms: None,
            };
            let target = WorkspaceSessionActionTarget::new(
                session.id.clone(),
                session.environment_authority_key.clone(),
            );
            workspace.restored_workspace_sessions.push(session);

            workspace.show_workspace_session_context_menu(&target, Vector2F::zero(), ctx);

            let menu_items = workspace.tab_right_click_menu.read(ctx, |menu, _| {
                menu.items()
                    .iter()
                    .map(new_session_menu_label)
                    .collect::<Vec<_>>()
            });
            assert_eq!(
                menu_items,
                vec![
                    crate::t!("workspace-session-navigator-menu-restore"),
                    "---".to_string(),
                    crate::t!("workspace-session-bridge-fork-to-target", target = "Ashide"),
                    crate::t!("workspace-session-bridge-fork-to-target", target = "Codex"),
                    crate::t!("workspace-session-bridge-fork-to-target", target = "Claude"),
                    crate::t!("workspace-session-bridge-edit-and-fork"),
                    crate::t!("workspace-session-bridge-export-bundle"),
                    "---".to_string(),
                    crate::t!("workspace-session-navigator-menu-pin"),
                    crate::t!("workspace-session-navigator-menu-rename-alias"),
                    "---".to_string(),
                    crate::t!("workspace-session-navigator-menu-copy-id"),
                    "永久删除…".to_string(),
                ]
            );

            let actions = workspace.tab_right_click_menu.read(ctx, |menu, _| {
                menu.items()
                    .iter()
                    .filter_map(|item| item.item_on_select_action().cloned())
                    .collect::<Vec<_>>()
            });
            assert!(
                actions.iter().any(|action| {
                    matches!(
                        action,
                        WorkspaceAction::ForkSessionBridge {
                                source: SessionBridgeActionSource::Conversation { conversation_id, .. },
                                fork_target: SessionBridgeForkTarget::Ashide,
                            } if *conversation_id == active_conversation_id
                    )
                }),
                "Session Navigator must dispatch fork-to-ashide for the active AI conversation"
            );
            assert!(
                actions.iter().any(|action| {
                    matches!(
                        action,
                        WorkspaceAction::ShowSessionBridgeEditDialog {
                                source: SessionBridgeActionSource::Conversation { conversation_id, .. },
                            } if *conversation_id == active_conversation_id
                    )
                }),
                "Session Navigator must dispatch edit-and-fork for the active AI conversation"
            );
            assert!(
                actions.iter().any(|action| {
                    matches!(
                        action,
                        WorkspaceAction::ExportSessionBridgeBundle {
                            source: SessionBridgeActionSource::Conversation { conversation_id, .. },
                        }
                            if *conversation_id == active_conversation_id
                    )
                }),
                "Session Navigator must dispatch export-session-bundle for the active AI conversation"
            );
        });
    });
}

#[test]
fn test_remote_workspace_session_context_menu_carries_source_authority_for_native_agent_fork() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment =
                crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                    "dnyx216".to_string(),
                    &server,
                    Some("/root/project".to_string()),
                    EnvironmentLifecycleState::Connected,
                );
            let authority = environment.authority_key.clone();
            workspace.set_active_tab_environment(environment);

            let conversation_id = AIConversationId::new();
            let session = WorkspaceSessionSnapshot {
                id: "environment-session-bridge-ai-session".to_string(),
                kind: WorkspaceSessionKind::AgentTerminal,
                label: Some("Remote SessionBridge AI session".to_string()),
                environment_authority_key: Some(authority.clone()),
                cwd: Some("/root/project".to_string()),
                startup_directory: None,
                cli_agent: Some(CLIAgent::Claude.to_serialized_name()),
                cli_command: Some("claude".to_string()),
                cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                conversation_ids: Vec::new(),
                active_conversation_id: Some(conversation_id.to_string()),
                cli_agent_session_id: None,
                is_active: false,
                is_pinned: false,
                updated_at_unix_ms: None,
            };
            let target = WorkspaceSessionActionTarget::new(
                session.id.clone(),
                session.environment_authority_key.clone(),
            );
            workspace.restored_workspace_sessions.push(session);

            workspace.show_workspace_session_context_menu(&target, Vector2F::zero(), ctx);

            let actions = workspace.tab_right_click_menu.read(ctx, |menu, _| {
                menu.items()
                    .iter()
                    .filter_map(|item| item.item_on_select_action().cloned())
                    .collect::<Vec<_>>()
            });
            assert!(
                actions.iter().any(|action| matches!(
                    action,
                    WorkspaceAction::ForkSessionBridge {
                        source: SessionBridgeActionSource::Conversation {
                            conversation_id: action_conversation_id,
                            source_environment_authority_key,
                        },
                        fork_target: SessionBridgeForkTarget::Agent(CLIAgent::Claude),
                    } if *action_conversation_id == conversation_id
                        && source_environment_authority_key.as_deref() == Some(authority.as_str())
                )),
                "Environment Session Navigator rows must dispatch native agent fork with the owning authority"
            );
        });
    });
}

#[test]
fn test_vertical_tabs_pane_context_menu_exposes_session_bridge_actions_for_active_conversation() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let _agent_view = FeatureFlag::AgentView.override_enabled(true);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let pane_group = workspace.active_tab_pane_group();
            let locator = PaneViewLocator {
                pane_group_id: pane_group.id(),
                pane_id: pane_group.as_ref(ctx).focused_pane_id(ctx),
            };
            let terminal_view = pane_group
                .as_ref(ctx)
                .terminal_view_from_pane_id(locator.pane_id, ctx)
                .expect("mock workspace should start with a terminal pane");
            let active_conversation_id = terminal_view.update(ctx, |terminal, ctx| {
                terminal
                    .agent_view_controller()
                    .update(ctx, |controller, ctx| {
                        controller
                            .try_enter_agent_view(
                                None,
                                AgentViewEntryOrigin::DefaultSessionMode,
                                ctx,
                            )
                            .expect("agent view should start a test conversation")
                    })
            });

            assert_eq!(
                workspace.active_conversation_id_for_pane_locator(locator, ctx),
                Some(active_conversation_id)
            );

            workspace.toggle_vertical_tabs_pane_context_menu(
                0,
                VerticalTabsPaneContextMenuTarget::ClickedPane(locator),
                Vector2F::zero(),
                ctx,
            );

            let menu_labels = workspace.tab_right_click_menu.read(ctx, |menu, _| {
                menu.items()
                    .iter()
                    .map(new_session_menu_label)
                    .collect::<Vec<_>>()
            });
            assert_eq!(
                &menu_labels[..6],
                &[
                    crate::t!("workspace-session-bridge-fork-to-target", target = "Ashide"),
                    crate::t!("workspace-session-bridge-fork-to-target", target = "Codex"),
                    crate::t!("workspace-session-bridge-fork-to-target", target = "Claude"),
                    crate::t!("workspace-session-bridge-edit-and-fork"),
                    crate::t!("workspace-session-bridge-export-bundle"),
                    "---".to_string(),
                ]
            );

            let actions = workspace.tab_right_click_menu.read(ctx, |menu, _| {
                menu.items()
                    .iter()
                    .filter_map(|item| item.item_on_select_action().cloned())
                    .collect::<Vec<_>>()
            });
            assert!(
                actions.iter().any(|action| matches!(
                    action,
                    WorkspaceAction::ForkSessionBridge {
                        source: SessionBridgeActionSource::ActivePane { locator: action_locator },
                        fork_target: SessionBridgeForkTarget::Ashide,
                    } if *action_locator == locator
                )),
                "current pane context menu must expose fork-to-ashide for the active conversation"
            );
            assert!(
                actions.iter().any(|action| matches!(
                    action,
                    WorkspaceAction::ShowSessionBridgeEditDialog {
                        source: SessionBridgeActionSource::ActivePane { locator: action_locator },
                    } if *action_locator == locator
                )),
                "current pane context menu must expose edit-and-fork for the active conversation"
            );
            assert!(
                actions.iter().any(|action| matches!(
                    action,
                    WorkspaceAction::ExportSessionBridgeBundle {
                        source: SessionBridgeActionSource::ActivePane { locator: action_locator },
                    } if *action_locator == locator
                )),
                "current pane context menu must expose export-session-bundle for the active conversation"
            );
        });
    });
}

#[test]
fn test_workspace_session_context_menu_exposes_session_bridge_for_live_conversation_without_fullscreen_active_pointer(
) {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let pane_group = workspace.active_tab_pane_group();
            let locator = PaneViewLocator {
                pane_group_id: pane_group.id(),
                pane_id: pane_group.as_ref(ctx).focused_pane_id(ctx),
            };
            let terminal_view = pane_group
                .as_ref(ctx)
                .terminal_view_from_pane_id(locator.pane_id, ctx)
                .expect("mock workspace should start with a terminal pane");
            let conversation_id = BlocklistAIHistoryModel::handle(ctx).update(ctx, |history, ctx| {
                history.start_new_conversation(terminal_view.id(), false, false, ctx)
            });

            assert_eq!(
                terminal_view.as_ref(ctx).active_conversation_id(ctx),
                None,
                "test setup must exercise the non-fullscreen/live-history fallback path"
            );
            assert_eq!(
                workspace.active_conversation_id_for_pane_locator(locator, ctx),
                Some(conversation_id)
            );

            let expected_conversation_id = conversation_id.to_string();
            let session = workspace
                .session_navigator_sessions(ctx)
                .into_iter()
                .find(|session| session.conversation_ids.contains(&expected_conversation_id))
                .expect("live Session Navigator row should carry live conversation ids");
            let target = WorkspaceSessionActionTarget::new(
                session.id.clone(),
                session.environment_authority_key.clone(),
            );
            workspace.show_workspace_session_context_menu(&target, Vector2F::zero(), ctx);

            let actions = workspace.tab_right_click_menu.read(ctx, |menu, _| {
                menu.items()
                    .iter()
                    .filter_map(|item| item.item_on_select_action().cloned())
                    .collect::<Vec<_>>()
            });
            assert!(
                actions.iter().any(|action| matches!(
                    action,
                    WorkspaceAction::ForkSessionBridge {
                        source: SessionBridgeActionSource::Conversation {
                            conversation_id: action_conversation_id,
                            ..
                        },
                        fork_target: SessionBridgeForkTarget::Ashide,
                    } if *action_conversation_id == conversation_id
                )),
                "selected live Session Navigator row must expose SessionBridge fork even when fullscreen active_conversation_id is absent"
            );
        });
    });
}

#[test]
fn test_tab_context_menu_exposes_session_bridge_actions_for_active_conversation() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let _agent_view = FeatureFlag::AgentView.override_enabled(true);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let pane_group = workspace.active_tab_pane_group();
            let locator = PaneViewLocator {
                pane_group_id: pane_group.id(),
                pane_id: pane_group
                    .as_ref(ctx)
                    .active_session_id(ctx)
                    .expect("mock workspace should start with an active terminal session")
                    .into(),
            };
            let terminal_view = pane_group
                .as_ref(ctx)
                .terminal_view_from_pane_id(locator.pane_id, ctx)
                .expect("mock workspace should start with a terminal pane");
            terminal_view.update(ctx, |terminal, ctx| {
                terminal
                    .agent_view_controller()
                    .update(ctx, |controller, ctx| {
                        controller
                            .try_enter_agent_view(
                                None,
                                AgentViewEntryOrigin::DefaultSessionMode,
                                ctx,
                            )
                            .expect("agent view should start a test conversation")
                    })
            });

            workspace.toggle_tab_right_click_menu(
                0,
                TabContextMenuAnchor::Pointer(Vector2F::zero()),
                ctx,
            );

            let menu_labels = workspace.tab_right_click_menu.read(ctx, |menu, _| {
                menu.items()
                    .iter()
                    .map(new_session_menu_label)
                    .collect::<Vec<_>>()
            });
            assert_eq!(
                &menu_labels[..6],
                &[
                    crate::t!("workspace-session-bridge-fork-to-target", target = "Ashide"),
                    crate::t!("workspace-session-bridge-fork-to-target", target = "Codex"),
                    crate::t!("workspace-session-bridge-fork-to-target", target = "Claude"),
                    crate::t!("workspace-session-bridge-edit-and-fork"),
                    crate::t!("workspace-session-bridge-export-bundle"),
                    "---".to_string(),
                ]
            );

            let actions = workspace.tab_right_click_menu.read(ctx, |menu, _| {
                menu.items()
                    .iter()
                    .filter_map(|item| item.item_on_select_action().cloned())
                    .collect::<Vec<_>>()
            });
            assert!(
                actions.iter().any(|action| matches!(
                    action,
                    WorkspaceAction::ForkSessionBridge {
                        source: SessionBridgeActionSource::ActivePane { locator: action_locator },
                        fork_target: SessionBridgeForkTarget::Ashide,
                    } if *action_locator == locator
                )),
                "tab context menu must expose fork-to-ashide for the active conversation"
            );
        });
    });
}

#[test]
fn test_session_navigator_activation_never_reuses_current_terminal_for_resume() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let initial_tab_count = workspace.tab_count();
            let initial_terminal_view_id = workspace
                .active_tab_pane_group()
                .as_ref(ctx)
                .active_session_view(ctx)
                .expect("mock workspace should start with a terminal")
                .id();
            assert!(
                CLIAgentSessionsModel::as_ref(ctx)
                    .session(initial_terminal_view_id)
                    .is_none(),
                "test setup must start with a reusable-looking plain terminal"
            );

            let session = WorkspaceSessionSnapshot {
                id: "history-codex-switch-target".to_string(),
                kind: WorkspaceSessionKind::AgentTerminal,
                label: Some("Codex history target".to_string()),
                environment_authority_key: None,
                cwd: Some("/Users/admin/ashide".to_string()),
                startup_directory: None,
                cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
                cli_command: Some("codex".to_string()),
                cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                conversation_ids: Vec::new(),
                active_conversation_id: None,
                cli_agent_session_id: Some("codex-history-switch-target".to_string()),
                is_active: false,
                is_pinned: false,
                updated_at_unix_ms: None,
            };
            let target = WorkspaceSessionActionTarget::new(
                session.id.clone(),
                session.environment_authority_key.clone(),
            );
            workspace.restored_workspace_sessions.push(session);
            workspace.sync_session_navigator_sessions(ctx);

            workspace.activate_restored_workspace_session(&target, ctx);

            assert_eq!(
                workspace.tab_count(),
                initial_tab_count + 1,
                "activating a historical CLI-agent session must open a new tab instead of resuming inside the current live terminal"
            );
            assert!(
                CLIAgentSessionsModel::as_ref(ctx)
                    .session(initial_terminal_view_id)
                    .is_none(),
                "the previously active terminal must not be overwritten or registered as the clicked session"
            );

            let active_terminal_view_id = workspace
                .active_tab_pane_group()
                .as_ref(ctx)
                .active_session_view(ctx)
                .expect("restored session should open an active terminal tab")
                .id();
            assert_ne!(
                active_terminal_view_id, initial_terminal_view_id,
                "the restored session should be backed by a distinct terminal view"
            );
            let restored_session = CLIAgentSessionsModel::as_ref(ctx)
                .session(active_terminal_view_id)
                .expect("new restore tab should be registered as the clicked CLI-agent session");
            assert_eq!(restored_session.agent, CLIAgent::Codex);
            assert_eq!(
                restored_session.session_context.session_id.as_deref(),
                Some("codex-history-switch-target")
            );
        });
    });
}

#[test]
fn test_session_navigator_refresh_preserves_order_when_resume_updates_timestamp() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let first = test_session_navigator_order_session("order-key-resume-a", "A", 10);
            let second = test_session_navigator_order_session("order-key-resume-b", "B", 20);
            workspace.restored_workspace_sessions.push(first.clone());
            workspace.restored_workspace_sessions.push(second);
            workspace.sync_session_navigator_sessions(ctx);
            assert_eq!(
                test_session_navigator_displayed_order(workspace, ctx),
                vec!["order-key-resume-a", "order-key-resume-b"]
            );

            workspace.active_restored_workspace_session_key =
                Some(Workspace::workspace_session_logical_key(&first));
            let session = workspace
                .restored_workspace_sessions
                .iter_mut()
                .find(|session| session.id == "order-key-resume-a")
                .expect("test session exists");
            session.updated_at_unix_ms = Some(10_000);
            workspace.sync_session_navigator_sessions(ctx);

            assert_eq!(
                test_session_navigator_displayed_order(workspace, ctx),
                vec!["order-key-resume-a", "order-key-resume-b"],
                "resume/status refresh must not reorder existing Session Navigator rows"
            );
        });
    });
}

#[test]
fn test_session_navigator_refresh_appends_new_rows_after_existing_order() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            workspace
                .restored_workspace_sessions
                .push(test_session_navigator_order_session(
                    "order-key-append-a",
                    "A",
                    10,
                ));
            workspace
                .restored_workspace_sessions
                .push(test_session_navigator_order_session(
                    "order-key-append-b",
                    "B",
                    20,
                ));
            workspace.sync_session_navigator_sessions(ctx);

            workspace.restored_workspace_sessions.insert(
                0,
                test_session_navigator_order_session("order-key-append-c", "C", 0),
            );
            workspace.sync_session_navigator_sessions(ctx);

            assert_eq!(
                test_session_navigator_displayed_order(workspace, ctx),
                vec![
                    "order-key-append-a",
                    "order-key-append-b",
                    "order-key-append-c"
                ],
                "manual refresh should reconcile rows and append newly discovered sessions"
            );
        });
    });
}

#[test]
fn test_session_navigator_refresh_prunes_missing_order_keys() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let first = test_session_navigator_order_session("order-key-prune-a", "A", 10);
            let second = test_session_navigator_order_session("order-key-prune-b", "B", 20);
            let first_order_key = Workspace::workspace_session_display_order_key(&first);
            let second_order_key = Workspace::workspace_session_display_order_key(&second);
            workspace.restored_workspace_sessions.push(first);
            workspace.restored_workspace_sessions.push(second);
            workspace.sync_session_navigator_sessions(ctx);
            assert!(workspace
                .session_navigator_display_order
                .contains_key(&second_order_key));

            workspace
                .restored_workspace_sessions
                .retain(|session| session.id != "order-key-prune-b");
            workspace.sync_session_navigator_sessions(ctx);

            assert!(workspace
                .session_navigator_display_order
                .contains_key(&first_order_key));
            assert!(
                !workspace
                    .session_navigator_display_order
                    .contains_key(&second_order_key),
                "refresh must prune order keys for rows that disappeared"
            );
        });
    });
}

#[test]
fn test_session_navigator_pin_only_changes_group_not_display_order() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            workspace
                .restored_workspace_sessions
                .push(test_session_navigator_order_session(
                    "order-key-pin-a",
                    "A",
                    10,
                ));
            workspace
                .restored_workspace_sessions
                .push(test_session_navigator_order_session(
                    "order-key-pin-b",
                    "B",
                    20,
                ));
            workspace
                .restored_workspace_sessions
                .push(test_session_navigator_order_session(
                    "order-key-pin-c",
                    "C",
                    30,
                ));
            workspace.sync_session_navigator_sessions(ctx);
            assert_eq!(
                test_session_navigator_displayed_order(workspace, ctx),
                vec!["order-key-pin-a", "order-key-pin-b", "order-key-pin-c"]
            );

            workspace
                .restored_workspace_sessions
                .iter_mut()
                .find(|session| session.id == "order-key-pin-b")
                .expect("test session exists")
                .is_pinned = true;
            workspace.sync_session_navigator_sessions(ctx);
            assert_eq!(
                test_session_navigator_displayed_order(workspace, ctx),
                vec!["order-key-pin-b", "order-key-pin-a", "order-key-pin-c"],
                "pin should move the row into the pinned group without reallocating order"
            );

            workspace
                .restored_workspace_sessions
                .iter_mut()
                .find(|session| session.id == "order-key-pin-b")
                .expect("test session exists")
                .is_pinned = false;
            workspace.sync_session_navigator_sessions(ctx);
            assert_eq!(
                test_session_navigator_displayed_order(workspace, ctx),
                vec!["order-key-pin-a", "order-key-pin-b", "order-key-pin-c"],
                "unpin should return the row to its original unpinned position"
            );
        });
    });
}

#[test]
fn test_remote_session_navigator_uses_environment_user_state() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let authority = "ssh:ssh-config:dnyx216".to_string();
            workspace.set_active_tab_environment(crate::app_state::EnvironmentSnapshot {
                authority_key: authority.clone(),
                label: "dnyx216".to_string(),
                kind: crate::app_state::EnvironmentKind::Ssh,
                lifecycle_state: crate::app_state::EnvironmentLifecycleState::Connected,
                active_workspace_root: Some("/root/project".to_string()),
                connection_ref: Some("dnyx216".to_string()),
            });
            let mut session =
                test_environment_runtime_session_snapshot("remote:test", authority.clone());
            session.cli_agent_session_id = Some("remote-provider-session".to_string());
            let logical_key = session.logical_key();
            workspace
                .indexed_environment_cli_agent_sessions
                .insert(authority.clone(), vec![session]);
            workspace
                .indexed_environment_cli_agent_session_user_states
                .insert(
                    authority,
                    crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserState {
                        aliases: HashMap::from([(logical_key.clone(), "Remote Alias".to_string())]),
                        pinned: HashSet::from([logical_key.clone()]),
                    },
                );

            let sessions = workspace.session_navigator_sessions(ctx);

            assert_eq!(sessions.len(), 1);
            assert_eq!(sessions[0].label.as_deref(), Some("Remote Alias"));
            assert!(
                sessions[0].is_pinned,
                "remote session pin must come from the remote environment user-state cache"
            );
        });
    });
}

#[test]
fn test_remote_session_navigator_scan_result_is_source_of_truth() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let authority = "ssh:ssh-config:dnyx216".to_string();
            workspace.set_active_tab_environment(crate::app_state::EnvironmentSnapshot {
                authority_key: authority.clone(),
                label: "dnyx216".to_string(),
                kind: crate::app_state::EnvironmentKind::Ssh,
                lifecycle_state: crate::app_state::EnvironmentLifecycleState::Connected,
                active_workspace_root: Some("/root/project".to_string()),
                connection_ref: Some("dnyx216".to_string()),
            });
            let mut session =
                test_environment_runtime_session_snapshot("remote:source", authority.clone());
            session.cli_agent_session_id = Some("remote-source-provider-session".to_string());
            workspace
                .indexed_environment_cli_agent_sessions
                .insert(authority.clone(), vec![session]);
            workspace
                .indexed_environment_cli_agent_session_user_states
                .insert(
                    authority,
                    crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserState {
                        aliases: HashMap::new(),
                        pinned: HashSet::new(),
                    },
                );

            let sessions = workspace.session_navigator_sessions(ctx);

            assert_eq!(
                sessions.len(),
                1,
                "Session Navigator must treat provider scan results as the source of truth instead of hiding rows via persisted UI state"
            );
        });
    });
}

#[test]
fn test_workspace_session_context_menu_hides_session_bridge_actions_without_ai_conversation() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let session = WorkspaceSessionSnapshot {
                id: "plain-cli-agent-session".to_string(),
                kind: WorkspaceSessionKind::AgentTerminal,
                label: Some("Plain CLI agent session".to_string()),
                environment_authority_key: None,
                cwd: Some("/Users/admin/ashide".to_string()),
                startup_directory: None,
                cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
                cli_command: Some("codex".to_string()),
                cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                conversation_ids: Vec::new(),
                active_conversation_id: None,
                cli_agent_session_id: Some("codex-provider-session".to_string()),
                is_active: false,
                is_pinned: false,
                updated_at_unix_ms: None,
            };
            let target = WorkspaceSessionActionTarget::new(
                session.id.clone(),
                session.environment_authority_key.clone(),
            );
            workspace.restored_workspace_sessions.push(session);

            workspace.show_workspace_session_context_menu(&target, Vector2F::zero(), ctx);

            let menu_items = workspace.tab_right_click_menu.read(ctx, |menu, _| {
                menu.items()
                    .iter()
                    .map(new_session_menu_label)
                    .collect::<Vec<_>>()
            });
            assert!(
                !menu_items.contains(&crate::t!(
                    "workspace-session-bridge-fork-to-target",
                    target = "Ashide"
                ))
            );
            assert!(!menu_items.contains(&crate::t!("workspace-session-bridge-edit-and-fork")));
            assert!(!menu_items.contains(&crate::t!(
                "workspace-session-bridge-export-bundle"
            )));
            assert!(menu_items.contains(&crate::t!(
                "workspace-session-bridge-fork-unavailable"
            )));

            let unavailable_item_is_disabled = workspace.tab_right_click_menu.read(ctx, |menu, _| {
                menu.items().iter().any(|item| {
                    item.fields().is_some_and(|fields| {
                        fields.label()
                            == crate::t!("workspace-session-bridge-fork-unavailable")
                            && fields.is_disabled()
                    })
                })
            });
            assert!(
                unavailable_item_is_disabled,
                "unmapped CLI agent rows must show a disabled fork placeholder instead of silently hiding SessionBridge"
            );
        });
    });
}

#[test]
fn test_workspace_session_context_menu_keeps_pi_fork_blocked_until_adapter_exists() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let session = WorkspaceSessionSnapshot {
                id: format!("external:Pi:{}", hex::encode("/tmp/pi-session.jsonl")),
                kind: WorkspaceSessionKind::AgentTerminal,
                label: Some("Pi session".to_string()),
                environment_authority_key: None,
                cwd: Some("/Users/admin/ashide".to_string()),
                startup_directory: None,
                cli_agent: Some(CLIAgent::Pi.to_serialized_name()),
                cli_command: Some("pi".to_string()),
                cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                conversation_ids: Vec::new(),
                active_conversation_id: None,
                cli_agent_session_id: Some("pi-provider-session".to_string()),
                is_active: false,
                is_pinned: false,
                updated_at_unix_ms: None,
            };
            let target = WorkspaceSessionActionTarget::new(
                session.id.clone(),
                session.environment_authority_key.clone(),
            );
            workspace.restored_workspace_sessions.push(session);

            workspace.show_workspace_session_context_menu(&target, Vector2F::zero(), ctx);

            let (fork_actions, unavailable_item_is_disabled) =
                workspace.tab_right_click_menu.read(ctx, |menu, _| {
                    let fork_actions = menu
                        .items()
                        .iter()
                        .filter_map(|item| item.item_on_select_action())
                        .filter(|action| matches!(
                            action,
                            WorkspaceAction::ForkSessionBridge { .. }
                        ))
                        .count();
                    let unavailable_item_is_disabled = menu.items().iter().any(|item| {
                        item.fields().is_some_and(|fields| {
                            fields.label()
                                == crate::t!("workspace-session-bridge-fork-unavailable")
                                && fields.is_disabled()
                        })
                    });
                    (fork_actions, unavailable_item_is_disabled)
                });

            assert_eq!(
                fork_actions, 0,
                "Pi rows must not expose SessionBridge fork actions before a Pi adapter exists"
            );
            assert!(
                unavailable_item_is_disabled,
                "Pi rows should show an explicit disabled fork placeholder instead of silently pretending conversion is supported"
            );
        });
    });
}

#[test]
fn test_workspace_session_context_menu_forks_indexed_cli_agent_session() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let cli_agent_session_id = "codex-provider-session";
            let session = WorkspaceSessionSnapshot {
                id: format!(
                    "external-index:Codex:{}",
                    hex::encode(cli_agent_session_id.as_bytes())
                ),
                kind: WorkspaceSessionKind::AgentTerminal,
                label: Some("Indexed Codex session".to_string()),
                environment_authority_key: None,
                cwd: Some("/Users/admin/ashide".to_string()),
                startup_directory: None,
                cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
                cli_command: Some("codex".to_string()),
                cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                conversation_ids: Vec::new(),
                active_conversation_id: None,
                cli_agent_session_id: Some(cli_agent_session_id.to_string()),
                is_active: false,
                is_pinned: false,
                updated_at_unix_ms: None,
            };
            let target = WorkspaceSessionActionTarget::new(
                session.id.clone(),
                session.environment_authority_key.clone(),
            );
            workspace.restored_workspace_sessions.push(session);

            workspace.show_workspace_session_context_menu(&target, Vector2F::zero(), ctx);

            let (labels, actions) = workspace.tab_right_click_menu.read(ctx, |menu, _| {
                let labels = menu
                    .items()
                    .iter()
                    .filter_map(|item| item.fields().map(|fields| fields.label().to_owned()))
                    .collect::<Vec<_>>();
                let actions = menu
                    .items()
                    .iter()
                    .filter_map(|item| item.item_on_select_action().cloned())
                    .collect::<Vec<_>>();
                (labels, actions)
            });
            assert!(
                actions.iter().any(|action| matches!(
                    action,
                    WorkspaceAction::ForkSessionBridge {
                        source: SessionBridgeActionSource::WorkspaceTarget { target: action_target },
                        fork_target: SessionBridgeForkTarget::Ashide,
                    }
                        if action_target.session_id == target.session_id
                )),
                "indexed CLI rows must expose a real fork action, not a disabled placeholder"
            );
            assert!(
                labels.contains(&crate::t!("workspace-session-bridge-edit-and-fork")),
                "indexed CLI rows must expose an edit-and-fork menu label"
            );
            assert!(
                labels.contains(&crate::t!(
                    "workspace-session-bridge-export-bundle"
                )),
                "indexed CLI rows must expose a portable bundle export label"
            );
            assert!(
                actions.iter().any(|action| matches!(
                    action,
                    WorkspaceAction::ShowSessionBridgeEditDialog {
                        source: SessionBridgeActionSource::WorkspaceTarget { target: action_target },
                    }
                        if action_target.session_id == target.session_id
                )),
                "indexed CLI rows must dispatch edit-and-fork through the workspace-session source"
            );
            assert!(
                actions.iter().any(|action| matches!(
                    action,
                    WorkspaceAction::ExportSessionBridgeBundle {
                        source: SessionBridgeActionSource::WorkspaceTarget { target: action_target },
                    }
                        if action_target.session_id == target.session_id
                )),
                "indexed CLI rows must dispatch portable bundle export through the workspace-session source"
            );
        });
    });
}

#[test]
fn test_workspace_session_context_menu_forks_live_cli_agent_session_with_indexed_backing_source() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let cli_agent_session_id = "codex-live-provider-session";
            let indexed_session = WorkspaceSessionSnapshot {
                id: format!(
                    "external-index:Codex:{}",
                    hex::encode(cli_agent_session_id.as_bytes())
                ),
                kind: WorkspaceSessionKind::AgentTerminal,
                label: Some("Indexed backing Codex session".to_string()),
                environment_authority_key: Some("local".to_string()),
                cwd: Some("/Users/admin/ashide".to_string()),
                startup_directory: None,
                cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
                cli_command: Some("codex".to_string()),
                cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                conversation_ids: Vec::new(),
                active_conversation_id: None,
                cli_agent_session_id: Some(cli_agent_session_id.to_string()),
                is_active: false,
                is_pinned: false,
                updated_at_unix_ms: None,
            };
            let live_session = WorkspaceSessionSnapshot {
                id: "tab:99:leaf:0".to_string(),
                kind: WorkspaceSessionKind::AgentTerminal,
                label: Some("Running Codex session".to_string()),
                environment_authority_key: Some("local".to_string()),
                cwd: Some("/Users/admin/ashide-live".to_string()),
                startup_directory: None,
                cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
                cli_command: Some("codex".to_string()),
                cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                conversation_ids: Vec::new(),
                active_conversation_id: None,
                cli_agent_session_id: Some(cli_agent_session_id.to_string()),
                is_active: true,
                is_pinned: false,
                updated_at_unix_ms: None,
            };
            workspace.indexed_cli_agent_sessions.push(indexed_session.clone());
            workspace.restored_workspace_sessions.push(live_session.clone());

            let sessions = workspace.session_navigator_sessions(ctx);
            let live_row = sessions
                .iter()
                .find(|session| {
                    session.id == live_session.id
                        && session.cli_agent_session_id.as_deref()
                            == Some(cli_agent_session_id)
                })
                .unwrap_or_else(|| {
                    panic!("live running CLI row should remain selectable; sessions={sessions:#?}")
                });
            assert_eq!(
                workspace
                    .cli_agent_history_source_session_for_workspace_session(live_row)
                    .as_ref()
                    .map(|session| session.id.as_str()),
                Some(indexed_session.id.as_str()),
                "running tab row must resolve its native history source through the indexed backing row"
            );

            let target = WorkspaceSessionActionTarget::new(
                live_row.id.clone(),
                live_row.environment_authority_key.clone(),
            );
            workspace.show_workspace_session_context_menu(&target, Vector2F::zero(), ctx);

            let actions = workspace.tab_right_click_menu.read(ctx, |menu, _| {
                menu.items()
                    .iter()
                    .filter_map(|item| item.item_on_select_action().cloned())
                    .collect::<Vec<_>>()
            });
            assert!(
                actions.iter().any(|action| matches!(
                    action,
                    WorkspaceAction::ForkSessionBridge {
                        source: SessionBridgeActionSource::WorkspaceTarget { target: action_target },
                        fork_target: SessionBridgeForkTarget::Ashide,
                    } if action_target.session_id == live_session.id
                )),
                "selected running CLI row must expose a real fork action via its indexed backing source"
            );
            assert!(
                actions.iter().any(|action| matches!(
                    action,
                    WorkspaceAction::ShowSessionBridgeEditDialog {
                        source: SessionBridgeActionSource::WorkspaceTarget { target: action_target },
                    } if action_target.session_id == live_session.id
                )),
                "selected running CLI row must expose edit-and-fork through its indexed backing source"
            );
            assert!(
                actions.iter().any(|action| matches!(
                    action,
                    WorkspaceAction::ExportSessionBridgeBundle {
                        source: SessionBridgeActionSource::WorkspaceTarget { target: action_target },
                    } if action_target.session_id == live_session.id
                )),
                "selected running CLI row must expose portable bundle export through its indexed backing source"
            );
        });
    });
}

#[test]
fn test_remote_live_cli_agent_session_fork_uses_remote_indexed_backing_source() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment =
                crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                    "dnyx216".to_string(),
                    &server,
                    Some("/root/project".to_string()),
                    EnvironmentLifecycleState::Connected,
                );
            let authority = environment.authority_key.clone();
            workspace.set_active_tab_environment(environment);

            let cli_agent_session_id = "remote-claude-live-session";
            let remote_source = "/root/.claude/projects/-root-project/remote-claude-live-session.jsonl";
            let indexed_session = WorkspaceSessionSnapshot {
                id: crate::workspace::environment_runtime::environment_cli_agent_session_source_id(
                    &authority,
                    &CLIAgent::Claude,
                    remote_source,
                ),
                kind: WorkspaceSessionKind::AgentTerminal,
                label: Some("Remote indexed Claude".to_string()),
                environment_authority_key: Some(authority.clone()),
                cwd: Some("/root/project".to_string()),
                startup_directory: None,
                cli_agent: Some(CLIAgent::Claude.to_serialized_name()),
                cli_command: Some("claude".to_string()),
                cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                conversation_ids: Vec::new(),
                active_conversation_id: None,
                cli_agent_session_id: Some(cli_agent_session_id.to_string()),
                is_active: false,
                is_pinned: false,
                updated_at_unix_ms: None,
            };
            let live_session = WorkspaceSessionSnapshot {
                id: "tab:88:leaf:0".to_string(),
                kind: WorkspaceSessionKind::AgentTerminal,
                label: Some("Running remote Claude".to_string()),
                environment_authority_key: Some(authority.clone()),
                cwd: Some("/root/live-project".to_string()),
                startup_directory: None,
                cli_agent: Some(CLIAgent::Claude.to_serialized_name()),
                cli_command: Some("claude".to_string()),
                cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                conversation_ids: Vec::new(),
                active_conversation_id: None,
                cli_agent_session_id: Some(cli_agent_session_id.to_string()),
                is_active: true,
                is_pinned: false,
                updated_at_unix_ms: None,
            };
            workspace.remember_indexed_environment_cli_agent_sessions(
                authority.clone(),
                vec![indexed_session.clone()],
            );
            workspace.restored_workspace_sessions.push(live_session.clone());

            let sessions = workspace.session_navigator_sessions(ctx);
            let live_row = sessions
                .iter()
                .find(|session| {
                    session.id == live_session.id
                        && session.environment_authority_key.as_deref() == Some(authority.as_str())
                })
                .unwrap_or_else(|| {
                    panic!("remote running CLI row should remain selectable; sessions={sessions:#?}")
                });
            assert_eq!(
                workspace
                    .cli_agent_history_source_session_for_workspace_session(live_row)
                    .as_ref()
                    .map(|session| session.id.as_str()),
                Some(indexed_session.id.as_str()),
                "remote running tab row must resolve to a remote native history source, never a current-app source"
            );

            let target = WorkspaceSessionActionTarget::new(
                live_row.id.clone(),
                live_row.environment_authority_key.clone(),
            );
            workspace.show_workspace_session_context_menu(&target, Vector2F::zero(), ctx);

            let actions = workspace.tab_right_click_menu.read(ctx, |menu, _| {
                menu.items()
                    .iter()
                    .filter_map(|item| item.item_on_select_action().cloned())
                    .collect::<Vec<_>>()
            });
            assert!(
                actions.iter().any(|action| matches!(
                    action,
                    WorkspaceAction::ForkSessionBridge {
                        source: SessionBridgeActionSource::WorkspaceTarget { target: action_target },
                        fork_target: SessionBridgeForkTarget::Agent(CLIAgent::Claude),
                    } if action_target.session_id == live_session.id
                        && action_target.environment_authority_key.as_deref()
                            == Some(authority.as_str())
                )),
                "selected remote running CLI row must dispatch fork with the owning remote authority"
            );
            assert!(
                actions.iter().any(|action| matches!(
                    action,
                    WorkspaceAction::ShowSessionBridgeEditDialog {
                        source: SessionBridgeActionSource::WorkspaceTarget { target: action_target },
                    } if action_target.session_id == live_session.id
                        && action_target.environment_authority_key.as_deref()
                            == Some(authority.as_str())
                )),
                "selected remote running CLI row must dispatch edit-and-fork with the owning remote authority"
            );
            assert!(
                actions.iter().any(|action| matches!(
                    action,
                    WorkspaceAction::ExportSessionBridgeBundle {
                        source: SessionBridgeActionSource::WorkspaceTarget { target: action_target },
                    } if action_target.session_id == live_session.id
                        && action_target.environment_authority_key.as_deref()
                            == Some(authority.as_str())
                )),
                "selected remote running CLI row must dispatch export with the owning remote authority"
            );
        });
    });
}

#[test]
fn test_workspace_session_context_menu_resolves_session_bridge_actions_from_cli_agent_session_id() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let conversation_id = AIConversationId::new();
        let cli_agent_session_id = "codex-provider-session";
        insert_historical_ashide_conversation_with_run_id(
            &mut app,
            conversation_id,
            "Mapped CLI agent session",
            "/Users/admin/ashide",
            Some(cli_agent_session_id),
        );

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let session = WorkspaceSessionSnapshot {
                id: "mapped-cli-agent-session".to_string(),
                kind: WorkspaceSessionKind::AgentTerminal,
                label: Some("Mapped CLI agent session".to_string()),
                environment_authority_key: None,
                cwd: Some("/Users/admin/ashide".to_string()),
                startup_directory: None,
                cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
                cli_command: Some("codex".to_string()),
                cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                conversation_ids: Vec::new(),
                active_conversation_id: None,
                cli_agent_session_id: Some(cli_agent_session_id.to_string()),
                is_active: false,
                is_pinned: false,
                updated_at_unix_ms: None,
            };
            let target = WorkspaceSessionActionTarget::new(
                session.id.clone(),
                session.environment_authority_key.clone(),
            );
            workspace.restored_workspace_sessions.push(session);

            workspace.show_workspace_session_context_menu(&target, Vector2F::zero(), ctx);

            let actions = workspace.tab_right_click_menu.read(ctx, |menu, _| {
                menu.items()
                    .iter()
                    .filter_map(|item| item.item_on_select_action().cloned())
                    .collect::<Vec<_>>()
            });
            assert!(
                actions.iter().any(|action| matches!(
                    action,
                    WorkspaceAction::ForkSessionBridge {
                        source: SessionBridgeActionSource::Conversation {
                            conversation_id: action_conversation_id,
                            ..
                        },
                        fork_target: SessionBridgeForkTarget::Ashide,
                    } if *action_conversation_id == conversation_id
                )),
                "Session Navigator must map CLI agent session ids back to native AI conversations before exposing fork"
            );
        });
    });
}

#[test]
fn test_historical_ashide_conversation_appears_in_session_navigator_with_session_bridge_actions() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let conversation_id = AIConversationId::new();
        let title = "Native Ashide historical session";
        let cwd = "/Users/admin/ashide";
        insert_historical_ashide_conversation(&mut app, conversation_id, title, cwd);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let expected_session_id = Workspace::ashide_conversation_session_id(conversation_id);
            let sessions = workspace.session_navigator_sessions(ctx);
            let session = sessions
                .iter()
                .find(|session| session.id == expected_session_id)
                .expect("historical Ashide conversation should become a Session Navigator row");

            assert_eq!(session.kind, WorkspaceSessionKind::AgentTerminal);
            assert_eq!(session.label.as_deref(), Some(title));
            assert_eq!(session.cwd.as_deref(), Some(cwd));
            let expected_authority = format!("local:{cwd}");
            let expected_conversation_id = conversation_id.to_string();
            assert_eq!(
                session.environment_authority_key.as_deref(),
                Some(expected_authority.as_str())
            );
            assert_eq!(
                session.active_conversation_id.as_deref(),
                Some(expected_conversation_id.as_str())
            );
            assert_eq!(session.conversation_ids, vec![expected_conversation_id]);

            let target = WorkspaceSessionActionTarget::new(
                session.id.clone(),
                session.environment_authority_key.clone(),
            );
            workspace.show_workspace_session_context_menu(&target, Vector2F::zero(), ctx);

            let actions = workspace.tab_right_click_menu.read(ctx, |menu, _| {
                menu.items()
                    .iter()
                    .filter_map(|item| item.item_on_select_action().cloned())
                    .collect::<Vec<_>>()
            });
            assert!(
                actions.iter().any(|action| matches!(
                    action,
                    WorkspaceAction::ForkSessionBridge {
                        source: SessionBridgeActionSource::Conversation {
                            conversation_id: action_conversation_id,
                            source_environment_authority_key,
                        },
                        fork_target: SessionBridgeForkTarget::Ashide,
                    } if *action_conversation_id == conversation_id
                        && source_environment_authority_key.as_deref()
                            == Some(expected_authority.as_str())
                )),
                "historical Ashide session must expose fork-to-ashide"
            );
            assert!(
                actions.iter().any(|action| matches!(
                    action,
                    WorkspaceAction::ShowSessionBridgeEditDialog {
                        source: SessionBridgeActionSource::Conversation {
                            conversation_id: action_conversation_id,
                            source_environment_authority_key,
                        },
                    } if *action_conversation_id == conversation_id
                        && source_environment_authority_key.as_deref()
                            == Some(expected_authority.as_str())
                )),
                "historical Ashide session must expose edit-and-fork"
            );
            assert!(
                actions.iter().any(|action| matches!(
                    action,
                    WorkspaceAction::ExportSessionBridgeBundle {
                        source: SessionBridgeActionSource::Conversation {
                            conversation_id: action_conversation_id,
                            ..
                        },
                    } if *action_conversation_id == conversation_id
                )),
                "historical Ashide session must expose export-session-bundle"
            );
        });
    });
}

#[test]
fn test_activate_historical_ashide_conversation_uses_conversation_restore_path() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let conversation_id = AIConversationId::new();
        insert_historical_ashide_conversation(
            &mut app,
            conversation_id,
            "Historical Ashide activation",
            "/Users/admin/ashide",
        );

        let workspace = mock_workspace(&mut app);

        let (initial_tab_count, expected_active_key, expected_session_id) =
            workspace.update(&mut app, |workspace, ctx| {
                let expected_session_id = Workspace::ashide_conversation_session_id(conversation_id);
                let session = workspace
                    .session_navigator_sessions(ctx)
                    .into_iter()
                    .find(|session| session.id == expected_session_id)
                    .expect("historical Ashide conversation should be restorable");
                let target = WorkspaceSessionActionTarget::new(
                    session.id.clone(),
                    session.environment_authority_key.clone(),
                );
                let expected_active_key = Workspace::workspace_session_logical_key(&session);
                let initial_tab_count = workspace.tab_count();

                workspace.activate_restored_workspace_session(&target, ctx);
                (
                    initial_tab_count,
                    expected_active_key,
                    expected_session_id,
                )
            });

        futures_lite::future::yield_now().await;

        workspace.update(&mut app, |workspace, _ctx| {
            assert_eq!(workspace.tab_count(), initial_tab_count + 1);
            assert_eq!(
                workspace.active_restored_workspace_session_key.as_deref(),
                Some(expected_active_key.as_str())
            );
            assert!(
                !workspace
                    .restoring_workspace_session_keys
                    .contains(&expected_session_id),
                "native Ashide historical sessions should not go through CLI resume restoring state"
            );
            assert!(
                !workspace
                    .restoring_workspace_session_keys
                    .contains(&expected_active_key),
                "native Ashide historical sessions should not mark their logical key as CLI restoring"
            );
        });
    });
}

#[test]
fn test_skill_manager_is_available_for_runtime_backed_environment() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        app.read(|ctx| {
            let local_views = Workspace::compute_left_panel_views_for_environment(
                &crate::workspace::environment_runtime::terminal_bootstrap_environment(None),
                ctx,
            );
            let runtime_views = Workspace::compute_left_panel_views_for_environment(
                &EnvironmentSnapshot::runtime_transport(
                    EnvironmentKind::Ssh,
                    "dnyx216".to_string(),
                    "ssh:ssh-config:dnyx216".to_string(),
                    Some("ssh-config:dnyx216".to_string()),
                    Some("/root".to_string()),
                    EnvironmentLifecycleState::Connected,
                ),
                ctx,
            );

            if cfg!(feature = "local_fs") {
                assert!(
                    local_views.contains(&ToolPanelView::SkillManager),
                    "current-app local environment should keep the local Skill Manager entry"
                );
            }
            assert!(
                runtime_views.contains(&ToolPanelView::SkillManager),
                "runtime-backed environments should expose the runtime-backed Skill Manager entry"
            );
        });
    });
}

#[test]
fn test_skill_manager_scope_uses_connected_runtime_placeholder() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9073);
            let host_id = HostId::new("skill-manager-placeholder-host".to_string());

            workspace.add_restored_environment_runtime_tab(
                environment.clone(),
                Some("root@dnyx216".to_string()),
                ctx,
            );
            let environment_tab_index = workspace
                .tab_index_for_environment_authority(&authority)
                .expect("test setup should create an Environment placeholder tab");
            workspace.environment_runtimes.mark_connecting(
                environment,
                session_id,
                PathBuf::from("/tmp/ashide-test-skill-manager-placeholder.sock"),
            );
            workspace
                .environment_runtimes
                .mark_connected_session(session_id, host_id.clone());
            workspace.activate_tab_internal(environment_tab_index, ctx);
            workspace.update_active_session(ctx);

            let runtime_session_id = workspace
                .environment_runtime_session_for_authority(&authority)
                .expect("activating a placeholder with a missing runtime client should reconnect");
            assert_ne!(
                runtime_session_id, session_id,
                "stale connected runtime target must be replaced before Skill Manager scopes it"
            );
            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                Some(EnvironmentLifecycleState::Connecting)
            );
            assert!(
                workspace.active_tab_contains_environment_runtime_placeholder(ctx),
                "test setup must cover the no-terminal placeholder state"
            );
            assert!(
                ActiveSession::as_ref(ctx).session(ctx.window_id()).is_none(),
                "placeholder state must not expose a terminal session"
            );

            let scope = workspace.active_skill_manager_environment_scope(ctx.window_id(), ctx);
            match scope {
                crate::workspace::SkillManagerEnvironmentScope::EnvironmentRuntime(scope) => {
                    assert_eq!(scope.session_id, Some(runtime_session_id));
                    assert_eq!(
                        scope.host_id, None,
                        "reconnecting Skill Manager scope must not expose the stale connected host"
                    );
                    assert_eq!(
                        scope.current_working_directory.as_deref(),
                        Some("/root/project")
                    );
                }
                crate::workspace::SkillManagerEnvironmentScope::CurrentApp => {
                    panic!("runtime placeholder Skill Manager must not fall back to current-app")
                }
            }
        });
    });
}

#[test]
fn test_global_search_is_hidden_for_runtime_backed_environment() {
    let _global_search_guard = FeatureFlag::GlobalSearch.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);

        app.read(|ctx| {
            let local_views = Workspace::compute_left_panel_views_for_environment(
                &crate::workspace::environment_runtime::terminal_bootstrap_environment(None),
                ctx,
            );
            let runtime_views = Workspace::compute_left_panel_views_for_environment(
                &EnvironmentSnapshot::runtime_transport(
                    EnvironmentKind::Ssh,
                    "dnyx216".to_string(),
                    "ssh:ssh-config:dnyx216".to_string(),
                    Some("ssh-config:dnyx216".to_string()),
                    Some("/root".to_string()),
                    EnvironmentLifecycleState::Connected,
                ),
                ctx,
            );

            if cfg!(feature = "local_fs") {
                assert!(
                    local_views
                        .iter()
                        .any(|view| matches!(view, ToolPanelView::GlobalSearch { .. })),
                    "current-app local environment should keep the local Global Search entry"
                );
            }
            assert!(
                runtime_views
                    .iter()
                    .all(|view| !matches!(view, ToolPanelView::GlobalSearch { .. })),
                "runtime-backed environments must not expose current-app/local Global Search as if it searched remote files"
            );
        });
    });
}

#[test]
fn test_project_explorer_is_hidden_for_runtime_backed_environment() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        app.read(|ctx| {
            let local_views = Workspace::compute_left_panel_views_for_environment(
                &crate::workspace::environment_runtime::terminal_bootstrap_environment(None),
                ctx,
            );
            let runtime_views = Workspace::compute_left_panel_views_for_environment(
                &EnvironmentSnapshot::runtime_transport(
                    EnvironmentKind::Ssh,
                    "dnyx216".to_string(),
                    "ssh:ssh-config:dnyx216".to_string(),
                    Some("ssh-config:dnyx216".to_string()),
                    Some("/root".to_string()),
                    EnvironmentLifecycleState::Connected,
                ),
                ctx,
            );

            if cfg!(feature = "local_fs") {
                assert!(
                    local_views.contains(&ToolPanelView::ProjectExplorer),
                    "current-app local environment should keep the local Project Explorer entry"
                );
            }
            assert!(
                !runtime_views.contains(&ToolPanelView::ProjectExplorer),
                "runtime-backed environments must not expose current-app/local Project Explorer as if it browsed remote files"
            );
        });
    });
}

#[test]
fn test_environment_runtime_authority_parsing_supports_profile_and_ssh_config() {
    assert_eq!(
        environment_provider::runtime_connection_ref_from_authority("ssh:node-1"),
        Some("node-1".to_string())
    );
    assert_eq!(
        environment_provider::runtime_connection_ref_from_authority("ssh:ssh-config:dev-150"),
        Some("ssh-config:dev-150".to_string())
    );
    assert_eq!(
        environment_provider::runtime_connection_ref_from_authority("ssh-config:dev-150"),
        Some("ssh-config:dev-150".to_string())
    );
    assert_eq!(
        environment_provider::runtime_connection_ref_from_authority("local:/repo"),
        None
    );
}

#[test]
fn test_restored_terminal_bootstrap_startup_command_matches_current_app_restore_cwd_and_pending_resume(
) {
    let session = WorkspaceSessionSnapshot {
        id: "tab:1:leaf:0".to_string(),
        kind: WorkspaceSessionKind::AgentTerminal,
        label: Some("Claude remote".to_string()),
        environment_authority_key: Some("ssh:ssh-config:dev-150".to_string()),
        cwd: Some("/root/repo with spaces".to_string()),
        startup_directory: None,
        cli_agent: Some(CLIAgent::Claude.to_serialized_name()),
        cli_command: Some("claude".to_string()),
        cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
        conversation_ids: Vec::new(),
        active_conversation_id: None,
        cli_agent_session_id: Some("session-123".to_string()),
        is_active: false,
        is_pinned: false,
        updated_at_unix_ms: None,
    };

    let pending_resume = Workspace::cli_agent_from_session(&session)
        .and_then(|agent| {
            agent.explicit_resume_command(
                session.cli_agent_session_id.as_deref(),
                session.cwd.as_deref(),
            )
        })
        .expect("Claude restored session should expose an explicit resume command");

    assert_eq!(pending_resume, "claude --resume session-123");
    assert_eq!(
        Workspace::restored_terminal_bootstrap_startup_command(&session, Some(pending_resume)),
        Some("cd '/root/repo with spaces' && claude --resume session-123".to_string())
    );
}

#[cfg(feature = "local_fs")]
#[test]
fn test_native_session_bridge_fork_opens_cli_agent_resume_tab() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let initial_tab_count = workspace.tab_count();
            let receipt = crate::session_bridge::native_writer::NativeSessionWriteReceipt {
                target: SessionBridgeForkTarget::Agent(CLIAgent::Claude),
                session_id: "claude-session-123".to_string(),
                title: "Forked Claude session".to_string(),
                project_path: "/tmp/ashide project".to_string(),
                session_file: PathBuf::from("/tmp/ashide-project/.claude/session.jsonl"),
                backup_dir: PathBuf::from("/tmp/ashide-project/.backup"),
            };

            workspace.finish_session_bridge_fork(
                Ok(SessionBridgeForkWriteBack::Native(receipt)),
                "Fork 会话失败".to_owned(),
                ctx,
            );

            assert_eq!(
                workspace.tab_count(),
                initial_tab_count + 1,
                "native SessionBridge fork must create a visible CLI-agent resume tab"
            );
            let terminal_view = workspace
                .active_session_view(ctx)
                .expect("native fork should focus the new terminal tab");
            let session = CLIAgentSessionsModel::as_ref(ctx)
                .session(terminal_view.id())
                .expect("new native fork tab should be registered as a CLI-agent session");
            assert_eq!(session.agent, CLIAgent::Claude);
            assert_eq!(
                session.session_context.session_id.as_deref(),
                Some("claude-session-123")
            );
            assert_eq!(
                session.session_context.cwd.as_deref(),
                Some("/tmp/ashide project")
            );
            assert_eq!(session.custom_command_prefix.as_deref(), Some("claude"));
            assert!(workspace
                .session_navigator_sessions(ctx)
                .iter()
                .any(|session| {
                    session.is_active
                        && session.cli_agent.as_deref() == Some("Claude")
                        && session.cli_agent_session_id.as_deref() == Some("claude-session-123")
                }));
        });
    });
}

#[cfg(feature = "local_fs")]
#[test]
fn test_codex_native_session_bridge_fork_opens_cli_agent_resume_tab() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let initial_tab_count = workspace.tab_count();
            let receipt = crate::session_bridge::native_writer::NativeSessionWriteReceipt {
                target: SessionBridgeForkTarget::Agent(CLIAgent::Codex),
                session_id: "019a5b5d-3f67-7c2e-8dc4-4f69f7efc2cb".to_string(),
                title: "Forked Codex session".to_string(),
                project_path: "/tmp/ashide codex project".to_string(),
                session_file: PathBuf::from(
                    "/tmp/ashide-project/.codex/sessions/2026/06/21/rollout-2026-06-21T01-02-03-019a5b5d-3f67-7c2e-8dc4-4f69f7efc2cb.jsonl",
                ),
                backup_dir: PathBuf::from("/tmp/ashide-project/.backup"),
            };

            workspace.finish_session_bridge_fork(
                Ok(SessionBridgeForkWriteBack::Native(receipt)),
                "Fork 会话失败".to_owned(),
                ctx,
            );

            assert_eq!(
                workspace.tab_count(),
                initial_tab_count + 1,
                "Codex SessionBridge fork must create a visible CLI-agent resume tab"
            );
            let terminal_view = workspace
                .active_session_view(ctx)
                .expect("Codex fork should focus the new terminal tab");
            let session = CLIAgentSessionsModel::as_ref(ctx)
                .session(terminal_view.id())
                .expect("new Codex fork tab should be registered as a CLI-agent session");
            assert_eq!(session.agent, CLIAgent::Codex);
            assert_eq!(
                session.session_context.session_id.as_deref(),
                Some("019a5b5d-3f67-7c2e-8dc4-4f69f7efc2cb")
            );
            assert_eq!(
                session.session_context.cwd.as_deref(),
                Some("/tmp/ashide codex project")
            );
            assert_eq!(session.custom_command_prefix.as_deref(), Some("codex"));
            assert!(workspace
                .session_navigator_sessions(ctx)
                .iter()
                .any(|session| {
                    session.is_active
                        && session.cli_agent.as_deref() == Some("Codex")
                        && session.cli_agent_session_id.as_deref()
                            == Some("019a5b5d-3f67-7c2e-8dc4-4f69f7efc2cb")
                }));
        });
    });
}

#[cfg(feature = "local_fs")]
#[test]
fn test_remote_native_session_bridge_fork_keeps_remote_resume_row_visible() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment =
                crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                    "dnyx216".to_string(),
                    &server,
                    Some("/root/project".to_string()),
                    EnvironmentLifecycleState::Connected,
                );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9018);
            workspace.environment_runtimes.mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-remote-native-fork.sock"),
            );
            workspace
                .environment_runtimes
                .mark_connected_session(session_id, HostId::new("remote-native-fork-host".to_string()));
            workspace.set_active_tab_environment(environment);

            let initial_tab_count = workspace.tab_count();
            let receipt = crate::session_bridge::native_writer::NativeSessionRemoteWriteReceipt {
                target: SessionBridgeForkTarget::Agent(CLIAgent::Claude),
                session_id: "remote-claude-fork-session".to_string(),
                title: "Remote Claude fork".to_string(),
                project_path: "/root/project".to_string(),
                session_file:
                    "/root/.claude/projects/-root-project/remote-claude-fork-session.jsonl"
                        .to_string(),
            };

            workspace.finish_session_bridge_fork(
                Ok(SessionBridgeForkWriteBack::RemoteNative {
                    authority: authority.clone(),
                    receipt,
                }),
                "Fork 远程 CLI 会话失败".to_owned(),
                ctx,
            );

            let sessions = workspace.session_navigator_sessions(ctx);
            let forked_session = sessions
                .iter()
                .find(|session| {
                    session.environment_authority_key.as_deref() == Some(authority.as_str())
                        && session.cli_agent.as_deref() == Some("Claude")
                        && session.cli_agent_session_id.as_deref()
                            == Some("remote-claude-fork-session")
                })
                .unwrap_or_else(|| {
                    panic!(
                        "remote native fork must leave a visible Session Navigator row in the owning environment; sessions={sessions:#?}"
                    )
                });

            assert!(
                forked_session.is_active,
                "remote fork row must be active immediately after a successful fork"
            );
            assert_eq!(
                forked_session.label.as_deref(),
                Some("Remote Claude fork")
            );
            assert_eq!(
                workspace.active_restored_workspace_session_key.as_deref(),
                Some(Workspace::workspace_session_logical_key(forked_session).as_str())
            );
            assert_eq!(
                workspace
                    .pending_environment_runtime_session_restores
                    .get(&authority)
                    .and_then(|pending| pending.startup_command.as_deref()),
                Some("claude --resume remote-claude-fork-session"),
                "remote native fork must queue provider resume on the remote runtime authority"
            );
            assert_eq!(
                workspace.tab_count(),
                initial_tab_count,
                "remote native fork must not create a current-app terminal-bootstrap tab"
            );
            let persisted_snapshot = workspace.snapshot(ctx.window_id(), false, ctx);
            assert!(
                persisted_snapshot.workspace_sessions.iter().any(|session| {
                    session.id.starts_with("remote:")
                        && session.environment_authority_key.as_deref() == Some(authority.as_str())
                        && session.cli_agent_session_id.as_deref()
                            == Some("remote-claude-fork-session")
                }),
                "remote native fork backing source must be persisted with its remote authority so restart does not downgrade it to a local tab row; persisted_sessions={:#?}",
                persisted_snapshot.workspace_sessions
            );

            workspace.set_active_tab_environment(
                crate::workspace::environment_runtime::terminal_bootstrap_environment(None),
            );
            let local_sessions = workspace.session_navigator_sessions(ctx);
            assert!(
                local_sessions.iter().all(|session| {
                    session.cli_agent_session_id.as_deref() != Some("remote-claude-fork-session")
                        && session.environment_authority_key.as_deref() != Some(authority.as_str())
                }),
                "remote native fork row must not leak into the local/current-app Session Navigator; local_sessions={local_sessions:#?}"
            );
        });
    });
}

#[cfg(feature = "local_fs")]
#[test]
fn test_ashide_session_bridge_fork_opens_focused_conversation_tab() {
    use diesel::connection::SimpleConnection;
    use diesel::Connection;
    use diesel_migrations::MigrationHarness;

    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let target_conversation_id = AIConversationId::new();
        let tempdir = tempfile::tempdir().unwrap();
        let database_path = tempdir.path().join("ashide.sqlite");
        let mut conn =
            diesel::SqliteConnection::establish(database_path.to_str().unwrap()).unwrap();
        conn.batch_execute("PRAGMA foreign_keys = ON;").unwrap();
        conn.run_pending_migrations(::persistence::MIGRATIONS)
            .unwrap();

        let mut source_session =
            crate::session_bridge::ir::SessionIr::new_ashide(AIConversationId::new().to_string());
        source_session.title = "Fork source Ashide conversation".to_string();
        source_session.project_path = Some("/Users/admin/ashide".to_string());
        source_session.messages = vec![
            crate::session_bridge::ir::SessionMessageIr {
                role: "user".to_string(),
                text: "fork this current Ashide session".to_string(),
                timestamp: Some(crate::session_bridge::ir::SessionTimestamp::String(
                    "2026-06-21T00:00:00Z".to_string(),
                )),
            },
            crate::session_bridge::ir::SessionMessageIr {
                role: "assistant".to_string(),
                text: "forked response".to_string(),
                timestamp: Some(crate::session_bridge::ir::SessionTimestamp::String(
                    "2026-06-21T00:00:01Z".to_string(),
                )),
            },
        ];
        let derivation = crate::session_bridge::transform::fork_session(
            &source_session,
            Some(target_conversation_id.to_string()),
        );
        let import_source =
            crate::session_bridge::ashide_store::SessionBridgeImportSource::from_derived_session(
                &derivation.receipt.operation,
                &derivation.receipt.source_session_id,
                &derivation.receipt.derived_session_id,
                &derivation.session,
            )
            .unwrap();
        let write_back =
            crate::session_bridge::ashide_store::import_ashide_session_write_back_with_payload(
                &mut conn,
                &derivation.session,
                import_source,
            )
            .unwrap();

        let workspace = mock_workspace(&mut app);

        let (initial_tab_count, initial_active_tab) = workspace.update(&mut app, |workspace, ctx| {
            let initial_tab_count = workspace.tab_count();
            let initial_active_tab = workspace.active_tab_index();

            workspace.finish_session_bridge_fork(
                Ok(SessionBridgeForkWriteBack::Ashide(write_back)),
                "Fork 会话失败".to_owned(),
                ctx,
            );
            (initial_tab_count, initial_active_tab)
        });

        futures_lite::future::yield_now().await;

        workspace.update(&mut app, |workspace, ctx| {
            assert_eq!(
                workspace.tab_count(),
                initial_tab_count + 1,
                "Ashide SessionBridge fork must create a visible conversation tab"
            );
            assert_ne!(
                workspace.active_tab_index(),
                initial_active_tab,
                "Ashide fork should focus the newly opened conversation tab"
            );
            assert_eq!(workspace.active_tab_index(), initial_tab_count);
            assert!(
                BlocklistAIHistoryModel::as_ref(ctx)
                    .conversation(&target_conversation_id)
                    .is_some(),
                "Ashide fork write-back must refresh the in-memory conversation history before navigation"
            );
        });
    });
}

#[cfg(feature = "local_fs")]
#[test]
fn test_native_session_bridge_fork_failure_does_not_open_fake_success_tab() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let initial_tab_count = workspace.tab_count();
            let initial_active_tab = workspace.active_tab_index();
            let receipt = crate::session_bridge::native_writer::NativeSessionWriteReceipt {
                target: SessionBridgeForkTarget::Ashide,
                session_id: "not-a-native-target".to_string(),
                title: "Impossible native receipt".to_string(),
                project_path: "/tmp/ashide project".to_string(),
                session_file: PathBuf::from("/tmp/ashide-project/session.jsonl"),
                backup_dir: PathBuf::from("/tmp/ashide-project/.backup"),
            };

            workspace.finish_session_bridge_fork(
                Ok(SessionBridgeForkWriteBack::Native(receipt)),
                "Fork 会话失败".to_owned(),
                ctx,
            );

            assert_eq!(
                workspace.tab_count(),
                initial_tab_count,
                "invalid native receipts must not create a fake visible tab"
            );
            assert_eq!(workspace.active_tab_index(), initial_active_tab);
            assert!(workspace
                .session_navigator_sessions(ctx)
                .iter()
                .all(|session| session.cli_agent_session_id.as_deref()
                    != Some("not-a-native-target")));
        });
    });
}

#[test]
fn test_restored_environment_runtime_startup_command_does_not_duplicate_cd() {
    assert_eq!(
        Workspace::restored_environment_runtime_startup_command(Some(
            "claude --resume session-123".to_string()
        )),
        Some("claude --resume session-123".to_string())
    );
    assert_eq!(
        Workspace::restored_environment_runtime_startup_command(None),
        None
    );
}

#[test]
fn test_restored_current_app_agent_resume_stays_explicit_pending_command() {
    let session = WorkspaceSessionSnapshot {
        id: "tab:0:leaf:0".to_string(),
        kind: WorkspaceSessionKind::AgentTerminal,
        label: Some("Codex local".to_string()),
        environment_authority_key: Some("local:/repo".to_string()),
        cwd: Some("/repo".to_string()),
        startup_directory: None,
        cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
        cli_command: Some("codex".to_string()),
        cli_agent_origin: Some(CliAgentSessionOrigin::CommandDetected),
        conversation_ids: Vec::new(),
        active_conversation_id: None,
        cli_agent_session_id: None,
        is_active: false,
        is_pinned: false,
        updated_at_unix_ms: None,
    };

    let pending_resume = Workspace::cli_agent_from_session(&session).and_then(|agent| {
        agent.explicit_resume_command(
            session.cli_agent_session_id.as_deref(),
            session.cwd.as_deref(),
        )
    });

    assert_eq!(pending_resume, None);
    assert_eq!(
        session.environment_authority_key.as_deref(),
        Some("local:/repo")
    );
    assert_eq!(session.cwd.as_deref(), Some("/repo"));
}

fn test_ssh_server_for_environment_tests() -> warp_ssh_manager::SshServerInfo {
    let mut server = warp_ssh_manager::SshServerInfo::new_default("ssh-config:dnyx216".to_string());
    server.host = "dnyx216".to_string();
    server.username = "root".to_string();
    server
}

fn test_environment_runtime_pty_options(
    session_id: CoreSessionId,
    ctx: &AppContext,
) -> NewTerminalOptions {
    let (client, _event_rx) = crate::workspace::environment_runtime::EnvironmentRuntimeClient::new(
        futures::io::empty(),
        futures::io::sink(),
        ctx.background_executor(),
    );

    NewTerminalOptions::default().with_environment_runtime_pty(EnvironmentRuntimePtyProcess {
        client: Arc::new(client),
        session_id,
        working_directory: "/root/project".to_string(),
        shell: "bash".to_string(),
        startup_command: None,
        environment_variables: HashMap::new(),
    })
}

fn test_environment_runtime_session_snapshot(
    id: impl Into<String>,
    authority: impl Into<String>,
) -> WorkspaceSessionSnapshot {
    WorkspaceSessionSnapshot {
        id: id.into(),
        kind: WorkspaceSessionKind::AgentTerminal,
        label: Some("Environment Codex".to_string()),
        environment_authority_key: Some(authority.into()),
        cwd: Some("/root/project".to_string()),
        startup_directory: None,
        cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
        cli_command: Some("codex".to_string()),
        cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
        conversation_ids: Vec::new(),
        active_conversation_id: None,
        cli_agent_session_id: Some("codex-session".to_string()),
        is_active: false,
        is_pinned: false,
        updated_at_unix_ms: None,
    }
}

fn test_session_navigator_order_session(
    id: impl Into<String>,
    label: impl Into<String>,
    updated_at_unix_ms: i64,
) -> WorkspaceSessionSnapshot {
    let id = id.into();
    WorkspaceSessionSnapshot {
        id: id.clone(),
        kind: WorkspaceSessionKind::AgentTerminal,
        label: Some(label.into()),
        environment_authority_key: Some("local".to_string()),
        cwd: Some("/Users/admin/ashide".to_string()),
        startup_directory: None,
        cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
        cli_command: Some("codex".to_string()),
        cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
        conversation_ids: Vec::new(),
        active_conversation_id: None,
        cli_agent_session_id: Some(format!("{id}-provider-session")),
        is_active: false,
        is_pinned: false,
        updated_at_unix_ms: Some(updated_at_unix_ms),
    }
}

fn test_session_navigator_displayed_order(
    workspace: &Workspace,
    ctx: &AppContext,
) -> Vec<&'static str> {
    workspace
        .session_navigator_sessions(ctx)
        .iter()
        .filter_map(|session| match session.id.as_str() {
            "order-key-resume-a" => Some("order-key-resume-a"),
            "order-key-resume-b" => Some("order-key-resume-b"),
            "order-key-append-a" => Some("order-key-append-a"),
            "order-key-append-b" => Some("order-key-append-b"),
            "order-key-append-c" => Some("order-key-append-c"),
            "order-key-prune-a" => Some("order-key-prune-a"),
            "order-key-prune-b" => Some("order-key-prune-b"),
            "order-key-pin-a" => Some("order-key-pin-a"),
            "order-key-pin-b" => Some("order-key-pin-b"),
            "order-key-pin-c" => Some("order-key-pin-c"),
            _ => None,
        })
        .collect()
}

fn test_pending_environment_runtime_session_restore(
    authority: &str,
) -> PendingEnvironmentRuntimeSessionRestore {
    PendingEnvironmentRuntimeSessionRestore {
        session: test_environment_runtime_session_snapshot(
            "environment-runtime-pending-restore",
            authority,
        ),
        startup_command: Some("codex resume codex-session".to_string()),
    }
}

fn test_pending_environment_runtime_agent_view_entry() -> AgentTabEntry {
    AgentTabEntry {
        initial_prompt: Some("inspect remote project".to_string()),
        origin: AgentViewEntryOrigin::DefaultSessionMode,
        codex_model_id: Some("codex-test-model".to_string()),
        open_code_review_pane: false,
        fallback_display_title: None,
        zero_state_prompt_suggestion_type: None,
        restore_left_panel_open: false,
    }
}

fn test_pending_environment_runtime_forked_conversation_entry(
) -> ForkEntry {
    use crate::ai::agent::conversation::AIConversation;
    ForkEntry {
        conversation: AIConversation::new(false),
        source_terminal_view_id: None,
        summarize_after_fork: false,
        summarization_prompt: None,
        initial_prompt: Some("continue remotely".to_string()),
    }
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn test_open_environment_runtime_syncs_session_navigator_environment_cache() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                None,
                EnvironmentLifecycleState::Connected,
            );
            let remote_authority = environment.authority_key.clone();
            workspace
                .restored_workspace_sessions
                .push(WorkspaceSessionSnapshot {
                    id: "ssh-manager-session".to_string(),
                    kind: WorkspaceSessionKind::AgentTerminal,
                    label: Some("SSH Manager Codex".to_string()),
                    environment_authority_key: Some(remote_authority),
                    cwd: None,
                    startup_directory: None,
                    cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
                    cli_command: Some("codex".to_string()),
                    cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                    conversation_ids: Vec::new(),
                    active_conversation_id: None,
                    cli_agent_session_id: Some("ssh-manager-1".to_string()),
                    is_active: false,
                    is_pinned: false,
                    updated_at_unix_ms: None,
                });

            workspace.open_environment_runtime_from_provider(
                environment_provider::source_saved_ssh::target_from_server(
                    server.node_id.clone(),
                    server,
                ),
                ctx,
            );

            let cached_ids = workspace
                .session_navigator_sessions(ctx)
                .into_iter()
                .map(|session| session.id)
                .collect::<Vec<_>>();
            assert!(cached_ids.iter().any(|id| id == "ssh-manager-session"));
        });
    });
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn test_open_environment_runtime_queues_startup_without_direct_process() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let mut server = test_ssh_server_for_environment_tests();
            server.startup_command = Some("cd /srv && codex".to_string());
            let target = environment_provider::source_saved_ssh::target_from_server(
                server.node_id.clone(),
                server,
            );
            let authority = target.dormant_environment(None).authority_key;

            workspace.open_environment_runtime_from_provider(target, ctx);

            assert_eq!(
                workspace
                    .pending_environment_runtime_startup_commands
                    .get(&authority)
                    .map(String::as_str),
                Some("cd /srv && codex")
            );
            let active_tab = &workspace.tabs[workspace.active_tab_index()];
            assert_eq!(
                active_tab
                    .environment
                    .as_ref()
                    .map(|environment| environment.authority_key.as_str()),
                Some(authority.as_str())
            );
        });
    });
}

#[test]
fn test_add_terminal_tab_from_ssh_tab_inherits_ssh_environment() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();

            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let session_id = CoreSessionId::from(9001);
            workspace.environment_runtimes.mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-ssh-control.sock"),
            );
            workspace
                .environment_runtimes
                .mark_connected_session(session_id, HostId::new("test-host".to_string()));
            workspace.set_active_tab_environment(environment);
            let ssh_tab_index = workspace.active_tab_index();

            workspace.handle_action(
                &WorkspaceAction::AddTerminalTab {
                    hide_homepage: false,
                },
                ctx,
            );

            assert_eq!(workspace.tab_count(), 2);
            assert_ne!(workspace.active_tab_index(), ssh_tab_index);
            assert_eq!(
                workspace.tabs[ssh_tab_index]
                    .environment
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Ssh)
            );
            assert_eq!(
                workspace.tabs[workspace.active_tab_index()]
                    .environment
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Ssh)
            );
            assert_eq!(
                workspace.tabs[workspace.active_tab_index()]
                    .environment
                    .as_ref()
                    .and_then(|environment| environment.active_workspace_root.as_deref()),
                Some("/root/project")
            );
            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Ssh)
            );
        });
    });
}

#[test]
fn test_switching_away_from_runtime_environment_tab_retains_connected_runtime() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9033);
            workspace.mark_environment_runtime_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-tab-switch-retain.sock"),
            );
            workspace
                .environment_runtimes
                .mark_connected_session(session_id, HostId::new("retain-runtime-host".to_string()));
            workspace.set_active_tab_environment(environment);
            let runtime_tab_index = workspace.active_tab_index();

            workspace.add_environment_terminal_tab(
                crate::workspace::environment_runtime::terminal_bootstrap_environment(None),
                true,
                ctx,
            );

            assert_ne!(
                workspace.active_tab_index(),
                runtime_tab_index,
                "test setup must switch focus away from the runtime Environment tab"
            );
            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Local),
                "switching focus should update the view Environment without tearing down the runtime"
            );
            assert_eq!(
                workspace.environment_runtime_session_for_authority(&authority),
                Some(session_id),
                "Workspace-owned runtime session must survive an ordinary tab switch"
            );
            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                Some(EnvironmentLifecycleState::Connected)
            );
            assert!(
                workspace.is_environment_authority_retained(&authority),
                "opened runtime authorities stay retained by the Workspace until explicit disconnect"
            );
        });
    });
}

#[test]
fn test_disconnect_environment_releases_retained_runtime_authority() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9034);
            workspace.mark_environment_runtime_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-disconnect-release.sock"),
            );
            workspace
                .environment_runtimes
                .mark_connected_session(session_id, HostId::new("release-runtime-host".to_string()));
            workspace.set_active_tab_environment(environment);

            assert!(workspace.is_environment_authority_retained(&authority));
            workspace.disconnect_environment_runtime_state(&authority, true, ctx);

            assert!(
                !workspace.is_environment_authority_retained(&authority),
                "explicit disconnect must release the Workspace lifecycle hold"
            );
            assert_eq!(workspace.environment_runtime_session_for_authority(&authority), None);
            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                None
            );
        });
    });
}

#[test]
fn test_retained_runtime_environment_reconnects_after_transport_disconnect() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let stale_session_id = CoreSessionId::from(9035);
            workspace.mark_environment_runtime_connecting(
                environment.clone(),
                stale_session_id,
                PathBuf::from("/tmp/ashide-test-retained-disconnect.sock"),
            );
            workspace
                .environment_runtimes
                .mark_connected_session(stale_session_id, HostId::new("disconnect-runtime-host".to_string()));
            workspace.set_active_tab_environment(environment);

            workspace.handle_environment_runtime_disconnected(stale_session_id, ctx);

            let replacement_session_id = workspace
                .environment_runtime_session_for_authority(&authority)
                .expect("retained Environment should be re-registered after transport disconnect");
            assert_ne!(
                replacement_session_id, stale_session_id,
                "transport disconnect must not leave a retained Environment bound to the dead session"
            );
            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                Some(EnvironmentLifecycleState::Connecting),
                "retained Environment should move back to Connecting while reconnecting"
            );
            assert!(
                workspace.is_environment_authority_retained(&authority),
                "transport disconnect should not release Workspace lifecycle ownership"
            );
        });
    });
}

#[test]
fn test_add_terminal_tab_method_routes_ssh_environment_through_runtime_facade() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9011);
            workspace.environment_runtimes.mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-ssh-control-direct-add.sock"),
            );
            workspace
                .environment_runtimes
                .mark_connected_session(session_id, HostId::new("test-host".to_string()));
            workspace.set_active_tab_environment(environment);

            workspace.add_terminal_tab(false, ctx);

            assert_eq!(workspace.tab_count(), 2);
            assert!(
                workspace
                    .pending_environment_runtime_terminal_authorities
                    .contains(&authority),
                "without a test remote-server client the runtime intent should stay pending instead of falling back to a current-app terminal"
            );
            let active_tab = &workspace.tabs[workspace.active_tab_index()];
            assert_eq!(
                active_tab
                    .environment
                    .as_ref()
                    .map(|environment| environment.authority_key.as_str()),
                Some(authority.as_str())
            );
            assert_eq!(
                active_tab
                    .environment
                    .as_ref()
                    .and_then(|environment| environment.active_workspace_root.as_deref()),
                Some("/root/project")
            );
        });
    });
}

#[test]
fn test_add_terminal_tab_from_environment_runtime_syncs_active_session_row() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9013);
            workspace.environment_runtimes.mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-ssh-control-session-row.sock"),
            );
            workspace
                .environment_runtimes
                .mark_connected_session(session_id, HostId::new("test-host".to_string()));
            workspace.set_active_tab_environment(environment);

            workspace.handle_action(
                &WorkspaceAction::AddTerminalTab {
                    hide_homepage: false,
                },
                ctx,
            );

            let live_sessions = workspace.live_workspace_sessions(ctx);
            let sessions = workspace.session_navigator_sessions(ctx);
            let current_environment = workspace.current_environment.clone();
            let tab_environments = workspace
                .tabs
                .iter()
                .map(|tab| {
                    tab.environment
                        .as_ref()
                        .map(|environment| environment.authority_key.clone())
                })
                .collect::<Vec<_>>();
            let tab_roots = workspace
                .tabs
                .iter()
                .map(|tab| tab.pane_group.as_ref(ctx).snapshot(ctx))
                .collect::<Vec<_>>();
            let active_remote_sessions = sessions
                .iter()
                .filter(|session| {
                    session.is_active
                        && session.environment_authority_key.as_deref() == Some(authority.as_str())
                        && session.id.starts_with("tab:")
                        && matches!(
                            session.kind,
                            WorkspaceSessionKind::Terminal | WorkspaceSessionKind::AgentTerminal
                        )
                })
                .collect::<Vec<_>>();
            assert_eq!(
                active_remote_sessions.len(),
                1,
                "new terminal in a connected Environment must produce one active remote session row; current_environment={current_environment:#?}; tab_environments={tab_environments:#?}; tab_roots={tab_roots:#?}; live_sessions={live_sessions:#?}; sessions={sessions:#?}"
            );

            let left_panel_sessions = workspace.session_navigator_sessions(ctx);
            assert!(
                left_panel_sessions.iter().any(|session| {
                    session.is_active
                        && session.environment_authority_key.as_deref() == Some(authority.as_str())
                        && session.id == active_remote_sessions[0].id
                }),
                "left panel must be synced with the active remote live session row; left_panel_sessions={left_panel_sessions:#?}"
            );
        });
    });
}

#[test]
fn test_workspace_sessions_refresh_state_reports_progress_success_and_failure() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            assert!(!workspace.is_workspace_sessions_refreshing());
            assert_eq!(
                workspace.workspace_sessions_refresh_tooltip(),
                "刷新会话列表"
            );

            let first_generation = workspace.begin_workspace_sessions_refresh(ctx);
            assert!(workspace.is_workspace_sessions_refreshing());
            assert_eq!(
                workspace.workspace_sessions_refresh_tooltip(),
                "正在刷新会话列表…"
            );

            workspace.finish_workspace_sessions_refresh_if_current(
                first_generation,
                "已刷新会话列表：41 个会话".to_owned(),
                ctx,
            );
            assert!(!workspace.is_workspace_sessions_refreshing());
            assert_eq!(
                workspace.workspace_sessions_refresh_tooltip(),
                "已刷新会话列表：41 个会话"
            );

            let second_generation = workspace.begin_workspace_sessions_refresh(ctx);
            workspace.fail_workspace_sessions_refresh_if_current(
                second_generation,
                "刷新会话列表失败：runtime unavailable".to_owned(),
                ctx,
            );
            assert!(!workspace.is_workspace_sessions_refreshing());
            assert_eq!(
                workspace.workspace_sessions_refresh_tooltip(),
                "刷新会话列表失败：runtime unavailable"
            );

            workspace.finish_workspace_sessions_refresh_if_current(
                second_generation.saturating_sub(1),
                "stale success must be ignored".to_owned(),
                ctx,
            );
            assert_eq!(
                workspace.workspace_sessions_refresh_tooltip(),
                "刷新会话列表失败：runtime unavailable"
            );
        });
    });
}

#[test]
fn test_environment_runtime_stale_binary_callbacks_are_ignored() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, _| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connecting,
            );
            let stale_session_id = CoreSessionId::from(9074);
            let replacement_session_id = CoreSessionId::from(9075);

            workspace.mark_environment_runtime_connecting(
                environment.clone(),
                stale_session_id,
                PathBuf::from("/tmp/ashide-test-stale-binary-callback.sock"),
            );
            assert!(
                !workspace.ignore_stale_environment_runtime_result(
                    stale_session_id,
                    "binary check"
                ),
                "freshly registered binary callback must be accepted"
            );

            workspace.mark_environment_runtime_connecting(
                environment,
                replacement_session_id,
                PathBuf::from("/tmp/ashide-test-replacement-binary-callback.sock"),
            );

            assert!(
                workspace.ignore_stale_environment_runtime_result(
                    stale_session_id,
                    "binary install"
                ),
                "older binary check/install callbacks must not poison the replacement runtime"
            );
            assert!(
                !workspace.ignore_stale_environment_runtime_result(
                    replacement_session_id,
                    "binary install"
                ),
                "replacement runtime callback must still be accepted"
            );
        });
    });
}

#[test]
fn test_environment_runtime_success_dismisses_stale_failure_toasts() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            workspace.show_environment_runtime_failure_toast(
                "ssh:ssh-config:dnyx216",
                "准备远程运行时失败".to_owned(),
                ctx,
            );
            workspace.show_environment_runtime_failure_toast(
                "ssh:ssh-config:o1",
                "准备远程运行时失败".to_owned(),
                ctx,
            );
            assert!(
                workspace
                    .toast_stack
                    .read(ctx, |toast_stack, _| toast_stack.has_toasts()),
                "test setup should have visible failure toasts before reconnect success"
            );

            workspace.dismiss_environment_runtime_failure_toasts(ctx);

            assert!(
                !workspace
                    .toast_stack
                    .read(ctx, |toast_stack, _| toast_stack.has_toasts()),
                "successful runtime connection should clear stale failure toasts for all authorities"
            );
        });
    });
}

#[test]
fn test_stale_connected_environment_without_runtime_client_reconnects() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let stale_session_id = CoreSessionId::from(9014);
            workspace.environment_runtimes.mark_connecting(
                environment.clone(),
                stale_session_id,
                PathBuf::from("/tmp/ashide-test-stale-connected.sock"),
            );
            workspace
                .environment_runtimes
                .mark_connected_session(stale_session_id, HostId::new("stale-host".to_string()));
            workspace.set_active_tab_environment(environment);

            assert!(
                crate::workspace::environment_runtime::client_for_session(stale_session_id, ctx)
                    .is_none(),
                "test setup must represent a persisted Connected Environment whose runtime client/proxy is gone"
            );

            workspace.ensure_current_environment_runtime_transport_if_needed(ctx);

            let replacement_session_id = workspace
                .environment_runtime_session_for_authority(&authority)
                .expect("stale connected Environment should be re-registered for reconnect");
            assert_ne!(
                replacement_session_id, stale_session_id,
                "Connected without a runtime client must not be treated as active; ensure should start a fresh runtime bootstrap"
            );
            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                Some(EnvironmentLifecycleState::Connecting),
                "stale Connected Environment should move back to Connecting while the runtime proxy is restarted"
            );
            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| &environment.lifecycle_state),
                Some(&EnvironmentLifecycleState::Connecting),
                "current Environment strip state should show reconnecting/preparing instead of stale Connected"
            );
        });
    });
}

#[test]
fn test_environment_file_browser_unavailable_event_reconnects_stale_runtime() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let stale_session_id = CoreSessionId::from(9024);
            let stale_host_id = HostId::new("stale-browser-host".to_string());
            workspace.environment_runtimes.mark_connecting(
                environment.clone(),
                stale_session_id,
                PathBuf::from("/tmp/ashide-test-browser-unavailable.sock"),
            );
            workspace
                .environment_runtimes
                .mark_connected_session(stale_session_id, stale_host_id.clone());
            workspace.set_active_tab_environment(environment);

            workspace.handle_left_panel_event(
                &LeftPanelEvent::ServerFileBrowser(
                    crate::workspace::view::server_file_browser::ServerFileBrowserEvent::EnvironmentRuntimeUnavailable {
                        session_id: Some(stale_session_id),
                        host_id: Some(stale_host_id),
                    },
                ),
                ctx,
            );

            let replacement_session_id = workspace
                .environment_runtime_session_for_authority(&authority)
                .expect("browser unavailable event should trigger runtime reconnect");
            assert_ne!(
                replacement_session_id, stale_session_id,
                "file browser must not leave a stale Connected runtime target in place after it discovers the client is gone"
            );
            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                Some(EnvironmentLifecycleState::Connecting)
            );
        });
    });
}

#[test]
fn test_environment_file_browser_unavailable_event_ignores_error_runtime() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connecting,
            );
            let authority = environment.authority_key.clone();
            let failed_session_id = CoreSessionId::from(9026);
            let failed_host_id = HostId::new("failed-browser-host".to_string());
            workspace.mark_environment_runtime_connecting(
                environment.clone(),
                failed_session_id,
                PathBuf::from("/tmp/ashide-test-browser-unavailable-error.sock"),
            );
            workspace.set_active_tab_environment(environment);
            workspace.handle_environment_runtime_failed(
                failed_session_id,
                "synthetic transport failure".to_string(),
                ctx,
            );

            workspace.handle_left_panel_event(
                &LeftPanelEvent::ServerFileBrowser(
                    crate::workspace::view::server_file_browser::ServerFileBrowserEvent::EnvironmentRuntimeUnavailable {
                        session_id: Some(failed_session_id),
                        host_id: Some(failed_host_id),
                    },
                ),
                ctx,
            );

            assert_eq!(
                workspace.environment_runtime_session_for_authority(&authority),
                Some(failed_session_id),
                "file browser unavailable events must not auto-retry a runtime that is already in Error"
            );
            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                Some(EnvironmentLifecycleState::Error),
                "Error is terminal for implicit file-browser refresh; explicit reconnect owns retry"
            );
        });
    });
}

#[test]
fn test_environment_left_panel_sync_reconnects_stale_connected_runtime() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let stale_session_id = CoreSessionId::from(9025);
            workspace.environment_runtimes.mark_connecting(
                environment.clone(),
                stale_session_id,
                PathBuf::from("/tmp/ashide-test-left-panel-sync-stale.sock"),
            );
            workspace
                .environment_runtimes
                .mark_connected_session(stale_session_id, HostId::new("stale-left-panel-host".to_string()));
            workspace.set_active_tab_environment(environment);

            workspace.sync_environment_runtime_left_panel_roots(ctx);

            let replacement_session_id = workspace
                .environment_runtime_session_for_authority(&authority)
                .expect("left panel sync should trigger runtime reconnect");
            assert_ne!(
                replacement_session_id, stale_session_id,
                "left panel root sync must not bind file browser roots to a Connected runtime target whose client is gone"
            );
            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                Some(EnvironmentLifecycleState::Connecting)
            );
        });
    });
}

#[test]
fn test_restored_preparing_environment_without_runtime_session_reconnects() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Installing,
            );
            let authority = environment.authority_key.clone();
            workspace.remember_environment_runtime_snapshot(environment.clone());
            workspace.set_active_tab_environment(environment);

            assert!(
                workspace
                    .environment_runtime_session_for_authority(&authority)
                    .is_none(),
                "test setup must represent a restored preparing Environment whose async runtime task is gone"
            );

            workspace.ensure_current_environment_runtime_transport_if_needed(ctx);

            assert!(
                workspace
                    .environment_runtime_session_for_authority(&authority)
                    .is_some(),
                "restored preparing Environment must start a fresh runtime bootstrap instead of staying stuck"
            );
            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                Some(EnvironmentLifecycleState::Connecting)
            );
            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| &environment.lifecycle_state),
                Some(&EnvironmentLifecycleState::Connecting)
            );
        });
    });
}

#[test]
fn test_restored_environment_runtime_tab_normalizes_stale_preparing_state() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Installing,
            );
            let authority = environment.authority_key.clone();

            workspace.add_restored_environment_runtime_tab(
                environment,
                Some("root@dnyx216".to_string()),
                ctx,
            );

            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                Some(EnvironmentLifecycleState::Dormant),
                "persisted preparing state has no live runtime task after restore and must not remain active"
            );
            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| &environment.lifecycle_state),
                Some(&EnvironmentLifecycleState::Dormant),
                "Environment Strip should not show stale 'preparing remote runtime' immediately after restore"
            );
            assert_eq!(
                workspace.tabs[workspace.active_tab_index()]
                    .environment
                    .as_ref()
                    .map(|environment| &environment.lifecycle_state),
                Some(&EnvironmentLifecycleState::Dormant),
                "restored Environment tab should render dormant until a fresh ensure starts"
            );
        });
    });
}

#[test]
fn test_environment_runtime_connecting_lifecycle_syncs_non_active_restored_tab() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Dormant,
            );
            let authority = environment.authority_key.clone();

            workspace.add_restored_environment_runtime_tab(
                environment.clone(),
                Some("root@dnyx216".to_string()),
                ctx,
            );
            let environment_tab_index = workspace
                .tab_index_for_environment_authority(&authority)
                .expect("test setup should create a restored Environment tab");
            workspace.activate_tab_internal(0, ctx);

            workspace.mark_environment_runtime_connecting(
                environment,
                CoreSessionId::from(9023),
                PathBuf::from("/tmp/ashide-test-connect-sync.sock"),
            );

            assert_eq!(
                workspace.tabs[environment_tab_index]
                    .environment
                    .as_ref()
                    .map(|environment| &environment.lifecycle_state),
                Some(&EnvironmentLifecycleState::Connecting),
                "starting a fresh runtime bootstrap must update restored tabs that are not active"
            );
            assert_ne!(
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| environment.authority_key.as_str()),
                Some(authority.as_str()),
                "test setup keeps the Environment tab in the background"
            );
        });
    });
}

#[test]
fn test_connecting_environment_runtime_blocks_duplicate_transport_ensure() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let target =
                crate::workspace::environment_provider::source_saved_ssh::target_from_server(
                    server.node_id.clone(),
                    server,
                );
            let environment =
                target.dormant_environment(Some("/root/project".to_string()));
            let authority = environment.authority_key.clone();
            let original_session_id = CoreSessionId::from(9084);
            workspace.mark_environment_runtime_connecting(
                environment.clone(),
                original_session_id,
                PathBuf::from("/tmp/ashide-test-single-flight.sock"),
            );

            workspace.ensure_environment_runtime_transport(
                environment,
                target.transport_descriptor(),
                ctx,
            );

            assert_eq!(
                workspace.environment_runtime_session_for_authority(&authority),
                Some(original_session_id),
                "a preparing Environment runtime must stay single-flight instead of allocating a second synthetic session"
            );
            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                Some(EnvironmentLifecycleState::Connecting)
            );
        });
    });
}

#[test]
fn test_error_environment_runtime_blocks_implicit_transport_ensure() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Error,
            );
            let authority = environment.authority_key.clone();
            workspace.set_active_tab_environment(environment);

            workspace.ensure_current_environment_runtime_transport_if_needed(ctx);

            assert_eq!(
                workspace.environment_runtime_session_for_authority(&authority),
                None,
                "implicit ensure must not allocate a new runtime session for Error environments"
            );
            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| &environment.lifecycle_state),
                Some(&EnvironmentLifecycleState::Error),
                "implicit ensure must leave the visible Environment error intact"
            );
        });
    });
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn test_environment_runtime_control_path_is_short_enough_for_openssh() {
    let path =
        Workspace::environment_runtime_control_path("ssh:898d71c9-74f9-4a41-ac98-1ece5f485b7b");
    let path_string = path.display().to_string();

    assert!(
        path_string.starts_with("/tmp/ashe/"),
        "ControlMaster socket path must live under the hard-cut short socket directory: {path_string}"
    );
    assert!(
        path_string.len() <= 48,
        "OpenSSH appends a random bind suffix; Ashide's base ControlPath must stay short, got {} chars: {path_string}",
        path_string.len()
    );
}

#[test]
fn test_activate_dormant_environment_runtime_tab_starts_transport() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Dormant,
            );
            let authority = environment.authority_key.clone();

            workspace.add_restored_environment_runtime_tab(
                environment,
                Some("root@dnyx216".to_string()),
                ctx,
            );
            let environment_tab_index = workspace
                .tab_index_for_environment_authority(&authority)
                .expect("test setup should create a restored Environment tab");
            workspace.activate_tab_internal(0, ctx);
            assert!(
                workspace
                    .environment_runtime_session_for_authority(&authority)
                    .is_none(),
                "background dormant Environment should not be bootstrapped until the user activates it"
            );

            workspace.handle_action(&WorkspaceAction::ActivateTab(environment_tab_index), ctx);

            assert!(
                workspace.has_pending_environment_runtime_entry_for_authority(&authority),
                "activating an Environment placeholder tab must enqueue a runtime terminal intent"
            );
            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| environment.authority_key.as_str()),
                Some(authority.as_str())
            );
            assert!(
                workspace
                    .environment_runtime_session_for_authority(&authority)
                    .is_some(),
                "activating a dormant Environment tab must start runtime bootstrap instead of leaving the placeholder stuck"
            );
            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                Some(EnvironmentLifecycleState::Connecting)
            );
            assert_eq!(
                workspace.tabs[environment_tab_index]
                    .environment
                    .as_ref()
                    .map(|environment| &environment.lifecycle_state),
                Some(&EnvironmentLifecycleState::Connecting)
            );
        });
    });
}

fn add_background_dormant_environment_runtime_tab(
    workspace: &mut Workspace,
    ctx: &mut ViewContext<Workspace>,
) -> (String, usize) {
    let server = test_ssh_server_for_environment_tests();
    let environment =
        crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
            server.node_id.clone(),
            &server,
            Some("/root/project".to_string()),
            EnvironmentLifecycleState::Dormant,
        );
    let authority = environment.authority_key.clone();
    workspace.add_restored_environment_runtime_tab(
        environment,
        Some("root@dnyx216".to_string()),
        ctx,
    );
    let environment_tab_index = workspace
        .tab_index_for_environment_authority(&authority)
        .expect("test setup should create a restored Environment tab");
    workspace.activate_tab_internal(0, ctx);
    assert!(
        !workspace.has_pending_environment_runtime_entry_for_authority(&authority),
        "test setup should start with the Environment tab dormant in the background"
    );
    (authority, environment_tab_index)
}

fn assert_active_background_environment_runtime_started(workspace: &Workspace, authority: &str) {
    assert!(
        workspace.has_pending_environment_runtime_entry_for_authority(authority),
        "user-visible tab navigation must enqueue a runtime terminal intent for dormant Environment placeholders"
    );
    assert_eq!(
        workspace
            .current_environment
            .as_ref()
            .map(|environment| environment.authority_key.as_str()),
        Some(authority)
    );
    assert!(
        workspace
            .environment_runtime_session_for_authority(authority)
            .is_some(),
        "user-visible tab navigation must start runtime bootstrap instead of leaving the placeholder stuck"
    );
}

#[test]
fn test_activate_next_tab_environment_runtime_placeholder_queues_terminal_intent() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let (authority, environment_tab_index) =
                add_background_dormant_environment_runtime_tab(workspace, ctx);

            workspace.handle_action(&WorkspaceAction::ActivateNextTab, ctx);

            assert_eq!(workspace.active_tab_index(), environment_tab_index);
            assert_active_background_environment_runtime_started(workspace, &authority);
        });
    });
}

#[test]
fn test_activate_prev_tab_environment_runtime_placeholder_queues_terminal_intent() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let (authority, environment_tab_index) =
                add_background_dormant_environment_runtime_tab(workspace, ctx);

            workspace.handle_action(&WorkspaceAction::ActivatePrevTab, ctx);

            assert_eq!(workspace.active_tab_index(), environment_tab_index);
            assert_active_background_environment_runtime_started(workspace, &authority);
        });
    });
}

#[test]
fn test_activate_last_tab_environment_runtime_placeholder_queues_terminal_intent() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let (authority, environment_tab_index) =
                add_background_dormant_environment_runtime_tab(workspace, ctx);

            workspace.handle_action(&WorkspaceAction::ActivateLastTab, ctx);

            assert_eq!(workspace.active_tab_index(), environment_tab_index);
            assert_active_background_environment_runtime_started(workspace, &authority);
        });
    });
}

#[test]
fn test_focus_pane_environment_runtime_placeholder_queues_terminal_intent() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let (authority, environment_tab_index) =
                add_background_dormant_environment_runtime_tab(workspace, ctx);
            let pane_group = workspace.tabs[environment_tab_index].pane_group.clone();
            let locator = PaneViewLocator {
                pane_group_id: pane_group.id(),
                pane_id: pane_group.as_ref(ctx).focused_pane_id(ctx),
            };

            workspace.focus_pane(locator, ctx);

            assert_eq!(workspace.active_tab_index(), environment_tab_index);
            assert_active_background_environment_runtime_started(workspace, &authority);
        });
    });
}

#[test]
fn test_close_active_tab_activating_environment_runtime_placeholder_queues_terminal_intent() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let (authority, _environment_tab_index) =
                add_background_dormant_environment_runtime_tab(workspace, ctx);

            workspace.handle_action(&WorkspaceAction::CloseTab(0), ctx);

            assert_eq!(workspace.active_tab_index(), 0);
            assert_active_background_environment_runtime_started(workspace, &authority);
        });
    });
}

#[test]
fn test_activate_connected_environment_runtime_placeholder_queues_terminal_intent() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Dormant,
            );
            let authority = environment.authority_key.clone();
            workspace.add_restored_environment_runtime_tab(
                environment.clone(),
                Some("root@dnyx216".to_string()),
                ctx,
            );
            let environment_tab_index = workspace
                .tab_index_for_environment_authority(&authority)
                .expect("test setup should create a restored Environment tab");
            let session_id = CoreSessionId::from(9023);
            let host_id = HostId::new("connected-placeholder-host".to_string());
            workspace.mark_environment_runtime_connecting(
                environment,
                session_id,
                PathBuf::from("/tmp/ashide-test-connected-placeholder.sock"),
            );
            let _ = workspace.mark_environment_runtime_connected_session(session_id, host_id);
            workspace.activate_tab_internal(0, ctx);

            assert!(
                !workspace.has_pending_environment_runtime_entry_for_authority(&authority),
                "test setup should start with a connected placeholder but no explicit pending intent"
            );

            workspace.handle_action(&WorkspaceAction::ActivateTab(environment_tab_index), ctx);

            assert!(
                workspace.has_pending_environment_runtime_entry_for_authority(&authority),
                "activating a connected Environment placeholder tab must materialize or preserve a terminal intent instead of staying as an empty shell"
            );
            assert!(
                workspace.active_tab_contains_environment_runtime_placeholder(ctx),
                "unit harness has no registered runtime client, so the placeholder should remain visible while the preserved pending intent reconnects"
            );
        });
    });
}

#[test]
fn test_environment_runtime_connected_without_client_reconnects_and_preserves_pending() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connecting,
            );
            let authority = environment.authority_key.clone();
            let stale_session_id = CoreSessionId::from(9015);
            workspace.mark_environment_runtime_connecting(
                environment.clone(),
                stale_session_id,
                PathBuf::from("/tmp/ashide-test-connected-without-client.sock"),
            );
            workspace.set_active_tab_environment(environment);
            workspace.queue_pending_environment_runtime_terminal(&authority, ctx);

            assert!(
                crate::workspace::environment_runtime::client_for_session(stale_session_id, ctx)
                    .is_none(),
                "test setup must simulate a transport SessionConnected event whose client is not registered"
            );

            workspace.handle_environment_runtime_connected(
                stale_session_id,
                HostId::new("missing-client-host".to_string()),
                ctx,
            );

            let replacement_session_id = workspace
                .environment_runtime_session_for_authority(&authority)
                .expect("missing-client connected event should trigger a fresh runtime bootstrap");
            assert_ne!(
                replacement_session_id, stale_session_id,
                "SessionConnected without a runtime client must not leave the environment bound to the unusable session"
            );
            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                Some(EnvironmentLifecycleState::Connecting),
                "missing-client connected event should move back to Connecting instead of fake Connected"
            );
            assert!(
                workspace.has_pending_environment_runtime_entry_for_authority(&authority),
                "reconnect after a missing-client connected event must preserve the pending terminal/agent/restore intent"
            );
        });
    });
}

#[test]
fn test_environment_runtime_pending_intents_are_single_slot_hard_cut() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let authority = "ssh:ssh-config:dnyx216".to_string();

            workspace.queue_environment_runtime_intent(
                &authority,
                EnvironmentRuntimeEntryIntent::Terminal,
                ctx,
            );
            assert!(workspace
                .pending_environment_runtime_terminal_authorities
                .contains(&authority));
            assert!(!workspace
                .pending_environment_runtime_startup_commands
                .contains_key(&authority));
            assert!(!workspace
                .pending_environment_runtime_agent_view_entries
                .contains_key(&authority));
            assert!(!workspace
                .pending_environment_runtime_forked_conversation_entries
                .contains_key(&authority));
            assert!(!workspace
                .pending_environment_runtime_session_restores
                .contains_key(&authority));

            workspace.queue_environment_runtime_intent(
                &authority,
                EnvironmentRuntimeEntryIntent::StartupCommand("codex resume hard-cut".to_string()),
                ctx,
            );
            assert_eq!(
                workspace
                    .pending_environment_runtime_startup_commands
                    .get(&authority)
                    .map(String::as_str),
                Some("codex resume hard-cut")
            );
            assert!(!workspace
                .pending_environment_runtime_terminal_authorities
                .contains(&authority));
            assert!(!workspace
                .pending_environment_runtime_agent_view_entries
                .contains_key(&authority));
            assert!(!workspace
                .pending_environment_runtime_forked_conversation_entries
                .contains_key(&authority));
            assert!(!workspace
                .pending_environment_runtime_session_restores
                .contains_key(&authority));

            workspace.queue_environment_runtime_intent(
                &authority,
                EnvironmentRuntimeEntryIntent::AgentView(
                    test_pending_environment_runtime_agent_view_entry(),
                ),
                ctx,
            );
            assert!(workspace
                .pending_environment_runtime_agent_view_entries
                .contains_key(&authority));
            assert!(!workspace
                .pending_environment_runtime_startup_commands
                .contains_key(&authority));
            assert!(!workspace
                .pending_environment_runtime_terminal_authorities
                .contains(&authority));
            assert!(!workspace
                .pending_environment_runtime_forked_conversation_entries
                .contains_key(&authority));
            assert!(!workspace
                .pending_environment_runtime_session_restores
                .contains_key(&authority));

            workspace.queue_environment_runtime_intent(
                &authority,
                EnvironmentRuntimeEntryIntent::ForkedConversation(
                    test_pending_environment_runtime_forked_conversation_entry(),
                ),
                ctx,
            );
            assert!(workspace
                .pending_environment_runtime_forked_conversation_entries
                .contains_key(&authority));
            assert!(!workspace
                .pending_environment_runtime_agent_view_entries
                .contains_key(&authority));
            assert!(!workspace
                .pending_environment_runtime_startup_commands
                .contains_key(&authority));
            assert!(!workspace
                .pending_environment_runtime_terminal_authorities
                .contains(&authority));
            assert!(!workspace
                .pending_environment_runtime_session_restores
                .contains_key(&authority));

            workspace.queue_environment_runtime_intent(
                &authority,
                EnvironmentRuntimeEntryIntent::SessionRestore(
                    test_pending_environment_runtime_session_restore(&authority),
                ),
                ctx,
            );
            assert!(workspace
                .pending_environment_runtime_session_restores
                .contains_key(&authority));
            assert!(!workspace
                .pending_environment_runtime_forked_conversation_entries
                .contains_key(&authority));
            assert!(!workspace
                .pending_environment_runtime_agent_view_entries
                .contains_key(&authority));
            assert!(!workspace
                .pending_environment_runtime_startup_commands
                .contains_key(&authority));
            assert!(!workspace
                .pending_environment_runtime_terminal_authorities
                .contains(&authority));
        });
    });
}

#[test]
fn test_environment_runtime_pending_intents_consume_only_after_terminal_created() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let authority = "ssh:ssh-config:dnyx216".to_string();

            workspace.queue_pending_environment_runtime_terminal(&authority, ctx);
            assert!(workspace.has_pending_environment_runtime_entry_for_authority(&authority));
            assert!(workspace
                .consume_pending_environment_runtime_entry(&authority, false)
                .is_none());
            assert!(workspace
                .pending_environment_runtime_terminal_authorities
                .contains(&authority));
            assert!(workspace
                .consume_pending_environment_runtime_entry(&authority, true)
                .is_none());
            assert!(!workspace.has_pending_environment_runtime_entry_for_authority(&authority));

            workspace.queue_pending_environment_runtime_startup_command(
                &authority,
                "claude --resume environment-session".to_string(),
                ctx,
            );
            assert!(workspace.has_pending_environment_runtime_entry_for_authority(&authority));
            assert!(workspace
                .consume_pending_environment_runtime_entry(&authority, false)
                .is_none());
            assert_eq!(
                workspace
                    .pending_environment_runtime_startup_commands
                    .get(&authority)
                    .map(String::as_str),
                Some("claude --resume environment-session")
            );
            assert!(workspace
                .consume_pending_environment_runtime_entry(&authority, true)
                .is_none());
            assert!(!workspace.has_pending_environment_runtime_entry_for_authority(&authority));

            workspace.queue_pending_environment_runtime_agent_view_entry(
                &authority,
                test_pending_environment_runtime_agent_view_entry(),
                ctx,
            );
            assert!(workspace.has_pending_environment_runtime_entry_for_authority(&authority));
            assert!(workspace
                .consume_pending_environment_runtime_entry(&authority, false)
                .is_none());
            assert!(workspace
                .pending_environment_runtime_agent_view_entries
                .contains_key(&authority));
            assert!(workspace
                .consume_pending_environment_runtime_entry(&authority, true)
                .is_none());
            assert!(!workspace.has_pending_environment_runtime_entry_for_authority(&authority));

            workspace.queue_pending_environment_runtime_forked_conversation_entry(
                &authority,
                test_pending_environment_runtime_forked_conversation_entry(),
                ctx,
            );
            assert!(workspace.has_pending_environment_runtime_entry_for_authority(&authority));
            assert!(workspace
                .consume_pending_environment_runtime_entry(&authority, false)
                .is_none());
            assert!(workspace
                .pending_environment_runtime_forked_conversation_entries
                .contains_key(&authority));
            assert!(workspace
                .consume_pending_environment_runtime_entry(&authority, true)
                .is_none());
            assert!(!workspace.has_pending_environment_runtime_entry_for_authority(&authority));

            let pending_restore = test_pending_environment_runtime_session_restore(&authority);
            workspace.queue_pending_environment_runtime_session_restore(
                &authority,
                pending_restore.clone(),
                ctx,
            );
            assert!(workspace.has_pending_environment_runtime_entry_for_authority(&authority));
            assert!(workspace
                .consume_pending_environment_runtime_entry(&authority, false)
                .is_none());
            assert!(workspace
                .pending_environment_runtime_session_restores
                .contains_key(&authority));
            let consumed_restore = workspace
                .consume_pending_environment_runtime_entry(&authority, true)
                .expect("session restore should be returned only after terminal creation");
            assert_eq!(consumed_restore.session.id, pending_restore.session.id);
            assert!(!workspace.has_pending_environment_runtime_entry_for_authority(&authority));
        });
    });
}

#[test]
fn test_environment_runtime_scan_without_client_reconnects() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let stale_session_id = CoreSessionId::from(9018);
            workspace.mark_environment_runtime_connecting(
                environment.clone(),
                stale_session_id,
                PathBuf::from("/tmp/ashide-test-scan-without-client.sock"),
            );
            workspace
                .environment_runtimes
                .mark_connected_session(stale_session_id, HostId::new("scan-missing-client-host".to_string()));
            workspace.set_active_tab_environment(environment);

            workspace.scan_environment_runtime_agent_sessions(
                authority.clone(),
                stale_session_id,
                ctx,
            );

            let replacement_session_id = workspace
                .environment_runtime_session_for_authority(&authority)
                .expect("scan without a runtime client should trigger reconnect");
            assert_ne!(
                replacement_session_id, stale_session_id,
                "agent-session scan must not silently return when a Connected runtime has no client"
            );
            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                Some(EnvironmentLifecycleState::Connecting)
            );
        });
    });
}

#[test]
fn test_environment_session_source_action_without_client_reconnects() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let stale_session_id = CoreSessionId::from(9019);
            workspace.mark_environment_runtime_connecting(
                environment.clone(),
                stale_session_id,
                PathBuf::from("/tmp/ashide-test-source-action-without-client.sock"),
            );
            workspace
                .environment_runtimes
                .mark_connected_session(stale_session_id, HostId::new("source-action-missing-client-host".to_string()));
            workspace.set_active_tab_environment(environment);

            let session_id = crate::workspace::environment_runtime::environment_cli_agent_session_source_id(
                &authority,
                &CLIAgent::Codex,
                "/root/.codex/sessions/session.jsonl",
            );
            let session = WorkspaceSessionSnapshot {
                id: session_id,
                kind: WorkspaceSessionKind::AgentTerminal,
                label: Some("Codex".to_string()),
                environment_authority_key: Some(authority.clone()),
                cwd: Some("/root/project".to_string()),
                startup_directory: None,
                cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
                cli_command: Some("codex".to_string()),
                cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                conversation_ids: Vec::new(),
                active_conversation_id: None,
                cli_agent_session_id: Some("codex-session".to_string()),
                is_active: false,
                is_pinned: false,
                updated_at_unix_ms: None,
            };

            assert!(
                !workspace.schedule_environment_cli_agent_session_source_action(
                    &session,
                    crate::workspace::environment_runtime::EnvironmentCliAgentSessionSourceAction::Delete,
                    ctx,
                ),
                "source action cannot run without a client, but it must trigger reconnect"
            );
            let replacement_session_id = workspace
                .environment_runtime_session_for_authority(&authority)
                .expect("source action without a runtime client should trigger reconnect");
            assert_ne!(replacement_session_id, stale_session_id);
            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                Some(EnvironmentLifecycleState::Connecting)
            );
        });
    });
}

#[test]
fn test_missing_environment_runtime_transport_descriptor_marks_environment_error() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let environment = EnvironmentSnapshot::runtime_transport(
                EnvironmentKind::Ssh,
                "Missing Provider".to_string(),
                "missing-provider-authority".to_string(),
                Some("missing-provider-ref".to_string()),
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Dormant,
            );
            let authority = environment.authority_key.clone();
            workspace.set_active_tab_environment(environment);

            workspace.ensure_current_environment_runtime_transport_if_needed(ctx);

            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| &environment.lifecycle_state),
                Some(&EnvironmentLifecycleState::Error),
                "missing runtime transport descriptor must become a visible Environment error"
            );
            assert_eq!(
                workspace.tabs[workspace.active_tab_index()]
                    .environment
                    .as_ref()
                    .map(|environment| &environment.lifecycle_state),
                Some(&EnvironmentLifecycleState::Error),
                "active tab Environment lifecycle must stay in sync with current_environment"
            );
            assert!(
                workspace
                    .session_navigator_sessions(ctx)
                    .iter()
                    .all(|session| {
                        session.environment_authority_key.as_deref() != Some(authority.as_str())
                            || !session.is_active
                            || workspace
                                .current_environment
                                .as_ref()
                                .is_some_and(|environment| {
                                    environment.lifecycle_state == EnvironmentLifecycleState::Error
                                })
                    }),
                "session navigator sync should run after missing transport descriptor failure"
            );
        });
    });
}

#[test]
fn test_environment_runtime_bootstrap_failure_clears_pending_entry() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connecting,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9015);
            workspace.mark_environment_runtime_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-ssh-control-bootstrap-fail.sock"),
            );
            workspace.set_active_tab_environment(environment);
            workspace.queue_pending_environment_runtime_terminal(&authority, ctx);

            assert!(
                workspace.has_pending_environment_runtime_entry_for_authority(&authority),
                "test setup must create a pending runtime entry before bootstrap failure"
            );

            workspace.handle_environment_runtime_failed(
                session_id,
                "synthetic bootstrap failure".to_string(),
                ctx,
            );

            assert!(
                !workspace.has_pending_environment_runtime_entry_for_authority(&authority),
                "bootstrap failure must clear stale pending runtime entries"
            );
            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| &environment.lifecycle_state),
                Some(&EnvironmentLifecycleState::Error),
                "bootstrap failure must surface as an Environment error"
            );
        });
    });
}

#[test]
fn test_environment_runtime_disconnected_request_after_bootstrap_marks_error() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9016);
            workspace.mark_environment_runtime_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-ssh-control-request-disconnected.sock"),
            );
            workspace
                .environment_runtimes
                .mark_connected_session(session_id, HostId::new("request-disconnected-host".to_string()));
            workspace.set_active_tab_environment(environment);
            workspace.queue_pending_environment_runtime_terminal(&authority, ctx);

            workspace.handle_environment_runtime_client_request_failed(
                session_id,
                crate::workspace::environment_runtime::EnvironmentRuntimeOperation::NavigateToDirectory,
                crate::workspace::environment_runtime::EnvironmentRuntimeErrorKind::Disconnected,
                ctx,
            );

            assert!(
                workspace.has_pending_environment_runtime_entry_for_authority(&authority),
                "post-bootstrap disconnect should mark the environment unhealthy without consuming pending terminal intent"
            );
            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                Some(EnvironmentLifecycleState::Error),
                "post-bootstrap disconnected client request must not leave the runtime showing Connected"
            );
            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| &environment.lifecycle_state),
                Some(&EnvironmentLifecycleState::Error),
                "current Environment strip state should surface the post-bootstrap runtime disconnect"
            );
        });
    });
}

#[test]
fn test_environment_runtime_decoding_error_after_bootstrap_marks_error() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9017);
            workspace.mark_environment_runtime_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-ssh-control-decode-error.sock"),
            );
            workspace
                .environment_runtimes
                .mark_connected_session(session_id, HostId::new("decode-error-host".to_string()));
            workspace.set_active_tab_environment(environment);

            workspace.handle_environment_runtime_server_message_decoding_error(session_id, ctx);

            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                Some(EnvironmentLifecycleState::Error),
                "post-bootstrap protocol decoding errors should invalidate the runtime instead of leaving a stale Connected environment"
            );
            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| &environment.lifecycle_state),
                Some(&EnvironmentLifecycleState::Error),
                "Environment Strip must surface protocol mismatch/helper incompatibility after bootstrap"
            );
        });
    });
}

#[test]
fn test_environment_live_row_activation_refuses_cross_environment_tab_locator() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let local_tab_index = workspace.active_tab_index();
            workspace.add_terminal_tab(false, ctx);
            let environment_tab_index = workspace.active_tab_index();
            assert_ne!(local_tab_index, environment_tab_index);

            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9014);
            workspace.environment_runtimes.mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-ssh-control-cross-env-live-row.sock"),
            );
            workspace
                .environment_runtimes
                .mark_connected_session(session_id, HostId::new("test-host".to_string()));
            workspace.set_active_tab_environment(environment);

            let cross_environment_live_id = format!("tab:{local_tab_index}:leaf:0");
            workspace
                .restored_workspace_sessions
                .push(WorkspaceSessionSnapshot {
                    id: cross_environment_live_id.clone(),
                    kind: WorkspaceSessionKind::Terminal,
                    label: Some("root@vps".to_string()),
                    environment_authority_key: Some(authority.clone()),
                    cwd: Some("/root".to_string()),
                    startup_directory: None,
                    cli_agent: None,
                    cli_command: None,
                    cli_agent_origin: None,
                    conversation_ids: Vec::new(),
                    active_conversation_id: None,
                    cli_agent_session_id: None,
                    is_active: false,
                    is_pinned: false,
                    updated_at_unix_ms: None,
                });

            workspace.activate_restored_workspace_session(
                &crate::workspace::action::WorkspaceSessionActionTarget::new(
                    cross_environment_live_id.clone(),
                    Some(authority.clone()),
                ),
                ctx,
            );

            assert_eq!(
                workspace.active_tab_index(),
                environment_tab_index,
                "clicking an Environment session row must not focus a tab whose authority is current-app/local"
            );
            assert!(
                workspace
                    .pending_environment_runtime_session_restores
                    .contains_key(&authority),
                "after refusing the cross-environment tab locator, activation should continue through Environment Runtime restore instead of silently doing nothing"
            );
        });
    });
}

#[test]
fn test_activate_restored_workspace_session_shows_error_for_cross_environment_session() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let initial_tab_count = workspace.tab_count();
            let initial_active_tab = workspace.active_tab_index();
            workspace.add_terminal_tab(false, ctx);
            let environment_tab_index = workspace.active_tab_index();
            assert_ne!(initial_active_tab, environment_tab_index);
            assert_eq!(workspace.tab_count(), initial_tab_count + 1);

            let server = test_ssh_server_for_environment_tests();
            let environment =
                crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                    "dnyx216".to_string(),
                    &server,
                    Some("/root/project".to_string()),
                    EnvironmentLifecycleState::Connected,
                );
            let runtime_authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9015);
            workspace.environment_runtimes.mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-ssh-control-cross-env-activate.sock"),
            );
            workspace
                .environment_runtimes
                .mark_connected_session(session_id, HostId::new("test-host".to_string()));
            workspace.set_active_tab_environment(environment);

            let local_session_id = Workspace::ashide_conversation_session_id(AIConversationId::new());
            workspace.restored_workspace_sessions.push(WorkspaceSessionSnapshot {
                id: local_session_id.clone(),
                kind: WorkspaceSessionKind::AgentTerminal,
                label: Some("Local Ashide history".to_string()),
                environment_authority_key: Some("local:/Users/admin/ashide".to_string()),
                cwd: Some("/Users/admin/ashide".to_string()),
                startup_directory: None,
                cli_agent: None,
                cli_command: None,
                cli_agent_origin: None,
                conversation_ids: Vec::new(),
                active_conversation_id: None,
                cli_agent_session_id: None,
                is_active: false,
                is_pinned: false,
                updated_at_unix_ms: None,
            });

            workspace.activate_restored_workspace_session(
                &crate::workspace::action::WorkspaceSessionActionTarget::new(
                    local_session_id,
                    Some("local:/Users/admin/ashide".to_string()),
                ),
                ctx,
            );

            assert_eq!(
                workspace.tab_count(),
                initial_tab_count + 1,
                "cross-environment session activation must not open a new tab"
            );
            assert_eq!(workspace.active_tab_index(), environment_tab_index);
            assert!(
                workspace.restoring_workspace_session_keys.is_empty(),
                "cross-environment session activation must not enter restoring state"
            );
            assert!(
                !workspace
                    .pending_environment_runtime_session_restores
                    .contains_key(&runtime_authority),
                "cross-environment local session activation must not queue runtime restore"
            );
        });
    });
}

#[test]
fn test_runtime_environment_tab_does_not_publish_current_app_terminal_live_row() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            workspace.add_terminal_tab(false, ctx);
            let environment = EnvironmentSnapshot::runtime_transport(
                EnvironmentKind::Ssh,
                "dnyx216".to_string(),
                "ssh:ssh-config:dnyx216".to_string(),
                Some("ssh-config:dnyx216".to_string()),
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();

            // Simulate the bad restored state seen in the GUI: the active tab is
            // labeled as an Environment tab, but the pane inside is still a
            // current-app/local terminal. That pane must not become a live
            // `tab:*` Session Navigator row for the Environment.
            workspace.set_active_tab_environment(environment);
            workspace.restored_workspace_sessions.push(WorkspaceSessionSnapshot {
                id: "persisted-environment-session".to_string(),
                kind: WorkspaceSessionKind::AgentTerminal,
                label: Some("Persisted Codex".to_string()),
                environment_authority_key: Some(authority.clone()),
                cwd: Some("/root/project".to_string()),
                startup_directory: None,
                cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
                cli_command: Some("codex".to_string()),
                cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                conversation_ids: Vec::new(),
                active_conversation_id: None,
                cli_agent_session_id: Some("persisted-codex-session".to_string()),
                is_active: false,
                is_pinned: false,
                updated_at_unix_ms: None,
            });

            let sessions = workspace.session_navigator_sessions(ctx);
            assert!(
                sessions.iter().any(|session| {
                    session.id == "persisted-environment-session"
                        && session.environment_authority_key.as_deref() == Some(authority.as_str())
                }),
                "persisted Environment sessions should still be visible"
            );
            assert!(
                sessions.iter().all(|session| {
                    !(session.id.starts_with("tab:")
                        && session.environment_authority_key.as_deref() == Some(authority.as_str()))
                }),
                "a current-app terminal inside a runtime Environment tab must not masquerade as an Environment live row"
            );
        });
    });
}

#[test]
fn test_restored_environment_session_registers_environment_host_key() {
    let mut session = WorkspaceSessionSnapshot {
        id: "remote-codex".to_string(),
        kind: WorkspaceSessionKind::AgentTerminal,
        label: Some("remote codex".to_string()),
        environment_authority_key: Some("ssh-config:missing-test-host".to_string()),
        cwd: Some("/root/project".to_string()),
        startup_directory: None,
        cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
        cli_command: Some("codex".to_string()),
        cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
        conversation_ids: Vec::new(),
        active_conversation_id: None,
        cli_agent_session_id: Some("codex-restore".to_string()),
        is_active: false,
        is_pinned: false,
        updated_at_unix_ms: None,
    };

    assert_eq!(
        Workspace::workspace_session_environment_host_key(&session),
        Some("ssh-config:missing-test-host".to_string()),
        "restored Environment agent rows must not be registered as current-app sessions"
    );

    session.environment_authority_key = None;
    assert_eq!(
        Workspace::workspace_session_environment_host_key(&session),
        None,
        "current-app restored sessions keep the current-app host key"
    );
}

#[test]
fn test_workspace_session_action_target_none_is_current_app_not_environment_wildcard() {
    let mut session = WorkspaceSessionSnapshot {
        id: "tab:1:leaf:0".to_string(),
        kind: WorkspaceSessionKind::AgentTerminal,
        label: Some("Environment Codex".to_string()),
        environment_authority_key: Some("ssh-config:dnyx216".to_string()),
        cwd: Some("/root/project".to_string()),
        startup_directory: None,
        cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
        cli_command: Some("codex".to_string()),
        cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
        conversation_ids: Vec::new(),
        active_conversation_id: None,
        cli_agent_session_id: Some("codex-restore".to_string()),
        is_active: false,
        is_pinned: false,
        updated_at_unix_ms: None,
    };

    let legacy_target_without_authority =
        crate::workspace::action::WorkspaceSessionActionTarget::new(session.id.clone(), None);
    assert!(
        !Workspace::workspace_session_matches_action_target(
            &session,
            &legacy_target_without_authority
        ),
        "authority-less legacy targets must not wildcard-match runtime-backed Environment sessions"
    );

    let environment_target = crate::workspace::action::WorkspaceSessionActionTarget::new(
        session.id.clone(),
        session.environment_authority_key.clone(),
    );
    assert!(
        Workspace::workspace_session_matches_action_target(&session, &environment_target),
        "authority-scoped Environment targets should still match their own row"
    );

    session.environment_authority_key = Some("local".to_string());
    assert!(
        Workspace::workspace_session_matches_action_target(
            &session,
            &legacy_target_without_authority
        ),
        "authority-less legacy targets remain compatible with current-app/local rows"
    );
}

#[test]
fn test_switch_to_runtime_registry_environment_without_tab_does_not_queue_terminal() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9019);
            workspace.environment_runtimes.mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-switch-registry-env.sock"),
            );
            workspace
                .environment_runtimes
                .mark_connected_session(session_id, HostId::new("switch-registry-host".to_string()));

            assert!(
                workspace.tab_index_for_environment_authority(&authority).is_none(),
                "test setup must simulate a runtime registry environment whose UI tab was lost"
            );
            assert!(
                !workspace.has_pending_environment_runtime_entry_for_authority(&authority),
                "test setup must start without any explicit terminal/agent/codex intent"
            );

            workspace.switch_to_environment_authority(&authority, ctx);

            assert!(
                workspace.tab_index_for_environment_authority(&authority).is_some(),
                "switching to a remembered runtime environment should recreate/activate its environment tab"
            );
            assert!(
                !workspace.has_pending_environment_runtime_entry_for_authority(&authority),
                "switching environments must not be treated as a request to create a new terminal session"
            );
            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| environment.authority_key.as_str()),
                Some(authority.as_str()),
                "switch should update the current environment boundary"
            );
        });
    });
}

#[test]
fn test_switch_to_existing_environment_placeholder_queues_terminal_intent() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Dormant,
            );
            let authority = environment.authority_key.clone();
            workspace.add_restored_environment_runtime_tab(
                environment,
                Some("root@dnyx216".to_string()),
                ctx,
            );
            let environment_tab_index = workspace
                .tab_index_for_environment_authority(&authority)
                .expect("test setup should create a restored Environment tab");
            workspace.activate_tab_internal(0, ctx);

            assert_eq!(
                workspace.tab_index_for_environment_authority(&authority),
                Some(environment_tab_index),
                "test setup should keep an existing placeholder tab for the Environment"
            );
            assert!(
                !workspace.has_pending_environment_runtime_entry_for_authority(&authority),
                "test setup should start without a pending runtime entry"
            );

            workspace.switch_to_environment_authority(&authority, ctx);

            assert!(
                workspace.has_pending_environment_runtime_entry_for_authority(&authority),
                "switching to an existing Environment placeholder must queue the native PTY intent"
            );
            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| environment.authority_key.as_str()),
                Some(authority.as_str()),
                "switch should activate the target Environment tab"
            );
            assert!(
                workspace
                    .environment_runtime_session_for_authority(&authority)
                    .is_some(),
                "switching to an existing dormant Environment placeholder must start runtime bootstrap"
            );
        });
    });
}

#[test]
fn test_authority_context_runtime_open_activates_target_environment_before_spawn() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9020);
            let host_id = HostId::new("authority-context-host".to_string());
            workspace.environment_runtimes.mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-authority-context-open.sock"),
            );
            workspace
                .environment_runtimes
                .mark_connected_session(session_id, host_id.clone());
            workspace.queue_pending_environment_runtime_terminal(&authority, ctx);

            assert!(
                workspace.tab_index_for_environment_authority(&authority).is_none(),
                "test setup must simulate an authority-context open after the environment tab disappeared"
            );
            assert!(
                workspace
                    .current_environment
                    .as_ref()
                    .is_none_or(|environment| environment.authority_key != authority),
                "test setup must start outside the target environment"
            );

            workspace.open_environment_runtime_terminal_for_authority_context(
                EnvironmentRuntimeTarget {
                    authority: authority.clone(),
                    session_id,
                    host_id,
                    root: Some("/root/project".to_string()),
                },
                "/root/project",
                None,
                true,
                ctx,
            );

            assert!(
                workspace.tab_index_for_environment_authority(&authority).is_some(),
                "authority-context opens must recreate/activate the target environment tab before trying to spawn"
            );
            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| environment.authority_key.as_str()),
                Some(authority.as_str()),
                "authority-context opens must switch the UI boundary to the target environment before spawn"
            );
            assert!(
                workspace.has_pending_environment_runtime_entry_for_authority(&authority),
                "without a real runtime client in the unit harness, the pending intent must stay queued instead of being silently consumed"
            );
        });
    });
}

#[test]
fn test_environment_runtime_root_resolution_without_pending_keeps_environment_idle() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9018);
            let host_id = HostId::new("idle-root-resolve-host".to_string());
            workspace.environment_runtimes.mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-ssh-control-idle-root.sock"),
            );
            workspace
                .environment_runtimes
                .mark_connected_session(session_id, host_id.clone());
            workspace.set_active_tab_environment(environment);

            assert!(
                !workspace.has_pending_environment_runtime_entry_for_authority(&authority),
                "test setup must represent connecting/switching an Environment, not creating a terminal session"
            );

            workspace.finish_resolve_environment_runtime_root(
                authority.clone(),
                session_id,
                host_id,
                Ok(EnvironmentRuntimeRoots {
                    workspace_root: "/root/project".to_string(),
                    home_root: "/root".to_string(),
                }),
                None,
                ctx,
            );

            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                Some(EnvironmentLifecycleState::Connected),
                "root resolution without a pending terminal intent should keep the runtime connected instead of trying to spawn a terminal and marking Error"
            );
            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .and_then(|environment| environment.active_workspace_root.as_deref()),
                Some("/root/project"),
                "Environment root should still sync for project/file browser even when no terminal is opened"
            );
            assert!(
                !workspace.has_pending_environment_runtime_entry_for_authority(&authority),
                "root resolution without a pending terminal intent must not invent a pending terminal"
            );
        });
    });
}

#[test]
fn test_environment_runtime_root_resolution_active_placeholder_queues_terminal_intent() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9024);
            let host_id = HostId::new("active-placeholder-root-resolve-host".to_string());
            workspace.add_restored_environment_runtime_tab(
                environment.clone(),
                Some("root@dnyx216".to_string()),
                ctx,
            );
            workspace.environment_runtimes.mark_connecting(
                environment,
                session_id,
                PathBuf::from("/tmp/ashide-test-active-placeholder-root.sock"),
            );
            workspace
                .environment_runtimes
                .mark_connected_session(session_id, host_id.clone());

            assert!(
                workspace.active_environment_runtime_placeholder_matches_authority(
                    &authority, ctx
                ),
                "test setup should have the target Environment placeholder as the active tab"
            );
            assert!(
                !workspace.has_pending_environment_runtime_entry_for_authority(&authority),
                "test setup should start without an explicit pending terminal/agent/restore intent"
            );

            workspace.finish_resolve_environment_runtime_root(
                authority.clone(),
                session_id,
                host_id,
                Ok(EnvironmentRuntimeRoots {
                    workspace_root: "/root/project".to_string(),
                    home_root: "/root".to_string(),
                }),
                None,
                ctx,
            );

            assert!(
                workspace.has_pending_environment_runtime_entry_for_authority(&authority),
                "active Environment placeholders must become pending terminal intents when roots resolve"
            );
        });
    });
}

#[test]
fn test_environment_runtime_root_resolution_updates_non_active_environment_tab() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/old".to_string()),
                EnvironmentLifecycleState::Connecting,
            );
            let authority = environment.authority_key.clone();
            workspace.add_restored_environment_runtime_tab(
                environment.clone(),
                Some("root@dnyx216".to_string()),
                ctx,
            );
            let environment_tab_index = workspace
                .tab_index_for_environment_authority(&authority)
                .expect("test setup should create an Environment tab");
            workspace.activate_tab_internal(0, ctx);

            let session_id = CoreSessionId::from(9022);
            let host_id = HostId::new("non-active-root-resolve-host".to_string());
            workspace.mark_environment_runtime_connecting(
                environment,
                session_id,
                PathBuf::from("/tmp/ashide-test-non-active-root.sock"),
            );
            let _ = workspace.mark_environment_runtime_connected_session(session_id, host_id.clone());

            workspace.finish_resolve_environment_runtime_root(
                authority.clone(),
                session_id,
                host_id,
                Ok(EnvironmentRuntimeRoots {
                    workspace_root: "/root/project-new".to_string(),
                    home_root: "/root".to_string(),
                }),
                None,
                ctx,
            );

            assert_eq!(
                workspace.tabs[environment_tab_index]
                    .environment
                    .as_ref()
                    .and_then(|environment| environment.active_workspace_root.as_deref()),
                Some("/root/project-new"),
                "root resolution must update matching Environment tabs even when they are not active"
            );
            assert_eq!(
                workspace.tabs[environment_tab_index]
                    .environment
                    .as_ref()
                    .map(|environment| &environment.lifecycle_state),
                Some(&EnvironmentLifecycleState::Connected),
                "late Connected events must not leave a background Environment tab in preparing"
            );
        });
    });
}

#[test]
fn test_environment_runtime_preparation_watchdog_marks_stuck_bootstrap_error() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connecting,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9021);
            workspace.mark_environment_runtime_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-preparation-watchdog.sock"),
            );
            workspace.set_active_tab_environment(environment);
            workspace
                .environment_runtime_preparation_generations
                .insert(authority.clone(), 1);

            workspace.handle_environment_runtime_preparation_watchdog_timeout(
                authority.clone(),
                session_id,
                1,
                "connecting",
                ENVIRONMENT_RUNTIME_PREPARATION_TIMEOUT,
                ctx,
            );

            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                Some(EnvironmentLifecycleState::Error),
                "preparation watchdog timeout must turn a stuck Connecting/Installing runtime into a visible Environment error"
            );
            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| &environment.lifecycle_state),
                Some(&EnvironmentLifecycleState::Error),
                "Environment Strip should not remain in the preparing state after watchdog timeout"
            );
        });
    });
}

#[test]
fn test_environment_runtime_root_resolution_keeps_pending_entry_until_terminal_created() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9014);
            let host_id = HostId::new("test-host".to_string());
            workspace.environment_runtimes.mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-ssh-control-drain.sock"),
            );
            workspace
                .environment_runtimes
                .mark_connected_session(session_id, host_id.clone());
            workspace.set_active_tab_environment(environment);

            workspace.handle_action(
                &WorkspaceAction::AddTerminalTab {
                    hide_homepage: false,
                },
                ctx,
            );
            assert!(
                workspace
                    .pending_environment_runtime_terminal_authorities
                    .contains(&authority),
                "new Environment terminal intent should be pending before native PTY exists"
            );

            workspace.finish_resolve_environment_runtime_root(
                authority.clone(),
                session_id,
                host_id,
                Ok(EnvironmentRuntimeRoots {
                    workspace_root: "/root/project".to_string(),
                    home_root: "/root".to_string(),
                }),
                None,
                ctx,
            );

            assert!(
                workspace
                    .pending_environment_runtime_terminal_authorities
                    .contains(&authority),
                "pending terminal intent must not be consumed when no runtime terminal was created"
            );
            assert!(
                workspace.session_navigator_sessions(ctx).iter().any(|session| {
                    session.is_active
                        && session.environment_authority_key.as_deref() == Some(authority.as_str())
                        && session.id.starts_with("tab:")
                }),
                "failed native PTY creation must keep the active Environment placeholder row visible"
            );
        });
    });
}

#[test]
fn test_open_directory_in_new_tab_from_environment_runtime_ignores_current_app_path() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let session_id = CoreSessionId::from(9011);
            workspace.environment_runtimes.mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-ssh-control-open-dir.sock"),
            );
            workspace
                .environment_runtimes
                .mark_connected_session(session_id, HostId::new("test-host".to_string()));
            workspace.set_active_tab_environment(environment);

            workspace.open_directory_in_new_tab(PathBuf::from("/srv/app"), ctx);

            // On a connected runtime env, open_directory_in_new_tab routes through
            // RuntimeEntryBackend::open_directory_tab → open_ready_environment_runtime_terminal_tab,
            // which opens a NEW runtime tab rooted at the requested path (not the
            // local current-app path or the env's prior active_workspace_root).
            assert_eq!(
                workspace.tab_count(),
                2,
                "open_directory_in_new_tab on connected runtime should open a new runtime tab"
            );
            let active_tab = &workspace.tabs[workspace.active_tab_index()];
            assert_eq!(
                active_tab
                    .environment
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Ssh)
            );
            assert_eq!(
                active_tab
                    .environment
                    .as_ref()
                    .and_then(|environment| environment.active_workspace_root.as_deref()),
                Some("/srv/app")
            );
        });
    });
}

#[cfg(feature = "local_fs")]
#[test]
fn test_open_directory_file_target_new_tab_from_environment_uses_runtime() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        let path = std::env::temp_dir().join("ashide-open-directory-file-target-remote");
        let _ = std::fs::create_dir_all(&path);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let session_id = CoreSessionId::from(9012);
            workspace.environment_runtimes.mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-ssh-control-open-file-dir.sock"),
            );
            workspace
                .environment_runtimes
                .mark_connected_session(session_id, HostId::new("test-host".to_string()));
            workspace.set_active_tab_environment(environment);

            workspace.open_file_with_target(
                path.clone(),
                FileTarget::CodeEditor(EditorLayout::NewTab),
                None,
                CodeSource::FileTree { path: path.clone() },
                ctx,
            );

            assert_eq!(workspace.tab_count(), 2);
            let active_tab = &workspace.tabs[workspace.active_tab_index()];
            assert_eq!(
                active_tab
                    .environment
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Ssh)
            );
            assert_eq!(
                active_tab
                    .environment
                    .as_ref()
                    .and_then(|environment| environment.active_workspace_root.as_deref()),
                path.to_str()
            );
        });

        let _ = std::fs::remove_dir_all(path);
    });
}

fn assert_new_environment_tab_stays_in_environment_group(
    workspace: &mut Workspace,
    environment: EnvironmentSnapshot,
    action: WorkspaceAction,
    ctx: &mut ViewContext<Workspace>,
) {
    workspace.set_active_tab_environment(environment.clone());
    let ssh_tab_index = workspace.active_tab_index();

    workspace.add_explicit_terminal_bootstrap_default_tab(None, ctx);
    assert_eq!(workspace.tab_count(), 2);
    assert_eq!(workspace.active_tab_index(), 1);
    assert_eq!(
        workspace.tabs[1]
            .environment
            .as_ref()
            .map(|environment| &environment.kind),
        Some(&EnvironmentKind::Local)
    );

    workspace.activate_tab_internal(ssh_tab_index, ctx);
    workspace.handle_action(&action, ctx);

    assert_eq!(workspace.tab_count(), 3);
    assert_eq!(workspace.active_tab_index(), 1);
    assert_eq!(
        workspace.tabs[0]
            .environment
            .as_ref()
            .map(|environment| environment.authority_key.as_str()),
        Some(environment.authority_key.as_str())
    );
    assert_eq!(
        workspace.tabs[1]
            .environment
            .as_ref()
            .map(|environment| environment.authority_key.as_str()),
        Some(environment.authority_key.as_str())
    );
    assert_eq!(
        workspace.tabs[2]
            .environment
            .as_ref()
            .map(|environment| &environment.kind),
        Some(&EnvironmentKind::Local)
    );
}

#[test]
fn test_new_terminal_tab_from_environment_runtime_stays_in_environment_group() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            assert_new_environment_tab_stays_in_environment_group(
                workspace,
                environment,
                WorkspaceAction::AddTerminalTab {
                    hide_homepage: false,
                },
                ctx,
            );
        });
    });
}

#[test]
fn test_new_agent_tab_from_environment_runtime_stays_in_environment_group() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            assert_new_environment_tab_stays_in_environment_group(
                workspace,
                environment,
                WorkspaceAction::AddAgentTab,
                ctx,
            );
        });
    });
}

#[test]
fn test_session_navigator_marks_only_one_environment_live_session_active() {
    let mut sessions = vec![
        WorkspaceSessionSnapshot {
            id: "tab:0:leaf:0".to_string(),
            kind: WorkspaceSessionKind::Terminal,
            label: Some("Terminal A".to_string()),
            environment_authority_key: Some("ssh:ssh-config:dnyx216".to_string()),
            cwd: Some("/root/project".to_string()),
            startup_directory: None,
            cli_agent: None,
            cli_command: None,
            cli_agent_origin: None,
            conversation_ids: Vec::new(),
            active_conversation_id: None,
            cli_agent_session_id: None,
            is_active: true,
            is_pinned: false,
            updated_at_unix_ms: Some(1),
        },
        WorkspaceSessionSnapshot {
            id: "tab:1:leaf:0".to_string(),
            kind: WorkspaceSessionKind::Terminal,
            label: Some("Terminal B".to_string()),
            environment_authority_key: Some("ssh:ssh-config:dnyx216".to_string()),
            cwd: Some("/root/project".to_string()),
            startup_directory: None,
            cli_agent: None,
            cli_command: None,
            cli_agent_origin: None,
            conversation_ids: Vec::new(),
            active_conversation_id: None,
            cli_agent_session_id: None,
            is_active: true,
            is_pinned: false,
            updated_at_unix_ms: Some(2),
        },
        WorkspaceSessionSnapshot {
            id: "tab:2:leaf:0".to_string(),
            kind: WorkspaceSessionKind::AgentTerminal,
            label: Some("Codex".to_string()),
            environment_authority_key: Some("ssh:ssh-config:dnyx216".to_string()),
            cwd: Some("/root/project".to_string()),
            startup_directory: None,
            cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
            cli_command: Some("codex".to_string()),
            cli_agent_origin: Some(CliAgentSessionOrigin::CommandDetected),
            conversation_ids: Vec::new(),
            active_conversation_id: None,
            cli_agent_session_id: Some("codex-live".to_string()),
            is_active: true,
            is_pinned: false,
            updated_at_unix_ms: Some(3),
        },
    ];
    let preferred_key = sessions[2].logical_key();

    Workspace::normalize_session_navigator_active_state(&mut sessions, Some(&preferred_key));

    let active_sessions = sessions
        .iter()
        .filter(|session| session.is_active)
        .collect::<Vec<_>>();
    assert_eq!(active_sessions.len(), 1);
    assert_eq!(active_sessions[0].label.as_deref(), Some("Codex"));
}

#[test]
fn test_new_specific_codex_tab_from_environment_runtime_stays_in_environment_group() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            assert_new_environment_tab_stays_in_environment_group(
                workspace,
                environment,
                WorkspaceAction::AddSpecificAgentTab(CLIAgent::Codex),
                ctx,
            );
        });
    });
}

#[test]
fn test_add_default_tab_from_environment_runtime_creates_runtime_terminal_even_with_welcome_tab() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let session_id = CoreSessionId::from(9002);
            workspace.environment_runtimes.mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-ssh-control-default.sock"),
            );
            workspace
                .environment_runtimes
                .mark_connected_session(session_id, HostId::new("test-host".to_string()));
            workspace.set_active_tab_environment(environment);

            workspace.handle_action(&WorkspaceAction::AddDefaultTab, ctx);

            assert_eq!(workspace.tab_count(), 2);
            let active_tab = &workspace.tabs[workspace.active_tab_index()];
            assert_eq!(
                active_tab
                    .environment
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Ssh)
            );
            assert_eq!(
                active_tab
                    .environment
                    .as_ref()
                    .and_then(|environment| environment.active_workspace_root.as_deref()),
                Some("/root/project")
            );
        });
    });
}

#[test]
fn test_add_default_tab_on_runtime_with_tab_config_mode_but_no_config_falls_through_to_runtime_terminal() {
    // #18: AddDefaultTab must consult default session mode BEFORE the runtime
    // try-route. With default mode = TabConfig but no default tab config
    // resolved, the missing-config fall-through must still open a runtime
    // terminal on a runtime env (previously the runtime try-route preempted the
    // TabConfig branch entirely and opened a plain terminal regardless of mode,
    // so TabConfig was silently ignored on runtime envs).
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            // Switch default session mode to TabConfig with no default config path
            // so resolved_default_tab_config() returns None.
            AISettings::handle(ctx).update(ctx, |settings, ctx| {
                report_if_error!(settings
                    .default_session_mode_internal
                    .set_value(DefaultSessionMode::TabConfig, ctx));
                report_if_error!(settings.default_tab_config_path.set_value(String::new(), ctx));
            });

            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let session_id = CoreSessionId::from(9003);
            workspace.environment_runtimes.mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-ssh-control-tabconfig.sock"),
            );
            workspace
                .environment_runtimes
                .mark_connected_session(session_id, HostId::new("test-host".to_string()));
            workspace.set_active_tab_environment(environment);

            workspace.handle_action(&WorkspaceAction::AddDefaultTab, ctx);

            assert_eq!(workspace.tab_count(), 2);
            let active_tab = &workspace.tabs[workspace.active_tab_index()];
            // Fall-through landed on a runtime (SSH) terminal, not a local tab.
            assert_eq!(
                active_tab
                    .environment
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Ssh)
            );
        });
    });
}

#[test]
fn test_deliver_fork_split_pane_on_connecting_runtime_stages_loading_pane_and_queues() {
    // #15: split-pane fork on a runtime that is still CONNECTING must not
    // silently abort. It should stage a loading pane and queue the ForkEntry
    // (tagged with the loading pane id) so the connect callback can replace
    // that pane and replay the restore after bootstrap — same queue/replay
    // discipline as the new-tab fork path.
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connecting,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9004);
            workspace.environment_runtimes.mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-ssh-control-fork-split.sock"),
            );
            // Intentionally NOT mark_connected_session: the runtime is still
            // connecting, so spawn_plan_for_environment -> RuntimeBootstrap.
            workspace.set_active_tab_environment(environment);

            let entry = test_pending_environment_runtime_forked_conversation_entry();
            workspace.deliver_fork_split_pane(entry, ctx);

            // ForkEntry queued for the authority.
            assert!(
                workspace
                    .pending_environment_runtime_forked_conversation_entries
                    .contains_key(&authority),
                "connecting-runtime fork split-pane must queue the ForkEntry"
            );
            // Loading pane id recorded so the connect callback replaces it.
            assert!(
                workspace
                    .pending_environment_runtime_split_pane_loading_ids
                    .contains_key(&authority),
                "connecting-runtime fork split-pane must record the loading pane id"
            );
        });
    });
}

#[test]
fn test_deliver_agent_pane_split_on_connecting_runtime_stages_loading_pane_and_queues() {
    // #16: split-pane agent on a runtime that is still CONNECTING must not
    // silently abort. It should stage a loading pane and queue the AgentTabEntry
    // (tagged with the loading pane id) so the connect callback can replace that
    // pane and enter agent view after bootstrap — same queue/replay discipline
    // as the new-tab agent path. Callers that need a live view to auto-send
    // (FixInAgentMode / FixSettingsWithOz) see `None` and skip the send.
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connecting,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9005);
            workspace.environment_runtimes.mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-ssh-control-agent-split.sock"),
            );
            workspace.set_active_tab_environment(environment);

            let returned_view =
                workspace.add_agent_pane_in_current_environment(None, None, ctx);
            assert!(
                returned_view.is_none(),
                "connecting-runtime agent split-pane must return no live view"
            );
            assert!(
                workspace
                    .pending_environment_runtime_agent_view_entries
                    .contains_key(&authority),
                "connecting-runtime agent split-pane must queue the AgentTabEntry"
            );
            assert!(
                workspace
                    .pending_environment_runtime_split_pane_loading_ids
                    .contains_key(&authority),
                "connecting-runtime agent split-pane must record the loading pane id"
            );
        });
    });
}

#[test]
fn test_deliver_startup_command_split_pane_on_connecting_runtime_stages_loading_pane_and_queues() {
    // #17: editor-fallback (and any split-pane startup command) on a runtime
    // that is still CONNECTING must not silently abort. It should stage a
    // loading pane and queue the command (tagged with the loading pane id) so
    // the connect callback replaces that pane and runs the command after
    // bootstrap — same queue/replay discipline as the new-tab startup-command
    // path.
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connecting,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9006);
            workspace.environment_runtimes.mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-ssh-control-editor-split.sock"),
            );
            workspace.set_active_tab_environment(environment);

            workspace.deliver_startup_command_split_pane(
                "vim /root/project/README.md".to_string(),
                ctx,
            );

            assert!(
                workspace
                    .pending_environment_runtime_startup_commands
                    .contains_key(&authority),
                "connecting-runtime editor split-pane must queue the startup command"
            );
            assert!(
                workspace
                    .pending_environment_runtime_split_pane_loading_ids
                    .contains_key(&authority),
                "connecting-runtime editor split-pane must record the loading pane id"
            );
        });
    });
}

#[test]
fn test_add_agent_tab_from_environment_runtime_inherits_environment() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            workspace.set_active_tab_environment(environment);

            workspace.handle_action(&WorkspaceAction::AddAgentTab, ctx);

            assert_eq!(workspace.tab_count(), 2);
            let active_tab = &workspace.tabs[workspace.active_tab_index()];
            assert_eq!(
                active_tab
                    .environment
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Ssh)
            );
            assert_eq!(
                active_tab
                    .environment
                    .as_ref()
                    .and_then(|environment| environment.active_workspace_root.as_deref()),
                Some("/root/project")
            );
            assert!(workspace
                .pending_environment_runtime_agent_view_entries
                .contains_key(&authority));

            workspace.disconnect_environment_authority(&authority, ctx);
            assert!(!workspace
                .pending_environment_runtime_agent_view_entries
                .contains_key(&authority));
        });
    });
}

#[test]
fn test_project_agent_directory_from_local_applies_via_shared_agent_tab_entry() {
    // #21: local open_agent_directory_tab must use the same AgentTabEntry +
    // apply_agent_tab_entry_immediately path as runtime (after bootstrap), not the
    // divergent start_agent_mode_in_new_pane + caller-side code-review apply.
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let initial_tabs = workspace.tab_count();
            let terminal_ready = workspace.open_agent_directory_tab_in_current_environment(
                PathBuf::from("/Users/admin/ashide"),
                false,
                ctx,
            );
            assert!(
                terminal_ready,
                "local project-agent must materialize synchronously"
            );
            assert_eq!(workspace.tab_count(), initial_tabs + 1);
            let terminal_view = workspace
                .active_tab_pane_group()
                .as_ref(ctx)
                .active_session_view(ctx)
                .expect("new agent directory tab must have a terminal");
            assert!(
                terminal_view
                    .as_ref(ctx)
                    .agent_view_controller()
                    .as_ref(ctx)
                    .is_active(),
                "local project-agent must enter agent view via apply_agent_tab_entry_immediately"
            );
        });
    });
}

#[test]
fn test_project_agent_directory_from_environment_runtime_queues_agent_intent() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment =
                crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                    "dnyx216".to_string(),
                    &server,
                    Some("/root/project".to_string()),
                    EnvironmentLifecycleState::Connected,
                );
            let authority = environment.authority_key.clone();
            workspace.set_active_tab_environment(environment);

            let terminal_ready = workspace.open_agent_directory_tab_in_current_environment(
                PathBuf::from("/root/agent-project"),
                false,
                ctx,
            );

            assert!(
                !terminal_ready,
                "project agent entry in an Environment Runtime must queue an agent intent instead of opening a current-app terminal immediately"
            );
            assert!(
                workspace
                    .pending_environment_runtime_agent_view_entries
                    .contains_key(&authority),
                "project agent entry should be stored as a pending Environment Runtime agent intent"
            );
            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .and_then(|environment| environment.active_workspace_root.as_deref()),
                Some("/root/agent-project"),
                "project agent entry should update the active Environment root before runtime drain"
            );
            assert_eq!(
                workspace.tabs[workspace.active_tab_index()]
                    .environment
                    .as_ref()
                    .and_then(|environment| environment.active_workspace_root.as_deref()),
                Some("/root/agent-project")
            );
        });
    });
}

#[test]
fn test_ai_mode_tab_from_environment_runtime_inherits_environment() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            workspace.set_active_tab_environment(environment);

            workspace.add_terminal_tab_in_ai_mode(None, ctx);

            assert_eq!(workspace.tab_count(), 2);
            let active_tab = &workspace.tabs[workspace.active_tab_index()];
            assert_eq!(
                active_tab
                    .environment
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Ssh)
            );
            assert_eq!(
                active_tab
                    .environment
                    .as_ref()
                    .and_then(|environment| environment.active_workspace_root.as_deref()),
                Some("/root/project")
            );
            assert!(workspace
                .pending_environment_runtime_agent_view_entries
                .contains_key(&authority));
        });
    });
}

#[test]
fn test_environment_restored_workspace_sessions_show_in_session_navigator() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority_key = environment.authority_key.clone();
            workspace.set_active_tab_environment(environment);
            workspace
                .restored_workspace_sessions
                .push(WorkspaceSessionSnapshot {
                    id: "environment-restored-session".to_string(),
                    kind: WorkspaceSessionKind::AgentTerminal,
                    label: Some("Environment Codex".to_string()),
                    environment_authority_key: Some(authority_key.clone()),
                    cwd: Some("/root/project".to_string()),
                    startup_directory: None,
                    cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
                    cli_command: Some("codex".to_string()),
                    cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                    conversation_ids: Vec::new(),
                    active_conversation_id: None,
                    cli_agent_session_id: Some("codex-environment-1".to_string()),
                    is_active: false,
                    is_pinned: false,
                    updated_at_unix_ms: None,
                });
            workspace
                .restored_workspace_sessions
                .push(WorkspaceSessionSnapshot {
                    id: "current-app-restored-session".to_string(),
                    kind: WorkspaceSessionKind::AgentTerminal,
                    label: Some("Current-App Codex".to_string()),
                    environment_authority_key: Some("local".to_string()),
                    cwd: Some("/Users/admin/project".to_string()),
                    startup_directory: None,
                    cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
                    cli_command: Some("codex".to_string()),
                    cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                    conversation_ids: Vec::new(),
                    active_conversation_id: None,
                    cli_agent_session_id: Some("codex-current-app-1".to_string()),
                    is_active: false,
                    is_pinned: false,
                    updated_at_unix_ms: None,
                });

            let sessions = workspace.session_navigator_sessions(ctx);

            assert!(
                sessions.iter().any(|session| {
                    session.id == "environment-restored-session"
                        && session.environment_authority_key.as_deref()
                            == Some(authority_key.as_str())
                }),
                "environment restored session should remain visible under its Environment"
            );
            assert!(
                sessions
                    .iter()
                    .all(|session| session.id != "current-app-restored-session"),
                "Environment Session Navigator must not leak current-app/external sessions"
            );
        });
    });
}

#[test]
fn test_environment_restored_session_keeps_pending_restore_until_terminal_created() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9010);
            workspace.environment_runtimes.mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-ssh-control-restore.sock"),
            );
            workspace
                .environment_runtimes
                .mark_connected_session(session_id, HostId::new("restore-host".to_string()));
            workspace.set_active_tab_environment(environment);

            let restored = WorkspaceSessionSnapshot {
                id: "remote-restore-connected".to_string(),
                kind: WorkspaceSessionKind::AgentTerminal,
                label: Some("Environment Codex Connected".to_string()),
                environment_authority_key: Some(authority.clone()),
                cwd: Some("/root/project".to_string()),
                startup_directory: None,
                cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
                cli_command: Some("codex".to_string()),
                cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                conversation_ids: Vec::new(),
                active_conversation_id: None,
                cli_agent_session_id: Some("codex-connected-restore".to_string()),
                is_active: false,
                is_pinned: false,
                updated_at_unix_ms: None,
            };
            let logical_key = Workspace::workspace_session_logical_key(&restored);
            workspace.restored_workspace_sessions.push(restored.clone());

            workspace.activate_restored_workspace_session(
                &crate::workspace::action::WorkspaceSessionActionTarget::new(
                    restored.id.clone(),
                    restored.environment_authority_key.clone(),
                ),
                ctx,
            );

            assert!(
                workspace
                    .pending_environment_runtime_session_restores
                    .contains_key(&authority),
                "pending restore must survive when no runtime terminal was actually created"
            );
            assert_eq!(
                workspace
                    .pending_environment_runtime_session_restores
                    .get(&authority)
                    .and_then(|pending| pending.startup_command.as_deref()),
                Some("codex resume codex-connected-restore"),
                "clicking an Environment Codex restore row must queue the explicit remote resume command, not only open a shell"
            );
            assert_eq!(
                workspace.active_restored_workspace_session_key.as_deref(),
                Some(logical_key.as_str())
            );

            let sessions = workspace.session_navigator_sessions(ctx);
            assert!(
                sessions.iter().any(|session| {
                    session.id == restored.id
                        && session.is_active
                        && session.environment_authority_key.as_deref() == Some(authority.as_str())
                }),
                "pending restore must keep an active Environment session row visible; sessions={sessions:#?}"
            );

            let left_panel_sessions = workspace.session_navigator_sessions(ctx);
            assert!(
                left_panel_sessions.iter().any(|session| {
                    session.id == restored.id
                        && session.is_active
                        && session.environment_authority_key.as_deref() == Some(authority.as_str())
                }),
                "left panel must stay synced with the active pending restore row; left_panel_sessions={left_panel_sessions:#?}"
            );
        });
    });
}

#[test]
fn test_environment_runtime_restored_first_class_cli_agents_queue_remote_startup_commands() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let cases = [
                (
                    CLIAgent::Claude,
                    Some("claude-session-123"),
                    "claude --resume claude-session-123",
                ),
                (
                    CLIAgent::Codex,
                    Some("rollout-2026-05-18T15-48-54-019e3a0f-2fa7-78d2-ac9d-09b9c6b228ed"),
                    "codex resume 019e3a0f-2fa7-78d2-ac9d-09b9c6b228ed",
                ),
                (
                    CLIAgent::Antigravity,
                    None,
                    "agy '/root/project with spaces'",
                ),
            ];

            for (index, (agent, cli_agent_session_id, expected_startup_command)) in
                cases.into_iter().enumerate()
            {
                let server = test_ssh_server_for_environment_tests();
                let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                    "dnyx216".to_string(),
                    &server,
                    Some("/root/project with spaces".to_string()),
                    EnvironmentLifecycleState::Connected,
                );
                let authority = environment.authority_key.clone();
                let session_id = CoreSessionId::from(9011 + index as u64);
                workspace.environment_runtimes.mark_connecting(
                    environment.clone(),
                    session_id,
                    PathBuf::from(format!(
                        "/tmp/ashide-test-ssh-control-first-class-restore-{index}.sock"
                    )),
                );
                workspace
                    .environment_runtimes
                    .mark_connected_session(session_id, HostId::new("restore-host".to_string()));
                workspace.set_active_tab_environment(environment);

                let initial_tab_count = workspace.tab_count();
                assert!(
                    agent.adapter_capabilities().can_target_environment_runtime,
                    "{agent:?} should be a first-class Environment Runtime target"
                );
                let restored = WorkspaceSessionSnapshot {
                    id: format!("environment-{}-restore", agent.command_prefix()),
                    kind: WorkspaceSessionKind::AgentTerminal,
                    label: Some(format!("Environment {}", agent.display_name())),
                    environment_authority_key: Some(authority.clone()),
                    cwd: Some("/root/project with spaces".to_string()),
                    startup_directory: None,
                    cli_agent: Some(agent.to_serialized_name()),
                    cli_command: Some(agent.command_prefix().to_string()),
                    cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                    conversation_ids: Vec::new(),
                    active_conversation_id: None,
                    cli_agent_session_id: cli_agent_session_id.map(str::to_string),
                    is_active: false,
                    is_pinned: false,
                    updated_at_unix_ms: None,
                };
                let logical_key = Workspace::workspace_session_logical_key(&restored);
                workspace.restored_workspace_sessions.push(restored.clone());

                workspace.activate_restored_workspace_session(
                    &crate::workspace::action::WorkspaceSessionActionTarget::new(
                        restored.id.clone(),
                        restored.environment_authority_key.clone(),
                    ),
                    ctx,
                );

                assert_eq!(
                    workspace
                        .pending_environment_runtime_session_restores
                        .get(&authority)
                        .and_then(|pending| pending.startup_command.as_deref()),
                    Some(expected_startup_command),
                    "{agent:?} restore must queue the native remote startup command without prepending a current-app cd"
                );
                assert_eq!(
                    workspace.active_restored_workspace_session_key.as_deref(),
                    Some(logical_key.as_str()),
                    "{agent:?} restore should keep the logical session active while waiting for the remote PTY"
                );
                assert_eq!(
                    workspace.tab_count(),
                    initial_tab_count,
                    "{agent:?} Environment restore must not create a current-app bootstrap tab before a runtime PTY exists"
                );

                let sessions = workspace.session_navigator_sessions(ctx);
                assert!(
                    sessions.iter().any(|session| {
                        session.id == restored.id
                            && session.is_active
                            && session.environment_authority_key.as_deref()
                                == Some(authority.as_str())
                    }),
                    "{agent:?} pending restore must keep an active Environment session row visible; sessions={sessions:#?}"
                );
            }
        });
    });
}

#[test]
fn test_open_terminal_bootstrap_restored_session_refuses_environment_runtime_session() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let initial_tab_count = workspace.tab_count();
            let session = WorkspaceSessionSnapshot {
                id: "environment-session-refused-by-terminal-bootstrap".to_string(),
                kind: WorkspaceSessionKind::AgentTerminal,
                label: Some(
                    "Environment Codex should not open through terminal bootstrap".to_string(),
                ),
                environment_authority_key: Some("ssh:ssh-config:dnyx216".to_string()),
                cwd: Some("/root/project".to_string()),
                startup_directory: None,
                cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
                cli_command: Some("codex".to_string()),
                cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                conversation_ids: Vec::new(),
                active_conversation_id: None,
                cli_agent_session_id: Some("codex-environment-refuse-1".to_string()),
                is_active: false,
                is_pinned: false,
                updated_at_unix_ms: None,
            };
            let logical_key = Workspace::workspace_session_logical_key(&session);
            workspace
                .restoring_workspace_session_keys
                .insert(session.id.clone());
            workspace
                .restoring_workspace_session_keys
                .insert(logical_key.clone());
            workspace.active_restored_workspace_session_key = Some(logical_key.clone());

            workspace.open_terminal_bootstrap_restored_session_terminal(
                Some(PathBuf::from("/root/project")),
                &session,
                Some("codex".to_string()),
                ctx,
            );

            assert_eq!(
                workspace.tab_count(),
                initial_tab_count,
                "environment restored session must not create a current-app terminal tab"
            );
            assert!(
                !workspace
                    .restoring_workspace_session_keys
                    .contains(&session.id),
                "environment restore id marker should be cleared after terminal-bootstrap refusal"
            );
            assert!(
                !workspace
                    .restoring_workspace_session_keys
                    .contains(&logical_key),
                "environment restore logical marker should be cleared after terminal-bootstrap refusal"
            );
            assert!(
                workspace.active_restored_workspace_session_key.is_none(),
                "refused environment restore should clear stale active restored marker"
            );
        });
    });
}

#[test]
fn test_add_tab_with_shell_clears_environment_restored_active_session_marker() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority_key = environment.authority_key.clone();
            workspace.set_active_tab_environment(environment);
            let session = WorkspaceSessionSnapshot {
                id: "environment-restored-shell-session".to_string(),
                kind: WorkspaceSessionKind::AgentTerminal,
                label: Some("Remote Shell".to_string()),
                environment_authority_key: Some(authority_key),
                cwd: Some("/root/project".to_string()),
                startup_directory: None,
                cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
                cli_command: Some("codex".to_string()),
                cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                conversation_ids: Vec::new(),
                active_conversation_id: None,
                cli_agent_session_id: Some("codex-environment-shell-1".to_string()),
                is_active: false,
                is_pinned: false,
                updated_at_unix_ms: None,
            };
            workspace.active_restored_workspace_session_key =
                Some(Workspace::workspace_session_logical_key(&session));
            workspace.restored_workspace_sessions.push(session);
            assert!(workspace
                .session_navigator_sessions(ctx)
                .iter()
                .any(|session| session.id == "environment-restored-shell-session"
                    && session.is_active));

            workspace.handle_action(
                &WorkspaceAction::AddTabWithShell {
                    shell: AvailableShell::default(),
                },
                ctx,
            );

            assert!(workspace.active_restored_workspace_session_key.is_none());
            assert!(!workspace
                .session_navigator_sessions(ctx)
                .iter()
                .any(|session| session.id == "environment-restored-shell-session"
                    && session.is_active));
            assert_eq!(
                workspace.tabs[workspace.active_tab_index()]
                    .environment
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Ssh)
            );
        });
    });
}

#[test]
fn test_new_terminal_clears_environment_restored_active_session_marker() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority_key = environment.authority_key.clone();
            workspace.set_active_tab_environment(environment);
            let session = WorkspaceSessionSnapshot {
                id: "environment-restored-session".to_string(),
                kind: WorkspaceSessionKind::AgentTerminal,
                label: Some("Environment Codex".to_string()),
                environment_authority_key: Some(authority_key),
                cwd: Some("/root/project".to_string()),
                startup_directory: None,
                cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
                cli_command: Some("codex".to_string()),
                cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                conversation_ids: Vec::new(),
                active_conversation_id: None,
                cli_agent_session_id: Some("codex-environment-1".to_string()),
                is_active: false,
                is_pinned: false,
                updated_at_unix_ms: None,
            };
            workspace.active_restored_workspace_session_key =
                Some(Workspace::workspace_session_logical_key(&session));
            workspace.restored_workspace_sessions.push(session);
            assert!(workspace
                .session_navigator_sessions(ctx)
                .iter()
                .any(|session| session.id == "environment-restored-session" && session.is_active));

            workspace.add_terminal_tab(false, ctx);

            assert!(workspace.active_restored_workspace_session_key.is_none());
            assert!(!workspace
                .session_navigator_sessions(ctx)
                .iter()
                .any(|session| session.id == "environment-restored-session" && session.is_active));
        });
    });
}

#[test]
fn test_activating_tab_syncs_session_navigator_environment_cache() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let remote_authority = environment.authority_key.clone();
            workspace.set_active_tab_environment(environment);
            let ssh_tab_index = workspace.active_tab_index();
            workspace
                .restored_workspace_sessions
                .push(WorkspaceSessionSnapshot {
                    id: "environment-session".to_string(),
                    kind: WorkspaceSessionKind::AgentTerminal,
                    label: Some("Environment Codex".to_string()),
                    environment_authority_key: Some(remote_authority.clone()),
                    cwd: Some("/root/project".to_string()),
                    startup_directory: None,
                    cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
                    cli_command: Some("codex".to_string()),
                    cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                    conversation_ids: Vec::new(),
                    active_conversation_id: None,
                    cli_agent_session_id: Some("environment-1".to_string()),
                    is_active: false,
                    is_pinned: false,
                    updated_at_unix_ms: None,
                });
            workspace
                .restored_workspace_sessions
                .push(WorkspaceSessionSnapshot {
                    id: "current-app-session".to_string(),
                    kind: WorkspaceSessionKind::AgentTerminal,
                    label: Some("Current-App Codex".to_string()),
                    environment_authority_key: Some("local".to_string()),
                    cwd: Some("/repo".to_string()),
                    startup_directory: None,
                    cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
                    cli_command: Some("codex".to_string()),
                    cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                    conversation_ids: Vec::new(),
                    active_conversation_id: None,
                    cli_agent_session_id: Some("current-app-1".to_string()),
                    is_active: false,
                    is_pinned: false,
                    updated_at_unix_ms: None,
                });

            workspace.add_explicit_terminal_bootstrap_default_tab(None, ctx);
            let current_app_cached_ids = workspace
                .session_navigator_sessions(ctx)
                .into_iter()
                .map(|session| session.id)
                .collect::<Vec<_>>();
            assert!(
                current_app_cached_ids
                    .iter()
                    .any(|id| id == "current-app-session")
            );
            assert!(
                !current_app_cached_ids
                    .iter()
                    .any(|id| id == "environment-session")
            );

            workspace.activate_tab_internal(ssh_tab_index, ctx);

            let environment_cached_ids = workspace
                .session_navigator_sessions(ctx)
                .into_iter()
                .map(|session| session.id)
                .collect::<Vec<_>>();
            assert!(
                environment_cached_ids
                    .iter()
                    .any(|id| id == "environment-session")
            );
            assert!(
                !environment_cached_ids
                    .iter()
                    .any(|id| id == "current-app-session")
            );
        });
    });
}

#[test]
fn test_ssh_environment_restores_from_window_snapshot() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        let snapshot = workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            workspace.set_active_tab_environment(environment);
            workspace.snapshot(ctx.window_id(), false, ctx)
        });

        let restored = restored_workspace(&mut app, snapshot);
        restored.read(&app, |workspace, _| {
            assert_eq!(workspace.tab_count(), 1);
            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Ssh)
            );
            assert_eq!(
                workspace.tabs[workspace.active_tab_index()]
                    .environment
                    .as_ref()
                    .map(|environment| &environment.authority_key),
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| &environment.authority_key)
            );
            assert_eq!(
                workspace.tabs[workspace.active_tab_index()]
                    .environment
                    .as_ref()
                    .and_then(|environment| environment.active_workspace_root.as_deref()),
                Some("/root/project")
            );
        });
    });
}

#[test]
fn test_transferred_ssh_tab_keeps_environment() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let source = mock_workspace(&mut app);
        let transferred_tab = source.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            workspace.set_active_tab_environment(
                crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                    "dnyx216".to_string(),
                    &server,
                    Some("/root/project".to_string()),
                    EnvironmentLifecycleState::Connected,
                ),
            );
            workspace.add_explicit_terminal_bootstrap_default_tab(None, ctx);
            workspace
                .get_tab_transfer_info_for_attach(0, ctx)
                .expect("ssh tab should be transferable")
        });

        let target = transferred_tab_workspace(&mut app, false);
        target.update(&mut app, |workspace, ctx| {
            workspace.insert_transferred_tab_at_index(transferred_tab, 0, ctx);

            assert_eq!(
                workspace.tabs[workspace.active_tab_index()]
                    .environment
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Ssh)
            );
            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Ssh)
            );
            assert_eq!(
                workspace.tabs[workspace.active_tab_index()]
                    .environment
                    .as_ref()
                    .and_then(|environment| environment.active_workspace_root.as_deref()),
                Some("/root/project")
            );
        });
    });
}

#[test]
fn test_deleting_only_live_environment_session_keeps_environment_selected() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        let authority = "ssh:dnyx216".to_string();
        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            assert_eq!(environment.authority_key, authority);
            // Install a real EnvironmentRuntimePlaceholder pane via the production
            // restore path so the environment tab actually surfaces a live session.
            workspace.add_restored_environment_runtime_tab(
                environment,
                Some("root@dnyx216".to_string()),
                ctx,
            );
            let environment_tab_index = workspace.active_tab_index();

            // The initial empty mock tab already provides the neighboring current-app
            // tab, so the workspace holds exactly [current-app, environment].
            workspace.activate_tab_internal(environment_tab_index, ctx);

            let live_session = workspace
                .live_workspace_sessions(ctx)
                .into_iter()
                .find(|session| {
                    session.is_active
                        && session.environment_authority_key.as_deref() == Some(authority.as_str())
                })
                .expect("expected active environment live session");

            workspace.delete_workspace_session(
                &crate::workspace::action::WorkspaceSessionActionTarget::new(
                    live_session.id.clone(),
                    live_session.environment_authority_key.clone(),
                ),
                ctx,
            );
        });

        // delete_workspace_session closes the live tab synchronously, then the
        // spawned completion handler reselects / recreates the Environment tab
        // via ensure_environment_tab_for_authority. Yield so those callbacks run
        // before asserting the post-delete Environment selection.
        futures_lite::future::yield_now().await;

        workspace.update(&mut app, |workspace, _ctx| {
            assert_eq!(
                workspace.tab_count(),
                2,
                "after deleting the only live Environment session, the Environment tab must be recreated and remain selected alongside the current-app tab"
            );
            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| environment.authority_key.as_str()),
                Some(authority.as_str())
            );
            assert_eq!(
                workspace.tabs[workspace.active_tab_index()]
                    .environment
                    .as_ref()
                    .map(|environment| environment.authority_key.as_str()),
                Some(authority.as_str())
            );
        });
    });
}

#[test]
fn test_deleting_active_environment_session_reselects_same_environment_session() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9021);
            workspace.environment_runtimes.mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-delete-reselect-same-env.sock"),
            );
            workspace
                .environment_runtimes
                .mark_connected_session(session_id, HostId::new("test-host".to_string()));

            // Two tabs in the *same* Environment, so deleting the active session can
            // reselect a sibling session within that Environment. Each restored
            // runtime tab installs an EnvironmentRuntimePlaceholder pane and thus
            // surfaces its own live session under the shared authority.
            workspace.add_restored_environment_runtime_tab(
                environment.clone(),
                Some("root@dnyx216".to_string()),
                ctx,
            );
            workspace.add_restored_environment_runtime_tab(
                environment,
                Some("root@dnyx216".to_string()),
                ctx,
            );
            let first_environment_tab_index = workspace.active_tab_index();
            workspace.activate_tab_internal(first_environment_tab_index, ctx);

            let environment_live_sessions = workspace
                .live_workspace_sessions(ctx)
                .into_iter()
                .filter(|session| {
                    session.environment_authority_key.as_deref() == Some(authority.as_str())
                })
                .collect::<Vec<_>>();
            assert!(
                environment_live_sessions.len() >= 2,
                "test setup must have at least two live sessions in the same Environment; sessions={environment_live_sessions:#?}"
            );
            let deleted_session = environment_live_sessions
                .iter()
                .find(|session| session.is_active)
                .expect("expected active Environment live session")
                .clone();

            workspace.delete_workspace_session(
                &crate::workspace::action::WorkspaceSessionActionTarget::new(
                    deleted_session.id.clone(),
                    deleted_session.environment_authority_key.clone(),
                ),
                ctx,
            );

            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| environment.authority_key.as_str()),
                Some(authority.as_str()),
                "deleting the active row must not fall back to current-app/local when the same Environment still has another live session"
            );
            assert_eq!(
                workspace.tabs[workspace.active_tab_index()]
                    .environment
                    .as_ref()
                    .map(|environment| environment.authority_key.as_str()),
                Some(authority.as_str())
            );
            let active_environment_sessions = workspace
                .session_navigator_sessions(ctx)
                .into_iter()
                .filter(|session| {
                    session.is_active
                        && session.environment_authority_key.as_deref() == Some(authority.as_str())
                })
                .collect::<Vec<_>>();
            assert_eq!(
                active_environment_sessions.len(),
                1,
                "after deleting one active Environment session, exactly one sibling Environment session should become active; active_environment_sessions={active_environment_sessions:#?}"
            );
            assert_ne!(
                active_environment_sessions[0].id, deleted_session.id,
                "deleted session row must not remain active"
            );
        });
    });
}

#[test]
fn test_deleting_active_environment_session_does_not_jump_to_next_environment() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            let first_server = test_ssh_server_for_environment_tests();
            let first_environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &first_server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let first_authority = first_environment.authority_key.clone();
            let first_session_id = CoreSessionId::from(9121);
            workspace.environment_runtimes.mark_connecting(
                first_environment.clone(),
                first_session_id,
                PathBuf::from("/tmp/ashide-test-delete-stay-first-env.sock"),
            );
            workspace
                .environment_runtimes
                .mark_connected_session(first_session_id, HostId::new("first-host".to_string()));
            workspace.remember_environment_runtime_snapshot(first_environment.clone());
            workspace.set_active_tab_environment(first_environment);
            let first_environment_tab_index = workspace.active_tab_index();

            workspace.handle_action(
                &WorkspaceAction::AddTerminalTab {
                    hide_homepage: false,
                },
                ctx,
            );
            let first_environment_second_tab_index = workspace.active_tab_index();

            let mut second_server =
                warp_ssh_manager::SshServerInfo::new_default("ssh-config:dnyx217".to_string());
            second_server.host = "dnyx217".to_string();
            second_server.username = "root".to_string();
            let second_environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx217".to_string(),
                &second_server,
                Some("/root/other-project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let second_authority = second_environment.authority_key.clone();
            workspace.add_restored_environment_runtime_tab(
                second_environment,
                Some("root@dnyx217".to_string()),
                ctx,
            );
            let second_environment_tab_index = workspace
                .tab_index_for_environment_authority(&second_authority)
                .expect("test setup should create the neighboring Environment tab");
            assert_ne!(first_environment_tab_index, second_environment_tab_index);
            assert_ne!(
                first_environment_second_tab_index,
                second_environment_tab_index
            );

            workspace.activate_tab_internal(first_environment_second_tab_index, ctx);
            let deleted_session = workspace
                .live_workspace_sessions(ctx)
                .into_iter()
                .find(|session| {
                    session.is_active
                        && session.environment_authority_key.as_deref()
                            == Some(first_authority.as_str())
                })
                .expect("expected active session in the first Environment")
                .clone();

            workspace.delete_workspace_session(
                &crate::workspace::action::WorkspaceSessionActionTarget::new(
                    deleted_session.id.clone(),
                    deleted_session.environment_authority_key.clone(),
                ),
                ctx,
            );

            let active_authority = workspace.tabs[workspace.active_tab_index()]
                .environment
                .as_ref()
                .map(|environment| environment.authority_key.as_str());
            assert_eq!(
                active_authority,
                Some(first_authority.as_str()),
                "deleting the active session in one Environment must reselect a sibling session in that Environment before considering a neighboring Environment tab"
            );
            assert_ne!(
                active_authority,
                Some(second_authority.as_str()),
                "delete fallback must not jump to the next Environment while same-Environment live sessions remain"
            );
        });
    });
}

#[test]
fn test_workspace_session_active_detection_uses_focused_live_pane_when_row_is_stale() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            // Install a real EnvironmentRuntimePlaceholder pane (as the production
            // restore path does) so `live_workspace_sessions` actually surfaces an
            // environment session. Tagging the tab via `set_active_tab_environment`
            // alone leaves the active pane a plain terminal leaf with no placeholder,
            // so no environment live session is produced.
            workspace.add_restored_environment_runtime_tab(
                environment,
                Some("root@dnyx216".to_string()),
                ctx,
            );

            let mut stale_session_row = workspace
                .live_workspace_sessions(ctx)
                .into_iter()
                .find(|session| session.is_active)
                .expect("expected active live session");
            stale_session_row.is_active = false;

            assert!(
                workspace.workspace_session_is_active_selection(&stale_session_row, ctx),
                "delete/reselect should trust the focused live pane as a fallback when Session Navigator active row metadata is stale"
            );
        });
    });
}

#[test]
fn test_deleting_inactive_environment_session_keeps_current_app_selected() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            // Install a real EnvironmentRuntimePlaceholder pane via the production
            // restore path so the environment tab surfaces a session that can be
            // targeted for deletion.
            workspace.add_restored_environment_runtime_tab(
                environment,
                Some("root@dnyx216".to_string()),
                ctx,
            );

            let inactive_environment_session = workspace
                .live_workspace_sessions(ctx)
                .into_iter()
                .find(|session| {
                    session.environment_authority_key.as_deref() == Some(authority.as_str())
                })
                .expect("expected inactive-delete Environment session");

            workspace.add_explicit_terminal_bootstrap_default_tab(None, ctx);
            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Local),
                "test setup should make current-app/local active before deleting an inactive Environment session"
            );

            workspace.delete_workspace_session(
                &crate::workspace::action::WorkspaceSessionActionTarget::new(
                    inactive_environment_session.id.clone(),
                    inactive_environment_session
                        .environment_authority_key
                        .clone(),
                ),
                ctx,
            );

            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Local),
                "deleting an inactive Environment session must not steal focus from the current-app tab"
            );
            assert_eq!(
                workspace.tabs[workspace.active_tab_index()]
                    .environment
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Local)
            );
            assert!(
                workspace
                    .session_navigator_sessions(ctx)
                    .into_iter()
                    .all(|session| session.environment_authority_key.as_deref()
                        != Some(authority.as_str())
                        || !session.is_active),
                "inactive Environment delete must not leave or create an active row for the deleted Environment"
            );
        });
    });
}

#[test]
fn test_closing_active_environment_tab_switches_current_environment_to_current_app() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            workspace.set_active_tab_environment(
                crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                    "dnyx216".to_string(),
                    &server,
                    Some("/root/project".to_string()),
                    EnvironmentLifecycleState::Connected,
                ),
            );
            workspace.add_explicit_terminal_bootstrap_default_tab(None, ctx);
            workspace.activate_tab_internal(0, ctx);

            workspace.close_tab(0, true, false, ctx);

            assert_eq!(workspace.tab_count(), 1);
            assert_eq!(workspace.active_tab_index(), 0);
            assert_eq!(
                workspace.tabs[workspace.active_tab_index()]
                    .environment
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Local)
            );
            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Local)
            );
        });
    });
}

#[test]
fn test_closing_inactive_environment_tab_keeps_active_current_app_environment() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            workspace.set_active_tab_environment(
                crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                    "dnyx216".to_string(),
                    &server,
                    Some("/root/project".to_string()),
                    EnvironmentLifecycleState::Connected,
                ),
            );
            workspace.add_explicit_terminal_bootstrap_default_tab(None, ctx);
            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Local)
            );

            workspace.close_tab(0, true, false, ctx);

            assert_eq!(workspace.tab_count(), 1);
            assert_eq!(workspace.active_tab_index(), 0);
            assert_eq!(
                workspace.tabs[workspace.active_tab_index()]
                    .environment
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Local)
            );
            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Local)
            );
        });
    });
}

#[test]
fn test_close_other_tabs_preserves_target_environment_boundary() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            workspace.set_active_tab_environment(environment);
            let environment_tab_index = workspace.active_tab_index();

            workspace.add_explicit_terminal_bootstrap_default_tab(None, ctx);
            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Local),
                "test setup should make current-app/local active before close-other chooses the Environment tab"
            );

            workspace.close_other_tabs(environment_tab_index, true, ctx);

            assert_eq!(workspace.tab_count(), 1);
            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| environment.authority_key.as_str()),
                Some(authority.as_str()),
                "close other tabs on an Environment tab must activate that Environment instead of falling back to current-app/local"
            );
            assert_eq!(
                workspace.tabs[workspace.active_tab_index()]
                    .environment
                    .as_ref()
                    .map(|environment| environment.authority_key.as_str()),
                Some(authority.as_str())
            );
        });
    });
}

#[test]
fn test_close_other_tabs_preserves_target_current_app_boundary() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            workspace.set_active_tab_environment(
                crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                    "dnyx216".to_string(),
                    &server,
                    Some("/root/project".to_string()),
                    EnvironmentLifecycleState::Connected,
                ),
            );
            workspace.add_explicit_terminal_bootstrap_default_tab(None, ctx);
            let current_app_tab_index = workspace.active_tab_index();
            workspace.activate_tab_internal(0, ctx);
            assert_ne!(workspace.active_tab_index(), current_app_tab_index);

            workspace.close_other_tabs(current_app_tab_index, true, ctx);

            assert_eq!(workspace.tab_count(), 1);
            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Local),
                "close other tabs on a current-app tab must not keep stale Environment state"
            );
            assert_eq!(
                workspace.tabs[workspace.active_tab_index()]
                    .environment
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Local)
            );
        });
    });
}

#[test]
fn test_close_tabs_left_preserves_target_environment_boundary_when_active_is_removed() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            workspace.add_explicit_terminal_bootstrap_default_tab(None, ctx);

            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            workspace.set_active_tab_environment(environment);
            let environment_tab_index = workspace.active_tab_index();

            workspace.activate_tab_internal(0, ctx);
            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Local),
                "test setup should make a current-app tab left of the Environment active"
            );

            workspace.close_tabs_direction(environment_tab_index, TabMovement::Left, true, ctx);

            assert_eq!(workspace.tab_count(), 1);
            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| environment.authority_key.as_str()),
                Some(authority.as_str()),
                "closing tabs to the left of an Environment target must activate that Environment when the old active current-app tab is removed"
            );
            assert_eq!(
                workspace.tabs[workspace.active_tab_index()]
                    .environment
                    .as_ref()
                    .map(|environment| environment.authority_key.as_str()),
                Some(authority.as_str())
            );
        });
    });
}

#[test]
fn test_close_tabs_right_preserves_target_current_app_boundary_when_active_is_removed() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            workspace.add_explicit_terminal_bootstrap_default_tab(None, ctx);
            let current_app_tab_index = 0;

            let server = test_ssh_server_for_environment_tests();
            workspace.set_active_tab_environment(
                crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                    "dnyx216".to_string(),
                    &server,
                    Some("/root/project".to_string()),
                    EnvironmentLifecycleState::Connected,
                ),
            );
            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Ssh),
                "test setup should make an Environment tab right of the current-app target active"
            );

            workspace.close_tabs_direction(current_app_tab_index, TabMovement::Right, true, ctx);

            assert_eq!(workspace.tab_count(), 1);
            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Local),
                "closing tabs to the right of a current-app target must not leave stale Environment state when the old active Environment tab is removed"
            );
            assert_eq!(
                workspace.tabs[workspace.active_tab_index()]
                    .environment
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Local)
            );
        });
    });
}

#[test]
fn test_close_active_environment_pane_syncs_session_navigator_active_row() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        let runtime_session_id = CoreSessionId::from(9031);
        let pane_group = workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment =
                crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                    "dnyx216".to_string(),
                    &server,
                    Some("/root/project".to_string()),
                    EnvironmentLifecycleState::Connected,
                );
            workspace.environment_runtimes.mark_connecting(
                environment.clone(),
                runtime_session_id,
                PathBuf::from("/tmp/ashide-test-close-pane-runtime.sock"),
            );
            workspace.environment_runtimes.mark_connected_session(
                runtime_session_id,
                HostId::new("close-pane-runtime-host".to_string()),
            );
            workspace.set_active_tab_environment(environment);
            let pane_group = workspace.active_tab_pane_group().clone();
            pane_group.update(ctx, |panes, ctx| {
                let focused_pane_id = panes.focused_pane_id(ctx);
                panes
                    .replace_pane_with_terminal_options(
                        focused_pane_id,
                        test_environment_runtime_pty_options(runtime_session_id, ctx),
                        ctx,
                    )
                    .expect("initial Environment pane should be backed by runtime PTY");
            });
            workspace.sync_session_navigator_sessions(ctx);
            pane_group
        });

        let second_terminal_id = pane_group.update(&mut app, |panes, ctx| {
            let first_terminal_id = panes
                .focused_pane_id(ctx)
                .as_terminal_pane_id()
                .expect("initial Environment pane should be a terminal pane");
            panes.add_terminal_pane_with_options(
                Direction::Right,
                test_environment_runtime_pty_options(runtime_session_id, ctx),
                ctx,
            );
            get_newly_created_pane_id(panes, &[first_terminal_id.into()])
                .as_terminal_pane_id()
                .expect("new split pane should be a terminal pane")
        });

        let (authority, active_before_close) = workspace.read(&app, |workspace, ctx| {
            let authority = workspace
                .current_environment
                .as_ref()
                .expect("Environment should be active")
                .authority_key
                .clone();
            let active_sessions = workspace
                .session_navigator_sessions(ctx)
                .into_iter()
                .filter(|session| {
                    session.is_active
                        && session.environment_authority_key.as_deref() == Some(authority.as_str())
                })
                .collect::<Vec<_>>();
            assert_eq!(
                active_sessions.len(),
                1,
                "test setup should have exactly one active Environment row after split-pane focus"
            );
            (authority, active_sessions[0].id.clone())
        });

        pane_group.update(&mut app, |panes, ctx| {
            panes.close_pane(second_terminal_id.into(), ctx);
        });

        workspace.read(&app, |workspace, ctx| {
            let active_sessions = workspace
                .session_navigator_sessions(ctx)
                .into_iter()
                .filter(|session| {
                    session.is_active
                        && session.environment_authority_key.as_deref() == Some(authority.as_str())
                })
                .collect::<Vec<_>>();
            assert_eq!(
                active_sessions.len(),
                1,
                "pane-level close must resync Session Navigator to the remaining active Environment pane"
            );
            assert_ne!(
                active_sessions[0].id, active_before_close,
                "closed pane row must not remain active after pane-level close"
            );
        });
    });
}

#[test]
fn test_undo_close_environment_pane_restores_session_navigator_active_row() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        let runtime_session_id = CoreSessionId::from(9032);
        let pane_group = workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment =
                crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                    "dnyx216".to_string(),
                    &server,
                    Some("/root/project".to_string()),
                    EnvironmentLifecycleState::Connected,
                );
            workspace.environment_runtimes.mark_connecting(
                environment.clone(),
                runtime_session_id,
                PathBuf::from("/tmp/ashide-test-undo-pane-runtime.sock"),
            );
            workspace.environment_runtimes.mark_connected_session(
                runtime_session_id,
                HostId::new("undo-pane-runtime-host".to_string()),
            );
            workspace.set_active_tab_environment(environment);
            let pane_group = workspace.active_tab_pane_group().clone();
            pane_group.update(ctx, |panes, ctx| {
                let focused_pane_id = panes.focused_pane_id(ctx);
                panes
                    .replace_pane_with_terminal_options(
                        focused_pane_id,
                        test_environment_runtime_pty_options(runtime_session_id, ctx),
                        ctx,
                    )
                    .expect("initial Environment pane should be backed by runtime PTY");
            });
            workspace.sync_session_navigator_sessions(ctx);
            pane_group
        });

        let second_terminal_id = pane_group.update(&mut app, |panes, ctx| {
            let first_terminal_id = panes
                .focused_pane_id(ctx)
                .as_terminal_pane_id()
                .expect("initial Environment pane should be a terminal pane");
            panes.add_terminal_pane_with_options(
                Direction::Right,
                test_environment_runtime_pty_options(runtime_session_id, ctx),
                ctx,
            );
            get_newly_created_pane_id(panes, &[first_terminal_id.into()])
                .as_terminal_pane_id()
                .expect("new split pane should be a terminal pane")
        });

        let authority = workspace.read(&app, |workspace, ctx| {
            let authority = workspace
                .current_environment
                .as_ref()
                .expect("Environment should be active")
                .authority_key
                .clone();
            let active_session = workspace
                .session_navigator_sessions(ctx)
                .into_iter()
                .find(|session| {
                    session.is_active
                        && session.environment_authority_key.as_deref() == Some(authority.as_str())
                })
                .expect("split Environment pane should have an active session row");
            assert!(
                workspace.workspace_session_is_active_selection(&active_session, ctx),
                "test setup should mark the focused split Environment pane as active"
            );
            authority
        });

        pane_group.update(&mut app, |panes, ctx| {
            panes.close_pane(second_terminal_id.into(), ctx);
        });

        UndoCloseStack::handle(&app).update(&mut app, |stack, ctx| {
            stack.undo_close(ctx);
        });

        workspace.read(&app, |workspace, ctx| {
            let active_sessions = workspace
                .session_navigator_sessions(ctx)
                .into_iter()
                .filter(|session| {
                    session.is_active
                        && session.environment_authority_key.as_deref() == Some(authority.as_str())
                })
                .collect::<Vec<_>>();
            assert_eq!(
                active_sessions.len(),
                1,
                "undo-close pane must resync Session Navigator to the restored active Environment pane"
            );
            assert!(
                workspace.workspace_session_is_active_selection(&active_sessions[0], ctx),
                "restored Environment pane should regain the active row after undo close"
            );
        });
    });
}

#[test]
fn test_undo_close_environment_tab_restores_environment_boundary() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        let authority = workspace.update(&mut app, |workspace, ctx| {
            workspace.add_explicit_terminal_bootstrap_default_tab(None, ctx);

            let server = test_ssh_server_for_environment_tests();
            let environment =
                crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                    "dnyx216".to_string(),
                    &server,
                    Some("/root/project".to_string()),
                    EnvironmentLifecycleState::Connected,
                );
            let authority = environment.authority_key.clone();
            workspace.set_active_tab_environment(environment);
            let environment_tab_index = workspace.active_tab_index();

            workspace.close_tab(environment_tab_index, true, true, ctx);
            assert_eq!(
                workspace.current_environment.as_ref().map(|environment| &environment.kind),
                Some(&EnvironmentKind::Local),
                "closing the active Environment tab should temporarily fall back to current-app/local"
            );
            authority
        });

        UndoCloseStack::handle(&app).update(&mut app, |stack, ctx| {
            stack.undo_close(ctx);
        });

        workspace.read(&app, |workspace, _ctx| {
            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| environment.authority_key.as_str()),
                Some(authority.as_str()),
                "undo-close tab must reactivate the restored Environment, not leave current-app/local active"
            );
            assert_eq!(
                workspace.tabs[workspace.active_tab_index()]
                    .environment
                    .as_ref()
                    .map(|environment| environment.authority_key.as_str()),
                Some(authority.as_str()),
                "restored Environment tab should be the active tab after undo-close"
            );
        });
    });
}

#[test]
fn test_switch_to_current_app_environment_from_runtime_creates_current_app_tab() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();

            workspace.set_active_tab_environment(
                crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                    "dnyx216".to_string(),
                    &server,
                    Some("/root/project".to_string()),
                    EnvironmentLifecycleState::Connected,
                ),
            );
            let ssh_tab_index = workspace.active_tab_index();

            workspace.handle_action(
                &WorkspaceAction::SwitchEnvironment {
                    authority_key: "local".to_string(),
                },
                ctx,
            );

            assert_eq!(workspace.tab_count(), 2);
            assert_ne!(workspace.active_tab_index(), ssh_tab_index);
            assert_eq!(
                workspace.tabs[ssh_tab_index]
                    .environment
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Ssh)
            );
            assert_eq!(
                workspace.tabs[workspace.active_tab_index()]
                    .environment
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Local)
            );
        });
    });
}

#[test]
fn test_disconnect_only_runtime_environment_leaves_current_app_tab() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "dnyx216".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority_key = environment.authority_key.clone();
            workspace.set_active_tab_environment(environment);

            workspace.handle_action(
                &WorkspaceAction::DisconnectEnvironment { authority_key },
                ctx,
            );

            assert_eq!(workspace.tab_count(), 1);
            assert_eq!(
                workspace.tabs[workspace.active_tab_index()]
                    .environment
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Local)
            );
            assert_eq!(
                workspace
                    .current_environment
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Local)
            );
        });
    });
}

// ---------------------------------------------------------------------------
// Local/remote routing regression gates (method 7: machine-checked invariants)
// ---------------------------------------------------------------------------

/// Split-pane environment pane creation must go through `deliver_*_split*`
/// helpers (or documented exceptions), not ad-hoc inline apply. If this fails,
/// add a deliver helper or update the approved list with a matrix note.
#[test]
fn test_split_pane_add_terminal_pane_call_sites_are_audited() {
    const VIEW_RS: &str = include_str!("view.rs");
    let call_count = VIEW_RS.matches("add_terminal_pane_in_current_environment(").count();
    const APPROVED_CALL_SITES: usize = 6;
    assert_eq!(
        call_count,
        APPROVED_CALL_SITES,
        "unexpected add_terminal_pane_in_current_environment call site — route split-pane \
         capabilities through deliver_agent_pane_split / deliver_fork_split_pane / \
         deliver_startup_command_split_pane, or document exceptions in \
         docs/design/local-remote-capability-matrix.csv"
    );
}

/// EnvironmentBackendKind dispatch must remain the single routing layer for
/// capability entry points documented in the matrix. New call sites need a
/// matrix row + test.
#[test]
fn test_environment_backend_kind_dispatch_call_sites_are_audited() {
    const VIEW_RS: &str = include_str!("view.rs");
    let for_env_count = VIEW_RS.matches("EnvironmentBackendKind::for_environment").count();
    const APPROVED_FOR_ENVIRONMENT_DISPATCHES: usize = 9;
    assert_eq!(
        for_env_count,
        APPROVED_FOR_ENVIRONMENT_DISPATCHES,
        "unexpected EnvironmentBackendKind::for_environment dispatch — add a capability-matrix \
         row and routing test when introducing a new local/remote entry point"
    );
}
