//! Module with integration test-only util methods setting up sqlite.

use std::path::PathBuf;

use chrono::NaiveDate;
use diesel::{Connection, ExpressionMethods, QueryDsl, RunQueryDsl};

use super::{model, schema, sqlite::init_db};
use crate::{
    code::editor_management::CodeSource, object_store::ObjectType, workflows::workflow::Workflow,
};

const LOCAL_TEST_USER_UID: &str = "local-test-user";
const NOTEBOOK_CLIENT_ID: &str = "Client-11111111-1111-4111-8111-111111111111";
const MISSING_NOTEBOOK_CLIENT_ID: &str = "Client-22222222-2222-4222-8222-222222222222";
const WORKFLOW_CLIENT_ID: &str = "Client-33333333-3333-4333-8333-333333333333";
const MISSING_WORKFLOW_CLIENT_ID: &str = "Client-44444444-4444-4444-8444-444444444444";

/// 当前 schema 下、供 session restoration integration tests 使用的最小数据库状态。
pub enum SessionRestorationTestFixture {
    Notebooks {
        terminal_cwd: PathBuf,
    },
    Workflows,
    MarkdownFile {
        terminal_cwd: PathBuf,
        markdown_path: PathBuf,
    },
    CodeFile {
        terminal_cwd: PathBuf,
        code_path: PathBuf,
    },
    Settings {
        terminal_cwd: PathBuf,
        current_page: String,
    },
}

enum TestPane {
    Terminal { cwd: PathBuf },
    Notebook { notebook_id: Option<String> },
    FileNotebook { path: PathBuf },
    Workflow { workflow_id: Option<String> },
    Code { path: PathBuf },
    Settings { current_page: String },
}

/// 先运行当前全部 migrations，再插入最小、等价的 session restoration 状态。
///
/// 这些测试以前复制历史二进制 SQLite fixture；二进制损坏后既无法打开，也会随着
/// schema 演进持续腐化。这里刻意通过当前 Diesel model 写入，保证测试数据始终符合
/// 当前 schema，而不是依赖旧数据库继续迁移。
pub fn create_session_restoration_test_database(fixture: SessionRestorationTestFixture) {
    let mut conn = init_db().expect("Should be able to initialize sqlite for integration tests.");

    conn.transaction::<(), diesel::result::Error, _>(|conn| {
        let panes = match fixture {
            SessionRestorationTestFixture::Notebooks { terminal_cwd } => {
                insert_notebook(
                    conn,
                    NOTEBOOK_CLIENT_ID,
                    "First Notebook",
                    "Notebook 1 content",
                )?;
                vec![
                    TestPane::Notebook {
                        notebook_id: Some(NOTEBOOK_CLIENT_ID.to_owned()),
                    },
                    TestPane::Notebook {
                        notebook_id: Some(MISSING_NOTEBOOK_CLIENT_ID.to_owned()),
                    },
                    TestPane::Terminal { cwd: terminal_cwd },
                ]
            }
            SessionRestorationTestFixture::Workflows => {
                insert_workflow(conn, WORKFLOW_CLIENT_ID, "My Workflow", "echo workflow")?;
                vec![
                    TestPane::Workflow {
                        workflow_id: Some(MISSING_WORKFLOW_CLIENT_ID.to_owned()),
                    },
                    TestPane::Workflow {
                        workflow_id: Some(WORKFLOW_CLIENT_ID.to_owned()),
                    },
                ]
            }
            SessionRestorationTestFixture::MarkdownFile {
                terminal_cwd,
                markdown_path,
            } => vec![
                TestPane::Terminal { cwd: terminal_cwd },
                TestPane::FileNotebook {
                    path: markdown_path,
                },
            ],
            SessionRestorationTestFixture::CodeFile {
                terminal_cwd,
                code_path,
            } => vec![
                TestPane::Terminal { cwd: terminal_cwd },
                TestPane::Code { path: code_path },
            ],
            SessionRestorationTestFixture::Settings {
                terminal_cwd,
                current_page,
            } => vec![
                TestPane::Terminal { cwd: terminal_cwd },
                TestPane::Settings { current_page },
            ],
        };

        insert_single_tab_snapshot(conn, panes)
    })
    .expect("Failed to create session restoration integration-test database.");
}

