#[cfg(feature = "local_fs")]
use std::collections::HashSet;
#[cfg(feature = "local_fs")]
use std::fs;
#[cfg(feature = "local_fs")]
use std::path::PathBuf;
#[cfg(feature = "local_fs")]
use std::time::UNIX_EPOCH;

#[cfg(feature = "local_fs")]
use crate::terminal::CLIAgent;

use serde_json::Value;

use super::*;

#[cfg(feature = "local_fs")]
use super::discovery::{
    parse_codex_discovery_index, recent_jcode_session_files, recent_omp_session_files,
    scan_agent_session_provider,
};

#[cfg(feature = "local_fs")]
struct TestSessionSource {
    agent: &'static str,
    session_id: &'static str,
    physical_source: &'static str,
    modified_epoch_millis: i64,
}

#[cfg(feature = "local_fs")]
impl CliAgentSessionSource for TestSessionSource {
    fn agent_key(&self) -> String {
        self.agent.to_owned()
    }

    fn provider_session_id(&self) -> &str {
        self.session_id
    }

    fn physical_source_key(&self) -> String {
        self.physical_source.to_owned()
    }

    fn modified_epoch_millis(&self) -> i64 {
        self.modified_epoch_millis
    }
}

#[cfg(feature = "local_fs")]
#[test]
fn discovery_plan_distinguishes_source_missing_from_successful_empty_store() {
    let home = tempfile::tempdir().expect("create discovery home");
    let roots = CliAgentStoreRoots::for_home(home.path().to_path_buf());
    let plan = AgentSessionDiscoveryPlan::for_test(vec![AgentSessionDiscoveryProvider::Jcode], 40);

    assert!(matches!(
        plan.execute(&roots, &HashSet::new()).transition(),
        AgentSessionDiscoveryTransition::Replace { records, .. } if records.is_empty()
    ));

    assert!(matches!(
        plan.execute(
            &roots,
            &HashSet::from([AgentSessionDiscoveryProvider::Jcode]),
        )
        .transition(),
        AgentSessionDiscoveryTransition::PreserveSourceMissing(
            AgentSessionDiscoveryProvider::Jcode
        )
    ));

    fs::create_dir_all(roots.jcode_sessions()).expect("provision observed Jcode store");
    assert!(matches!(
        plan.execute(
            &roots,
            &HashSet::from([AgentSessionDiscoveryProvider::Jcode]),
        )
        .transition(),
        AgentSessionDiscoveryTransition::Replace { records, .. } if records.is_empty()
    ));
}

#[cfg(feature = "local_fs")]
#[test]
fn discovery_plan_uses_only_enabled_unique_file_backed_agents() {
    let plan = AgentSessionDiscoveryPlan::from_enabled_agents(
        [
            CLIAgent::Jcode,
            CLIAgent::Unknown,
            CLIAgent::Omp,
            CLIAgent::Jcode,
            CLIAgent::Claude,
        ],
        40,
    );

    assert_eq!(
        plan.providers(),
        [
            AgentSessionDiscoveryProvider::Jcode,
            AgentSessionDiscoveryProvider::Omp,
            AgentSessionDiscoveryProvider::Claude,
        ],
        "discovery selection must be an ordered, duplicate-free projection of the setting",
    );
}

#[cfg(feature = "local_fs")]
#[test]
fn discovery_plan_rejects_partial_collection_on_scan_failure() {
    let home = tempfile::tempdir().expect("create scan home");
    fs::create_dir_all(home.path().join(".claude/projects")).expect("create Claude store");
    fs::write(
        home.path().join(".claude/projects/observed.jsonl"),
        serde_json::json!({"sessionId": "target"}).to_string(),
    )
    .expect("write valid target");
    fs::create_dir_all(home.path().join(".codex/sessions")).expect("create Codex store");
    fs::create_dir_all(home.path().join(".codex/session_index.jsonl"))
        .expect("make index path invalid");

    assert!(matches!(
        AgentSessionDiscoveryPlan::for_test(
            vec![
                AgentSessionDiscoveryProvider::Claude,
                AgentSessionDiscoveryProvider::Codex,
            ],
            40,
        )
        .execute(
            &CliAgentStoreRoots::for_home(home.path().to_path_buf()),
            &HashSet::new(),
        )
        .transition(),
        AgentSessionDiscoveryTransition::PreserveFailed(_)
    ));
}

