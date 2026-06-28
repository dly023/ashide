use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, SecondsFormat, TimeZone, Utc};
use diesel::prelude::*;
use diesel::sql_types::Text;
use serde_json::{json, Value};
use uuid::Uuid;
use walkdir::WalkDir;

use crate::session_bridge::adapter_registry::{
    session_bridge_adapter_for_target, SessionBridgeForkTarget,
};
use crate::terminal::CLIAgent;

use super::ir::{SessionIr, SessionMessageIr, SessionTimestamp};
use super::SessionBridgeError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeSessionWriteReceipt {
    pub target: SessionBridgeForkTarget,
    pub session_id: String,
    pub title: String,
    pub project_path: String,
    pub session_file: PathBuf,
    pub backup_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeSessionRemoteWriteReceipt {
    pub target: SessionBridgeForkTarget,
    pub session_id: String,
    pub title: String,
    pub project_path: String,
    pub session_file: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeSessionWriteOperation {
    Write { path: String, contents: Vec<u8> },
    Append { path: String, contents: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeSessionWritePlan {
    pub receipt: NativeSessionRemoteWriteReceipt,
    pub operations: Vec<NativeSessionWriteOperation>,
}

/// Writes a SessionBridge session into the current app user's local native CLI-agent history.
///
/// Environment Runtime callers must not use this helper. Remote/native write-back has to run
/// against the owning runtime authority so a remote session cannot silently land in local
/// `~/.claude` or `~/.codex`.
pub fn write_current_app_native_session(
    session: &SessionIr,
    target: SessionBridgeForkTarget,
) -> Result<NativeSessionWriteReceipt, SessionBridgeError> {
    let home_dir = dirs::home_dir().ok_or_else(|| SessionBridgeError::InvalidImport {
        message: "home directory is unavailable".to_owned(),
    })?;
    write_native_session_to_home(session, target, &home_dir)
}

pub(crate) fn write_native_session_to_home(
    session: &SessionIr,
    target: SessionBridgeForkTarget,
    home_dir: &Path,
) -> Result<NativeSessionWriteReceipt, SessionBridgeError> {
    let adapter = session_bridge_adapter_for_target(target).ok_or_else(|| {
        SessionBridgeError::InvalidImport {
            message: format!(
                "{} has no registered SessionBridge adapter",
                target.display_label()
            ),
        }
    })?;
    if !adapter.capabilities.can_write_native_history {
        return Err(SessionBridgeError::InvalidImport {
            message: format!(
                "{} does not support native SessionBridge write-back",
                adapter.label
            ),
        });
    }
    let native_writer = adapter
        .native_writer
        .ok_or_else(|| SessionBridgeError::InvalidImport {
            message: format!(
                "{} does not support native SessionBridge write-back",
                adapter.label
            ),
        })?;
    native_writer(session, home_dir)
}

pub fn plan_native_session_write_for_home_root(
    session: &SessionIr,
    target: SessionBridgeForkTarget,
    home_root: &str,
) -> Result<NativeSessionWritePlan, SessionBridgeError> {
    let adapter = session_bridge_adapter_for_target(target).ok_or_else(|| {
        SessionBridgeError::InvalidImport {
            message: format!(
                "{} has no registered SessionBridge adapter",
                target.display_label()
            ),
        }
    })?;
    if !adapter.capabilities.can_write_native_history {
        return Err(SessionBridgeError::InvalidImport {
            message: format!(
                "{} does not support native SessionBridge write-back",
                adapter.label
            ),
        });
    }

    match target {
        SessionBridgeForkTarget::Ashide => Err(SessionBridgeError::InvalidImport {
            message: "Ashide does not use native CLI-agent history write-back".to_owned(),
        }),
        SessionBridgeForkTarget::Agent(CLIAgent::Codex) => {
            plan_codex_session_write(session, home_root)
        }
        SessionBridgeForkTarget::Agent(CLIAgent::Claude) => {
            plan_claude_session_write(session, home_root, "2.1.0")
        }
        SessionBridgeForkTarget::Agent(agent) => Err(SessionBridgeError::InvalidImport {
            message: format!(
                "{} does not support native SessionBridge write-back",
                agent.display_name()
            ),
        }),
    }
}

pub(crate) fn write_codex_session(
    session: &SessionIr,
    home_dir: &Path,
) -> Result<NativeSessionWriteReceipt, SessionBridgeError> {
    let root = codex_home(home_dir);
    let backup_dir = backup_paths(
        home_dir,
        &[
            root.join("session_index.jsonl"),
            root.join("history.jsonl"),
            root.join("state_5.sqlite"),
            root.join("state_5.sqlite-wal"),
            root.join("state_5.sqlite-shm"),
        ],
    )?;
    let sid = codex_session_id();
    let now = Utc::now();
    let timestamp = now.to_rfc3339_opts(SecondsFormat::Secs, true);
    let project_path = session_project_path(session, home_dir);
    let title = native_title(session);
    let first_user_message = first_user_message(session).unwrap_or_else(|| title.clone());
    let rollout_path = codex_rollout_path(&root, &sid, now);

    let mut rows = vec![json!({
        "timestamp": timestamp,
        "type": "session_meta",
        "payload": {
            "id": sid,
            "timestamp": timestamp,
            "cwd": project_path,
            "originator": "codex-tui",
            "source": "cli",
            "thread_source": "user",
            "cli_version": "ashide-session-bridge",
            "instructions": null,
            "model_provider": "codex",
            "model": "gpt-5.5",
            "reasoning_effort": "medium",
        },
    })];
    rows.extend(codex_transcript_rows(&session.messages, &timestamp));
    rows.push(json!({
        "timestamp": timestamp,
        "type": "turn_context",
        "payload": {
            "cwd": project_path,
            "approval_policy": "on-request",
            "sandbox_policy": "workspace-write",
            "model": "gpt-5.5",
            "effort": "medium",
            "summary": title,
        },
    }));

    write_jsonl_values(&rollout_path, &rows)?;
    append_jsonl(
        &root.join("session_index.jsonl"),
        &json!({"id": sid, "thread_name": title, "updated_at": timestamp}),
    )?;
    append_jsonl(
        &root.join("history.jsonl"),
        &json!({"session_id": sid, "ts": now.timestamp(), "text": first_user_message}),
    )?;
    if let Err(error) = upsert_codex_thread(
        &root,
        &sid,
        &title,
        &rollout_path,
        &project_path,
        &first_user_message,
        now,
    ) {
        log::warn!("codex thread enrichment failed for forked session {sid}: {error}");
    }

    Ok(NativeSessionWriteReceipt {
        target: SessionBridgeForkTarget::Agent(CLIAgent::Codex),
        session_id: sid,
        title,
        project_path,
        session_file: rollout_path,
        backup_dir,
    })
}

fn plan_codex_session_write(
    session: &SessionIr,
    home_root: &str,
) -> Result<NativeSessionWritePlan, SessionBridgeError> {
    let root = native_join(home_root, &[".codex"]);
    let sid = codex_session_id();
    let now = Utc::now();
    let timestamp = now.to_rfc3339_opts(SecondsFormat::Secs, true);
    let project_path = session_project_path_for_home_root(session, home_root);
    let title = native_title(session);
    let first_user_message = first_user_message(session).unwrap_or_else(|| title.clone());
    let rollout_path = codex_rollout_path_for_root(&root, &sid, now);

    let mut rows = vec![json!({
        "timestamp": timestamp,
        "type": "session_meta",
        "payload": {
            "id": sid,
            "timestamp": timestamp,
            "cwd": project_path,
            "originator": "codex-tui",
            "source": "cli",
            "thread_source": "user",
            "cli_version": "ashide-session-bridge",
            "instructions": null,
            "model_provider": "codex",
            "model": "gpt-5.5",
            "reasoning_effort": "medium",
        },
    })];
    rows.extend(codex_transcript_rows(&session.messages, &timestamp));
    rows.push(json!({
        "timestamp": timestamp,
        "type": "turn_context",
        "payload": {
            "cwd": project_path,
            "approval_policy": "on-request",
            "sandbox_policy": "workspace-write",
            "model": "gpt-5.5",
            "effort": "medium",
            "summary": title,
        },
    }));

    let mut operations = vec![NativeSessionWriteOperation::Write {
        path: rollout_path.clone(),
        contents: jsonl_values_to_bytes(&rows)?,
    }];
    operations.push(NativeSessionWriteOperation::Append {
        path: native_join(&root, &["session_index.jsonl"]),
        contents: jsonl_value_to_bytes(&json!({
            "id": sid,
            "thread_name": title,
            "updated_at": timestamp,
        }))?,
    });
    operations.push(NativeSessionWriteOperation::Append {
        path: native_join(&root, &["history.jsonl"]),
        contents: jsonl_value_to_bytes(&json!({
            "session_id": sid,
            "ts": now.timestamp(),
            "text": first_user_message,
        }))?,
    });

    Ok(NativeSessionWritePlan {
        receipt: NativeSessionRemoteWriteReceipt {
            target: SessionBridgeForkTarget::Agent(CLIAgent::Codex),
            session_id: sid,
            title,
            project_path,
            session_file: rollout_path,
        },
        operations,
    })
}

pub(crate) fn write_claude_session(
    session: &SessionIr,
    home_dir: &Path,
) -> Result<NativeSessionWriteReceipt, SessionBridgeError> {
    let root = claude_home(home_dir);
    let project_path = session_project_path(session, home_dir);
    let title = native_title(session);
    let project_dir = root
        .join("projects")
        .join(project_to_claude_slug(&project_path));
    let backup_dir = backup_paths(home_dir, &[root.join("history.jsonl"), project_dir.clone()])?;
    let sid = Uuid::new_v4().to_string();
    let session_file = project_dir.join(format!("{sid}.jsonl"));
    let timestamp = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let version = claude_native_version(&root);

    let mut rows = Vec::new();
    let mut parent_uuid = None::<String>;
    let mut leaf_uuid = None::<String>;
    for message in &session.messages {
        let row_uuid = Uuid::new_v4().to_string();
        let row_timestamp = source_timestamp_iso(message.timestamp.as_ref(), &timestamp);
        rows.push(json!({
            "isSidechain": false,
            "userType": "external",
            "entrypoint": "cli",
            "cwd": project_path,
            "sessionId": sid,
            "version": version,
            "gitBranch": "HEAD",
            "parentUuid": parent_uuid,
            "type": message.role,
            "uuid": row_uuid,
            "timestamp": row_timestamp,
            "message": claude_message_payload(&message.role, &message.text, &row_uuid),
        }));
        parent_uuid = Some(row_uuid.clone());
        leaf_uuid = Some(row_uuid);
    }
    rows.extend([
        json!({"type": "last-prompt", "lastPrompt": title, "leafUuid": leaf_uuid, "sessionId": sid}),
        json!({"type": "mode", "mode": "normal", "sessionId": sid}),
        json!({"type": "permission-mode", "permissionMode": "default", "sessionId": sid}),
    ]);

    write_jsonl_values(&session_file, &rows)?;
    append_jsonl(
        &root.join("history.jsonl"),
        &json!({
            "display": title,
            "pastedContents": {},
            "timestamp": current_timestamp_millis(),
            "project": project_path,
            "sessionId": sid,
        }),
    )?;

    Ok(NativeSessionWriteReceipt {
        target: SessionBridgeForkTarget::Agent(CLIAgent::Claude),
        session_id: sid,
        title,
        project_path,
        session_file,
        backup_dir,
    })
}

fn plan_claude_session_write(
    session: &SessionIr,
    home_root: &str,
    version: &str,
) -> Result<NativeSessionWritePlan, SessionBridgeError> {
    let root = native_join(home_root, &[".claude"]);
    let project_path = session_project_path_for_home_root(session, home_root);
    let title = native_title(session);
    let project_slug = project_to_claude_slug(&project_path);
    let project_dir = native_join(&root, &["projects", &project_slug]);
    let sid = Uuid::new_v4().to_string();
    let session_filename = format!("{sid}.jsonl");
    let session_file = native_join(&project_dir, &[&session_filename]);
    let timestamp = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);

    let mut rows = Vec::new();
    let mut parent_uuid = None::<String>;
    let mut leaf_uuid = None::<String>;
    for message in &session.messages {
        let row_uuid = Uuid::new_v4().to_string();
        let row_timestamp = source_timestamp_iso(message.timestamp.as_ref(), &timestamp);
        rows.push(json!({
            "isSidechain": false,
            "userType": "external",
            "entrypoint": "cli",
            "cwd": project_path,
            "sessionId": sid,
            "version": version,
            "gitBranch": "HEAD",
            "parentUuid": parent_uuid,
            "type": message.role,
            "uuid": row_uuid,
            "timestamp": row_timestamp,
            "message": claude_message_payload(&message.role, &message.text, &row_uuid),
        }));
        parent_uuid = Some(row_uuid.clone());
        leaf_uuid = Some(row_uuid);
    }
    rows.extend([
        json!({"type": "last-prompt", "lastPrompt": title, "leafUuid": leaf_uuid, "sessionId": sid}),
        json!({"type": "mode", "mode": "normal", "sessionId": sid}),
        json!({"type": "permission-mode", "permissionMode": "default", "sessionId": sid}),
    ]);

    let operations = vec![
        NativeSessionWriteOperation::Write {
            path: session_file.clone(),
            contents: jsonl_values_to_bytes(&rows)?,
        },
        NativeSessionWriteOperation::Append {
            path: native_join(&root, &["history.jsonl"]),
            contents: jsonl_value_to_bytes(&json!({
                "display": title,
                "pastedContents": {},
                "timestamp": current_timestamp_millis(),
                "project": project_path,
                "sessionId": sid,
            }))?,
        },
    ];

    Ok(NativeSessionWritePlan {
        receipt: NativeSessionRemoteWriteReceipt {
            target: SessionBridgeForkTarget::Agent(CLIAgent::Claude),
            session_id: sid,
            title,
            project_path,
            session_file,
        },
        operations,
    })
}

fn codex_home(home_dir: &Path) -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir.join(".codex"))
}

fn claude_home(home_dir: &Path) -> PathBuf {
    std::env::var_os("CLAUDE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir.join(".claude"))
}

fn native_title(session: &SessionIr) -> String {
    let title = session.title.trim();
    if title.is_empty() {
        "Untitled fork".to_owned()
    } else {
        title.to_owned()
    }
}

fn session_project_path(session: &SessionIr, home_dir: &Path) -> String {
    session
        .project_path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| home_dir.display().to_string())
}

fn session_project_path_for_home_root(session: &SessionIr, home_root: &str) -> String {
    session
        .project_path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| home_root.to_owned())
}