/// 用当前 commands schema 创建 history integration tests 依赖的固定历史数据。
pub fn create_history_test_database() {
    let mut conn = init_db().expect("Should be able to initialize sqlite for integration tests.");
    let first_day = NaiveDate::from_ymd_opt(2023, 7, 11).expect("valid fixture date");
    let second_day = NaiveDate::from_ymd_opt(2023, 7, 12).expect("valid fixture date");

    let commands = [
        (
            r#"echo "foo""#,
            first_day
                .and_hms_micro_opt(16, 29, 32, 92_176)
                .expect("valid fixture timestamp"),
            first_day
                .and_hms_micro_opt(16, 29, 33, 124_078)
                .expect("valid fixture timestamp"),
            None,
        ),
        (
            r#"[[ -n "foo" ]]"#,
            first_day
                .and_hms_micro_opt(16, 29, 34, 837_961)
                .expect("valid fixture timestamp"),
            first_day
                .and_hms_micro_opt(16, 29, 35, 124_078)
                .expect("valid fixture timestamp"),
            Some(r#"[[ -n {{string}} ]]"#),
        ),
        (
            r#"echo "bar""#,
            first_day
                .and_hms_opt(16, 29, 42)
                .expect("valid fixture timestamp"),
            first_day
                .and_hms_opt(16, 29, 43)
                .expect("valid fixture timestamp"),
            None,
        ),
        (
            "sed -i '' '/hello/d' foo",
            second_day
                .and_hms_opt(16, 29, 42)
                .expect("valid fixture timestamp"),
            second_day
                .and_hms_opt(16, 29, 43)
                .expect("valid fixture timestamp"),
            Some("sed -i '' '/{{string}}/d' {{file}}"),
        ),
    ];

    conn.transaction::<(), diesel::result::Error, _>(|conn| {
        for shell in ["zsh", "bash", "fish", "pwsh"] {
            for (command, start_ts, completed_ts, workflow_command) in commands {
                diesel::insert_into(schema::commands::dsl::commands)
                    .values(model::NewCommand {
                        command: command.to_owned(),
                        exit_code: Some(0),
                        start_ts: Some(start_ts),
                        completed_ts: Some(completed_ts),
                        pwd: Some("/Users/user".to_owned()),
                        shell: Some(shell.to_owned()),
                        username: Some("local:user".to_owned()),
                        hostname: Some("local:host".to_owned()),
                        session_id: Some(168_911_816_423_351),
                        git_branch: None,
                        object_store_workflow_id: None,
                        workflow_command: workflow_command.map(str::to_owned),
                        is_agent_executed: Some(false),
                    })
                    .execute(conn)?;
            }
        }
        Ok(())
    })
    .expect("Failed to create history integration-test database.");
}

fn insert_single_tab_snapshot(
    conn: &mut diesel::SqliteConnection,
    panes: Vec<TestPane>,
) -> Result<(), diesel::result::Error> {
    diesel::insert_into(schema::windows::dsl::windows)
        .values(model::NewWindow {
            active_tab_index: 0,
            window_width: None,
            window_height: None,
            origin_x: None,
            origin_y: None,
            quake_mode: false,
            universal_search_width: None,
            warp_ai_width: None,
            voltron_width: None,
            local_drive_index_width: None,
            fullscreen_state: 0,
            agent_management_filters: None,
            left_panel_open: Some(false),
            vertical_tabs_panel_open: Some(false),
            environment_json: None,
            restored_workspace_sessions_json: Some("[]".to_owned()),
        })
        .execute(conn)?;
    let window_id = latest_insert_id(conn)?;

    diesel::insert_into(schema::tabs::dsl::tabs)
        .values(model::NewTab {
            window_id,
            custom_title: None,
            color: None,
            environment_json: None,
        })
        .execute(conn)?;
    let tab_id = latest_insert_id(conn)?;

    diesel::insert_into(schema::pane_nodes::dsl::pane_nodes)
        .values(model::NewPaneNode {
            tab_id,
            parent_pane_node_id: None,
            flex: None,
            is_leaf: false,
        })
        .execute(conn)?;
    let root_id = latest_insert_id(conn)?;
    diesel::insert_into(schema::pane_branches::dsl::pane_branches)
        .values(model::NewPaneBranch {
            pane_node_id: root_id,
            horizontal: true,
        })
        .execute(conn)?;

    for (index, pane) in panes.into_iter().enumerate() {
        diesel::insert_into(schema::pane_nodes::dsl::pane_nodes)
            .values(model::NewPaneNode {
                tab_id,
                parent_pane_node_id: Some(root_id),
                flex: Some(1.0),
                is_leaf: true,
            })
            .execute(conn)?;
        let pane_node_id = latest_insert_id(conn)?;
        let kind = match &pane {
            TestPane::Terminal { .. } => model::TERMINAL_PANE_KIND,
            TestPane::Notebook { .. } | TestPane::FileNotebook { .. } => model::NOTEBOOK_PANE_KIND,
            TestPane::Workflow { .. } => model::WORKFLOW_PANE_KIND,
            TestPane::Code { .. } => model::CODE_PANE_KIND,
            TestPane::Settings { .. } => model::SETTINGS_PANE_KIND,
        };

        diesel::insert_into(schema::pane_leaves::dsl::pane_leaves)
            .values(model::NewPane {
                pane_node_id,
                kind: kind.to_owned(),
                is_focused: index == 0,
                custom_vertical_tabs_title: None,
            })
            .execute(conn)?;
        diesel::insert_into(schema::pane_container_identities::dsl::pane_container_identities)
            .values(model::NewPaneContainerIdentity {
                pane_node_id,
                uuid: vec![index as u8 + 1; 16],
                session_binding_json: None,
            })
            .execute(conn)?;

        match pane {
            TestPane::Terminal { cwd } => {
                diesel::insert_into(schema::terminal_panes::dsl::terminal_panes)
                    .values(model::NewTerminalPane {
                        id: pane_node_id,
                        uuid: vec![index as u8 + 101; 16],
                        cwd: Some(cwd.to_string_lossy().into_owned()),
                        is_active: true,
                        shell_launch_data: None,
                        input_config: None,
                        llm_model_override: None,
                        active_profile_id: None,
                        conversation_ids: None,
                        active_conversation_id: None,
                    })
                    .execute(conn)?;
            }
            TestPane::Notebook { notebook_id } => {
                diesel::insert_into(schema::notebook_panes::dsl::notebook_panes)
                    .values(model::NewNotebookPane {
                        id: pane_node_id,
                        notebook_id,
                        local_path: None,
                    })
                    .execute(conn)?;
            }
            TestPane::FileNotebook { path } => {
                diesel::insert_into(schema::notebook_panes::dsl::notebook_panes)
                    .values(model::NewNotebookPane {
                        id: pane_node_id,
                        notebook_id: None,
                        local_path: Some(encode_path(path)),
                    })
                    .execute(conn)?;
            }
            TestPane::Workflow { workflow_id } => {
                diesel::insert_into(schema::workflow_panes::dsl::workflow_panes)
                    .values(model::NewWorkflowPane {
                        id: pane_node_id,
                        workflow_id,
                    })
                    .execute(conn)?;
            }
            TestPane::Code { path } => {
                let source_data =
                    serde_json::to_string(&CodeSource::FileTree { path: path.clone() })
                        .expect("Code source must serialize");
                diesel::insert_into(schema::code_panes::dsl::code_panes)
                    .values(model::NewCodePane {
                        id: pane_node_id,
                        active_tab_index: 0,
                        source_data: Some(source_data),
                    })
                    .execute(conn)?;
                diesel::insert_into(schema::code_pane_tabs::dsl::code_pane_tabs)
                    .values(model::NewCodePaneTab {
                        code_pane_id: pane_node_id,
                        tab_index: 0,
                        local_path: Some(encode_path(path)),
                    })
                    .execute(conn)?;
            }
            TestPane::Settings { current_page } => {
                diesel::insert_into(schema::settings_panes::dsl::settings_panes)
                    .values(model::NewSettingsPane {
                        id: pane_node_id,
                        current_page,
                    })
                    .execute(conn)?;
            }
        }
    }

    diesel::insert_into(schema::app::dsl::app)
        .values(model::NewApp {
            active_window_id: Some(window_id),
        })
        .execute(conn)?;
    Ok(())
}

fn insert_notebook(
    conn: &mut diesel::SqliteConnection,
    client_id: &str,
    title: &str,
    data: &str,
) -> Result<(), diesel::result::Error> {
    diesel::insert_into(schema::notebooks::dsl::notebooks)
        .values(model::NewNotebook {
            title: Some(title.to_owned()),
            data: Some(data.to_owned()),
            ai_document_id: None,
        })
        .execute(conn)?;
    let object_id = latest_insert_id(conn)?;
    insert_local_object_metadata(
        conn,
        object_id,
        ObjectType::Notebook
            .sqlite_object_type_as_str()
            .into_owned(),
        client_id,
    )
}

fn insert_workflow(
    conn: &mut diesel::SqliteConnection,
    client_id: &str,
    name: &str,
    command: &str,
) -> Result<(), diesel::result::Error> {
    diesel::insert_into(schema::workflows::dsl::workflows)
        .values(model::NewWorkflow {
            data: serde_json::to_string(&Workflow::new(name, command))
                .expect("workflow fixture must serialize"),
        })
        .execute(conn)?;
    let object_id = latest_insert_id(conn)?;
    insert_local_object_metadata(
        conn,
        object_id,
        ObjectType::Workflow
            .sqlite_object_type_as_str()
            .into_owned(),
        client_id,
    )
}

fn insert_local_object_metadata(
    conn: &mut diesel::SqliteConnection,
    object_id: i32,
    object_type: String,
    client_id: &str,
) -> Result<(), diesel::result::Error> {
    diesel::insert_into(schema::object_metadata::dsl::object_metadata)
        .values(model::NewObjectMetadata {
            is_pending: true,
            object_type,
            revision_ts: None,
            server_id: None,
            client_id: Some(client_id.to_owned()),
            shareable_object_id: object_id,
            author_id: None,
            retry_count: 0,
            metadata_last_updated_ts: None,
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: Some(LOCAL_TEST_USER_UID.to_owned()),
            last_editor_uid: Some(LOCAL_TEST_USER_UID.to_owned()),
            current_editor: None,
        })
        .execute(conn)?;
    let metadata_id = latest_insert_id(conn)?;

    diesel::insert_into(schema::object_permissions::dsl::object_permissions)
        .values(model::NewObjectPermissions {
            object_metadata_id: metadata_id,
            subject_type: "USER".to_owned(),
            subject_id: Some(LOCAL_TEST_USER_UID.to_owned()),
            subject_uid: LOCAL_TEST_USER_UID.to_owned(),
            permissions_last_updated_at: None,
            object_guests: None,
            anyone_with_link_access_level: None,
            anyone_with_link_source: None,
        })
        .execute(conn)?;
    Ok(())
}

fn latest_insert_id(conn: &mut diesel::SqliteConnection) -> Result<i32, diesel::result::Error> {
    diesel::select(diesel::dsl::sql::<diesel::sql_types::Integer>(
        "last_insert_rowid()",
    ))
    .get_result(conn)
}

fn encode_path(path: PathBuf) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        path.into_os_string().into_vec()
    }

    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let wide_char_sequence: Vec<u16> = path.into_os_string().encode_wide().collect();
        bytemuck::cast_slice(wide_char_sequence.as_slice()).to_vec()
    }
}

