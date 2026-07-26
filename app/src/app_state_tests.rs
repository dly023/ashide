use super::*;

#[test]
fn test_has_horizontal_split() {
    let single_leaf = PaneNodeSnapshot::Leaf(LeafSnapshot {
        container_uuid: vec![6; 16],
        session_binding: None,
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
                    container_uuid: vec![24; 16],
                    session_binding: None,
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
                    container_uuid: vec![38; 16],
                    session_binding: None,
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

#[cfg(feature = "local_fs")]
fn terminal_pane_node(uuid: u8, is_read_only: bool) -> PaneNodeSnapshot {
    PaneNodeSnapshot::Leaf(LeafSnapshot {
        container_uuid: vec![57; 16],
        session_binding: None,
        is_focused: false,
        custom_vertical_tabs_title: None,
        contents: LeafContents::Terminal(TerminalPaneSnapshot {
            uuid: vec![uuid],
            cwd: Some(format!("/tmp/{uuid}")),
            shell_launch_data: None,
            is_active: false,
            is_read_only,
            input_config: None,
            llm_model_override: None,
            active_profile_id: None,
            conversation_ids_to_restore: Vec::new(),
            active_conversation_id: None,
        }),
    })
}

#[cfg(feature = "local_fs")]
#[test]
fn test_environment_runtime_restore_tree_preserves_leaf_session_binding() {
    let binding = PaneSessionBinding {
        agent: Some("Antigravity".to_string()),
        command: Some("agy".to_string()),
        origin: Some(CliAgentSessionOrigin::CommandDetected),
        session_id: None,
        cwd: Some("/root/manga-review-platform".to_string()),
        source_identity_keys: vec![
            "lr117-remote-sourceless".to_string(),
            "ssh:ssh-config:remote-fixture-secondary::source:lr117-remote-sourceless".to_string(),
        ],
    };
    let mut tree = terminal_pane_node(7, false);
    let PaneNodeSnapshot::Leaf(leaf) = &mut tree else {
        panic!("expected terminal leaf");
    };
    leaf.container_uuid = vec![0x61, 0x72, 0x83, 0x94];
    leaf.session_binding = Some(binding.clone());

    let restored = tree.into_environment_runtime_restore_tree();
    let PaneNodeSnapshot::Leaf(leaf) = restored else {
        panic!("expected placeholder leaf");
    };
    assert!(matches!(
        leaf.contents,
        LeafContents::EnvironmentRuntimePlaceholder
    ));
    assert_eq!(leaf.container_uuid, vec![0x61, 0x72, 0x83, 0x94]);
    assert_eq!(leaf.session_binding, Some(binding));
}

#[cfg(feature = "local_fs")]
#[test]
fn test_read_only_terminal_is_not_persistable() {
    assert_eq!(terminal_pane_node(1, true).into_persistable(), None);
}

#[cfg(feature = "local_fs")]
#[test]
fn test_persistable_tree_prunes_read_only_sibling_and_collapses_branch() {
    let tree = PaneNodeSnapshot::Branch(BranchSnapshot {
        direction: SplitDirection::Horizontal,
        children: vec![
            (PaneFlex(1.0), terminal_pane_node(1, true)),
            (PaneFlex(2.0), terminal_pane_node(2, false)),
        ],
    });

    let persisted = tree
        .into_persistable()
        .expect("writable sibling must survive");
    let PaneNodeSnapshot::Leaf(LeafSnapshot {
        contents: LeafContents::Terminal(terminal),
        ..
    }) = persisted
    else {
        panic!("single surviving child must collapse to a terminal leaf");
    };
    assert_eq!(terminal.uuid, vec![2]);
    assert!(!terminal.is_read_only);
}

#[cfg(feature = "local_fs")]
#[test]
fn test_persistable_tree_drops_tab_when_all_children_are_read_only() {
    let tree = PaneNodeSnapshot::Branch(BranchSnapshot {
        direction: SplitDirection::Vertical,
        children: vec![
            (PaneFlex(1.0), terminal_pane_node(1, true)),
            (PaneFlex(1.0), terminal_pane_node(2, true)),
        ],
    });

    assert_eq!(tree.into_persistable(), None);
}

#[cfg(feature = "local_fs")]
#[test]
fn test_persistable_tree_prunes_transient_provider_panes_before_sqlite() {
    let tree = PaneNodeSnapshot::Branch(BranchSnapshot {
        direction: SplitDirection::Horizontal,
        children: vec![
            (
                PaneFlex(1.0),
                PaneNodeSnapshot::Leaf(LeafSnapshot {
                    container_uuid: vec![132; 16],
                    session_binding: None,
                    is_focused: true,
                    custom_vertical_tabs_title: None,
                    contents: LeafContents::ProviderConnection {
                        node_id: "node-1".to_string(),
                    },
                }),
            ),
            (PaneFlex(1.0), terminal_pane_node(3, false)),
        ],
    });

    let persisted = tree
        .into_persistable()
        .expect("terminal sibling must survive");
    assert!(matches!(
        persisted,
        PaneNodeSnapshot::Leaf(LeafSnapshot {
            contents: LeafContents::Terminal(TerminalPaneSnapshot { uuid, .. }),
            ..
        }) if uuid == vec![3]
    ));
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
            container_uuid: vec![14; 16],
            session_binding: None,
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
            }),
        }),
        default_directory_color: None,
        selected_color: SelectedTabColor::default(),
        left_panel: None,
        right_panel: None,
    }
}

