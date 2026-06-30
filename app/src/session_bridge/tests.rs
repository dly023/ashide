use std::path::{Path, PathBuf};

use super::bundle::{
    build_bundle, bundle_output_path, default_bundle_name, read_bundle, safe_filename_component,
    write_bundle, BUNDLE_FORMAT, BUNDLE_VERSION, SESSION_BRIDGE_VERSION,
};
use super::ir::{SessionArtifactIr, SessionIr, SessionMessageIr, SessionTimestamp};
use super::preview::SessionBridgePreview;
use super::sanitize::{clean_text, redact, sanitize_embedded_images};
use super::transform::{edit_session, fork_session, SessionEditSpec};

fn sample_session() -> SessionIr {
    SessionIr {
        source: "ashide".to_owned(),
        session_id: "session/id with spaces".to_owned(),
        title: "Test Session".to_owned(),
        project_path: Some("/tmp/project".to_owned()),
        created_at: Some(SessionTimestamp::String("2026-06-18T00:00:00Z".to_owned())),
        updated_at: Some(SessionTimestamp::String("2026-06-18T00:01:00Z".to_owned())),
        messages: vec![
            SessionMessageIr {
                role: "user".to_owned(),
                text: "hello\n  indented".to_owned(),
                timestamp: Some(SessionTimestamp::String("2026-06-18T00:00:00Z".to_owned())),
            },
            SessionMessageIr {
                role: "assistant".to_owned(),
                text: "token=secret sk-abcdefghijklmnop".to_owned(),
                timestamp: Some(SessionTimestamp::String("2026-06-18T00:00:01Z".to_owned())),
            },
        ],
        artifacts: vec![SessionArtifactIr {
            kind: "artifact".to_owned(),
            text: "image data:image/png;base64,AAAA after".to_owned(),
            path: None,
            metadata: serde_json::Value::Null,
        }],
        metadata: serde_json::json!({ "runId": "run-123" }),
    }
}