#[cfg(feature = "local_fs")]
#[test]
fn discovery_cancel_has_explicit_preserve_transition() {
    assert!(matches!(
        AgentSessionDiscoveryResult::Cancelled.transition(),
        AgentSessionDiscoveryTransition::PreserveCancelled
    ));
}

#[cfg(feature = "local_fs")]
fn discovery_record(
    agent: CLIAgent,
    provider_session_id: &str,
    modified_epoch_millis: i64,
) -> AgentSessionDiscoveryRecord {
    AgentSessionDiscoveryRecord {
        agent,
        provider_session_id: provider_session_id.to_owned(),
        source: AgentSessionDiscoverySource::Transcript(PathBuf::from(format!(
            "/fixtures/{provider_session_id}.jsonl"
        ))),
        label: None,
        cwd: None,
        modified_epoch_millis,
    }
}

#[cfg(feature = "local_fs")]
#[test]
fn provider_outcomes_preserve_identity_and_order_until_explicit_permanent_deletion() {
    let current = vec![
        discovery_record(CLIAgent::Claude, "unrelated-claude-a", 30),
        discovery_record(CLIAgent::Codex, "target-codex", 20),
        discovery_record(CLIAgent::Claude, "unrelated-claude-b", 10),
    ];
    let identities = |records: &[AgentSessionDiscoveryRecord]| {
        records
            .iter()
            .map(|record| (record.agent, record.provider_session_id.clone()))
            .collect::<Vec<_>>()
    };
    let _expected = identities(&current);

    let missing = AgentSessionDiscoveryResult::SourceMissing(AgentSessionDiscoveryProvider::Codex)
        .transition();
    let cancelled = AgentSessionDiscoveryResult::Cancelled.transition();
    let deleted =
        AgentSessionDiscoveryResult::PermanentlyDeleted(AgentSessionDiscoveryProvider::Codex)
            .transition();

    assert!(matches!(
        missing,
        AgentSessionDiscoveryTransition::PreserveSourceMissing(_)
    ));
    assert!(matches!(
        cancelled,
        AgentSessionDiscoveryTransition::PreserveCancelled
    ));
    assert!(matches!(
        deleted,
        AgentSessionDiscoveryTransition::RemoveProvider(_)
    ));
}

#[test]
fn parse_jsonl_skips_blank_and_unparseable_lines() {
    let text = "\n{\"a\":1}\nnot json\n  {\"b\":2}  \n";
    let values = parse_jsonl_values(text, None);
    assert_eq!(values.len(), 2);
    assert_eq!(values[0]["a"], 1);
    assert_eq!(values[1]["b"], 2);
}

#[test]
fn parse_jsonl_limit_counts_physical_lines() {
    // Blank lines count toward the physical-line limit (matches `.lines().take`).
    let text = "\n{\"a\":1}\n{\"b\":2}\n{\"c\":3}\n";
    let values = parse_jsonl_values(text, Some(2));
    // Lines 1 (blank) and 2 ({"a":1}) are consumed → only one value.
    assert_eq!(values.len(), 1);
    assert_eq!(values[0]["a"], 1);
}

#[test]
fn nested_string_walks_objects_and_rejects_blank() {
    let value: Value = serde_json::json!({"payload": {"id": "abc", "blank": "  "}});
    assert_eq!(nested_string(&value, &["payload", "id"]), Some("abc"));
    assert_eq!(nested_string(&value, &["payload", "blank"]), None);
    assert_eq!(nested_string(&value, &["payload", "missing"]), None);
}