#[cfg(feature = "local_fs")]
#[test]
fn workspace_session_from_tabs_uses_leaf_title_not_tab_group_title() {
    let mut first = terminal_pane_node(1, false);
    let PaneNodeSnapshot::Leaf(first_leaf) = &mut first else {
        panic!("expected first terminal leaf");
    };
    first_leaf.container_uuid = vec![1; 16];
    first_leaf.custom_vertical_tabs_title = Some("API pane".to_owned());

    let mut second = terminal_pane_node(2, false);
    let PaneNodeSnapshot::Leaf(second_leaf) = &mut second else {
        panic!("expected second terminal leaf");
    };
    second_leaf.container_uuid = vec![2; 16];
    second_leaf.custom_vertical_tabs_title = Some("Tests pane".to_owned());

    let sessions = WorkspaceSessionSnapshot::from_tabs(
        &[TabSnapshot {
            environment: Some(EnvironmentSnapshot::local(Some("/repo".to_owned()))),
            custom_title: Some("Backend group".to_owned()),
            root: PaneNodeSnapshot::Branch(BranchSnapshot {
                direction: SplitDirection::Horizontal,
                children: vec![(PaneFlex(1.0), first), (PaneFlex(1.0), second)],
            }),
            default_directory_color: None,
            selected_color: SelectedTabColor::default(),
            left_panel: None,
            right_panel: None,
        }],
        None,
    );

    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].container_uuid, Some(vec![1; 16]));
    assert_eq!(sessions[0].label.as_deref(), Some("API pane"));
    assert_eq!(sessions[1].container_uuid, Some(vec![2; 16]));
    assert_eq!(sessions[1].label.as_deref(), Some("Tests pane"));
    assert!(sessions
        .iter()
        .all(|session| session.label.as_deref() != Some("Backend group")));
}

#[test]
fn test_workspace_session_snapshot_uses_tab_scoped_environment() {
    let mut ssh_server =
        warp_ssh_manager::SshServerInfo::new_default("ssh-config:remote-fixture-dev".to_string());
    ssh_server.host = "remote-fixture-dev".to_string();
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
                    "ssh-config:remote-fixture-dev".to_string(),
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
            "ssh-config:remote-fixture-dev".to_string(),
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
        Some("ssh:ssh-config:remote-fixture-dev")
    );
}