#[cfg(feature = "local_fs")]
static NATIVE_HISTORY_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(feature = "local_fs")]
fn with_isolated_native_history_env<T>(home_dir: &Path, f: impl FnOnce() -> T) -> T {
    let _guard = NATIVE_HISTORY_ENV_LOCK
        .lock()
        .expect("native history env lock should not be poisoned");
    let old_codex_home = std::env::var_os("CODEX_HOME");
    let old_claude_home = std::env::var_os("CLAUDE_HOME");
    std::env::set_var("CODEX_HOME", home_dir.join(".codex"));
    std::env::set_var("CLAUDE_HOME", home_dir.join(".claude"));

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));

    match old_codex_home {
        Some(value) => std::env::set_var("CODEX_HOME", value),
        None => std::env::remove_var("CODEX_HOME"),
    }
    match old_claude_home {
        Some(value) => std::env::set_var("CLAUDE_HOME", value),
        None => std::env::remove_var("CLAUDE_HOME"),
    }

    match result {
        Ok(value) => value,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

#[cfg(feature = "local_fs")]
fn read_native_receipt_back(
    agent: crate::terminal::CLIAgent,
    receipt: &super::native_writer::NativeSessionWriteReceipt,
) -> super::cli_agent_reader::CliAgentSessionReadResult {
    let bytes = std::fs::read(&receipt.session_file).unwrap();
    super::cli_agent_reader::parse_cli_agent_session_source_bytes(
        agent,
        receipt.session_id.clone(),
        super::cli_agent_reader::CliAgentSessionSourceBytes {
            reference: receipt.session_file.display().to_string(),
            sha256: "roundtrip-hash".to_owned(),
            bytes,
        },
        None,
        None,
    )
    .unwrap()
}

#[test]
fn safe_filename_component_preserves_allowed_chars_and_collapses_unsafe_runs() {
    assert_eq!(
        safe_filename_component("abc.DEF_123-xyz"),
        "abc.DEF_123-xyz"
    );
    assert_eq!(
        safe_filename_component(" session/id with spaces "),
        "session-id-with-spaces"
    );
    assert_eq!(safe_filename_component("***"), "session");
}

#[test]
fn default_bundle_name_uses_safe_full_session_id() {
    let session = sample_session();
    assert_eq!(
        default_bundle_name(&session),
        "ashide-sessionbridge-export-ashide-session-id-with-spaces.json"
    );
}

#[test]
fn build_bundle_sets_format_version_and_sanitizes_text() {
    let bundle = build_bundle(&sample_session());
    assert_eq!(bundle.format, BUNDLE_FORMAT);
    assert_eq!(bundle.version, BUNDLE_VERSION);
    assert_eq!(bundle.session.session_id, "session/id with spaces");
    assert_eq!(bundle.session.messages[0].text, "hello\n  indented");
    assert!(bundle.session.messages[1].text.contains("token=[REDACTED]"));
    assert!(!bundle.session.messages[1].text.contains("secret"));
    assert!(!bundle.session.messages[1]
        .text
        .contains("sk-abcdefghijklmnop"));
    assert!(bundle.session.artifacts[0]
        .text
        .contains("[Image attachment not imported: embedded PNG data URL"));
    assert!(bundle.session.artifacts[0].text.ends_with(" after"));
}

#[test]
fn redact_leaves_words_merely_ending_in_pass_alone() {
    let text = redact("set compass=north and multipass: ticket for the trip");
    assert!(text.contains("compass=north"));
    assert!(text.contains("multipass: ticket"));

    let redacted = redact("pass=secret password: hunter2 api_key: abc123");
    assert!(redacted.contains("pass=[REDACTED]"));
    assert!(redacted.contains("password: [REDACTED]"));
    assert!(redacted.contains("api_key: [REDACTED]"));
    assert!(!redacted.contains("hunter2"));
    assert!(!redacted.contains("abc123"));
}

#[test]
fn sanitize_embedded_images_replaces_data_url_and_preserves_surrounding_text() {
    let text = "before input_image data:image/png;base64,AAAA after";
    let result = sanitize_embedded_images(text);
    assert!(result.starts_with("before "));
    assert!(result.contains("embedded PNG data URL, approx 3 B"));
    assert!(result.ends_with(" after"));
}

#[test]
fn clean_text_redacts_then_sanitizes() {
    let result = clean_text("api_key: abc123 data:image/jpeg;base64,AAAA");
    assert!(result.contains("api_key: [REDACTED]"));
    assert!(result.contains("embedded JPEG data URL"));
}

#[test]
fn dry_run_preview_reports_counts_without_writing() {
    let session = sample_session();
    let preview = SessionBridgePreview::from_session(
        &session,
        Some(PathBuf::from("/tmp/session.json")),
        vec!["artifact warning".to_owned()],
    );
    assert_eq!(preview.message_count, 2);
    assert_eq!(preview.artifact_count, 1);
    let text = preview.dry_run_text();
    assert!(text.contains("DRY RUN: would export ashide session session/id with spaces"));
    assert!(text.contains("Messages: 2"));
    assert!(text.contains("Artifacts: 1"));
    assert!(text.contains("- artifact warning"));
}

#[test]
fn bundle_output_path_matches_directory_write_target() {
    let tempdir = tempfile::tempdir().unwrap();
    let session = sample_session();

    let output_path = bundle_output_path(&session, Some(tempdir.path())).unwrap();
    let written_path = write_bundle(&session, Some(tempdir.path())).unwrap();

    assert_eq!(written_path, output_path);
    assert!(written_path.exists());
    assert!(written_path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("ashide-sessionbridge-export-ashide-")));
}

#[test]
fn read_bundle_rejects_wrong_format() {
    let tempdir = tempfile::tempdir().unwrap();
    let path = tempdir.path().join("bundle.json");
    let mut bundle = build_bundle(&sample_session());
    bundle.format = "old-sessionbridge-bundle".to_owned();
    std::fs::write(&path, serde_json::to_string(&bundle).unwrap()).unwrap();

    let error = read_bundle(&path).unwrap_err();

    assert!(error
        .to_string()
        .contains("invalid SessionBridge bundle format"));
}

