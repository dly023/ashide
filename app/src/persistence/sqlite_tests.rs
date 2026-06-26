use std::{path::PathBuf, sync::Arc};

use crate::{
    app_state::{
        AppState, CliAgentSessionOrigin, CodePaneSnapShot, CodePaneTabSnapshot, EnvironmentKind,
        EnvironmentLifecycleState, EnvironmentSnapshot, LeafContents, LeafSnapshot,
        PaneNodeSnapshot, TabSnapshot, TerminalPaneSnapshot, WindowSnapshot, WorkspaceSessionKind,
        WorkspaceSessionSnapshot,
    },
    code::editor_management::CodeSource,
    notebooks::{NotebookObject, NotebookObjectModel},
    object_store::ids::ClientId,
    object_store::{Owner, StoredObjectPermissions},
    persistence::{model::ObjectPermissions, BlockCompleted, ModelEvent},
    server_time::ServerTimestamp,
    tab::SelectedTabColor,
    terminal::model::block::SerializedBlock,
    terminal::ShellLaunchData,
};

use super::{
    decode_path, deduplicate_events, encode_path, read_sqlite_data, save_app_state, setup_database,
};

#[test]
fn test_deduplicate_snapshots() {
    let local_notebook = NotebookObject::new_local(
        NotebookObjectModel {
            title: "Hello".to_string(),
            data: "World".to_string(),
            ai_document_id: None,
            conversation_id: None,
        },
        Owner::mock_current_user(),
        None,
        ClientId::new(),
    );
    let completed_block_1 = BlockCompleted {
        pane_id: vec![1, 2, 3],
        block: Arc::new(SerializedBlock::default()),
        is_local: true,
    };
    let completed_block_2 = BlockCompleted {
        pane_id: vec![4, 5, 6],
        block: Arc::new(SerializedBlock::default()),
        is_local: true,
    };
    let snapshot_1 = AppState {
        active_window_index: Some(1),
        block_lists: Default::default(),
        windows: Default::default(),
    };
    let snapshot_2 = AppState {
        active_window_index: Some(2),
        block_lists: Default::default(),
        windows: Default::default(),
    };
    let snapshot_3 = AppState {
        active_window_index: Some(3),
        block_lists: Default::default(),
        windows: Default::default(),
    };

    let original_events = vec![
        ModelEvent::UpsertNotebook {
            notebook: local_notebook.clone(),
        },
        ModelEvent::Snapshot(snapshot_1.clone()),
        ModelEvent::SaveBlock(completed_block_1.clone()),
        ModelEvent::Snapshot(snapshot_2.clone()),
        ModelEvent::SaveBlock(completed_block_2.clone()),
        ModelEvent::Snapshot(snapshot_3.clone()),
        ModelEvent::UpsertNotebook {
            notebook: local_notebook.clone(),
        },
    ];

    let filtered_events = deduplicate_events(original_events);
    assert_eq!(filtered_events.len(), 5);

    assert!(matches!(
        &filtered_events[0],
        &ModelEvent::UpsertNotebook { .. }
    ));
    // The first snapshot should have been filtered out.
    assert!(matches!(&filtered_events[1], &ModelEvent::SaveBlock(_)));
    // The second snapshot should have been filtered out.
    assert!(matches!(&filtered_events[2], &ModelEvent::SaveBlock(_)));
    // The third snapshot should be preserved.
    match &filtered_events[3] {
        ModelEvent::Snapshot(snapshot) => assert_eq!(snapshot, &snapshot_3),
        other => panic!("Expected ModelEvent::Snapshot, got {other:?}"),
    }
    assert!(matches!(
        &filtered_events[4],
        &ModelEvent::UpsertNotebook { .. }
    ));
}

#[test]
fn test_deduplicate_no_snapshots() {
    let original_events = vec![ModelEvent::SaveBlock(BlockCompleted {
        pane_id: vec![1, 2, 3],
        block: Default::default(),
        is_local: true,
    })];
    let filtered_events = deduplicate_events(original_events);
    assert_eq!(filtered_events.len(), 1);
    assert!(matches!(&filtered_events[0], &ModelEvent::SaveBlock(_)));
}