#[test]
fn test_workspace_session_snapshot_collects_terminal_metadata() {
    let environment = EnvironmentSnapshot::local(Some("/repo".to_string()));
    let tabs = vec![TabSnapshot {
        environment: None,
        custom_title: Some("Codex".to_string()),
        root: PaneNodeSnapshot::Leaf(LeafSnapshot {
            container_uuid: vec![94; 16],
            session_binding: None,
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
    assert_eq!(sessions[0].label, None);
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
            container_uuid: vec![139; 16],
            session_binding: Some(PaneSessionBinding {
                agent: Some("Codex".to_string()),
                command: Some("codex".to_string()),
                origin: Some(CliAgentSessionOrigin::CommandDetected),
                session_id: None,
                cwd: Some("/repo".to_string()),
                source_identity_keys: Vec::new(),
            }),
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
            container_uuid: vec![182; 16],
            session_binding: None,
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
    assert_eq!(sessions[0].container_uuid, Some(vec![182; 16]));
    assert_eq!(
        sessions[0].logical_key(),
        format!("local::pane:{}", "b6".repeat(16))
    );
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
    // 与生产一致:live container 由 id 的 tab: 前缀决定，不由 is_active 推断；
    // 同时必须携带稳定 pane UUID，不能让测试 fixture 继续保护 tab/leaf 身份债务。
    let is_live_container = id.starts_with("tab:");
    WorkspaceSessionSnapshot {
        id: id.to_string(),
        container_uuid: is_live_container.then(|| id.as_bytes().to_vec()),
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
        is_live_container,
    }
}

/// 构建一个携带 stale 派生 pin 的 indexed/virtual snapshot。
///
/// 生产 builder 已禁止预投影 pin；此 fixture 只验证 merge 即使收到旧/错误输入，
/// 也会清除派生展示态并把最终投影交给 SessionNavigatorReducer::Refresh。
fn test_indexed_session_snapshot(
    id: &str,
    cli_agent: &str,
    native_session_id: &str,
    updated_at_unix_ms: Option<i64>,
    environment_authority_key: Option<&str>,
) -> WorkspaceSessionSnapshot {
    let mut snapshot = test_workspace_session_in_environment(
        id,
        Some(cli_agent),
        Some(native_session_id),
        false,
        updated_at_unix_ms,
        environment_authority_key,
    );
    snapshot.is_pinned = true;
    snapshot
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

    let sessions =
        WorkspaceSessionSnapshot::merge_for_session_navigator(vec![file_source, index_source]);

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

    let sessions =
        WorkspaceSessionSnapshot::merge_for_session_navigator(vec![local_source, ssh_source]);

    assert_eq!(sessions.len(), 2);
    let authorities = sessions
        .iter()
        .map(|session| session.environment_authority_key.as_deref())
        .collect::<std::collections::HashSet<_>>();
    assert!(authorities.contains(&Some("local:/repo")));
    assert!(authorities.contains(&Some("ssh:dev-box")));
}

#[test]
fn test_session_navigator_merges_terminal_bootstrap_authority_variants() {
    let indexed_source = test_workspace_session_in_environment(
        "external:Codex:indexed",
        Some("Codex"),
        Some("shared-local-session"),
        false,
        Some(100),
        Some("local"),
    );
    let live_source = test_workspace_session_in_environment(
        "tab:1:leaf:0",
        Some("Codex"),
        Some("shared-local-session"),
        true,
        Some(200),
        Some("local:/Users/admin/ashide"),
    );
    let live_container_key = live_source.logical_key();

    let sessions =
        WorkspaceSessionSnapshot::merge_for_session_navigator(vec![indexed_source, live_source]);

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "tab:1:leaf:0");
    assert!(sessions[0].is_active);
    assert_eq!(sessions[0].logical_key(), live_container_key);
    assert!(sessions[0]
        .stable_pin_keys()
        .contains(&"local::agent:Codex:shared-local-session".to_string()));
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

    let sessions =
        WorkspaceSessionSnapshot::merge_for_session_navigator(vec![history_source, live_source]);

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "tab:0:leaf:0");
    assert!(sessions[0].is_active);
}

#[test]
fn test_session_navigator_keeps_specific_index_title_when_live_label_is_generic_agent_name() {
    let mut live_source = test_workspace_session(
        "tab:0:leaf:0",
        Some("Codex"),
        Some("codex-session-1"),
        true,
        Some(300),
    );
    live_source.label = Some("Codex".to_string());
    let mut index_source = test_workspace_session(
        "external-index:Codex:index-a",
        Some("Codex"),
        Some("codex-session-1"),
        false,
        Some(200),
    );
    index_source.label = Some("Fix split pane title".to_string());

    let sessions =
        WorkspaceSessionSnapshot::merge_for_session_navigator(vec![live_source, index_source]);

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "tab:0:leaf:0");
    assert!(sessions[0].is_active);
    assert_eq!(sessions[0].label.as_deref(), Some("Fix split pane title"));
}

#[test]
fn test_session_navigator_keeps_specific_index_title_when_live_label_is_remote_prompt_title() {
    let mut live_source = test_workspace_session_in_environment(
        "tab:0:leaf:0",
        Some("Codex"),
        Some("codex-session-1"),
        true,
        Some(300),
        Some("ssh:root@remote-fixture-primary"),
    );
    live_source.label = Some("root@remote-fixture-primary".to_string());
    let mut index_source = test_workspace_session_in_environment(
        "external-index:Codex:index-a",
        Some("Codex"),
        Some("codex-session-1"),
        false,
        Some(200),
        Some("ssh:root@remote-fixture-primary"),
    );
    index_source.label = Some("这台机器是不是有挂载Nas mnt 目录".to_string());

    let sessions =
        WorkspaceSessionSnapshot::merge_for_session_navigator(vec![live_source, index_source]);

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "tab:0:leaf:0");
    assert!(sessions[0].is_active);
    assert_eq!(
        sessions[0].label.as_deref(),
        Some("这台机器是不是有挂载Nas mnt 目录")
    );
}