#[test]
fn shared_codex_metadata_ignores_injected_user_content() {
    let provider_session_id = "019f5f34-b6b7-70b3-8e50-e98504691ca7";
    let values = vec![
        serde_json::json!({
            "type": "session_meta",
            "payload": {"id": provider_session_id, "cwd": "/repo"}
        }),
        serde_json::json!({
            "type": "response_item",
            "payload": {"type": "message", "role": "user", "content": [
                {"type": "input_text", "text": "# AGENTS.md instructions"}
            ]}
        }),
        serde_json::json!({
            "type": "event_msg",
            "payload": {"type": "user_message", "message": "修复本地标题\n不要显示 Codex"}
        }),
    ];

    let metadata = codex_session_metadata(&values);
    assert_eq!(metadata.session_id.as_deref(), Some(provider_session_id));
    assert_eq!(metadata.cwd.as_deref(), Some("/repo"));
    assert_eq!(metadata.title, None);
    assert_eq!(metadata.display_title().as_deref(), Some("修复本地标题"));
}

#[test]
fn shared_codex_metadata_never_promotes_response_item_message_id() {
    let values = vec![
        serde_json::json!({
            "type": "session_meta",
            "payload": {
                "id": "019f5f34-b6b7-70b3-8e50-e98504691ca7",
                "cwd": "/Users/admin/manga_data"
            }
        }),
        serde_json::json!({
            "type": "response_item",
            "payload": {
                "type": "message",
                "id": "msg_06435be93b11cbcc016a55deda46808197a7e8894330ebe948",
                "role": "assistant",
                "content": []
            }
        }),
        serde_json::json!({
            "type": "response_item",
            "payload": {
                "type": "function_call",
                "id": "fc_0cbbc521aa68e6da016a57017553e081909e7762be87374e44"
            }
        }),
    ];

    let metadata = codex_session_metadata(&values);
    assert_eq!(
        metadata.session_id.as_deref(),
        Some("019f5f34-b6b7-70b3-8e50-e98504691ca7")
    );
}

#[test]
fn canonical_codex_session_id_rejects_runtime_object_ids() {
    let provider_session_id = "019f5f34-b6b7-70b3-8e50-e98504691ca7";
    assert_eq!(
        canonical_codex_session_id(provider_session_id).as_deref(),
        Some(provider_session_id)
    );
    assert_eq!(
        canonical_codex_session_id(&format!(
            "rollout-2026-07-14T13-58-38-{provider_session_id}"
        ))
        .as_deref(),
        Some(provider_session_id)
    );
    assert_eq!(
        canonical_codex_session_id("msg_06435be93b11cbcc016a55deda46808197a7e8894330ebe948"),
        None
    );
    assert_eq!(
        canonical_codex_session_id("fc_0cbbc521aa68e6da016a57017553e081909e7762be87374e44"),
        None
    );
}

#[test]
fn shared_provider_title_wins_over_first_user_message() {
    let values = vec![
        serde_json::json!({"thread_name": "正式标题"}),
        serde_json::json!({
            "type": "event_msg",
            "payload": {"type": "user_message", "message": "首条消息"}
        }),
    ];

    let metadata = codex_session_metadata(&values);
    assert_eq!(metadata.display_title().as_deref(), Some("正式标题"));
}

#[test]
fn codex_session_index_record_parser_is_shared() {
    let provider_session_id = "019f5f34-b6b7-70b3-8e50-e98504691ca7";
    let record = codex_session_index_record(&serde_json::json!({
        "session_id": provider_session_id,
        "thread_name": "共享 Index 标题",
        "cwd": "~/project",
        "updated_at": "2026-07-12T08:00:00Z",
    }))
    .expect("shared index record");

    assert_eq!(record.session_id, provider_session_id);
    assert_eq!(record.title.as_deref(), Some("共享 Index 标题"));
    assert_eq!(record.cwd.as_deref(), Some("~/project"));
    assert_eq!(record.updated_at_epoch_millis, Some(1_783_843_200_000));

    let camel_case = codex_session_index_record(&serde_json::json!({
        "sessionId": "019f5629-5daf-7381-b33e-00d8efba617f",
        "updated_at_unix_ms": 1234,
    }))
    .expect("camel-case session id fallback");
    assert_eq!(
        camel_case.session_id,
        "019f5629-5daf-7381-b33e-00d8efba617f"
    );
    assert_eq!(camel_case.updated_at_epoch_millis, Some(1234));

    assert!(codex_session_index_record(&serde_json::json!({
        "id": "msg_06435be93b11cbcc016a55deda46808197a7e8894330ebe948"
    }))
    .is_none());
}