fn test_terminal_window_snapshot(vertical_tabs_panel_open: bool) -> WindowSnapshot {
    WindowSnapshot {
        environment: None,
        workspace_sessions: vec![],
        tabs: vec![TabSnapshot {
            environment: None,
            custom_title: None,
            root: PaneNodeSnapshot::Leaf(LeafSnapshot {
                is_focused: true,
                custom_vertical_tabs_title: None,
                contents: LeafContents::Terminal(TerminalPaneSnapshot {
                    uuid: vec![u8::from(vertical_tabs_panel_open) + 1],
                    cwd: Some("/tmp".to_string()),
                    shell_launch_data: Some(ShellLaunchData::Executable {
                        executable_path: PathBuf::from("/bin/zsh"),
                        shell_type: crate::terminal::shell::ShellType::Zsh,
                    }),
                    is_active: true,
                    is_read_only: false,
                    input_config: None,
                    llm_model_override: None,
                    active_profile_id: None,
                    conversation_ids_to_restore: vec![],
                    active_conversation_id: None,
                    cli_agent: None,
                    cli_command: None,
                    cli_agent_origin: None,
                    cli_agent_session_id: None,
                }),
            }),
            default_directory_color: None,
            selected_color: SelectedTabColor::default(),
            left_panel: None,
            right_panel: None,
        }],
        active_tab_index: 0,
        bounds: None,
        fullscreen_state: Default::default(),
        quake_mode: false,
        universal_search_width: None,
        warp_ai_width: None,
        voltron_width: None,
        local_drive_index_width: None,
        left_panel_open: false,
        vertical_tabs_panel_open,
        left_panel_width: None,
        right_panel_width: None,
        agent_management_filters: None,
    }
}

#[test]
fn test_sqlite_round_trips_environment_snapshot() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let database_path = tempdir.path().join("ashide.sqlite");
    let mut conn = setup_database(&database_path).expect("database should initialize");

    let mut window = test_terminal_window_snapshot(false);
    window.environment = Some(EnvironmentSnapshot::local(Some("/tmp".to_string())));
    window.workspace_sessions =
        WorkspaceSessionSnapshot::from_tabs(&window.tabs, window.environment.as_ref());

    let app_state = AppState {
        windows: vec![window],
        active_window_index: Some(0),
        block_lists: Default::default(),
    };

    save_app_state(&mut conn, &app_state).expect("app state should save");

    let restored = read_sqlite_data(&mut conn, None)
        .expect("app state should load")
        .app_state;

    let environment = restored.windows[0]
        .environment
        .as_ref()
        .expect("environment should round-trip");
    assert_eq!(environment.label, "Local");
    assert_eq!(environment.kind, EnvironmentKind::Local);
    assert_eq!(environment.authority_key, "local:/tmp");
    assert_eq!(environment.connection_ref, None);
    assert_eq!(environment.active_workspace_root.as_deref(), Some("/tmp"));
    assert_eq!(
        environment.lifecycle_state,
        EnvironmentLifecycleState::Connected
    );
    let sessions = &restored.windows[0].workspace_sessions;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].kind, WorkspaceSessionKind::Terminal);
    assert_eq!(
        sessions[0].environment_authority_key.as_deref(),
        Some("local:/tmp")
    );
    assert_eq!(sessions[0].cwd.as_deref(), Some("/tmp"));
}