#[test]
fn test_workspace_session_title_fallback_is_independent_from_resume_flow() {
    let mut generic_codex_title = test_workspace_session(
        "external-index:Codex:index-a",
        Some("Codex"),
        Some("codex-session-1"),
        false,
        Some(200),
    );
    generic_codex_title.label = Some("Codex".to_string());

    assert_eq!(generic_codex_title.title_fallback_label(None), None);
    assert_eq!(
        generic_codex_title.title_fallback_label(Some("  Manual alias  ".to_string())),
        Some("Manual alias".to_string())
    );

    generic_codex_title.label = Some("Fix split pane title".to_string());
    assert_eq!(
        generic_codex_title.title_fallback_label(None),
        Some("Fix split pane title".to_string())
    );
}

#[test]
fn test_session_navigator_ignores_volatile_tab_pin_keys() {
    let live_terminal = test_workspace_session("tab:4:leaf:0", None, None, true, None);
    let mut pinned_ids = std::collections::HashSet::new();
    pinned_ids.insert("tab:4:leaf:0".to_string());
    pinned_ids.insert("local::source:tab:4:leaf:0".to_string());

    let sessions = WorkspaceSessionSnapshot::merge_for_session_navigator(vec![live_terminal]);

    assert_eq!(sessions.len(), 1);
    assert!(!sessions[0].is_pinned);
}

#[test]
fn test_session_navigator_merge_does_not_project_or_preserve_pin_state() {
    let mut virtual_session = test_workspace_session(
        "external-index:Codex:pin-owner",
        Some("Codex"),
        Some("codex-pin-owner"),
        false,
        Some(100),
    );
    virtual_session.is_pinned = true;

    let sessions = WorkspaceSessionSnapshot::merge_for_session_navigator(vec![virtual_session]);

    assert_eq!(sessions.len(), 1);
    assert!(
        !sessions[0].is_pinned,
        "merge only reconciles backing rows; SessionNavigatorReducer::Refresh must be the sole pin projection owner"
    );
}

#[test]
fn test_session_navigator_consumed_live_row_does_not_inherit_virtual_pin_state() {
    let live_source = test_workspace_session_in_environment(
        "tab:7:leaf:0",
        Some("Codex"),
        Some("remote-session-1"),
        true,
        None,
        Some("ssh:remote-fixture-primary"),
    );
    let indexed_source = test_workspace_session_in_environment(
        "remote-index:codex-session-1",
        Some("Codex"),
        Some("remote-session-1"),
        false,
        Some(100),
        Some("ssh:remote-fixture-primary"),
    );
    let sessions =
        WorkspaceSessionSnapshot::merge_for_session_navigator(vec![live_source, indexed_source]);

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "tab:7:leaf:0");
    assert!(sessions[0].is_active);
    assert!(sessions[0].is_live_container());
    assert!(
        !sessions[0].is_pinned,
        "a consumed indexed/virtual row must not make its materialized live tab look pinned just because a durable agent pin key exists"
    );
}

