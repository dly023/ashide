use super::*;

#[test]
fn test_has_horizontal_split() {
    let single_leaf = PaneNodeSnapshot::Leaf(LeafSnapshot {
        is_focused: false,
        custom_vertical_tabs_title: None,
        contents: LeafContents::Code(CodePaneSnapShot::Local {
            tabs: vec![CodePaneTabSnapshot {
                path: Some(PathBuf::new()),
            }],
            active_tab_index: 0,
            source: None,
        }),
    });
    assert!(!single_leaf.has_horizontal_split());

    let horizontal_split = PaneNodeSnapshot::Branch(BranchSnapshot {
        direction: SplitDirection::Horizontal,
        children: vec![
            (
                PaneFlex(1.),
                PaneNodeSnapshot::Leaf(LeafSnapshot {
                    is_focused: false,
                    custom_vertical_tabs_title: None,
                    contents: LeafContents::Code(CodePaneSnapShot::Local {
                        tabs: vec![CodePaneTabSnapshot {
                            path: Some(PathBuf::new()),
                        }],
                        active_tab_index: 0,
                        source: None,
                    }),
                }),
            ),
            (
                PaneFlex(1.),
                PaneNodeSnapshot::Leaf(LeafSnapshot {
                    is_focused: false,
                    custom_vertical_tabs_title: None,
                    contents: LeafContents::Code(CodePaneSnapShot::Local {
                        tabs: vec![CodePaneTabSnapshot {
                            path: Some(PathBuf::new()),
                        }],
                        active_tab_index: 0,
                        source: None,
                    }),
                }),
            ),
        ],
    });
    assert!(horizontal_split.has_horizontal_split());
}

#[test]
fn test_code_pane_snapshot_single_tab() {
    let snapshot = CodePaneSnapShot::Local {
        tabs: vec![CodePaneTabSnapshot {
            path: Some(PathBuf::from("/tmp/test.rs")),
        }],
        active_tab_index: 0,
        source: Some(CodeSource::FileTree {
            path: PathBuf::from("/tmp/test.rs"),
        }),
    };
    let CodePaneSnapShot::Local {
        tabs,
        active_tab_index,
        source,
    } = &snapshot;
    assert_eq!(tabs.len(), 1);
    assert_eq!(*active_tab_index, 0);
    assert_eq!(tabs[0].path, Some(PathBuf::from("/tmp/test.rs")));
    assert!(matches!(source, Some(CodeSource::FileTree { .. })));
}

#[test]
fn test_code_pane_snapshot_with_multiple_tabs() {
    let snapshot = CodePaneSnapShot::Local {
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
        source: Some(CodeSource::Link {
            path: PathBuf::from("/tmp/main.rs"),
            range_start: None,
            range_end: None,
        }),
    };
    let CodePaneSnapShot::Local {
        tabs,
        active_tab_index,
        source,
    } = &snapshot;
    assert_eq!(tabs.len(), 3);
    assert_eq!(*active_tab_index, 1);
    assert_eq!(tabs[0].path, Some(PathBuf::from("/tmp/main.rs")));
    assert_eq!(tabs[1].path, Some(PathBuf::from("/tmp/lib.rs")));
    assert_eq!(tabs[2].path, None);
    assert!(matches!(source, Some(CodeSource::Link { .. })));
}

#[test]
fn test_dormant_provider_environment_snapshot_uses_connection_ref() {
    let mut server = warp_ssh_manager::SshServerInfo::new_default("node-1".to_string());
    server.host = "example.internal".to_string();
    server.username = "root".to_string();
    server.port = 2222;

    let environment =
        crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
            "node-1".to_string(),
            &server,
            Some("/root/project".to_string()),
            EnvironmentLifecycleState::Dormant,
        );

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
    assert_eq!(environment.runtime_connection_ref(), Some("node-1"));
}

#[test]
fn test_runtime_connection_ref_falls_back_to_authority_key_for_legacy_snapshots() {
    let environment = EnvironmentSnapshot {
        label: "legacy".to_string(),
        kind: EnvironmentKind::Ssh,
        authority_key: "ssh:legacy-node".to_string(),
        connection_ref: None,
        active_workspace_root: None,
        lifecycle_state: EnvironmentLifecycleState::Dormant,
    };

    assert_eq!(environment.runtime_connection_ref(), Some("legacy-node"));
}