fn first_user_message(session: &SessionIr) -> Option<String> {
    session
        .messages
        .iter()
        .find(|message| message.role == "user")
        .map(|message| message.text.clone())
        .filter(|text| !text.trim().is_empty())
}

fn codex_session_id() -> String {
    Uuid::new_v4().to_string()
}

fn codex_rollout_path(root: &Path, sid: &str, timestamp: DateTime<Utc>) -> PathBuf {
    root.join("sessions")
        .join(timestamp.format("%Y").to_string())
        .join(timestamp.format("%m").to_string())
        .join(timestamp.format("%d").to_string())
        .join(format!(
            "rollout-{}-{sid}.jsonl",
            timestamp.format("%Y-%m-%dT%H-%M-%S")
        ))
}

fn codex_rollout_path_for_root(root: &str, sid: &str, timestamp: DateTime<Utc>) -> String {
    let year = timestamp.format("%Y").to_string();
    let month = timestamp.format("%m").to_string();
    let day = timestamp.format("%d").to_string();
    let filename = format!(
        "rollout-{}-{sid}.jsonl",
        timestamp.format("%Y-%m-%dT%H-%M-%S")
    );
    native_join(root, &["sessions", &year, &month, &day, &filename])
}

fn codex_transcript_rows(messages: &[SessionMessageIr], fallback_timestamp: &str) -> Vec<Value> {
    let mut rows = Vec::new();
    let mut turn_id = None::<String>;
    let mut last_agent_message = None::<String>;
    let mut user_turn_count = 0usize;

    for message in messages {
        let timestamp = source_timestamp_iso(message.timestamp.as_ref(), fallback_timestamp);
        if message.role == "user" {
            if let Some(turn_id) = turn_id.take() {
                rows.push(codex_turn_complete_row(
                    fallback_timestamp,
                    &turn_id,
                    last_agent_message.as_deref(),
                    None,
                ));
            }
            user_turn_count += 1;
            let new_turn_id = format!("ashide-session-bridge-turn-{user_turn_count}");
            rows.push(codex_turn_started_row(
                &timestamp,
                &new_turn_id,
                message.timestamp.as_ref(),
            ));
            rows.extend(codex_message_rows(message, &timestamp));
            turn_id = Some(new_turn_id);
            last_agent_message = None;
            continue;
        }

        if turn_id.is_none() {
            user_turn_count += 1;
            let new_turn_id = format!("ashide-session-bridge-turn-{user_turn_count}");
            rows.push(codex_turn_started_row(
                &timestamp,
                &new_turn_id,
                message.timestamp.as_ref(),
            ));
            turn_id = Some(new_turn_id);
        }
        rows.extend(codex_message_rows(message, &timestamp));
        if message.role == "assistant" {
            last_agent_message = Some(message.text.clone());
        }
    }

    if let Some(turn_id) = turn_id {
        rows.push(json!({
            "timestamp": fallback_timestamp,
            "type": "event_msg",
            "payload": {
                "type": "agent_message",
                "message": "<ASHIDE SESSION FORK>",
                "phase": null,
                "memory_citation": null,
            },
        }));
        rows.push(codex_token_count_row(fallback_timestamp, messages));
        rows.push(codex_turn_complete_row(
            fallback_timestamp,
            &turn_id,
            last_agent_message.as_deref(),
            messages
                .last()
                .and_then(|message| message.timestamp.as_ref()),
        ));
    }
    rows
}