#[test]
fn test_session_navigator_consumed_live_row_does_not_inherit_preassigned_virtual_pin() {
    // 模拟旧 builder 或错误调用方携带 stale is_pinned=true。merge 必须清掉
    // 这个派生展示态，最终只允许 reducer 根据 effective identity keys 投影。
    let live_source = test_workspace_session_in_environment(
        "tab:7:leaf:0",
        Some("Codex"),
        Some("remote-session-1"),
        true,
        None,
        Some("ssh:remote-fixture-primary"),
    );
    let mut pinned_ids = std::collections::HashSet::new();
    pinned_ids.insert("ssh:remote-fixture-primary::agent:Codex:remote-session-1".to_string());
    let indexed_source = test_indexed_session_snapshot(
        "remote-index:codex-session-1",
        "Codex",
        "remote-session-1",
        Some(100),
        Some("ssh:remote-fixture-primary"),
    );
    // 确认 helper 正确预置了 pin(与生产一致)
    assert!(
        indexed_source.is_pinned,
        "fixture must carry stale is_pinned=true before merge"
    );

    let sessions =
        WorkspaceSessionSnapshot::merge_for_session_navigator(vec![live_source, indexed_source]);

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "tab:7:leaf:0");
    assert!(sessions[0].is_live_container());
    assert!(
        !sessions[0].is_pinned,
        "merge must clear stale source pin state before reducer projection"
    );
}

#[test]
fn test_session_navigator_ignores_stable_agent_pin_key_for_live_tab() {
    let live_agent = test_workspace_session(
        "tab:4:leaf:0",
        Some("Codex"),
        Some("codex-session-1"),
        true,
        None,
    );
    let sessions = WorkspaceSessionSnapshot::merge_for_session_navigator(vec![live_agent]);

    assert_eq!(sessions.len(), 1);
    assert!(
        !sessions[0].is_pinned,
        "durable agent pin keys apply to virtual/history rows, not to physical live tabs"
    );
}

#[test]
fn test_session_navigator_keeps_plain_terminal_without_agent() {
    let terminal = test_workspace_session("tab:0:leaf:0", None, None, true, None);

    let sessions = WorkspaceSessionSnapshot::merge_for_session_navigator(vec![terminal]);

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].kind, WorkspaceSessionKind::Terminal);
    assert_eq!(sessions[0].cli_agent, None);
}

#[test]
fn test_session_navigator_merge_preserves_source_order_and_clears_pin_projection() {
    // merge 不排序,只负责合并去重。排序语义(pinned 优先 + updated_at 降序)
    // 由 Session Navigator reducer Refresh.reconcile_display_order 在分配 display_order 时实现。
    // 这里验证 merge 保持 sources 原始顺序，并清除所有派生 pin 展示态。
    let older = test_workspace_session("older", Some("Claude"), Some("older"), false, Some(10));
    let newer = test_workspace_session("newer", Some("Claude"), Some("newer"), false, Some(20));
    let active = test_workspace_session("active", Some("Claude"), Some("active"), true, Some(1));
    let mut pinned =
        test_workspace_session("pinned", Some("Claude"), Some("pinned"), false, Some(0));
    pinned.is_pinned = true;

    let sessions =
        WorkspaceSessionSnapshot::merge_for_session_navigator(vec![older, newer, active, pinned]);

    let ids = sessions
        .iter()
        .map(|session| session.id.as_str())
        .collect::<Vec<_>>();
    // sources 原始顺序保留
    assert_eq!(ids, vec!["older", "newer", "active", "pinned"]);
    assert!(
        !sessions
            .iter()
            .find(|session| session.id == "pinned")
            .expect("pinned session exists")
            .is_pinned
    );
    assert!(
        !sessions
            .iter()
            .find(|session| session.id == "older")
            .expect("older session exists")
            .is_pinned
    );
}

