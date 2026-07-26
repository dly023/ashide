use super::*;
use crate::ai::agent::conversation::AIConversation;
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
use repo_metadata::repositories::DetectedRepositories;
use repo_metadata::watcher::DirectoryWatcher;
#[cfg(feature = "local_fs")]
use repo_metadata::RepoMetadataModel;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use watcher::HomeDirectoryWatcher;

use crate::object_store::update_manager::UpdateManager;
use crate::server::experiments::ServerExperiments;

use crate::settings::{AutoupdateSettings, PrivacySettings};
use crate::settings_view::keybindings::KeybindingChangedNotifier;
use crate::settings_view::DisplayCount;
#[cfg(test)]
use crate::ssh_manager::catalog::SshTargetCatalogSnapshot;
use crate::system::SystemStats;
use crate::tab_configs::tab_config::{TabConfigPaneNode, TabConfigPaneType};
use crate::terminal::history::History;
use crate::terminal::keys::TerminalKeybindings;
use crate::terminal::model::terminal_model::ExitReason;
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
use crate::terminal::cli_agent_sessions::{
    CLIAgentInputState, CLIAgentSession, CLIAgentSessionContext, CLIAgentSessionStatus,
    CLIAgentSessionsModel,
};
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
    app.add_singleton_model(crate::ssh_manager::SshTargetCatalog::new);
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
        ensure_test_saved_ssh_server_fixture(conn)?;
        Ok(())
    });
}

fn ensure_test_saved_ssh_server_fixture(conn: &mut diesel::SqliteConnection) -> anyhow::Result<()> {
    use diesel::connection::SimpleConnection;

    conn.batch_execute(
        "INSERT INTO ssh_nodes (id, parent_id, kind, name, sort_order) \
         VALUES ('remote-fixture-primary', NULL, 'server', 'Remote Fixture Primary', 0) \
         ON CONFLICT(id) DO UPDATE SET \
           parent_id = NULL, kind = 'server', name = 'Remote Fixture Primary', sort_order = 0; \
         INSERT INTO ssh_servers (node_id, host, port, username, auth_type, key_path, startup_command, notes) \
         VALUES ('remote-fixture-primary', 'remote-fixture-primary', 22, 'root', 'password', NULL, NULL, NULL) \
         ON CONFLICT(node_id) DO UPDATE SET \
           host = 'remote-fixture-primary', port = 22, username = 'root', \
           auth_type = 'password', key_path = NULL, startup_command = NULL, notes = NULL;",
    )?;
    Ok(())
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
fn test_live_pane_rename_commits_on_enter_or_blur_and_escape_keeps_container_title() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            let pane_group = workspace
                .get_pane_group_view(0)
                .expect("mock workspace must have an active pane group");
            let locator = PaneViewLocator {
                pane_group_id: pane_group.id(),
                pane_id: pane_group.read(ctx, |pane_group, _| {
                    pane_group
                        .pane_id_by_index(0)
                        .expect("active pane group must have a pane")
                }),
            };
            let pane_title = |workspace: &Workspace, ctx: &AppContext| {
                workspace
                    .get_pane_group_view_with_id(locator.pane_group_id)
                    .expect("pane group must still be available")
                    .as_ref(ctx)
                    .pane_by_id(locator.pane_id)
                    .expect("pane must still be available")
                    .pane_configuration()
                    .as_ref(ctx)
                    .custom_vertical_tabs_title()
                    .map(str::to_owned)
            };

            workspace.rename_pane(locator, ctx);
            workspace.pane_rename_editor.update(ctx, |editor, ctx| {
                editor.insert_selected_text("  Enter title  ", ctx);
            });
            workspace.handle_pane_rename_editor_event(&Event::Enter, ctx);
            assert_eq!(pane_title(workspace, ctx).as_deref(), Some("Enter title"));

            workspace.rename_pane(locator, ctx);
            workspace.pane_rename_editor.update(ctx, |editor, ctx| {
                editor.insert_selected_text("Blur title", ctx);
            });
            workspace.handle_pane_rename_editor_event(&Event::Blurred, ctx);
            assert_eq!(pane_title(workspace, ctx).as_deref(), Some("Blur title"));

            workspace.rename_pane(locator, ctx);
            workspace.pane_rename_editor.update(ctx, |editor, ctx| {
                editor.insert_selected_text("Discarded title", ctx);
            });
            workspace.handle_pane_rename_editor_event(&Event::Escape, ctx);
            assert_eq!(pane_title(workspace, ctx).as_deref(), Some("Blur title"));
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
            container_uuid: vec![61; 16],
            session_binding: None,
            is_focused: true,
            custom_vertical_tabs_title: None,
            contents: LeafContents::GetStarted,
        }))),
        Arc::new(HashMap::<PaneUuid, Vec<SerializedBlockListItem>>::new()),
        None,
        ctx,
    );
}

#[test]
fn test_restored_runtime_placeholder_configuration_owns_leaf_session_binding() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let binding = crate::app_state::PaneSessionBinding {
                agent: Some("Antigravity".to_string()),
                command: Some("agy".to_string()),
                origin: Some(CliAgentSessionOrigin::CommandDetected),
                session_id: None,
                cwd: Some("/root/manga-review-platform".to_string()),
                source_identity_keys: vec![
                    "lr117-remote-sourceless".to_string(),
                    "ssh:ssh-config:remote-fixture-secondary::source:lr117-remote-sourceless"
                        .to_string(),
                ],
            };
            let container_uuid = vec![0x61, 0x72, 0x83, 0x94];
            workspace.add_tab_with_pane_layout(
                PanesLayout::Snapshot(Box::new(PaneNodeSnapshot::Leaf(LeafSnapshot {
                    container_uuid: container_uuid.clone(),
                    session_binding: Some(binding.clone()),
                    is_focused: true,
                    custom_vertical_tabs_title: Some("LR117 remote".to_string()),
                    contents: LeafContents::EnvironmentRuntimePlaceholder,
                }))),
                Arc::new(HashMap::<PaneUuid, Vec<SerializedBlockListItem>>::new()),
                None,
                ctx,
            );

            let pane_group = workspace.active_tab_pane_group();
            let pane_id = pane_group.as_ref(ctx).focused_pane_id(ctx);
            assert_eq!(
                pane_group
                    .as_ref(ctx)
                    .container_uuid_for_pane_id(pane_id, ctx),
                Some(container_uuid)
            );
            assert_eq!(
                pane_group
                    .as_ref(ctx)
                    .session_binding_for_pane_id(pane_id, ctx),
                Some(binding)
            );
        });
    });
}

#[test]
fn test_cold_restored_runtime_placeholder_materialization_preserves_pane_session_binding() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment =
                crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                    "lr117-cold-restore".to_owned(),
                    &server,
                    Some("/root/manga-review-platform".to_owned()),
                    EnvironmentLifecycleState::Connected,
                );
            let authority = environment.authority_key.clone();
            let runtime_session_id = CoreSessionId::from(9117);
            workspace.environments_mut().mark_connecting(
                environment.clone(),
                runtime_session_id,
                PathBuf::from("/tmp/ashide-test-lr117-cold-restore.sock"),
            );
            workspace.environments_mut().mark_connected_session(
                runtime_session_id,
                HostId::new("lr117-cold-restore-host".to_owned()),
            );

            let restored = WorkspaceSessionSnapshot {
                id: "lr117-remote-sourceless".to_owned(),
                container_uuid: None,
                kind: WorkspaceSessionKind::AgentTerminal,
                label: Some("LR117 remote Antigravity".to_owned()),
                environment_authority_key: Some(authority.clone()),
                cwd: Some("/root/manga-review-platform".to_owned()),
                startup_directory: None,
                cli_agent: Some("Antigravity".to_owned()),
                cli_command: Some("agy".to_owned()),
                cli_agent_origin: Some(CliAgentSessionOrigin::CommandDetected),
                conversation_ids: Vec::new(),
                active_conversation_id: None,
                cli_agent_session_id: None,
                is_active: false,
                is_pinned: false,
                updated_at_unix_ms: Some(117),
                is_live_container: false,
            };
            let binding = crate::app_state::PaneSessionBinding::from_workspace_session(&restored)
                .expect("sourceless provider metadata must still form a stable pane binding");
            let container_uuid = vec![0x11, 0x72, 0x83, 0x94];
            let persisted_terminal_tree = PaneNodeSnapshot::Leaf(LeafSnapshot {
                container_uuid: container_uuid.clone(),
                session_binding: Some(binding.clone()),
                is_focused: true,
                custom_vertical_tabs_title: Some("LR117 cold remote".to_owned()),
                contents: LeafContents::Terminal(TerminalPaneSnapshot {
                    uuid: vec![0x11; 16],
                    cwd: restored.cwd.clone(),
                    shell_launch_data: None,
                    is_active: true,
                    is_read_only: false,
                    input_config: None,
                    llm_model_override: None,
                    active_profile_id: None,
                    conversation_ids_to_restore: Vec::new(),
                    active_conversation_id: None,
                }),
            });
            let cold_restore_tree = persisted_terminal_tree.into_environment_runtime_restore_tree();

            workspace.add_tab_with_pane_layout_in_environment(
                PanesLayout::Snapshot(Box::new(cold_restore_tree)),
                Arc::new(HashMap::<PaneUuid, Vec<SerializedBlockListItem>>::new()),
                None,
                environment,
                None,
                ctx,
            );

            let pane_group = workspace.active_tab_pane_group().clone();
            let placeholder_pane_id = pane_group.as_ref(ctx).focused_pane_id(ctx);
            assert_eq!(
                pane_group
                    .as_ref(ctx)
                    .container_uuid_for_pane_id(placeholder_pane_id, ctx),
                Some(container_uuid.clone())
            );
            assert_eq!(
                pane_group
                    .as_ref(ctx)
                    .session_binding_for_pane_id(placeholder_pane_id, ctx),
                Some(binding.clone())
            );

            workspace.queue_environment_runtime_intent(
                &authority,
                EnvironmentEntryIntent::SessionRestore(SessionRestoreEntry {
                    session: restored,
                    resume_command: None,
                }),
                placeholder_pane_id,
            );
            let materialized = workspace.materialize_environment_runtime_terminal(
                &authority,
                test_environment_runtime_pty_options(runtime_session_id, ctx),
                placeholder_pane_id,
                ctx,
            );
            let terminal_pane_id = materialized
                .terminal_pane_id
                .expect("cold-restored placeholder must materialize its runtime terminal");

            assert_eq!(
                pane_group
                    .as_ref(ctx)
                    .container_uuid_for_pane_id(terminal_pane_id, ctx),
                Some(container_uuid)
            );
            assert_eq!(
                pane_group
                    .as_ref(ctx)
                    .session_binding_for_pane_id(terminal_pane_id, ctx),
                Some(binding.clone()),
                "carrier replacement must preserve the pane-owned provider binding"
            );

            workspace.complete_environment_runtime_terminal_materialization(
                &pane_group,
                terminal_pane_id,
                ctx,
            );
            assert_eq!(
                pane_group
                    .as_ref(ctx)
                    .session_binding_for_pane_id(terminal_pane_id, ctx),
                Some(binding),
                "materialization completion must not detach the cold-restored binding"
            );
            assert!(
                workspace
                    .environments_mut()
                    .pending_materialization_for_pane(&authority, terminal_pane_id)
                    .is_none(),
                "completed materialization must clear its pane-owned lifecycle state"
            );
        });
    });
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
fn test_unified_new_session_menu_lists_each_coding_agent_directly() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            crate::terminal::cli_agent::CLIAgentInstallModel::handle(ctx).update(
                ctx,
                |model, _| {
                    model.set_installed_agents_for_test([
                        CLIAgent::Claude,
                        CLIAgent::Codex,
                        CLIAgent::Jcode,
                        CLIAgent::Omp,
                    ]);
                },
            );
            crate::settings::AISettings::handle(ctx).update(ctx, |settings, ctx| {
                report_if_error!(settings.cli_agent_history_enabled_agents.set_value(
                    vec![
                        CLIAgent::Claude.to_serialized_name(),
                        CLIAgent::Codex.to_serialized_name(),
                        CLIAgent::Jcode.to_serialized_name(),
                        CLIAgent::Omp.to_serialized_name(),
                    ],
                    ctx,
                ));
            });

            let menu_items = workspace.unified_new_session_menu_items(ctx);
            let labels = menu_items
                .iter()
                .map(new_session_menu_label)
                .collect::<Vec<_>>();

            assert!(!labels
                .iter()
                .any(|label| *label == crate::t!("workspace-coding-agent-actions")));
            for agent in [
                CLIAgent::Claude,
                CLIAgent::Codex,
                CLIAgent::Jcode,
                CLIAgent::Omp,
            ] {
                let expected_label = format!("New {} session", agent.display_name());
                assert!(labels.iter().any(|label| label == &expected_label));
            }

            let direct_agents = menu_items
                .iter()
                .filter_map(|item| match item.item_on_select_action() {
                    Some(WorkspaceAction::AddSpecificAgentTab(agent)) => Some(*agent),
                    Some(_) | None => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(
                direct_agents,
                vec![
                    CLIAgent::Claude,
                    CLIAgent::Codex,
                    CLIAgent::Jcode,
                    CLIAgent::Omp
                ]
            );
        });
    });
}

#[test]
fn test_unified_new_session_menu_projects_history_discovery_selection() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            crate::terminal::cli_agent::CLIAgentInstallModel::handle(ctx).update(
                ctx,
                |model, _| {
                    model.set_installed_agents_for_test([
                        CLIAgent::Claude,
                        CLIAgent::Codex,
                        CLIAgent::Jcode,
                        CLIAgent::Omp,
                    ]);
                },
            );

            for (enabled, expected_direct_agents, expects_domain) in [
                (Vec::new(), Vec::new(), false),
                (vec![CLIAgent::Jcode], vec![CLIAgent::Jcode], false),
                (
                    vec![CLIAgent::Claude, CLIAgent::Omp],
                    vec![CLIAgent::Claude, CLIAgent::Omp],
                    false,
                ),
                (
                    vec![CLIAgent::Claude, CLIAgent::Codex, CLIAgent::Jcode],
                    vec![CLIAgent::Claude, CLIAgent::Codex, CLIAgent::Jcode],
                    false,
                ),
            ] {
                crate::settings::AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings.cli_agent_history_enabled_agents.set_value(
                        enabled.iter().map(CLIAgent::to_serialized_name).collect(),
                        ctx,
                    ));
                });

                let menu_items = workspace.unified_new_session_menu_items(ctx);
                let direct_agents = menu_items
                    .iter()
                    .filter_map(|item| match item.item_on_select_action() {
                        Some(WorkspaceAction::AddSpecificAgentTab(agent)) => Some(*agent),
                        Some(_) | None => None,
                    })
                    .collect::<Vec<_>>();
                let labels = menu_items
                    .iter()
                    .map(new_session_menu_label)
                    .collect::<Vec<_>>();

                assert_eq!(direct_agents, expected_direct_agents);
                assert_eq!(
                    labels
                        .iter()
                        .filter(|label| **label == crate::t!("workspace-coding-agent-actions"))
                        .count(),
                    usize::from(expects_domain),
                );
            }
        });
    });
}

#[test]
fn test_unified_new_session_menu_has_one_coding_agent_domain_not_provider_rows() {
    // LR-172 (AGENT-ACTION-SIDECAR-IA-76): 当多个 coding agent 折叠成领域入口时,
    // 统一 New Session 菜单必须呈现「唯一一个」coding-agent 领域行,而不是每个
    // provider 各占一行,也不是 provider × action 的笛卡尔菜单。
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            crate::terminal::cli_agent::CLIAgentInstallModel::handle(ctx).update(
                ctx,
                |model, _| {
                    model.set_installed_agents_for_test([
                        CLIAgent::Claude,
                        CLIAgent::Codex,
                        CLIAgent::Jcode,
                        CLIAgent::Omp,
                    ]);
                },
            );
            crate::settings::AISettings::handle(ctx).update(ctx, |settings, ctx| {
                report_if_error!(settings.cli_agent_history_enabled_agents.set_value(
                    vec![
                        CLIAgent::Claude.to_serialized_name(),
                        CLIAgent::Codex.to_serialized_name(),
                        CLIAgent::Jcode.to_serialized_name(),
                        CLIAgent::Omp.to_serialized_name(),
                    ],
                    ctx,
                ));
            });

            let menu_items = workspace.unified_new_session_menu_items(ctx);

            // LR-172 契约:coding agent 以「每个 agent 一行直连创建」呈现,而不是
            // provider × action 的笛卡尔菜单。每个启用 agent 恰好产生一个
            // AddSpecificAgentTab 顶层行,顺序与启用顺序一致。
            let direct_agents = menu_items
                .iter()
                .filter_map(|item| match item.item_on_select_action() {
                    Some(WorkspaceAction::AddSpecificAgentTab(agent)) => Some(*agent),
                    Some(_) | None => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(
                direct_agents,
                vec![
                    CLIAgent::Claude,
                    CLIAgent::Codex,
                    CLIAgent::Jcode,
                    CLIAgent::Omp
                ],
                "each enabled coding agent must contribute exactly one direct row, not a provider×action grid"
            );

            // 不得出现 provider 重复行:同一 agent 只能有一个顶层直连入口。
            let mut seen = std::collections::HashSet::new();
            for agent in &direct_agents {
                assert!(
                    seen.insert(*agent),
                    "coding agent {agent:?} appears in more than one top-level row"
                );
            }
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
            crate::terminal::cli_agent::CLIAgentInstallModel::handle(ctx).update(
                ctx,
                |model, _| {
                    model.set_installed_agents_for_test([CLIAgent::Claude, CLIAgent::Codex]);
                },
            );
            let active_conversation_id = AIConversationId::new();
            let older_conversation_id = AIConversationId::new();
            let session = WorkspaceSessionSnapshot {
                id: "session-bridge-ai-session".to_string(),
                container_uuid: None,
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
                is_live_container: false,
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
                    crate::t!("workspace-agent-actions"),
                    crate::t!("workspace-session-bridge-fork-to-target", target = "Ashide"),
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
            assert!(matches!(
                workspace.agent_action_sidecar_source,
                Some(AgentActionSidecarSource::SessionBridge(
                    SessionBridgeActionSource::Conversation { conversation_id, .. }
                )) if conversation_id == active_conversation_id
            ));
            let domain_index = menu_items
                .iter()
                .position(|label| *label == crate::t!("workspace-agent-actions"))
                .expect("Agent actions domain must exist");
            workspace.tab_right_click_menu.update(ctx, |menu, ctx| {
                menu.set_selected_by_index(domain_index, ctx);
            });
            let expected = vec![
                (CLIAgent::Claude, AgentActionIntent::Fork),
                (CLIAgent::Claude, AgentActionIntent::Edit),
                (CLIAgent::Codex, AgentActionIntent::Fork),
                (CLIAgent::Codex, AgentActionIntent::Edit),
            ];
            for event in [MenuEvent::ItemHovered, MenuEvent::ItemSelected] {
                workspace.handle_tab_right_click_menu_event(&event, ctx);
                let projected = workspace.new_session_sidecar_menu.read(ctx, |menu, _| {
                    menu.items()
                        .iter()
                        .filter_map(|item| match item.item_on_select_action() {
                            Some(NewSessionSidecarSelection::AgentAction {
                                agent,
                                intent,
                                source: AgentActionSidecarSource::SessionBridge(
                                    SessionBridgeActionSource::Conversation {
                                        conversation_id,
                                        ..
                                    },
                                ),
                            }) if *conversation_id == active_conversation_id => {
                                Some((*agent, *intent))
                            }
                            Some(NewSessionSidecarSelection::AgentAction { .. }) | None => None,
                        })
                        .collect::<Vec<_>>()
                });
                assert_eq!(projected, expected);
            }
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
            crate::terminal::cli_agent::CLIAgentInstallModel::handle(ctx).update(
                ctx,
                |model, _| model.set_installed_agents_for_test([CLIAgent::Claude]),
            );
            let server = test_ssh_server_for_environment_tests();
            let environment =
                crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                    "remote-fixture-primary".to_string(),
                    &server,
                    Some("/root/project".to_string()),
                    EnvironmentLifecycleState::Connected,
                );
            let authority = environment.authority_key.clone();
            workspace.set_active_tab_environment(environment.clone());
            let runtime_session_id = CoreSessionId::from(9172);
            workspace.mark_environment_runtime_connecting(
                environment,
                runtime_session_id,
                PathBuf::from("/tmp/lr172-remote-agent-actions.sock"),
            );
            workspace
                .mark_environment_runtime_connected_session(
                    runtime_session_id,
                    HostId::new("lr172-remote-agent-actions-host".to_string()),
                )
                .expect("remote Agent actions fixture must be canonically connected");

            let conversation_id = AIConversationId::new();
            let session = WorkspaceSessionSnapshot {
                id: "environment-session-bridge-ai-session".to_string(),
                container_uuid: None,
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
                is_live_container: false,
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
                        fork_target: SessionBridgeForkTarget::Ashide,
                    } if *action_conversation_id == conversation_id
                        && source_environment_authority_key.as_deref() == Some(authority.as_str())
                )),
                "Environment Session Navigator rows must keep Ashide fork with the owning authority"
            );
            let source = workspace
                .agent_action_sidecar_source
                .clone()
                .expect("remote conversation must publish one Agent actions source");
            assert!(matches!(
                &source,
                AgentActionSidecarSource::SessionBridge(
                    SessionBridgeActionSource::Conversation {
                        conversation_id: action_conversation_id,
                        source_environment_authority_key,
                    }
                ) if *action_conversation_id == conversation_id
                    && source_environment_authority_key.as_deref() == Some(authority.as_str())
            ));
            let projected = workspace
                .agent_action_sidecar_items(source, ctx)
                .iter()
                .filter_map(|item| match item.item_on_select_action() {
                    Some(NewSessionSidecarSelection::AgentAction {
                        agent: CLIAgent::Claude,
                        intent,
                        ..
                    }) => Some(*intent),
                    Some(NewSessionSidecarSelection::AgentAction { .. }) | None => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(
                projected,
                vec![AgentActionIntent::Fork, AgentActionIntent::Edit]
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
                &menu_labels[..4],
                &[
                    crate::t!("workspace-agent-actions"),
                    crate::t!("workspace-session-bridge-fork-to-target", target = "Ashide"),
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
            assert!(matches!(
                workspace.agent_action_sidecar_source,
                Some(AgentActionSidecarSource::SessionBridge(
                    SessionBridgeActionSource::ActivePane { locator: action_locator }
                )) if action_locator == locator
            ));
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
            workspace.sync_session_navigator_sessions(ctx);

            let expected_conversation_id = conversation_id.to_string();
            let session = workspace
                .session_navigator_sessions()
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
                &menu_labels[..4],
                &[
                    crate::t!("workspace-agent-actions"),
                    crate::t!("workspace-session-bridge-fork-to-target", target = "Ashide"),
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
                container_uuid: None,
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
                is_live_container: false,
            };
            let target = WorkspaceSessionActionTarget::new(
                session.id.clone(),
                session.environment_authority_key.clone(),
            );
            workspace.restored_workspace_sessions.push(session);
            workspace.sync_session_navigator_sessions(ctx);
            let user_state_before_resume = workspace.workspace_session_user_state_for_authority(
                &workspace.current_environment_authority_key(ctx),
            );

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
            assert_eq!(
                restored_session.session_context.title_like_text(),
                Some("Codex history target".to_string()),
                "restoring a CLI-agent session must carry the indexed/restored title into the live tab title fallback"
            );

            let user_state = workspace.workspace_session_user_state_for_authority(
                &workspace.current_environment_authority_key(ctx),
            );
            assert_eq!(
                user_state.pinned, user_state_before_resume.pinned,
                "resuming a session must preserve the pre-existing pin set instead of persisting a pin mutation"
            );
            let resumed_rows = workspace
                .session_navigator_sessions()
                .into_iter()
                .filter(|session| {
                    session.cli_agent_session_id.as_deref() == Some("codex-history-switch-target")
                })
                .collect::<Vec<_>>();
            assert!(
                resumed_rows.iter().all(|session| !session.is_pinned),
                "resuming a session must not make the materialized live row appear pinned"
            );
        });
    });
}

#[test]
fn test_cross_window_activation_reuses_existing_durable_session_owner() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let owner_workspace = mock_workspace(&mut app);
        let contender_workspace = mock_workspace(&mut app);
        let provider_session_id = "cross-window-codex-owner";

        let (owner_window_id, durable_identity_key) =
            owner_workspace.update(&mut app, |workspace, ctx| {
                let terminal_view_id = workspace
                    .active_tab_pane_group()
                    .as_ref(ctx)
                    .active_session_view(ctx)
                    .expect("owner workspace should start with a terminal")
                    .id();
                CLIAgentSessionsModel::handle(ctx).update(ctx, |sessions, ctx| {
                    sessions.set_session(
                        terminal_view_id,
                        CLIAgentSession {
                            agent: CLIAgent::Codex,
                            status: CLIAgentSessionStatus::InProgress,
                            session_context: CLIAgentSessionContext {
                                session_id: Some(provider_session_id.to_string()),
                                ..Default::default()
                            },
                            input_state: CLIAgentInputState::Closed,
                            should_auto_toggle_input: false,
                            listener: None,
                            plugin_version: None,
                            environment_host_key: None,
                            draft_text: None,
                            custom_command_prefix: None,
                        },
                        ctx,
                    );
                });
                assert!(workspace.refresh_terminal_pane_session_binding(terminal_view_id, ctx));

                let session = workspace
                    .live_workspace_sessions(ctx)
                    .into_iter()
                    .find(|session| {
                        session.cli_agent_session_id.as_deref() == Some(provider_session_id)
                    })
                    .expect("registered CLI agent session should enrich the live pane");
                (
                    ctx.window_id(),
                    session
                        .durable_identity_key()
                        .expect("provider session must expose a durable identity"),
                )
            });

        contender_workspace.update(&mut app, |workspace, ctx| {
            let session = WorkspaceSessionSnapshot {
                id: "cross-window-codex-restore".to_string(),
                container_uuid: None,
                kind: WorkspaceSessionKind::AgentTerminal,
                label: Some("Cross-window Codex restore".to_string()),
                environment_authority_key: None,
                cwd: Some("/Users/admin/ashide".to_string()),
                startup_directory: None,
                cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
                cli_command: Some("codex".to_string()),
                cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                conversation_ids: Vec::new(),
                active_conversation_id: None,
                cli_agent_session_id: Some(provider_session_id.to_string()),
                is_active: false,
                is_pinned: false,
                updated_at_unix_ms: None,
                is_live_container: false,
            };
            assert_eq!(session.durable_identity_key().as_deref(), Some(durable_identity_key.as_str()));
            let target = WorkspaceSessionActionTarget::new(
                session.id.clone(),
                session.environment_authority_key.clone(),
            );
            workspace.restored_workspace_sessions.push(session);
            workspace.sync_session_navigator_sessions(ctx);
            let contender_tab_count = workspace.tab_count();
            let owner = WorkspaceRegistry::as_ref(ctx)
                .other_workspace_session_owner(
                    ctx.window_id(),
                    &durable_identity_key,
                    ctx,
                )
                .expect("the first window must be the app-wide durable owner");
            assert_eq!(owner.window_id, owner_window_id);

            workspace.activate_restored_workspace_session(&target, ctx);

            assert_eq!(
                workspace.tab_count(),
                contender_tab_count,
                "a second window must focus the existing durable owner instead of spawning another resume terminal"
            );
        });
    });
}

#[test]
fn test_closed_window_tracks_every_duplicate_durable_terminal_owner_until_exit() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);
        let provider_session_id = "019f5f34-b6b7-70b3-8e50-e98504691ca8";

        let (window_id, durable_identity_key, owners) =
            workspace.update(&mut app, |workspace, ctx| {
                let mut owners = Vec::new();
                for index in 0..2 {
                    if index > 0 {
                        workspace.add_terminal_tab(false, ctx);
                    }
                    let pane_group = workspace.active_tab_pane_group().clone();
                    let terminal_view = pane_group
                        .as_ref(ctx)
                        .active_session_view(ctx)
                        .expect("each owner tab must expose a terminal");
                    let terminal_manager = pane_group
                        .as_ref(ctx)
                        .terminal_manager(0, ctx)
                        .expect("each owner tab must expose a terminal manager");
                    CLIAgentSessionsModel::handle(ctx).update(ctx, |sessions, ctx| {
                        sessions.set_session(
                            terminal_view.id(),
                            CLIAgentSession {
                                agent: CLIAgent::Codex,
                                status: CLIAgentSessionStatus::InProgress,
                                session_context: CLIAgentSessionContext {
                                    session_id: Some(provider_session_id.to_owned()),
                                    ..Default::default()
                                },
                                input_state: CLIAgentInputState::Closed,
                                should_auto_toggle_input: false,
                                listener: None,
                                plugin_version: None,
                                environment_host_key: None,
                                draft_text: None,
                                custom_command_prefix: None,
                            },
                            ctx,
                        );
                    });
                    assert!(
                        workspace.refresh_terminal_pane_session_binding(terminal_view.id(), ctx)
                    );
                    owners.push((terminal_view.as_ref(ctx).model.clone(), terminal_manager));
                }

                let durable_identity_key = workspace
                    .live_workspace_sessions(ctx)
                    .into_iter()
                    .find_map(|session| {
                        (session.cli_agent_session_id.as_deref() == Some(provider_session_id))
                            .then(|| session.durable_identity_key())
                            .flatten()
                    })
                    .expect("duplicate provider sessions must expose one durable key");

                workspace.on_window_closed(ctx);
                assert!(WorkspaceRegistry::handle(ctx).update(ctx, |registry, _| {
                    registry.is_session_owner_retiring(&durable_identity_key)
                }));

                (ctx.window_id(), durable_identity_key, owners)
            });

        WorkspaceRegistry::handle(&app).update(&mut app, |registry, ctx| {
            registry.shutdown_retiring_session_owners_for_window(window_id, ctx);
        });
        workspace.update(&mut app, |_, ctx| {
            for (_, terminal_manager) in &owners {
                assert!(terminal_manager
                    .as_ref(ctx)
                    .as_any()
                    .downcast_ref::<crate::terminal::local_tty::TerminalManager>()
                    .expect("mock workspace uses local_tty::TerminalManager")
                    .shutdown_requested());
            }

            owners[0].0.lock().exit(ExitReason::PtyDisconnected);
            assert!(WorkspaceRegistry::handle(ctx).update(ctx, |registry, _| {
                registry.is_session_owner_retiring(&durable_identity_key)
            }));
            owners[1].0.lock().exit(ExitReason::PtyDisconnected);
            assert!(!WorkspaceRegistry::handle(ctx).update(ctx, |registry, _| {
                registry.is_session_owner_retiring(&durable_identity_key)
            }));
        });
    });
}

#[test]
fn test_transferred_tab_placeholder_is_permanently_shutdown_on_adoption() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let source_workspace = mock_workspace(&mut app);
        let transferred_pane_group = source_workspace.update(&mut app, |workspace, _| {
            workspace.active_tab_pane_group().clone()
        });
        let target_workspace = transferred_tab_workspace(&mut app, false);

        target_workspace.update(&mut app, |workspace, ctx| {
            let placeholder_terminal_manager = workspace
                .active_tab_pane_group()
                .as_ref(ctx)
                .terminal_manager(0, ctx)
                .expect("transferred workspace placeholder must expose a terminal manager");

            workspace.adopt_transferred_pane_group(transferred_pane_group, ctx);

            assert!(placeholder_terminal_manager
                .as_ref(ctx)
                .as_any()
                .downcast_ref::<crate::terminal::local_tty::TerminalManager>()
                .expect("transferred placeholder uses local_tty::TerminalManager")
                .shutdown_requested());
        });
    });
}

#[test]
fn test_closed_window_retains_durable_session_ownership_until_terminal_exit() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let owner_workspace = mock_workspace(&mut app);
        let contender_workspace = mock_workspace(&mut app);
        let provider_session_id = "019f5f34-b6b7-70b3-8e50-e98504691ca7";

        let (durable_identity_key, terminal_model) =
            owner_workspace.update(&mut app, |workspace, ctx| {
                let terminal_view = workspace
                    .active_tab_pane_group()
                    .as_ref(ctx)
                    .active_session_view(ctx)
                    .expect("owner workspace should start with a terminal");
                CLIAgentSessionsModel::handle(ctx).update(ctx, |sessions, ctx| {
                    sessions.set_session(
                        terminal_view.id(),
                        CLIAgentSession {
                            agent: CLIAgent::Codex,
                            status: CLIAgentSessionStatus::InProgress,
                            session_context: CLIAgentSessionContext {
                                session_id: Some(provider_session_id.to_string()),
                                ..Default::default()
                            },
                            input_state: CLIAgentInputState::Closed,
                            should_auto_toggle_input: false,
                            listener: None,
                            plugin_version: None,
                            environment_host_key: None,
                            draft_text: None,
                            custom_command_prefix: None,
                        },
                        ctx,
                    );
                });
                assert!(workspace.refresh_terminal_pane_session_binding(terminal_view.id(), ctx));
                let durable_identity_key = workspace
                    .live_workspace_sessions(ctx)
                    .into_iter()
                    .find(|session| {
                        session.cli_agent_session_id.as_deref() == Some(provider_session_id)
                    })
                    .and_then(|session| session.durable_identity_key())
                    .expect("registered CLI agent session should expose durable identity");
                let terminal_model = terminal_view.as_ref(ctx).model.clone();

                workspace.on_window_closed(ctx);

                assert!(WorkspaceRegistry::handle(ctx).update(ctx, |registry, _| {
                    registry.is_session_owner_retiring(&durable_identity_key)
                }));
                (durable_identity_key, terminal_model)
            });

        contender_workspace.update(&mut app, |workspace, ctx| {
            let session = WorkspaceSessionSnapshot {
                id: "closed-window-codex-restore".to_string(),
                container_uuid: None,
                kind: WorkspaceSessionKind::AgentTerminal,
                label: Some("Closing Codex restore".to_string()),
                environment_authority_key: None,
                cwd: Some("/Users/admin/manga_data".to_string()),
                startup_directory: None,
                cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
                cli_command: Some("codex".to_string()),
                cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                conversation_ids: Vec::new(),
                active_conversation_id: None,
                cli_agent_session_id: Some(provider_session_id.to_string()),
                is_active: false,
                is_pinned: false,
                updated_at_unix_ms: None,
                is_live_container: false,
            };
            assert_eq!(
                session.durable_identity_key().as_deref(),
                Some(durable_identity_key.as_str())
            );
            let target = WorkspaceSessionActionTarget::new(
                session.id.clone(),
                session.environment_authority_key.clone(),
            );
            workspace.restored_workspace_sessions.push(session);
            workspace.sync_session_navigator_sessions(ctx);
            let tab_count = workspace.tab_count();

            workspace.activate_restored_workspace_session(&target, ctx);

            assert_eq!(
                workspace.tab_count(),
                tab_count,
                "a retiring process owner must block a second resume after its window disappears"
            );
        });

        owner_workspace.update(&mut app, |workspace, ctx| {
            workspace.handle_reopen(ctx);
            assert!(!WorkspaceRegistry::handle(ctx).update(ctx, |registry, _| {
                registry.is_session_owner_retiring(&durable_identity_key)
            }));
            workspace.on_window_closed(ctx);
            assert!(WorkspaceRegistry::handle(ctx).update(ctx, |registry, _| {
                registry.is_session_owner_retiring(&durable_identity_key)
            }));
        });

        terminal_model.lock().exit(ExitReason::PtyDisconnected);
        contender_workspace.update(&mut app, |_, ctx| {
            assert!(!WorkspaceRegistry::handle(ctx).update(ctx, |registry, _| {
                registry.is_session_owner_retiring(&durable_identity_key)
            }));
        });
    });
}

#[test]
fn test_closed_nonfinal_tab_retains_durable_owner_until_terminal_exit() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);
        let provider_session_id = "019f5f34-b6b7-70b3-8e50-e98504691cb1";

        workspace.update(&mut app, |workspace, ctx| {
            let terminal_view = workspace
                .active_tab_pane_group()
                .as_ref(ctx)
                .active_session_view(ctx)
                .expect("owner tab must start with a terminal");
            CLIAgentSessionsModel::handle(ctx).update(ctx, |sessions, ctx| {
                sessions.set_session(
                    terminal_view.id(),
                    CLIAgentSession {
                        agent: CLIAgent::Codex,
                        status: CLIAgentSessionStatus::InProgress,
                        session_context: CLIAgentSessionContext {
                            session_id: Some(provider_session_id.to_owned()),
                            ..Default::default()
                        },
                        input_state: CLIAgentInputState::Closed,
                        should_auto_toggle_input: false,
                        listener: None,
                        plugin_version: None,
                        environment_host_key: None,
                        draft_text: None,
                        custom_command_prefix: None,
                    },
                    ctx,
                );
            });
            assert!(workspace.refresh_terminal_pane_session_binding(terminal_view.id(), ctx));
            let durable_identity_key = workspace
                .live_workspace_sessions(ctx)
                .into_iter()
                .find(|session| {
                    session.cli_agent_session_id.as_deref() == Some(provider_session_id)
                })
                .and_then(|session| session.durable_identity_key())
                .expect("registered tab must expose a durable owner");
            let terminal_model = terminal_view.as_ref(ctx).model.clone();

            workspace.add_welcome_tab(ctx);
            assert!(workspace.remove_tab(0, true, true, ctx));
            assert!(WorkspaceRegistry::handle(ctx).update(ctx, |registry, _| {
                registry.is_session_owner_retiring(&durable_identity_key)
            }));

            terminal_model.lock().exit(ExitReason::PtyDisconnected);
            assert!(!WorkspaceRegistry::handle(ctx).update(ctx, |registry, _| {
                registry.is_session_owner_retiring(&durable_identity_key)
            }));
        });
    });
}

#[test]
fn test_restored_closed_tab_returns_retiring_owner_to_live() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);
        let provider_session_id = "019f5f34-b6b7-70b3-8e50-e98504691cb3";

        let (durable_identity_key, terminal_model) =
            workspace.update(&mut app, |workspace, ctx| {
                let terminal_view = workspace
                    .active_tab_pane_group()
                    .as_ref(ctx)
                    .active_session_view(ctx)
                    .expect("owner tab must start with a terminal");
                CLIAgentSessionsModel::handle(ctx).update(ctx, |sessions, ctx| {
                    sessions.set_session(
                        terminal_view.id(),
                        CLIAgentSession {
                            agent: CLIAgent::Codex,
                            status: CLIAgentSessionStatus::InProgress,
                            session_context: CLIAgentSessionContext {
                                session_id: Some(provider_session_id.to_owned()),
                                ..Default::default()
                            },
                            input_state: CLIAgentInputState::Closed,
                            should_auto_toggle_input: false,
                            listener: None,
                            plugin_version: None,
                            environment_host_key: None,
                            draft_text: None,
                            custom_command_prefix: None,
                        },
                        ctx,
                    );
                });
                assert!(workspace.refresh_terminal_pane_session_binding(terminal_view.id(), ctx));
                let durable_identity_key = workspace
                    .live_workspace_sessions(ctx)
                    .into_iter()
                    .find(|session| {
                        session.cli_agent_session_id.as_deref() == Some(provider_session_id)
                    })
                    .and_then(|session| session.durable_identity_key())
                    .expect("registered tab must expose a durable owner");
                let terminal_model = terminal_view.as_ref(ctx).model.clone();

                workspace.add_welcome_tab(ctx);
                assert!(workspace.remove_tab(0, true, true, ctx));
                assert!(WorkspaceRegistry::handle(ctx).update(ctx, |registry, _| {
                    registry.is_session_owner_retiring(&durable_identity_key)
                }));
                (durable_identity_key, terminal_model)
            });

        UndoCloseStack::handle(&app).update(&mut app, |stack, ctx| {
            stack.undo_close(ctx);
        });

        workspace.update(&mut app, |_, ctx| {
            assert!(!WorkspaceRegistry::handle(ctx).update(ctx, |registry, _| {
                registry.is_session_owner_retiring(&durable_identity_key)
            }));
            assert!(
                !terminal_model.lock().has_exited(),
                "undo-retained terminal must remain running after tab restore"
            );
        });
    });
}

#[test]
fn test_discarded_closed_tab_shuts_down_retained_terminal() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);
        let provider_session_id = "019f5f34-b6b7-70b3-8e50-e98504691cb4";

        let (durable_identity_key, terminal_model, terminal_manager) =
            workspace.update(&mut app, |workspace, ctx| {
                let pane_group = workspace.active_tab_pane_group().clone();
                let terminal_view = pane_group
                    .as_ref(ctx)
                    .active_session_view(ctx)
                    .expect("owner tab must start with a terminal");
                let terminal_manager = pane_group
                    .as_ref(ctx)
                    .terminal_manager(0, ctx)
                    .expect("owner tab must expose a terminal manager");
                CLIAgentSessionsModel::handle(ctx).update(ctx, |sessions, ctx| {
                    sessions.set_session(
                        terminal_view.id(),
                        CLIAgentSession {
                            agent: CLIAgent::Codex,
                            status: CLIAgentSessionStatus::InProgress,
                            session_context: CLIAgentSessionContext {
                                session_id: Some(provider_session_id.to_owned()),
                                ..Default::default()
                            },
                            input_state: CLIAgentInputState::Closed,
                            should_auto_toggle_input: false,
                            listener: None,
                            plugin_version: None,
                            environment_host_key: None,
                            draft_text: None,
                            custom_command_prefix: None,
                        },
                        ctx,
                    );
                });
                assert!(workspace.refresh_terminal_pane_session_binding(terminal_view.id(), ctx));
                let durable_identity_key = workspace
                    .live_workspace_sessions(ctx)
                    .into_iter()
                    .find(|session| {
                        session.cli_agent_session_id.as_deref() == Some(provider_session_id)
                    })
                    .and_then(|session| session.durable_identity_key())
                    .expect("registered tab must expose a durable owner");
                let terminal_model = terminal_view.as_ref(ctx).model.clone();

                workspace.add_welcome_tab(ctx);
                assert!(workspace.remove_tab(0, true, true, ctx));
                assert!(WorkspaceRegistry::handle(ctx).update(ctx, |registry, _| {
                    registry.is_session_owner_retiring(&durable_identity_key)
                }));
                assert!(!terminal_manager
                    .as_ref(ctx)
                    .as_any()
                    .downcast_ref::<crate::terminal::local_tty::TerminalManager>()
                    .expect("mock workspace uses local_tty::TerminalManager")
                    .shutdown_requested());

                (durable_identity_key, terminal_model, terminal_manager)
            });

        UndoCloseSettings::handle(&app).update(&mut app, |settings, ctx| {
            settings
                .enabled
                .set_value(false, ctx)
                .expect("undo-close can be disabled for the discard test");
        });

        workspace.update(&mut app, |_, ctx| {
            assert!(terminal_manager
                .as_ref(ctx)
                .as_any()
                .downcast_ref::<crate::terminal::local_tty::TerminalManager>()
                .expect("mock workspace uses local_tty::TerminalManager")
                .shutdown_requested());
            assert!(WorkspaceRegistry::handle(ctx).update(ctx, |registry, _| {
                registry.is_session_owner_retiring(&durable_identity_key)
            }));

            terminal_model.lock().exit(ExitReason::PtyDisconnected);
            assert!(!WorkspaceRegistry::handle(ctx).update(ctx, |registry, _| {
                registry.is_session_owner_retiring(&durable_identity_key)
            }));
        });
    });
}

#[test]
fn test_hidden_closed_pane_reserves_durable_owner_while_undoable() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        FeatureFlag::UndoClosedPanes.set_enabled(true);
        let workspace = mock_workspace(&mut app);
        let provider_session_id = "019f5f34-b6b7-70b3-8e50-e98504691cb2";

        let (durable_identity_key, terminal_model) =
            workspace.update(&mut app, |workspace, ctx| {
                let pane_group = workspace.active_tab_pane_group().clone();
                let owner_pane_id = pane_group.as_ref(ctx).focused_pane_id(ctx);
                let terminal_view = pane_group
                    .as_ref(ctx)
                    .terminal_view_from_pane_id(owner_pane_id, ctx)
                    .expect("owner pane must be a terminal");
                CLIAgentSessionsModel::handle(ctx).update(ctx, |sessions, ctx| {
                    sessions.set_session(
                        terminal_view.id(),
                        CLIAgentSession {
                            agent: CLIAgent::Codex,
                            status: CLIAgentSessionStatus::InProgress,
                            session_context: CLIAgentSessionContext {
                                session_id: Some(provider_session_id.to_owned()),
                                ..Default::default()
                            },
                            input_state: CLIAgentInputState::Closed,
                            should_auto_toggle_input: false,
                            listener: None,
                            plugin_version: None,
                            environment_host_key: None,
                            draft_text: None,
                            custom_command_prefix: None,
                        },
                        ctx,
                    );
                });
                assert!(workspace.refresh_terminal_pane_session_binding(terminal_view.id(), ctx));
                let durable_identity_key = workspace
                    .live_workspace_sessions(ctx)
                    .into_iter()
                    .find(|session| {
                        session.cli_agent_session_id.as_deref() == Some(provider_session_id)
                    })
                    .and_then(|session| session.durable_identity_key())
                    .expect("registered pane must expose a durable owner");
                let terminal_model = terminal_view.as_ref(ctx).model.clone();
                pane_group.update(ctx, |pane_group, ctx| {
                    pane_group.handle_action(&PaneGroupAction::Add(Direction::Right), ctx);
                });

                workspace.close_pane(pane_group.id(), owner_pane_id, ctx);

                assert!(pane_group
                    .as_ref(ctx)
                    .is_pane_hidden_for_close(owner_pane_id));
                assert!(WorkspaceRegistry::handle(ctx).update(ctx, |registry, _| {
                    registry.is_session_owner_retiring(&durable_identity_key)
                }));

                (durable_identity_key, terminal_model)
            });

        UndoCloseStack::handle(&app).update(&mut app, |stack, ctx| {
            stack.undo_close(ctx);
        });

        workspace.update(&mut app, |_, ctx| {
            assert!(!WorkspaceRegistry::handle(ctx).update(ctx, |registry, _| {
                registry.is_session_owner_retiring(&durable_identity_key)
            }));
            assert!(
                !terminal_model.lock().has_exited(),
                "undo-retained terminal must remain running after pane restore"
            );
        });
    });
}

#[test]
fn test_discarded_hidden_pane_shuts_down_retained_terminal() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        FeatureFlag::UndoClosedPanes.set_enabled(true);
        let workspace = mock_workspace(&mut app);
        let provider_session_id = "019f5f34-b6b7-70b3-8e50-e98504691cb5";

        let (durable_identity_key, terminal_model, terminal_manager) =
            workspace.update(&mut app, |workspace, ctx| {
                let pane_group = workspace.active_tab_pane_group().clone();
                let owner_pane_id = pane_group.as_ref(ctx).focused_pane_id(ctx);
                let terminal_view = pane_group
                    .as_ref(ctx)
                    .terminal_view_from_pane_id(owner_pane_id, ctx)
                    .expect("owner pane must be a terminal");
                let terminal_manager = pane_group
                    .as_ref(ctx)
                    .terminal_manager_for_pane_id(owner_pane_id, ctx)
                    .expect("owner pane must expose a terminal manager");
                CLIAgentSessionsModel::handle(ctx).update(ctx, |sessions, ctx| {
                    sessions.set_session(
                        terminal_view.id(),
                        CLIAgentSession {
                            agent: CLIAgent::Codex,
                            status: CLIAgentSessionStatus::InProgress,
                            session_context: CLIAgentSessionContext {
                                session_id: Some(provider_session_id.to_owned()),
                                ..Default::default()
                            },
                            input_state: CLIAgentInputState::Closed,
                            should_auto_toggle_input: false,
                            listener: None,
                            plugin_version: None,
                            environment_host_key: None,
                            draft_text: None,
                            custom_command_prefix: None,
                        },
                        ctx,
                    );
                });
                assert!(workspace.refresh_terminal_pane_session_binding(terminal_view.id(), ctx));
                let durable_identity_key = workspace
                    .live_workspace_sessions(ctx)
                    .into_iter()
                    .find(|session| {
                        session.cli_agent_session_id.as_deref() == Some(provider_session_id)
                    })
                    .and_then(|session| session.durable_identity_key())
                    .expect("registered pane must expose a durable owner");
                let terminal_model = terminal_view.as_ref(ctx).model.clone();
                pane_group.update(ctx, |pane_group, ctx| {
                    pane_group.handle_action(&PaneGroupAction::Add(Direction::Right), ctx);
                });

                workspace.close_pane(pane_group.id(), owner_pane_id, ctx);

                assert!(pane_group
                    .as_ref(ctx)
                    .is_pane_hidden_for_close(owner_pane_id));
                assert!(WorkspaceRegistry::handle(ctx).update(ctx, |registry, _| {
                    registry.is_session_owner_retiring(&durable_identity_key)
                }));
                assert!(!terminal_manager
                    .as_ref(ctx)
                    .as_any()
                    .downcast_ref::<crate::terminal::local_tty::TerminalManager>()
                    .expect("mock workspace uses local_tty::TerminalManager")
                    .shutdown_requested());

                (durable_identity_key, terminal_model, terminal_manager)
            });

        UndoCloseSettings::handle(&app).update(&mut app, |settings, ctx| {
            settings
                .enabled
                .set_value(false, ctx)
                .expect("undo-close can be disabled for the pane discard test");
        });

        workspace.update(&mut app, |_, ctx| {
            assert!(terminal_manager
                .as_ref(ctx)
                .as_any()
                .downcast_ref::<crate::terminal::local_tty::TerminalManager>()
                .expect("mock workspace uses local_tty::TerminalManager")
                .shutdown_requested());
            assert!(WorkspaceRegistry::handle(ctx).update(ctx, |registry, _| {
                registry.is_session_owner_retiring(&durable_identity_key)
            }));

            terminal_model.lock().exit(ExitReason::PtyDisconnected);
            assert!(!WorkspaceRegistry::handle(ctx).update(ctx, |registry, _| {
                registry.is_session_owner_retiring(&durable_identity_key)
            }));
        });
    });
}

#[test]
fn test_cross_window_pending_restore_reserves_durable_session_owner() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let owner_workspace = mock_workspace(&mut app);
        let contender_workspace = mock_workspace(&mut app);
        let server = test_ssh_server_for_environment_tests();
        let environment =
            crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "cross-window-remote".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
        let authority = environment.authority_key.clone();
        let pending_restore = test_pending_environment_runtime_session_restore(&authority);
        let durable_identity_key = pending_restore
            .session
            .durable_identity_key()
            .expect("pending restore must expose a durable identity");

        let owner_window_id = owner_workspace.update(&mut app, |workspace, ctx| {
            workspace.set_active_tab_environment(environment.clone());
            workspace.restore_environment_runtime_session(&authority, pending_restore.clone(), ctx);
            assert!(workspace
                .live_or_pending_workspace_session_locator(&durable_identity_key, ctx)
                .is_some());
            ctx.window_id()
        });

        contender_workspace.update(&mut app, |workspace, ctx| {
            workspace.set_active_tab_environment(environment);
            let session = pending_restore.session;
            let target = WorkspaceSessionActionTarget::new(
                session.id.clone(),
                session.environment_authority_key.clone(),
            );
            workspace.restored_workspace_sessions.push(session);
            workspace.sync_session_navigator_sessions(ctx);
            let contender_tab_count = workspace.tab_count();
            let owner = WorkspaceRegistry::as_ref(ctx)
                .other_workspace_session_owner(
                    ctx.window_id(),
                    &durable_identity_key,
                    ctx,
                )
                .expect("pane-owned pending restore must reserve app-wide ownership");
            assert_eq!(owner.window_id, owner_window_id);

            workspace.activate_restored_workspace_session(&target, ctx);

            assert_eq!(
                workspace.tab_count(),
                contender_tab_count,
                "a pane-owned pending restore must reserve the durable session before remote materialization finishes"
            );
            assert!(workspace
                .latest_pending_session_restore_for_authority(&authority)
                .is_none());
        });
    });
}

#[test]
fn test_local_resume_preserves_full_navigator_cardinality() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let first = test_session_navigator_order_session("order-key-resume-a", "A", 10);
            let second = test_session_navigator_order_session("order-key-resume-b", "B", 20);
            let third = test_session_navigator_order_session("order-key-append-c", "C", 30);
            let target = WorkspaceSessionActionTarget::new(
                second.id.clone(),
                second.environment_authority_key.clone(),
            );
            workspace.restored_workspace_sessions.push(first);
            workspace.restored_workspace_sessions.push(second);
            workspace.restored_workspace_sessions.push(third);
            workspace.sync_session_navigator_sessions(ctx);

            assert_eq!(
                test_session_navigator_displayed_order(workspace),
                vec![
                    "order-key-append-c",
                    "order-key-resume-b",
                    "order-key-resume-a"
                ],
                "baseline order should match the list the user sees before clicking resume"
            );

            let user_state_before_resume = workspace.workspace_session_user_state_for_authority(
                &workspace.current_environment_authority_key(ctx),
            );
            workspace.activate_restored_workspace_session(&target, ctx);

            let order_after_resume = workspace
                .session_navigator_sessions()
                .iter()
                .filter_map(|session| match session.cli_agent_session_id.as_deref() {
                    Some("order-key-resume-a-provider-session") => Some("order-key-resume-a"),
                    Some("order-key-resume-b-provider-session") => Some("order-key-resume-b"),
                    Some("order-key-append-c-provider-session") => Some("order-key-append-c"),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(
                order_after_resume,
                vec![
                    "order-key-append-c",
                    "order-key-resume-b",
                    "order-key-resume-a"
                ],
                "local Resume must preserve all target and unrelated rows in their original order"
            );
            let user_state = workspace.workspace_session_user_state_for_authority(
                &workspace.current_environment_authority_key(ctx),
            );
            assert_eq!(
                user_state.pinned, user_state_before_resume.pinned,
                "resuming a row must not mutate existing pinned state"
            );
            assert_eq!(
                user_state.aliases, user_state_before_resume.aliases,
                "resuming a row must not mutate existing alias state"
            );
            let resumed_rows = workspace
                .session_navigator_sessions()
                .into_iter()
                .filter(|session| {
                    session.cli_agent_session_id.as_deref()
                        == Some("order-key-resume-b-provider-session")
                })
                .collect::<Vec<_>>();
            assert!(
                resumed_rows.iter().all(|session| !session.is_pinned),
                "resuming a row must not render it as pinned"
            );
        });
    });
}

#[test]
fn test_environment_runtime_resume_preserves_full_navigator_cardinality() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9921);
            workspace.environments_mut().mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-ssh-control-remote-order.sock"),
            );
            workspace
                .environments_mut()
                .mark_connected_session(session_id, HostId::new("remote-order-host".to_string()));
            workspace.set_active_tab_environment(environment);

            let make_session = |id: &str, label: &str, provider_session_id: &str| {
                WorkspaceSessionSnapshot {
                    id: id.to_string(),
                    container_uuid: None,
                    kind: WorkspaceSessionKind::AgentTerminal,
                    label: Some(label.to_string()),
                    environment_authority_key: Some(authority.clone()),
                    cwd: Some("/root/project".to_string()),
                    startup_directory: None,
                    cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
                    cli_command: Some("codex".to_string()),
                    cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                    conversation_ids: Vec::new(),
                    active_conversation_id: None,
                    cli_agent_session_id: Some(provider_session_id.to_string()),
                    is_active: false,
                    is_pinned: false,
                    updated_at_unix_ms: None,
                    is_live_container: false,
                }
            };

            let first = make_session("remote-order-a", "A", "remote-order-provider-a");
            let second = make_session("remote-order-b", "B", "remote-order-provider-b");
            let third = make_session("remote-order-c", "C", "remote-order-provider-c");
            let target = WorkspaceSessionActionTarget::new(
                second.id.clone(),
                second.environment_authority_key.clone(),
            );
            workspace.restored_workspace_sessions.push(first);
            workspace.restored_workspace_sessions.push(second);
            workspace.restored_workspace_sessions.push(third);
            workspace.sync_session_navigator_sessions(ctx);

            let baseline = workspace
                .session_navigator_sessions()
                .iter()
                .filter_map(|session| match session.cli_agent_session_id.as_deref() {
                    Some("remote-order-provider-a") => Some("remote-order-a"),
                    Some("remote-order-provider-b") => Some("remote-order-b"),
                    Some("remote-order-provider-c") => Some("remote-order-c"),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(baseline, vec!["remote-order-a", "remote-order-b", "remote-order-c"]);

            workspace.activate_restored_workspace_session(&target, ctx);

            let order_after_resume = workspace
                .session_navigator_sessions()
                .iter()
                .filter_map(|session| match session.cli_agent_session_id.as_deref() {
                    Some("remote-order-provider-a") => Some("remote-order-a"),
                    Some("remote-order-provider-b") => Some("remote-order-b"),
                    Some("remote-order-provider-c") => Some("remote-order-c"),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(
                order_after_resume,
                vec!["remote-order-a", "remote-order-b", "remote-order-c"],
                "runtime Resume must preserve all target and unrelated rows in their original order"
            );
            let user_state = workspace.workspace_session_user_state_for_authority(&authority);
            assert!(
                user_state.pinned.is_empty(),
                "remote resume must not persist pinned state"
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
                test_session_navigator_displayed_order(workspace),
                vec!["order-key-resume-b", "order-key-resume-a"]
            );

            workspace.dispatch_session_navigator_state_action(
                session_navigator_reducer::SessionNavigatorAction::SelectionChanged {
                    session_logical_key: Some(Workspace::workspace_session_logical_key(&first)),
                },
                ctx,
            );
            let session = workspace
                .restored_workspace_sessions
                .iter_mut()
                .find(|session| session.id == "order-key-resume-a")
                .expect("test session exists");
            session.updated_at_unix_ms = Some(10_000);
            workspace.sync_session_navigator_sessions(ctx);

            assert_eq!(
                test_session_navigator_displayed_order(workspace),
                vec!["order-key-resume-b", "order-key-resume-a"],
                "resume/status refresh must not reorder existing Session Navigator rows"
            );
        });
    });
}

#[test]
fn test_session_navigator_refresh_preserves_order_when_restore_becomes_live_container() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let mut first = test_session_navigator_order_session("order-key-resume-a", "A", 10);
            let second = test_session_navigator_order_session("order-key-resume-b", "B", 20);
            workspace.restored_workspace_sessions.push(first.clone());
            workspace.restored_workspace_sessions.push(second);
            workspace.sync_session_navigator_sessions(ctx);
            assert_eq!(
                test_session_navigator_displayed_order(workspace),
                vec!["order-key-resume-b", "order-key-resume-a"]
            );

            workspace
                .restored_workspace_sessions
                .retain(|session| session.id != "order-key-resume-a");
            first.id = "tab:99:leaf:0".to_string();
            first.container_uuid = Some(vec![0x99; 16]);
            first.is_live_container = true;
            workspace.restored_workspace_sessions.push(first);
            workspace.sync_session_navigator_sessions(ctx);

            assert_eq!(
                workspace
                    .session_navigator_sessions()
                    .iter()
                    .filter_map(|session| match session.id.as_str() {
                        "tab:99:leaf:0" => Some("materialized-a"),
                        "order-key-resume-b" => Some("order-key-resume-b"),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
                vec!["order-key-resume-b", "materialized-a"],
                "clicking/restoring a row must keep the materialized row in its existing durable slot instead of moving it to the top"
            );
        });
    });
}

#[test]
fn test_session_navigator_live_row_delete_keeps_materialized_backing_source() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let indexed = test_session_navigator_order_session("order-key-resume-a", "A", 10);
            let mut live = indexed.clone();
            live.id = "tab:99:leaf:0".to_string();
            live.container_uuid = Some(vec![0x98; 16]);
            live.is_active = true;
            live.is_live_container = true;
            let authority = workspace.current_environment_authority_key(ctx);
            workspace
                .environments
                .commit_indexed_cli_agent_sessions(
                    &authority,
                    Ok::<_, String>(vec![indexed.clone()]),
                )
                .expect("complete local session index commit cannot fail");
            workspace.restored_workspace_sessions.push(indexed.clone());

            assert!(
                workspace
                    .backing_sessions_for_workspace_session(&live)
                    .iter()
                    .any(|session| session.id == indexed.id),
                "deleting a materialized live row must still target the indexed/restored provider source; otherwise the UI reports success but the scan brings the row back"
            );

            for candidate in workspace.backing_sessions_for_workspace_session(&live) {
                assert!(
                    !candidate.is_live_container() || candidate.container_uuid.is_some(),
                    "非法 backing session: {candidate:?}"
                );
            }
            let plan = workspace.workspace_session_delete_plan(live, ctx);
            workspace.begin_workspace_session_delete_plan(&plan, ctx);

            assert!(
                workspace
                    .session_navigator_sessions()
                    .into_iter()
                    .all(|session| session.id != indexed.id),
                "accepted deletes must hide all visible aliases of the row immediately; stale indexed/restored sources must not remain as broken restore targets while provider deletion is in flight"
            );

            workspace.rollback_workspace_session_delete_plan(&plan, ctx);
            assert!(
                workspace
                    .session_navigator_sessions()
                    .into_iter()
                    .any(|session| session.id == indexed.id),
                "if provider deletion fails, the transient delete tombstone must roll back instead of permanently hiding the source"
            );

            workspace.begin_workspace_session_delete_plan(&plan, ctx);
            workspace.finish_workspace_session_delete_plan(&plan, ctx);
            workspace.sync_session_navigator_sessions(ctx);

            assert!(
                workspace
                    .session_navigator_sessions()
                    .into_iter()
                    .all(|session| session.id != indexed.id),
                "after delete succeeds, cached indexed/restored rows must be removed immediately so the deleted row does not linger as a broken restore target"
            );

            workspace.restored_workspace_sessions.push(indexed.clone());
            workspace.sync_session_navigator_sessions(ctx);

            assert!(
                workspace
                    .session_navigator_sessions()
                    .into_iter()
                    .all(|session| session.id != indexed.id),
                "successful delete keeps the durable identity tombstoned, so stale restored/history sources cannot immediately resurrect the deleted row"
            );

            let mut unrelated_live = test_session_navigator_order_session(
                "order-key-unrelated-live",
                "Unrelated",
                30,
            );
            unrelated_live.id = "tab:99:leaf:0".to_string();
            unrelated_live.container_uuid = Some(vec![0x97; 16]);
            unrelated_live.cli_agent_session_id = Some("different-provider-session".to_string());
            unrelated_live.is_live_container = true;
            let unrelated_identities = Workspace::workspace_session_identity_keys(&unrelated_live);
            let navigator_state = workspace.snapshot_session_navigator_state();
            let unrelated_row_id = Workspace::workspace_session_row_id(&unrelated_live, &navigator_state);
            let mut sessions = vec![unrelated_live.clone()];
            workspace.filter_deleting_workspace_sessions(&mut sessions);
            assert_eq!(
                sessions.len(),
                1,
                "successful delete must release volatile tab identity keys so a later live row reusing tab coordinates is not hidden; identities={unrelated_identities:?}, row_id={unrelated_row_id}, deleting={:?}, deleted={:?}, registry={:?}",
                navigator_state.deleting_row_ids,
                navigator_state.deleted_row_ids,
                navigator_state.row_id_by_identity,
            );
        });
    });
}

#[test]
fn test_session_navigator_refresh_inserts_new_rows_at_top() {
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
                test_session_navigator_displayed_order(workspace),
                vec![
                    "order-key-append-c",
                    "order-key-append-b",
                    "order-key-append-a"
                ],
                "manual refresh should give newly discovered sessions a larger display_order so they sit at the top of the unpinned list"
            );
        });
    });
}

#[test]
fn test_session_navigator_groups_same_window_split_rows_next_to_each_other() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            workspace.sync_session_navigator_sessions(ctx);
            workspace.restored_workspace_sessions.push(
                test_session_navigator_order_session("order-key-split-between", "Between", 20),
            );
            workspace.sync_session_navigator_sessions(ctx);

            let pane_group = workspace.active_tab_pane_group();
            pane_group.update(ctx, |panes, ctx| {
                panes.add_terminal_pane_with_options(
                    Direction::Right,
                    NewTerminalOptions::default(),
                    ctx,
                );
            });
            workspace.sync_session_navigator_sessions(ctx);

            let relevant_rows = workspace
                .session_navigator_sessions()
                .into_iter()
                .filter(|session| {
                    matches!(
                        session.id.as_str(),
                        "tab:0:leaf:0" | "tab:0:leaf:1" | "order-key-split-between"
                    )
                })
                .collect::<Vec<_>>();
            let split_group_numbers =
                workspace.same_window_split_group_numbers_for_sessions(&relevant_rows, ctx);
            assert_eq!(
                relevant_rows
                    .iter()
                    .map(|session| session.id.as_str())
                    .collect::<Vec<_>>(),
                vec!["tab:0:leaf:0", "tab:0:leaf:1", "order-key-split-between"],
                "new split-pane rows should stay adjacent to the existing row from the same window instead of being stranded after older navigator rows"
            );
            assert_eq!(
                relevant_rows
                    .iter()
                    .filter(|session| session.id.starts_with("tab:0:leaf:"))
                    .map(|session| {
                        workspace.workspace_session_same_window_split_group_number(
                            session,
                            &split_group_numbers,
                        )
                    })
                    .collect::<Vec<_>>(),
                vec![Some(1), Some(1)],
                "live rows from a split pane should carry the same numbered split-group marker"
            );
            assert_eq!(
                workspace.workspace_session_same_window_split_group_number(
                    relevant_rows
                        .iter()
                        .find(|session| session.id == "order-key-split-between")
                        .expect("test row exists"),
                    &split_group_numbers,
                ),
                None,
                "non split rows must not carry a split-group marker"
            );
        });
    });
}

#[test]
fn test_session_navigator_refresh_preserves_temporarily_hidden_order_keys() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let first = test_session_navigator_order_session("order-key-hidden-a", "A", 10);
            let second = test_session_navigator_order_session("order-key-hidden-b", "B", 20);
            let third = test_session_navigator_order_session("order-key-hidden-c", "C", 30);
            workspace.restored_workspace_sessions.push(first.clone());
            workspace.restored_workspace_sessions.push(second.clone());
            workspace.sync_session_navigator_sessions(ctx);
            let initial_state = workspace.snapshot_session_navigator_state();
            let first_order_key = Workspace::workspace_session_row_id(&first, &initial_state);
            let second_order_key = Workspace::workspace_session_row_id(&second, &initial_state);
            assert!(workspace
                .snapshot_session_navigator_state()
                .display_order
                .contains_key(&second_order_key));

            // Switching tabs/environments makes some rows temporarily absent
            // from the current Session Navigator source set. That must not
            // delete their display order, otherwise switching back reallocates
            // them after newer rows and the left rail appears to drift.
            workspace
                .restored_workspace_sessions
                .retain(|session| session.id != "order-key-hidden-b");
            workspace.sync_session_navigator_sessions(ctx);
            workspace.restored_workspace_sessions.push(third);
            workspace.sync_session_navigator_sessions(ctx);
            workspace.restored_workspace_sessions.push(second);
            workspace.sync_session_navigator_sessions(ctx);

            assert!(workspace
                .snapshot_session_navigator_state()
                .display_order
                .contains_key(&first_order_key));
            assert!(
                workspace
                    .snapshot_session_navigator_state()
                    .display_order
                    .contains_key(&second_order_key),
                "refresh must preserve order keys for rows that are temporarily hidden by tab/environment switches"
            );
            assert_eq!(
                test_session_navigator_displayed_order(workspace),
                vec![
                    "order-key-hidden-c",
                    "order-key-hidden-b",
                    "order-key-hidden-a"
                ],
                "a row returning after a tab/environment switch must keep its preserved order relative to older rows; newer rows added while it was hidden stay above it"
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
                test_session_navigator_displayed_order(workspace),
                vec!["order-key-pin-c", "order-key-pin-b", "order-key-pin-a"]
            );

            let target = WorkspaceSessionActionTarget::new("order-key-pin-b".to_owned(), None);
            workspace.toggle_workspace_session_pinned(&target, true, ctx);
            assert_eq!(
                test_session_navigator_displayed_order(workspace),
                vec!["order-key-pin-b", "order-key-pin-c", "order-key-pin-a"],
                "pin should move the row into the pinned group without reallocating order"
            );

            workspace.toggle_workspace_session_pinned(&target, false, ctx);
            assert_eq!(
                test_session_navigator_displayed_order(workspace),
                vec!["order-key-pin-c", "order-key-pin-b", "order-key-pin-a"],
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
            let authority = "ssh:ssh-config:remote-fixture-primary".to_string();
            workspace.set_active_tab_environment(crate::app_state::EnvironmentSnapshot {
                authority_key: authority.clone(),
                label: "remote-fixture-primary".to_string(),
                kind: crate::app_state::EnvironmentKind::Ssh,
                lifecycle_state: crate::app_state::EnvironmentLifecycleState::Connected,
                active_workspace_root: Some("/root/project".to_string()),
                connection_ref: Some("remote-fixture-primary".to_string()),
            });
            let mut session =
                test_environment_runtime_session_snapshot("remote:test", authority.clone());
            session.cli_agent_session_id = Some("remote-provider-session".to_string());
            let logical_key = session.logical_key();
            workspace
                .environments_mut()
                .commit_indexed_cli_agent_sessions(&authority, Ok::<_, String>(vec![session]))
                .expect("commit indexed remote session fixture");
            workspace
                .environments_mut()
                .set_cli_agent_session_user_state(
                    authority,
                    crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserState {
                        aliases: HashMap::from([(logical_key.clone(), "Remote Alias".to_string())]),
                        pinned: HashSet::from([logical_key.clone()]),
                    },
                );

            workspace.sync_session_navigator_sessions(ctx);
            let sessions = workspace.session_navigator_sessions();

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
fn test_session_navigator_external_user_alias_is_projected_before_resume() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let provider_session_id = "codex-external-alias-before-resume";
            let rollout_id = "external:Codex:rollout-before-resume".to_string();
            let rollout = WorkspaceSessionSnapshot {
                id: rollout_id.clone(),
                container_uuid: None,
                kind: WorkspaceSessionKind::AgentTerminal,
                label: None,
                environment_authority_key: Some("local".to_string()),
                cwd: Some("/Users/admin/ashide".to_string()),
                startup_directory: None,
                cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
                cli_command: Some(CLIAgent::Codex.command_prefix().to_string()),
                cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                conversation_ids: Vec::new(),
                active_conversation_id: None,
                cli_agent_session_id: Some(provider_session_id.to_string()),
                is_active: false,
                is_pinned: false,
                updated_at_unix_ms: Some(300),
                is_live_container: false,
            };
            let mut index = rollout.clone();
            index.id = "external-index:Codex:before-resume".to_string();
            index.cwd = None;
            index.updated_at_unix_ms = Some(10);
            let indexed_sessions = vec![rollout.clone(), index];

            let mut merged =
                WorkspaceSessionSnapshot::merge_for_session_navigator(indexed_sessions);
            assert_eq!(merged.len(), 1);
            workspace.apply_workspace_session_aliases(
                &mut merged,
                &crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserState {
                    // 故意只写 rollout backing source key，验证 projection 在
                    // Resume/materialize 前即可沿 backing source 找到外置别名。
                    aliases: HashMap::from([(rollout_id, "外置配置别名".to_string())]),
                    pinned: HashSet::new(),
                },
                ctx,
            );

            assert_eq!(merged[0].label.as_deref(), Some("外置配置别名"));
            assert!(!merged[0].is_live_container);
        });
    });
}

#[test]
fn test_session_navigator_durable_alias_never_overrides_live_container_title() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let mut session =
                test_session_navigator_order_session("tab:0:leaf:0", "Container Title", 20);
            session.container_uuid = Some(vec![7; 16]);
            session.is_live_container = true;
            let aliases = session
                .stable_user_state_keys()
                .into_iter()
                .map(|key| (key, "Stale Durable Alias".to_owned()))
                .collect();
            let user_state =
                crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserState {
                    aliases,
                    pinned: HashSet::new(),
                };
            let mut sessions = vec![session];

            workspace.apply_workspace_session_aliases(&mut sessions, &user_state, ctx);

            assert_eq!(sessions[0].label.as_deref(), Some("Container Title"));
            assert_eq!(
                workspace.workspace_session_alias_with_state(&sessions[0], &user_state, ctx),
                None
            );
        });
    })
}

#[test]
fn test_session_navigator_alias_projection_uses_canonical_environment_navigation_key() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let local = EnvironmentSnapshot::local(None);
            let mut local_with_root = EnvironmentSnapshot::local(Some("/tmp/project".to_owned()));
            local_with_root.authority_key = "local:/tmp/project".to_owned();
            workspace.set_active_tab_environment(local.clone());
            workspace.environments_mut().upsert(local_with_root.clone());

            let mut first = test_session_navigator_order_session(
                "alias-projection-unrelated-first",
                "Unrelated First",
                30,
            );
            let mut target =
                test_session_navigator_order_session("alias-projection-target", "Source Title", 20);
            target.environment_authority_key = Some(local_with_root.authority_key.clone());
            let mut second = test_session_navigator_order_session(
                "alias-projection-unrelated-second",
                "Unrelated Second",
                10,
            );
            first.environment_authority_key = Some(local.authority_key.clone());
            second.environment_authority_key = Some(local.authority_key.clone());
            let target_keys = target.stable_user_state_keys();
            workspace.restored_workspace_sessions = vec![first, target.clone(), second];
            workspace
                .environments_mut()
                .set_cli_agent_session_user_state(
                    local_with_root.authority_key,
                    crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserState {
                        aliases: target_keys
                            .iter()
                            .cloned()
                            .map(|key| (key, "Canonical Alias".to_owned()))
                            .collect(),
                        pinned: HashSet::new(),
                    },
                );

            workspace.sync_session_navigator_sessions(ctx);
            let sessions = workspace.session_navigator_sessions();
            let fixture_ids = [
                "alias-projection-unrelated-first",
                "alias-projection-target",
                "alias-projection-unrelated-second",
            ];
            let fixture_sessions = sessions
                .iter()
                .filter(|session| fixture_ids.contains(&session.id.as_str()))
                .collect::<Vec<_>>();
            assert_eq!(
                fixture_sessions.len(),
                3,
                "alias mutation must preserve the target and both unrelated rows"
            );
            assert_eq!(
                fixture_sessions
                    .iter()
                    .map(|session| session.id.as_str())
                    .collect::<Vec<_>>(),
                fixture_ids,
                "alias mutation must not change display order or unrelated rows"
            );
            assert_eq!(
                sessions
                    .iter()
                    .find(|session| session.id == target.id)
                    .and_then(|session| session.label.as_deref()),
                Some("Canonical Alias"),
                "current local projection must observe user state committed through local:<root>"
            );
            assert_eq!(
                workspace
                    .snapshot_session_navigator_model()
                    .sessions
                    .iter()
                    .find(|session| session.id == target.id)
                    .and_then(|session| session.label.as_deref()),
                Some("Canonical Alias"),
                "Environment-owned reducer model must be the alias-visible carrier"
            );
        });
    });
}

#[test]
fn test_materialized_sourceless_restore_does_not_apply_source_alias_and_preserves_pin() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            workspace.add_test_environment_runtime_placeholder_tab(
                environment,
                Some("Antigravity restore".to_string()),
                ctx,
            );

            let mut source = test_environment_runtime_session_snapshot(
                "environment-antigravity-restore",
                authority.clone(),
            );
            source.cli_agent = Some("Antigravity".to_string());
            source.cli_command = Some("antigravity".to_string());
            source.cli_agent_session_id = None;
            let source_key = source.id.clone();
            let binding = crate::app_state::PaneSessionBinding::from_workspace_session(
                &source,
            )
            .expect("sourceless agent restore still has semantic provider metadata");

            let pane_group = workspace.active_tab_pane_group();
            pane_group.update(ctx, |pane_group, ctx| {
                let pane_id = pane_group.focused_pane_id(ctx);
                assert!(pane_group.restore_session_binding_for_pane_id(
                    pane_id,
                    Some(binding),
                    ctx,
                ));
            });
            workspace.environments_mut().set_cli_agent_session_user_state(
                authority.clone(),
                crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserState {
                    aliases: HashMap::from([(source_key.clone(), "跨恢复别名".to_string())]),
                    pinned: HashSet::from([source_key]),
                },
            );
            // 生产 Resume 在 carrier binding 与 user state 就绪后由 owner 显式提交；
            // 直接 fixture mutation 不能依赖只读 getter 隐式 Refresh。
            workspace.sync_session_navigator_sessions(ctx);

            let sessions = workspace.session_navigator_sessions();
            let matching = sessions
                .iter()
                .filter(|session| {
                    session.environment_authority_key.as_deref() == Some(authority.as_str())
                })
                .collect::<Vec<_>>();
            assert_eq!(matching.len(), 1);
            assert!(matching[0].is_live_container());
            assert_eq!(matching[0].label.as_deref(), Some("Antigravity restore"));
            assert!(matching[0].is_pinned);
            assert_eq!(matching[0].cli_agent_session_id, None);
        });
    });
}

#[test]
fn test_local_materialized_sourceless_restore_does_not_apply_source_alias_and_preserves_pin() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let authority = workspace.current_environment_authority_key(ctx);
            let mut source = workspace
                .live_workspace_sessions(ctx)
                .into_iter()
                .find(|session| session.is_active)
                .expect("expected active local terminal container");
            source.id = "current-app-antigravity-restore".to_string();
            source.container_uuid = None;
            source.kind = WorkspaceSessionKind::AgentTerminal;
            source.cli_agent = Some("Antigravity".to_string());
            source.cli_command = Some("antigravity".to_string());
            source.cli_agent_origin = Some(CliAgentSessionOrigin::CommandDetected);
            source.cli_agent_session_id = None;
            source.is_active = false;
            source.is_live_container = false;
            let source_key = source.id.clone();
            let binding = crate::app_state::PaneSessionBinding::from_workspace_session(&source)
                .expect("local sourceless restore still has semantic provider metadata");

            let pane_group = workspace.active_tab_pane_group();
            pane_group.update(ctx, |pane_group, ctx| {
                let pane_id = pane_group.focused_pane_id(ctx);
                assert!(pane_group.restore_session_binding_for_pane_id(
                    pane_id,
                    Some(binding),
                    ctx,
                ));
            });
            workspace
                .environments_mut()
                .entry_target_snapshot(&authority);
            workspace
                .environments_mut()
                .set_cli_agent_session_user_state(
                    authority.clone(),
                    crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserState {
                        aliases: HashMap::from([(
                            source_key.clone(),
                            "本地跨恢复别名".to_string(),
                        )]),
                        pinned: HashSet::from([source_key.clone()]),
                    },
                );
            // 与真实 local delivery 的 owner commit 边界一致；render getter 只读
            // committed snapshot，不负责发布 fixture 的直接 mutation。
            workspace.sync_session_navigator_sessions(ctx);

            let sessions = workspace.session_navigator_sessions();
            let matching = sessions
                .iter()
                .filter(|session| session.cli_agent.as_deref() == Some("Antigravity"))
                .collect::<Vec<_>>();
            assert_eq!(matching.len(), 1);
            assert!(matching[0].is_live_container());
            assert_eq!(matching[0].label.as_deref(), None);
            assert!(matching[0].is_pinned);
            assert_eq!(matching[0].cli_agent_session_id, None);
            assert!(workspace
                .workspace_session_pin_keys(matching[0], ctx)
                .contains(&source_key));
        });
    });
}

#[test]
fn test_remote_session_navigator_metadata_failure_preserves_cached_enrichment() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, _ctx| {
            let authority = "ssh:ssh-config:remote-fixture-primary".to_string();
            let cached =
                crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserState {
                    aliases: HashMap::from([(
                        "remote::agent:Codex:session-a".to_string(),
                        "固定别名".to_string(),
                    )]),
                    pinned: HashSet::from(["remote::agent:Codex:session-a".to_string()]),
                };
            workspace
                .environments_mut()
                .upsert(crate::app_state::EnvironmentSnapshot {
                    authority_key: authority.clone(),
                    label: "remote-fixture-primary".to_string(),
                    kind: crate::app_state::EnvironmentKind::Ssh,
                    lifecycle_state: crate::app_state::EnvironmentLifecycleState::Connected,
                    active_workspace_root: Some("/root/project".to_string()),
                    connection_ref: Some("remote-fixture-primary".to_string()),
                });
            workspace
                .environments_mut()
                .set_cli_agent_session_user_state(authority.clone(), cached.clone());

            workspace
                .commit_indexed_environment_cli_agent_sessions(
                    &authority,
                    Ok::<_, String>(Vec::new()),
                )
                .expect("source scan commit should not replace cached enrichment");

            let actual = workspace.workspace_session_user_state_for_authority(&authority);
            assert_eq!(actual.aliases, cached.aliases);
            assert_eq!(actual.pinned, cached.pinned);
        });
    });
}

#[test]
fn test_remote_session_navigator_scan_result_is_source_of_truth() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let authority = "ssh:ssh-config:remote-fixture-primary".to_string();
            workspace.set_active_tab_environment(crate::app_state::EnvironmentSnapshot {
                authority_key: authority.clone(),
                label: "remote-fixture-primary".to_string(),
                kind: crate::app_state::EnvironmentKind::Ssh,
                lifecycle_state: crate::app_state::EnvironmentLifecycleState::Connected,
                active_workspace_root: Some("/root/project".to_string()),
                connection_ref: Some("remote-fixture-primary".to_string()),
            });
            let mut session =
                test_environment_runtime_session_snapshot("remote:source", authority.clone());
            session.cli_agent_session_id = Some("remote-source-provider-session".to_string());
            workspace
                .environments_mut()
                .commit_indexed_cli_agent_sessions(
                    &authority,
                    Ok::<_, String>(vec![session]),
                )
                .expect("commit indexed remote source fixture");
            workspace
                .environments_mut()
                .set_cli_agent_session_user_state(
                    authority,
                    crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserState {
                        aliases: HashMap::new(),
                        pinned: HashSet::new(),
                    },
                );

            workspace.sync_session_navigator_sessions(ctx);
            let sessions = workspace.session_navigator_sessions();

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
            workspace.agent_action_sidecar_source = Some(AgentActionSidecarSource::SessionBridge(
                SessionBridgeActionSource::Conversation {
                    conversation_id: AIConversationId::new(),
                    source_environment_authority_key: Some("ssh:stale".to_string()),
                },
            ));
            let session = WorkspaceSessionSnapshot {
                id: "plain-cli-agent-session".to_string(),
                container_uuid: None,
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
                is_live_container: false,
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
            assert!(workspace.agent_action_sidecar_source.is_none());
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
                container_uuid: None,
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
                is_live_container: false,
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
                container_uuid: None,
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
                is_live_container: false,
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
                labels.contains(&crate::t!("workspace-agent-actions")),
                "indexed CLI rows must expose one Agent actions domain"
            );
            assert!(
                labels.contains(&crate::t!(
                    "workspace-session-bridge-export-bundle"
                )),
                "indexed CLI rows must expose a portable bundle export label"
            );
            assert!(matches!(
                workspace.agent_action_sidecar_source,
                Some(AgentActionSidecarSource::SessionBridge(
                    SessionBridgeActionSource::WorkspaceTarget { target: ref action_target }
                )) if action_target.session_id == target.session_id
            ));
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
fn test_session_navigator_display_order_normalizes_terminal_bootstrap_authority_variants() {
    let local = WorkspaceSessionSnapshot {
        id: "external:Codex:local".to_string(),
        container_uuid: None,
        kind: WorkspaceSessionKind::AgentTerminal,
        label: Some("Codex".to_string()),
        environment_authority_key: Some("local".to_string()),
        cwd: Some("/Users/admin/ashide".to_string()),
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
        is_live_container: false,
    };
    let local_with_root = WorkspaceSessionSnapshot {
        environment_authority_key: Some("local:/Users/admin/ashide".to_string()),
        ..local.clone()
    };

    assert_eq!(
        Workspace::workspace_session_display_order_key(&local),
        Workspace::workspace_session_display_order_key(&local_with_root)
    );
}

#[test]
fn test_hoa_onboarding_only_welcome_banner_blocks_workspace_interaction() {
    assert!(Workspace::hoa_onboarding_blocks_workspace_interaction(
        HoaOnboardingStep::WelcomeBanner
    ));
    assert!(!Workspace::hoa_onboarding_blocks_workspace_interaction(
        HoaOnboardingStep::VerticalTabsCallout
    ));
    assert!(!Workspace::hoa_onboarding_blocks_workspace_interaction(
        HoaOnboardingStep::AgentInboxCallout
    ));
    assert!(!Workspace::hoa_onboarding_blocks_workspace_interaction(
        HoaOnboardingStep::TabConfig
    ));
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
                container_uuid: None,
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
                is_live_container: false,
            };
            let live_session = WorkspaceSessionSnapshot {
                id: "tab:99:leaf:0".to_string(),
                container_uuid: None,
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
                is_live_container: false,
            };
            let authority = workspace.current_environment_authority_key(ctx);
            workspace
                .environments
                .commit_indexed_cli_agent_sessions(
                    &authority,
                    Ok::<_, String>(vec![indexed_session.clone()]),
                )
                .expect("complete local session index commit cannot fail");
            workspace.restored_workspace_sessions.push(live_session.clone());
            workspace.sync_session_navigator_sessions(ctx);

            let sessions = workspace.session_navigator_sessions();
            // Container model: the restored "live" session and the indexed
            // session share the same agent session id, so they merge into a
            // single virtual container row. Find it by agent session id.
            let live_row = sessions
                .iter()
                .find(|session| {
                    session.cli_agent_session_id.as_deref() == Some(cli_agent_session_id)
                        && session.environment_authority_key.as_deref() == Some("local")
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
                    } if action_target.session_id == live_row.id
                )),
                "selected running CLI row must expose a real fork action via its indexed backing source"
            );
            assert!(matches!(
                workspace.agent_action_sidecar_source,
                Some(AgentActionSidecarSource::SessionBridge(
                    SessionBridgeActionSource::WorkspaceTarget { target: ref action_target }
                )) if action_target.session_id == live_row.id
            ));
            assert!(
                actions.iter().any(|action| matches!(
                    action,
                    WorkspaceAction::ExportSessionBridgeBundle {
                        source: SessionBridgeActionSource::WorkspaceTarget { target: action_target },
                    } if action_target.session_id == live_row.id
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
            crate::terminal::cli_agent::CLIAgentInstallModel::handle(ctx).update(
                ctx,
                |model, _| model.set_installed_agents_for_test([CLIAgent::Claude]),
            );
            let server = test_ssh_server_for_environment_tests();
            let environment =
                crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                    "remote-fixture-primary".to_string(),
                    &server,
                    Some("/root/project".to_string()),
                    EnvironmentLifecycleState::Connected,
                );
            let authority = environment.authority_key.clone();
            workspace.set_active_tab_environment(environment.clone());
            let runtime_session_id = CoreSessionId::from(9173);
            workspace.mark_environment_runtime_connecting(
                environment,
                runtime_session_id,
                PathBuf::from("/tmp/lr172-remote-live-agent-actions.sock"),
            );
            workspace
                .mark_environment_runtime_connected_session(
                    runtime_session_id,
                    HostId::new("lr172-remote-live-agent-actions-host".to_string()),
                )
                .expect("remote live Agent actions fixture must be canonically connected");

            let cli_agent_session_id = "remote-claude-live-session";
            let remote_source = "/root/.claude/projects/-root-project/remote-claude-live-session.jsonl";
            let indexed_session = WorkspaceSessionSnapshot {
                id: crate::workspace::environment_runtime::environment_cli_agent_session_source_id(
                    &authority,
                    &CLIAgent::Claude,
                    remote_source,
                ),
                container_uuid: None,
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
                is_live_container: false,
            };
            let live_session = WorkspaceSessionSnapshot {
                id: "tab:88:leaf:0".to_string(),
                container_uuid: None,
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
                is_live_container: false,
            };
            workspace
                .commit_indexed_environment_cli_agent_sessions(
                    &authority,
                    Ok(vec![indexed_session.clone()]),
                )
                .expect("commit indexed remote session fixture");
            workspace.restored_workspace_sessions.push(live_session.clone());
            workspace.sync_session_navigator_sessions(ctx);

            let sessions = workspace.session_navigator_sessions();
            // Container model: the restored "live" session and the indexed
            // session share the same agent session id, so they merge into a
            // single virtual container row. Find it by agent session id.
            let live_row = sessions
                .iter()
                .find(|session| {
                    session.cli_agent_session_id.as_deref() == Some(cli_agent_session_id)
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
                        fork_target: SessionBridgeForkTarget::Ashide,
                    } if action_target.session_id == live_row.id
                        && action_target.environment_authority_key == authority
                )),
                "selected remote running CLI row must keep the Ashide fork with the owning remote authority"
            );
            assert!(matches!(
                workspace.agent_action_sidecar_source,
                Some(AgentActionSidecarSource::SessionBridge(
                    SessionBridgeActionSource::WorkspaceTarget { target: ref action_target }
                )) if action_target.session_id == live_row.id
                    && action_target.environment_authority_key == authority
            ));
            let sidecar_actions = workspace.agent_action_sidecar_items(
                workspace
                    .agent_action_sidecar_source
                    .clone()
                    .expect("remote Agent actions source must exist"),
                ctx,
            );
            let projected = sidecar_actions
                .iter()
                .filter_map(|item| match item.item_on_select_action() {
                    Some(NewSessionSidecarSelection::AgentAction {
                        agent: CLIAgent::Claude,
                        intent,
                        source:
                            AgentActionSidecarSource::SessionBridge(
                                SessionBridgeActionSource::WorkspaceTarget { target },
                            ),
                    }) if target.environment_authority_key == authority => Some(*intent),
                    Some(NewSessionSidecarSelection::AgentAction { .. }) | None => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(
                projected,
                vec![AgentActionIntent::Fork, AgentActionIntent::Edit]
            );
            assert!(
                actions.iter().any(|action| matches!(
                    action,
                    WorkspaceAction::ExportSessionBridgeBundle {
                        source: SessionBridgeActionSource::WorkspaceTarget { target: action_target },
                    } if action_target.session_id == live_row.id
                        && action_target.environment_authority_key == authority
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
                container_uuid: None,
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
                is_live_container: false,
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
            let sessions = workspace.session_navigator_sessions();
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
            assert!(matches!(
                &workspace.agent_action_sidecar_source,
                Some(AgentActionSidecarSource::SessionBridge(
                    SessionBridgeActionSource::Conversation {
                        conversation_id: action_conversation_id,
                        source_environment_authority_key,
                    }
                )) if *action_conversation_id == conversation_id
                    && source_environment_authority_key.as_deref()
                        == Some(expected_authority.as_str())
            ));
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

#[cfg(all(feature = "local_fs", not(target_family = "wasm")))]
#[test]
fn test_remote_authority_historical_conversation_edit_reuses_shared_session_bridge_source_reader() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let conversation_id = AIConversationId::new();
        let mut source_session =
            crate::session_bridge::ir::SessionIr::new_ashide(conversation_id.to_string());
        source_session.title = "Remote-context historical Ashide conversation".to_owned();
        source_session.project_path = Some("/srv/ashide".to_owned());
        source_session.messages = vec![
            crate::session_bridge::ir::SessionMessageIr {
                role: "user".to_owned(),
                text: "open the persisted draft".to_owned(),
                timestamp: None,
            },
            crate::session_bridge::ir::SessionMessageIr {
                role: "assistant".to_owned(),
                text: "persisted response".to_owned(),
                timestamp: None,
            },
        ];
        let import_source =
            crate::session_bridge::ashide_store::SessionBridgeImportSource::from_derived_session(
                "edit-source",
                &source_session.session_id,
                &source_session.session_id,
                &source_session,
            )
            .expect("historical source metadata should be valid");
        let mut conn = crate::persistence::establish_rw_connection()
            .expect("test app database should be available");
        crate::session_bridge::ashide_store::import_ashide_session_write_back(
            &mut conn,
            &source_session,
            import_source,
        )
        .expect("historical conversation should be persisted");
        drop(conn);

        let has_live_conversation = app.read(|ctx| {
            BlocklistAIHistoryModel::as_ref(ctx)
                .conversation(&conversation_id)
                .is_some()
        });
        assert!(
            !has_live_conversation,
            "test must exercise the SQLite cold-read path instead of the live IR fast path"
        );

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            workspace.show_session_bridge_edit_dialog(
                conversation_id,
                Some("ssh:session-bridge-edit-parity".to_owned()),
                SessionBridgeForkTarget::Ashide,
                ctx,
            );
        });

        futures_lite::future::yield_now().await;

        workspace.update(&mut app, |workspace, _| {
            assert!(
                workspace
                    .current_workspace_state
                    .is_session_bridge_edit_dialog_open,
                "runtime authority is execution/fork context and must not block the shared app-store Conversation source"
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

        let (initial_tab_count, expected_session_id) =
            workspace.update(&mut app, |workspace, ctx| {
                let expected_session_id =
                    Workspace::ashide_conversation_session_id(conversation_id);
                let session = workspace
                    .session_navigator_sessions()
                    .into_iter()
                    .find(|session| session.id == expected_session_id)
                    .expect("historical Ashide conversation should be restorable");
                let target = WorkspaceSessionActionTarget::new(
                    session.id.clone(),
                    session.environment_authority_key.clone(),
                );
                let initial_tab_count = workspace.tab_count();

                workspace.activate_restored_workspace_session(&target, ctx);
                (initial_tab_count, expected_session_id)
            });

        futures_lite::future::yield_now().await;

        workspace.update(&mut app, |workspace, ctx| {
            // Re-sync so the navigator picks up the newly materialized live
            // container and reconciles the active restored key onto it.
            workspace.sync_session_navigator_sessions(ctx);
        });

        workspace.update(&mut app, |workspace, ctx| {
            assert_eq!(workspace.tab_count(), initial_tab_count + 1);
            // Container model: the historical Ashide conversation has been
            // materialized into a live container. Its identity is now the
            // live tab's `source:tab:...` key, not the virtual
            // `source:ashide-conversation:...` key. The materialized live
            // container is the active row; the active restored key either
            // tracks it or is cleared (both are acceptable — the live tab is
            // highlighted via `is_active` regardless).
            let live_sessions = workspace.live_workspace_sessions(ctx);
            let active_live = live_sessions
                .iter()
                .find(|session| session.is_active)
                .expect("materialized historical conversation should have an active live container");
            let live_key = Workspace::workspace_session_logical_key(active_live);
            let navigator_state = workspace.snapshot_session_navigator_state();
            let live_row_id = Workspace::workspace_session_row_id(active_live, &navigator_state);
            let selected_row_id_ok = navigator_state
                .selected_row_id
                .as_deref()
                .map(|row_id| row_id == live_row_id || row_id.is_empty())
                .unwrap_or(true);
            assert!(
                selected_row_id_ok,
                "active restored key should track the materialized live container or be cleared; got {:?}, live row {live_row_id}, live key {live_key}, registry {:?}",
                navigator_state.selected_row_id,
                navigator_state.row_id_by_identity,
            );
            // The historical virtual container should not also appear as a
            // separate row — it has been consumed/represented by the live tab.
            let sessions = workspace.session_navigator_sessions();
            let historical_rows = sessions
                .iter()
                .filter(|session| session.id == expected_session_id)
                .count();
            assert_eq!(
                historical_rows, 0,
                "materialized historical conversation should not appear as a separate virtual row"
            );
            assert!(
                !navigator_state.restoring_row_ids.contains(
                    &Workspace::session_navigator_row_id_for_identity(
                        &expected_session_id,
                        &navigator_state,
                    ),
                ),
                "native Ashide historical sessions should not go through CLI resume restoring state"
            );
            assert!(
                !workspace
                    .snapshot_session_navigator_state()
                    .restoring_row_ids
                    .contains(&Workspace::workspace_session_logical_key(active_live)),
                "native Ashide historical sessions should not mark their logical key as CLI restoring"
            );
        });
    });
}

#[test]
fn test_session_navigator_historical_conversation_honors_shared_split_layout_preference() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        crate::util::file::external_editor::EditorSettings::handle(&app).update(
            &mut app,
            |settings, ctx| {
                settings
                    .open_conversation_layout_preference
                    .set_value(
                        crate::util::file::external_editor::settings::OpenConversationPreference::SplitPane,
                        ctx,
                    )
                    .expect("test layout preference should update");
            },
        );

        let conversation_id = AIConversationId::new();
        insert_historical_ashide_conversation(
            &mut app,
            conversation_id,
            "Historical Ashide split activation",
            "/Users/admin/ashide",
        );

        let workspace = mock_workspace(&mut app);
        let (initial_tab_count, initial_pane_count) =
            workspace.update(&mut app, |workspace, ctx| {
                let session_id = Workspace::ashide_conversation_session_id(conversation_id);
                let session = workspace
                    .session_navigator_sessions()
                    .into_iter()
                    .find(|session| session.id == session_id)
                    .expect("historical Ashide conversation should be restorable");
                let target = WorkspaceSessionActionTarget::new(
                    session.id.clone(),
                    session.environment_authority_key.clone(),
                );
                let initial_tab_count = workspace.tab_count();
                let initial_pane_count = workspace
                    .active_tab_pane_group()
                    .as_ref(ctx)
                    .visible_pane_ids()
                    .len();

                workspace.activate_restored_workspace_session(&target, ctx);
                (initial_tab_count, initial_pane_count)
            });

        futures_lite::future::yield_now().await;

        workspace.update(&mut app, |workspace, ctx| {
            assert_eq!(
                workspace.tab_count(),
                initial_tab_count,
                "Session Navigator historical Conversation activation must not override the shared SplitPane preference with NewTab"
            );
            assert_eq!(
                workspace
                    .active_tab_pane_group()
                    .as_ref(ctx)
                    .visible_pane_ids()
                    .len(),
                initial_pane_count + 1,
                "shared SplitPane preference must add exactly one pane in the target Environment tab"
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
                    "remote-fixture-primary".to_string(),
                    "ssh:ssh-config:remote-fixture-primary".to_string(),
                    Some("ssh-config:remote-fixture-primary".to_string()),
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
            install_test_saved_ssh_target_catalog(&server, ctx);
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9073);
            let host_id = HostId::new("skill-manager-placeholder-host".to_string());

            workspace.add_test_environment_runtime_placeholder_tab(
                environment.clone(),
                Some("root@remote-fixture-primary".to_string()),
                ctx,
            );
            let environment_tab_index = workspace
                .tab_index_for_environment_authority(&authority)
                .expect("test setup should create an Environment placeholder tab");
            workspace.environments_mut().mark_connecting(
                environment,
                session_id,
                PathBuf::from("/tmp/ashide-test-skill-manager-placeholder.sock"),
            );
            workspace
                .environments_mut()
                .mark_connected_session(session_id, host_id.clone());
            workspace.activate_tab_internal(environment_tab_index, ctx);
            workspace.update_active_session(ctx);

            let runtime_session_id = workspace
                .environment_runtime_session_for_authority(&authority)
                .expect("activating a placeholder with a missing runtime client should reconnect");
            assert_eq!(
                runtime_session_id, session_id,
                "reconnect must preserve the Environment-owned SessionId while replacing only its stale transport generation"
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
fn test_skill_manager_observation_rejects_stale_client_then_reconnects() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            install_test_saved_ssh_target_catalog(&server, ctx);
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let stale_session_id = CoreSessionId::from(9074);
            workspace.environments_mut().mark_connecting(
                environment.clone(),
                stale_session_id,
                PathBuf::from("/tmp/ashide-test-skill-manager-stale-client.sock"),
            );
            workspace.environments_mut().mark_connected_session(
                stale_session_id,
                HostId::new("skill-manager-stale-host".to_string()),
            );
            workspace.set_active_tab_environment(environment);

            let scope = workspace.active_skill_manager_environment_scope(ctx.window_id(), ctx);
            match scope {
                crate::workspace::SkillManagerEnvironmentScope::EnvironmentRuntime(scope) => {
                    assert_eq!(
                        scope.session_id, None,
                        "Skill Manager must fail closed instead of exposing a stale SessionId"
                    );
                    assert_eq!(
                        scope.host_id, None,
                        "Skill Manager must fail closed instead of exposing a stale HostId"
                    );
                }
                crate::workspace::SkillManagerEnvironmentScope::CurrentApp => {
                    panic!("runtime Skill Manager must not fall back to current-app")
                }
            }

            workspace.reconnect_active_skill_manager_environment_if_stale(ctx.window_id(), ctx);

            let owner_session_id = workspace
                .environment_runtime_session_for_authority(&authority)
                .expect("Skill Manager stale-client observation should trigger reconnect");
            assert_eq!(owner_session_id, stale_session_id);
            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                Some(EnvironmentLifecycleState::Connecting)
            );
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
                    "remote-fixture-primary".to_string(),
                    "ssh:ssh-config:remote-fixture-primary".to_string(),
                    Some("ssh-config:remote-fixture-primary".to_string()),
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
                    "remote-fixture-primary".to_string(),
                    "ssh:ssh-config:remote-fixture-primary".to_string(),
                    Some("ssh-config:remote-fixture-primary".to_string()),
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
fn environment_snapshot_runtime_connection_ref_uses_shared_authority_parser() {
    let snapshot = |authority_key: &str, connection_ref: Option<&str>| {
        EnvironmentSnapshot::runtime_transport(
            EnvironmentKind::Ssh,
            "runtime".to_owned(),
            authority_key.to_owned(),
            connection_ref.map(str::to_owned),
            None,
            EnvironmentLifecycleState::Dormant,
        )
    };

    assert_eq!(
        snapshot("ssh:node-1", None).runtime_connection_ref(),
        Some("node-1")
    );
    assert_eq!(
        snapshot("ssh:ssh-config:remote-fixture-dev", None).runtime_connection_ref(),
        Some("ssh-config:remote-fixture-dev")
    );
    assert_eq!(
        snapshot("ssh-config:remote-fixture-dev", None).runtime_connection_ref(),
        Some("ssh-config:remote-fixture-dev")
    );
    assert_eq!(snapshot("local:/repo", None).runtime_connection_ref(), None);
    assert_eq!(
        snapshot("ssh:ssh-config:authority", Some("ssh-config:explicit")).runtime_connection_ref(),
        Some("ssh-config:explicit"),
        "persisted provider connection_ref must take precedence over authority fallback"
    );
}

#[test]
fn test_restored_terminal_bootstrap_startup_command_matches_current_app_restore_cwd_and_pending_resume(
) {
    let session = WorkspaceSessionSnapshot {
        id: "tab:1:leaf:0".to_string(),
        container_uuid: None,
        kind: WorkspaceSessionKind::AgentTerminal,
        label: Some("Claude remote".to_string()),
        environment_authority_key: Some("ssh:ssh-config:remote-fixture-dev".to_string()),
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
        is_live_container: false,
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
                None,
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
                .session_navigator_sessions()
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
                None,
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
                .session_navigator_sessions()
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
                    "remote-fixture-primary".to_string(),
                    &server,
                    Some("/root/project".to_string()),
                    EnvironmentLifecycleState::Connected,
                );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9018);
            workspace.environments_mut().mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-remote-native-fork.sock"),
            );
            workspace
                .environments_mut()
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
                None,
                "Fork 远程 CLI 会话失败".to_owned(),
                ctx,
            );

            let sessions = workspace.session_navigator_sessions();
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
            let navigator_state = workspace.snapshot_session_navigator_state();
            let forked_row_id = Workspace::workspace_session_row_id(
                forked_session,
                &navigator_state,
            );
            assert_eq!(
                navigator_state.selected_row_id.as_deref(),
                Some(forked_row_id.as_str())
            );
            assert_eq!(
                workspace.latest_pending_session_restore_for_authority(&authority)
                    .and_then(|pending| pending.resume_command.as_deref()),
                Some("claude --resume remote-claude-fork-session"),
                "remote native fork must queue provider resume on the remote runtime authority"
            );
            assert_eq!(
                workspace.tab_count(),
                initial_tab_count + 1,
                "remote native fork must allocate a dedicated remote runtime container"
            );
            assert_eq!(
                workspace.current_environment_authority_key(ctx),
                authority,
                "remote native fork must keep the new container owned by the remote authority"
            );
            assert!(
                workspace.active_tab_contains_environment_runtime_placeholder(ctx),
                "remote native fork must wait in a runtime placeholder instead of creating a current-app terminal"
            );
            let persisted_snapshot = workspace.snapshot(ctx.window_id(), false, ctx);
            assert!(
                persisted_snapshot.restored_workspace_sessions.iter().any(|session| {
                    session.id.starts_with("remote:")
                        && session.environment_authority_key.as_deref() == Some(authority.as_str())
                        && session.cli_agent_session_id.as_deref()
                            == Some("remote-claude-fork-session")
                }),
                "remote native fork backing source must be persisted with its remote authority so restart does not downgrade it to a local tab row; persisted_sessions={:#?}",
                persisted_snapshot.restored_workspace_sessions
            );

            workspace.set_active_tab_environment(
                crate::workspace::environment_runtime::terminal_bootstrap_environment(None),
            );
            let local_sessions = workspace.session_navigator_sessions();
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

        let (initial_tab_count, initial_active_tab) =
            workspace.update(&mut app, |workspace, ctx| {
                let initial_tab_count = workspace.tab_count();
                let initial_active_tab = workspace.active_tab_index();

                workspace.finish_session_bridge_fork(
                    Ok(SessionBridgeForkWriteBack::Ashide(write_back)),
                    None,
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
fn test_remote_session_bridge_fork_to_ashide_preserves_explicit_source_authority_for_delivery() {
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
            crate::session_bridge::ir::SessionIr::new_ashide("remote-codex-source".to_owned());
        source_session.source = "codex".to_owned();
        source_session.title = "Remote Codex source".to_owned();
        source_session.project_path = Some("/srv/ashide".to_owned());
        source_session.messages = vec![crate::session_bridge::ir::SessionMessageIr {
            role: "user".to_owned(),
            text: "fork this remote session into Ashide".to_owned(),
            timestamp: None,
        }];
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
        let authority = "ssh:session-bridge-completion".to_owned();
        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "session-bridge-completion".to_owned(),
                &server,
                Some("/srv/ashide".to_owned()),
                EnvironmentLifecycleState::Connecting,
            );
            assert_eq!(environment.authority_key, authority);
            workspace.environments_mut().mark_connecting(
                environment,
                CoreSessionId::from(9914),
                PathBuf::from("/tmp/ashide-test-session-bridge-completion.sock"),
            );

            workspace.finish_session_bridge_fork(
                Ok(SessionBridgeForkWriteBack::Ashide(write_back)),
                Some(authority.clone()),
                "Fork 会话失败".to_owned(),
                ctx,
            );
        });

        for _ in 0..4 {
            futures_lite::future::yield_now().await;
        }

        workspace.update(&mut app, |workspace, ctx| {
            assert!(
                BlocklistAIHistoryModel::as_ref(ctx)
                    .conversation(&target_conversation_id)
                    .is_some(),
                "Fork-to-Ashide write-back must commit to the current-app Conversation store"
            );
            let pending = workspace
                .pending_forked_conversation_for_authority(&authority)
                .expect("explicit remote source authority must own the deferred delivery");
            assert_eq!(pending.conversation.id(), target_conversation_id);
            assert!(
                workspace
                    .pending_forked_conversation_for_authority(
                        crate::environment_authority::TERMINAL_BOOTSTRAP_AUTHORITY,
                    )
                    .is_none(),
                "completion must not infer delivery from the ambient local Environment"
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
                None,
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
                .session_navigator_sessions()
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
        container_uuid: None,
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
        is_live_container: false,
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
    let mut server =
        warp_ssh_manager::SshServerInfo::new_default("remote-fixture-primary".to_string());
    server.host = "remote-fixture-primary".to_string();
    server.username = "root".to_string();
    server
}

fn install_test_saved_ssh_target_catalog(
    server: &warp_ssh_manager::SshServerInfo,
    ctx: &mut warpui::ViewContext<Workspace>,
) {
    crate::ssh_manager::SshTargetCatalog::handle(ctx).update(ctx, |catalog, _| {
        let generation = catalog.begin_refresh_for_test(
            crate::ssh_manager::SshTargetCatalogRefreshIntent::ExplicitRefresh,
        );
        assert!(catalog.finish_refresh_for_test(
            generation,
            Ok(SshTargetCatalogSnapshot::merge(
                warp_ssh_manager::LoadResult {
                    path: None,
                    outcome: warp_ssh_manager::LoadOutcome::NotFound,
                    has_unexpanded_includes: false,
                },
                vec![("Remote Fixture Primary".to_string(), server.clone())],
            )),
        ));
    });
}

fn unavailable_test_ssh_config_catalog() -> crate::ssh_manager::SshTargetCatalog {
    crate::ssh_manager::SshTargetCatalog::with_snapshot(warp_ssh_manager::LoadResult {
        path: None,
        outcome: warp_ssh_manager::LoadOutcome::Error("test SSH config unavailable".into()),
        has_unexpanded_includes: false,
    })
}

fn unavailable_test_ssh_config_catalog_with_saved_fixture() -> crate::ssh_manager::SshTargetCatalog
{
    crate::ssh_manager::SshTargetCatalog::with_catalog_snapshot(SshTargetCatalogSnapshot::merge(
        warp_ssh_manager::LoadResult {
            path: None,
            outcome: warp_ssh_manager::LoadOutcome::Error("test SSH config unavailable".into()),
            has_unexpanded_includes: false,
        },
        vec![(
            "Remote Fixture Primary".to_string(),
            test_ssh_server_for_environment_tests(),
        )],
    ))
}

#[test]
fn test_workspace_environment_fixture_uses_saved_provider_without_user_ssh_config() {
    let catalog = unavailable_test_ssh_config_catalog_with_saved_fixture();

    let descriptor =
        crate::workspace::environment_provider::runtime_transport_descriptor_for_connection_ref(
            "remote-fixture-primary",
            &catalog,
        )
        .expect(
            "shared Workspace Environment fixture must resolve from the committed saved-provider catalog",
        );

    assert_eq!(descriptor.connection_ref(), "remote-fixture-primary");
    assert_eq!(descriptor.host_label(), "remote-fixture-primary");
    assert_eq!(descriptor.target(), "root@remote-fixture-primary");
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
        container_uuid: None,
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
        is_live_container: false,
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
        container_uuid: None,
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
        is_live_container: false,
    }
}

fn test_session_navigator_displayed_order(workspace: &Workspace) -> Vec<&'static str> {
    workspace
        .session_navigator_sessions()
        .iter()
        .filter_map(|session| match session.id.as_str() {
            "order-key-resume-a" => Some("order-key-resume-a"),
            "order-key-resume-b" => Some("order-key-resume-b"),
            "order-key-append-a" => Some("order-key-append-a"),
            "order-key-append-b" => Some("order-key-append-b"),
            "order-key-append-c" => Some("order-key-append-c"),
            "order-key-hidden-a" => Some("order-key-hidden-a"),
            "order-key-hidden-b" => Some("order-key-hidden-b"),
            "order-key-hidden-c" => Some("order-key-hidden-c"),
            "order-key-pin-a" => Some("order-key-pin-a"),
            "order-key-pin-b" => Some("order-key-pin-b"),
            "order-key-pin-c" => Some("order-key-pin-c"),
            _ => None,
        })
        .collect()
}

fn test_pending_environment_runtime_session_restore(authority: &str) -> SessionRestoreEntry {
    SessionRestoreEntry {
        session: test_environment_runtime_session_snapshot(
            "environment-runtime-pending-restore",
            authority,
        ),
        resume_command: Some("codex resume codex-session".to_string()),
    }
}

fn active_test_pane_id(workspace: &Workspace, ctx: &AppContext) -> PaneId {
    workspace
        .active_tab_pane_group()
        .as_ref(ctx)
        .focused_pane_id(ctx)
}

fn test_pending_environment_runtime_forked_conversation_entry() -> ForkEntry {
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
                    container_uuid: None,
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
                is_live_container: false,
                });

            workspace.open_environment_runtime_from_provider(
                environment_provider::source_saved_ssh::target_from_server(
                    server.node_id.clone(),
                    server,
                ),
                ctx,
            );

            let cached_ids = workspace
                .session_navigator_sessions()
                .into_iter()
                .map(|session| session.id)
                .collect::<Vec<_>>();
            assert!(cached_ids.iter().any(|id| id == "ssh-manager-session"));
        });
    });
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn test_open_environment_runtime_provider_without_startup_queues_plain_terminal_intent() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            assert!(
                server.startup_command.is_none(),
                "fixture must exercise provider open without a startup command"
            );
            let target = environment_provider::source_saved_ssh::target_from_server(
                server.node_id.clone(),
                server,
            );
            let authority = target.dormant_environment(None).authority_key;
            let tab_count_before = workspace.tab_count();

            workspace.open_environment_runtime_from_provider(target.clone(), ctx);

            assert_eq!(workspace.tab_count(), tab_count_before + 1);
            assert!(
                workspace.has_pending_terminal_for_authority(&authority),
                "a newly allocated provider placeholder must own the shared PlainTerminal intent before transport Connected"
            );
            assert_eq!(
                workspace
                    .environments
                    .entry_for_authority(&authority)
                    .expect("provider open must register its Environment owner")
                    .pending_materializations
                    .len(),
                1,
                "the first provider container must own exactly one terminal materialization"
            );

            workspace.open_environment_runtime_from_provider(target, ctx);

            assert_eq!(
                workspace.tab_count(),
                tab_count_before + 1,
                "activating an existing provider container must not create another terminal tab"
            );
            assert_eq!(
                workspace
                    .environments
                    .entry_for_authority(&authority)
                    .expect("provider Environment owner must remain registered")
                    .pending_materializations
                    .len(),
                1,
                "re-activation must not duplicate the pane-owned terminal intent"
            );
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
                    .pending_startup_command_for_authority(&authority)
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
            workspace.environments_mut().mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-ssh-control.sock"),
            );
            workspace
                .environments_mut()
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
                workspace.current_environment_snapshot()
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
                .environments_mut()
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
                workspace.current_environment_snapshot()
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
                .environments_mut()
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
            workspace.sync_session_navigator_sessions(ctx);
            assert!(
                workspace
                    .session_navigator_sessions()
                    .iter()
                    .all(|session| session.environment_authority_key.as_deref() != Some(authority.as_str())),
                "disconnect must clear the authority-scoped remote session cache instead of leaving a local persistent copy"
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
            install_test_saved_ssh_target_catalog(&server, ctx);
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
                .environments_mut()
                .mark_connected_session(stale_session_id, HostId::new("disconnect-runtime-host".to_string()));
            workspace.set_active_tab_environment(environment);

            workspace.handle_environment_runtime_disconnected(
                stale_session_id,
                crate::workspace::environment_runtime::EnvironmentRuntimeDisconnectCause::TransportFailure,
                ctx,
            );

            assert!(
                workspace
                    .session_navigator_sessions()
                    .iter()
                    .all(|session| session.environment_authority_key.as_deref() != Some(authority.as_str())),
                "transport disconnect must clear the authority-scoped session cache before reconnect"
            );

            let owner_session_id = workspace
                .environment_runtime_session_for_authority(&authority)
                .expect("retained Environment should be re-registered after transport disconnect");
            assert_eq!(
                owner_session_id, stale_session_id,
                "transport disconnect must preserve the retained Environment owner while replacing its dead transport"
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
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9011);
            workspace.environments_mut().mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-ssh-control-direct-add.sock"),
            );
            workspace
                .environments_mut()
                .mark_connected_session(session_id, HostId::new("test-host".to_string()));
            workspace.set_active_tab_environment(environment);

            workspace.add_terminal_tab(false, ctx);

            assert_eq!(workspace.tab_count(), 2);
            assert!(
                workspace.has_pending_terminal_for_authority(&authority),
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
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9013);
            workspace.environments_mut().mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-ssh-control-session-row.sock"),
            );
            workspace
                .environments_mut()
                .mark_connected_session(session_id, HostId::new("test-host".to_string()));
            workspace.set_active_tab_environment(environment);

            workspace.handle_action(
                &WorkspaceAction::AddTerminalTab {
                    hide_homepage: false,
                },
                ctx,
            );

            let live_sessions = workspace.live_workspace_sessions(ctx);
            workspace.sync_session_navigator_sessions(ctx);
            let sessions = workspace.session_navigator_sessions();
            let current_environment = workspace.current_environment_snapshot().clone();
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

            let left_panel_sessions = workspace.session_navigator_sessions();
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
                "ssh:ssh-config:remote-fixture-primary",
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
            install_test_saved_ssh_target_catalog(&server, ctx);
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let stale_session_id = CoreSessionId::from(9014);
            workspace.environments_mut().mark_connecting(
                environment.clone(),
                stale_session_id,
                PathBuf::from("/tmp/ashide-test-stale-connected.sock"),
            );
            workspace
                .environments_mut()
                .mark_connected_session(stale_session_id, HostId::new("stale-host".to_string()));
            workspace.set_active_tab_environment(environment);

            assert!(
                crate::workspace::environment_runtime::client_for_session(stale_session_id, ctx)
                    .is_none(),
                "test setup must represent a persisted Connected Environment whose runtime client/proxy is gone"
            );

            workspace.ensure_current_environment_runtime_transport_if_needed(ctx);

            let owner_session_id = workspace
                .environment_runtime_session_for_authority(&authority)
                .expect("stale connected Environment should be re-registered for reconnect");
            assert_eq!(
                owner_session_id, stale_session_id,
                "Connected without a runtime client must not be treated as active; ensure must restart transport on the retained runtime owner"
            );
            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                Some(EnvironmentLifecycleState::Connecting),
                "stale Connected Environment should move back to Connecting while the runtime proxy is restarted"
            );
            assert_eq!(
                workspace.current_environment_snapshot()
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
            install_test_saved_ssh_target_catalog(&server, ctx);
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let stale_session_id = CoreSessionId::from(9024);
            let stale_host_id = HostId::new("stale-browser-host".to_string());
            workspace.environments_mut().mark_connecting(
                environment.clone(),
                stale_session_id,
                PathBuf::from("/tmp/ashide-test-browser-unavailable.sock"),
            );
            workspace
                .environments_mut()
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

            let owner_session_id = workspace
                .environment_runtime_session_for_authority(&authority)
                .expect("browser unavailable event should trigger runtime reconnect");
            assert_eq!(
                owner_session_id, stale_session_id,
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
fn test_restored_error_environment_browser_unavailable_stays_error() {
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

            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                Some(EnvironmentLifecycleState::Error),
                "restoring the persisted tab must preserve Error in the authoritative environment table"
            );

            workspace.handle_left_panel_event(
                &LeftPanelEvent::ServerFileBrowser(
                    crate::workspace::view::server_file_browser::ServerFileBrowserEvent::EnvironmentRuntimeUnavailable {
                        session_id: None,
                        host_id: None,
                    },
                ),
                ctx,
            );

            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                Some(EnvironmentLifecycleState::Error),
                "cold-start browser refresh must not reinterpret a persisted Error as Dormant and reconnect"
            );
            assert_eq!(
                workspace.environment_runtime_session_for_authority(&authority),
                None,
                "implicit browser refresh must not create a transport generation for persisted Error"
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
            install_test_saved_ssh_target_catalog(&server, ctx);
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let stale_session_id = CoreSessionId::from(9025);
            workspace.environments_mut().mark_connecting(
                environment.clone(),
                stale_session_id,
                PathBuf::from("/tmp/ashide-test-left-panel-sync-stale.sock"),
            );
            workspace
                .environments_mut()
                .mark_connected_session(stale_session_id, HostId::new("stale-left-panel-host".to_string()));
            workspace.set_active_tab_environment(environment);

            workspace.sync_environment_runtime_left_panel_roots(ctx);

            let owner_session_id = workspace
                .environment_runtime_session_for_authority(&authority)
                .expect("left panel sync should trigger runtime reconnect");
            assert_eq!(
                owner_session_id, stale_session_id,
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
            install_test_saved_ssh_target_catalog(&server, ctx);
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
                workspace.current_environment_snapshot()
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

            workspace.add_test_environment_runtime_placeholder_tab(
                environment,
                Some("root@remote-fixture-primary".to_string()),
                ctx,
            );

            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                Some(EnvironmentLifecycleState::Dormant),
                "persisted preparing state has no live runtime task after restore and must not remain active"
            );
            assert_eq!(
                workspace.current_environment_snapshot()
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

            workspace.add_test_environment_runtime_placeholder_tab(
                environment.clone(),
                Some("root@remote-fixture-primary".to_string()),
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
                workspace.current_environment_snapshot()
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
                EnvironmentRuntimeTransportPreparation::Ensure,
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
fn test_error_environment_runtime_does_not_implicitly_retry() {
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
                workspace.current_environment_snapshot()
                    .as_ref()
                    .map(|environment| &environment.lifecycle_state),
                Some(&EnvironmentLifecycleState::Error),
                "implicit ensure must leave the visible Environment error intact"
            );
        });
    });
}

#[test]
fn test_reselect_failed_environment_runtime_tab_explicitly_reconnects() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            install_test_saved_ssh_target_catalog(&server, ctx);
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Dormant,
            );
            let authority = environment.authority_key.clone();

            workspace.add_test_environment_runtime_placeholder_tab(
                environment.clone(),
                Some("root@remote-fixture-primary".to_string()),
                ctx,
            );
            let environment_tab_index = workspace
                .tab_index_for_environment_authority(&authority)
                .expect("test setup should create a failed Environment tab");
            let failed_session_id = CoreSessionId::from(9088);
            workspace.mark_environment_runtime_connecting(
                environment,
                failed_session_id,
                PathBuf::from("/tmp/ashide-test-explicit-reselect.sock"),
            );
            workspace.handle_environment_runtime_failed(
                failed_session_id,
                "synthetic first connection failure".to_string(),
                ctx,
            );
            workspace.activate_tab_internal(0, ctx);
            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                Some(EnvironmentLifecycleState::Error)
            );
            assert_eq!(
                workspace.environment_runtime_session_for_authority(&authority),
                Some(failed_session_id),
                "a failed Environment must retain the failed generation until explicit reconnect"
            );

            workspace.handle_action(&WorkspaceAction::ActivateTab(environment_tab_index), ctx);

            assert!(
                workspace.has_pending_environment_runtime_entry_for_authority(&authority),
                "explicit activation must retain the placeholder terminal intent across reconnect"
            );
            let reconnect_session_id = workspace
                .environment_runtime_session_for_authority(&authority)
                .expect("reselecting a failed Environment must allocate a runtime session");
            assert_eq!(
                reconnect_session_id, failed_session_id,
                "explicit reconnect must advance the transport generation under the stable Environment owner"
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
                Some(&EnvironmentLifecycleState::Connecting),
                "the visible Environment tab must leave Error as part of the explicit reconnect transition"
            );
        });
    });
}

#[test]
fn test_switch_environment_action_reconnects_failed_runtime() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            install_test_saved_ssh_target_catalog(&server, ctx);
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Dormant,
            );
            let authority = environment.authority_key.clone();

            workspace.add_test_environment_runtime_placeholder_tab(
                environment.clone(),
                Some("root@remote-fixture-primary".to_string()),
                ctx,
            );
            let environment_tab_index = workspace
                .tab_index_for_environment_authority(&authority)
                .expect("test setup should create a failed Environment tab");
            let failed_session_id = CoreSessionId::from(9089);
            workspace.mark_environment_runtime_connecting(
                environment,
                failed_session_id,
                PathBuf::from("/tmp/ashide-test-switch-environment-reselect.sock"),
            );
            workspace.handle_environment_runtime_failed(
                failed_session_id,
                "synthetic Environment Strip connection failure".to_string(),
                ctx,
            );
            workspace.activate_tab_internal(0, ctx);
            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                Some(EnvironmentLifecycleState::Error)
            );

            workspace.handle_action(
                &WorkspaceAction::SwitchEnvironment {
                    authority_key: authority.clone(),
                },
                ctx,
            );

            assert_eq!(workspace.active_tab_index(), environment_tab_index);
            let reconnect_session_id = workspace
                .environment_runtime_session_for_authority(&authority)
                .expect("Environment Strip reselect must allocate a runtime session");
            assert_eq!(
                reconnect_session_id, failed_session_id,
                "Environment Strip reselect must preserve the Environment owner while replacing transport resources"
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
                Some(&EnvironmentLifecycleState::Connecting),
                "the Environment chip action must route through the shared user-visible activation boundary"
            );
        });
    });
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn environment_runtime_control_paths_are_session_owned_for_same_authority() {
    let authority = "ssh:ssh-config:remote-fixture-secondary";
    let first = Workspace::environment_runtime_control_path(authority, CoreSessionId::from(9101));
    let second = Workspace::environment_runtime_control_path(authority, CoreSessionId::from(9102));

    assert_ne!(
        first, second,
        "two Environment owners for one authority must not share a ControlMaster socket"
    );
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn environment_runtime_control_path_is_stable_and_short_for_owner_session() {
    let authority = "ssh:898d71c9-74f9-4a41-ac98-1ece5f485b7b";
    let session_id = CoreSessionId::from(9103);
    let first = Workspace::environment_runtime_control_path(authority, session_id);
    let second = Workspace::environment_runtime_control_path(authority, session_id);
    let path_string = first.display().to_string();

    assert_eq!(first, second, "one Environment owner needs a stable path");

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
            install_test_saved_ssh_target_catalog(&server, ctx);
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Dormant,
            );
            let authority = environment.authority_key.clone();

            workspace.add_test_environment_runtime_placeholder_tab(
                environment,
                Some("root@remote-fixture-primary".to_string()),
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
                workspace.current_environment_snapshot()
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
    install_test_saved_ssh_target_catalog(&server, ctx);
    let environment =
        crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
            server.node_id.clone(),
            &server,
            Some("/root/project".to_string()),
            EnvironmentLifecycleState::Dormant,
        );
    let authority = environment.authority_key.clone();
    workspace.add_test_environment_runtime_placeholder_tab(
        environment,
        Some("root@remote-fixture-primary".to_string()),
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
            .current_environment_snapshot()
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
            workspace.add_test_environment_runtime_placeholder_tab(
                environment.clone(),
                Some("root@remote-fixture-primary".to_string()),
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
fn test_stale_client_reconnect_failure_preserves_pane_owned_restore_intent() {
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
            let pending_restore = test_pending_environment_runtime_session_restore(&authority);
            workspace.queue_pending_environment_runtime_session_restore(
                &authority,
                pending_restore.clone(),
                active_test_pane_id(workspace, ctx),
            );

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

            let owner_session_id = workspace
                .environment_runtime_session_for_authority(&authority)
                .expect("missing-client connected event should trigger a fresh runtime bootstrap");
            assert_eq!(
                owner_session_id, stale_session_id,
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
            let retained = workspace
                .latest_pending_session_restore_for_authority(&authority)
                .expect("pane-owned restore must remain queued across stale-client reconnect");
            assert_eq!(retained.session.id, pending_restore.session.id);
            assert_eq!(retained.resume_command, pending_restore.resume_command);
        });
    });
}

#[test]
fn test_environment_runtime_pending_materializations_are_pane_owned_queue() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, _ctx| {
            let authority = "ssh:ssh-config:remote-fixture-primary".to_string();
            let first_pane_id: PaneId = TerminalPaneId::dummy_terminal_pane_id().into();
            let second_pane_id: PaneId = TerminalPaneId::dummy_terminal_pane_id().into();
            let unrelated_pane_id: PaneId = TerminalPaneId::dummy_terminal_pane_id().into();
            let first_terminal_pane_id: PaneId = TerminalPaneId::dummy_terminal_pane_id().into();
            let second_terminal_pane_id: PaneId = TerminalPaneId::dummy_terminal_pane_id().into();
            let mut first_restore = test_pending_environment_runtime_session_restore(&authority);
            first_restore.session.id = "remote-restore-first".to_owned();
            let mut second_restore = test_pending_environment_runtime_session_restore(&authority);
            second_restore.session.id = "remote-restore-second".to_owned();

            workspace.queue_environment_runtime_intent(
                &authority,
                EnvironmentEntryIntent::SessionRestore(first_restore.clone()),
                first_pane_id,
            );
            workspace.queue_environment_runtime_intent(
                &authority,
                EnvironmentEntryIntent::SessionRestore(second_restore.clone()),
                second_pane_id,
            );

            let entry = workspace
                .environments
                .entry_for_authority(&authority)
                .expect("queue must create the authority row");
            assert_eq!(entry.pending_materializations.len(), 2);
            let mut pending = entry.pending_materializations.iter();
            let first = pending.next().expect("first restore must remain queued");
            let second = pending.next().expect("second restore must remain queued");
            assert_eq!(first.pane_id(), first_pane_id);
            assert_eq!(second.pane_id(), second_pane_id);
            assert!(matches!(
                &first.intent,
                EnvironmentEntryIntent::SessionRestore(restore)
                    if restore.session.id == first_restore.session.id
            ));
            assert!(matches!(
                &second.intent,
                EnvironmentEntryIntent::SessionRestore(restore)
                    if restore.session.id == second_restore.session.id
            ));

            assert!(workspace
                .environments
                .begin_materialization(&authority, unrelated_pane_id, first_terminal_pane_id,)
                .is_none());
            assert_eq!(
                workspace
                    .environments
                    .entry_for_authority(&authority)
                    .expect("failed or mismatched materialization must preserve queue")
                    .pending_materializations
                    .len(),
                2
            );

            let first_transition = workspace
                .environments
                .begin_materialization(&authority, first_pane_id, first_terminal_pane_id)
                .expect("first pane must enter materializing");
            let second_transition = workspace
                .environments
                .begin_materialization(&authority, second_pane_id, second_terminal_pane_id)
                .expect("second pane must enter materializing");
            assert_eq!(
                workspace
                    .environments
                    .entry_for_authority(&authority)
                    .expect("terminal creation must not commit pending intents")
                    .pending_materializations
                    .len(),
                2
            );

            let SessionRestoreFinalizeResult::Applied(consumed_second) =
                workspace.environments.finalize_session_restore(
                    second_transition,
                    SessionRestoreFinalizeOutcome::Success,
                )
            else {
                panic!("second pane may bootstrap independently of the first");
            };
            assert_eq!(consumed_second.pane_id(), second_terminal_pane_id);
            assert!(matches!(
                consumed_second.intent,
                EnvironmentEntryIntent::SessionRestore(restore)
                    if restore.session.id == second_restore.session.id
            ));

            let SessionRestoreFinalizeResult::Applied(consumed_first) = workspace
                .environments
                .finalize_session_restore(first_transition, SessionRestoreFinalizeOutcome::Success)
            else {
                panic!("first pane commits only after its own bootstrap");
            };
            assert_eq!(consumed_first.pane_id(), first_terminal_pane_id);
            assert!(matches!(
                consumed_first.intent,
                EnvironmentEntryIntent::SessionRestore(restore)
                    if restore.session.id == first_restore.session.id
            ));
            assert!(!workspace.has_pending_environment_runtime_entry_for_authority(&authority));
        });
    });
}

#[test]
fn test_environment_runtime_pane_close_reconciles_only_orphaned_pending_request() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        let (pane_group, authority, closed_pane_id, surviving_pane_id) =
            workspace.update(&mut app, |workspace, ctx| {
                let server = test_ssh_server_for_environment_tests();
                let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                    "remote-fixture-primary".to_string(),
                    &server,
                    Some("/root/project".to_string()),
                    EnvironmentLifecycleState::Connecting,
                );
                let authority = environment.authority_key.clone();
                workspace.add_test_environment_runtime_placeholder_tab(
                    environment,
                    Some("root@remote-fixture-primary".to_string()),
                    ctx,
                );
                let pane_group = workspace.active_tab_pane_group().clone();
                let closed_pane_id = pane_group.as_ref(ctx).focused_pane_id(ctx);
                let surviving_pane_id = pane_group.update(ctx, |pane_group, ctx| {
                    pane_group.add_loading_conversation_pane(Direction::Right, None, ctx)
                });

                workspace.queue_environment_runtime_intent(
                    &authority,
                    EnvironmentEntryIntent::PlainTerminal(PlainTerminalEntry::default_tab(false)),
                    closed_pane_id,
                );
                workspace.queue_environment_runtime_intent(
                    &authority,
                    EnvironmentEntryIntent::PlainTerminal(PlainTerminalEntry::default_tab(false)),
                    surviving_pane_id,
                );

                (pane_group, authority, closed_pane_id, surviving_pane_id)
            });

        pane_group.update(&mut app, |pane_group, ctx| {
            pane_group.close_pane(closed_pane_id, ctx);
        });

        workspace.read(&app, |workspace, _ctx| {
            let entry = workspace
                .environments
                .entry_for_authority(&authority)
                .expect("authority row must survive one placeholder close");
            assert_eq!(entry.pending_materializations.len(), 1);
            assert_eq!(
                entry
                    .pending_materializations
                    .front()
                    .expect("second request must remain queued")
                    .pane_id(),
                surviving_pane_id
            );
        });
    });
}

#[test]
fn test_environment_runtime_materializing_terminal_close_reconciles_pending_request() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        let (pane_group, authority, materializing_pane_id) =
            workspace.update(&mut app, |workspace, ctx| {
                let server = test_ssh_server_for_environment_tests();
                let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                    "remote-fixture-primary".to_string(),
                    &server,
                    Some("/root/project".to_string()),
                    EnvironmentLifecycleState::Connecting,
                );
                let authority = environment.authority_key.clone();
                workspace.add_test_environment_runtime_placeholder_tab(
                    environment,
                    Some("root@remote-fixture-primary".to_string()),
                    ctx,
                );
                let pane_group = workspace.active_tab_pane_group().clone();
                let queued_pane_id = pane_group.as_ref(ctx).focused_pane_id(ctx);
                let materializing_pane_id = pane_group.update(ctx, |pane_group, ctx| {
                    pane_group.add_loading_conversation_pane(Direction::Right, None, ctx)
                });

                workspace.queue_environment_runtime_intent(
                    &authority,
                    EnvironmentEntryIntent::PlainTerminal(PlainTerminalEntry::default_tab(false)),
                    queued_pane_id,
                );
                workspace
                    .environments
                    .begin_materialization(
                        &authority,
                        queued_pane_id,
                        materializing_pane_id,
                    )
                    .expect("queued pane must enter materializing");

                (pane_group, authority, materializing_pane_id)
            });

        pane_group.update(&mut app, |pane_group, ctx| {
            pane_group.close_pane(materializing_pane_id, ctx);
        });

        workspace.read(&app, |workspace, _ctx| {
            assert!(
                !workspace.has_pending_environment_runtime_entry_for_authority(&authority),
                "closing a materializing terminal before bootstrap must not leave an orphaned owner"
            );
        });
    });
}

#[test]
fn test_environment_runtime_pane_state_save_reconciles_pending_owners_first() {
    const VIEW_RS: &str = include_str!("view.rs");
    let app_state_arm = VIEW_RS
        .split_once("pane_group::Event::AppStateChanged => {")
        .expect("PaneGroup AppStateChanged arm must exist")
        .1;
    let reconcile = app_state_arm
        .find("self.reconcile_pending_environment_runtime_pane_owners(ctx);")
        .expect("pane state changes must reconcile pending pane ownership");
    let save = app_state_arm
        .find("ctx.dispatch_global_action(\"workspace:save_app\", ());")
        .expect("pane state changes must persist workspace state");
    assert!(
        reconcile < save,
        "pending pane ownership must be reconciled before pane state persistence"
    );
}

#[test]
fn test_environment_runtime_scan_without_client_reconnects() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            install_test_saved_ssh_target_catalog(&server, ctx);
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
                .environments_mut()
                .mark_connected_session(stale_session_id, HostId::new("scan-missing-client-host".to_string()));
            workspace.set_active_tab_environment(environment);

            workspace.scan_environment_runtime_agent_sessions_with_refresh_generation(
                authority.clone(),
                stale_session_id,
                None,
                ctx,
            );

            let owner_session_id = workspace
                .environment_runtime_session_for_authority(&authority)
                .expect("scan without a runtime client should trigger reconnect");
            assert_eq!(
                owner_session_id, stale_session_id,
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
fn remote_cli_agent_source_id_rejects_authority_mismatch() {
    let encoded_authority = "ssh:ssh-config:source-owner";
    let snapshot_authority = "ssh:ssh-config:other-owner";
    let session = WorkspaceSessionSnapshot {
        id: crate::workspace::environment_runtime::environment_cli_agent_session_source_id(
            encoded_authority,
            &CLIAgent::Codex,
            "/root/.codex/sessions/session.jsonl",
        ),
        environment_authority_key: Some(snapshot_authority.to_owned()),
        cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
        cli_agent_session_id: Some("codex-session".to_owned()),
        ..test_environment_runtime_session_snapshot("authority-mismatch", snapshot_authority)
    };

    let error = Workspace::environment_cli_agent_session_source_target_for_snapshot(&session)
        .expect_err("encoded source ownership must match snapshot authority");
    assert!(error.contains(encoded_authority));
    assert!(error.contains(snapshot_authority));
}

#[test]
fn remote_cli_agent_source_delete_never_crosses_authority() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_owned()),
                EnvironmentLifecycleState::Connected,
            );
            let snapshot_authority = environment.authority_key.clone();
            let stale_session_id = CoreSessionId::from(9031);
            workspace.mark_environment_runtime_connecting(
                environment.clone(),
                stale_session_id,
                PathBuf::from("/tmp/ashide-test-source-authority-mismatch.sock"),
            );
            workspace.environments_mut().mark_connected_session(
                stale_session_id,
                HostId::new("source-authority-mismatch-host".to_owned()),
            );
            workspace.set_active_tab_environment(environment);

            let encoded_authority = "ssh:ssh-config:different-owner";
            let session = WorkspaceSessionSnapshot {
                id: crate::workspace::environment_runtime::environment_cli_agent_session_source_id(
                    encoded_authority,
                    &CLIAgent::Codex,
                    "/root/.codex/sessions/session.jsonl",
                ),
                environment_authority_key: Some(snapshot_authority.clone()),
                cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
                cli_agent_session_id: Some("codex-session".to_owned()),
                ..test_environment_runtime_session_snapshot(
                    "delete-authority-mismatch",
                    &snapshot_authority,
                )
            };

            assert!(!workspace.schedule_environment_cli_agent_session_source_action(
                &session,
                EnvironmentCliAgentSessionSourceAction::Delete,
                ctx,
            ));
            assert_eq!(
                workspace.environment_runtime_session_for_authority(&snapshot_authority),
                Some(stale_session_id),
                "authority mismatch must fail before selecting or reconnecting the snapshot runtime"
            );
        });
    });
}

#[test]
fn remote_scan_malformed_record_preserves_cached_rows() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, _ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "scan-owner".to_owned(),
                &server,
                Some("/root/project".to_owned()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            workspace.set_active_tab_environment(environment);
            let existing = test_environment_runtime_session_snapshot("cached-row", &authority);
            workspace
                .commit_indexed_environment_cli_agent_sessions(
                    &authority,
                    Ok(vec![existing.clone()]),
                )
                .expect("valid scan should commit");

            let error = workspace
                .commit_indexed_environment_cli_agent_sessions(
                    &authority,
                    Err("malformed record 1: missing source".to_owned()),
                )
                .expect_err("malformed scan must reject the whole replacement");
            assert!(error.contains("malformed record"));
            assert_eq!(
                workspace.indexed_cli_agent_sessions_for_authority(&authority),
                vec![existing]
            );
        });
    });
}

#[test]
fn saved_ssh_node_runtime_lookup_uses_shared_authority_identity() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, _ctx| {
            let server = test_ssh_server_for_environment_tests();
            let node_id = server.node_id.clone();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                node_id.clone(),
                &server,
                Some("/root/project".to_owned()),
                EnvironmentLifecycleState::Connecting,
            );
            assert_eq!(
                environment.authority_key,
                crate::environment_authority::saved_ssh_authority(&node_id)
            );
            let session_id = CoreSessionId::from(9032);
            let host_id = HostId::new("saved-ssh-authority-host".to_owned());
            workspace.mark_environment_runtime_connecting(
                environment,
                session_id,
                PathBuf::from("/tmp/ashide-test-saved-ssh-authority.sock"),
            );
            workspace
                .environments_mut()
                .mark_connected_session(session_id, host_id.clone());

            let target = workspace
                .connected_environment_runtime_target_for_node(&node_id)
                .expect("saved SSH authority should resolve its connected target");
            assert_eq!(target.host_id, host_id);
            assert_eq!(target.session_id, session_id);
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
            install_test_saved_ssh_target_catalog(&server, ctx);
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
                .environments_mut()
                .mark_connected_session(stale_session_id, HostId::new("source-action-missing-client-host".to_string()));
            workspace.set_active_tab_environment(environment);

            let session_id = crate::workspace::environment_runtime::environment_cli_agent_session_source_id(
                &authority,
                &CLIAgent::Codex,
                "/root/.codex/sessions/session.jsonl",
            );
            let session = WorkspaceSessionSnapshot {
                id: session_id,
                container_uuid: None,
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
                is_live_container: false,
            };

            assert!(
                !workspace.schedule_environment_cli_agent_session_source_action(
                    &session,
                    crate::workspace::environment_runtime::EnvironmentCliAgentSessionSourceAction::Delete,
                    ctx,
                ),
                "source action cannot run without a client, but it must trigger reconnect"
            );
            let owner_session_id = workspace
                .environment_runtime_session_for_authority(&authority)
                .expect("source action without a runtime client should trigger reconnect");
            assert_eq!(owner_session_id, stale_session_id);
            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                Some(EnvironmentLifecycleState::Connecting)
            );
        });
    });
}

#[test]
fn test_environment_session_user_state_without_client_reconnects() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            install_test_saved_ssh_target_catalog(&server, ctx);
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let stale_session_id = CoreSessionId::from(9020);
            workspace.mark_environment_runtime_connecting(
                environment.clone(),
                stale_session_id,
                PathBuf::from("/tmp/ashide-test-user-state-without-client.sock"),
            );
            workspace.environments_mut().mark_connected_session(
                stale_session_id,
                HostId::new("user-state-missing-client-host".to_string()),
            );
            workspace.set_active_tab_environment(environment);

            let error = workspace
                .mutate_workspace_session_user_state_for_authority(
                    &authority,
                    &["agent:codex:session-1".to_string()],
                    crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserStateMutation::SetPinned,
                    crate::workspace::environment_backend::SessionUserStateMutationFeedback::Pinned,
                    ctx,
                )
                .expect_err("stale remote user-state mutation must fail closed");

            assert!(error.contains("reconnecting"));
            let owner_session_id = workspace
                .environment_runtime_session_for_authority(&authority)
                .expect("stale remote user-state mutation should trigger reconnect");
            assert_eq!(owner_session_id, stale_session_id);
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
                    .current_environment_snapshot()
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
                    .session_navigator_sessions()
                    .iter()
                    .all(|session| {
                        session.environment_authority_key.as_deref() != Some(authority.as_str())
                            || !session.is_active
                            || workspace
                                .current_environment_snapshot()
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
fn test_runtime_bootstrap_failure_preserves_all_authority_pending_intents() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "remote-fixture-primary".to_string(),
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
            let pane_group = workspace.active_tab_pane_group().clone();
            let terminal_pane_id = active_test_pane_id(workspace, ctx);
            let command_pane_id = pane_group.update(ctx, |pane_group, ctx| {
                pane_group.add_loading_conversation_pane(Direction::Right, None, ctx)
            });
            let restore_pane_id = pane_group.update(ctx, |pane_group, ctx| {
                pane_group.add_loading_conversation_pane(Direction::Right, None, ctx)
            });
            let pending_restore = test_pending_environment_runtime_session_restore(&authority);
            workspace.queue_pending_environment_runtime_terminal(
                &authority,
                PlainTerminalEntry::default_tab(false),
                terminal_pane_id,
            );
            workspace.queue_pending_environment_runtime_startup_command(
                &authority,
                "pwd".to_owned(),
                command_pane_id,
            );
            workspace.queue_pending_environment_runtime_session_restore(
                &authority,
                pending_restore.clone(),
                restore_pane_id,
            );

            assert!(
                workspace.has_pending_environment_runtime_entry_for_authority(&authority),
                "test setup must create a pending runtime entry before bootstrap failure"
            );

            workspace.handle_environment_runtime_failed(
                session_id,
                "synthetic bootstrap failure".to_string(),
                ctx,
            );

            let pending = &workspace
                .environments
                .entry_for_authority(&authority)
                .expect("failed authority row must remain available for retry")
                .pending_materializations;
            assert_eq!(pending.len(), 3, "bootstrap failure must preserve every pane-owned intent");
            assert_eq!(
                workspace
                    .latest_pending_session_restore_for_authority(&authority)
                    .expect("restore intent must remain queued")
                    .session
                    .id,
                pending_restore.session.id
            );
            assert_eq!(
                workspace.current_environment_snapshot()
                    .as_ref()
                    .map(|environment| &environment.lifecycle_state),
                Some(&EnvironmentLifecycleState::Error),
                "bootstrap failure must surface as an Environment error"
            );
        });
    });
}

#[test]
fn test_environment_runtime_disconnected_request_after_bootstrap_reconnects_retained_authority() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            install_test_saved_ssh_target_catalog(&server, ctx);
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "remote-fixture-primary".to_string(),
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
                .environments_mut()
                .mark_connected_session(session_id, HostId::new("request-disconnected-host".to_string()));
            workspace.set_active_tab_environment(environment);
            workspace.queue_pending_environment_runtime_terminal(
                &authority,
                PlainTerminalEntry::default_tab(false),
                active_test_pane_id(workspace, ctx),
            );

            workspace.handle_environment_runtime_client_request_failed(
                session_id,
                crate::workspace::environment_runtime::EnvironmentRuntimeOperation::NavigateToDirectory,
                crate::workspace::environment_runtime::EnvironmentRuntimeErrorKind::Disconnected,
                ctx,
            );

            assert!(
                workspace.has_pending_environment_runtime_entry_for_authority(&authority),
                "post-bootstrap disconnect must preserve the pending terminal intent while the retained Environment reconnects"
            );
            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                Some(EnvironmentLifecycleState::Connecting),
                "a retained post-bootstrap disconnect must enter the canonical reconnect generation instead of stopping at Error"
            );
            assert_eq!(
                workspace.current_environment_snapshot()
                    .as_ref()
                    .map(|environment| &environment.lifecycle_state),
                Some(&EnvironmentLifecycleState::Connecting),
                "Environment Strip must project the same retained reconnect lifecycle"
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
                "remote-fixture-primary".to_string(),
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
                .environments_mut()
                .mark_connected_session(session_id, HostId::new("decode-error-host".to_string()));
            workspace.set_active_tab_environment(environment);

            workspace.handle_environment_runtime_server_message_decoding_error(session_id, ctx);

            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                Some(EnvironmentLifecycleState::Error),
                "post-bootstrap protocol decoding errors should invalidate the runtime instead of leaving a stale Connected environment"
            );
            assert_eq!(
                workspace.current_environment_snapshot()
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
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9014);
            workspace.environments_mut().mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-ssh-control-cross-env-live-row.sock"),
            );
            workspace
                .environments_mut()
                .mark_connected_session(session_id, HostId::new("test-host".to_string()));
            workspace.set_active_tab_environment(environment);
            let initial_tab_count = workspace.tab_count();

            let cross_environment_live_id = format!("tab:{local_tab_index}:leaf:0");
            workspace
                .restored_workspace_sessions
                .push(WorkspaceSessionSnapshot {
                    id: cross_environment_live_id.clone(),
                    container_uuid: None,
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
                is_live_container: false,
                });

            workspace.activate_restored_workspace_session(
                &crate::workspace::action::WorkspaceSessionActionTarget::new(
                    cross_environment_live_id.clone(),
                    Some(authority.clone()),
                ),
                ctx,
            );

            assert_ne!(
                workspace.active_tab_index(),
                local_tab_index,
                "clicking an Environment session row must not focus a tab whose authority is current-app/local"
            );
            assert_eq!(
                workspace.current_environment_authority_key(ctx),
                authority,
                "cross-environment locator rejection must keep delivery owned by the remote authority"
            );
            assert_eq!(
                workspace.tab_count(),
                initial_tab_count + 1,
                "the invalid physical locator must be treated as a virtual restore and receive a dedicated runtime container"
            );
            assert!(
                workspace.tabs[environment_tab_index]
                    .environment
                    .as_ref()
                    .is_some_and(|environment| environment.authority_key == authority),
                "the pre-existing remote container must remain intact"
            );
            assert!(
                workspace.latest_pending_session_restore_for_authority(&authority).is_some(),
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
                    "remote-fixture-primary".to_string(),
                    &server,
                    Some("/root/project".to_string()),
                    EnvironmentLifecycleState::Connected,
                );
            let runtime_authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9015);
            workspace.environments_mut().mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-ssh-control-cross-env-activate.sock"),
            );
            workspace
                .environments_mut()
                .mark_connected_session(session_id, HostId::new("test-host".to_string()));
            workspace.set_active_tab_environment(environment);

            let local_session_id = Workspace::ashide_conversation_session_id(AIConversationId::new());
            workspace.restored_workspace_sessions.push(WorkspaceSessionSnapshot {
                id: local_session_id.clone(),
                container_uuid: None,
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
                is_live_container: false,
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
                workspace.snapshot_session_navigator_state().restoring_row_ids.is_empty(),
                "cross-environment session activation must not enter restoring state"
            );
            assert!(
                !workspace.latest_pending_session_restore_for_authority(&runtime_authority).is_some(),
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
                "remote-fixture-primary".to_string(),
                "ssh:ssh-config:remote-fixture-primary".to_string(),
                Some("ssh-config:remote-fixture-primary".to_string()),
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
                container_uuid: None,
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
                is_live_container: false,
            });
            workspace.sync_session_navigator_sessions(ctx);
            workspace.notify_session_navigator_focus_changed(ctx);

            let sessions = workspace.session_navigator_sessions();
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
    let catalog = unavailable_test_ssh_config_catalog();
    let mut session = WorkspaceSessionSnapshot {
        id: "remote-codex".to_string(),
        container_uuid: None,
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
        is_live_container: false,
    };

    assert_eq!(
        Workspace::workspace_session_environment_host_key(&session, &catalog),
        Some("ssh-config:missing-test-host".to_string()),
        "restored Environment agent rows must not be registered as current-app sessions"
    );

    session.environment_authority_key = None;
    assert_eq!(
        Workspace::workspace_session_environment_host_key(&session, &catalog),
        None,
        "current-app restored sessions keep the current-app host key"
    );
}

#[test]
fn custom_runtime_session_registers_environment_host_key_without_saved_ssh_connection_ref() {
    let catalog = unavailable_test_ssh_config_catalog();
    let mut session = WorkspaceSessionSnapshot {
        id: "custom-runtime-codex".to_owned(),
        container_uuid: None,
        kind: WorkspaceSessionKind::AgentTerminal,
        label: Some("custom runtime codex".to_owned()),
        environment_authority_key: Some("container:devbox".to_owned()),
        cwd: Some("/workspace".to_owned()),
        startup_directory: None,
        cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
        cli_command: Some("codex".to_owned()),
        cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
        conversation_ids: Vec::new(),
        active_conversation_id: None,
        cli_agent_session_id: Some("custom-runtime-restore".to_owned()),
        is_active: false,
        is_pinned: false,
        updated_at_unix_ms: None,
        is_live_container: false,
    };

    assert_eq!(
        Workspace::workspace_session_environment_host_key(&session, &catalog).as_deref(),
        Some("container:devbox"),
        "runtime capability must survive even when the authority has no saved-SSH connection ref"
    );

    session.environment_authority_key = Some("local:/workspace".to_owned());
    assert_eq!(
        Workspace::workspace_session_environment_host_key(&session, &catalog),
        None,
        "only TerminalBootstrap authority may use current-app plugin scope"
    );
}

#[test]
fn test_workspace_session_action_target_always_carries_canonical_authority() {
    let mut session = WorkspaceSessionSnapshot {
        id: "tab:1:leaf:0".to_string(),
        container_uuid: None,
        kind: WorkspaceSessionKind::AgentTerminal,
        label: Some("Environment Codex".to_string()),
        environment_authority_key: Some("ssh-config:remote-fixture-primary".to_string()),
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
        is_live_container: false,
    };

    let terminal_bootstrap_target =
        crate::workspace::action::WorkspaceSessionActionTarget::new(session.id.clone(), None);
    assert_eq!(
        terminal_bootstrap_target.environment_authority_key,
        crate::environment_authority::TERMINAL_BOOTSTRAP_AUTHORITY,
        "action construction must canonicalize missing snapshot authority exactly once"
    );
    assert!(
        !Workspace::workspace_session_matches_action_target(&session, &terminal_bootstrap_target),
        "canonical TerminalBootstrap target must not match a runtime-backed Environment session"
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
        Workspace::workspace_session_matches_action_target(&session, &terminal_bootstrap_target),
        "canonical TerminalBootstrap target must match a local snapshot"
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
            workspace.environments_mut().mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-switch-registry-env.sock"),
            );
            workspace
                .environments_mut()
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
                workspace.current_environment_snapshot()
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
            install_test_saved_ssh_target_catalog(&server, ctx);
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Dormant,
            );
            let authority = environment.authority_key.clone();
            workspace.add_test_environment_runtime_placeholder_tab(
                environment,
                Some("root@remote-fixture-primary".to_string()),
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
                workspace.current_environment_snapshot()
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
fn test_authority_context_runtime_open_preserves_active_environment_until_explicit_activation() {
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
            workspace.environments_mut().mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-authority-context-open.sock"),
            );
            workspace
                .environments_mut()
                .mark_connected_session(session_id, host_id.clone());
            workspace.add_test_environment_runtime_placeholder_tab(
                environment,
                Some("Authority Context Placeholder".to_string()),
                ctx,
            );
            let materialization_pane_id = active_test_pane_id(workspace, ctx);
            let environment_tab_index = workspace.active_tab_index();
            workspace.activate_tab_internal(0, ctx);
            workspace.queue_pending_environment_runtime_terminal(
                &authority,
                PlainTerminalEntry::default_tab(false),
                materialization_pane_id,
            );

            assert_eq!(
                workspace.tab_index_for_environment_authority(&authority),
                Some(environment_tab_index),
                "test setup must bind the pending intent to a real inactive Environment placeholder"
            );
            assert!(
                workspace.current_environment_snapshot()
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
                true,
                ctx,
            );

            assert!(
                workspace.tab_index_for_environment_authority(&authority).is_some(),
                "background authority-context completion must preserve the bound remote placeholder"
            );
            assert!(
                workspace
                    .current_environment_snapshot()
                    .as_ref()
                    .is_none_or(|environment| environment.authority_key != authority),
                "background authority-context completion must preserve the user's active environment"
            );
            assert!(
                workspace.has_pending_environment_runtime_entry_for_authority(&authority),
                "without a real runtime client in the unit harness, the pending intent must stay queued instead of being silently consumed"
            );
        });
    });
}

#[test]
fn test_environment_runtime_materialization_rejects_pane_from_other_authority() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let target_pane_id = active_test_pane_id(workspace, ctx);
            let original_pane_group_id = workspace.active_tab_pane_group().id();
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connecting,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9021);
            workspace.environments_mut().mark_connecting(
                environment,
                session_id,
                PathBuf::from("/tmp/ashide-test-authority-mismatch.sock"),
            );
            workspace.queue_pending_environment_runtime_terminal(
                &authority,
                PlainTerminalEntry::default_tab(false),
                target_pane_id,
            );

            let result = workspace.materialize_environment_runtime_terminal(
                &authority,
                test_environment_runtime_pty_options(session_id, ctx),
                target_pane_id,
                ctx,
            );

            assert!(result.terminal_view.is_none());
            assert!(result.discard_pending);
            assert_eq!(workspace.active_tab_pane_group().id(), original_pane_group_id);
            assert!(
                workspace
                    .active_tab_pane_group()
                    .as_ref(ctx)
                    .visible_pane_ids()
                    .contains(&target_pane_id),
                "authority mismatch must not replace the unrelated live pane"
            );
            assert!(
                workspace.has_pending_environment_runtime_entry_for_authority(&authority),
                "materialize only diagnoses ownership; the queue driver owns cancellation"
            );
        });
    });
}

#[test]
fn test_environment_runtime_materialization_only_focuses_previously_focused_placeholder() {
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
            workspace.add_test_environment_runtime_placeholder_tab(
                environment,
                Some("Background Placeholder".to_string()),
                ctx,
            );
            let target_pane_id = active_test_pane_id(workspace, ctx);
            let pane_group = workspace.active_tab_pane_group().clone();
            let sibling_pane_id = pane_group.update(ctx, |pane_group, ctx| {
                pane_group.add_loading_conversation_pane(Direction::Right, None, ctx)
            });
            workspace.queue_environment_runtime_intent(
                &authority,
                EnvironmentEntryIntent::PlainTerminal(PlainTerminalEntry::default_tab(false)),
                target_pane_id,
            );
            assert_eq!(
                pane_group.as_ref(ctx).focused_pane_id(ctx),
                sibling_pane_id,
                "test setup must move focus away from the placeholder before materialization"
            );

            let result = workspace.materialize_environment_runtime_terminal(
                &authority,
                test_environment_runtime_pty_options(CoreSessionId::from(9023), ctx),
                target_pane_id,
                ctx,
            );

            assert!(result.terminal_view.is_some());
            assert!(!result.discard_pending);
            assert_eq!(
                pane_group.as_ref(ctx).focused_pane_id(ctx),
                sibling_pane_id,
                "background placeholder materialization inside the active tab must not steal focus from a sibling pane"
            );
        });
    });
}

#[test]
fn test_environment_runtime_materialization_preserves_container_identity() {
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
            workspace.add_test_environment_runtime_placeholder_tab(
                environment,
                Some("Identity Placeholder".to_string()),
                ctx,
            );
            let placeholder_pane_id = active_test_pane_id(workspace, ctx);
            let pane_group = workspace.active_tab_pane_group().clone();
            let container_uuid = pane_group
                .as_ref(ctx)
                .container_uuid_for_pane_id(placeholder_pane_id, ctx)
                .expect("placeholder must own a durable container identity");
            workspace.queue_environment_runtime_intent(
                &authority,
                EnvironmentEntryIntent::PlainTerminal(PlainTerminalEntry::default_tab(false)),
                placeholder_pane_id,
            );

            let result = workspace.materialize_environment_runtime_terminal(
                &authority,
                test_environment_runtime_pty_options(CoreSessionId::from(9024), ctx),
                placeholder_pane_id,
                ctx,
            );
            let terminal_pane_id = result
                .terminal_pane_id
                .expect("placeholder must materialize into a terminal pane");

            assert_eq!(
                pane_group
                    .as_ref(ctx)
                    .container_uuid_for_pane_id(terminal_pane_id, ctx)
                    .as_deref(),
                Some(container_uuid.as_slice()),
                "permanent pane replacement must preserve container identity"
            );
        });
    });
}

#[test]
fn test_environment_runtime_materialization_commits_only_after_terminal_bootstrap() {
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
            workspace.add_test_environment_runtime_placeholder_tab(
                environment,
                Some("Bootstrap Commit Placeholder".to_string()),
                ctx,
            );
            let placeholder_pane_id = active_test_pane_id(workspace, ctx);
            let pane_group = workspace.active_tab_pane_group().clone();
            workspace.queue_environment_runtime_intent(
                &authority,
                EnvironmentEntryIntent::StartupCommand("pwd".to_owned()),
                placeholder_pane_id,
            );

            let result = workspace.materialize_environment_runtime_terminal(
                &authority,
                test_environment_runtime_pty_options(CoreSessionId::from(9025), ctx),
                placeholder_pane_id,
                ctx,
            );
            let terminal_pane_id = result
                .terminal_pane_id
                .expect("runtime terminal must be created");
            assert!(workspace
                .environments
                .pending_materialization_for_pane(&authority, terminal_pane_id)
                .is_some_and(|pending| {
                    pending.stage
                        == PendingMaterializationStage::Materializing {
                            pane_id: terminal_pane_id,
                        }
                }));

            workspace.complete_environment_runtime_terminal_materialization(
                &pane_group,
                terminal_pane_id,
                ctx,
            );

            assert!(
                !workspace.has_pending_environment_runtime_entry_for_authority(&authority),
                "intent may only be consumed by the SessionBootstrapped commit edge"
            );
        });
    });
}

#[test]
fn test_environment_runtime_failed_terminal_returns_to_retryable_placeholder() {
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
            workspace.add_test_environment_runtime_placeholder_tab(
                environment,
                Some("Bootstrap Failure Placeholder".to_string()),
                ctx,
            );
            let original_placeholder_pane_id = active_test_pane_id(workspace, ctx);
            let pane_group = workspace.active_tab_pane_group().clone();
            let container_uuid = pane_group
                .as_ref(ctx)
                .container_uuid_for_pane_id(original_placeholder_pane_id, ctx)
                .expect("placeholder must own a durable container identity");
            workspace.queue_environment_runtime_intent(
                &authority,
                EnvironmentEntryIntent::PlainTerminal(PlainTerminalEntry::default_tab(false)),
                original_placeholder_pane_id,
            );

            let result = workspace.materialize_environment_runtime_terminal(
                &authority,
                test_environment_runtime_pty_options(CoreSessionId::from(9026), ctx),
                original_placeholder_pane_id,
                ctx,
            );
            let failed_terminal_pane_id = result
                .terminal_pane_id
                .expect("runtime terminal must be created before failure");

            workspace.fail_environment_runtime_terminal_materialization(
                &pane_group,
                failed_terminal_pane_id,
                ctx,
            );

            let pending = workspace
                .environments
                .next_queued_materialization(&authority)
                .expect("failed bootstrap must retain a retryable request");
            let retry_placeholder_pane_id = pending.pane_id();
            assert_ne!(retry_placeholder_pane_id, failed_terminal_pane_id);
            assert_eq!(
                pane_group
                    .as_ref(ctx)
                    .container_uuid_for_pane_id(retry_placeholder_pane_id, ctx)
                    .as_deref(),
                Some(container_uuid.as_slice()),
                "failure recovery must not replace the stable user container"
            );
            assert!(matches!(pending.intent, EnvironmentEntryIntent::PlainTerminal(_)));
        });
    });
}

#[test]
fn test_environment_runtime_active_placeholder_queues_its_own_intent() {
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

            workspace.add_test_environment_runtime_placeholder_tab(
                environment.clone(),
                Some("First Placeholder".to_string()),
                ctx,
            );
            let first_pane_id = active_test_pane_id(workspace, ctx);
            workspace.queue_pending_environment_runtime_terminal(
                &authority,
                PlainTerminalEntry::default_tab(false),
                first_pane_id,
            );

            workspace.add_test_environment_runtime_placeholder_tab(
                environment,
                Some("Second Placeholder".to_string()),
                ctx,
            );
            let second_pane_id = active_test_pane_id(workspace, ctx);
            assert_ne!(first_pane_id, second_pane_id);

            workspace.queue_active_environment_runtime_placeholder_terminals_if_needed(ctx);

            let entry = workspace
                .environments
                .entry_for_authority(&authority)
                .expect("both pane-owned intents must share the authority runtime row");
            assert_eq!(entry.pending_materializations.len(), 2);
            assert_eq!(entry.pending_materializations[0].pane_id(), first_pane_id);
            assert_eq!(entry.pending_materializations[1].pane_id(), second_pane_id);
        });
    });
}

#[test]
fn test_environment_runtime_delivery_does_not_activate_another_same_authority_tab() {
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

            workspace.add_test_environment_runtime_placeholder_tab(
                environment.clone(),
                Some("Pending Delivery".to_string()),
                ctx,
            );
            let first_tab_id = workspace.active_tab_pane_group().id();
            let first_pane_id = active_test_pane_id(workspace, ctx);
            workspace.queue_pending_environment_runtime_terminal(
                &authority,
                PlainTerminalEntry::default_tab(false),
                first_pane_id,
            );

            workspace.add_test_environment_runtime_placeholder_tab(
                environment,
                Some("User Selected".to_string()),
                ctx,
            );
            let selected_tab_id = workspace.active_tab_pane_group().id();
            assert_ne!(first_tab_id, selected_tab_id);

            workspace.environments_mut().remember_active_tab(
                ParsedEnvironmentAuthority::parse(&authority)
                    .navigation_key()
                    .to_owned(),
                first_tab_id,
            );

            workspace.open_environment_runtime_terminal_for_authority_context(
                EnvironmentRuntimeTarget {
                    authority: authority.clone(),
                    session_id: CoreSessionId::from(9025),
                    host_id: HostId::new("same-authority-delivery-host".to_string()),
                    root: Some("/root/project".to_string()),
                },
                "/root/project",
                true,
                ctx,
            );

            assert_eq!(
                workspace.active_tab_pane_group().id(),
                selected_tab_id,
                "runtime delivery must never navigate through the authority's remembered tab"
            );
            assert!(
                workspace.has_pending_environment_runtime_entry_for_authority(&authority),
                "the unit harness has no registered runtime client, so delivery must stay queued"
            );
        });
    });
}

#[test]
fn environment_native_project_context_uses_runtime_resolved_path() {
    let project_context =
        environment_native_project_context_from_resolved_roots(EnvironmentRuntimeRoots {
            workspace_root: "/canonical/runtime/project".to_owned(),
            home_root: "/home/runtime".to_owned(),
        })
        .unwrap();

    assert_eq!(project_context.as_str(), "/canonical/runtime/project");
}

#[cfg(all(feature = "local_fs", not(target_family = "wasm"), unix))]
#[test]
#[ignore = "requires ASHIDE_LR134_REMOTE_* and an installed remote Claude CLI"]
fn environment_native_fork_remote_symlink_real_cli_resume_bootstrap() {
    use remote_server::auth::RemoteServerAuthContext;
    use remote_server::transport::RemoteTransport;
    use warp_core::{
        channel::{Channel, ChannelConfig, ChannelState},
        AppId,
    };

    use crate::session_bridge::adapter_registry::SessionBridgeForkTarget;
    use crate::session_bridge::ir::{SessionIr, SessionMessageIr};
    use crate::terminal::CLIAgent;
    use crate::workspace::environment_runtime::EnvironmentRuntimeTransport;

    App::test((), |app| async move {
        // 该真实 fixture 由 AshideDev 部署，测试进程本身不会经过
        // `app/src/bin/ashide.rs` 初始化 channel。显式复现真实客户端 channel，
        // 让生产 `remote_server_binary()` 选择同一个 dev protocol slot。
        ChannelState::set(ChannelState::new(
            Channel::Dev,
            ChannelConfig {
                app_id: AppId::new("dev", "ashide", "AshideDev"),
                logfile_name: "ashide_dev.log".into(),
                autoupdate_config: None,
                mcp_static_config: None,
            },
        ));
        let target = std::env::var("ASHIDE_LR134_REMOTE_TARGET").unwrap();
        let control_path =
            PathBuf::from(std::env::var("ASHIDE_LR134_REMOTE_CONTROL_PATH").unwrap());
        let fixture_root = std::env::var("ASHIDE_LR134_REMOTE_FIXTURE_ROOT").unwrap();
        let remote_path = std::env::var("ASHIDE_LR134_REMOTE_PATH").unwrap();
        let identity_key = std::env::var("ASHIDE_LR134_REMOTE_IDENTITY_KEY")
            .unwrap_or_else(|_| "test_user_uid".to_owned());
        let project_alias = format!("{fixture_root}/symlink-project");
        let canonical_project = format!("{fixture_root}/real-project");
        let home_root = format!("{fixture_root}/runtime-home");
        let claude_config_dir = format!("{home_root}/.claude");
        let codex_home = format!("{home_root}/.codex");

        let auth_context = Arc::new(RemoteServerAuthContext::new(
            || Box::pin(async { None }),
            move || identity_key.clone(),
        ));
        let transport =
            EnvironmentRuntimeTransport::new_with_target(control_path, target, auth_context);
        let connection = transport
            .connect(app.background_executor())
            .await
            .expect("remote runtime proxy must connect");
        let remote_server::transport::Connection {
            client,
            event_rx: _event_rx,
            child: _child,
            control_path: _connected_control_path,
        } = connection;
        client
            .initialize(None)
            .await
            .expect("remote runtime protocol must initialize");
        let client = Arc::new(client);
        let session_id = CoreSessionId::from(9134);
        client.notify_session_bootstrapped(
            session_id,
            "bash",
            Some("/bin/bash"),
            Some(&project_alias),
            &HashMap::from([
                ("HOME".to_owned(), home_root.clone()),
                ("PATH".to_owned(), remote_path),
                (
                    "ASHIDE_SESSION_EXECUTION_CONTEXT".to_owned(),
                    "1".to_owned(),
                ),
            ]),
        );

        let roots = CliAgentStoreRoots::from_explicit_target_paths(
            PathBuf::from(&home_root),
            PathBuf::from(&claude_config_dir),
            PathBuf::from(&codex_home),
        )
        .unwrap();
        let source = SessionIr {
            source: "claude".to_owned(),
            session_id: "lr134-remote-source".to_owned(),
            title: "LR134 remote symlink source".to_owned(),
            project_path: Some(project_alias.clone()),
            created_at: None,
            updated_at: None,
            messages: vec![
                SessionMessageIr {
                    role: "user".to_owned(),
                    text: "LR134_REMOTE_CONTEXT_MARKER".to_owned(),
                    timestamp: None,
                },
                SessionMessageIr {
                    role: "assistant".to_owned(),
                    text: "LR134_REMOTE_CONTEXT_ACK".to_owned(),
                    timestamp: None,
                },
            ],
            artifacts: Vec::new(),
            metadata: serde_json::json!({}),
        };
        let derivation = crate::session_bridge::transform::fork_session(&source, None);
        let write_back = write_environment_native_session_bridge_derivation(
            client.clone(),
            session_id,
            "ssh:lr134-runtime".to_owned(),
            roots,
            derivation,
            SessionBridgeForkTarget::Agent(CLIAgent::Claude),
        )
        .await
        .expect("production Environment native fork must write remotely");
        let SessionBridgeForkWriteBack::RemoteNative { receipt, .. } = write_back else {
            panic!("expected remote native write-back");
        };
        assert_eq!(receipt.project_path, canonical_project);
        assert!(!receipt.session_file.contains("symlink-project"));
        assert!(receipt.session_file.starts_with(&claude_config_dir));

        let harness_path = format!("{fixture_root}/lr134_remote_resume_harness.py");
        let harness = r#"from http.server import BaseHTTPRequestHandler, HTTPServer
import json, os, subprocess, sys, threading
config_dir, cwd, session_id = sys.argv[1:]
requests = []
class H(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args): pass
    def do_HEAD(self):
        self.send_response(200); self.send_header('Content-Length','0'); self.end_headers()
    def do_POST(self):
        n = int(self.headers.get('Content-Length','0'))
        requests.append(self.rfile.read(n).decode('utf-8', 'replace'))
        events = [
          ('message_start', {'type':'message_start','message':{'id':'msg_lr134_remote_resume','type':'message','role':'assistant','model':'claude-sonnet-4-5-20250929','content':[],'stop_reason':None,'stop_sequence':None,'usage':{'input_tokens':1,'cache_creation_input_tokens':0,'cache_read_input_tokens':0,'output_tokens':0}}}),
          ('content_block_start', {'type':'content_block_start','index':0,'content_block':{'type':'text','text':''}}),
          ('content_block_delta', {'type':'content_block_delta','index':0,'delta':{'type':'text_delta','text':'LR134_REMOTE_RESUME_OK'}}),
          ('content_block_stop', {'type':'content_block_stop','index':0}),
          ('message_delta', {'type':'message_delta','delta':{'stop_reason':'end_turn','stop_sequence':None},'usage':{'output_tokens':1}}),
          ('message_stop', {'type':'message_stop'})]
        body = ''.join(f'event: {name}\ndata: {json.dumps(event)}\n\n' for name,event in events).encode()
        self.send_response(200); self.send_header('Content-Type','text/event-stream'); self.send_header('Content-Length',str(len(body))); self.send_header('Connection','close'); self.end_headers(); self.wfile.write(body)
server = HTTPServer(('127.0.0.1', 0), H)
thread = threading.Thread(target=server.serve_forever, daemon=True); thread.start()
env = os.environ.copy(); env.update({'CLAUDE_CONFIG_DIR':config_dir,'ANTHROPIC_API_KEY':'ashide-lr134','ANTHROPIC_BASE_URL':f'http://127.0.0.1:{server.server_port}','CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC':'1'})
result = subprocess.run(['claude','--resume',session_id,'--print','LR134_REMOTE_PROMPT_MARKER','--output-format','json','--tools',''], cwd=cwd, env=env, capture_output=True, text=True, timeout=30)
server.shutdown(); thread.join(timeout=5)
assert result.returncode == 0, result.stderr
payload = json.loads(result.stdout)
assert payload['session_id'] == session_id, payload
assert payload['result'] == 'LR134_REMOTE_RESUME_OK', payload
body = '\n'.join(requests)
assert 'LR134_REMOTE_CONTEXT_MARKER' in body, body
assert 'LR134_REMOTE_PROMPT_MARKER' in body, body
print('LR134_REMOTE_RUNTIME_OK')
"#;
        client
            .write_file(harness_path.clone(), harness.to_owned())
            .await
            .expect("runtime harness must be written remotely");
        let command = format!(
            "python3 {} {} {} {}",
            shell_words::quote(&harness_path),
            shell_words::quote(&claude_config_dir),
            shell_words::quote(&project_alias),
            shell_words::quote(&receipt.session_id),
        );
        let stdout = crate::workspace::environment_runtime::run_command_success(
            &client, session_id, command,
        )
        .await
        .expect("real remote Claude must resume the generated fork");
        assert_eq!(
            String::from_utf8_lossy(&stdout).trim(),
            "LR134_REMOTE_RUNTIME_OK"
        );
        client.notify_session_deregistered(session_id);
    });
}

#[test]
fn test_environment_runtime_passive_refresh_without_client_preserves_projection_and_lifecycle() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/project".to_owned()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9032);
            workspace.mark_environment_runtime_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-passive-refresh-no-client.sock"),
            );
            workspace
                .environments_mut()
                .mark_connected_session(session_id, HostId::new("passive-refresh-host".to_owned()));
            workspace.set_active_tab_environment(environment);
            workspace.remember_environment_runtime_home_root(authority.clone(), "/root".to_owned());

            let mut rows = vec![
                test_environment_runtime_session_snapshot("passive-target", &authority),
                test_environment_runtime_session_snapshot("passive-unrelated-a", &authority),
                test_environment_runtime_session_snapshot("passive-unrelated-b", &authority),
            ];
            for (index, row) in rows.iter_mut().enumerate() {
                row.cli_agent_session_id = Some(format!("passive-provider-{index}"));
            }
            workspace
                .commit_indexed_environment_cli_agent_sessions(
                    &authority,
                    Ok::<_, String>(rows.clone()),
                )
                .expect("test cache commit must succeed");

            let spawned = workspace
                .refresh_indexed_sessions_for_authority(
                    &authority,
                    EnvironmentSessionRefreshIntent::PassiveProjection,
                    ctx,
                )
                .expect("passive projection without a client is a preserved no-op");

            assert!(!spawned);
            assert_eq!(
                workspace.environment_runtime_session_for_authority(&authority),
                Some(session_id),
                "passive projection must not replace the runtime owner"
            );
            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                Some(EnvironmentLifecycleState::Connected),
                "passive projection must not reconnect or mutate lifecycle"
            );
            assert_eq!(
                workspace.indexed_cli_agent_sessions_for_authority(&authority),
                rows,
                "passive projection must preserve the canonical collection when no scan can start"
            );
        });
    });
}

#[test]
fn test_environment_runtime_root_materialization_owns_passive_session_refresh() {
    const VIEW_RS: &str = include_str!("view.rs");

    let connected = VIEW_RS
        .split_once("fn handle_environment_runtime_connected(")
        .expect("connected handler must exist")
        .1
        .split_once("fn handle_environment_runtime_disconnected(")
        .expect("connected handler must end before disconnected handler")
        .0;
    assert!(
        !connected.contains("scan_environment_runtime_agent_sessions"),
        "SessionConnected must not race indexed-session scan before target HOME materializes"
    );

    let materialized = VIEW_RS
        .split_once("fn finish_resolve_environment_runtime_root(")
        .expect("root materialization handler must exist")
        .1
        .split_once("fn pending_environment_runtime_entry_plan(")
        .expect("root materialization handler must end before pending-entry helpers")
        .0;
    let remember_home = materialized
        .find("remember_environment_runtime_home_root")
        .expect("successful root materialization must remember exact target HOME");
    let passive_refresh = materialized
        .find("refresh_indexed_sessions_for_authority")
        .expect("successful root materialization must refresh through EnvironmentBackend");
    let passive_intent = materialized
        .find("EnvironmentSessionRefreshIntent::PassiveProjection")
        .expect("root materialization refresh must use PassiveProjection");
    let final_sync = materialized
        .rfind("sync_session_navigator_sessions")
        .expect("root materialization must project the resulting EnvironmentTable state");

    assert!(remember_home < passive_refresh);
    assert!(passive_refresh <= passive_intent);
    assert!(passive_intent < final_sync);
    assert!(
        !materialized.contains("workspace_root: \"/\"")
            && !materialized.contains("home_root: \"/\""),
        "unknown target roots must remain unresolved instead of becoming filesystem root"
    );
}

#[test]
fn heartbeat_waits_for_bootstrapped_execution_carrier() {
    const VIEW_RS: &str = include_str!("view.rs");

    let connected = VIEW_RS
        .split_once("fn handle_environment_runtime_connected(")
        .expect("connected handler must exist")
        .1
        .split_once("fn handle_environment_runtime_execution_context_established(")
        .expect("connected handler must end before execution-context handling")
        .0;
    assert!(
        !connected.contains("schedule_environment_runtime_heartbeat"),
        "transport Connected is not command-ready and must not schedule heartbeat before an execution carrier exists"
    );

    let context_established = VIEW_RS
        .split_once("fn handle_environment_runtime_execution_context_established(")
        .expect("execution-context handler must exist")
        .1
        .split_once("fn handle_environment_runtime_disconnected(")
        .expect("execution-context handler must end before disconnect handling")
        .0;
    let readiness_gate = context_established
        .find("environment_runtime_execution_carrier_gate")
        .expect("execution-context handling must use the shared carrier readiness gate");
    let heartbeat = context_established
        .find("schedule_environment_runtime_heartbeat")
        .expect("the first validated owner context must release heartbeat");
    let root_resolution = context_established
        .find("resolve_environment_runtime_root")
        .expect("the first validated owner context must release root resolution");
    assert!(readiness_gate < heartbeat);
    assert!(heartbeat < root_resolution);

    let heartbeat_scheduler = VIEW_RS
        .split_once("fn schedule_environment_runtime_heartbeat(")
        .expect("heartbeat scheduler must exist")
        .1
        .split_once("fn handle_environment_runtime_heartbeat_result(")
        .expect("heartbeat scheduler must end before its result handler")
        .0;
    assert!(
        heartbeat_scheduler.contains("environment_runtime_execution_carrier_gate"),
        "every initial or recursive heartbeat schedule must revalidate the canonical execution carrier"
    );
}

#[test]
fn test_environment_runtime_reconnect_preserves_owner_session_id() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            install_test_saved_ssh_target_catalog(&server, ctx);
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_owned()),
                EnvironmentLifecycleState::Dormant,
            );
            let authority = environment.authority_key.clone();
            let owner_session_id = CoreSessionId::from(9150);
            workspace.mark_environment_runtime_connecting(
                environment,
                owner_session_id,
                PathBuf::from("/tmp/ashide-test-stable-reconnect-owner.sock"),
            );
            workspace.handle_environment_runtime_failed(
                owner_session_id,
                "synthetic transport failure".to_owned(),
                ctx,
            );

            assert!(workspace.reconnect_environment_runtime_authority(&authority, ctx));
            assert_eq!(
                workspace.environment_runtime_session_for_authority(&authority),
                Some(owner_session_id),
                "retained Environment reconnect must advance the transport generation without replacing its canonical owner SessionId"
            );
        });
    });
}

#[test]
fn test_environment_runtime_reconnect_preserves_project_explorer_session_binding() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            install_test_saved_ssh_target_catalog(&server, ctx);
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_owned()),
                EnvironmentLifecycleState::Dormant,
            );
            let authority = environment.authority_key.clone();
            let bound_session_id = CoreSessionId::from(9151);
            workspace.mark_environment_runtime_connecting(
                environment,
                bound_session_id,
                PathBuf::from("/tmp/ashide-test-project-explorer-reconnect.sock"),
            );
            workspace.handle_environment_runtime_failed(
                bound_session_id,
                "synthetic project explorer transport failure".to_owned(),
                ctx,
            );

            assert!(workspace.reconnect_environment_runtime_authority(&authority, ctx));
            assert_eq!(
                workspace.environment_runtime_authority_for_session(bound_session_id),
                Some(authority.as_str()),
                "the exact session binding carried by Project Explorer/FileModel must remain owned by the Environment across reconnect"
            );
            assert!(
                workspace
                    .environment_runtime_control_path_for_session(bound_session_id)
                    .is_some(),
                "the retained owner must receive the new transport generation instead of becoming an orphan alias"
            );
        });
    });
}

#[test]
fn test_environment_runtime_explicit_transport_restart_does_not_reenter_reconnect() {
    const VIEW_RS: &str = include_str!("view.rs");
    let handler = VIEW_RS
        .split_once("fn handle_environment_runtime_disconnected(")
        .expect("Workspace must own one Environment disconnect handler")
        .1
        .split_once("fn handle_environment_runtime_deregistered(")
        .expect("disconnect handler must end before deregistration handling")
        .0;

    let explicit_restart = handler
        .split_once("EnvironmentRuntimeDisconnectCause::ExplicitTransportRestart")
        .expect("explicit transport restart must have a typed non-recovery branch")
        .1
        .split_once("EnvironmentRuntimeDisconnectCause::")
        .expect("explicit restart branch must end before the next disconnect cause")
        .0;
    assert!(
        !explicit_restart.contains("reconnect_environment_runtime_authority"),
        "a transport restart event must not re-enter the high-level reconnect intent"
    );
}

#[test]
fn test_environment_runtime_automatic_reconnect_waits_for_active_generation() {
    const VIEW_RS: &str = include_str!("view.rs");
    let helper = VIEW_RS
        .split_once("fn reconnect_environment_runtime_authority_if_transport_inactive(")
        .expect("automatic reconnect callers must share one inactive-only boundary")
        .1
        .split_once("fn reconnect_environment_runtime_authority(")
        .expect("inactive-only boundary must wrap the force restart primitive")
        .0;

    assert!(
        helper.contains("is_session_potentially_active"),
        "automatic reconnect must preserve Connecting/Initializing/Connected/Reconnecting generations"
    );
    assert!(
        helper.contains("reconnect_environment_runtime_authority(authority, ctx)"),
        "inactive-only requests must reuse the existing same-owner reconnect primitive once the manager is terminally inactive"
    );
}

#[test]
fn test_environment_runtime_heartbeat_execution_failure_does_not_reconnect() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/tmp/deleted-workspace".to_owned()),
                EnvironmentLifecycleState::Dormant,
            );
            let authority = environment.authority_key.clone();
            let owner_session_id = CoreSessionId::from(9152);
            workspace.mark_environment_runtime_connecting(
                environment,
                owner_session_id,
                PathBuf::from("/tmp/ashide-test-heartbeat-execution-failure.sock"),
            );
            workspace.mark_environment_runtime_connected_session(
                owner_session_id,
                HostId::new("heartbeat-execution-host".to_owned()),
            );
            let generation = workspace
                .environments
                .bump_heartbeat_generation(&authority)
                .expect("connected Environment must own a heartbeat generation");

            workspace.handle_environment_runtime_heartbeat_result(
                authority.clone(),
                owner_session_id,
                generation,
                crate::workspace::environment_runtime::EnvironmentRuntimeHeartbeatResult::ExecutionFailure(
                    "No such file or directory (os error 2)".to_owned(),
                ),
                ctx,
            );

            assert_eq!(
                workspace.environment_runtime_session_for_authority(&authority),
                Some(owner_session_id),
                "target cwd/command execution failure is not transport liveness failure and must not replace the owner"
            );
            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                Some(EnvironmentLifecycleState::Connected),
                "execution failure must preserve the connected transport lifecycle"
            );
        });
    });
}

#[test]
fn test_environment_runtime_heartbeat_transport_failure_reconnects_same_owner() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            install_test_saved_ssh_target_catalog(&server, ctx);
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_owned()),
                EnvironmentLifecycleState::Dormant,
            );
            let authority = environment.authority_key.clone();
            let owner_session_id = CoreSessionId::from(9153);
            workspace.mark_environment_runtime_connecting(
                environment,
                owner_session_id,
                PathBuf::from("/tmp/ashide-test-heartbeat-transport-failure.sock"),
            );
            workspace.mark_environment_runtime_connected_session(
                owner_session_id,
                HostId::new("heartbeat-transport-host".to_owned()),
            );
            let generation = workspace
                .environments
                .bump_heartbeat_generation(&authority)
                .expect("connected Environment must own a heartbeat generation");

            workspace.handle_environment_runtime_heartbeat_result(
                authority.clone(),
                owner_session_id,
                generation,
                crate::workspace::environment_runtime::EnvironmentRuntimeHeartbeatResult::TransportFailure(
                    "transport disconnected".to_owned(),
                ),
                ctx,
            );

            assert_eq!(
                workspace.environment_runtime_session_for_authority(&authority),
                Some(owner_session_id),
                "transport reconnect must advance the connection generation under the existing owner"
            );
            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                Some(EnvironmentLifecycleState::Connecting)
            );
        });
    });
}

#[test]
fn test_environment_runtime_root_resolution_failure_never_materializes_filesystem_root_home() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "remote-fixture-primary".to_string(),
                &server,
                None,
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9031);
            let host_id = HostId::new("failed-root-resolution-host".to_string());
            workspace.remember_environment_runtime_home_root(
                authority.clone(),
                "/stale-home-from-previous-session".to_owned(),
            );
            workspace.mark_environment_runtime_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-failed-root-resolution.sock"),
            );
            workspace
                .environments_mut()
                .mark_connected_session(session_id, host_id.clone());
            workspace.set_active_tab_environment(environment);

            let mut rows = vec![
                test_environment_runtime_session_snapshot("remote-history-target", &authority),
                test_environment_runtime_session_snapshot("remote-history-unrelated-a", &authority),
                test_environment_runtime_session_snapshot("remote-history-unrelated-b", &authority),
            ];
            for (index, row) in rows.iter_mut().enumerate() {
                row.cli_agent_session_id = Some(format!("remote-provider-{index}"));
            }
            workspace
                .commit_indexed_environment_cli_agent_sessions(
                    &authority,
                    Ok::<_, String>(rows.clone()),
                )
                .expect("test cache commit must succeed");

            workspace.finish_resolve_environment_runtime_root(
                authority.clone(),
                session_id,
                host_id,
                Err("target pwd/home resolution failed".to_owned()),
                ctx,
            );

            assert_eq!(workspace.environment_runtime_home_root(&authority), None);
            assert_eq!(
                workspace
                    .current_environment_snapshot()
                    .and_then(|environment| environment.active_workspace_root),
                None
            );
            assert_eq!(
                workspace.indexed_cli_agent_sessions_for_authority(&authority),
                rows,
                "failed materialization must preserve the previous canonical indexed collection"
            );
            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                Some(EnvironmentLifecycleState::Error),
                "target root/HOME failure must be explicit instead of fabricating a usable / root"
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
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9018);
            let host_id = HostId::new("idle-root-resolve-host".to_string());
            workspace.environments_mut().mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-ssh-control-idle-root.sock"),
            );
            workspace
                .environments_mut()
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
                ctx,
            );

            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                Some(EnvironmentLifecycleState::Connected),
                "root resolution without a pending terminal intent should keep the runtime connected instead of trying to spawn a terminal and marking Error"
            );
            assert_eq!(
                workspace.current_environment_snapshot()
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
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9024);
            let host_id = HostId::new("active-placeholder-root-resolve-host".to_string());
            workspace.add_test_environment_runtime_placeholder_tab(
                environment.clone(),
                Some("root@remote-fixture-primary".to_string()),
                ctx,
            );
            workspace.environments_mut().mark_connecting(
                environment,
                session_id,
                PathBuf::from("/tmp/ashide-test-active-placeholder-root.sock"),
            );
            workspace
                .environments_mut()
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
fn test_environment_runtime_root_resolution_updates_non_active_environment_tab_without_stealing_focus(
) {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/old".to_string()),
                EnvironmentLifecycleState::Connecting,
            );
            let authority = environment.authority_key.clone();
            workspace.add_test_environment_runtime_placeholder_tab(
                environment.clone(),
                Some("root@remote-fixture-primary".to_string()),
                ctx,
            );
            let environment_tab_index = workspace
                .tab_index_for_environment_authority(&authority)
                .expect("test setup should create an Environment tab");
            let materialization_pane_id = workspace.tabs[environment_tab_index]
                .pane_group
                .as_ref(ctx)
                .focused_pane_id(ctx);
            workspace.activate_tab_internal(0, ctx);
            let local_tab_id = workspace.active_tab_pane_group().id();
            workspace.queue_pending_environment_runtime_terminal(
                &authority,
                PlainTerminalEntry::default_tab(false),
                materialization_pane_id,
            );

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
            assert_eq!(workspace.active_tab_pane_group().id(), local_tab_id);
            assert_eq!(
                workspace.current_environment_snapshot()
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Local),
                "background runtime completion must not steal the user's local environment focus"
            );
            assert!(
                workspace.has_pending_environment_runtime_entry_for_authority(&authority),
                "background completion must preserve pending delivery until the user explicitly activates the remote environment"
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
                "remote-fixture-primary".to_string(),
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
            workspace.environments_mut().bump_preparation_generation(&authority);

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
                workspace.current_environment_snapshot()
                    .as_ref()
                    .map(|environment| &environment.lifecycle_state),
                Some(&EnvironmentLifecycleState::Error),
                "Environment Strip should not remain in the preparing state after watchdog timeout"
            );
        });
    });
}

#[test]
fn test_dev_environment_runtime_installation_watchdog_exceeds_cross_compile_budget() {
    assert!(
        Workspace::environment_runtime_installation_timeout(true)
            > remote_server::setup::DEV_CROSS_COMPILE_TIMEOUT,
        "dev runtime watchdog must leave time for gate wait, upload and verification after cargo"
    );
}

#[test]
fn test_environment_runtime_root_resolution_keeps_pending_entry_until_terminal_created() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9014);
            let host_id = HostId::new("test-host".to_string());
            workspace.environments_mut().mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-ssh-control-drain.sock"),
            );
            workspace
                .environments_mut()
                .mark_connected_session(session_id, host_id.clone());
            workspace.set_active_tab_environment(environment);

            workspace.handle_action(
                &WorkspaceAction::AddTerminalTab {
                    hide_homepage: false,
                },
                ctx,
            );
            assert!(
                workspace.has_pending_terminal_for_authority(&authority),
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
                ctx,
            );

            assert!(
                workspace.has_pending_terminal_for_authority(&authority),
                "pending terminal intent must not be consumed when no runtime terminal was created"
            );
            assert!(
                workspace.session_navigator_sessions().iter().any(|session| {
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
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let session_id = CoreSessionId::from(9011);
            workspace.environments_mut().mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-ssh-control-open-dir.sock"),
            );
            workspace
                .environments_mut()
                .mark_connected_session(session_id, HostId::new("test-host".to_string()));
            workspace.set_active_tab_environment(environment);

            workspace.open_directory_in_new_tab(PathBuf::from("/srv/app"), ctx);

            // On a connected runtime env, open_directory_in_new_tab routes through
            // RuntimeEnvironmentBackend::open_directory_tab → open_ready_environment_runtime_terminal_tab,
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
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let session_id = CoreSessionId::from(9012);
            workspace.environments_mut().mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-ssh-control-open-file-dir.sock"),
            );
            workspace
                .environments_mut()
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
                "remote-fixture-primary".to_string(),
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
                "remote-fixture-primary".to_string(),
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
fn test_session_navigator_normalizes_stale_focused_environment_live_sessions() {
    let mut sessions = vec![
        WorkspaceSessionSnapshot {
            id: "tab:0:leaf:0".to_string(),
            container_uuid: None,
            kind: WorkspaceSessionKind::Terminal,
            label: Some("Terminal A".to_string()),
            environment_authority_key: Some("ssh:ssh-config:remote-fixture-primary".to_string()),
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
            is_live_container: false,
        },
        WorkspaceSessionSnapshot {
            id: "tab:1:leaf:0".to_string(),
            container_uuid: None,
            kind: WorkspaceSessionKind::Terminal,
            label: Some("Terminal B".to_string()),
            environment_authority_key: Some("ssh:ssh-config:remote-fixture-primary".to_string()),
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
            is_live_container: false,
        },
        WorkspaceSessionSnapshot {
            id: "tab:2:leaf:0".to_string(),
            container_uuid: None,
            kind: WorkspaceSessionKind::AgentTerminal,
            label: Some("Codex".to_string()),
            environment_authority_key: Some("ssh:ssh-config:remote-fixture-primary".to_string()),
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
            is_live_container: false,
        },
    ];
    Workspace::normalize_session_navigator_active_state(&mut sessions, None);

    let active_sessions = sessions
        .iter()
        .filter(|session| session.is_active)
        .collect::<Vec<_>>();
    assert_eq!(active_sessions.len(), 1);
    assert_eq!(active_sessions[0].label.as_deref(), Some("Terminal A"));
}

#[test]
fn test_session_navigator_keeps_only_one_active_session_after_new_shell() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let authority = workspace.current_environment_authority_key(ctx);

            let first = WorkspaceSessionSnapshot {
                id: "environment-restored-session-a".to_string(),
                container_uuid: None,
                kind: WorkspaceSessionKind::AgentTerminal,
                label: Some("Environment Codex A".to_string()),
                environment_authority_key: Some(authority.clone()),
                cwd: Some("/root/project/a".to_string()),
                startup_directory: None,
                cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
                cli_command: Some("codex".to_string()),
                cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                conversation_ids: Vec::new(),
                active_conversation_id: None,
                cli_agent_session_id: Some("codex-environment-a".to_string()),
                is_active: false,
                is_pinned: false,
                updated_at_unix_ms: None,
                is_live_container: false,
            };
            let second = WorkspaceSessionSnapshot {
                id: "environment-restored-session-b".to_string(),
                container_uuid: None,
                kind: WorkspaceSessionKind::AgentTerminal,
                label: Some("Environment Codex B".to_string()),
                environment_authority_key: Some(authority.clone()),
                cwd: Some("/root/project/b".to_string()),
                startup_directory: None,
                cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
                cli_command: Some("codex".to_string()),
                cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                conversation_ids: Vec::new(),
                active_conversation_id: None,
                cli_agent_session_id: Some("codex-environment-b".to_string()),
                is_active: false,
                is_pinned: false,
                updated_at_unix_ms: None,
                is_live_container: false,
            };
            let first_key = Workspace::workspace_session_logical_key(&first);
            let second_key = Workspace::workspace_session_logical_key(&second);
            workspace.restored_workspace_sessions.push(first);
            workspace.restored_workspace_sessions.push(second);
            workspace.dispatch_session_navigator_state_action(
                session_navigator_reducer::SessionNavigatorAction::SelectionChanged {
                    session_logical_key: Some(first_key.clone()),
                },
                ctx,
            );
            workspace.dispatch_session_navigator_state_action(
                session_navigator_reducer::SessionNavigatorAction::SelectionChanged {
                    session_logical_key: Some(second_key.clone()),
                },
                ctx,
            );

            workspace.add_terminal_tab(false, ctx);

            let selected_row_ids = workspace
                .session_navigator_sessions()
                .into_iter()
                .filter(|session| session.is_active)
                .map(|session| Workspace::workspace_session_logical_key(&session))
                .collect::<HashSet<_>>();
            assert_eq!(
                selected_row_ids.len(),
                1,
                "opening another shell in the same Environment must not leave multiple restored sessions active; selected_row_ids={selected_row_ids:#?}"
            );
            assert!(
                !selected_row_ids.contains(&first_key) && !selected_row_ids.contains(&second_key),
                "resume/keepalive state must not be conflated with the single UI active selection; selected_row_ids={selected_row_ids:#?}"
            );
        });
    });
}

#[test]
fn test_session_navigator_materialized_unfocused_split_preserves_selection_and_projects_focus() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let pane_group = workspace.active_tab_pane_group();
            pane_group.update(ctx, |panes, ctx| {
                panes.add_terminal_pane_with_options(
                    Direction::Right,
                    NewTerminalOptions::default(),
                    ctx,
                );
                let first_pane_id = panes
                    .pane_id_by_index(0)
                    .expect("first split pane should exist");
                panes.focus_pane_by_id(first_pane_id, ctx);
            });

            let unfocused_live_session = workspace
                .live_workspace_sessions(ctx)
                .into_iter()
                .find(|session| session.id == "tab:0:leaf:1")
                .expect("second split pane should have a live Session Navigator row");
            let unfocused_key = Workspace::workspace_session_logical_key(&unfocused_live_session);
            workspace.dispatch_session_navigator_state_action(
                session_navigator_reducer::SessionNavigatorAction::RestoreStarted {
                    session_keys: vec![unfocused_live_session.id.clone(), unfocused_key.clone()],
                    selected_logical_key: Some(unfocused_key.clone()),
                },
                ctx,
            );

            workspace.sync_session_navigator_sessions(ctx);
            workspace.notify_session_navigator_focus_changed(ctx);

            let sessions = workspace.session_navigator_sessions();
            let active_rows = sessions
                .iter()
                .filter(|session| session.is_active)
                .collect::<Vec<_>>();
            assert_eq!(
                active_rows.len(),
                1,
                "materialized-but-unfocused live panes must not keep an extra active/restoring row"
            );
            assert_eq!(
                active_rows[0].id, "tab:0:leaf:0",
                "visual active selection should follow the focused pane, not stale resume state"
            );
            let unfocused_row = sessions
                .iter()
                .find(|session| session.id == "tab:0:leaf:1")
                .expect("unfocused split row should still be listed as live");
            assert!(
                !workspace.is_restoring_workspace_session(unfocused_row),
                "restore spinner/highlight must clear once a split pane has materialized, even when it is not focused"
            );
            assert_eq!(
                workspace.snapshot_session_navigator_state().selected_row_id.as_deref(),
                Some(
                    Workspace::session_navigator_row_id_for_identity(
                        &unfocused_key,
                        &workspace.snapshot_session_navigator_state(),
                    )
                    .as_str(),
                ),
                "focus projection must not overwrite this Environment's persistent selection"
            );
        });
    });
}

#[test]
fn test_session_navigator_tab_switch_preserves_selection_and_projects_focused_row() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            workspace.add_terminal_tab(false, ctx);
            workspace.activate_tab(0, ctx);

            let authority = workspace.current_environment_authority_key(ctx);
            let restored = WorkspaceSessionSnapshot {
                id: "environment-restored-session-switch-away".to_string(),
                container_uuid: None,
                kind: WorkspaceSessionKind::AgentTerminal,
                label: Some("Environment Codex switch away".to_string()),
                environment_authority_key: Some(authority),
                cwd: Some("/root/project/switch-away".to_string()),
                startup_directory: None,
                cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
                cli_command: Some("codex".to_string()),
                cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                conversation_ids: Vec::new(),
                active_conversation_id: None,
                cli_agent_session_id: Some("codex-environment-switch-away".to_string()),
                is_active: false,
                is_pinned: false,
                updated_at_unix_ms: None,
                is_live_container: false,
            };
            let restored_key = Workspace::workspace_session_logical_key(&restored);
            workspace.restored_workspace_sessions.push(restored);
            workspace.dispatch_session_navigator_state_action(
                session_navigator_reducer::SessionNavigatorAction::SelectionChanged {
                    session_logical_key: Some(restored_key.clone()),
                },
                ctx,
            );
            let selected_row_id = Workspace::session_navigator_row_id_for_identity(
                &restored_key,
                &workspace.snapshot_session_navigator_state(),
            );
            assert_eq!(
                workspace
                    .snapshot_session_navigator_state()
                    .selected_row_id
                    .as_deref(),
                Some(selected_row_id.as_str())
            );

            workspace.activate_tab(1, ctx);

            assert_eq!(
                workspace
                    .snapshot_session_navigator_state()
                    .selected_row_id
                    .as_deref(),
                Some(selected_row_id.as_str()),
                "switching tabs must preserve this Environment's Navigator selection"
            );
            let active_rows = workspace
                .session_navigator_sessions()
                .into_iter()
                .filter(|session| session.is_active)
                .collect::<Vec<_>>();
            assert_eq!(
                active_rows.len(),
                1,
                "Session Navigator must expose a single active row after tab switching"
            );
            assert_ne!(
                Workspace::workspace_session_logical_key(&active_rows[0]),
                restored_key,
                "resume/keepalive state must not keep a switched-away row visually active"
            );
        });
    });
}

#[test]
fn test_session_navigator_active_row_uses_focused_cli_resume_identity() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            use crate::terminal::model::ansi::Handler;

            let session_id = "019e3a0f-2fa7-78d2-ac9d-09b9c6b228ed";
            let indexed = WorkspaceSessionSnapshot {
                id: "codex-jsonl-active-row".to_string(),
                container_uuid: None,
                kind: WorkspaceSessionKind::AgentTerminal,
                label: Some("Codex active row should win".to_string()),
                environment_authority_key: Some(workspace.current_environment_authority_key(ctx)),
                cwd: Some("/Users/admin/ashide".to_string()),
                startup_directory: None,
                cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
                cli_command: Some("codex".to_string()),
                cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                conversation_ids: Vec::new(),
                active_conversation_id: None,
                cli_agent_session_id: Some(session_id.to_string()),
                is_active: false,
                is_pinned: false,
                updated_at_unix_ms: Some(10),
                is_live_container: false,
            };
            let authority = workspace.current_environment_authority_key(ctx);
            workspace
                .environments
                .commit_indexed_cli_agent_sessions(&authority, Ok::<_, String>(vec![indexed]))
                .expect("complete local session index commit cannot fail");

            let terminal_view = workspace
                .active_tab_pane_group()
                .as_ref(ctx)
                .focused_session_view(ctx)
                .expect("test workspace should start with a focused terminal");
            terminal_view.update(ctx, |view, _| {
                let mut model = view.model.lock();
                model.init_shell(crate::terminal::model::ansi::InitShellValue {
                    session_id: 0.into(),
                    shell: "zsh".to_owned(),
                    ..Default::default()
                });
                model.bootstrapped(crate::terminal::model::ansi::BootstrappedValue {
                    shell: "zsh".to_owned(),
                    ..Default::default()
                });
                model.start_command_execution();
                let blocks = model.block_list_mut();
                for ch in format!("cd /Users/admin/ashide && codex resume {session_id}").chars() {
                    blocks.input(ch);
                }
                blocks
                    .active_block_for_test()
                    .set_session_id(CoreSessionId::from(0));
                blocks.linefeed();
                blocks.preexec(crate::terminal::model::ansi::PreexecValue::default());
                blocks.on_finish_byte_processing(
                    &crate::terminal::model::ansi::ProcessorInput::new(&[]),
                );
            });
            CLIAgentSessionsModel::handle(ctx).update(ctx, |sessions, ctx| {
                sessions.set_session(
                    terminal_view.id(),
                    crate::terminal::cli_agent_sessions::CLIAgentSession {
                        agent: CLIAgent::Codex,
                        status:
                            crate::terminal::cli_agent_sessions::CLIAgentSessionStatus::InProgress,
                        session_context:
                            crate::terminal::cli_agent_sessions::CLIAgentSessionContext {
                                cwd: Some("/Users/admin/ashide".to_string()),
                                session_id: Some(session_id.to_string()),
                                ..Default::default()
                            },
                        input_state: crate::terminal::cli_agent_sessions::CLIAgentInputState::Closed,
                        should_auto_toggle_input: false,
                        listener: None,
                        plugin_version: None,
                        environment_host_key: None,
                        draft_text: None,
                        custom_command_prefix: Some("codex".to_string()),
                    },
                    ctx,
                );
            });
            workspace.sync_session_navigator_sessions(ctx);

            let sessions = workspace.session_navigator_sessions();
            let active_rows = sessions
                .iter()
                .filter(|session| session.is_active)
                .collect::<Vec<_>>();
            assert_eq!(
                active_rows.len(),
                1,
                "focused CLI resume pane must produce exactly one visual active row; sessions={sessions:#?}"
            );
            // Container model: the live tab is a container with stable pane UUID
            // identity. The indexed Codex session is
            // consumed (hidden) because the live container binds the same
            // agent session id. The active row is the container, not the
            // virtual target.
            let active_logical_key = active_rows[0].logical_key();
            assert!(
                active_logical_key.starts_with("local::pane:")
                    && !WorkspaceSessionSnapshot::is_volatile_layout_identity_key(
                        &active_logical_key
                    ),
                "active row must use stable pane identity across agent changes: {active_logical_key}"
            );
            assert_eq!(
                active_rows[0].id, "tab:0:leaf:0",
                "merged active Codex row must still point at the live focused pane"
            );
            // The active row remains the live container, but the consumed
            // indexed row can still contribute the more specific title.
            assert_eq!(
                Workspace::workspace_session_label(active_rows[0]),
                "Codex active row should win",
                "active live row should keep the indexed session title instead of regressing to the generic agent name"
            );
            // The indexed virtual row must be consumed (not shown) because the
            // live container binds the same agent session.
            let indexed_rows = sessions
                .iter()
                .filter(|session| session.id == "codex-jsonl-active-row")
                .collect::<Vec<_>>();
            assert!(
                indexed_rows.is_empty(),
                "indexed Codex session must be consumed by the live container binding, sessions={sessions:#?}"
            );
        });
    });
}

#[test]
fn test_new_specific_codex_tab_from_environment_runtime_stays_in_environment_group() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "remote-fixture-primary".to_string(),
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
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let session_id = CoreSessionId::from(9002);
            workspace.environments_mut().mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-ssh-control-default.sock"),
            );
            workspace
                .environments_mut()
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
fn test_add_default_tab_on_runtime_with_tab_config_mode_but_no_config_falls_through_to_runtime_terminal(
) {
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
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let session_id = CoreSessionId::from(9003);
            workspace.environments_mut().mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-ssh-control-tabconfig.sock"),
            );
            workspace
                .environments_mut()
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
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connecting,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9004);
            workspace.environments_mut().mark_connecting(
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
                workspace.pending_forked_conversation_for_authority(&authority).is_some(),
                "connecting-runtime fork split-pane must queue the ForkEntry"
            );
            // Loading pane id recorded so the connect callback replaces it.
            assert!(
                workspace.pending_materialization_pane_id_for_authority(&authority).is_some(),
                "connecting-runtime fork split-pane must record the loading pane id"
            );
        });
    });
}

#[test]
fn test_add_plain_terminal_pane_on_connecting_runtime_stages_loading_pane_and_queues() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connecting,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9007);
            workspace.environments_mut().mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-ssh-control-plain-terminal-split.sock"),
            );
            workspace.set_active_tab_environment(environment);

            let pane_count_before = workspace
                .active_tab_pane_group()
                .as_ref(ctx)
                .visible_pane_ids()
                .len();
            let result =
                workspace.add_terminal_pane_in_current_environment(Direction::Right, None, ctx);

            assert!(result.is_none());
            assert_eq!(
                workspace
                    .active_tab_pane_group()
                    .as_ref(ctx)
                    .visible_pane_ids()
                    .len(),
                pane_count_before + 1,
                "connecting-runtime plain terminal split must allocate its visible loading carrier before transport"
            );
            let entry = workspace
                .environments
                .entry_for_authority(&authority)
                .expect("plain terminal split must retain the environment owner");
            assert_eq!(entry.pending_materializations.len(), 1);
            assert!(matches!(
                entry.pending_materializations[0].intent,
                EnvironmentEntryIntent::PlainTerminal(_)
            ));
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
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connecting,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9005);
            workspace.environments_mut().mark_connecting(
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
                workspace.pending_agent_view_for_authority(&authority).is_some(),
                "connecting-runtime agent split-pane must queue the AgentTabEntry"
            );
            assert!(
                workspace.pending_materialization_pane_id_for_authority(&authority).is_some(),
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
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connecting,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9006);
            workspace.environments_mut().mark_connecting(
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
                workspace.pending_startup_command_for_authority(&authority).is_some(),
                "connecting-runtime editor split-pane must queue the startup command"
            );
            assert!(
                workspace.pending_materialization_pane_id_for_authority(&authority).is_some(),
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
                "remote-fixture-primary".to_string(),
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
            assert!(workspace.pending_agent_view_for_authority(&authority).is_some());

            workspace.disconnect_environment_authority(&authority, ctx);
            assert!(!workspace.pending_agent_view_for_authority(&authority).is_some());
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
                    "remote-fixture-primary".to_string(),
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
                workspace.pending_agent_view_for_authority(&authority).is_some(),
                "project agent entry should be stored as a pending Environment Runtime agent intent"
            );
            assert_eq!(
                workspace.current_environment_snapshot()
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
                "remote-fixture-primary".to_string(),
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
            assert!(workspace.pending_agent_view_for_authority(&authority).is_some());
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
                "remote-fixture-primary".to_string(),
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
                    container_uuid: None,
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
                is_live_container: false,
                });
            workspace
                .restored_workspace_sessions
                .push(WorkspaceSessionSnapshot {
                    id: "current-app-restored-session".to_string(),
                    container_uuid: None,
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
                is_live_container: false,
                });

            workspace.sync_session_navigator_sessions(ctx);
            let sessions = workspace.session_navigator_sessions();

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
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9010);
            workspace.environments_mut().mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-ssh-control-restore.sock"),
            );
            workspace
                .environments_mut()
                .mark_connected_session(session_id, HostId::new("restore-host".to_string()));
            workspace.set_active_tab_environment(environment);

            let restored = WorkspaceSessionSnapshot {
                id: "remote-restore-connected".to_string(),
                container_uuid: None,
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
                cli_agent_session_id: Some(
                    "11111111-1111-4111-8111-111111111111".to_string(),
                ),
                is_active: false,
                is_pinned: false,
                updated_at_unix_ms: None,
                is_live_container: false,
            };
            let logical_key = Workspace::workspace_session_logical_key(&restored);
            let durable_identity_key = restored
                .durable_identity_key()
                .expect("Codex restore must expose a durable provider identity");
            workspace.restored_workspace_sessions.push(restored.clone());

            workspace.activate_restored_workspace_session(
                &crate::workspace::action::WorkspaceSessionActionTarget::new(
                    restored.id.clone(),
                    restored.environment_authority_key.clone(),
                ),
                ctx,
            );

            assert!(
                workspace.latest_pending_session_restore_for_authority(&authority).is_some(),
                "pending restore must survive when no runtime terminal was actually created"
            );
            assert_eq!(
                workspace.latest_pending_session_restore_for_authority(&authority)
                    .and_then(|pending| pending.resume_command.as_deref()),
                Some("codex resume 11111111-1111-4111-8111-111111111111"),
                "clicking an Environment Codex restore row must queue the explicit remote resume command, not only open a shell"
            );
            assert_eq!(
                workspace.snapshot_session_navigator_state().selected_row_id.as_deref(),
                Some(
                    Workspace::session_navigator_row_id_for_identity(
                        &logical_key,
                        &workspace.snapshot_session_navigator_state(),
                    )
                    .as_str(),
                )
            );

            let sessions = workspace.session_navigator_sessions();
            let authority_rows = sessions
                .iter()
                .filter(|session| {
                    session.environment_authority_key.as_deref() == Some(authority.as_str())
                })
                .collect::<Vec<_>>();
            assert_eq!(
                authority_rows.len(),
                1,
                "allocation-time binding must consume the virtual row without projecting a generic carrier; sessions={sessions:#?}"
            );
            let live_session = authority_rows[0];
            assert!(live_session.is_live_container);
            assert!(live_session.container_uuid.is_some());
            assert!(live_session.is_active);
            assert_eq!(
                live_session.durable_identity_key().as_deref(),
                Some(durable_identity_key.as_str())
            );
            assert_eq!(live_session.cli_agent, restored.cli_agent);
            assert_eq!(live_session.cli_command, restored.cli_command);
            assert_eq!(live_session.cli_agent_origin, restored.cli_agent_origin);
            assert_eq!(
                live_session.cli_agent_session_id,
                restored.cli_agent_session_id
            );
            assert_ne!(
                live_session.id, restored.id,
                "live row id remains a physical action locator; durable identity carries continuity"
            );
            let state = workspace.snapshot_session_navigator_state();
            let live_row_id = Workspace::workspace_session_row_id(live_session, &state);
            assert_eq!(state.selected_row_id.as_deref(), Some(live_row_id.as_str()));

            let left_panel_sessions = workspace.session_navigator_sessions();
            assert_eq!(
                left_panel_sessions
                    .iter()
                    .filter(|session| {
                        session.environment_authority_key.as_deref() == Some(authority.as_str())
                            && session.durable_identity_key().as_deref()
                                == Some(durable_identity_key.as_str())
                    })
                    .count(),
                1,
                "left panel must project the same canonical live container; left_panel_sessions={left_panel_sessions:#?}"
            );
        });
    });
}

#[test]
fn test_environment_runtime_placeholder_allocation_binds_restore_before_reconnect() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment =
                crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                    "remote-fixture-primary".to_string(),
                    &server,
                    Some("/root/project".to_string()),
                    EnvironmentLifecycleState::Connected,
                );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9091);
            workspace.environments_mut().mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-placeholder-restore-transaction.sock"),
            );
            workspace.environments_mut().mark_connected_session(
                session_id,
                HostId::new("placeholder-restore-transaction-host".to_string()),
            );
            workspace.set_active_tab_environment(environment);

            let initial_tab_count = workspace.tab_count();
            let restored = WorkspaceSessionSnapshot {
                id: "environment-placeholder-restore-transaction".to_string(),
                container_uuid: None,
                kind: WorkspaceSessionKind::AgentTerminal,
                label: Some("Remote Claude restore transaction".to_string()),
                environment_authority_key: Some(authority.clone()),
                cwd: Some("/root/project".to_string()),
                startup_directory: None,
                cli_agent: Some(CLIAgent::Claude.to_serialized_name()),
                cli_command: Some("claude".to_string()),
                cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                conversation_ids: Vec::new(),
                active_conversation_id: None,
                cli_agent_session_id: Some("claude-restore-transaction".to_string()),
                is_active: false,
                is_pinned: false,
                updated_at_unix_ms: None,
                is_live_container: false,
            };
            workspace.restored_workspace_sessions.push(restored.clone());

            workspace.activate_restored_workspace_session(
                &crate::workspace::action::WorkspaceSessionActionTarget::new(
                    restored.id.clone(),
                    restored.environment_authority_key.clone(),
                ),
                ctx,
            );

            assert_eq!(workspace.tab_count(), initial_tab_count + 1);
            assert!(workspace.active_tab_contains_environment_runtime_placeholder(ctx));
            let pending = workspace
                .latest_pending_session_restore_for_authority(&authority)
                .expect("placeholder allocation must bind the restore before reconnect side effects run");
            assert_eq!(pending.session.id, restored.id);
            assert_eq!(
                pending.resume_command.as_deref(),
                Some("claude --resume claude-restore-transaction")
            );
        });
    });
}

#[test]
fn test_terminal_bootstrap_resume_allocates_bound_container_before_projection() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let environment =
                crate::workspace::environment_runtime::terminal_bootstrap_environment(None);
            let restored = WorkspaceSessionSnapshot {
                id: "local-resume-bound-at-allocation".to_owned(),
                container_uuid: None,
                kind: WorkspaceSessionKind::AgentTerminal,
                label: Some("Stable local Codex row".to_owned()),
                environment_authority_key: Some(environment.authority_key.clone()),
                cwd: None,
                startup_directory: None,
                cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
                cli_command: Some("codex".to_owned()),
                cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                conversation_ids: Vec::new(),
                active_conversation_id: None,
                cli_agent_session_id: Some("codex-local-bound-at-allocation".to_owned()),
                is_active: false,
                is_pinned: false,
                updated_at_unix_ms: Some(42),
                is_live_container: false,
            };
            workspace.restored_workspace_sessions.push(restored.clone());
            workspace.sync_session_navigator_sessions(ctx);
            let baseline_count = workspace.session_navigator_sessions().len();

            crate::workspace::environment_backend::EnvironmentBackendKind::for_environment(
                &environment,
            )
            .backend()
            .deliver_entry(
                workspace,
                &environment,
                crate::workspace::environment_backend::EnvironmentEntryIntent::SessionRestore(
                    crate::workspace::environment_backend::SessionRestoreEntry {
                        session: restored,
                        resume_command: None,
                    },
                ),
                ctx,
            );

            let pane_group = workspace.active_tab_pane_group().clone();
            let pane_id = pane_group.as_ref(ctx).focused_pane_id(ctx);
            let binding = pane_group
                .as_ref(ctx)
                .session_binding_for_pane_id(pane_id, ctx)
                .expect("local Resume carrier must own source binding before first projection");
            assert!(binding
                .source_identity_keys()
                .iter()
                .any(|key| key.contains("codex-local-bound-at-allocation")));

            let projected = workspace.session_navigator_sessions();
            assert_eq!(
                projected.len(),
                baseline_count,
                "local delivery must consume the virtual source in its first visible frame: {projected:#?}"
            );
            assert_eq!(
                projected
                    .iter()
                    .filter(|session| {
                        session.cli_agent_session_id.as_deref()
                            == Some("codex-local-bound-at-allocation")
                    })
                    .count(),
                1,
                "local delivery must never expose a generic live row beside the restore source"
            );
        });
    });
}

#[test]
fn test_environment_runtime_resume_never_projects_transient_generic_terminal_row() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment =
                crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                    "resume-no-transient-row".to_owned(),
                    &server,
                    Some("/root/project".to_owned()),
                    EnvironmentLifecycleState::Connected,
                );
            let authority = environment.authority_key.clone();
            let runtime_session_id = CoreSessionId::from(9092);
            workspace.environments_mut().mark_connecting(
                environment.clone(),
                runtime_session_id,
                PathBuf::from("/tmp/ashide-test-resume-no-transient-row.sock"),
            );
            workspace.environments_mut().mark_connected_session(
                runtime_session_id,
                HostId::new("resume-no-transient-row-host".to_owned()),
            );
            workspace.set_active_tab_environment(environment);

            let restored = WorkspaceSessionSnapshot {
                id: "remote-resume-no-transient-row".to_owned(),
                container_uuid: None,
                kind: WorkspaceSessionKind::AgentTerminal,
                label: Some("Stable remote Codex row".to_owned()),
                environment_authority_key: Some(authority.clone()),
                cwd: Some("/root/project".to_owned()),
                startup_directory: None,
                cli_agent: Some(CLIAgent::Codex.to_serialized_name()),
                cli_command: Some("codex".to_owned()),
                cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
                conversation_ids: Vec::new(),
                active_conversation_id: None,
                cli_agent_session_id: Some("codex-stable-resume-id".to_owned()),
                is_active: false,
                is_pinned: false,
                updated_at_unix_ms: Some(42),
                is_live_container: false,
            };
            let mut unrelated_newer = test_environment_runtime_session_snapshot(
                "remote-resume-unrelated-newer",
                authority.clone(),
            );
            unrelated_newer.label = Some("Unrelated newer remote session".to_owned());
            unrelated_newer.cli_agent_session_id =
                Some("codex-unrelated-newer-session-id".to_owned());
            unrelated_newer.updated_at_unix_ms = Some(84);
            let mut unrelated_older = test_environment_runtime_session_snapshot(
                "remote-resume-unrelated-older",
                authority.clone(),
            );
            unrelated_older.label = Some("Unrelated older remote session".to_owned());
            unrelated_older.cli_agent_session_id =
                Some("codex-unrelated-older-session-id".to_owned());
            unrelated_older.updated_at_unix_ms = Some(21);
            let virtual_logical_key = restored.logical_key();
            let durable_identity_key = restored
                .durable_identity_key()
                .expect("provider session must expose a durable identity");
            workspace
                .environments_mut()
                .commit_indexed_cli_agent_sessions(
                    &authority,
                    Ok::<_, String>(vec![
                        unrelated_older.clone(),
                        restored.clone(),
                        unrelated_newer.clone(),
                    ]),
                )
                .expect("remote Resume fixture must use the Environment-owned indexed source");

            workspace.sync_session_navigator_sessions(ctx);
            let before = workspace.session_navigator_sessions();
            assert_eq!(
                before
                    .iter()
                    .filter(|session| {
                        session.environment_authority_key.as_deref() == Some(authority.as_str())
                    })
                    .map(WorkspaceSessionSnapshot::logical_key)
                    .collect::<Vec<_>>(),
                vec![
                    unrelated_newer.logical_key(),
                    virtual_logical_key.clone(),
                    unrelated_older.logical_key(),
                ]
            );
            let state_before = workspace.snapshot_session_navigator_state();
            let row_ids_before = before
                .iter()
                .map(|session| Workspace::workspace_session_row_id(session, &state_before))
                .collect::<Vec<_>>();

            workspace.activate_restored_workspace_session(
                &crate::workspace::action::WorkspaceSessionActionTarget::new(
                    restored.id.clone(),
                    restored.environment_authority_key.clone(),
                ),
                ctx,
            );

            let pane_group = workspace.active_tab_pane_group().clone();
            let placeholder_pane_id = pane_group.as_ref(ctx).focused_pane_id(ctx);
            let container_uuid = pane_group
                .as_ref(ctx)
                .container_uuid_for_pane_id(placeholder_pane_id, ctx)
                .expect("allocated placeholder must already own the durable container");
            let binding = pane_group
                .as_ref(ctx)
                .session_binding_for_pane_id(placeholder_pane_id, ctx)
                .expect("allocated placeholder must already own provider binding");
            let selected_after_allocation = workspace
                .snapshot_session_navigator_state()
                .selected_row_id
                .clone();
            assert_eq!(
                workspace.logical_key_for_focused_live_pane(ctx),
                Some(virtual_logical_key.clone()),
                "the allocated placeholder binding, not the pending runtime queue, owns focused restore identity"
            );
            let sessions_after_allocation = workspace.session_navigator_sessions();
            let state_after_allocation = workspace.snapshot_session_navigator_state();
            let row_ids_after_allocation = sessions_after_allocation
                .iter()
                .map(|session| {
                    Workspace::workspace_session_row_id(session, &state_after_allocation)
                })
                .collect::<Vec<_>>();
            assert_eq!(
                sessions_after_allocation.len(),
                before.len(),
                "placeholder allocation must not change remote list cardinality"
            );
            assert_eq!(
                row_ids_after_allocation, row_ids_before,
                "placeholder allocation must preserve target and unrelated RowId order"
            );

            workspace
                .environments_mut()
                .commit_indexed_cli_agent_sessions(
                    &authority,
                    Ok::<_, String>(vec![unrelated_older, unrelated_newer]),
                )
                .expect("a complete scan may temporarily omit the restoring provider source");
            workspace.sync_session_navigator_sessions(ctx);
            let sessions_while_source_missing = workspace.session_navigator_sessions();
            let state_while_source_missing = workspace.snapshot_session_navigator_state();
            assert_eq!(
                sessions_while_source_missing.len(),
                before.len(),
                "a source-missing frame must preserve the restoring carrier and unrelated rows"
            );
            assert_eq!(
                sessions_while_source_missing
                    .iter()
                    .map(|session| {
                        Workspace::workspace_session_row_id(session, &state_while_source_missing)
                    })
                    .collect::<Vec<_>>(),
                row_ids_before,
                "a source-missing frame must preserve target and unrelated RowId order"
            );
            assert_eq!(
                sessions_while_source_missing
                    .iter()
                    .filter(|session| {
                        session.cli_agent_session_id.as_deref()
                            == Some("codex-stable-resume-id")
                    })
                    .count(),
                1,
                "the restoring target must retain exactly one visible carrier while its source is absent"
            );

            let materialized = workspace.materialize_environment_runtime_terminal(
                &authority,
                test_environment_runtime_pty_options(runtime_session_id, ctx),
                placeholder_pane_id,
                ctx,
            );
            let terminal_pane_id = materialized
                .terminal_pane_id
                .expect("remote restore must materialize a terminal runtime");
            assert_eq!(
                pane_group
                    .as_ref(ctx)
                    .container_uuid_for_pane_id(terminal_pane_id, ctx),
                Some(container_uuid.clone())
            );
            assert_eq!(
                pane_group
                    .as_ref(ctx)
                    .session_binding_for_pane_id(terminal_pane_id, ctx),
                Some(binding.clone())
            );

            for phase in ["materializing", "bootstrapped"] {
                if phase == "bootstrapped" {
                    workspace.complete_environment_runtime_terminal_materialization(
                        &pane_group,
                        terminal_pane_id,
                        ctx,
                    );
                }
                let sessions = workspace.session_navigator_sessions();
                let target_rows = sessions
                    .iter()
                    .filter(|session| {
                        session.cli_agent_session_id.as_deref()
                            == Some("codex-stable-resume-id")
                    })
                    .collect::<Vec<_>>();
                assert_eq!(
                    target_rows.len(),
                    1,
                    "{phase} must not project a second generic terminal row: {sessions:#?}"
                );
                assert_eq!(
                    target_rows[0].durable_identity_key().as_deref(),
                    Some(durable_identity_key.as_str())
                );
                assert_eq!(
                    target_rows[0].cli_agent_session_id.as_deref(),
                    Some("codex-stable-resume-id")
                );
                assert_eq!(
                    workspace.snapshot_session_navigator_state().selected_row_id,
                    selected_after_allocation
                );
                assert_eq!(
                    sessions.len(),
                    before.len(),
                    "{phase} must preserve remote list cardinality"
                );
                let state = workspace.snapshot_session_navigator_state();
                assert_eq!(
                    sessions
                        .iter()
                        .map(|session| Workspace::workspace_session_row_id(session, &state))
                        .collect::<Vec<_>>(),
                    row_ids_before,
                    "{phase} must preserve target and unrelated RowId order"
                );
            }
        });
    });
}

#[test]
fn test_session_navigator_render_never_projects_uncommitted_source_rows() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment =
                crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                    "committed-render-projection".to_owned(),
                    &server,
                    Some("/root/project".to_owned()),
                    EnvironmentLifecycleState::Connected,
                );
            let authority = environment.authority_key.clone();
            workspace.set_active_tab_environment(environment);

            let mut target = test_environment_runtime_session_snapshot(
                "committed-render-target",
                authority.clone(),
            );
            target.cli_agent_session_id = Some("committed-render-target-id".to_owned());
            let mut unrelated_newer = test_environment_runtime_session_snapshot(
                "committed-render-unrelated-newer",
                authority.clone(),
            );
            unrelated_newer.cli_agent_session_id =
                Some("committed-render-unrelated-newer-id".to_owned());
            let mut unrelated_older = test_environment_runtime_session_snapshot(
                "committed-render-unrelated-older",
                authority.clone(),
            );
            unrelated_older.cli_agent_session_id =
                Some("committed-render-unrelated-older-id".to_owned());

            workspace
                .environments_mut()
                .commit_indexed_cli_agent_sessions(
                    &authority,
                    Ok::<_, String>(vec![
                        unrelated_older.clone(),
                        target.clone(),
                        unrelated_newer.clone(),
                    ]),
                )
                .expect("baseline indexed source commits");
            workspace.sync_session_navigator_sessions(ctx);
            let committed_before = workspace.snapshot_session_navigator_model();
            let row_ids_before = committed_before
                .sessions
                .iter()
                .map(|session| {
                    Workspace::workspace_session_row_id(session, &committed_before.state)
                })
                .collect::<Vec<_>>();
            assert_eq!(committed_before.sessions.len(), 3);

            let mut newly_observed = test_environment_runtime_session_snapshot(
                "committed-render-new-source",
                authority.clone(),
            );
            newly_observed.cli_agent_session_id =
                Some("committed-render-new-source-id".to_owned());
            workspace
                .environments_mut()
                .commit_indexed_cli_agent_sessions(
                    &authority,
                    Ok::<_, String>(vec![
                        unrelated_older,
                        target,
                        unrelated_newer,
                        newly_observed,
                    ]),
                )
                .expect("source owner may change before projection sync");

            let render_before_sync = workspace.session_navigator_model();
            assert_eq!(
                render_before_sync.sessions, committed_before.sessions,
                "read-only render must keep the last committed projection until sync"
            );
            assert_eq!(
                render_before_sync
                    .sessions
                    .iter()
                    .map(|session| {
                        Workspace::workspace_session_row_id(session, &render_before_sync.state)
                    })
                    .collect::<Vec<_>>(),
                row_ids_before,
                "render must not allocate speculative RowId/display order"
            );
            assert_eq!(
                workspace.snapshot_session_navigator_model(),
                committed_before,
                "render query must remain a read-only canonical model lookup"
            );

            workspace.sync_session_navigator_sessions(ctx);
            let committed_after = workspace.snapshot_session_navigator_model();
            assert_eq!(
                committed_after.sessions.len(),
                4,
                "explicit sync must atomically publish the new source collection"
            );
            for session in &committed_after.sessions {
                let row_id =
                    Workspace::workspace_session_row_id(session, &committed_after.state);
                assert!(row_id.starts_with("row:"));
                assert!(committed_after.state.display_order.contains_key(&row_id));
            }
        });
    });
}

#[test]
fn test_session_navigator_render_model_keeps_sessions_and_state_atomic() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment =
                crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "atomic-render-model".to_owned(),
                &server,
                Some("/root/project".to_owned()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            workspace.set_active_tab_environment(environment);

            let mut first = test_environment_runtime_session_snapshot(
                "atomic-render-first",
                authority.clone(),
            );
            first.cli_agent_session_id = Some("atomic-render-first-id".to_owned());
            let mut second = test_environment_runtime_session_snapshot(
                "atomic-render-second",
                authority.clone(),
            );
            second.cli_agent_session_id = Some("atomic-render-second-id".to_owned());
            let mut third = test_environment_runtime_session_snapshot(
                "atomic-render-third",
                authority.clone(),
            );
            third.cli_agent_session_id = Some("atomic-render-third-id".to_owned());
            workspace
                .environments_mut()
                .commit_indexed_cli_agent_sessions(
                    &authority,
                    Ok::<_, String>(vec![first.clone(), second.clone(), third.clone()]),
                )
                .expect("initial complete scan must commit");
            workspace.sync_session_navigator_sessions(ctx);

            let mut newly_observed = test_environment_runtime_session_snapshot(
                "atomic-render-newly-observed",
                authority.clone(),
            );
            newly_observed.cli_agent_session_id = Some("atomic-render-new-id".to_owned());
            workspace
                .environments_mut()
                .commit_indexed_cli_agent_sessions(
                    &authority,
                    Ok::<_, String>(vec![first, second, third, newly_observed.clone()]),
                )
                .expect("next complete scan must commit without pre-applying reducer state");
            workspace.sync_session_navigator_sessions(ctx);

            let render_model = workspace.session_navigator_model();
            let rendered = render_model
                .sessions
                .iter()
                .find(|session| session.id == newly_observed.id)
                .expect("newly observed row must be present in the render model");
            let row_id = Workspace::workspace_session_row_id(rendered, &render_model.state);
            assert!(
                row_id.starts_with("row:"),
                "render must not pair a newly refreshed row with stale fallback identity: {row_id}"
            );
            assert!(
                Workspace::workspace_session_identity_keys(rendered)
                    .iter()
                    .any(|identity| {
                        render_model.state.row_id_by_identity.get(identity) == Some(&row_id)
                    }),
                "the same render snapshot must carry the row identity registry"
            );
        });
    });
}

#[test]
fn test_session_navigator_action_model_refreshes_sessions_and_state_atomically() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment =
                crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "atomic-action-model".to_owned(),
                &server,
                Some("/root/project".to_owned()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            workspace.set_active_tab_environment(environment);

            let mut existing = test_environment_runtime_session_snapshot(
                "atomic-action-existing",
                authority.clone(),
            );
            existing.cli_agent_session_id = Some("atomic-action-existing-id".to_owned());
            workspace
                .environments_mut()
                .commit_indexed_cli_agent_sessions(
                    &authority,
                    Ok::<_, String>(vec![existing.clone()]),
                )
                .expect("initial scan must commit");
            workspace.sync_session_navigator_sessions(ctx);

            let mut target = test_environment_runtime_session_snapshot(
                "atomic-action-target",
                authority.clone(),
            );
            target.cli_agent_session_id = Some("atomic-action-target-id".to_owned());
            let mut unrelated_newer = test_environment_runtime_session_snapshot(
                "atomic-action-unrelated-newer",
                authority.clone(),
            );
            unrelated_newer.cli_agent_session_id =
                Some("atomic-action-unrelated-newer-id".to_owned());
            let mut unrelated_older = test_environment_runtime_session_snapshot(
                "atomic-action-unrelated-older",
                authority.clone(),
            );
            unrelated_older.cli_agent_session_id =
                Some("atomic-action-unrelated-older-id".to_owned());
            workspace
                .environments_mut()
                .commit_indexed_cli_agent_sessions(
                    &authority,
                    Ok::<_, String>(vec![
                        existing,
                        unrelated_newer,
                        target.clone(),
                        unrelated_older,
                    ]),
                )
                .expect("new source rows must commit without pre-refreshing owner state");

            let target_key = Workspace::workspace_session_logical_key(&target);
            assert!(workspace.dispatch_session_navigator_state_action(
                session_navigator_reducer::SessionNavigatorAction::RestoreStarted {
                    session_keys: vec![target_key.clone()],
                    selected_logical_key: Some(target_key),
                },
                ctx,
            ));

            let committed = workspace.snapshot_session_navigator_model();
            assert_eq!(
                committed.sessions.len(),
                4,
                "action must commit the complete refreshed collection"
            );
            for session in &committed.sessions {
                let row_id = Workspace::workspace_session_row_id(session, &committed.state);
                assert!(
                    row_id.starts_with("row:"),
                    "action must not commit a refreshed row with fallback identity: {} -> {row_id}",
                    session.id
                );
                assert!(
                    committed.state.display_order.contains_key(&row_id),
                    "action must commit display order with the same refreshed model: {} -> {row_id}",
                    session.id
                );
            }

            let mut focus_new = test_environment_runtime_session_snapshot(
                "atomic-action-focus-new",
                authority.clone(),
            );
            focus_new.cli_agent_session_id = Some("atomic-action-focus-new-id".to_owned());
            let mut source_after_focus = committed.sessions.clone();
            source_after_focus.push(focus_new);
            workspace
                .environments_mut()
                .commit_indexed_cli_agent_sessions(
                    &authority,
                    Ok::<_, String>(source_after_focus),
                )
                .expect("focus source update must commit without pre-refreshing owner state");

            workspace.notify_session_navigator_focus_changed(ctx);
            let focused = workspace.snapshot_session_navigator_model();
            assert_eq!(
                focused.sessions.len(),
                5,
                "TabActivated/PaneFocused must consume the complete refreshed model"
            );
            for session in &focused.sessions {
                let row_id = Workspace::workspace_session_row_id(session, &focused.state);
                assert!(
                    row_id.starts_with("row:")
                        && focused.state.display_order.contains_key(&row_id),
                    "focus action must commit row identity and order atomically: {} -> {row_id}",
                    session.id
                );
            }
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
                    "remote-fixture-primary".to_string(),
                    &server,
                    Some("/root/project with spaces".to_string()),
                    EnvironmentLifecycleState::Connected,
                );
                let authority = environment.authority_key.clone();
                let session_id = CoreSessionId::from(9011 + index as u64);
                workspace.environments_mut().mark_connecting(
                    environment.clone(),
                    session_id,
                    PathBuf::from(format!(
                        "/tmp/ashide-test-ssh-control-first-class-restore-{index}.sock"
                    )),
                );
                workspace
                    .environments_mut()
                    .mark_connected_session(session_id, HostId::new("restore-host".to_string()));
                workspace.set_active_tab_environment(environment);

                let authority_row_count_before = workspace
                    .session_navigator_sessions()
                    .iter()
                    .filter(|session| {
                        session.environment_authority_key.as_deref() == Some(authority.as_str())
                    })
                    .count();

                let initial_tab_count = workspace.tab_count();
                let original_tab_id = workspace.tabs[workspace.active_tab_index()]
                    .pane_group
                    .id();
                assert!(
                    agent.capabilities().can_target_environment_runtime,
                    "{agent:?} should be a first-class Environment Runtime target"
                );
                let restored = WorkspaceSessionSnapshot {
                    id: format!("environment-{}-restore", agent.command_prefix()),
                    container_uuid: None,
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
                is_live_container: false,
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
                        .latest_pending_session_restore_for_authority(&authority)
                        .and_then(|pending| pending.resume_command.as_deref()),
                    Some(expected_startup_command),
                    "{agent:?} restore must queue the native remote startup command without prepending a current-app cd"
                );
                assert_eq!(
                    workspace.snapshot_session_navigator_state().selected_row_id.as_deref(),
                    Some(
                        Workspace::session_navigator_row_id_for_identity(
                            &logical_key,
                            &workspace.snapshot_session_navigator_state(),
                        )
                        .as_str(),
                    ),
                    "{agent:?} restore should keep the logical session active while waiting for the remote PTY"
                );
                assert_eq!(
                    workspace.tab_count(),
                    initial_tab_count + 1,
                    "{agent:?} Environment restore must allocate a dedicated runtime session container"
                );
                assert!(
                    workspace
                        .tabs
                        .iter()
                        .any(|tab| tab.pane_group.id() == original_tab_id),
                    "{agent:?} Environment restore must preserve the previously active live session container"
                );
                assert_ne!(
                    workspace.tabs[workspace.active_tab_index()].pane_group.id(),
                    original_tab_id,
                    "{agent:?} restored session must materialize in the newly allocated container"
                );

                let sessions = workspace.session_navigator_sessions();
                let authority_rows = sessions
                    .iter()
                    .filter(|session| {
                        session.environment_authority_key.as_deref() == Some(authority.as_str())
                    })
                    .collect::<Vec<_>>();
                assert_eq!(
                    authority_rows.len(),
                    authority_row_count_before + 1,
                    "{agent:?} restore must add exactly one semantic row, never a virtual row plus generic carrier; sessions={sessions:#?}"
                );
                let matching_rows = authority_rows
                    .into_iter()
                    .filter(|session| {
                        session.cli_agent == restored.cli_agent
                            && session.cli_command == restored.cli_command
                            && session.cli_agent_session_id == restored.cli_agent_session_id
                    })
                    .collect::<Vec<_>>();
                assert_eq!(
                    matching_rows.len(),
                    1,
                    "{agent:?} restore must project one bound live container; sessions={sessions:#?}"
                );
                let live_session = matching_rows[0];
                assert!(live_session.is_live_container);
                assert!(live_session.container_uuid.is_some());
                assert!(live_session.is_active);
                assert_eq!(live_session.cli_agent_origin, restored.cli_agent_origin);
                assert_ne!(
                    live_session.id, restored.id,
                    "{agent:?} live id remains a physical action locator"
                );
                if let Some(durable_identity_key) = restored.durable_identity_key() {
                    assert_eq!(
                        live_session.durable_identity_key().as_deref(),
                        Some(durable_identity_key.as_str())
                    );
                }
                let state = workspace.snapshot_session_navigator_state();
                let live_row_id = Workspace::workspace_session_row_id(live_session, &state);
                assert_eq!(
                    state.selected_row_id.as_deref(),
                    Some(live_row_id.as_str()),
                    "{agent:?} selection must follow the canonical RowId across virtual-to-live replacement"
                );
            }
        });
    });
}

#[test]
fn test_custom_runtime_restore_never_falls_back_to_terminal_bootstrap() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let authority = "container:test-runtime";
            let session = test_environment_runtime_session_snapshot("custom-runtime", authority);
            let initial_tab_count = workspace.tab_count();

            workspace.open_terminal_bootstrap_restored_session_terminal(
                Some(PathBuf::from("/workspace")),
                &session,
                Some("codex resume custom-runtime".to_owned()),
                None,
                ctx,
            );

            assert_eq!(workspace.tab_count(), initial_tab_count);
        });
    });
}

#[test]
fn test_virtual_session_restore_dispatches_by_authority_capability_not_connection_ref() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let authority = "container:test-runtime";
            let session = test_environment_runtime_session_snapshot("typed-runtime", authority);
            let initial_tab_count = workspace.tab_count();
            workspace.restored_workspace_sessions.push(session.clone());

            workspace.spawn_virtual_workspace_session_from_activate(&session, ctx);

            assert_eq!(
                workspace.tab_count(),
                initial_tab_count,
                "custom runtime authority must not fall through to current-app terminal bootstrap"
            );
            assert!(workspace
                .restored_workspace_sessions
                .iter()
                .any(|restored| restored.id == session.id));
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
                container_uuid: None,
                kind: WorkspaceSessionKind::AgentTerminal,
                label: Some(
                    "Environment Codex should not open through terminal bootstrap".to_string(),
                ),
                environment_authority_key: Some("ssh:ssh-config:remote-fixture-primary".to_string()),
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
                is_live_container: false,
            };
            let logical_key = Workspace::workspace_session_logical_key(&session);
            workspace.dispatch_session_navigator_state_action(
                session_navigator_reducer::SessionNavigatorAction::RestoreStarted {
                    session_keys: vec![session.id.clone(), logical_key.clone()],
                    selected_logical_key: Some(logical_key.clone()),
                },
                ctx,
            );
            let restoring_row_id = Workspace::session_navigator_row_id_for_identity(
                &logical_key,
                &workspace.snapshot_session_navigator_state(),
            );
            workspace.open_terminal_bootstrap_restored_session_terminal(
                Some(PathBuf::from("/root/project")),
                &session,
                Some("codex".to_string()),
                None,
                ctx,
            );

            assert_eq!(
                workspace.tab_count(),
                initial_tab_count,
                "environment restored session must not create a current-app terminal tab"
            );
            assert!(
                !workspace
                    .snapshot_session_navigator_state()
                    .restoring_row_ids
                    .contains(&restoring_row_id),
                "environment restore RowId marker should be cleared after terminal-bootstrap refusal"
            );
            assert!(
                workspace.snapshot_session_navigator_state().selected_row_id.as_deref()
                    != Some(restoring_row_id.as_str()),
                "refused environment restore should clear stale active restored marker"
            );
        });
    });
}

#[test]
fn test_explicit_new_terminal_selects_new_live_session_in_local_environment() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let previous_logical_key = workspace
                .logical_key_for_focused_live_pane(ctx)
                .expect("mock workspace must start with a focused local live session");
            workspace.dispatch_session_navigator_state_action(
                session_navigator_reducer::SessionNavigatorAction::SelectionChanged {
                    session_logical_key: Some(previous_logical_key.clone()),
                },
                ctx,
            );
            let previous_row_id = Workspace::session_navigator_row_id_for_identity(
                &previous_logical_key,
                &workspace.snapshot_session_navigator_state(),
            );

            workspace.add_terminal_tab(false, ctx);

            let new_logical_key = workspace
                .logical_key_for_focused_live_pane(ctx)
                .expect("explicit local terminal creation must materialize a focused live session");
            let new_row_id = Workspace::session_navigator_row_id_for_identity(
                &new_logical_key,
                &workspace.snapshot_session_navigator_state(),
            );
            assert_ne!(
                new_logical_key, previous_logical_key,
                "explicit local terminal creation must produce a distinct live session identity"
            );

            assert_eq!(
                workspace.snapshot_session_navigator_state().selected_row_id,
                Some(new_row_id),
                "explicit local terminal creation must move the Environment-owned selection to the new live row"
            );
            assert_ne!(
                workspace.snapshot_session_navigator_state().selected_row_id.as_deref(),
                Some(previous_row_id.as_str()),
                "the previously focused local row must no longer own selection"
            );
            let active_rows = workspace
                .session_navigator_sessions()
                .into_iter()
                .filter(|session| session.is_active)
                .collect::<Vec<_>>();
            assert_eq!(
                active_rows.len(),
                1,
                "Session Navigator must expose a single active row after opening a new local terminal"
            );
            assert_eq!(
                Workspace::workspace_session_logical_key(&active_rows[0]),
                new_logical_key,
                "active projection and Environment-owned selection must point at the same new local row"
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
fn test_explicit_new_terminal_selects_new_live_session_in_remote_environment() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);

        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority_key = environment.authority_key.clone();
            workspace.set_active_tab_environment(environment);
            let session = WorkspaceSessionSnapshot {
                id: "environment-restored-session".to_string(),
                container_uuid: None,
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
                is_live_container: false,
            };
            let logical_key = Workspace::workspace_session_logical_key(&session);
            workspace.restored_workspace_sessions.push(session);
            workspace.dispatch_session_navigator_state_action(
                session_navigator_reducer::SessionNavigatorAction::SelectionChanged {
                    session_logical_key: Some(logical_key.clone()),
                },
                ctx,
            );
            let restored_row_id = Workspace::session_navigator_row_id_for_identity(
                &logical_key,
                &workspace.snapshot_session_navigator_state(),
            );
            assert!(workspace
                .session_navigator_sessions()
                .iter()
                .any(|session| session.id == "environment-restored-session" && session.is_active));

            workspace.add_terminal_tab(false, ctx);

            let new_logical_key = workspace
                .logical_key_for_focused_live_pane(ctx)
                .expect("explicit terminal creation must expose a focused live session");
            let new_row_id = Workspace::session_navigator_row_id_for_identity(
                &new_logical_key,
                &workspace.snapshot_session_navigator_state(),
            );

            assert_eq!(
                workspace.snapshot_session_navigator_state().selected_row_id,
                Some(new_row_id),
                "explicit terminal creation must move the Environment-owned selection to the new live row"
            );
            assert_ne!(
                workspace.snapshot_session_navigator_state().selected_row_id.as_deref(),
                Some(restored_row_id.as_str()),
                "the previous restored row must no longer own selection"
            );
            let active_rows = workspace
                .session_navigator_sessions()
                .into_iter()
                .filter(|session| session.is_active)
                .collect::<Vec<_>>();
            assert_eq!(
                active_rows.len(),
                1,
                "Session Navigator must expose a single active row after opening a new terminal"
            );
            assert!(
                active_rows
                    .iter()
                    .all(|session| session.id != "environment-restored-session"),
                "the restored row must not stay visually active after a new terminal takes focus"
            );
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
                "remote-fixture-primary".to_string(),
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
                    container_uuid: None,
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
                is_live_container: false,
                });
            workspace
                .restored_workspace_sessions
                .push(WorkspaceSessionSnapshot {
                    id: "current-app-session".to_string(),
                    container_uuid: None,
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
                is_live_container: false,
                });

            workspace.add_explicit_terminal_bootstrap_default_tab(None, ctx);
            let current_app_cached_ids = workspace
                .session_navigator_sessions()
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
                .session_navigator_sessions()
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
                "remote-fixture-primary".to_string(),
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
                    .current_environment_snapshot()
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
                    .current_environment_snapshot()
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
fn test_cold_restored_active_environment_placeholder_rebuilds_terminal_intent_before_transport() {
    fn first_leaf_container_uuid(node: &crate::app_state::PaneNodeSnapshot) -> &[u8] {
        match node {
            crate::app_state::PaneNodeSnapshot::Leaf(leaf) => &leaf.container_uuid,
            crate::app_state::PaneNodeSnapshot::Branch(branch) => first_leaf_container_uuid(
                &branch
                    .children
                    .first()
                    .expect("pane tree must contain a leaf")
                    .1,
            ),
        }
    }

    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        let (snapshot, authority, restored_container_uuid) =
            workspace.update(&mut app, |workspace, ctx| {
                let server = test_ssh_server_for_environment_tests();
                let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                    server.node_id.clone(),
                    &server,
                    Some("/root/project".to_string()),
                    EnvironmentLifecycleState::Dormant,
                );
                let authority = environment.authority_key.clone();
                workspace.add_test_environment_runtime_placeholder_tab(
                    environment,
                    Some("root@remote-fixture-primary".to_string()),
                    ctx,
                );
                assert!(
                    !workspace.has_pending_environment_runtime_entry_for_authority(&authority),
                    "WindowSnapshot must not persist the runtime pending queue"
                );
                let restored_container_uuid = first_leaf_container_uuid(
                    &workspace.tabs[workspace.active_tab_index()]
                        .pane_group
                        .as_ref(ctx)
                        .snapshot(ctx),
                )
                .to_vec();
                (
                    workspace.snapshot(ctx.window_id(), false, ctx),
                    authority,
                    restored_container_uuid,
                )
            });

        let restored = restored_workspace(&mut app, snapshot);
        restored.read(&app, |workspace, ctx| {
            assert_eq!(
                workspace.current_environment_authority_key(ctx),
                authority,
                "cold restore must reactivate the persisted Environment authority"
            );
            assert_eq!(
                first_leaf_container_uuid(
                    &workspace.tabs[workspace.active_tab_index()]
                        .pane_group
                        .as_ref(ctx)
                        .snapshot(ctx),
                ),
                restored_container_uuid.as_slice(),
                "pending intent reconstruction must preserve the persisted placeholder container"
            );
            assert!(
                workspace.has_pending_terminal_for_authority(&authority),
                "cold-restored active Runtime placeholder must rebuild its PlainTerminal owner before transport Connected"
            );
        });
    });
}

#[test]
fn test_restored_error_window_snapshot_does_not_implicitly_connect() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        let (snapshot, authority) = workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                server.node_id.clone(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Error,
            );
            let authority = environment.authority_key.clone();
            workspace.set_active_tab_environment(environment);
            (workspace.snapshot(ctx.window_id(), false, ctx), authority)
        });

        let restored = restored_workspace(&mut app, snapshot);
        restored.read(&app, |workspace, _| {
            assert_eq!(
                workspace.environment_runtime_lifecycle_for_authority(&authority),
                Some(EnvironmentLifecycleState::Error),
                "full WindowSnapshot restore must preserve the persisted Error retry boundary"
            );
            assert_eq!(
                workspace
                    .current_environment_snapshot()
                    .as_ref()
                    .map(|environment| &environment.lifecycle_state),
                Some(&EnvironmentLifecycleState::Error),
                "startup implicit ensure must not replace restored Error with Connecting"
            );
            assert_eq!(
                workspace.environment_runtime_session_for_authority(&authority),
                None,
                "restoring an Error window must not create a transport generation"
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
                    "remote-fixture-primary".to_string(),
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
                    .current_environment_snapshot()
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
fn test_delete_confirmation_uses_resolved_session_snapshot() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            workspace.add_test_environment_runtime_placeholder_tab(
                environment,
                Some("root@remote-fixture-primary".to_string()),
                ctx,
            );

            let live_session = workspace
                .live_workspace_sessions(ctx)
                .into_iter()
                .find(|session| {
                    session.is_active
                        && session.environment_authority_key.as_deref() == Some(authority.as_str())
                })
                .expect("expected active environment live session");

            let plan = workspace.workspace_session_delete_plan(live_session.clone(), ctx);
            workspace.begin_workspace_session_delete_plan(&plan, ctx);
            workspace.delete_workspace_session_for_session(&live_session, ctx);
        });

        futures_lite::future::yield_now().await;

        workspace.update(&mut app, |workspace, _| {
            assert!(
                workspace
                    .session_navigator_sessions()
                    .into_iter()
                    .all(|session| {
                        session.id != "tab:99:leaf:0"
                            || session.environment_authority_key.as_deref() != Some("ssh:remote-fixture-primary")
                    }),
                "confirming delete must remove the resolved live session even after the row has been tombstoned before confirm"
            );
        });
    });
}

#[test]
fn test_deleting_only_live_environment_session_keeps_environment_selected() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        let authority = "ssh:remote-fixture-primary".to_string();
        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            assert_eq!(environment.authority_key, authority);
            // Install a real EnvironmentRuntimePlaceholder pane via the production
            // restore path so the environment tab actually surfaces a live session.
            workspace.add_test_environment_runtime_placeholder_tab(
                environment,
                Some("root@remote-fixture-primary".to_string()),
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
        // via activate_or_recreate_environment_tab_for_authority. Yield so those callbacks run
        // before asserting the post-delete Environment selection.
        futures_lite::future::yield_now().await;

        workspace.update(&mut app, |workspace, ctx| {
            assert_eq!(
                workspace.tab_count(),
                2,
                "after deleting the only live Environment session, the Environment tab must be recreated and remain selected alongside the current-app tab"
            );
            assert_eq!(
                workspace.current_environment_snapshot()
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
            assert!(
                !workspace.has_pending_environment_runtime_entry_for_authority(&authority),
                "deleting the final live session may recreate its Environment navigation container, but must not synthesize a replacement terminal"
            );
            assert!(
                workspace.active_tab_contains_environment_runtime_placeholder(ctx),
                "the recreated Environment tab should remain an inert navigation placeholder until an explicit creation action"
            );
        });
    });
}

#[test]
fn test_deleting_split_pane_session_keeps_focus_on_sibling_in_same_tab() {
    // EC-01: delete one pane in a split tab → focus stays on sibling, no tab jump.
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            let tab_index = workspace.active_tab_index();
            if let Some(tab_view) = workspace.get_pane_group_view(tab_index) {
                tab_view.update(ctx, |view, ctx| {
                    view.handle_action(&PaneGroupAction::Add(Direction::Right), ctx);
                });
            }

            let pane_group = workspace.tabs[tab_index].pane_group.clone();
            let visible = pane_group.as_ref(ctx).visible_pane_ids();
            assert_eq!(visible.len(), 2, "expected a two-pane split");

            let focused_before = pane_group.as_ref(ctx).focused_pane_id(ctx);
            let sibling = visible
                .iter()
                .copied()
                .find(|id| *id != focused_before)
                .expect("sibling pane");

            let deleted_session = workspace
                .live_workspace_sessions(ctx)
                .into_iter()
                .find(|session| {
                    session.is_active
                        && workspace
                            .locator_for_workspace_session_snapshot(session, ctx)
                            .is_some_and(|locator| locator.pane_id == focused_before)
                })
                .expect("expected active live session for focused pane");

            workspace.delete_workspace_session_for_session(&deleted_session, ctx);

            assert_eq!(
                workspace.active_tab_index(),
                tab_index,
                "delete in a split must not jump to another tab"
            );
            assert_eq!(
                workspace.tabs[tab_index]
                    .pane_group
                    .as_ref(ctx)
                    .visible_pane_ids()
                    .len(),
                1,
                "exactly one pane should remain after delete"
            );
            assert_eq!(
                workspace.tabs[tab_index]
                    .pane_group
                    .as_ref(ctx)
                    .focused_pane_id(ctx),
                sibling,
                "focus must stay on the sibling pane in the same tab"
            );

            let active_sessions: Vec<_> = workspace
                .session_navigator_sessions()
                .into_iter()
                .filter(|session| session.is_active)
                .collect();
            assert_eq!(active_sessions.len(), 1);
            assert!(
                workspace
                    .locator_for_workspace_session_snapshot(&active_sessions[0], ctx)
                    .is_some_and(|locator| locator.pane_id == sibling),
                "navigator active row must track the sibling pane"
            );
        });
    });
}

#[test]
fn test_deleting_local_tab_session_focuses_same_env_neighbor_tab() {
    // EC-02 / EC-16 (local): delete sole pane of a local tab → focus same-env neighbor.
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            workspace.add_terminal_tab(false, ctx);
            assert_eq!(workspace.tab_count(), 2);
            let deleted_tab = workspace.active_tab_index();
            assert_eq!(deleted_tab, 1);

            let deleted_session = workspace
                .live_workspace_sessions(ctx)
                .into_iter()
                .find(|session| session.is_active)
                .expect("expected active live session on new tab");

            workspace.delete_workspace_session_for_session(&deleted_session, ctx);

            assert_eq!(workspace.tab_count(), 1);
            assert_eq!(workspace.active_tab_index(), 0);
            let remaining_is_local = workspace.tabs[0].environment.as_ref().is_none_or(|env| {
                matches!(env.kind, EnvironmentKind::Local)
                    || ParsedEnvironmentAuthority::parse(&env.authority_key)
                        .uses_terminal_bootstrap()
            });
            assert!(
                remaining_is_local,
                "should remain on local / terminal-bootstrap tab, got {:?}",
                workspace.tabs[0].environment.as_ref().map(|e| &e.kind)
            );
            let active = workspace
                .session_navigator_sessions()
                .into_iter()
                .find(|session| session.is_active);
            assert!(active.is_some(), "neighbor local session should be active");
        });
    });
}

#[test]
fn test_deleting_only_live_local_session_does_not_close_window() {
    // EC-03 / EC-09: last local session must not close the window.
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            assert_eq!(workspace.tab_count(), 1);
            let deleted_session = workspace
                .live_workspace_sessions(ctx)
                .into_iter()
                .find(|session| session.is_active)
                .expect("expected active local session");
            let logical_key = Workspace::workspace_session_logical_key(&deleted_session);
            workspace.dispatch_session_navigator_state_action(
                session_navigator_reducer::SessionNavigatorAction::SelectionChanged {
                    session_logical_key: Some(logical_key),
                },
                ctx,
            );
            let state_before = workspace.snapshot_session_navigator_state();
            let rows_before = workspace
                .session_navigator_sessions()
                .into_iter()
                .map(|session| session.id)
                .collect::<Vec<_>>();

            workspace.delete_workspace_session_for_session_with_refused_side_effect(
                &deleted_session,
                ctx,
            );

            assert!(
                workspace.tab_count() >= 1,
                "deleting the last local session must not close the window"
            );
            assert_eq!(
                workspace.snapshot_session_navigator_state(),
                state_before,
                "物理关闭被拒绝时必须原子恢复 selection/lifecycle/order/identity/counters"
            );
            assert_eq!(
                workspace
                    .session_navigator_sessions()
                    .into_iter()
                    .map(|session| session.id)
                    .collect::<Vec<_>>(),
                rows_before,
                "物理关闭被拒绝时被删行必须恢复可见"
            );
        });
    });
}

#[test]
fn test_reorder_session_navigator_sessions_keeps_selected_row_id() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            workspace
                .restored_workspace_sessions
                .push(test_session_navigator_order_session("reorder-a", "A", 10));
            workspace
                .restored_workspace_sessions
                .push(test_session_navigator_order_session("reorder-b", "B", 20));
            workspace.sync_session_navigator_sessions(ctx);

            let sessions = workspace.session_navigator_sessions();
            let target = sessions
                .iter()
                .find(|session| session.id == "reorder-b")
                .expect("reorder-b");
            let target_key = Workspace::workspace_session_logical_key(target);
            workspace.dispatch_session_navigator_state_action(
                session_navigator_reducer::SessionNavigatorAction::SelectionChanged {
                    session_logical_key: Some(target_key.clone()),
                },
                ctx,
            );
            let target_row_id = Workspace::session_navigator_row_id_for_identity(
                &target_key,
                &workspace.snapshot_session_navigator_state(),
            );

            let ordered = sessions
                .iter()
                .rev()
                .map(Workspace::workspace_session_logical_key)
                .collect::<Vec<_>>();
            workspace.reorder_session_navigator_sessions(ordered, ctx);

            assert_eq!(
                workspace
                    .snapshot_session_navigator_state()
                    .selected_row_id
                    .as_deref(),
                Some(target_row_id.as_str()),
                "Workspace Reorder must keep selected_row_id on the same logical_key"
            );
        });
    });
}

#[test]
fn test_reorder_session_navigator_unit_moves_split_group() {
    // EC-17：拖动同屏 split 组时保持 leaf 相邻，并保持 selected_row_id。
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            workspace.sync_session_navigator_sessions(ctx);
            workspace
                .restored_workspace_sessions
                .push(test_session_navigator_order_session(
                    "order-key-unit-between",
                    "Between",
                    20,
                ));
            workspace.sync_session_navigator_sessions(ctx);

            let pane_group = workspace.active_tab_pane_group();
            pane_group.update(ctx, |panes, ctx| {
                panes.add_terminal_pane_with_options(
                    Direction::Right,
                    NewTerminalOptions::default(),
                    ctx,
                );
            });
            workspace.sync_session_navigator_sessions(ctx);

            let sessions = workspace.session_navigator_sessions();
            let units = super::session_navigator_reducer::build_reorder_units(&sessions);
            let group_unit = units
                .iter()
                .find(|unit| {
                    matches!(
                        unit,
                        super::session_navigator_reducer::ReorderUnit::Group { tab_index: 0, .. }
                    )
                })
                .expect("tab:0 split group");
            let group_id = group_unit.id();
            let from_index = units
                .iter()
                .position(|unit| unit.id() == group_id)
                .expect("group index");
            let active_before = workspace
                .snapshot_session_navigator_state()
                .selected_row_id
                .clone();

            // Move group past the virtual row (insert at end).
            workspace.reorder_session_navigator_unit(&group_id, units.len(), ctx);

            let after = workspace.session_navigator_sessions();
            let relevant: Vec<&str> = after
                .iter()
                .filter(|session| {
                    matches!(
                        session.id.as_str(),
                        "tab:0:leaf:0" | "tab:0:leaf:1" | "order-key-unit-between"
                    )
                })
                .map(|session| session.id.as_str())
                .collect();
            assert_eq!(
                relevant,
                vec!["order-key-unit-between", "tab:0:leaf:0", "tab:0:leaf:1",],
                "split group must move as one contiguous unit (from_index was {from_index})"
            );
            assert_eq!(
                workspace.snapshot_session_navigator_state().selected_row_id,
                active_before,
                "unit reorder must keep selected_row_id"
            );
        });
    });
}

#[test]
fn test_pin_workspace_session_does_not_change_focus() {
    // EC-10: pin/unpin must not change focus or active navigator row.
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            workspace
                .restored_workspace_sessions
                .push(test_session_navigator_order_session("pin-focus-a", "A", 10));
            workspace
                .restored_workspace_sessions
                .push(test_session_navigator_order_session("pin-focus-b", "B", 20));
            workspace.sync_session_navigator_sessions(ctx);

            let focused_before = workspace
                .tabs
                .get(workspace.active_tab_index())
                .map(|tab| tab.pane_group.as_ref(ctx).focused_pane_id(ctx));
            let active_before = workspace
                .session_navigator_sessions()
                .into_iter()
                .find(|session| session.is_active)
                .map(|session| session.id);

            let pin_target = workspace
                .session_navigator_sessions()
                .into_iter()
                .find(|session| session.id == "pin-focus-b")
                .expect("pin target");
            let pin_target = Workspace::workspace_session_action_target(&pin_target);
            workspace.toggle_workspace_session_pinned(&pin_target, true, ctx);

            let focused_after = workspace
                .tabs
                .get(workspace.active_tab_index())
                .map(|tab| tab.pane_group.as_ref(ctx).focused_pane_id(ctx));
            let active_after = workspace
                .session_navigator_sessions()
                .into_iter()
                .find(|session| session.is_active)
                .map(|session| session.id);

            assert_eq!(
                focused_before, focused_after,
                "pin must not change physical pane focus"
            );
            assert_eq!(
                active_before, active_after,
                "pin must not change navigator active row"
            );

            // Always unpin: toggle persists to ~/.ashide/session_state.json and
            // would otherwise pollute later tests that assert empty pinned state.
            workspace.toggle_workspace_session_pinned(&pin_target, false, ctx);
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
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let session_id = CoreSessionId::from(9021);
            workspace.environments_mut().mark_connecting(
                environment.clone(),
                session_id,
                PathBuf::from("/tmp/ashide-test-delete-reselect-same-env.sock"),
            );
            workspace
                .environments_mut()
                .mark_connected_session(session_id, HostId::new("test-host".to_string()));

            // Two tabs in the *same* Environment, so deleting the active session can
            // reselect a sibling session within that Environment. Each restored
            // runtime tab installs an EnvironmentRuntimePlaceholder pane and thus
            // surfaces its own live session under the shared authority.
            workspace.add_test_environment_runtime_placeholder_tab(
                environment.clone(),
                Some("root@remote-fixture-primary".to_string()),
                ctx,
            );
            workspace.add_test_environment_runtime_placeholder_tab(
                environment,
                Some("root@remote-fixture-primary".to_string()),
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
                workspace.current_environment_snapshot()
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
                .session_navigator_sessions()
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
                "remote-fixture-primary".to_string(),
                &first_server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let first_authority = first_environment.authority_key.clone();
            let first_session_id = CoreSessionId::from(9121);
            workspace.environments_mut().mark_connecting(
                first_environment.clone(),
                first_session_id,
                PathBuf::from("/tmp/ashide-test-delete-stay-first-env.sock"),
            );
            workspace
                .environments_mut()
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

            let mut second_server = warp_ssh_manager::SshServerInfo::new_default(
                "remote-fixture-tertiary".to_string(),
            );
            second_server.host = "remote-fixture-tertiary".to_string();
            second_server.username = "root".to_string();
            let second_environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "remote-fixture-tertiary".to_string(),
                &second_server,
                Some("/root/other-project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let second_authority = second_environment.authority_key.clone();
            workspace.add_test_environment_runtime_placeholder_tab(
                second_environment,
                Some("root@remote-fixture-tertiary".to_string()),
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
fn test_environment_runtime_live_placeholder_keeps_registered_cli_agent_session_after_tab_switch() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let terminal_options = test_environment_runtime_pty_options(CoreSessionId::from(9101), ctx);
            workspace.add_tab_with_pane_layout(
                PanesLayout::SingleTerminal(Box::new(terminal_options)),
                Arc::new(HashMap::new()),
                Some("root@remote-fixture-primary".to_string()),
                ctx,
            );
            workspace.apply_active_tab_environment(environment, ctx);
            let environment_tab_index = workspace.active_tab_index();
            let terminal_view = workspace
                .active_tab_pane_group()
                .as_ref(ctx)
                .active_session_view(ctx)
                .expect("environment runtime terminal should be active");

            CLIAgentSessionsModel::handle(ctx).update(ctx, |sessions, ctx| {
                sessions.set_session(
                    terminal_view.id(),
                    CLIAgentSession {
                        agent: CLIAgent::Codex,
                        status: CLIAgentSessionStatus::InProgress,
                        session_context: CLIAgentSessionContext {
                            cwd: Some("/root/project".to_string()),
                            session_id: Some("codex-remote-live-session".to_string()),
                            fallback_title: Some("Fixed Remote Codex".to_string()),
                            ..Default::default()
                        },
                        input_state: CLIAgentInputState::Closed,
                        should_auto_toggle_input: false,
                        listener: None,
                        plugin_version: None,
                        environment_host_key: Some("root@remote-fixture-primary".to_string()),
                        draft_text: None,
                        custom_command_prefix: Some("codex".to_string()),
                    },
                    ctx,
                );
            });

            workspace.activate_tab_internal(0, ctx);
            workspace.activate_tab_internal(environment_tab_index, ctx);

            let live_session = workspace
                .live_workspace_sessions(ctx)
                .into_iter()
                .find(|session| session.environment_authority_key.as_deref() == Some(authority.as_str()))
                .expect("expected the remote environment live session after switching tabs");

            assert!(
                live_session.is_live_container,
                "the row must stay bound to the physical tab/pane live container"
            );
            assert_eq!(live_session.kind, WorkspaceSessionKind::AgentTerminal);
            assert_eq!(
                live_session.cli_agent_session_id.as_deref(),
                Some("codex-remote-live-session"),
                "switching tabs must not degrade the restored CLI-agent pane into a plain environment terminal"
            );
            assert_eq!(live_session.cli_agent.as_deref(), Some("Codex"));
            assert_eq!(live_session.cli_command.as_deref(), Some("codex"));
            assert_eq!(
                live_session.label.as_deref(),
                None,
                "live pane title must stay PaneConfiguration-owned rather than inherit provider fallback title"
            );
            assert_eq!(
                Workspace::workspace_session_display_order_key(&live_session),
                Workspace::workspace_session_logical_key(&live_session),
                "live rows keep physical identity; durable agent state is bridged through metadata and aliases"
            );
        });
    });
}

#[test]
fn test_remote_runtime_agent_row_stays_live_when_active_block_is_non_runtime_subshell() {
    // 回归用户报告:远程 Environment 里有一个正在运行的 CLI-agent 终端。agent 在
    // 子 shell 里跑,active block 因此切到一个未登记为 Environment Runtime 的
    // session。旧实现用易失的 active_session_uses_environment_runtime 判断 liveness,
    // 会把这行 live 远程行丢掉 → 退化成 virtual "历史" 行 → 丢失激活高亮、丢失活动
    // 点、rename 走远程 alias RPC(又丑又不生效)。liveness 必须由稳定的 runtime
    // transport 身份决定。
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let terminal_options = test_environment_runtime_pty_options(CoreSessionId::from(9105), ctx);
            workspace.add_tab_with_pane_layout(
                PanesLayout::SingleTerminal(Box::new(terminal_options)),
                Arc::new(HashMap::new()),
                Some("root@remote-fixture-primary".to_string()),
                ctx,
            );
            workspace.apply_active_tab_environment(environment, ctx);
            let terminal_view = workspace
                .active_tab_pane_group()
                .as_ref(ctx)
                .active_session_view(ctx)
                .expect("environment runtime terminal should be active");

            // Bootstrap the runtime session, then move the active block onto a
            // *different*, non-runtime session id — exactly what happens when a
            // CLI agent runs inside a subshell. This flips the volatile
            // active-block runtime check to false.
            terminal_view.update(ctx, |view, _| {
                use crate::terminal::model::ansi::Handler;
                let mut model = view.model.lock();
                model.init_shell(crate::terminal::model::ansi::InitShellValue {
                    session_id: CoreSessionId::from(9105),
                    shell: "bash".to_owned(),
                    ..Default::default()
                });
                model.bootstrapped(crate::terminal::model::ansi::BootstrappedValue {
                    shell: "bash".to_owned(),
                    ..Default::default()
                });
                model.start_command_execution();
                let blocks = model.block_list_mut();
                blocks
                    .active_block_for_test()
                    .set_session_id(CoreSessionId::from(7777));
                assert!(
                    !model.active_block_uses_environment_runtime(),
                    "precondition: the active block must resolve to a non-runtime session"
                );
                assert!(
                    model.is_environment_runtime_transport(),
                    "precondition: the terminal must still be a runtime transport"
                );
            });

            CLIAgentSessionsModel::handle(ctx).update(ctx, |sessions, ctx| {
                sessions.set_session(
                    terminal_view.id(),
                    CLIAgentSession {
                        agent: CLIAgent::Codex,
                        status: CLIAgentSessionStatus::InProgress,
                        session_context: CLIAgentSessionContext {
                            cwd: Some("/root/project".to_string()),
                            session_id: Some("codex-remote-live-session".to_string()),
                            fallback_title: Some("Remote Codex".to_string()),
                            ..Default::default()
                        },
                        input_state: CLIAgentInputState::Closed,
                        should_auto_toggle_input: false,
                        listener: None,
                        plugin_version: None,
                        environment_host_key: Some("root@remote-fixture-primary".to_string()),
                        draft_text: None,
                        custom_command_prefix: Some("codex".to_string()),
                    },
                    ctx,
                );
            });

            workspace.sync_session_navigator_sessions(ctx);
            workspace.notify_session_navigator_focus_changed(ctx);

            let rows = workspace.session_navigator_sessions();
            let remote_rows: Vec<_> = rows
                .iter()
                .filter(|s| s.environment_authority_key.as_deref() == Some(authority.as_str()))
                .collect();
            assert_eq!(
                remote_rows.len(),
                1,
                "the running remote agent must surface as exactly one row; rows={rows:#?}"
            );
            let row = remote_rows[0];
            assert!(
                row.is_live_container(),
                "the row must be a live container so it shows the activity dot and renames the pane title; row={row:#?}"
            );
            assert!(
                row.is_active,
                "the focused running remote agent row must project is_active for the Navigator highlight; row={row:#?}"
            );
        });
    });
}

#[test]
fn test_environment_runtime_live_placeholder_does_not_apply_durable_alias() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            let terminal_options = test_environment_runtime_pty_options(CoreSessionId::from(9102), ctx);
            workspace.add_tab_with_pane_layout(
                PanesLayout::SingleTerminal(Box::new(terminal_options)),
                Arc::new(HashMap::new()),
                Some("root@remote-fixture-primary".to_string()),
                ctx,
            );
            workspace.apply_active_tab_environment(environment, ctx);
            let terminal_view = workspace
                .active_tab_pane_group()
                .as_ref(ctx)
                .active_session_view(ctx)
                .expect("environment runtime terminal should be active");

            CLIAgentSessionsModel::handle(ctx).update(ctx, |sessions, ctx| {
                sessions.set_session(
                    terminal_view.id(),
                    CLIAgentSession {
                        agent: CLIAgent::Codex,
                        status: CLIAgentSessionStatus::InProgress,
                        session_context: CLIAgentSessionContext {
                            cwd: Some("/root/project".to_string()),
                            session_id: Some("codex-remote-live-session".to_string()),
                            fallback_title: Some("Environment Codex".to_string()),
                            ..Default::default()
                        },
                        input_state: CLIAgentInputState::Closed,
                        should_auto_toggle_input: false,
                        listener: None,
                        plugin_version: None,
                        environment_host_key: Some("root@remote-fixture-primary".to_string()),
                        draft_text: None,
                        custom_command_prefix: Some("codex".to_string()),
                    },
                    ctx,
                );
            });

            let durable_key = format!(
                "{}::agent:Codex:codex-remote-live-session",
                WorkspaceSessionSnapshot::logical_environment_key(Some(authority.as_str()))
            );
            workspace.environments_mut().set_cli_agent_session_user_state(
                authority.clone(),
                crate::workspace::environment_runtime::EnvironmentCliAgentSessionUserState {
                    aliases: HashMap::from([(durable_key, "固定远程名".to_string())]),
                    pinned: HashSet::new(),
                },
            );
            workspace.sync_session_navigator_sessions(ctx);

            let session = workspace
                .session_navigator_sessions()
                .into_iter()
                .find(|session| session.environment_authority_key.as_deref() == Some(authority.as_str()))
                .expect("expected merged remote live session");

            assert_ne!(
                session.label.as_deref(),
                Some("固定远程名"),
                "live rows own their title in PaneConfiguration and must ignore virtual durable aliases"
            );
            assert_ne!(
                session.label.as_deref(),
                Some("remote-fixture-primary"),
                "a virtual alias must not turn into an environment-derived live title"
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
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            // Install a real EnvironmentRuntimePlaceholder pane (as the production
            // restore path does) so `live_workspace_sessions` actually surfaces an
            // environment session. Tagging the tab via `set_active_tab_environment`
            // alone leaves the active pane a plain terminal leaf with no placeholder,
            // so no environment live session is produced.
            workspace.add_test_environment_runtime_placeholder_tab(
                environment,
                Some("root@remote-fixture-primary".to_string()),
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
fn test_environment_runtime_placeholder_preserves_container_identity_across_environment_round_trip()
{
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            workspace.add_test_environment_runtime_placeholder_tab(
                environment,
                Some("root@remote-fixture-primary".to_string()),
                ctx,
            );
            let environment_tab_index = workspace.active_tab_index();

            let before = workspace
                .live_workspace_sessions(ctx)
                .into_iter()
                .find(|session| {
                    session.environment_authority_key.as_deref() == Some(authority.as_str())
                })
                .expect("expected environment runtime placeholder session");
            let container_uuid = before
                .container_uuid
                .clone()
                .expect("placeholder live container must expose a stable UUID");
            let logical_key = before.logical_key();

            workspace.activate_tab_internal(0, ctx);
            workspace.activate_tab_internal(environment_tab_index, ctx);

            let after = workspace
                .live_workspace_sessions(ctx)
                .into_iter()
                .find(|session| {
                    session.environment_authority_key.as_deref() == Some(authority.as_str())
                })
                .expect("expected placeholder after environment round-trip");
            assert_eq!(after.container_uuid.as_deref(), Some(container_uuid.as_slice()));
            assert_eq!(after.logical_key(), logical_key);
            assert_ne!(
                after.logical_key(),
                format!("{authority}::source:{}", after.id),
                "placeholder 禁止退回 tab/leaf locator 身份"
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
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            // Install a real EnvironmentRuntimePlaceholder pane via the production
            // restore path so the environment tab surfaces a session that can be
            // targeted for deletion.
            workspace.add_test_environment_runtime_placeholder_tab(
                environment,
                Some("root@remote-fixture-primary".to_string()),
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
                workspace.current_environment_snapshot()
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
                workspace.current_environment_snapshot()
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
                    .session_navigator_sessions()
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
fn test_deleting_active_current_app_session_with_neighbor_tab_does_not_create_empty_replacement() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            assert_eq!(workspace.tab_count(), 1);
            let current_app_tab_index = workspace.active_tab_index();

            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            workspace.add_test_environment_runtime_placeholder_tab(
                environment,
                Some("root@remote-fixture-primary".to_string()),
                ctx,
            );
            assert_eq!(workspace.tab_count(), 2);
            workspace.activate_tab_internal(current_app_tab_index, ctx);

            let deleted_session = workspace
                .live_workspace_sessions(ctx)
                .into_iter()
                .find(|session| {
                    session.is_active
                        && session.environment_authority_key.as_deref().is_none_or(|authority| {
                            ParsedEnvironmentAuthority::parse(authority)
                                .uses_terminal_bootstrap()
                        })
                })
                .expect("expected active current-app live session");

            workspace.delete_workspace_session_for_session(&deleted_session, ctx);

            assert_eq!(
                workspace.tab_count(),
                1,
                "deleting a current-app live session with an existing neighbor tab must not create an empty replacement tab"
            );
            assert_eq!(
                workspace.tabs[workspace.active_tab_index()]
                    .environment
                    .as_ref()
                    .map(|environment| environment.authority_key.as_str()),
                Some(authority.as_str())
            );
            assert!(
                workspace.tabs.iter().all(|tab| tab.environment.is_some()),
                "no replacement current-app tab should be left behind"
            );
        });
    });
}

#[test]
fn test_deleting_only_live_current_app_session_creates_replacement_before_close() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            assert_eq!(
                workspace.tab_count(),
                1,
                "test starts with exactly one current-app tab"
            );
            assert_eq!(
                workspace.current_environment_snapshot()
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Local)
            );

            let deleted_session = workspace
                .live_workspace_sessions(ctx)
                .into_iter()
                .find(|session| {
                    session.is_active
                        && session.environment_authority_key.as_deref().is_none_or(|authority| {
                            ParsedEnvironmentAuthority::parse(authority)
                                .uses_terminal_bootstrap()
                        })
                })
                .expect("expected active current-app live session");

            workspace.delete_workspace_session_for_session(&deleted_session, ctx);

            assert!(
                workspace.tab_count() >= 1,
                "deleting the first/current-app live row must not leave the workspace with no visible tab"
            );
            assert_eq!(workspace.active_tab_index(), 0);
            assert_eq!(
                workspace.tabs[workspace.active_tab_index()]
                    .environment
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Local),
                "delete fallback should keep a replacement current-app tab active instead of letting window close/minimize"
            );
            assert_eq!(
                workspace.current_environment_snapshot()
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Local)
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
                    "remote-fixture-primary".to_string(),
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
                workspace.current_environment_snapshot()
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
                    "remote-fixture-primary".to_string(),
                    &server,
                    Some("/root/project".to_string()),
                    EnvironmentLifecycleState::Connected,
                ),
            );
            workspace.add_explicit_terminal_bootstrap_default_tab(None, ctx);
            assert_eq!(
                workspace.current_environment_snapshot()
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
                workspace.current_environment_snapshot()
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
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            workspace.set_active_tab_environment(environment);
            let environment_tab_index = workspace.active_tab_index();

            workspace.add_explicit_terminal_bootstrap_default_tab(None, ctx);
            assert_eq!(
                workspace.current_environment_snapshot()
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Local),
                "test setup should make current-app/local active before close-other chooses the Environment tab"
            );

            workspace.close_other_tabs(environment_tab_index, true, ctx);

            assert_eq!(workspace.tab_count(), 1);
            assert_eq!(
                workspace.current_environment_snapshot()
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
                    "remote-fixture-primary".to_string(),
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
                workspace.current_environment_snapshot()
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
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            workspace.set_active_tab_environment(environment);
            let environment_tab_index = workspace.active_tab_index();

            workspace.activate_tab_internal(0, ctx);
            assert_eq!(
                workspace.current_environment_snapshot()
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Local),
                "test setup should make a current-app tab left of the Environment active"
            );

            workspace.close_tabs_direction(environment_tab_index, TabMovement::Left, true, ctx);

            assert_eq!(workspace.tab_count(), 1);
            assert_eq!(
                workspace.current_environment_snapshot()
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
                    "remote-fixture-primary".to_string(),
                    &server,
                    Some("/root/project".to_string()),
                    EnvironmentLifecycleState::Connected,
                ),
            );
            assert_eq!(
                workspace.current_environment_snapshot()
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Ssh),
                "test setup should make an Environment tab right of the current-app target active"
            );

            workspace.close_tabs_direction(current_app_tab_index, TabMovement::Right, true, ctx);

            assert_eq!(workspace.tab_count(), 1);
            assert_eq!(
                workspace.current_environment_snapshot()
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
                    "remote-fixture-primary".to_string(),
                    &server,
                    Some("/root/project".to_string()),
                    EnvironmentLifecycleState::Connected,
                );
            workspace.environments_mut().mark_connecting(
                environment.clone(),
                runtime_session_id,
                PathBuf::from("/tmp/ashide-test-close-pane-runtime.sock"),
            );
            workspace.environments_mut().mark_connected_session(
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
        futures_lite::future::yield_now().await;

        let (authority, active_before_close) = workspace.read(&app, |workspace, _| {
            let authority = workspace
                .current_environment_snapshot()
                .as_ref()
                .expect("Environment should be active")
                .authority_key
                .clone();
            let active_sessions = workspace
                .session_navigator_sessions()
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
        futures_lite::future::yield_now().await;

        workspace.read(&app, |workspace, _| {
            let active_sessions = workspace
                .session_navigator_sessions()
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
                    "remote-fixture-primary".to_string(),
                    &server,
                    Some("/root/project".to_string()),
                    EnvironmentLifecycleState::Connected,
                );
            workspace.environments_mut().mark_connecting(
                environment.clone(),
                runtime_session_id,
                PathBuf::from("/tmp/ashide-test-undo-pane-runtime.sock"),
            );
            workspace.environments_mut().mark_connected_session(
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

        workspace.update(&mut app, |workspace, ctx| {
            workspace.sync_session_navigator_sessions(ctx);
        });

        let authority = workspace.read(&app, |workspace, ctx| {
            let authority = workspace
                .current_environment_snapshot()
                .as_ref()
                .expect("Environment should be active")
                .authority_key
                .clone();
            let active_session = workspace
                .session_navigator_sessions()
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
                .session_navigator_sessions()
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
                    "remote-fixture-primary".to_string(),
                    &server,
                    Some("/root/project".to_string()),
                    EnvironmentLifecycleState::Connected,
                );
            let authority = environment.authority_key.clone();
            workspace.set_active_tab_environment(environment);
            let environment_tab_index = workspace.active_tab_index();

            workspace.close_tab(environment_tab_index, true, true, ctx);
            assert_eq!(
                workspace.current_environment_snapshot().as_ref().map(|environment| &environment.kind),
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
                workspace.current_environment_snapshot()
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
                    "remote-fixture-primary".to_string(),
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
fn test_switching_between_environments_restores_each_last_active_tab() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            let first_local_tab_id = workspace.tabs[0].pane_group.id();
            workspace.add_terminal_tab(false, ctx);
            let last_local_tab_id = workspace.active_tab_pane_group().id();
            assert_ne!(last_local_tab_id, first_local_tab_id);

            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            workspace.add_test_environment_runtime_placeholder_tab(
                environment.clone(),
                Some("remote one".to_owned()),
                ctx,
            );
            workspace.add_test_environment_runtime_placeholder_tab(
                environment,
                Some("remote two".to_owned()),
                ctx,
            );
            let last_remote_tab_id = workspace.active_tab_pane_group().id();
            let original_tab_count = workspace.tab_count();
            let open_navigation_keys = workspace
                .open_environment_snapshots(ctx)
                .into_iter()
                .map(|environment| {
                    ParsedEnvironmentAuthority::parse(&environment.authority_key)
                        .navigation_key()
                        .to_owned()
                })
                .collect::<HashSet<_>>();
            assert!(open_navigation_keys.contains("local"));
            assert!(open_navigation_keys.contains(&authority));

            workspace.handle_action(
                &WorkspaceAction::SwitchEnvironment {
                    authority_key: "local".to_owned(),
                },
                ctx,
            );
            assert_eq!(workspace.active_tab_pane_group().id(), last_local_tab_id);

            workspace.handle_action(
                &WorkspaceAction::SwitchEnvironment {
                    authority_key: authority.clone(),
                },
                ctx,
            );
            assert_eq!(workspace.active_tab_pane_group().id(), last_remote_tab_id);

            workspace.handle_action(
                &WorkspaceAction::SwitchEnvironment {
                    authority_key: "local:/tmp/another-root".to_owned(),
                },
                ctx,
            );
            assert_eq!(
                workspace.active_tab_pane_group().id(),
                last_local_tab_id,
                "local authority aliases must share one Current App navigation context"
            );
            assert_eq!(
                workspace.tab_count(),
                original_tab_count,
                "Environment round-trips must reactivate durable containers instead of creating replacement tabs"
            );
        });
    });
}

#[test]
fn test_environment_runtime_session_ids_are_global_across_workspaces() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let first_workspace = mock_workspace(&mut app);
        let second_workspace = mock_workspace(&mut app);
        let first_session_id = first_workspace.update(&mut app, |workspace, ctx| {
            workspace.next_environment_runtime_session_id(ctx)
        });
        let second_session_id = second_workspace.update(&mut app, |workspace, ctx| {
            workspace.next_environment_runtime_session_id(ctx)
        });

        assert_ne!(
            first_session_id, second_session_id,
            "synthetic Environment sessions share one app-global transport manager and must never collide across windows"
        );

        let first_authority = "ssh:ssh-config:first";
        let second_authority = "ssh:ssh-config:second";
        first_workspace.update(&mut app, |workspace, _ctx| {
            workspace.mark_environment_runtime_connecting(
                EnvironmentSnapshot {
                    label: "first".to_owned(),
                    kind: EnvironmentKind::Ssh,
                    authority_key: first_authority.to_owned(),
                    connection_ref: Some("ssh-config:first".to_owned()),
                    active_workspace_root: None,
                    lifecycle_state: EnvironmentLifecycleState::Connecting,
                },
                first_session_id,
                PathBuf::from("/tmp/first-environment-runtime.sock"),
            );
        });
        second_workspace.update(&mut app, |workspace, _ctx| {
            workspace.mark_environment_runtime_connecting(
                EnvironmentSnapshot {
                    label: "second".to_owned(),
                    kind: EnvironmentKind::Ssh,
                    authority_key: second_authority.to_owned(),
                    connection_ref: Some("ssh-config:second".to_owned()),
                    active_workspace_root: None,
                    lifecycle_state: EnvironmentLifecycleState::Connecting,
                },
                second_session_id,
                PathBuf::from("/tmp/second-environment-runtime.sock"),
            );
        });

        first_workspace.read(&app, |workspace, _ctx| {
            assert!(workspace.owns_environment_runtime_transport_session(first_session_id));
            assert!(!workspace.owns_environment_runtime_transport_session(second_session_id));
        });
        second_workspace.read(&app, |workspace, _ctx| {
            assert!(workspace.owns_environment_runtime_transport_session(second_session_id));
            assert!(!workspace.owns_environment_runtime_transport_session(first_session_id));
        });
    });
}

#[test]
fn test_switching_between_environments_preserves_each_active_pane() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            let local_group = workspace.active_tab_pane_group();
            let local_group_id = local_group.id();
            let local_focused_pane = local_group.update(ctx, |panes, ctx| {
                panes.add_terminal_pane_with_options(
                    Direction::Right,
                    NewTerminalOptions::default(),
                    ctx,
                );
                let pane_id = panes.pane_id_by_index(0).expect("local split pane");
                panes.focus_pane_by_id(pane_id, ctx);
                pane_id
            });

            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "remote-fixture-primary".to_owned(),
                &server,
                Some("/root/project".to_owned()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            workspace.add_test_environment_runtime_placeholder_tab(
                environment,
                Some("remote".to_owned()),
                ctx,
            );
            let remote_group = workspace.active_tab_pane_group();
            let remote_group_id = remote_group.id();
            let remote_focused_pane = remote_group.update(ctx, |panes, ctx| {
                panes.add_terminal_pane_with_options(
                    Direction::Right,
                    NewTerminalOptions::default(),
                    ctx,
                );
                let pane_id = panes.pane_id_by_index(1).expect("remote split pane");
                panes.focus_pane_by_id(pane_id, ctx);
                pane_id
            });

            workspace.handle_action(
                &WorkspaceAction::SwitchEnvironment {
                    authority_key: "local".to_owned(),
                },
                ctx,
            );
            assert_eq!(workspace.active_tab_pane_group().id(), local_group_id);
            assert_eq!(
                workspace.active_tab_pane_group().as_ref(ctx).focused_pane_id(ctx),
                local_focused_pane
            );

            workspace.handle_action(
                &WorkspaceAction::SwitchEnvironment {
                    authority_key: authority,
                },
                ctx,
            );
            assert_eq!(workspace.active_tab_pane_group().id(), remote_group_id);
            assert_eq!(
                workspace.active_tab_pane_group().as_ref(ctx).focused_pane_id(ctx),
                remote_focused_pane
            );
        });
    });
}

#[test]
fn test_environment_last_active_tab_fallback_and_reorder() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            let local_tab_id = workspace.active_tab_pane_group().id();
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            workspace.add_test_environment_runtime_placeholder_tab(
                environment.clone(),
                Some("remote one".to_owned()),
                ctx,
            );
            let first_remote_tab_id = workspace.active_tab_pane_group().id();
            workspace.add_test_environment_runtime_placeholder_tab(
                environment,
                Some("remote two".to_owned()),
                ctx,
            );
            let remembered_remote_tab_id = workspace.active_tab_pane_group().id();

            workspace.handle_action(
                &WorkspaceAction::SwitchEnvironment {
                    authority_key: "local".to_owned(),
                },
                ctx,
            );
            let remembered_index = workspace
                .tabs
                .iter()
                .position(|tab| tab.pane_group.id() == remembered_remote_tab_id)
                .expect("remembered remote tab");
            let remembered_tab = workspace.tabs.remove(remembered_index);
            workspace.tabs.insert(0, remembered_tab);
            workspace.reconcile_workspace_state_after_tab_collection_changed(
                workspace
                    .tabs
                    .iter()
                    .position(|tab| tab.pane_group.id() == local_tab_id)
                    .expect("active local tab after reorder"),
                ctx,
            );

            workspace.handle_action(
                &WorkspaceAction::SwitchEnvironment {
                    authority_key: authority.clone(),
                },
                ctx,
            );
            assert_eq!(workspace.active_tab_pane_group().id(), remembered_remote_tab_id);

            workspace.handle_action(
                &WorkspaceAction::SwitchEnvironment {
                    authority_key: "local".to_owned(),
                },
                ctx,
            );
            let remembered_index = workspace
                .tabs
                .iter()
                .position(|tab| tab.pane_group.id() == remembered_remote_tab_id)
                .expect("remembered remote tab before close");
            workspace.close_tab(remembered_index, true, false, ctx);

            workspace.handle_action(
                &WorkspaceAction::SwitchEnvironment {
                    authority_key: authority.clone(),
                },
                ctx,
            );
            assert_eq!(workspace.active_tab_pane_group().id(), first_remote_tab_id);

            workspace.handle_action(
                &WorkspaceAction::SwitchEnvironment {
                    authority_key: "local".to_owned(),
                },
                ctx,
            );
            workspace.handle_action(
                &WorkspaceAction::SwitchEnvironment {
                    authority_key: authority,
                },
                ctx,
            );
            assert_eq!(
                workspace.active_tab_pane_group().id(),
                first_remote_tab_id,
                "fallback activation must rewrite stale remembered-tab state"
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
                "remote-fixture-primary".to_string(),
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
                workspace.current_environment_snapshot()
                    .as_ref()
                    .map(|environment| &environment.kind),
                Some(&EnvironmentKind::Local)
            );
        });
    });
}

#[test]
fn test_closing_active_tab_prefers_same_navigation_environment() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            let local_tab_id = workspace.active_tab_pane_group().id();
            let server = test_ssh_server_for_environment_tests();
            let first_environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/first".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = first_environment.authority_key.clone();
            workspace.add_test_environment_runtime_placeholder_tab(
                first_environment,
                Some("remote one".to_owned()),
                ctx,
            );
            let first_remote_tab_id = workspace.active_tab_pane_group().id();
            workspace.tabs.swap(0, 1);
            assert_eq!(workspace.tabs[1].pane_group.id(), local_tab_id);

            let second_environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/second".to_string()),
                EnvironmentLifecycleState::Error,
            );
            assert_eq!(second_environment.authority_key, authority);
            workspace.add_test_environment_runtime_placeholder_tab(
                second_environment,
                Some("remote two".to_owned()),
                ctx,
            );
            let active_remote_index = workspace.active_tab_index();

            workspace.close_tab(active_remote_index, true, false, ctx);

            assert_eq!(workspace.active_tab_pane_group().id(), first_remote_tab_id);
            assert_eq!(
                workspace.current_environment_snapshot()
                    .as_ref()
                    .map(|environment| environment.authority_key.as_str()),
                Some(authority.as_str()),
                "snapshot lifecycle/root differences must not cause a cross-environment focus jump"
            );
        });
    });
}

#[test]
fn test_disconnect_environment_closes_all_authority_tabs_and_restores_local() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            workspace.add_terminal_tab(false, ctx);
            let remembered_local_tab_id = workspace.active_tab_pane_group().id();

            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "remote-fixture-primary".to_string(),
                &server,
                Some("/root/project".to_string()),
                EnvironmentLifecycleState::Connected,
            );
            let authority = environment.authority_key.clone();
            workspace.add_test_environment_runtime_placeholder_tab(
                environment.clone(),
                Some("remote one".to_owned()),
                ctx,
            );
            let first_pending_pane_id = workspace
                .active_tab_pane_group()
                .as_ref(ctx)
                .focused_pane_id(ctx);
            workspace.queue_environment_runtime_intent(
                &authority,
                EnvironmentEntryIntent::PlainTerminal(PlainTerminalEntry::default_tab(false)),
                first_pending_pane_id,
            );
            workspace.add_test_environment_runtime_placeholder_tab(
                environment,
                Some("remote two".to_owned()),
                ctx,
            );
            let second_pending_pane_id = workspace
                .active_tab_pane_group()
                .as_ref(ctx)
                .focused_pane_id(ctx);
            workspace.queue_environment_runtime_intent(
                &authority,
                EnvironmentEntryIntent::PlainTerminal(PlainTerminalEntry::default_tab(false)),
                second_pending_pane_id,
            );
            assert_eq!(
                workspace
                    .environments
                    .entry_for_authority(&authority)
                    .expect("disconnect setup must retain the environment row")
                    .pending_materializations
                    .len(),
                2
            );

            workspace.handle_action(
                &WorkspaceAction::DisconnectEnvironment {
                    authority_key: authority.clone(),
                },
                ctx,
            );

            assert!(workspace.tabs.iter().all(|tab| {
                tab.environment
                    .as_ref()
                    .is_none_or(|environment| environment.authority_key != authority)
            }));
            assert_eq!(workspace.active_tab_pane_group().id(), remembered_local_tab_id);
            assert_eq!(workspace.environments.last_active_tab(&authority), None);
            assert!(workspace.environments.entry_for_authority(&authority).is_none());
            assert_eq!(
                workspace.current_environment_snapshot()
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
fn test_plain_terminal_entry_uses_shared_environment_backend_for_local_and_runtime() {
    const VIEW_RS: &str = include_str!("view.rs");
    let helper = VIEW_RS
        .split_once("fn add_default_plain_terminal_tab_route_aware")
        .expect("default plain-terminal helper must exist")
        .1
        .split_once("fn add_welcome_tab")
        .expect("helper boundary must remain visible")
        .0;
    assert!(helper.contains("EnvironmentBackendKind::for_environment"));
    assert!(helper.contains("EnvironmentEntryIntent::PlainTerminal"));
    assert!(helper.contains(".deliver_entry("));

    let action = VIEW_RS
        .split_once("AddTerminalTab { hide_homepage } =>")
        .expect("AddTerminalTab action arm must exist")
        .1
        .split_once("OpenEnvironmentRuntimeTerminal")
        .expect("AddTerminalTab action boundary must remain visible")
        .0;
    assert!(action.contains("EnvironmentBackendKind::for_environment"));
    assert!(action.contains("EnvironmentEntryIntent::PlainTerminal"));
    assert!(action.contains(".deliver_entry("));
    assert!(!action.contains("try_route_current_runtime_environment_entry"));
}

#[test]
fn test_restored_conversation_new_tab_uses_shared_fork_delivery() {
    const VIEW_RS: &str = include_str!("view.rs");
    let function = VIEW_RS
        .split_once("fn deliver_restored_conversation")
        .expect("restored conversation delivery function must exist")
        .1
        .split_once("fn restore_or_navigate_to_conversation")
        .expect("restored conversation function boundary must remain visible")
        .0;
    let new_tab = function
        .split_once("RestoreConversationLayout::NewTab =>")
        .expect("new-tab restore arm must exist")
        .1
        .split_once("RestoreConversationLayout::SplitPane")
        .expect("new-tab restore arm boundary must remain visible")
        .0;
    assert!(new_tab.contains("EnvironmentBackendKind::for_environment"));
    assert!(new_tab.contains("EnvironmentEntryIntent::ForkedConversation"));
    assert!(new_tab.contains(".deliver_entry("));
    assert!(!new_tab.contains("uses_terminal_bootstrap"));
}

#[test]
fn test_restored_conversation_split_pane_reuses_local_environment_container() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        workspace.update(&mut app, |workspace, ctx| {
            let authority = crate::environment_authority::TERMINAL_BOOTSTRAP_AUTHORITY;
            let initial_tab_count = workspace.tab_count();
            let initial_pane_count = workspace
                .active_tab_pane_group()
                .as_ref(ctx)
                .visible_pane_ids()
                .len();

            workspace.deliver_restored_conversation(
                authority,
                AIConversation::new(false),
                RestoreConversationLayout::SplitPane,
                ctx,
            );

            assert_eq!(
                workspace.tab_count(),
                initial_tab_count,
                "local SplitPane restore must reuse the target Environment tab"
            );
            assert_eq!(
                workspace
                    .active_tab_pane_group()
                    .as_ref(ctx)
                    .visible_pane_ids()
                    .len(),
                initial_pane_count + 1,
                "local SplitPane restore must add exactly one pane"
            );
            assert_eq!(
                workspace
                    .current_environment_snapshot()
                    .as_ref()
                    .map(|environment| environment.authority_key.as_str()),
                Some(authority)
            );
        });
    });
}

#[test]
fn test_restored_conversation_split_pane_reuses_runtime_environment_container_without_plain_terminal_intent(
) {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        let authority = "ssh:lr113-split-restore".to_owned();
        workspace.update(&mut app, |workspace, ctx| {
            let server = test_ssh_server_for_environment_tests();
            let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                "lr113-split-restore".to_owned(),
                &server,
                Some("/root/project".to_owned()),
                EnvironmentLifecycleState::Connecting,
            );
            assert_eq!(environment.authority_key, authority);
            workspace.add_test_environment_runtime_placeholder_tab(
                environment,
                Some("LR113 runtime split".to_owned()),
                ctx,
            );

            let initial_tab_count = workspace.tab_count();
            let initial_pane_count = workspace
                .active_tab_pane_group()
                .as_ref(ctx)
                .visible_pane_ids()
                .len();
            let conversation = AIConversation::new(false);
            let conversation_id = conversation.id();

            workspace.deliver_restored_conversation(
                &authority,
                conversation,
                RestoreConversationLayout::SplitPane,
                ctx,
            );

            assert_eq!(
                workspace.tab_count(),
                initial_tab_count,
                "runtime SplitPane restore must reuse the target Environment tab"
            );
            assert_eq!(
                workspace
                    .active_tab_pane_group()
                    .as_ref(ctx)
                    .visible_pane_ids()
                    .len(),
                initial_pane_count + 1,
                "runtime SplitPane restore must add exactly one loading carrier"
            );
            assert_eq!(
                workspace
                    .pending_forked_conversation_for_authority(&authority)
                    .map(|entry| entry.conversation.id()),
                Some(conversation_id),
                "runtime SplitPane restore must queue the ForkedConversation on its loading carrier"
            );
            assert!(
                !workspace.has_pending_terminal_for_authority(&authority),
                "navigation activation must not synthesize a PlainTerminal intent"
            );
            assert_eq!(
                workspace
                    .current_environment_snapshot()
                    .as_ref()
                    .map(|environment| environment.authority_key.as_str()),
                Some(authority.as_str())
            );
        });
    });
}

#[test]
fn test_environment_entry_intents_share_container_allocation_delivery_contract() {
    const BACKEND_RS: &str = include_str!("environment_backend.rs");
    const VIEW_RS: &str = include_str!("view.rs");
    const PANE_GROUP_RS: &str = include_str!("../pane_group/mod.rs");

    for variant in [
        "PlainTerminal",
        "StartupCommand",
        "AgentView",
        "ForkedConversation",
        "SessionRestore",
    ] {
        assert!(
            BACKEND_RS.contains(&format!("{variant}(")),
            "EnvironmentEntryIntent must own {variant}"
        );
    }
    assert!(BACKEND_RS.contains("fn deliver_entry("));
    assert!(BACKEND_RS.contains("fn pane_session_binding("));
    assert!(VIEW_RS.contains("fn deliver_workspace_session_restore("));
    assert!(VIEW_RS.contains(".deliver_entry(self, &environment, intent, ctx)"));
    assert!(VIEW_RS.contains("intent.pane_session_binding()"));
    assert!(VIEW_RS.contains("initial_session_binding: Option<PaneSessionBinding>"));
    assert!(PANE_GROUP_RS.contains("session_binding"));
    assert_eq!(
        VIEW_RS.matches("impl EnvironmentBackend for ").count(),
        2,
        "the compiler-enforced local/runtime pair must remain the only backend implementations"
    );
}

#[test]
fn test_environment_navigation_activation_uses_shared_backend_for_delete_and_split_restore() {
    const BACKEND_RS: &str = include_str!("environment_backend.rs");
    const VIEW_RS: &str = include_str!("view.rs");
    const NAVIGATOR_RS: &str = include_str!("view/session_navigator.rs");

    assert!(BACKEND_RS.contains("fn activate_navigation_container("));
    assert_eq!(
        VIEW_RS.matches("fn activate_navigation_container(").count(),
        2,
        "both existing Environment backends must implement navigation activation"
    );

    let snapshot = VIEW_RS
        .split_once("fn environment_snapshot_for_conversation_restore(")
        .expect("conversation restore snapshot resolver must exist")
        .1
        .split_once("fn activate_environment_for_conversation_restore(")
        .expect("snapshot resolver boundary must remain visible")
        .0;
    assert!(snapshot.contains("entry_target_snapshot"));
    assert!(!snapshot.contains("uses_terminal_bootstrap"));
    assert!(!snapshot.contains("runtime_transport_snapshot"));

    let conversation_activation = VIEW_RS
        .split_once("fn activate_environment_for_conversation_restore(")
        .expect("conversation restore activation must exist")
        .1
        .split_once("fn fork_entry_for_restored_conversation(")
        .expect("conversation activation boundary must remain visible")
        .0;
    assert!(conversation_activation.contains("activate_navigation_container"));
    assert!(!conversation_activation.contains("uses_terminal_bootstrap"));
    assert!(!conversation_activation.contains("activate_or_recreate_environment_tab_for_authority"));

    let delete_activation = NAVIGATOR_RS
        .split_once("fn apply_delete_adapter_hooks(")
        .expect("delete post-close hook must exist")
        .1
        .split_once("pub(super) fn delete_workspace_session_for_session(")
        .expect("delete hook boundary must remain visible")
        .0;
    assert!(delete_activation.contains("activate_navigation_container"));
    assert!(!delete_activation.contains("uses_terminal_bootstrap"));
    assert!(!delete_activation.contains("activate_or_recreate_environment_tab_for_authority"));
}

#[test]
fn test_session_navigator_source_and_user_state_use_environment_backend() {
    const BACKEND_RS: &str = include_str!("environment_backend.rs");
    const VIEW_RS: &str = include_str!("view.rs");
    const NAVIGATOR_RS: &str = include_str!("view/session_navigator.rs");
    const TABLE_RS: &str = include_str!("environment_table.rs");

    assert!(!VIEW_RS.contains("indexed_cli_agent_sessions: Vec<WorkspaceSessionSnapshot>"));
    let environment_entry = TABLE_RS
        .split_once("pub(crate) struct EnvironmentEntry {")
        .expect("EnvironmentEntry must exist")
        .1
        .split_once("impl EnvironmentEntry")
        .expect("EnvironmentEntry boundary must remain visible")
        .0;
    assert!(!environment_entry.contains("indexed_cli_agent_sessions"));
    assert!(!environment_entry.contains("cli_agent_session_user_state"));
    assert!(TABLE_RS.contains("indexed_cli_agent_sessions_by_navigation_key"));
    assert!(TABLE_RS.contains("cli_agent_session_user_state_by_navigation_key"));
    assert!(TABLE_RS.contains("fn projection_navigation_key("));
    assert!(TABLE_RS.contains("fn all_indexed_cli_agent_sessions("));
    let display_update = NAVIGATOR_RS
        .split_once("fn session_navigator_sessions_for_display_update(")
        .expect("display refresh path must exist")
        .1
        .split_once("fn reduce_session_navigator_refresh(")
        .expect("display refresh boundary must remain visible")
        .0;
    assert!(!display_update.contains("apply_workspace_session_aliases"));
    assert!(!NAVIGATOR_RS.contains("fn raw_session_navigator_sessions("));
    assert!(NAVIGATOR_RS.contains("fn session_navigator_model("));
    let refresh = NAVIGATOR_RS
        .split_once("fn reduce_session_navigator_refresh(")
        .expect("canonical refresh path must exist")
        .1
        .split_once("fn apply_session_navigator_reduction(")
        .expect("canonical refresh boundary must remain visible")
        .0;
    let alias_projection = refresh
        .find("apply_workspace_session_aliases(&mut merged")
        .expect("Alias must enter canonical projection before reducer Refresh");
    let reducer_refresh = refresh
        .find("SessionNavigatorAction::Refresh")
        .expect("Refresh reducer action must exist");
    assert!(alias_projection < reducer_refresh);
    for capability in [
        "fn session_user_state(",
        "fn mutate_session_user_state(",
        "fn refresh_indexed_sessions(",
    ] {
        assert!(BACKEND_RS.contains(capability));
        assert_eq!(
            VIEW_RS.matches(capability).count(),
            2,
            "both existing Environment backends must implement {capability}"
        );
    }

    let read = NAVIGATOR_RS
        .split_once("fn workspace_session_user_state_for_authority(")
        .expect("Navigator user-state capability must exist")
        .1
        .split_once("pub(super) fn local_cli_agent_session_aliases")
        .expect("read capability boundary must remain visible")
        .0;
    assert!(read.contains("EnvironmentBackendKind::for_authority"));
    assert!(!read.contains("uses_terminal_bootstrap"));

    let indexed = NAVIGATOR_RS
        .split_once("fn indexed_cli_agent_sessions_for_authority(")
        .expect("Navigator indexed-source capability must exist")
        .1
        .split_once("fn all_indexed_environment_cli_agent_sessions")
        .expect("indexed capability boundary must remain visible")
        .0;
    assert!(indexed.contains("self.environments"));
    assert!(!indexed.contains("uses_terminal_bootstrap"));
}

#[test]
fn test_local_and_runtime_session_user_state_share_optimistic_commit_contract() {
    const NAVIGATOR_RS: &str = include_str!("view/session_navigator.rs");
    const VIEW_RS: &str = include_str!("view.rs");

    let mutation = NAVIGATOR_RS
        .split_once("fn mutate_workspace_session_user_state_for_authority(")
        .expect("shared optimistic mutation capability must exist")
        .1
        .split_once("fn mutate_local_workspace_session_user_state(")
        .expect("mutation capability boundary must remain visible")
        .0;
    let optimistic_commit = mutation
        .find("apply_workspace_session_user_state_mutation")
        .expect("shared optimistic state transition must run");
    let backend_dispatch = mutation
        .find(".mutate_session_user_state(")
        .expect("persistence must dispatch through EnvironmentBackend");
    assert!(optimistic_commit < backend_dispatch);
    assert!(mutation.contains("SessionUserStateMutationDelivery::Applied"));
    assert!(mutation.contains("SessionUserStateMutationDelivery::Pending"));
    assert!(mutation.contains("previous_state"));
    assert!(mutation.contains("begin_cli_agent_session_user_state_mutation"));
    assert!(mutation.contains("complete_cli_agent_session_user_state_mutation"));
    assert!(mutation.contains("is_volatile_layout_identity_key"));
    assert!(!mutation.contains("uses_terminal_bootstrap"));
    assert!(!mutation.contains("uses_runtime_environment"));

    let runtime_impl = VIEW_RS
        .split_once("impl EnvironmentBackend for RuntimeEnvironmentBackend")
        .expect("runtime backend implementation must exist")
        .1;
    assert!(runtime_impl.contains("previous_state"));
    assert!(runtime_impl.contains("SessionUserStateMutationDelivery::Pending"));
    assert!(runtime_impl.contains("complete_cli_agent_session_user_state_mutation"));
    assert!(runtime_impl.contains("feedback.success_message()"));
    assert!(!runtime_impl.contains("set_cli_agent_session_user_state"));
}

#[test]
fn terminal_interactions_use_stable_focus_composition_and_filter_owners() {
    const FOCUS_STATE_RS: &str = include_str!("../pane_group/focus_state.rs");
    const PANE_GROUP_RS: &str = include_str!("../pane_group/mod.rs");
    const TERMINAL_MODEL_RS: &str = include_str!("../terminal/model/terminal_model.rs");
    const BLOCKS_RS: &str = include_str!("../terminal/model/blocks.rs");
    const FILTER_RS: &str = include_str!("../terminal/block_filter.rs");
    const TERMINAL_VIEW_RS: &str = include_str!("../terminal/view.rs");

    assert!(FOCUS_STATE_RS.contains("pub fn commit_application_focus"));
    assert!(PANE_GROUP_RS.contains("PaneGroupFocusEvent::FocusChanged { new_focused"));
    assert!(!PANE_GROUP_RS.contains("HandleFocusChange"));
    assert!(TERMINAL_MODEL_RS.contains("marked_text_carrier: Option<MarkedTextCarrier>"));
    assert!(TERMINAL_MODEL_RS.contains("match self.marked_text_carrier.take()"));
    assert!(BLOCKS_RS.contains("clear_marked_text_for_block"));
    assert!(
        !BLOCKS_RS.contains("Tried to clear marked text on blocklist while no block was active")
    );
    assert!(FILTER_RS.contains("pub struct BlockFilterEditSession"));
    assert!(FILTER_RS.contains("generation: self.next_generation"));
    assert!(FILTER_RS.contains("UpdateFilter {"));
    assert!(!TERMINAL_VIEW_RS.contains("active_filter_editor_block_index"));
}

#[cfg(all(feature = "local_fs", not(target_family = "wasm")))]
#[test]
fn remote_native_write_executor_consumes_only_declared_backup_paths() {
    use crate::session_bridge::native_writer::{
        NativeSessionRemoteWriteReceipt, NativeSessionWriteOperation, NativeSessionWritePlan,
    };

    let plan = NativeSessionWritePlan {
        receipt: NativeSessionRemoteWriteReceipt {
            target: SessionBridgeForkTarget::Agent(CLIAgent::Codex),
            session_id: "remote-executor-session".to_owned(),
            title: "Remote executor".to_owned(),
            project_path: "/root/project".to_owned(),
            session_file: "/root/.codex/sessions/rollout.jsonl".to_owned(),
        },
        operations: vec![NativeSessionWriteOperation::Write {
            path: "/root/.codex/sessions/rollout.jsonl".to_owned(),
            contents: b"session\n".to_vec(),
        }],
        backup_paths: vec!["/root/.codex/history.jsonl".to_owned()],
    };

    let backup_root = environment_native_session_backup_root("/root", &plan);
    let backup = environment_native_session_backup_command(&backup_root, &plan);
    for path in &plan.backup_paths {
        assert!(backup.contains(path), "backup command omitted {path}");
    }
    assert!(!backup.contains("state_5.sqlite"));
}

#[cfg(all(feature = "local_fs", not(target_family = "wasm")))]
#[test]
fn remote_native_write_executor_rolls_back_all_plan_paths_on_failure() {
    use crate::session_bridge::native_writer::{
        NativeSessionRemoteWriteReceipt, NativeSessionWriteOperation, NativeSessionWritePlan,
    };

    let plan = NativeSessionWritePlan {
        receipt: NativeSessionRemoteWriteReceipt {
            target: SessionBridgeForkTarget::Agent(CLIAgent::Claude),
            session_id: "remote-rollback-session".to_owned(),
            title: "Remote rollback".to_owned(),
            project_path: "/root/project".to_owned(),
            session_file: "/root/.claude/projects/project/session.jsonl".to_owned(),
        },
        operations: vec![
            NativeSessionWriteOperation::Write {
                path: "/root/.claude/projects/project/session.jsonl".to_owned(),
                contents: b"session\n".to_vec(),
            },
            NativeSessionWriteOperation::Append {
                path: "/root/.claude/history.jsonl".to_owned(),
                contents: b"history\n".to_vec(),
            },
        ],
        backup_paths: vec![
            "/root/.claude/history.jsonl".to_owned(),
            "/root/.claude/projects/project".to_owned(),
        ],
    };

    let backup_root = environment_native_session_backup_root("/root", &plan);
    let rollback = environment_native_session_rollback_command(&backup_root, &plan);
    assert!(rollback.contains("rm -rf -- /root/.claude/history.jsonl"));
    assert!(rollback.contains("rm -rf -- /root/.claude/projects/project"));
    assert!(rollback.contains(&format!("{backup_root}/0")));
    assert!(rollback.contains(&format!("{backup_root}/1")));
    assert!(rollback.contains(&format!("rm -rf -- {backup_root}")));
}

#[test]
fn agent_split_layout_contract_is_shared_by_real_and_loading_panes() {
    const VIEW_RS: &str = include_str!("view.rs");
    let delivery = VIEW_RS
        .split_once("fn deliver_agent_pane_split(")
        .expect("agent split delivery must exist")
        .1
        .split_once("/// Split-pane counterpart of `deliver_startup_command`")
        .expect("agent split delivery boundary must remain visible")
        .0;

    assert!(delivery.contains("let layout = AgentSplitLayoutIntent::default();"));
    assert!(delivery.contains("add_loading_conversation_pane_with_agent_split_layout(layout"));
    assert!(delivery.contains("deliver_agent_pane_split_in_real_pane(layout"));
    assert!(delivery.contains("Some(layout)"));
    assert!(!delivery.contains("add_loading_conversation_pane(direction, None"));
}

#[test]
fn test_split_pane_add_terminal_pane_call_sites_are_audited() {
    const VIEW_RS: &str = include_str!("view.rs");
    let call_count = VIEW_RS
        .matches("add_terminal_pane_in_current_environment(")
        .count();
    const APPROVED_CALL_SITES: usize = 7;
    assert_eq!(
        call_count, APPROVED_CALL_SITES,
        "unexpected add_terminal_pane_in_current_environment call site — route split-pane \
         capabilities through deliver_agent_pane_split / deliver_fork_split_pane / \
         deliver_startup_command_split_pane, or document exceptions in \
         docs/design/local-remote-capability-matrix.csv"
    );
}

#[test]
fn test_runtime_materialization_call_sites_establish_owner_before_transport() {
    const VIEW_RS: &str = include_str!("view.rs");

    fn section<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
        source
            .split_once(start)
            .unwrap_or_else(|| panic!("missing audited start marker: {start}"))
            .1
            .split_once(end)
            .unwrap_or_else(|| panic!("missing audited end marker: {end}"))
            .0
    }

    assert_eq!(
        VIEW_RS
            .matches(".ensure_current_environment_runtime_transport_if_needed(")
            .count(),
        3,
        "exactly three classified callers are allowed; a new direct ensure must be routed through an audited owner-before-transport boundary"
    );
    assert_eq!(
        VIEW_RS
            .matches(".ensure_environment_runtime_transport(")
            .count(),
        4,
        "exactly four low-level transport starts are classified: restored Resume, provider open, shared implicit ensure, and explicit reconnect"
    );
    assert_eq!(
        VIEW_RS
            .matches(".prepare_active_environment_after_activation(")
            .count(),
        3,
        "cold restore, user-visible tab activation, and environment switching must be the complete shared preparation caller set"
    );
    assert_eq!(
        VIEW_RS
            .matches(".stage_environment_runtime_split_pane_intent(")
            .count(),
        4,
        "plain, agent, startup-command, and fork split flows must be the complete shared staging caller set"
    );
    assert_eq!(
        VIEW_RS
            .matches("spawn_plan.open_with(&mut ApplyEnvironmentRuntimeSpawn")
            .count(),
        1,
        "new-tab runtime spawn plans must have one apply boundary"
    );
    assert_eq!(
        VIEW_RS
            .matches("spawn_plan.open_with(&mut AddPaneEnvironmentRuntimeSpawn")
            .count(),
        1,
        "split-pane runtime spawn plans must have one apply boundary"
    );

    let preparation = section(
        VIEW_RS,
        "fn prepare_active_environment_after_activation(",
        "fn reconnect_failed_environment_after_user_visible_activation(",
    );
    let queue = preparation
        .find("queue_active_environment_runtime_placeholder_terminals_if_needed")
        .expect("shared preparation must reconcile pane-owned placeholder intents");
    let ensure = preparation
        .find("ensure_current_environment_runtime_transport_if_needed")
        .expect("shared preparation must own transport ensure");
    let delivery = preparation
        .find("open_pending_environment_runtime_terminal_for_current_environment")
        .expect("shared preparation must own pending delivery");
    assert!(queue < ensure && ensure < delivery);
    assert!(preparation.contains("ActiveEnvironmentPreparationReason::UserVisibleActivation"));
    assert!(preparation.contains("ActiveEnvironmentPreparationReason::ColdRestore"));

    let split_stage = section(
        VIEW_RS,
        "fn stage_environment_runtime_split_pane_intent(",
        "fn queue_environment_runtime_intent(",
    );
    let queue = split_stage
        .find("queue_pending_environment_runtime_split_pane_entry")
        .expect("split staging must commit its pane-owned intent");
    let ensure = split_stage
        .find("ensure_current_environment_runtime_transport_if_needed")
        .expect("split staging must start transport after owner commit");
    assert!(queue < ensure);

    for (start, end) in [
        (
            "fn deliver_agent_pane_split(",
            "fn deliver_agent_pane_split_in_real_pane(",
        ),
        (
            "fn deliver_startup_command_split_pane(",
            "fn deliver_startup_command_split_pane_in_real_pane(",
        ),
        (
            "fn deliver_fork_split_pane(",
            "fn deliver_fork_split_pane_in_real_pane(",
        ),
    ] {
        let caller = section(VIEW_RS, start, end);
        assert!(caller.contains("stage_environment_runtime_split_pane_intent"));
        assert!(!caller.contains("ensure_current_environment_runtime_transport_if_needed"));
    }

    let restore = section(
        VIEW_RS,
        "NewWorkspaceSource::Restored {",
        "NewWorkspaceSource::FromTemplate",
    );
    assert!(restore.contains("ActiveEnvironmentPreparationReason::ColdRestore"));
    assert!(!restore.contains("ensure_current_environment_runtime_transport_if_needed"));
    assert!(!restore.contains("add_restored_environment_runtime_tab"));

    for (start, end) in [
        (
            "fn activate_tab_for_user_visible_navigation(",
            "fn prepare_active_environment_after_activation(",
        ),
        (
            "fn switch_to_environment_authority(",
            "fn disconnect_environment_runtime_state(",
        ),
    ] {
        let caller = section(VIEW_RS, start, end);
        assert!(caller.contains("prepare_active_environment_after_activation"));
        assert!(caller.contains("ActiveEnvironmentPreparationReason::UserVisibleActivation"));
        assert!(!caller.contains("ensure_current_environment_runtime_transport_if_needed"));
    }

    let add_pane_spawn = section(
        VIEW_RS,
        "impl EnvironmentRuntimeSpawnPlanHandler for AddPaneEnvironmentRuntimeSpawn<'_, '_> {",
        "impl<'a, 'b> EnvironmentSessionTabPlanHandler",
    );
    let loading_pane = add_pane_spawn
        .find("add_loading_conversation_pane")
        .expect("plain terminal AddPane bootstrap must allocate a visible loading carrier");
    let stage = add_pane_spawn
        .find("stage_environment_runtime_split_pane_intent")
        .expect("plain terminal AddPane bootstrap must use shared split staging");
    assert!(loading_pane < stage);
    assert!(!add_pane_spawn.contains("ensure_current_environment_runtime_transport_if_needed"));

    for (start, end, owner_marker) in [
        (
            "fn open_restored_environment_runtime_session(",
            "fn restored_environment_runtime_startup_command(",
            "restore_environment_runtime_session(",
        ),
        (
            "fn open_or_switch_environment_runtime_in_current_window(",
            "fn open_environment_provider_candidate(",
            "queue_active_environment_runtime_placeholder_terminals_if_needed",
        ),
    ] {
        let caller = section(VIEW_RS, start, end);
        let owner = caller
            .find(owner_marker)
            .unwrap_or_else(|| panic!("missing owner marker {owner_marker} in {start}"));
        let transport = caller
            .find("ensure_environment_runtime_transport")
            .unwrap_or_else(|| panic!("missing low-level transport start in {start}"));
        assert!(owner < transport);
    }

    for (start, end) in [
        (
            "fn ensure_current_environment_runtime_transport_if_needed(",
            "fn reconnect_environment_runtime_authority(",
        ),
        (
            "fn reconnect_environment_runtime_authority(",
            "pub fn reconnect_current_environment(",
        ),
    ] {
        let caller = section(VIEW_RS, start, end);
        assert_eq!(
            caller
                .matches("ensure_environment_runtime_transport")
                .count(),
            1,
            "classified transport boundary {start} must contain exactly one low-level start"
        );
    }

    assert!(!VIEW_RS.contains("environment_runtime_placeholder_leaf_index("));
    assert!(!VIEW_RS.contains("active_environment_runtime_placeholder_pane_id("));
}

#[test]
fn test_workspace_split_bindings_route_through_workspace_action() {
    const WORKSPACE_VIEW: &str = include_str!("view.rs");
    const PANE_GROUP_MOD: &str = include_str!("../pane_group/mod.rs");

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        app.update(crate::pane_group::init);

        app.update(|ctx| {
            for (binding_name, expected_direction) in [
                ("pane_group:add_left", "Left"),
                ("pane_group:add_right", "Right"),
                ("pane_group:add_up", "Up"),
                ("pane_group:add_down", "Down"),
            ] {
                let bindings = ctx
                    .editable_bindings()
                    .filter(|binding| binding.name == binding_name)
                    .collect::<Vec<_>>();
                assert_eq!(
                    bindings.len(),
                    1,
                    "{binding_name} must be registered exactly once so a later PaneGroup binding cannot override Workspace authority"
                );
                let action = bindings[0]
                    .action
                    .as_any()
                    .downcast_ref::<WorkspaceAction>()
                    .unwrap_or_else(|| {
                        panic!("{binding_name} must dispatch WorkspaceAction, not a focused PaneGroup action")
                    });
                let WorkspaceAction::AddTerminalPane(direction) = action else {
                    panic!("{binding_name} must dispatch AddTerminalPane")
                };
                assert_eq!(format!("{direction:?}"), expected_direction);
                assert!(
                    bindings[0]
                        .action
                        .as_any()
                        .downcast_ref::<PaneGroupAction>()
                        .is_none(),
                    "{binding_name} must not retain a PaneGroupAction payload"
                );
            }
        });
    });

    for deleted_parallel_path in [
        "environment_new_terminal_options",
        "environment_new_terminal_uses_runtime",
        "set_environment_new_terminal_options",
    ] {
        assert!(
            !PANE_GROUP_MOD.contains(deleted_parallel_path),
            "PaneGroup Environment authority cache must remain deleted: {deleted_parallel_path}"
        );
    }
    assert!(
        !WORKSPACE_VIEW.contains("sync_active_pane_group_environment_terminal_options"),
        "Workspace must not copy Environment authority into the focused PaneGroup"
    );
}

#[test]
fn test_workspace_split_action_uses_active_environment_when_focus_is_stale() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let workspace = mock_workspace(&mut app);
        let (stale_local_pane_group, local_pane_ids_before, local_terminal_pane_ids_before) =
            workspace.update(&mut app, |workspace, ctx| {
                let stale_local_pane_group = workspace.active_tab_pane_group().clone();
                stale_local_pane_group.update(ctx, |pane_group, ctx| {
                    pane_group.add_loading_conversation_pane(Direction::Right, None, ctx);
                    pane_group.add_loading_conversation_pane(Direction::Down, None, ctx);
                });
                let local_pane_ids_before =
                    stale_local_pane_group.as_ref(ctx).visible_pane_ids();
                let local_terminal_pane_ids_before = stale_local_pane_group
                    .as_ref(ctx)
                    .terminal_pane_ids()
                    .collect::<HashSet<_>>();
                assert_eq!(
                    local_pane_ids_before.len(),
                    3,
                    "the stale local PaneGroup must contain two unrelated panes in addition to its original pane"
                );
                (
                    stale_local_pane_group,
                    local_pane_ids_before,
                    local_terminal_pane_ids_before,
                )
            });

        workspace.update(&mut app, |workspace, ctx| workspace.focus_active_tab(ctx));
        app.update(|ctx| {
            assert!(
                stale_local_pane_group.is_self_or_child_focused(ctx),
                "test setup must begin with the old local PaneGroup focused"
            );
        });

        let (authority, remote_pane_group, remote_pane_ids_before) =
            workspace.update(&mut app, |workspace, ctx| {
                let server = test_ssh_server_for_environment_tests();
                let environment = crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                    "remote-fixture-primary".to_string(),
                    &server,
                    Some("/root/project".to_string()),
                    EnvironmentLifecycleState::Connecting,
                );
                let authority = environment.authority_key.clone();
                let session_id = CoreSessionId::from(9016);
                workspace.environments_mut().mark_connecting(
                    environment.clone(),
                    session_id,
                    PathBuf::from("/tmp/ashide-test-ssh-control-workspace-split.sock"),
                );
                workspace.add_test_environment_runtime_placeholder_tab(
                    environment,
                    Some("remote-fixture-primary".to_string()),
                    ctx,
                );
                let remote_pane_group = workspace.active_tab_pane_group().clone();
                let remote_pane_ids_before = remote_pane_group.as_ref(ctx).visible_pane_ids();
                assert_eq!(remote_pane_ids_before.len(), 1);
                (authority, remote_pane_group, remote_pane_ids_before)
            });

        workspace.update(&mut app, |workspace, ctx| {
            assert!(
                stale_local_pane_group.is_self_or_child_focused(ctx),
                "provider activation must reproduce the interval where application focus remains on the old local PaneGroup"
            );
            assert_eq!(
                workspace.active_tab_pane_group().id(),
                remote_pane_group.id(),
                "stale focus must not change the Workspace active tab owner"
            );
            assert_eq!(
                workspace
                    .current_environment_snapshot()
                    .as_ref()
                    .map(|environment| environment.authority_key.as_str()),
                Some(authority.as_str()),
                "test setup must keep the remote Environment active while focus is stale on the old local PaneGroup"
            );

            workspace.handle_action(
                &WorkspaceAction::AddTerminalPane(Direction::Right),
                ctx,
            );

            assert_eq!(
                stale_local_pane_group.as_ref(ctx).visible_pane_ids(),
                local_pane_ids_before,
                "Workspace-owned split must not mutate the stale focused local PaneGroup"
            );
            assert_eq!(
                stale_local_pane_group
                    .as_ref(ctx)
                    .terminal_pane_ids()
                    .collect::<HashSet<_>>(),
                local_terminal_pane_ids_before,
                "Workspace-owned split must not spawn a local terminal process in the stale focused PaneGroup"
            );
            let remote_pane_ids_after = remote_pane_group.as_ref(ctx).visible_pane_ids();
            assert_eq!(
                remote_pane_ids_after.len(),
                remote_pane_ids_before.len() + 1,
                "the active remote PaneGroup must receive the loading carrier"
            );
            let pending_pane_id = workspace
                .pending_materialization_pane_id_for_authority(&authority)
                .expect("remote split must commit an exact pane-owned PendingMaterialization");
            assert!(remote_pane_ids_after.contains(&pending_pane_id));
            assert!(!local_pane_ids_before.contains(&pending_pane_id));
            let entry = workspace
                .environments
                .entry_for_authority(&authority)
                .expect("remote split must retain the active Environment owner");
            assert_eq!(entry.pending_materializations.len(), 1);
            assert!(matches!(
                entry.pending_materializations[0].intent,
                EnvironmentEntryIntent::PlainTerminal(_)
            ));
        });
    });
}

#[test]
fn test_environment_backend_kind_dispatch_call_sites_are_audited() {
    const VIEW_RS: &str = include_str!("view.rs");
    let mut dispatch_count_by_function = std::collections::BTreeMap::new();
    for (offset, _) in VIEW_RS.match_indices("EnvironmentBackendKind::for_environment") {
        let function_name = VIEW_RS[..offset]
            .lines()
            .rev()
            .find_map(|line| {
                let line = line.trim_start();
                let function = line
                    .strip_prefix("fn ")
                    .or_else(|| line.split_once(" fn ").map(|(_, function)| function))?;
                function
                    .split_once('(')
                    .map(|(name, _)| name.trim().to_owned())
            })
            .expect("every Environment backend dispatch must belong to a named function");
        *dispatch_count_by_function.entry(function_name).or_insert(0) += 1;
    }

    let approved_dispatch_count_by_function = std::collections::BTreeMap::from([
        (
            "activate_environment_for_conversation_restore".to_owned(),
            1,
        ), // LR-113
        ("add_default_plain_terminal_tab_route_aware".to_owned(), 1), // LR-073
        ("add_tab_with_specific_agent".to_owned(), 1),                // LR-073
        ("add_terminal_tab_in_ai_mode".to_owned(), 1),                // LR-073
        ("add_terminal_tab_with_new_agent_view".to_owned(), 1),       // LR-073
        ("cd_to_directory".to_owned(), 1),                            // LR-073
        ("deliver_restored_conversation".to_owned(), 1),              // LR-109/LR-113
        ("deliver_workspace_session_restore".to_owned(), 1),          // LR-109
        ("fork_ai_conversation".to_owned(), 1),                       // LR-073
        ("handle_action".to_owned(), 2),                              // LR-073/LR-109
        ("handle_codex_modal_event".to_owned(), 1),                   // LR-073
        (
            "open_agent_directory_tab_in_current_environment".to_owned(),
            1,
        ), // LR-073
        ("open_directory_tab_in_current_environment".to_owned(), 1),  // LR-073
        ("open_linear_issue_work".to_owned(), 1),                     // LR-073
        (
            "try_open_environment_runtime_tab_config_template".to_owned(),
            1,
        ), // LR-109
    ]);
    assert_eq!(
        dispatch_count_by_function, approved_dispatch_count_by_function,
        "unexpected EnvironmentBackendKind::for_environment dispatch home — reuse an existing \
         capability or add its SPEC/matrix/tracker/test contract before introducing a new \
         local/remote entry point"
    );
}

#[test]
fn session_bridge_cli_agent_operations_require_resolved_store_root_context() {
    const VIEW_RS: &str = include_str!("view.rs");
    const NAVIGATOR_RS: &str = include_str!("view/session_navigator.rs");

    assert!(VIEW_RS.contains("enum ResolvedSessionBridgeEnvironment"));
    assert!(VIEW_RS.contains("async fn resolve_session_bridge_environment_pair("));
    assert!(
        VIEW_RS.contains("resolve_session_bridge_environment_pair(source_env, target_env).await?")
    );
    assert!(VIEW_RS.contains("read_current_app_cli_agent_session_bridge_with_roots_blocking("));
    assert!(VIEW_RS.contains("write_session_bridge_derivation_with_roots("));
    assert!(VIEW_RS.contains("delete_current_app_cli_agent_session_with_roots("));
    assert!(NAVIGATOR_RS.contains(".resolve_cli_agent_store_roots()"));

    let resolver = VIEW_RS
        .split("async fn resolve_cli_agent_store_roots(")
        .nth(1)
        .and_then(|tail| {
            tail.split("async fn resolve_session_bridge_environment_pair(")
                .next()
        })
        .expect("resolved environment 必须唯一拥有 roots 解析实现");
    assert_eq!(
        resolver
            .matches("resolve_current_process_cli_agent_store_roots()")
            .count(),
        1,
        "本地 operation context 只能解析一次当前进程 roots"
    );
    assert_eq!(
        resolver
            .matches("resolve_environment_cli_agent_store_roots(")
            .count(),
        1,
        "远程 operation context 只能 probe 一次目标环境 roots"
    );

    let pair_resolver = VIEW_RS
        .split("async fn resolve_session_bridge_environment_pair(")
        .nth(1)
        .and_then(|tail| tail.split("enum CliAgentSourceLocator").next())
        .expect("source/target pair resolver 必须位于 source locator 之前");
    assert!(pair_resolver.contains("source.has_same_execution_context(&target)"));
    assert!(pair_resolver.contains("Ok((resolved.clone(), resolved))"));
}

#[test]
fn local_cli_agent_session_refresh_runs_blocking_scan_off_ui_thread() {
    const VIEW_RS: &str = include_str!("view.rs");
    const NAVIGATOR_RS: &str = include_str!("view/session_navigator.rs");

    let local_backend = VIEW_RS
        .split_once("impl EnvironmentBackend for TerminalBootstrapEnvironmentBackend")
        .expect("local Environment backend must exist")
        .1
        .split_once("impl EnvironmentBackend for RuntimeEnvironmentBackend")
        .expect("runtime Environment backend must delimit the local implementation")
        .0;
    let refresh = local_backend
        .split_once("fn refresh_indexed_sessions(")
        .expect("local backend must own Session Navigator refresh")
        .1;
    let blocking_scan = refresh
        .find("tokio::task::spawn_blocking(move ||")
        .expect("local provider-store scan must run on a blocking worker");
    let scan = refresh
        .find("Workspace::try_scan_terminal_cli_agent_session_discovery(")
        .expect("local refresh must reuse the shared discovery projection");

    assert!(
        blocking_scan < scan,
        "local provider-store discovery must not start before the worker boundary"
    );
    assert!(refresh.contains("ctx.spawn("));
    assert!(refresh.contains("commit_indexed_environment_cli_agent_session_discovery"));
    assert!(refresh.contains("if refresh_generation.is_some() {\n                    workspace.prune_restored_workspace_sessions_with_missing_cli_sources();\n                }"));
    assert!(refresh.contains("finish_workspace_sessions_refresh_if_current"));
    assert!(refresh.contains("fail_workspace_sessions_refresh_if_current"));
    assert!(refresh.contains("Ok(true)"));
    assert!(
        NAVIGATOR_RS.contains(
            "enabled_agents: Vec<crate::terminal::CLIAgent>,\n        previously_observed_agents"
        ),
        "the blocking scan helper must consume captured data rather than an AppContext"
    );
}