/// Updates the 'user' and 'host' columns for stored blocks to the given values.
///
/// This is used at runtime to update the user and host values to real values based on the running
/// machine in integration tests that rely on accuracy of these values.
pub fn set_user_and_hostname_for_blocks(user: String, hostname: String) {
    let mut conn = init_db().expect("Should be able to establish sqlite connection.");

    // Update the 'user' and 'host' columns to their real values (based on the machine on which this test is running)
    // for blocks that were stored with the placeholder 'local:user' and 'local:host' values.
    //
    // This allows us to use real (rather than mocked out) logic for matching restored
    // blocks to the appropriate session based on session hostnamebased on system hostname.
    diesel::update(schema::blocks::dsl::blocks.filter(schema::blocks::user.eq("local:user")))
        .set((
            schema::blocks::user.eq(user),
            schema::blocks::host.eq(hostname),
        ))
        .execute(&mut conn)
        .expect("Failed to update user and hostname for restored blocks.");
}

pub fn set_user_and_hostname_for_commands(user: String, hostname: String) {
    let mut conn = init_db().expect("Should be able to establish sqlite connection.");

    // Update the 'user' and 'host' columns to their real values (based on the machine on which
    // this test is running) for commands that were stored with the placeholder 'local:user' and
    // 'local:host' values.
    //
    // This allows us to use real (rather than mocked out) logic for matching history commands to
    // the appropriate session based on session hostnamebased on system hostname.
    diesel::update(
        schema::commands::dsl::commands.filter(schema::commands::username.eq("local:user")),
    )
    .set((
        schema::commands::username.eq(user),
        schema::commands::hostname.eq(hostname),
    ))
    .execute(&mut conn)
    .expect("Failed to update user and hostname for persisted commands.");
}