#[test]
fn test_sqlite_round_trips_tab_scoped_environments() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let database_path = tempdir.path().join("ashide.sqlite");
    let mut conn = setup_database(&database_path).expect("database should initialize");

    let mut ssh_server =
        warp_ssh_manager::SshServerInfo::new_default("ssh-config:dev-150".to_string());
    ssh_server.host = "dev-150".to_string();
    ssh_server.username = "root".to_string();

    let mut window = test_terminal_window_snapshot(false);
    window.environment = Some(EnvironmentSnapshot::local(Some("/repo".to_string())));
    window.tabs[0].environment = Some(EnvironmentSnapshot::local(Some("/repo".to_string())));

    let mut remote_tab = window.tabs[0].clone();
    remote_tab.environment = Some(
        crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
            "ssh-config:dev-150".to_string(),
            &ssh_server,
            Some("/root/repo".to_string()),
            EnvironmentLifecycleState::Dormant,
        ),
    );
    if let PaneNodeSnapshot::Leaf(LeafSnapshot {
        contents: LeafContents::Terminal(terminal),
        ..
    }) = &mut remote_tab.root
    {
        terminal.uuid = vec![42];
        terminal.cwd = Some("/root/repo".to_string());
    }
    window.tabs.push(remote_tab);
    window.workspace_sessions =
        WorkspaceSessionSnapshot::from_tabs(&window.tabs, window.environment.as_ref());

    let app_state = AppState {
        windows: vec![window],
        active_window_index: Some(0),
        block_lists: Default::default(),
    };

    save_app_state(&mut conn, &app_state).expect("app state should save");

    let restored = read_sqlite_data(&mut conn, None)
        .expect("app state should load")
        .app_state;

    let tabs = &restored.windows[0].tabs;
    assert_eq!(tabs.len(), 2);
    assert_eq!(
        tabs[0]
            .environment
            .as_ref()
            .map(|environment| environment.authority_key.as_str()),
        Some("local:/repo")
    );
    assert_eq!(
        tabs[1]
            .environment
            .as_ref()
            .map(|environment| environment.authority_key.as_str()),
        Some("ssh:ssh-config:dev-150")
    );
    assert_eq!(
        tabs[1]
            .environment
            .as_ref()
            .and_then(EnvironmentSnapshot::runtime_connection_ref),
        Some("ssh-config:dev-150")
    );

    let sessions = &restored.windows[0].workspace_sessions;
    assert_eq!(sessions.len(), 2);
    assert_eq!(
        sessions[0].environment_authority_key.as_deref(),
        Some("local:/repo")
    );
    assert_eq!(
        sessions[1].environment_authority_key.as_deref(),
        Some("ssh:ssh-config:dev-150")
    );
}

#[test]
fn test_sqlite_round_trips_cli_agent_workspace_session_metadata() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let database_path = tempdir.path().join("ashide.sqlite");
    let mut conn = setup_database(&database_path).expect("database should initialize");

    let mut window = test_terminal_window_snapshot(false);
    window.environment = Some(EnvironmentSnapshot::local(Some("/repo".to_string())));
    window.workspace_sessions = vec![WorkspaceSessionSnapshot {
        id: "tab:0:leaf:0".to_string(),
        kind: WorkspaceSessionKind::AgentTerminal,
        label: Some("Codex refactor".to_string()),
        environment_authority_key: Some("local:/repo".to_string()),
        cwd: Some("/repo".to_string()),
        startup_directory: None,
        cli_agent: Some("Codex".to_string()),
        cli_command: Some("codex".to_string()),
        cli_agent_origin: Some(CliAgentSessionOrigin::CommandDetected),
        cli_agent_session_id: None,
        conversation_ids: vec!["conv-1".to_string(), "conv-2".to_string()],
        active_conversation_id: Some("conv-2".to_string()),
        is_active: true,
        is_pinned: false,
        updated_at_unix_ms: None,
    }];

    let app_state = AppState {
        windows: vec![window],
        active_window_index: Some(0),
        block_lists: Default::default(),
    };

    save_app_state(&mut conn, &app_state).expect("app state should save");

    let restored = read_sqlite_data(&mut conn, None)
        .expect("app state should load")
        .app_state;

    let sessions = &restored.windows[0].workspace_sessions;
    assert_eq!(sessions.len(), 1);
    let session = &sessions[0];
    assert_eq!(session.kind, WorkspaceSessionKind::AgentTerminal);
    assert_eq!(session.label.as_deref(), Some("Codex refactor"));
    assert_eq!(
        session.environment_authority_key.as_deref(),
        Some("local:/repo")
    );
    assert_eq!(session.cwd.as_deref(), Some("/repo"));
    assert_eq!(session.cli_agent.as_deref(), Some("Codex"));
    assert_eq!(session.cli_command.as_deref(), Some("codex"));
    assert_eq!(
        session.cli_agent_origin,
        Some(CliAgentSessionOrigin::CommandDetected)
    );
    assert_eq!(session.conversation_ids, vec!["conv-1", "conv-2"]);
    assert_eq!(session.active_conversation_id.as_deref(), Some("conv-2"));
    assert!(session.is_active);
}