#[test]
fn read_bundle_rejects_wrong_versions() {
    let tempdir = tempfile::tempdir().unwrap();
    let path = tempdir.path().join("bundle.json");
    let mut bundle = build_bundle(&sample_session());
    bundle.version = BUNDLE_VERSION + 1;
    std::fs::write(&path, serde_json::to_string(&bundle).unwrap()).unwrap();

    let error = read_bundle(&path).unwrap_err();

    assert!(error
        .to_string()
        .contains("unsupported SessionBridge bundle version"));

    bundle.version = BUNDLE_VERSION;
    bundle.session_bridge_version = SESSION_BRIDGE_VERSION + 1;
    std::fs::write(&path, serde_json::to_string(&bundle).unwrap()).unwrap();

    let error = read_bundle(&path).unwrap_err();

    assert!(error
        .to_string()
        .contains("unsupported SessionBridge runtime version"));
}

#[test]
fn live_ashide_conversation_to_session_ir_exports_in_memory_exchange() {
    use std::collections::HashMap;

    use crate::ai::agent::conversation::{AIConversation, AIConversationId};
    use warp_multi_agent_api as api;

    let task = api::Task {
        id: "root-task".to_string(),
        description: "Live title".to_string(),
        dependencies: None,
        messages: vec![
            api::Message {
                id: "user-1".to_string(),
                task_id: "root-task".to_string(),
                request_id: "request-1".to_string(),
                timestamp: None,
                server_message_data: String::new(),
                citations: vec![],
                message: Some(api::message::Message::UserQuery(api::message::UserQuery {
                    query: "live user prompt".to_string(),
                    context: None,
                    referenced_attachments: HashMap::new(),
                    mode: None,
                    intended_agent: Default::default(),
                })),
            },
            api::Message {
                id: "assistant-1".to_string(),
                task_id: "root-task".to_string(),
                request_id: "request-1".to_string(),
                timestamp: None,
                server_message_data: String::new(),
                citations: vec![],
                message: Some(api::message::Message::AgentOutput(
                    api::message::AgentOutput {
                        text: "live assistant response".to_string(),
                    },
                )),
            },
        ],
        summary: String::new(),
        server_data: String::new(),
    };
    let conversation = AIConversation::new_restored(AIConversationId::new(), vec![task], None)
        .expect("test conversation should restore from in-memory task data");

    let read_result = super::ashide_store::live_ashide_conversation_to_session_ir(&conversation);

    assert_eq!(read_result.session.title, "Live title");
    assert_eq!(read_result.session.messages.len(), 2);
    assert_eq!(read_result.session.messages[0].role, "user");
    assert_eq!(read_result.session.messages[0].text, "live user prompt");
    assert_eq!(read_result.session.messages[1].role, "assistant");
    assert_eq!(
        read_result.session.messages[1].text,
        "live assistant response"
    );
    assert!(read_result.warnings.is_empty());
}

#[cfg(feature = "local_fs")]
#[test]
fn adapter_registry_keeps_pi_blocked_until_native_history_contract_exists() {
    use crate::terminal::CLIAgent;

    use super::adapter_registry::{
        session_bridge_adapter_for_agent, session_bridge_fork_targets, SessionBridgeForkTarget,
    };

    assert!(session_bridge_adapter_for_agent(CLIAgent::Codex)
        .is_some_and(|adapter| adapter.capabilities.can_read_cli_history));
    assert!(session_bridge_adapter_for_agent(CLIAgent::Claude)
        .is_some_and(|adapter| adapter.capabilities.can_read_cli_history));

    assert!(
        session_bridge_adapter_for_agent(CLIAgent::Pi).is_none(),
        "Pi must not be exposed as SessionBridge-capable until a stable native history/read/write contract exists"
    );
    assert!(
        !session_bridge_fork_targets()
            .any(|target| target == SessionBridgeForkTarget::Agent(CLIAgent::Pi)),
        "SessionBridge fork targets must come from registered adapters, not CLIAgent identity metadata"
    );
}