#[cfg(feature = "local_fs")]
#[test]
fn discovery_candidate_gate_rejects_partial_jsonl_scan() {
    let root = tempfile::tempdir().expect("create JSONL discovery root");
    fs::write(root.path().join("first.jsonl"), "{}\n").expect("write first JSONL session");
    fs::write(root.path().join("second.jsonl"), "{}\n").expect("write second JSONL session");

    let error = recent_jsonl_files(root.path(), 10, Some(1))
        .expect_err("physical candidate gate must reject a partial scan");

    assert_eq!(error.operation(), "扫描 CLI-agent 会话候选项");
    assert!(error.message().contains("安全上限 1"));
}

#[cfg(feature = "local_fs")]
#[test]
fn recent_jsonl_scan_error_is_not_silently_dropped() {
    let root = tempfile::tempdir().expect("create JSONL failure root");
    fs::write(root.path().join("first.jsonl"), "{}\n").expect("write first JSONL session");
    fs::write(root.path().join("second.jsonl"), "{}\n").expect("write second JSONL session");

    assert!(
        recent_jsonl_files(root.path(), 10, Some(1)).is_err(),
        "an over-limit scan must fail instead of returning the first directory-order subset"
    );
}

#[cfg(feature = "local_fs")]
#[test]
fn discovery_candidate_gate_rejects_jcode_and_omp_across_buckets() {
    let home = tempfile::tempdir().expect("create discovery home");
    let jcode_sessions = home.path().join(".jcode/sessions");
    fs::create_dir_all(&jcode_sessions).expect("create Jcode sessions");
    fs::write(jcode_sessions.join("session_first.json"), "{}").expect("write first Jcode session");
    fs::write(jcode_sessions.join("session_second.json"), "{}")
        .expect("write second Jcode session");
    assert!(recent_jcode_session_files(&jcode_sessions, 1).is_err());

    let omp_sessions = home.path().join(".omp/agent/sessions");
    for bucket in ["first", "second"] {
        let bucket = omp_sessions.join(bucket);
        fs::create_dir_all(&bucket).expect("create Omp bucket");
        fs::write(bucket.join("session.jsonl"), "{}\n").expect("write Omp session");
    }
    assert!(recent_omp_session_files(&omp_sessions, 10, 1).is_err());
}

#[cfg(feature = "local_fs")]
#[test]
fn discovery_candidate_gate_rejects_codex_index_before_partial_projection() {
    let home = tempfile::tempdir().expect("create Codex home");
    let roots = CliAgentStoreRoots::for_home(home.path().to_path_buf());
    let index = roots.codex_index();
    fs::create_dir_all(index.parent().expect("Codex index parent"))
        .expect("create Codex index parent");
    fs::write(
        &index,
        concat!(
            "{\"session_id\":\"019f5629-5daf-7381-b33e-00d8efba617f\"}\n",
            "{\"session_id\":\"019f5629-5daf-7381-b33e-00d8efba617e\"}\n"
        ),
    )
    .expect("write Codex index");

    let error = parse_codex_discovery_index(&index, &roots, 1)
        .expect_err("physical index gate must reject a partial projection");

    assert_eq!(error.operation(), "扫描 CLI-agent 会话候选项");
    assert!(error.message().contains("安全上限 1"));
}

#[cfg(feature = "local_fs")]
#[test]
fn cli_agent_home_resolution_never_falls_back_to_filesystem_root() {
    let result = require_cli_agent_home(None);

    assert!(result.is_err(), "unknown home must remain an error");
}

