use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

#[cfg(target_family = "wasm")]
use crate::cli_agent_jsonl::resolve_current_process_cli_agent_store_roots;
use crate::cli_agent_jsonl::{
    canonical_codex_session_id, claude_session_metadata, codex_session_metadata,
    nested_string as shared_nested_string, parse_jsonl_values, recent_jsonl_files,
    CliAgentStoreRoots,
};
#[cfg(test)]
use crate::cli_agent_jsonl::{require_cli_agent_home, sha256_hex};
use crate::terminal::cli_agent_session_index::CurrentAppCliAgentSessionSourceTarget;
use crate::terminal::CLIAgent;

use super::adapter_registry::session_bridge_adapter_for_agent;
#[cfg(test)]
use super::ashide_store::SessionBridgeImportSource;
use super::ir::{SessionIr, SessionMessageIr, SessionTimestamp};
use super::SessionBridgeError;

#[derive(Debug, Clone)]
pub(crate) struct CliAgentSessionSourceBytes {
    pub(crate) reference: String,
    pub(crate) bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub(crate) struct CliAgentSessionReadResult {
    pub(crate) session: SessionIr,
    #[cfg(test)]
    pub(crate) source: SessionBridgeImportSource,
}

#[cfg(target_family = "wasm")]
pub(crate) fn read_current_app_cli_agent_session(
    target: CurrentAppCliAgentSessionSourceTarget,
    title: Option<String>,
    cwd: Option<String>,
) -> Result<CliAgentSessionReadResult, SessionBridgeError> {
    let roots = resolve_current_process_cli_agent_store_roots().map_err(|error| {
        SessionBridgeError::InvalidImport {
            message: error.to_string(),
        }
    })?;
    read_current_app_cli_agent_session_with_roots(target, &roots, title, cwd)
}

pub(crate) fn read_current_app_cli_agent_session_with_roots(
    target: CurrentAppCliAgentSessionSourceTarget,
    roots: &CliAgentStoreRoots,
    title: Option<String>,
    cwd: Option<String>,
) -> Result<CliAgentSessionReadResult, SessionBridgeError> {
    let agent = target
        .agent
        .ok_or_else(|| SessionBridgeError::InvalidImport {
            message: "indexed CLI session is missing agent metadata".to_owned(),
        })?;
    let provider_session_id = target
        .provider_session_id
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| SessionBridgeError::InvalidImport {
            message: "indexed CLI session is missing provider session id".to_owned(),
        })?;
    let source = read_current_app_cli_agent_session_source_from_roots(&target.source, roots)?;
    parse_cli_agent_session_source_bytes(agent, provider_session_id, source, title, cwd)
}

pub(crate) fn parse_cli_agent_session_source_bytes(
    agent: CLIAgent,
    provider_session_id: String,
    source: CliAgentSessionSourceBytes,
    title: Option<String>,
    cwd: Option<String>,
) -> Result<CliAgentSessionReadResult, SessionBridgeError> {
    let session = parse_cli_agent_session_ir(
        agent,
        &provider_session_id,
        &source.reference,
        &source.bytes,
        title,
        cwd,
    )?;
    #[cfg(test)]
    let import_source = SessionBridgeImportSource {
        source_session_id: provider_session_id,
        reference: source.reference.clone(),
        sha256: sha256_hex(&source.bytes),
    };
    Ok(CliAgentSessionReadResult {
        session,
        #[cfg(test)]
        source: import_source,
    })
}

#[cfg(test)]
fn read_current_app_cli_agent_session_source_with_home(
    source: &str,
    home: Option<PathBuf>,
) -> Result<CliAgentSessionSourceBytes, SessionBridgeError> {
    let home = require_cli_agent_home(home).map_err(|error| SessionBridgeError::InvalidImport {
        message: error.to_string(),
    })?;
    let roots = CliAgentStoreRoots::for_home(home);
    read_current_app_cli_agent_session_source_from_roots(source, &roots)
}