fn codex_message_rows(message: &SessionMessageIr, timestamp: &str) -> Vec<Value> {
    match message.role.as_str() {
        "user" => vec![
            json!({
                "timestamp": timestamp,
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": message.text}],
                },
            }),
            json!({
                "timestamp": timestamp,
                "type": "event_msg",
                "payload": {
                    "type": "user_message",
                    "message": message.text,
                    "images": [],
                    "local_images": [],
                    "text_elements": [],
                },
            }),
        ],
        "assistant" => vec![
            json!({
                "timestamp": timestamp,
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": message.text}],
                },
            }),
            json!({
                "timestamp": timestamp,
                "type": "event_msg",
                "payload": {
                    "type": "agent_message",
                    "message": message.text,
                    "phase": "final_answer",
                    "memory_citation": null,
                },
            }),
        ],
        _ => Vec::new(),
    }
}

fn codex_turn_started_row(
    timestamp: &str,
    turn_id: &str,
    source_timestamp: Option<&SessionTimestamp>,
) -> Value {
    let mut payload = json!({
        "type": "task_started",
        "turn_id": turn_id,
        "trace_id": null,
        "model_context_window": null,
    });
    if let Some(started_at) = source_timestamp_seconds(source_timestamp) {
        payload["started_at"] = json!(started_at);
    }
    json!({"timestamp": timestamp, "type": "event_msg", "payload": payload})
}