#[cfg(feature = "local_fs")]
#[test]
fn claude_native_writer_reader_round_trip_preserves_forked_session() {
    use crate::terminal::CLIAgent;

    use super::adapter_registry::SessionBridgeForkTarget;

    let tempdir = tempfile::tempdir().unwrap();
    with_isolated_native_history_env(tempdir.path(), || {
        let fork = fork_session(&sample_session(), Some(uuid::Uuid::new_v4().to_string()));

        let receipt = super::native_writer::write_native_session_to_home(
            &fork.session,
            SessionBridgeForkTarget::Agent(CLIAgent::Claude),
            tempdir.path(),
        )
        .unwrap();

        assert!(receipt
            .session_file
            .starts_with(tempdir.path().join(".claude")));
        let read_result = read_native_receipt_back(CLIAgent::Claude, &receipt);

        assert_eq!(read_result.session.source, "claude");
        assert_eq!(read_result.session.session_id, receipt.session_id);
        assert_eq!(read_result.session.title, fork.session.title);
        assert_eq!(read_result.session.project_path, fork.session.project_path);
        assert_eq!(read_result.session.messages, fork.session.messages);
        assert_eq!(read_result.source.source_session_id, receipt.session_id);
        assert_eq!(
            read_result.source.reference,
            receipt.session_file.display().to_string()
        );
    });
}

#[cfg(feature = "local_fs")]
#[test]
fn codex_native_writer_reader_round_trip_preserves_forked_session() {
    use crate::terminal::CLIAgent;

    use super::adapter_registry::SessionBridgeForkTarget;

    let tempdir = tempfile::tempdir().unwrap();
    with_isolated_native_history_env(tempdir.path(), || {
        let fork = fork_session(&sample_session(), Some(uuid::Uuid::new_v4().to_string()));

        let receipt = super::native_writer::write_native_session_to_home(
            &fork.session,
            SessionBridgeForkTarget::Agent(CLIAgent::Codex),
            tempdir.path(),
        )
        .unwrap();

        assert!(receipt
            .session_file
            .starts_with(tempdir.path().join(".codex")));
        assert!(receipt
            .session_file
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(&format!("{}.jsonl", receipt.session_id))));
        let read_result = read_native_receipt_back(CLIAgent::Codex, &receipt);

        assert_eq!(read_result.session.source, "codex");
        assert_eq!(read_result.session.session_id, receipt.session_id);
        assert_eq!(read_result.session.title, fork.session.title);
        assert_eq!(read_result.session.project_path, fork.session.project_path);
        assert_eq!(read_result.session.messages, fork.session.messages);
        assert_eq!(read_result.source.source_session_id, receipt.session_id);
        assert_eq!(
            read_result.source.reference,
            receipt.session_file.display().to_string()
        );
    });
}

#[cfg(feature = "local_fs")]
#[test]
fn native_writer_registers_forked_session_for_listing_in_history_and_index() {
    use crate::terminal::CLIAgent;

    use super::adapter_registry::SessionBridgeForkTarget;

    // The round-trip tests above cover "resume" (our reader reads the transcript
    // back). This covers the other half — "list": the forked session must be
    // appended to the tool's discovery file (Claude `history.jsonl` / Codex
    // `session_index.jsonl`) so the real CLI surfaces it. It also pins the
    // discovery-row fields the reader never consumes, so a future field cleanup
    // can't silently drop what makes the session discoverable (ZAP-H2 caution).
    fn jsonl_has_line(path: &Path, pred: impl Fn(&serde_json::Value) -> bool) -> bool {
        let Ok(text) = std::fs::read_to_string(path) else {
            return false;
        };
        text.lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line.trim()).ok())
            .any(|value| pred(&value))
    }

    let tempdir = tempfile::tempdir().unwrap();
    with_isolated_native_history_env(tempdir.path(), || {
        let claude_fork = fork_session(&sample_session(), Some(uuid::Uuid::new_v4().to_string()));
        let claude_receipt = super::native_writer::write_native_session_to_home(
            &claude_fork.session,
            SessionBridgeForkTarget::Agent(CLIAgent::Claude),
            tempdir.path(),
        )
        .unwrap();
        let claude_history = tempdir.path().join(".claude").join("history.jsonl");
        assert!(
            jsonl_has_line(&claude_history, |row| {
                row["sessionId"] == serde_json::json!(claude_receipt.session_id)
                    && row.get("display").is_some()
            }),
            "claude history.jsonl must list the forked session by sessionId (with a display title)"
        );

        let codex_fork = fork_session(&sample_session(), Some(uuid::Uuid::new_v4().to_string()));
        let codex_receipt = super::native_writer::write_native_session_to_home(
            &codex_fork.session,
            SessionBridgeForkTarget::Agent(CLIAgent::Codex),
            tempdir.path(),
        )
        .unwrap();
        let codex_index = tempdir.path().join(".codex").join("session_index.jsonl");
        assert!(
            jsonl_has_line(&codex_index, |row| {
                row["id"] == serde_json::json!(codex_receipt.session_id)
                    && row.get("thread_name").is_some()
            }),
            "codex session_index.jsonl must list the forked session by id (with a thread_name)"
        );
    });
}