#[cfg(feature = "local_fs")]
#[test]
fn cli_agent_session_cwd_normalization_is_shared() {
    let home = tempfile::tempdir().expect("create temp home");
    let project = home.path().join("project");
    fs::create_dir(&project).expect("create project");
    fs::create_dir_all(home.path().join(".codex/sessions")).expect("create session store");

    assert_eq!(
        normalize_cli_agent_session_cwd(
            Some("~/project"),
            &CliAgentStoreRoots::for_home(home.path().to_path_buf()),
        )
        .as_deref(),
        Some(project.to_string_lossy().as_ref())
    );
    assert_eq!(
        normalize_cli_agent_session_cwd(
            Some("relative/project"),
            &CliAgentStoreRoots::for_home(home.path().to_path_buf()),
        ),
        None
    );
    assert_eq!(
        normalize_cli_agent_session_cwd(
            Some("~/missing"),
            &CliAgentStoreRoots::for_home(home.path().to_path_buf()),
        ),
        None
    );
    assert_eq!(
        normalize_cli_agent_session_cwd(
            Some("~/.codex/sessions"),
            &CliAgentStoreRoots::for_home(home.path().to_path_buf()),
        ),
        None
    );
}

#[cfg(feature = "local_fs")]
#[test]
fn cli_agent_logical_limit_is_shared_across_providers() {
    let limited = limit_cli_agent_session_sources(
        vec![
            TestSessionSource {
                agent: "claude",
                session_id: "claude-new",
                physical_source: "claude-new.jsonl",
                modified_epoch_millis: 400,
            },
            TestSessionSource {
                agent: "codex",
                session_id: "codex-new",
                physical_source: "codex-new-rollout.jsonl",
                modified_epoch_millis: 300,
            },
            TestSessionSource {
                agent: "codex",
                session_id: "codex-new",
                physical_source: "session_index.jsonl:codex-new",
                modified_epoch_millis: 100,
            },
            TestSessionSource {
                agent: "claude",
                session_id: "claude-old",
                physical_source: "claude-old.jsonl",
                modified_epoch_millis: 200,
            },
            TestSessionSource {
                agent: "codex",
                session_id: "codex-old",
                physical_source: "codex-old.jsonl",
                modified_epoch_millis: 50,
            },
        ],
        2,
    );
    let logical_ids = limited
        .iter()
        .map(|source| (source.agent, source.session_id))
        .collect::<HashSet<_>>();
    let codex_new_backing_sources = limited
        .iter()
        .filter(|source| source.session_id == "codex-new")
        .count();

    assert_eq!(
        logical_ids,
        HashSet::from([("claude", "claude-new"), ("codex", "codex-new")])
    );
    assert_eq!(codex_new_backing_sources, 2);
}

#[cfg(feature = "local_fs")]
#[test]
fn jcode_discovery_reads_authoritative_session_json_and_uses_updated_at() {
    let home = tempfile::tempdir().expect("create Jcode home");
    let project = home.path().join("project");
    let sessions = home.path().join(".jcode/sessions");
    fs::create_dir_all(&project).expect("create Jcode project");
    fs::create_dir_all(&sessions).expect("create Jcode sessions");

    let session_id = "session_bug_1784897411999_c3c24cb8ea67c6a2";
    let session_path = sessions.join(format!("{session_id}.json"));
    fs::write(
        &session_path,
        serde_json::json!({
            "id": session_id,
            "short_name": "bug",
            "working_dir": project,
            "updated_at": "2026-07-24T13:28:15.634345Z",
            "is_debug": false,
            "messages": [{"role": "user"}],
        })
        .to_string(),
    )
    .expect("write authoritative Jcode session");
    fs::write(
        sessions.join(format!("{session_id}.json.bak")),
        "not a session",
    )
    .expect("write ignored Jcode backup");
    fs::create_dir_all(home.path().join(".jcode/cache")).expect("create Jcode cache");
    fs::write(
        home.path().join(".jcode/cache/session-picker-list-v1.json"),
        serde_json::json!([{"id": "session_stale"}]).to_string(),
    )
    .expect("write ignored Jcode picker cache");

    let records = scan_agent_session_provider(
        AgentSessionDiscoveryProvider::Jcode,
        &CliAgentStoreRoots::for_home(home.path().to_path_buf()),
        10,
    )
    .expect("scan Jcode authoritative sessions");

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].agent, CLIAgent::Jcode);
    assert_eq!(records[0].provider_session_id, session_id);
    assert_eq!(records[0].label.as_deref(), Some("bug"));
    assert_eq!(records[0].cwd.as_deref(), project.to_str());
    assert_eq!(records[0].modified_epoch_millis, 1_784_899_695_634);
    assert_eq!(
        records[0].source,
        AgentSessionDiscoverySource::Transcript(session_path)
    );
}