fn codex_turn_complete_row(
    timestamp: &str,
    turn_id: &str,
    last_agent_message: Option<&str>,
    source_timestamp: Option<&SessionTimestamp>,
) -> Value {
    let mut payload = json!({
        "type": "task_complete",
        "turn_id": turn_id,
        "last_agent_message": last_agent_message,
        "duration_ms": null,
        "time_to_first_token_ms": null,
    });
    if let Some(completed_at) = source_timestamp_seconds(source_timestamp) {
        payload["completed_at"] = json!(completed_at);
    }
    json!({"timestamp": timestamp, "type": "event_msg", "payload": payload})
}

fn codex_token_count_row(timestamp: &str, messages: &[SessionMessageIr]) -> Value {
    let total_tokens = messages
        .iter()
        .map(|message| (message.text.chars().count() / 4).max(1))
        .sum::<usize>();
    let usage = json!({"total_tokens": total_tokens});
    json!({
        "timestamp": timestamp,
        "type": "event_msg",
        "payload": {
            "type": "token_count",
            "info": {
                "total_token_usage": usage,
                "last_token_usage": usage,
                "model_context_window": null,
            },
            "rate_limits": null,
        },
    })
}

fn claude_message_payload(role: &str, text: &str, row_uuid: &str) -> Value {
    if role == "user" {
        return json!({"role": "user", "content": text});
    }
    json!({
        "id": row_uuid,
        "type": "message",
        "role": "assistant",
        "model": "ashide-session-bridge",
        "content": [{"type": "text", "text": text}],
        "stop_reason": "end_turn",
        "stop_sequence": null,
        "usage": {},
    })
}