#[test]
fn test_runtime_connection_ref_ignores_terminal_bootstrap_environments() {
    let environment = EnvironmentSnapshot::local(Some("/tmp".to_string()));

    assert_eq!(environment.runtime_connection_ref(), None);
}

fn terminal_tab(environment: Option<EnvironmentSnapshot>, cwd: &str, title: &str) -> TabSnapshot {
    TabSnapshot {
        environment,
        custom_title: Some(title.to_string()),
        root: PaneNodeSnapshot::Leaf(LeafSnapshot {
            is_focused: true,
            custom_vertical_tabs_title: None,
            contents: LeafContents::Terminal(TerminalPaneSnapshot {
                uuid: vec![],
                cwd: Some(cwd.to_string()),
                shell_launch_data: None,
                is_active: true,
                is_read_only: false,
                input_config: None,
                llm_model_override: None,
                active_profile_id: None,
                conversation_ids_to_restore: Vec::new(),
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
    }
}

#[test]
fn test_workspace_session_snapshot_uses_tab_scoped_environment() {
    let mut ssh_server =
        warp_ssh_manager::SshServerInfo::new_default("ssh-config:dev-150".to_string());
    ssh_server.host = "dev-150".to_string();
    ssh_server.username = "root".to_string();

    let tabs = vec![
        terminal_tab(
            Some(EnvironmentSnapshot::local(Some("/repo".to_string()))),
            "/repo",
            "Local",
        ),
        terminal_tab(
            Some(
                crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
                    "ssh-config:dev-150".to_string(),
                    &ssh_server,
                    Some("/root/repo".to_string()),
                    EnvironmentLifecycleState::Connected,
                ),
            ),
            "/root/repo",
            "Remote",
        ),
    ];

    let window_fallback =
        crate::workspace::environment_provider::source_saved_ssh::runtime_transport_snapshot(
            "ssh-config:dev-150".to_string(),
            &ssh_server,
            Some("/root/repo".to_string()),
            EnvironmentLifecycleState::Connected,
        );
    let sessions = WorkspaceSessionSnapshot::from_tabs(&tabs, Some(&window_fallback));

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
fn test_workspace_session_snapshot_collects_terminal_metadata() {
    let environment = EnvironmentSnapshot::local(Some("/repo".to_string()));
    let tabs = vec![TabSnapshot {
        environment: None,
        custom_title: Some("Codex".to_string()),
        root: PaneNodeSnapshot::Leaf(LeafSnapshot {
            is_focused: true,
            custom_vertical_tabs_title: None,
            contents: LeafContents::Terminal(TerminalPaneSnapshot {
                uuid: vec![1, 2, 3],
                cwd: Some("/repo".to_string()),
                shell_launch_data: None,
                is_active: true,
                is_read_only: false,
                input_config: None,
                llm_model_override: None,
                active_profile_id: None,
                conversation_ids_to_restore: Vec::new(),
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
    }];

    let sessions = WorkspaceSessionSnapshot::from_tabs(&tabs, Some(&environment));

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "tab:0:leaf:0");
    assert_eq!(sessions[0].kind, WorkspaceSessionKind::Terminal);
    assert_eq!(sessions[0].label.as_deref(), Some("Codex"));
    assert_eq!(
        sessions[0].environment_authority_key.as_deref(),
        Some("local:/repo")
    );
    assert_eq!(sessions[0].cwd.as_deref(), Some("/repo"));
    assert!(sessions[0].is_active);
}

#[test]
fn test_workspace_session_snapshot_carries_cli_agent_command() {
    let tabs = vec![TabSnapshot {
        environment: None,
        custom_title: Some("Codex".to_string()),
        root: PaneNodeSnapshot::Leaf(LeafSnapshot {
            is_focused: true,
            custom_vertical_tabs_title: None,
            contents: LeafContents::Terminal(TerminalPaneSnapshot {
                uuid: vec![4, 5, 6],
                cwd: Some("/repo".to_string()),
                shell_launch_data: None,
                is_active: true,
                is_read_only: false,
                input_config: None,
                llm_model_override: None,
                active_profile_id: None,
                conversation_ids_to_restore: Vec::new(),
                active_conversation_id: None,
                cli_agent: Some("Codex".to_string()),
                cli_command: Some("codex".to_string()),
                cli_agent_origin: Some(CliAgentSessionOrigin::CommandDetected),
                cli_agent_session_id: None,
            }),
        }),
        default_directory_color: None,
        selected_color: SelectedTabColor::default(),
        left_panel: None,
        right_panel: None,
    }];

    let sessions = WorkspaceSessionSnapshot::from_tabs(&tabs, None);

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].kind, WorkspaceSessionKind::AgentTerminal);
    assert_eq!(sessions[0].cli_agent.as_deref(), Some("Codex"));
    assert_eq!(sessions[0].cli_command.as_deref(), Some("codex"));
    assert_eq!(
        sessions[0].cli_agent_origin,
        Some(CliAgentSessionOrigin::CommandDetected)
    );
}

#[test]
fn test_workspace_session_snapshot_collects_welcome_startup_directory() {
    let tabs = vec![TabSnapshot {
        environment: None,
        custom_title: None,
        root: PaneNodeSnapshot::Leaf(LeafSnapshot {
            is_focused: false,
            custom_vertical_tabs_title: None,
            contents: LeafContents::Welcome {
                startup_directory: Some(PathBuf::from("/repo")),
            },
        }),
        default_directory_color: None,
        selected_color: SelectedTabColor::default(),
        left_panel: None,
        right_panel: None,
    }];

    let sessions = WorkspaceSessionSnapshot::from_tabs(&tabs, None);

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].kind, WorkspaceSessionKind::Welcome);
    assert_eq!(sessions[0].startup_directory.as_deref(), Some("/repo"));
    assert_eq!(sessions[0].environment_authority_key, None);
}

fn test_workspace_session(
    id: &str,
    cli_agent: Option<&str>,
    native_session_id: Option<&str>,
    is_active: bool,
    updated_at_unix_ms: Option<i64>,
) -> WorkspaceSessionSnapshot {
    test_workspace_session_in_environment(
        id,
        cli_agent,
        native_session_id,
        is_active,
        updated_at_unix_ms,
        Some("local"),
    )
}

fn test_workspace_session_in_environment(
    id: &str,
    cli_agent: Option<&str>,
    native_session_id: Option<&str>,
    is_active: bool,
    updated_at_unix_ms: Option<i64>,
    environment_authority_key: Option<&str>,
) -> WorkspaceSessionSnapshot {
    WorkspaceSessionSnapshot {
        id: id.to_string(),
        kind: if cli_agent.is_some() {
            WorkspaceSessionKind::AgentTerminal
        } else {
            WorkspaceSessionKind::Terminal
        },
        label: Some(id.to_string()),
        environment_authority_key: environment_authority_key.map(str::to_string),
        cwd: None,
        startup_directory: None,
        cli_agent: cli_agent.map(str::to_string),
        cli_command: cli_agent.map(str::to_lowercase),
        cli_agent_origin: None,
        conversation_ids: Vec::new(),
        active_conversation_id: None,
        cli_agent_session_id: native_session_id.map(str::to_string),
        is_active,
        is_pinned: false,
        updated_at_unix_ms,
    }
}

#[test]
fn test_session_navigator_merges_codex_file_and_index_by_native_session_id() {
    let file_source = test_workspace_session(
        "external:Codex:file-a",
        Some("Codex"),
        Some("codex-session-1"),
        false,
        Some(100),
    );
    let index_source = test_workspace_session(
        "external-index:Codex:index-a",
        Some("Codex"),
        Some("codex-session-1"),
        false,
        Some(200),
    );

    let sessions = WorkspaceSessionSnapshot::merge_for_session_navigator(
        vec![file_source, index_source],
        &std::collections::HashSet::new(),
    );

    assert_eq!(sessions.len(), 1);
    assert_eq!(
        sessions[0].cli_agent_session_id.as_deref(),
        Some("codex-session-1")
    );
    assert_eq!(sessions[0].updated_at_unix_ms, Some(200));
}

#[test]
fn test_session_navigator_keeps_environments_separate_for_same_native_session_id() {
    let local_source = test_workspace_session_in_environment(
        "external:Codex:local",
        Some("Codex"),
        Some("shared-session"),
        false,
        Some(100),
        Some("local:/repo"),
    );
    let ssh_source = test_workspace_session_in_environment(
        "external:Codex:remote",
        Some("Codex"),
        Some("shared-session"),
        false,
        Some(200),
        Some("ssh:dev-box"),
    );

    let sessions = WorkspaceSessionSnapshot::merge_for_session_navigator(
        vec![local_source, ssh_source],
        &std::collections::HashSet::new(),
    );

    assert_eq!(sessions.len(), 2);
    let authorities = sessions
        .iter()
        .map(|session| session.environment_authority_key.as_deref())
        .collect::<std::collections::HashSet<_>>();
    assert!(authorities.contains(&Some("local:/repo")));
    assert!(authorities.contains(&Some("ssh:dev-box")));
}

#[test]
fn test_session_navigator_prefers_live_source_for_same_logical_session() {
    let history_source = test_workspace_session(
        "external-index:Codex:index-a",
        Some("Codex"),
        Some("codex-session-1"),
        false,
        Some(300),
    );
    let live_source = test_workspace_session(
        "tab:0:leaf:0",
        Some("Codex"),
        Some("codex-session-1"),
        true,
        None,
    );

    let sessions = WorkspaceSessionSnapshot::merge_for_session_navigator(
        vec![history_source, live_source],
        &std::collections::HashSet::new(),
    );

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "tab:0:leaf:0");
    assert!(sessions[0].is_active);
}

#[test]
fn test_session_navigator_ignores_volatile_tab_pin_keys() {
    let live_terminal = test_workspace_session("tab:4:leaf:0", None, None, true, None);
    let mut pinned_ids = std::collections::HashSet::new();
    pinned_ids.insert("tab:4:leaf:0".to_string());
    pinned_ids.insert("local::source:tab:4:leaf:0".to_string());

    let sessions =
        WorkspaceSessionSnapshot::merge_for_session_navigator(vec![live_terminal], &pinned_ids);

    assert_eq!(sessions.len(), 1);
    assert!(!sessions[0].is_pinned);
}

#[test]
fn test_session_navigator_uses_stable_agent_pin_key_for_resumed_tab() {
    let live_agent = test_workspace_session(
        "tab:4:leaf:0",
        Some("Codex"),
        Some("codex-session-1"),
        true,
        None,
    );
    let mut pinned_ids = std::collections::HashSet::new();
    pinned_ids.insert("local::agent:Codex:codex-session-1".to_string());

    let sessions =
        WorkspaceSessionSnapshot::merge_for_session_navigator(vec![live_agent], &pinned_ids);

    assert_eq!(sessions.len(), 1);
    assert!(sessions[0].is_pinned);
}

#[test]
fn test_session_navigator_keeps_plain_terminal_without_agent() {
    let terminal = test_workspace_session("tab:0:leaf:0", None, None, true, None);

    let sessions = WorkspaceSessionSnapshot::merge_for_session_navigator(
        vec![terminal],
        &std::collections::HashSet::new(),
    );

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].kind, WorkspaceSessionKind::Terminal);
    assert_eq!(sessions[0].cli_agent, None);
}

#[test]
fn test_session_navigator_sorts_pinned_then_updated_asc_without_active_jump() {
    let older = test_workspace_session("older", Some("Claude"), Some("older"), false, Some(10));
    let newer = test_workspace_session("newer", Some("Claude"), Some("newer"), false, Some(20));
    let active = test_workspace_session("active", Some("Claude"), Some("active"), true, Some(1));
    let pinned = test_workspace_session("pinned", Some("Claude"), Some("pinned"), false, Some(0));
    let mut pinned_ids = std::collections::HashSet::new();
    pinned_ids.insert(pinned.logical_key());

    let sessions = WorkspaceSessionSnapshot::merge_for_session_navigator(
        vec![older, newer, active, pinned],
        &pinned_ids,
    );

    let ids = sessions
        .iter()
        .map(|session| session.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["pinned", "active", "older", "newer"]);
}