#[cfg(feature = "local_fs")]
#[test]
fn jcode_discovery_filters_debug_and_empty_sessions_before_recency_limit() {
    let home = tempfile::tempdir().expect("create Jcode home");
    let sessions = home.path().join(".jcode/sessions");
    fs::create_dir_all(&sessions).expect("create Jcode sessions");

    let parent_session_id = "session_a_parent";
    fs::write(
        sessions.join(format!("{parent_session_id}.json")),
        serde_json::json!({
            "id": parent_session_id,
            "parent_id": "session_upstream_parent",
            "short_name": "visible parent session",
            "is_debug": false,
            "messages": [{"role": "user"}],
        })
        .to_string(),
    )
    .expect("write visible Jcode parent session");
    fs::write(
        sessions.join("session_y_empty.json"),
        serde_json::json!({
            "id": "session_y_empty",
            "is_debug": false,
            "messages": [],
        })
        .to_string(),
    )
    .expect("write empty Jcode session");
    fs::write(
        sessions.join("session_z_debug.json"),
        serde_json::json!({
            "id": "session_z_debug",
            "is_debug": true,
            "messages": [{"role": "user"}],
        })
        .to_string(),
    )
    .expect("write debug Jcode session");

    let records = scan_agent_session_provider(
        AgentSessionDiscoveryProvider::Jcode,
        &CliAgentStoreRoots::for_home(home.path().to_path_buf()),
        1,
    )
    .expect("scan Jcode sessions");

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].provider_session_id, parent_session_id);
    assert_eq!(records[0].label.as_deref(), Some("visible parent session"));
}

#[cfg(feature = "local_fs")]
#[test]
fn jcode_discovery_rejects_filename_id_mismatch() {
    let home = tempfile::tempdir().expect("create Jcode home");
    let sessions = home.path().join(".jcode/sessions");
    fs::create_dir_all(&sessions).expect("create Jcode sessions");
    fs::write(
        sessions.join("session_filename.json"),
        serde_json::json!({"id": "session_payload"}).to_string(),
    )
    .expect("write mismatched Jcode session");

    assert!(scan_agent_session_provider(
        AgentSessionDiscoveryProvider::Jcode,
        &CliAgentStoreRoots::for_home(home.path().to_path_buf()),
        10,
    )
    .is_err());
}

#[cfg(feature = "local_fs")]
#[test]
fn omp_discovery_reads_one_project_level_and_prefers_title_slot() {
    let home = tempfile::tempdir().expect("create Omp home");
    let project = home.path().join("project");
    let bucket = home.path().join(".omp/agent/sessions/-ashide");
    fs::create_dir_all(&project).expect("create Omp project");
    fs::create_dir_all(bucket.join("tool-logs")).expect("create Omp nested tool logs");

    let session_id = "019f0a0b-1111-4222-8333-444444444444";
    let session_path = bucket.join(format!("1784897000000_{session_id}.jsonl"));
    fs::write(
        &session_path,
        format!(
            "{}\n{}\n",
            serde_json::json!({"type": "title", "title": "标题槽优先"}),
            serde_json::json!({
                "type": "session",
                "id": session_id,
                "cwd": project,
                "title": "header title",
            })
        ),
    )
    .expect("write Omp session");
    fs::write(
        bucket.join("tool-logs/1784897000001_nested.jsonl"),
        serde_json::json!({"type": "session", "id": "nested"}).to_string(),
    )
    .expect("write ignored nested Omp log");

    let records = scan_agent_session_provider(
        AgentSessionDiscoveryProvider::Omp,
        &CliAgentStoreRoots::for_home(home.path().to_path_buf()),
        10,
    )
    .expect("scan Omp session");

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].agent, CLIAgent::Omp);
    assert_eq!(records[0].provider_session_id, session_id);
    assert_eq!(records[0].label.as_deref(), Some("标题槽优先"));
    assert_eq!(records[0].cwd.as_deref(), project.to_str());
    assert_eq!(
        records[0].source,
        AgentSessionDiscoverySource::Transcript(session_path)
    );
}