#[cfg(feature = "local_fs")]
#[test]
fn edit_fork_round_trips_through_native_history_without_mutating_source() {
    use crate::terminal::CLIAgent;

    use super::adapter_registry::SessionBridgeForkTarget;

    let tempdir = tempfile::tempdir().unwrap();
    with_isolated_native_history_env(tempdir.path(), || {
        let source = sample_session();
        let edited = edit_session(
            &source,
            SessionEditSpec {
                redactions: vec!["hello".to_owned(), "secret".to_owned()],
                trim_after: Some(1),
            },
            Some(uuid::Uuid::new_v4().to_string()),
        )
        .unwrap();

        let receipt = super::native_writer::write_native_session_to_home(
            &edited.session,
            SessionBridgeForkTarget::Agent(CLIAgent::Codex),
            tempdir.path(),
        )
        .unwrap();
        let read_result = read_native_receipt_back(CLIAgent::Codex, &receipt);

        assert_eq!(source.messages.len(), 2);
        assert_eq!(source.messages[0].text, "hello\n  indented");
        assert!(source.messages[1].text.contains("secret"));
        assert_eq!(read_result.session.title, "Test Session (edited)");
        assert_eq!(read_result.session.messages.len(), 1);
        assert_eq!(
            read_result.session.messages[0].text,
            "[REDACTED_BY_SESSION_BRIDGE]\n  indented"
        );
        assert!(!read_result.session.messages[0].text.contains("hello"));
        assert!(!read_result.session.messages[0].text.contains("secret"));
        assert_eq!(
            read_result.session.metadata["sessionBridge"]["providerSessionId"],
            receipt.session_id
        );
    });
}

#[cfg(feature = "local_fs")]
#[test]
fn native_writer_rejects_unregistered_session_bridge_target() {
    use crate::terminal::CLIAgent;

    use super::adapter_registry::SessionBridgeForkTarget;

    let tempdir = tempfile::tempdir().unwrap();
    let error = super::native_writer::write_native_session_to_home(
        &sample_session(),
        SessionBridgeForkTarget::Agent(CLIAgent::Pi),
        tempdir.path(),
    )
    .unwrap_err();

    assert!(error
        .to_string()
        .contains("has no registered SessionBridge adapter"));
    assert!(
        !tempdir
            .path()
            .join(".agents/session-bridge/backups")
            .exists(),
        "unsupported targets must fail before creating backup or fake native history state"
    );
}