#[test]
fn test_sqlite_round_trips_ssh_environment_snapshot() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let database_path = tempdir.path().join("ashide.sqlite");
    let mut conn = setup_database(&database_path).expect("database should initialize");

    let mut server = warp_ssh_manager::SshServerInfo::new_default("node-1".to_string());
    server.host = "example.internal".to_string();
    server.username = "root".to_string();
    server.port = 2222;

    let mut window = test_terminal_window_snapshot(false);
    window.environment = Some(
        crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
            "node-1".to_string(),
            &server,
            Some("/root/project".to_string()),
            EnvironmentLifecycleState::Dormant,
        ),
    );

    let app_state = AppState {
        windows: vec![window],
        active_window_index: Some(0),
        block_lists: Default::default(),
    };

    save_app_state(&mut conn, &app_state).expect("app state should save");

    let restored = read_sqlite_data(&mut conn, None)
        .expect("app state should load")
        .app_state;

    let environment = restored.windows[0]
        .environment
        .as_ref()
        .expect("environment should round-trip");
    assert_eq!(environment.label, "root@example.internal:2222");
    assert_eq!(environment.kind, EnvironmentKind::Ssh);
    assert_eq!(environment.authority_key, "ssh:node-1");
    assert_eq!(environment.connection_ref.as_deref(), Some("node-1"));
    assert_eq!(
        environment.active_workspace_root.as_deref(),
        Some("/root/project")
    );
    assert_eq!(
        environment.lifecycle_state,
        EnvironmentLifecycleState::Dormant
    );
}

#[test]
fn test_environment_snapshot_accepts_legacy_json_without_connection_ref() {
    let raw = r#"{
        "label": "Local",
        "kind": "Local",
        "authority_key": "local:/tmp",
        "active_workspace_root": "/tmp",
        "lifecycle_state": "Connected"
    }"#;

    let environment: EnvironmentSnapshot =
        serde_json::from_str(raw).expect("legacy environment json should decode");

    assert_eq!(environment.kind, EnvironmentKind::Local);
    assert_eq!(environment.connection_ref, None);
}

#[test]
fn test_sqlite_round_trips_vertical_tabs_panel_open() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let database_path = tempdir.path().join("ashide.sqlite");
    let mut conn = setup_database(&database_path).expect("database should initialize");

    let app_state = AppState {
        windows: vec![
            test_terminal_window_snapshot(false),
            test_terminal_window_snapshot(true),
        ],
        active_window_index: Some(1),
        block_lists: Default::default(),
    };

    save_app_state(&mut conn, &app_state).expect("app state should save");

    let restored = read_sqlite_data(&mut conn, None)
        .expect("app state should load")
        .app_state;

    assert_eq!(restored.active_window_index, Some(1));
    assert_eq!(
        restored
            .windows
            .iter()
            .map(|window| window.vertical_tabs_panel_open)
            .collect::<Vec<_>>(),
        vec![false, true]
    );
}