#[cfg(feature = "local_fs")]
#[test]
fn omp_discovery_orders_by_mtime_and_rejects_filename_id_mismatch() {
    let home = tempfile::tempdir().expect("create Omp home");
    let bucket = home.path().join(".omp/agent/sessions/-ashide");
    fs::create_dir_all(&bucket).expect("create Omp bucket");
    let old_id = "019f0a0b-1111-4222-8333-555555555555";
    let new_id = "019f0a0b-1111-4222-8333-666666666666";
    let old_path = bucket.join(format!("1000_{old_id}.jsonl"));
    let new_path = bucket.join(format!("2000_{new_id}.jsonl"));
    for (path, id, title) in [(&old_path, old_id, "old"), (&new_path, new_id, "new")] {
        fs::write(
            path,
            serde_json::json!({"type": "session", "id": id, "title": title}).to_string(),
        )
        .expect("write Omp ordering fixture");
    }
    fs::File::options()
        .write(true)
        .open(&old_path)
        .expect("open old Omp fixture")
        .set_times(
            fs::FileTimes::new().set_modified(UNIX_EPOCH + std::time::Duration::from_secs(100)),
        )
        .expect("set old Omp mtime");
    fs::File::options()
        .write(true)
        .open(&new_path)
        .expect("open new Omp fixture")
        .set_times(
            fs::FileTimes::new().set_modified(UNIX_EPOCH + std::time::Duration::from_secs(200)),
        )
        .expect("set new Omp mtime");

    let records = scan_agent_session_provider(
        AgentSessionDiscoveryProvider::Omp,
        &CliAgentStoreRoots::for_home(home.path().to_path_buf()),
        1,
    )
    .expect("scan latest Omp session");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].provider_session_id, new_id);
    assert_eq!(records[0].label.as_deref(), Some("new"));

    fs::write(
        bucket.join("3000_filename.jsonl"),
        serde_json::json!({"type": "session", "id": "payload"}).to_string(),
    )
    .expect("write mismatched Omp session");
    assert!(scan_agent_session_provider(
        AgentSessionDiscoveryProvider::Omp,
        &CliAgentStoreRoots::for_home(home.path().to_path_buf()),
        10,
    )
    .is_err());
}

#[cfg(feature = "local_fs")]
#[test]
fn explicit_target_roots_keep_jcode_and_omp_under_target_home() {
    let home = PathBuf::from("/target/home");
    let roots = CliAgentStoreRoots::from_explicit_target_paths(
        home.clone(),
        PathBuf::from("/target/claude"),
        PathBuf::from("/target/codex"),
    )
    .expect("construct explicit target roots");

    assert_eq!(roots.jcode_home, home.join(".jcode"));
    assert_eq!(roots.omp_agent_home, home.join(".omp/agent"));
}

#[cfg(feature = "local_fs")]
#[test]
fn jcode_and_omp_are_file_backed_session_index_providers() {
    let plan = AgentSessionDiscoveryPlan::from_registry(40);

    assert_eq!(
        CLIAgent::Jcode.session_discovery_provider(),
        Some(AgentSessionDiscoveryProvider::Jcode)
    );
    assert_eq!(
        CLIAgent::Omp.session_discovery_provider(),
        Some(AgentSessionDiscoveryProvider::Omp)
    );
    assert_eq!(
        plan.providers(),
        [
            AgentSessionDiscoveryProvider::Jcode,
            AgentSessionDiscoveryProvider::Claude,
            AgentSessionDiscoveryProvider::Codex,
            AgentSessionDiscoveryProvider::Omp,
        ],
        "all read-only persisted-session adapters must use the shared discovery plan",
    );
}