fn claude_native_version(root: &Path) -> String {
    let projects = root.join("projects");
    let Ok(entries) = WalkDir::new(projects)
        .follow_links(false)
        .max_depth(4)
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
    else {
        return "2.1.0".to_owned();
    };
    let mut files = entries
        .into_iter()
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "jsonl")
        })
        .filter_map(|entry| {
            let modified = fs::metadata(entry.path()).ok()?.modified().ok()?;
            Some((modified, entry.into_path()))
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| right.0.cmp(&left.0));
    for (_, path) in files.into_iter().take(50) {
        if path
            .components()
            .any(|component| component.as_os_str() == "subagents")
        {
            continue;
        }
        let Ok(contents) = fs::read_to_string(path) else {
            continue;
        };
        for line in contents.lines() {
            let Ok(value) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if let Some(version) = value.get("version").and_then(Value::as_str) {
                return version.to_owned();
            }
        }
    }
    "2.1.0".to_owned()
}

fn project_to_claude_slug(project_path: &str) -> String {
    let normalized = project_path.trim();
    let normalized = if normalized.is_empty() {
        "workspace"
    } else {
        normalized
    };
    let slug = normalized
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>();
    if slug.is_empty() {
        "-".to_owned()
    } else {
        slug
    }
}

fn source_timestamp_iso(timestamp: Option<&SessionTimestamp>, fallback: &str) -> String {
    source_datetime(timestamp)
        .map(|timestamp| timestamp.to_rfc3339_opts(SecondsFormat::Secs, true))
        .unwrap_or_else(|| fallback.to_owned())
}