#[cfg(feature = "local_fs")]
#[test]
fn remote_native_write_plan_targets_supplied_home_root_without_python() {
    use crate::terminal::CLIAgent;

    use super::adapter_registry::SessionBridgeForkTarget;
    use super::native_writer::NativeSessionWriteOperation;

    let session = sample_session();
    let claude_plan = super::native_writer::plan_native_session_write_for_home_root(
        &session,
        SessionBridgeForkTarget::Agent(CLIAgent::Claude),
        "/home/remote-user",
    )
    .unwrap();
    assert_eq!(
        claude_plan.receipt.target,
        SessionBridgeForkTarget::Agent(CLIAgent::Claude)
    );
    assert!(claude_plan
        .receipt
        .session_file
        .starts_with("/home/remote-user/.claude/projects/"));
    assert!(claude_plan.operations.iter().any(|operation| matches!(
        operation,
        NativeSessionWriteOperation::Append { path, .. }
            if path == "/home/remote-user/.claude/history.jsonl"
    )));

    let codex_plan = super::native_writer::plan_native_session_write_for_home_root(
        &session,
        SessionBridgeForkTarget::Agent(CLIAgent::Codex),
        r"C:\Users\remote-user",
    )
    .unwrap();
    assert_eq!(
        codex_plan.receipt.target,
        SessionBridgeForkTarget::Agent(CLIAgent::Codex)
    );
    assert!(
        codex_plan
            .receipt
            .session_file
            .starts_with(r"C:\Users\remote-user\.codex\sessions\"),
        "remote plan must preserve Windows-style remote home roots"
    );
    assert!(codex_plan.operations.iter().any(|operation| matches!(
        operation,
        NativeSessionWriteOperation::Append { path, .. }
            if path == r"C:\Users\remote-user\.codex\session_index.jsonl"
    )));
}

#[cfg(feature = "local_fs")]
#[test]
fn import_bundle_writes_native_session_and_dry_run_stays_read_only() {
    use diesel::connection::SimpleConnection;
    use diesel::Connection;
    use diesel_migrations::MigrationHarness;
    use uuid::Uuid;

    let tempdir = tempfile::tempdir().unwrap();
    let database_path = tempdir.path().join("ashide.sqlite");
    let mut conn = diesel::SqliteConnection::establish(database_path.to_str().unwrap()).unwrap();
    conn.batch_execute("PRAGMA foreign_keys = ON;").unwrap();
    conn.run_pending_migrations(::persistence::MIGRATIONS)
        .unwrap();

    let mut session = sample_session();
    session.session_id = Uuid::new_v4().to_string();
    session.title = "Imported Session".to_owned();
    let bundle_path = write_bundle(&session, Some(tempdir.path())).unwrap();
    let bundle = read_bundle(&bundle_path).unwrap();

    let dry_run_plan =
        super::ashide_store::preview_ashide_session_import(&mut conn, &bundle, &bundle_path, None)
            .unwrap();
    assert_eq!(dry_run_plan.target_session_id, session.session_id);
    assert_eq!(dry_run_plan.project_path, Some("/tmp/project".to_owned()));
    assert!(super::ashide_store::list_ashide_sessions(&mut conn)
        .unwrap()
        .is_empty());

    let plan =
        super::ashide_store::import_ashide_session_bundle(&mut conn, &bundle, &bundle_path, None)
            .unwrap();

    assert_eq!(plan.source_session_id, session.session_id);
    assert_eq!(plan.target_session_id, session.session_id);
    assert_eq!(plan.message_count, 2);
    assert_eq!(plan.artifact_count, 1);
    assert_eq!(
        plan.source_reference,
        bundle_path.canonicalize().unwrap().display().to_string()
    );

    let read_result =
        super::ashide_store::read_ashide_session_by_id(&mut conn, &plan.target_session_id).unwrap();
    assert_eq!(read_result.session.title, "Imported Session");
    assert_eq!(
        read_result.session.project_path,
        Some("/tmp/project".to_owned())
    );
    assert_eq!(read_result.session.messages.len(), 2);
    assert_eq!(read_result.session.artifacts.len(), 1);
    assert_eq!(
        read_result.session.metadata["sessionBridgeImport"]["sourceSessionId"],
        session.session_id
    );
    assert_eq!(
        read_result.session.metadata["sessionBridgeImport"]["sourceReference"],
        bundle_path.canonicalize().unwrap().display().to_string()
    );

    let error =
        super::ashide_store::import_ashide_session_bundle(&mut conn, &bundle, &bundle_path, None)
            .unwrap_err();
    assert!(error.to_string().contains("conversation already exists"));
}

#[cfg(feature = "local_fs")]
#[test]
fn derivation_write_back_writes_native_session_and_preserves_original_source_provenance() {
    use diesel::connection::SimpleConnection;
    use diesel::Connection;
    use diesel_migrations::MigrationHarness;
    use uuid::Uuid;

    let tempdir = tempfile::tempdir().unwrap();
    let database_path = tempdir.path().join("ashide.sqlite");
    let mut conn = diesel::SqliteConnection::establish(database_path.to_str().unwrap()).unwrap();
    conn.batch_execute("PRAGMA foreign_keys = ON;").unwrap();
    conn.run_pending_migrations(::persistence::MIGRATIONS)
        .unwrap();

    let mut source_session = sample_session();
    source_session.session_id = Uuid::new_v4().to_string();
    source_session.title = "Source Session".to_owned();
    let target_session_id = Uuid::new_v4().to_string();
    let derivation = edit_session(
        &source_session,
        SessionEditSpec {
            redactions: vec!["secret".to_owned()],
            trim_after: Some(2),
        },
        Some(target_session_id.clone()),
    )
    .unwrap();
    let import_source = super::ashide_store::SessionBridgeImportSource::from_derived_session(
        &derivation.receipt.operation,
        &derivation.receipt.source_session_id,
        &derivation.receipt.derived_session_id,
        &derivation.session,
    )
    .unwrap();

    let dry_run_plan = super::ashide_store::preview_ashide_session_write_back(
        &mut conn,
        &derivation.session,
        import_source.clone(),
    )
    .unwrap();
    assert_eq!(dry_run_plan.source_session_id, source_session.session_id);
    assert_eq!(dry_run_plan.target_session_id, target_session_id);
    assert!(dry_run_plan
        .source_reference
        .starts_with("session-bridge://derived/edit/"));
    assert_eq!(dry_run_plan.source_sha256.len(), 64);
    assert!(super::ashide_store::list_ashide_sessions(&mut conn)
        .unwrap()
        .is_empty());

    let plan = super::ashide_store::import_ashide_session_write_back(
        &mut conn,
        &derivation.session,
        import_source,
    )
    .unwrap();
    assert_eq!(plan.source_session_id, source_session.session_id);
    assert_eq!(plan.target_session_id, target_session_id);

    let read_result =
        super::ashide_store::read_ashide_session_by_id(&mut conn, &target_session_id).unwrap();
    assert_eq!(read_result.session.title, "Source Session (edited)");
    assert!(read_result.session.messages[1]
        .text
        .contains("[REDACTED_BY_SESSION_BRIDGE]"));
    assert!(!read_result.session.messages[1].text.contains("secret"));
    assert_eq!(
        read_result.session.metadata["sessionBridge"]["operation"],
        "edit"
    );
    assert_eq!(
        read_result.session.metadata["sessionBridgeImport"]["sourceSessionId"],
        source_session.session_id
    );
    assert_eq!(
        read_result.session.metadata["sessionBridgeImport"]["sourceSha256"]
            .as_str()
            .unwrap()
            .len(),
        64
    );

    let duplicate_source = super::ashide_store::SessionBridgeImportSource::from_derived_session(
        &derivation.receipt.operation,
        &derivation.receipt.source_session_id,
        &derivation.receipt.derived_session_id,
        &derivation.session,
    )
    .unwrap();
    let error = super::ashide_store::import_ashide_session_write_back(
        &mut conn,
        &derivation.session,
        duplicate_source,
    )
    .unwrap_err();
    assert!(error.to_string().contains("conversation already exists"));
}

#[cfg(feature = "local_fs")]
#[test]
fn import_bundle_rejects_non_uuid_native_session_id() {
    use diesel::connection::SimpleConnection;
    use diesel::Connection;
    use diesel_migrations::MigrationHarness;

    let tempdir = tempfile::tempdir().unwrap();
    let database_path = tempdir.path().join("ashide.sqlite");
    let mut conn = diesel::SqliteConnection::establish(database_path.to_str().unwrap()).unwrap();
    conn.batch_execute("PRAGMA foreign_keys = ON;").unwrap();
    conn.run_pending_migrations(::persistence::MIGRATIONS)
        .unwrap();

    let mut session = sample_session();
    session.session_id = "not-a-native-uuid".to_owned();
    let bundle_path = write_bundle(&session, Some(tempdir.path())).unwrap();
    let bundle = read_bundle(&bundle_path).unwrap();

    let error =
        super::ashide_store::preview_ashide_session_import(&mut conn, &bundle, &bundle_path, None)
            .unwrap_err();
    assert!(error
        .to_string()
        .contains("native imported sessions require a UUID id"));

    let error = super::ashide_store::preview_ashide_session_import(
        &mut conn,
        &bundle,
        &bundle_path,
        Some("also-not-a-native-uuid".to_owned()),
    )
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("native imported sessions require a UUID id"));
}