fn read_current_app_cli_agent_session_source_from_roots(
    source: &str,
    roots: &CliAgentStoreRoots,
) -> Result<CliAgentSessionSourceBytes, SessionBridgeError> {
    let path = if let Some((index_path, session_id)) = split_codex_index_source(source, roots) {
        validate_current_app_session_source_path(&index_path, roots)?;
        find_codex_session_path_by_id(&session_id, roots)?.ok_or_else(|| {
            SessionBridgeError::InvalidImport {
                message: format!("Codex transcript not found for indexed session {session_id}"),
            }
        })?
    } else {
        let path = PathBuf::from(source);
        validate_current_app_session_source_path(&path, roots)?
    };
    let bytes = fs::read(&path)?;
    let reference = path.canonicalize().unwrap_or(path).display().to_string();
    Ok(CliAgentSessionSourceBytes { reference, bytes })
}

fn validate_current_app_session_source_path(
    path: &Path,
    roots: &CliAgentStoreRoots,
) -> Result<PathBuf, SessionBridgeError> {
    let canonical_path = path.canonicalize()?;
    let allowed_roots = [roots.claude_projects(), roots.codex_sessions()];
    let codex_index = roots.codex_index();
    if codex_index
        .canonicalize()
        .ok()
        .is_some_and(|index| canonical_path == index)
    {
        return Ok(canonical_path);
    }
    let is_allowed = allowed_roots.iter().any(|root| {
        root.canonicalize()
            .ok()
            .is_some_and(|root| canonical_path == root || canonical_path.starts_with(root))
    });
    if !is_allowed {
        return Err(SessionBridgeError::InvalidImport {
            message: format!(
                "refusing to read CLI session outside known history stores: {}",
                canonical_path.display()
            ),
        });
    }
    Ok(canonical_path)
}

fn split_codex_index_source(source: &str, roots: &CliAgentStoreRoots) -> Option<(PathBuf, String)> {
    let (path, session_id) = source.rsplit_once(':')?;
    if session_id.trim().is_empty() {
        return None;
    }
    let codex_index = roots.codex_index();
    let path = PathBuf::from(path);
    let path_matches_index = path
        .canonicalize()
        .ok()
        .zip(codex_index.canonicalize().ok())
        .is_some_and(|(path, index)| path == index);
    path_matches_index.then(|| (path, session_id.to_owned()))
}

fn find_codex_session_path_by_id(
    session_id: &str,
    roots: &CliAgentStoreRoots,
) -> Result<Option<PathBuf>, SessionBridgeError> {
    let root = roots.codex_sessions();
    let files = recent_jsonl_files(&root, usize::MAX).map_err(|error| {
        SessionBridgeError::InvalidImport {
            message: error.to_string(),
        }
    })?;
    Ok(files.into_iter().find_map(|file| {
        let path = file.path;
        codex_provider_session_id_for_file(&path)
            .filter(|id| id == session_id)
            .map(|_| path)
    }))
}

fn codex_provider_session_id_for_file(path: &Path) -> Option<String> {
    let file_stem = path.file_stem()?.to_string_lossy().into_owned();
    let contents = fs::read_to_string(path).ok()?;
    codex_session_metadata(&parse_jsonl_values(&contents, Some(80)))
        .session_id
        .or_else(|| canonical_codex_session_id(&file_stem))
}

fn parse_cli_agent_session_ir(
    agent: CLIAgent,
    provider_session_id: &str,
    source_reference: &str,
    bytes: &[u8],
    title_override: Option<String>,
    cwd_override: Option<String>,
) -> Result<SessionIr, SessionBridgeError> {
    let adapter = session_bridge_adapter_for_agent(agent).ok_or_else(|| {
        SessionBridgeError::InvalidImport {
            message: format!(
                "{} has no registered SessionBridge adapter",
                agent.display_name()
            ),
        }
    })?;
    if !agent.capabilities().can_read_session_ir {
        return Err(SessionBridgeError::InvalidImport {
            message: format!("{} session fork is not supported yet", adapter.label),
        });
    }
    let cli_reader = adapter
        .cli_reader
        .ok_or_else(|| SessionBridgeError::InvalidImport {
            message: format!("{} session fork is not supported yet", adapter.label),
        })?;
    cli_reader(
        provider_session_id,
        source_reference,
        bytes,
        title_override,
        cwd_override,
    )
}