#[test]
fn test_sqlite_round_trips_custom_vertical_tabs_title() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let database_path = tempdir.path().join("ashide.sqlite");
    let mut conn = setup_database(&database_path).expect("database should initialize");

    let app_state = AppState {
        windows: vec![WindowSnapshot {
            environment: None,
            workspace_sessions: vec![],
            tabs: vec![TabSnapshot {
                environment: None,
                custom_title: None,
                root: PaneNodeSnapshot::Leaf(LeafSnapshot {
                    is_focused: true,
                    custom_vertical_tabs_title: Some("Production API".to_string()),
                    contents: LeafContents::Terminal(TerminalPaneSnapshot {
                        uuid: vec![42],
                        cwd: Some("/tmp".to_string()),
                        shell_launch_data: Some(ShellLaunchData::Executable {
                            executable_path: PathBuf::from("/bin/zsh"),
                            shell_type: crate::terminal::shell::ShellType::Zsh,
                        }),
                        is_active: true,
                        is_read_only: false,
                        input_config: None,
                        llm_model_override: None,
                        active_profile_id: None,
                        conversation_ids_to_restore: vec![],
                        active_conversation_id: None,
                        cli_agent: None,
                        cli_command: None,
                        cli_agent_origin: None,
                        cli_agent_session_id: None,
                    }),
                }),
                default_directory_color: None,
                selected_color: SelectedTabColor::default(),
                left_panel: None,
                right_panel: None,
            }],
            active_tab_index: 0,
            bounds: None,
            fullscreen_state: Default::default(),
            quake_mode: false,
            universal_search_width: None,
            warp_ai_width: None,
            voltron_width: None,
            local_drive_index_width: None,
            left_panel_open: false,
            vertical_tabs_panel_open: false,
            left_panel_width: None,
            right_panel_width: None,
            agent_management_filters: None,
        }],
        active_window_index: Some(0),
        block_lists: Default::default(),
    };

    save_app_state(&mut conn, &app_state).expect("app state should save");

    let restored = read_sqlite_data(&mut conn, None)
        .expect("app state should load")
        .app_state;

    let PaneNodeSnapshot::Leaf(LeafSnapshot {
        custom_vertical_tabs_title,
        ..
    }) = &restored.windows[0].tabs[0].root
    else {
        panic!("Expected terminal pane leaf");
    };
    assert_eq!(
        custom_vertical_tabs_title.as_deref(),
        Some("Production API")
    );
}

#[test]
fn test_sqlite_round_trips_code_pane_with_multiple_tabs() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let database_path = tempdir.path().join("ashide.sqlite");
    let mut conn = setup_database(&database_path).expect("database should initialize");

    let app_state = AppState {
        windows: vec![WindowSnapshot {
            environment: None,
            workspace_sessions: vec![],
            tabs: vec![TabSnapshot {
                environment: None,
                custom_title: None,
                root: PaneNodeSnapshot::Leaf(LeafSnapshot {
                    is_focused: true,
                    custom_vertical_tabs_title: None,
                    contents: LeafContents::Code(CodePaneSnapShot::Local {
                        tabs: vec![
                            CodePaneTabSnapshot {
                                path: Some(PathBuf::from("/tmp/main.rs")),
                            },
                            CodePaneTabSnapshot {
                                path: Some(PathBuf::from("/tmp/lib.rs")),
                            },
                            CodePaneTabSnapshot { path: None },
                        ],
                        active_tab_index: 1,
                        source: Some(CodeSource::FileTree {
                            path: PathBuf::from("/tmp/main.rs"),
                        }),
                    }),
                }),
                default_directory_color: None,
                selected_color: SelectedTabColor::default(),
                left_panel: None,
                right_panel: None,
            }],
            active_tab_index: 0,
            bounds: None,
            fullscreen_state: Default::default(),
            quake_mode: false,
            universal_search_width: None,
            warp_ai_width: None,
            voltron_width: None,
            local_drive_index_width: None,
            left_panel_open: false,
            vertical_tabs_panel_open: false,
            left_panel_width: None,
            right_panel_width: None,
            agent_management_filters: None,
        }],
        active_window_index: Some(0),
        block_lists: Default::default(),
    };

    save_app_state(&mut conn, &app_state).expect("app state should save");

    let restored = read_sqlite_data(&mut conn, None)
        .expect("app state should load")
        .app_state;

    assert_eq!(restored.windows.len(), 1);
    let restored_tab = &restored.windows[0].tabs[0];
    let PaneNodeSnapshot::Leaf(LeafSnapshot {
        contents:
            LeafContents::Code(CodePaneSnapShot::Local {
                tabs,
                active_tab_index,
                source,
            }),
        ..
    }) = &restored_tab.root
    else {
        panic!("Expected code pane leaf");
    };

    assert_eq!(tabs.len(), 3);
    assert_eq!(*active_tab_index, 1);
    assert_eq!(tabs[0].path, Some(PathBuf::from("/tmp/main.rs")));
    assert_eq!(tabs[1].path, Some(PathBuf::from("/tmp/lib.rs")));
    assert_eq!(tabs[2].path, None);
    assert!(matches!(source, Some(CodeSource::FileTree { .. })));
}