fn source_timestamp_seconds(timestamp: Option<&SessionTimestamp>) -> Option<i64> {
    source_datetime(timestamp).map(|timestamp| timestamp.timestamp())
}

fn source_datetime(timestamp: Option<&SessionTimestamp>) -> Option<DateTime<Utc>> {
    match timestamp? {
        SessionTimestamp::String(value) => DateTime::parse_from_rfc3339(value)
            .ok()
            .map(|timestamp| timestamp.with_timezone(&Utc)),
        SessionTimestamp::Integer(value) => {
            if *value > 100_000_000_000 {
                Utc.timestamp_millis_opt(*value).single()
            } else {
                Utc.timestamp_opt(*value, 0).single()
            }
        }
        SessionTimestamp::Float(value) => {
            let millis = if *value > 100_000_000_000. {
                *value as i64
            } else {
                (*value * 1000.) as i64
            };
            Utc.timestamp_millis_opt(millis).single()
        }
    }
}

fn current_timestamp_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

fn append_jsonl(path: &Path, row: &Value) -> Result<(), SessionBridgeError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let line = jsonl_value_to_string(row)?;
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(line.as_bytes())?;
    Ok(())
}

fn write_jsonl_values(path: &Path, rows: &[Value]) -> Result<(), SessionBridgeError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let contents = jsonl_values_to_string(rows)?;
    fs::write(path, contents)?;
    Ok(())
}

fn jsonl_value_to_string(row: &Value) -> Result<String, SessionBridgeError> {
    let mut line = serde_json::to_string(row)?;
    line.push('\n');
    Ok(line)
}

fn jsonl_value_to_bytes(row: &Value) -> Result<Vec<u8>, SessionBridgeError> {
    Ok(jsonl_value_to_string(row)?.into_bytes())
}

fn jsonl_values_to_string(rows: &[Value]) -> Result<String, SessionBridgeError> {
    let mut contents = String::new();
    for row in rows {
        contents.push_str(&jsonl_value_to_string(row)?);
    }
    Ok(contents)
}