#[test]
fn read_native_session_rejects_non_uuid_conversation_id() {
    use crate::persistence::model::{AgentConversation, AgentConversationRecord};

    let persisted = AgentConversation {
        conversation: AgentConversationRecord {
            conversation_id: "not-a-native-uuid".to_owned(),
            conversation_data: "{}".to_owned(),
            ..Default::default()
        },
        tasks: Vec::new(),
    };

    let error = super::ashide_store::agent_conversation_to_session_ir(persisted).unwrap_err();
    assert!(error
        .to_string()
        .contains("native sessions require a UUID id"));
}

#[test]
fn fork_session_creates_derived_id_and_preserves_parent_provenance() {
    let session = sample_session();
    let fork = fork_session(&session, Some("fork-session".to_owned()));

    assert_eq!(session.session_id, "session/id with spaces");
    assert_eq!(fork.session.session_id, "fork-session");
    assert_eq!(fork.session.messages, session.messages);
    assert_eq!(fork.receipt.operation, "fork");
    assert_eq!(fork.receipt.source_session_id, "session/id with spaces");
    assert_eq!(fork.receipt.derived_session_id, "fork-session");

    let metadata = fork
        .session
        .metadata
        .get("sessionBridge")
        .expect("derived metadata should include a sessionBridge receipt");
    assert_eq!(metadata["operation"], "fork");
    assert_eq!(metadata["sourceSessionId"], "session/id with spaces");
    assert_eq!(
        metadata["operationMetadata"]["forkedFromSessionId"],
        "session/id with spaces"
    );
}