#[test]
fn test_live_pane_identity_uses_container_uuid_not_layout_coordinate() {
    let container_uuid = vec![0x10, 0x20, 0x30, 0x40];
    let mut first_tab = terminal_tab(Some(EnvironmentSnapshot::local(None)), "/repo", "Codex");
    let PaneNodeSnapshot::Leaf(LeafSnapshot {
        container_uuid: leaf_container_uuid,
        session_binding: None,
        contents: LeafContents::Terminal(terminal),
        ..
    }) = &mut first_tab.root
    else {
        panic!("terminal_tab 必须生成 terminal leaf");
    };
    *leaf_container_uuid = container_uuid.clone();
    terminal.uuid = vec![0xee; 16];

    let first = WorkspaceSessionSnapshot::from_tabs(&[first_tab.clone()], None)
        .pop()
        .expect("first live terminal");
    assert_eq!(
        first.container_uuid.as_deref(),
        Some(container_uuid.as_slice())
    );
    assert_eq!(first.logical_key(), "local::pane:10203040");
    assert!(!first.stable_user_state_keys().contains(&first.id));
    assert!(first
        .stable_user_state_keys()
        .contains(&"local::pane:10203040".to_owned()));

    let shifted = WorkspaceSessionSnapshot::from_tabs(
        &[
            terminal_tab(Some(EnvironmentSnapshot::local(None)), "/other", "Other"),
            first_tab,
        ],
        None,
    )
    .into_iter()
    .find(|session| session.container_uuid.as_deref() == Some(container_uuid.as_slice()))
    .expect("shifted live terminal");

    assert_eq!(shifted.id, "tab:1:leaf:0");
    assert_eq!(shifted.logical_key(), first.logical_key());
    assert_eq!(
        shifted.stable_user_state_keys(),
        first.stable_user_state_keys()
    );
}

#[test]
fn test_session_navigator_merge_is_source_order_independent_for_live_container_ownership() {
    let provider_session_id = "codex-order-independent";
    let virtual_session = WorkspaceSessionSnapshot {
        id: "external:Codex:order-independent".to_owned(),
        container_uuid: None,
        kind: WorkspaceSessionKind::AgentTerminal,
        label: Some("外置标题".to_owned()),
        environment_authority_key: Some("local".to_owned()),
        cwd: Some("/repo".to_owned()),
        startup_directory: None,
        cli_agent: Some("Codex".to_owned()),
        cli_command: Some("codex".to_owned()),
        cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
        conversation_ids: Vec::new(),
        active_conversation_id: None,
        cli_agent_session_id: Some(provider_session_id.to_owned()),
        is_active: false,
        is_pinned: false,
        updated_at_unix_ms: Some(20),
        is_live_container: false,
    };
    let live_session = WorkspaceSessionSnapshot {
        id: "tab:3:leaf:0".to_owned(),
        container_uuid: Some(vec![0xde, 0xad, 0xbe, 0xef]),
        kind: WorkspaceSessionKind::AgentTerminal,
        label: Some("Codex".to_owned()),
        environment_authority_key: Some("local".to_owned()),
        cwd: Some("/repo".to_owned()),
        startup_directory: None,
        cli_agent: Some("Codex".to_owned()),
        cli_command: Some("codex".to_owned()),
        cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
        conversation_ids: Vec::new(),
        active_conversation_id: None,
        cli_agent_session_id: Some(provider_session_id.to_owned()),
        is_active: true,
        is_pinned: false,
        updated_at_unix_ms: None,
        is_live_container: true,
    };

    for sources in [
        vec![virtual_session.clone(), live_session.clone()],
        vec![live_session.clone(), virtual_session.clone()],
    ] {
        let merged = WorkspaceSessionSnapshot::merge_for_session_navigator(sources);
        assert_eq!(merged.len(), 1);
        assert!(merged[0].is_live_container());
        assert_eq!(merged[0].id, live_session.id);
        assert_eq!(merged[0].container_uuid, live_session.container_uuid);
        assert_eq!(merged[0].logical_key(), "local::pane:deadbeef");
        assert_eq!(merged[0].label.as_deref(), Some("外置标题"));
    }
}