fn jsonl_values_to_bytes(rows: &[Value]) -> Result<Vec<u8>, SessionBridgeError> {
    Ok(jsonl_values_to_string(rows)?.into_bytes())
}

fn native_join(root: &str, segments: &[&str]) -> String {
    let separator = native_path_separator(root);
    let mut path = root.trim_end_matches(['/', '\\']).to_owned();
    if path.is_empty() {
        path.push(separator);
    }
    for segment in segments {
        let segment = segment.trim_matches(['/', '\\']);
        if segment.is_empty() {
            continue;
        }
        if path != separator.to_string() && !path.ends_with(['/', '\\']) {
            path.push(separator);
        }
        path.push_str(segment);
    }
    path
}

fn native_path_separator(path: &str) -> char {
    if path.contains('\\') || path.as_bytes().get(1).is_some_and(|byte| *byte == b':') {
        '\\'
    } else {
        '/'
    }
}

fn backup_paths(home_dir: &Path, paths: &[PathBuf]) -> Result<PathBuf, SessionBridgeError> {
    let backup_dir = home_dir
        .join(".agents")
        .join("session-bridge")
        .join("backups")
        .join(format!(
            "{}-{}",
            Utc::now().format("%Y%m%dT%H%M%SZ"),
            Uuid::new_v4()
        ));
    fs::create_dir_all(&backup_dir)?;

    for path in paths.iter().filter(|path| path.exists()) {
        let relative_path = path
            .strip_prefix(home_dir)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| {
                path.file_name()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("unknown"))
            });
        let destination = backup_dir.join(relative_path);
        if path.is_dir() {
            copy_dir_recursive(path, &destination)?;
        } else {
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(path, destination)?;
        }
    }
    Ok(backup_dir)
}