#[test]
fn edit_session_redacts_and_trims_without_mutating_source() {
    let session = sample_session();
    let edited = edit_session(
        &session,
        SessionEditSpec {
            redactions: vec!["hello".to_owned(), "secret".to_owned()],
            trim_after: Some(1),
        },
        Some("edited-session".to_owned()),
    )
    .unwrap();

    assert_eq!(session.messages[0].text, "hello\n  indented");
    assert_eq!(edited.session.session_id, "edited-session");
    assert_eq!(edited.session.messages.len(), 1);
    assert_eq!(
        edited.session.messages[0].text,
        "[REDACTED_BY_SESSION_BRIDGE]\n  indented"
    );
    assert_eq!(edited.receipt.operation, "edit");
    assert_eq!(edited.receipt.trimmed_message_count, 1);
    assert_eq!(edited.receipt.redaction_replacement_count, 1);
    assert_eq!(
        edited.session.metadata["sessionBridge"]["operationMetadata"]["trimmedMessageCount"],
        1
    );
}

#[test]
fn edit_session_messages_replaces_message_draft_without_mutating_source() {
    let session = sample_session();
    let mut edited_messages = session.messages.clone();
    edited_messages.remove(1);
    edited_messages[0].text = "edited user prompt".to_owned();

    let edited = super::transform::edit_session_messages(
        &session,
        edited_messages,
        Some("edited-session".to_owned()),
    )
    .unwrap();

    assert_eq!(session.messages.len(), 2);
    assert_eq!(session.messages[0].text, "hello\n  indented");
    assert!(session.messages[1].text.contains("secret"));
    assert_eq!(edited.session.session_id, "edited-session");
    assert_eq!(edited.session.title, "Test Session (edited)");
    assert_eq!(edited.session.messages.len(), 1);
    assert_eq!(edited.session.messages[0].text, "edited user prompt");
    assert_eq!(
        edited.session.messages[0].timestamp,
        session.messages[0].timestamp
    );
    assert_eq!(edited.receipt.operation, "edit");
    assert_eq!(edited.receipt.original_message_count, 2);
    assert_eq!(edited.receipt.message_count, 1);
    assert_eq!(edited.receipt.trimmed_message_count, 1);
    assert_eq!(edited.receipt.redaction_replacement_count, 0);
    assert_eq!(
        edited.session.metadata["sessionBridge"]["operationMetadata"]["messageEditor"],
        true
    );
}

#[test]
fn edit_session_requires_a_real_operation() {
    let error = edit_session(
        &sample_session(),
        SessionEditSpec {
            redactions: vec!["".to_owned()],
            trim_after: None,
        },
        Some("edited-session".to_owned()),
    )
    .unwrap_err();

    assert!(error.to_string().contains("provide at least one --redact"));
}