pub(crate) fn parse_codex_session_ir(
    provider_session_id: &str,
    source_reference: &str,
    bytes: &[u8],
    title_override: Option<String>,
    cwd_override: Option<String>,
) -> Result<SessionIr, SessionBridgeError> {
    let values = parse_jsonl_values(&String::from_utf8_lossy(bytes), None);
    let metadata = codex_session_metadata(&values);
    let transcript_title = codex_transcript_title(&values);
    let expected_session_id = canonical_codex_session_id(provider_session_id).ok_or_else(|| {
        SessionBridgeError::InvalidImport {
            message: format!(
                "expected Codex provider session id is not a canonical UUID: {provider_session_id}"
            ),
        }
    })?;
    let transcript_session_id = codex_transcript_session_meta_id(&values)?.or(metadata.session_id);
    if transcript_session_id
        .as_deref()
        .is_some_and(|session_id| session_id != expected_session_id)
    {
        return Err(SessionBridgeError::InvalidImport {
            message: format!(
                "Codex provider session id mismatch: expected {expected_session_id}, transcript reported {}",
                transcript_session_id.as_deref().unwrap_or_default()
            ),
        });
    }
    let session_id = transcript_session_id.unwrap_or(expected_session_id);
    let title = resolve_session_title(
        title_override,
        metadata.title,
        transcript_title,
        metadata.first_user_message,
    );
    let project_path = clean_optional(cwd_override).or(metadata.cwd);
    let mut messages = Vec::new();

    for value in values {
        if value.get("type").and_then(Value::as_str) != Some("response_item") {
            continue;
        }
        let Some(payload) = value.get("payload") else {
            continue;
        };
        if payload.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        let Some(role) = payload.get("role").and_then(Value::as_str) else {
            continue;
        };
        if role != "user" && role != "assistant" {
            continue;
        }
        let Some(text) = content_text(payload.get("content")) else {
            continue;
        };
        messages.push(SessionMessageIr {
            role: role.to_owned(),
            text,
            timestamp: timestamp_from_value(value.get("timestamp")),
        });
    }

    finalize_cli_session_ir(
        "codex",
        session_id,
        source_reference,
        title,
        project_path,
        messages,
        "Codex",
    )
}

fn codex_transcript_session_meta_id(
    values: &[Value],
) -> Result<Option<String>, SessionBridgeError> {
    let mut transcript_session_id = None::<String>;
    for value in values {
        if value.get("type").and_then(Value::as_str) != Some("session_meta") {
            continue;
        }
        let candidate = [
            shared_nested_string(value, &["payload", "id"]),
            shared_nested_string(value, &["payload", "session_id"]),
            shared_nested_string(value, &["payload", "sessionId"]),
            value.get("session_id").and_then(Value::as_str),
            value.get("sessionId").and_then(Value::as_str),
        ]
        .into_iter()
        .flatten()
        .find(|candidate| !candidate.trim().is_empty());
        let Some(candidate) = candidate else {
            continue;
        };
        let canonical = canonical_codex_session_id(candidate).ok_or_else(|| {
            SessionBridgeError::InvalidImport {
                message: format!(
                    "Codex transcript session_meta id is not a canonical UUID: {candidate}"
                ),
            }
        })?;
        if transcript_session_id
            .as_deref()
            .is_some_and(|session_id| session_id != canonical)
        {
            return Err(SessionBridgeError::InvalidImport {
                message: format!(
                    "Codex transcript contains conflicting session_meta ids: {} and {canonical}",
                    transcript_session_id.as_deref().unwrap_or_default()
                ),
            });
        }
        transcript_session_id = Some(canonical);
    }
    Ok(transcript_session_id)
}