fn assert_encode_then_decode_preserves_original_path(original_path: PathBuf) {
    let bytes = encode_path(original_path.clone());
    let decoded_path = decode_path(bytes);
    assert_eq!(original_path, decoded_path);
}

/// Test that a local path can be encoded and decoded. We use this when persisting a local
/// file path for notebooks in sqlite. We need this test because Windows `OsString`s are
/// often arbitrary sequences of 16-bit values, unlike Unix which uses sequences of 8-bit
/// values (bytes). Since `diesel::sql_types::Binary` deals with sequences of bytes (`u8`)
/// we need to perform special casting on `OsString`s on Windows.
#[test]
fn test_path_encode_decode() {
    // Empty path
    assert_encode_then_decode_preserves_original_path(PathBuf::new());

    // Windows-style paths
    assert_encode_then_decode_preserves_original_path(PathBuf::from(r"C:\windows\system32.dll"));
    assert_encode_then_decode_preserves_original_path(PathBuf::from("c:temp"));
    assert_encode_then_decode_preserves_original_path(PathBuf::from(r"\temp"));
    assert_encode_then_decode_preserves_original_path(PathBuf::from(r"\temp\emoji\🙈.txt"));
    assert_encode_then_decode_preserves_original_path(PathBuf::from(r"\temp\ñoñàscii\temp.txt"));
    assert_encode_then_decode_preserves_original_path(PathBuf::from(r"\temp\hindi\हिन्दी"));
    assert_encode_then_decode_preserves_original_path(PathBuf::from(r"\temp\cjk\狗没有耐心"));

    // Unix-style paths
    assert_encode_then_decode_preserves_original_path(PathBuf::from(
        "/home/persistence/example.sql",
    ));
    assert_encode_then_decode_preserves_original_path(PathBuf::from("./database/log.txt"));
    assert_encode_then_decode_preserves_original_path(PathBuf::from("/temp/emoji/🙈.txt"));
    assert_encode_then_decode_preserves_original_path(PathBuf::from("/temp/ñoñàscii/temp.txt"));
    assert_encode_then_decode_preserves_original_path(PathBuf::from("/temp/hindi/हिन्दी"));
    assert_encode_then_decode_preserves_original_path(PathBuf::from("/temp/cjk/狗没有耐心"));
}

#[test]
fn test_local_permissions_ignore_legacy_guest_columns() {
    // Use a hardcoded timestamp to ensure this test works on systems with more-than-microsecond
    // precision.
    let permissions_ts_micros = 123456;
    let permissions_ts =
        ServerTimestamp::from_unix_timestamp_micros(permissions_ts_micros).unwrap();

    let db_permissions = ObjectPermissions {
        id: 42,
        object_metadata_id: 10,
        subject_type: "USER".to_string(),
        subject_id: Some("7".to_string()),
        subject_uid: "user_uid12345678912345".to_string(),
        permissions_last_updated_at: Some(permissions_ts_micros),
        object_guests: Some(vec![1, 2, 3]),
        anyone_with_link_access_level: Some("VIEW".to_string()),
        anyone_with_link_source: Some(vec![4, 5, 6]),
    };

    let stored_permissions = super::to_stored_object_permissions(&db_permissions, None);
    assert_eq!(
        stored_permissions,
        Some(StoredObjectPermissions {
            owner: Owner::User {
                user_uid: crate::auth::UserUid::new("7"),
            },
            permissions_last_updated_ts: Some(permissions_ts),
        })
    );
}

#[test]
fn test_local_permissions_reject_legacy_team_owner() {
    let db_permissions = ObjectPermissions {
        id: 42,
        object_metadata_id: 10,
        subject_type: "TEAM".to_string(),
        subject_id: Some("7".to_string()),
        subject_uid: "team_uid12345678912345".to_string(),
        permissions_last_updated_at: None,
        object_guests: None,
        anyone_with_link_access_level: None,
        anyone_with_link_source: None,
    };

    assert_eq!(
        super::to_stored_object_permissions(&db_permissions, None),
        None
    );
}