fn copy_dir_recursive(source: &Path, destination: &Path) -> Result<(), SessionBridgeError> {
    for entry in WalkDir::new(source).follow_links(false) {
        let entry = entry.map_err(|error| SessionBridgeError::Io(error.into()))?;
        let relative_path = entry.path().strip_prefix(source).map_err(|error| {
            SessionBridgeError::Io(std::io::Error::new(std::io::ErrorKind::Other, error))
        })?;
        let target = destination.join(relative_path);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

#[derive(QueryableByName)]
struct SqliteNameRow {
    #[diesel(sql_type = Text)]
    name: String,
}

/// Best-effort enrichment of Codex's own `threads` sqlite so the forked
/// session shows up in Codex's thread listing. An absent DB/table means this
/// Codex install simply has no thread index, so we skip with `Ok(())`; but a
/// DB that exists yet fails to open, introspect, or accept the write is a real
/// failure that must surface (the caller logs it) instead of being swallowed.
fn upsert_codex_thread(
    root: &Path,
    sid: &str,
    title: &str,
    rollout_path: &Path,
    cwd: &str,
    first_user_message: &str,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let db_path = root.join("state_5.sqlite");
    if !db_path.exists() {
        return Ok(());
    }
    let db_url = db_path
        .to_str()
        .ok_or_else(|| format!("codex thread db path is not valid UTF-8: {}", db_path.display()))?;
    let mut conn = diesel::SqliteConnection::establish(db_url)
        .map_err(|error| format!("failed to open codex thread db {db_url}: {error}"))?;
    let table_rows = diesel::sql_query(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'threads'",
    )
    .load::<SqliteNameRow>(&mut conn)
    .map_err(|error| format!("failed to introspect codex thread db tables: {error}"))?;
    if table_rows.is_empty() {
        return Ok(());
    }
    let column_rows = diesel::sql_query("SELECT name FROM pragma_table_info('threads')")
        .load::<SqliteNameRow>(&mut conn)
        .map_err(|error| format!("failed to introspect codex threads columns: {error}"))?;
    let available_columns = column_rows
        .into_iter()
        .map(|row| row.name)
        .collect::<std::collections::HashSet<_>>();
    let now_seconds = now.timestamp();
    let now_millis = now.timestamp_millis();
    let mut values = BTreeMap::new();
    values.insert("id", SqlLiteral::Text(sid.to_owned()));
    values.insert(
        "rollout_path",
        SqlLiteral::Text(rollout_path.display().to_string()),
    );
    values.insert("created_at", SqlLiteral::Integer(now_seconds));
    values.insert("updated_at", SqlLiteral::Integer(now_seconds));
    values.insert("source", SqlLiteral::Text("cli".to_owned()));
    values.insert("model_provider", SqlLiteral::Text("codex".to_owned()));
    values.insert("cwd", SqlLiteral::Text(cwd.to_owned()));
    values.insert("title", SqlLiteral::Text(title.to_owned()));
    values.insert(
        "sandbox_policy",
        SqlLiteral::Text("workspace-write".to_owned()),
    );
    values.insert("approval_mode", SqlLiteral::Text("on-request".to_owned()));
    values.insert("tokens_used", SqlLiteral::Integer(0));
    values.insert("has_user_event", SqlLiteral::Integer(0));
    values.insert("archived", SqlLiteral::Integer(0));
    values.insert(
        "cli_version",
        SqlLiteral::Text("ashide-session-bridge".to_owned()),
    );
    values.insert(
        "first_user_message",
        SqlLiteral::Text(first_user_message.chars().take(2000).collect()),
    );
    values.insert("memory_mode", SqlLiteral::Text("enabled".to_owned()));
    values.insert("model", SqlLiteral::Text("gpt-5.5".to_owned()));
    values.insert("reasoning_effort", SqlLiteral::Text("medium".to_owned()));
    values.insert("created_at_ms", SqlLiteral::Integer(now_millis));
    values.insert("updated_at_ms", SqlLiteral::Integer(now_millis));
    values.insert("thread_source", SqlLiteral::Text("user".to_owned()));
    values.insert(
        "preview",
        SqlLiteral::Text(title.chars().take(500).collect()),
    );

    let selected = values
        .into_iter()
        .filter(|(column, _)| available_columns.contains(*column))
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Ok(());
    }
    let columns = selected
        .iter()
        .map(|(column, _)| *column)
        .collect::<Vec<_>>()
        .join(", ");
    let literals = selected
        .iter()
        .map(|(_, value)| value.to_sql())
        .collect::<Vec<_>>()
        .join(", ");
    backup_codex_state_db(root).map_err(|error| {
        format!("refusing to mutate codex thread db without a backup: {error}")
    })?;
    let sql = format!("INSERT OR REPLACE INTO threads ({columns}) VALUES ({literals})");
    diesel::sql_query(sql)
        .execute(&mut conn)
        .map_err(|error| format!("failed to upsert codex thread row for {sid}: {error}"))?;
    Ok(())
}

/// Best-effort rolling backup of Codex's `state_5.sqlite` before we mutate it,
/// as a backup-before-write safety net (the Rust port dropped the original).
/// Overwrites
/// the previous backup so there is always one pre-write rollback point. Returns `Err`
/// if the main DB file can't be copied — the caller then skips the write rather than
/// risk an unrecoverable mutation of another tool's database. WAL/SHM sidecars are
/// best-effort (absent unless the DB is in WAL mode).
fn backup_codex_state_db(root: &Path) -> Result<(), String> {
    let db = root.join("state_5.sqlite");
    let backup = root.join("state_5.sqlite.ashide-bak");
    std::fs::copy(&db, &backup)
        .map_err(|error| format!("failed to back up {}: {error}", db.display()))?;
    for suffix in ["-wal", "-shm"] {
        let side = root.join(format!("state_5.sqlite{suffix}"));
        if side.exists() {
            let dst = root.join(format!("state_5.sqlite{suffix}.ashide-bak"));
            if let Err(error) = std::fs::copy(&side, &dst) {
                log::warn!(
                    "codex state db sidecar backup failed for {}: {error}",
                    side.display()
                );
            }
        }
    }
    Ok(())
}

enum SqlLiteral {
    Text(String),
    Integer(i64),
}

impl SqlLiteral {
    fn to_sql(&self) -> String {
        match self {
            SqlLiteral::Text(value) => format!("'{}'", value.replace('\'', "''")),
            SqlLiteral::Integer(value) => value.to_string(),
        }
    }
}