pub(crate) fn parse_claude_session_ir(
    provider_session_id: &str,
    source_reference: &str,
    bytes: &[u8],
    title_override: Option<String>,
    cwd_override: Option<String>,
) -> Result<SessionIr, SessionBridgeError> {
    let values = parse_jsonl_values(&String::from_utf8_lossy(bytes), None);
    let metadata = claude_session_metadata(&values);
    let transcript_title = claude_transcript_title(&values);
    let session_id = metadata
        .session_id
        .unwrap_or_else(|| provider_session_id.to_owned());
    let title = resolve_session_title(
        title_override,
        metadata.title,
        transcript_title,
        metadata.first_user_message,
    );
    let project_path = clean_optional(cwd_override).or(metadata.cwd);
    let mut messages = Vec::new();

    for value in values {
        let Some(top_level_type) = value.get("type").and_then(Value::as_str) else {
            continue;
        };
        if top_level_type != "user" && top_level_type != "assistant" {
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        let Some(role) = message.get("role").and_then(Value::as_str) else {
            continue;
        };
        if role != "user" && role != "assistant" {
            continue;
        }
        let Some(text) = content_text(message.get("content")) else {
            continue;
        };
        messages.push(SessionMessageIr {
            role: role.to_owned(),
            text,
            timestamp: timestamp_from_value(value.get("timestamp")),
        });
    }

    finalize_cli_session_ir(
        "claude",
        session_id,
        source_reference,
        title,
        project_path,
        messages,
        "Claude Code",
    )
}

fn resolve_session_title(
    title_override: Option<String>,
    provider_title: Option<String>,
    transcript_title: Option<String>,
    first_user_message: Option<String>,
) -> Option<String> {
    clean_optional(title_override)
        .or_else(|| clean_optional(provider_title))
        .or_else(|| clean_optional(transcript_title))
        .or_else(|| clean_optional(first_user_message))
}

fn codex_transcript_title(values: &[Value]) -> Option<String> {
    values.iter().find_map(|value| {
        (value.get("type").and_then(Value::as_str) == Some("turn_context"))
            .then(|| nested_string(value, &["payload", "summary"]))
            .flatten()
    })
}

fn claude_transcript_title(values: &[Value]) -> Option<String> {
    values.iter().find_map(|value| {
        (value.get("type").and_then(Value::as_str) == Some("last-prompt"))
            .then(|| nested_string(value, &["lastPrompt"]))
            .flatten()
    })
}

fn finalize_cli_session_ir(
    source: &str,
    provider_session_id: String,
    source_reference: &str,
    title: Option<String>,
    project_path: Option<String>,
    messages: Vec<SessionMessageIr>,
    fallback_title: &str,
) -> Result<SessionIr, SessionBridgeError> {
    if messages.is_empty() {
        return Err(SessionBridgeError::InvalidImport {
            message: format!("no user/assistant messages found in {source} transcript"),
        });
    }
    let created_at = messages
        .first()
        .and_then(|message| message.timestamp.clone());
    let updated_at = messages
        .last()
        .and_then(|message| message.timestamp.clone());
    Ok(SessionIr {
        source: source.to_owned(),
        session_id: provider_session_id.clone(),
        title: title
            .and_then(|title| clean_optional(Some(title)))
            .unwrap_or_else(|| fallback_title.to_owned()),
        project_path,
        created_at,
        updated_at,
        messages,
        artifacts: Vec::new(),
        metadata: json!({
            "sessionBridge": {
                "operation": "read_cli_agent_history",
                "providerSessionId": provider_session_id,
                "sourceReference": source_reference,
                "source": source,
            }
        }),
    })
}

fn content_text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => clean_optional(Some(text.clone())),
        Value::Array(items) => {
            let parts = items
                .iter()
                .filter_map(content_part_text)
                .collect::<Vec<_>>();
            clean_optional(Some(parts.join("\n\n")))
        }
        Value::Object(_) => content_part_text(value?),
        Value::Null | Value::Bool(_) | Value::Number(_) => None,
    }
}

fn content_part_text(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    let part_type = object.get("type").and_then(Value::as_str);
    match part_type {
        Some("text" | "input_text" | "output_text") | None => object
            .get("text")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .map(str::to_owned),
        Some(_) => None,
    }
}

fn nested_string(value: &Value, path: &[&str]) -> Option<String> {
    shared_nested_string(value, path).map(str::to_owned)
}

fn timestamp_from_value(value: Option<&Value>) -> Option<SessionTimestamp> {
    match value? {
        Value::String(text) if !text.trim().is_empty() => {
            Some(SessionTimestamp::String(text.clone()))
        }
        Value::Number(number) => number
            .as_i64()
            .map(SessionTimestamp::Integer)
            .or_else(|| number.as_f64().map(SessionTimestamp::Float)),
        Value::Null | Value::Bool(_) | Value::Array(_) | Value::Object(_) | Value::String(_) => {
            None
        }
    }
}

fn clean_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_bridge_cli_agent_read_requires_resolved_home() {
        let error = read_current_app_cli_agent_session_source_with_home(
            "/tmp/should-never-be-read.jsonl",
            None,
        )
        .expect_err("SessionBridge read must fail before touching a source path");

        assert!(error.to_string().contains("home directory"));
    }

    #[test]
    fn parses_codex_response_items_into_session_ir() {
        let bytes = br#"
{"timestamp":"2026-06-20T01:00:00Z","type":"session_meta","payload":{"id":"11111111-1111-4111-8111-111111111111","cwd":"/repo"}}
{"timestamp":"2026-06-20T01:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}}
{"timestamp":"2026-06-20T01:00:02Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"world"}]}}
"#;
        let result = parse_cli_agent_session_source_bytes(
            CLIAgent::Codex,
            "11111111-1111-4111-8111-111111111111".to_owned(),
            CliAgentSessionSourceBytes {
                reference: "/tmp/rollout.jsonl".to_owned(),
                bytes: bytes.to_vec(),
            },
            None,
            None,
        )
        .unwrap();

        assert_eq!(result.session.source, "codex");
        assert_eq!(result.session.project_path.as_deref(), Some("/repo"));
        assert_eq!(result.session.messages.len(), 2);
        assert_eq!(result.session.messages[0].role, "user");
        assert_eq!(result.session.messages[0].text, "hello");
        assert_eq!(result.session.messages[1].role, "assistant");
        assert_eq!(result.session.messages[1].text, "world");
        assert_eq!(
            result.source.source_session_id,
            "11111111-1111-4111-8111-111111111111"
        );
    }

    #[test]
    fn codex_reader_rejects_expected_vs_transcript_session_id_mismatch() {
        let transcript =
            br#"{"type":"session_meta","payload":{"id":"22222222-2222-4222-8222-222222222222"}}"#;

        let error = parse_codex_session_ir(
            "11111111-1111-4111-8111-111111111111",
            "/tmp/rollout.jsonl",
            transcript,
            None,
            None,
        )
        .expect_err("mismatched Codex identities must be rejected before SessionIr creation");

        assert!(error.to_string().contains("provider session id mismatch"));
    }

    #[test]
    fn codex_reader_uses_real_user_event_for_fallback_title() {
        let transcript = r##"
{"type":"session_meta","payload":{"id":"33333333-3333-4333-8333-333333333333","cwd":"/repo"}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"# AGENTS.md instructions"}]}}
{"type":"event_msg","payload":{"type":"user_message","message":"修复真实标题\n后续说明"}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"修复真实标题"}]}}
{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"完成"}]}}
"##;

        let result = parse_codex_session_ir(
            "33333333-3333-4333-8333-333333333333",
            "/tmp/rollout.jsonl",
            transcript.as_bytes(),
            None,
            None,
        )
        .unwrap();

        assert_eq!(result.title, "修复真实标题");
    }

    #[test]
    fn codex_reader_provider_title_wins_over_real_user_event() {
        let transcript = r#"
{"type":"session_meta","payload":{"id":"44444444-4444-4444-8444-444444444444","cwd":"/repo"}}
{"thread_name":"正式标题"}
{"type":"event_msg","payload":{"type":"user_message","message":"首条真实消息"}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"首条真实消息"}]}}
{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"完成"}]}}
"#;

        let result = parse_codex_session_ir(
            "44444444-4444-4444-8444-444444444444",
            "/tmp/rollout.jsonl",
            transcript.as_bytes(),
            None,
            None,
        )
        .unwrap();

        assert_eq!(result.title, "正式标题");
    }

    #[test]
    fn parses_claude_text_messages_and_skips_tool_results() {
        let bytes = br#"
{"type":"ai-title","aiTitle":"Claude title","sessionId":"claude-1"}
{"timestamp":"2026-06-20T01:00:01Z","type":"user","sessionId":"claude-1","cwd":"/repo","message":{"role":"user","content":"hello"}}
{"timestamp":"2026-06-20T01:00:02Z","type":"assistant","sessionId":"claude-1","message":{"role":"assistant","content":[{"type":"thinking","thinking":"x"},{"type":"text","text":"world"}]}}
{"timestamp":"2026-06-20T01:00:03Z","type":"user","sessionId":"claude-1","message":{"role":"user","content":[{"type":"tool_result","content":"skip"}]}}
"#;
        let result = parse_cli_agent_session_source_bytes(
            CLIAgent::Claude,
            "claude-1".to_owned(),
            CliAgentSessionSourceBytes {
                reference: "/tmp/claude.jsonl".to_owned(),
                bytes: bytes.to_vec(),
            },
            None,
            None,
        )
        .unwrap();

        assert_eq!(result.session.title, "Claude title");
        assert_eq!(result.session.messages.len(), 2);
        assert_eq!(result.session.messages[0].text, "hello");
        assert_eq!(result.session.messages[1].text, "world");
    }
}
