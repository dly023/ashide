use chrono::{DateTime, SecondsFormat, TimeZone, Utc};
use diesel::SqliteConnection;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::Path;
use uuid::Uuid;
use warp_multi_agent_api as api;

use crate::ai::agent::conversation::AIConversation;
use crate::ai::agent::conversation::AIConversationId;
use crate::ai::artifacts::Artifact;
use crate::persistence::agent::{
    insert_agent_conversation, read_agent_conversation_by_id, read_agent_conversations,
    InsertConversationError,
};
use crate::persistence::model::{
    AgentConversation, AgentConversationData, SessionBridgeImportMetadata,
};

use super::bundle::SessionBridgeBundle;
use super::ir::{SessionArtifactIr, SessionIr, SessionMessageIr, SessionTimestamp};
use super::SessionBridgeError;

#[derive(Debug, Clone, PartialEq)]
pub struct AshideSessionReadResult {
    pub session: SessionIr,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AshideSessionImportPlan {
    pub source_session_id: String,
    pub target_session_id: String,
    pub title: String,
    pub project_path: Option<String>,
    pub message_count: usize,
    pub artifact_count: usize,
    pub source_reference: String,
    pub source_sha256: String,
}

#[derive(Debug, Clone)]
pub struct AshideSessionWriteBack {
    pub plan: AshideSessionImportPlan,
    pub tasks: Vec<api::Task>,
    pub conversation_data: AgentConversationData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionBridgeImportSource {
    pub source_session_id: String,
    pub reference: String,
    pub sha256: String,
}

impl SessionBridgeImportSource {
    pub fn from_bundle_path(
        path: &Path,
        source_session_id: &str,
    ) -> Result<Self, SessionBridgeError> {
        let canonical_path = path.canonicalize()?;
        Ok(Self {
            source_session_id: source_session_id.to_owned(),
            reference: canonical_path.display().to_string(),
            sha256: sha256_file(&canonical_path)?,
        })
    }

    pub fn from_derived_session(
        operation: &str,
        source_session_id: &str,
        derived_session_id: &str,
        session: &SessionIr,
    ) -> Result<Self, SessionBridgeError> {
        let bytes = serde_json::to_vec(session)?;
        Ok(Self {
            source_session_id: source_session_id.to_owned(),
            reference: format!(
                "session-bridge://derived/{operation}/{source_session_id}/{derived_session_id}"
            ),
            sha256: sha256_bytes(&bytes),
        })
    }
}

struct AshideSessionImportPayload {
    plan: AshideSessionImportPlan,
    tasks: Vec<api::Task>,
    conversation_data: AgentConversationData,
}

pub fn read_ashide_session_by_id(
    conn: &mut SqliteConnection,
    conversation_id: &str,
) -> Result<AshideSessionReadResult, SessionBridgeError> {
    let persisted = read_agent_conversation_by_id(conn, conversation_id)
        .map_err(|error| SessionBridgeError::Persistence(error.to_string()))?
        .ok_or_else(|| SessionBridgeError::ConversationNotFound {
            id: conversation_id.to_owned(),
        })?;
    agent_conversation_to_session_ir(persisted)
}

pub fn list_ashide_sessions(
    conn: &mut SqliteConnection,
) -> Result<Vec<AshideSessionReadResult>, SessionBridgeError> {
    read_agent_conversations(conn)
        .map_err(|error| SessionBridgeError::Persistence(error.to_string()))?
        .into_iter()
        .map(agent_conversation_to_session_ir)
        .collect()
}

pub fn preview_ashide_session_import(
    conn: &mut SqliteConnection,
    bundle: &SessionBridgeBundle,
    source_bundle_path: &Path,
    new_session_id: Option<String>,
) -> Result<AshideSessionImportPlan, SessionBridgeError> {
    let source = SessionBridgeImportSource::from_bundle_path(
        source_bundle_path,
        &bundle.session.session_id,
    )?;
    let payload = build_import_payload(&bundle.session, source, new_session_id)?;
    ensure_target_session_absent(conn, &payload.plan.target_session_id)?;
    Ok(payload.plan)
}

pub fn import_ashide_session_bundle(
    conn: &mut SqliteConnection,
    bundle: &SessionBridgeBundle,
    source_bundle_path: &Path,
    new_session_id: Option<String>,
) -> Result<AshideSessionImportPlan, SessionBridgeError> {
    let source = SessionBridgeImportSource::from_bundle_path(
        source_bundle_path,
        &bundle.session.session_id,
    )?;
    let payload = build_import_payload(&bundle.session, source, new_session_id)?;
    insert_agent_conversation(
        conn,
        &payload.plan.target_session_id,
        &payload.tasks,
        payload.conversation_data,
    )
    .map_err(import_error_to_session_bridge_error)?;
    Ok(payload.plan)
}

pub fn preview_ashide_session_write_back(
    conn: &mut SqliteConnection,
    session: &SessionIr,
    source: SessionBridgeImportSource,
) -> Result<AshideSessionImportPlan, SessionBridgeError> {
    let payload = build_import_payload(session, source, None)?;
    ensure_target_session_absent(conn, &payload.plan.target_session_id)?;
    Ok(payload.plan)
}

pub fn import_ashide_session_write_back(
    conn: &mut SqliteConnection,
    session: &SessionIr,
    source: SessionBridgeImportSource,
) -> Result<AshideSessionImportPlan, SessionBridgeError> {
    Ok(import_ashide_session_write_back_with_payload(conn, session, source)?.plan)
}

pub fn import_ashide_session_write_back_with_payload(
    conn: &mut SqliteConnection,
    session: &SessionIr,
    source: SessionBridgeImportSource,
) -> Result<AshideSessionWriteBack, SessionBridgeError> {
    let payload = build_import_payload(session, source, None)?;
    insert_agent_conversation(
        conn,
        &payload.plan.target_session_id,
        &payload.tasks,
        payload.conversation_data.clone(),
    )
    .map_err(import_error_to_session_bridge_error)?;
    Ok(AshideSessionWriteBack {
        plan: payload.plan,
        tasks: payload.tasks,
        conversation_data: payload.conversation_data,
    })
}

fn build_import_payload(
    session: &SessionIr,
    source: SessionBridgeImportSource,
    new_session_id: Option<String>,
) -> Result<AshideSessionImportPayload, SessionBridgeError> {
    let target_session_id = new_session_id.unwrap_or_else(|| session.session_id.clone());
    validate_native_session_id(&target_session_id)?;

    let imported_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let tasks = session_ir_to_native_tasks(session, &target_session_id)?;
    let artifacts_json = native_artifacts_json(session, &target_session_id)?;
    let conversation_data = AgentConversationData {
        server_conversation_token: None,
        conversation_usage_metadata: None,
        reverted_action_ids: None,
        forked_from_server_conversation_token: None,
        artifacts_json,
        parent_agent_id: None,
        agent_name: Some(session.title.clone()),
        parent_conversation_id: None,
        run_id: None,
        autoexecute_override: None,
        last_event_sequence: None,
        compaction_state_json: None,
        byop_repair_state_json: None,
        session_bridge_import: Some(SessionBridgeImportMetadata {
            source_session_id: source.source_session_id.clone(),
            imported_at,
            source_reference: source.reference.clone(),
            source_sha256: source.sha256.clone(),
            derivation_metadata: session.metadata.get("sessionBridge").cloned(),
        }),
    };
    let plan = AshideSessionImportPlan {
        source_session_id: source.source_session_id,
        target_session_id,
        title: session.title.clone(),
        project_path: session.project_path.clone(),
        message_count: session.messages.len(),
        artifact_count: session.artifacts.len(),
        source_reference: source.reference,
        source_sha256: source.sha256,
    };
    Ok(AshideSessionImportPayload {
        plan,
        tasks,
        conversation_data,
    })
}

fn import_error_to_session_bridge_error(error: InsertConversationError) -> SessionBridgeError {
    match error {
        InsertConversationError::AlreadyExists(id) => {
            SessionBridgeError::ConversationAlreadyExists { id }
        }
        InsertConversationError::Serialization(source) => SessionBridgeError::Json(source),
        InsertConversationError::DB(source) => SessionBridgeError::Persistence(source.to_string()),
    }
}

fn ensure_target_session_absent(
    conn: &mut SqliteConnection,
    target_session_id: &str,
) -> Result<(), SessionBridgeError> {
    if read_agent_conversation_by_id(conn, target_session_id)
        .map_err(|error| SessionBridgeError::Persistence(error.to_string()))?
        .is_some()
    {
        return Err(SessionBridgeError::ConversationAlreadyExists {
            id: target_session_id.to_owned(),
        });
    }
    Ok(())
}

fn validate_native_session_id(session_id: &str) -> Result<(), SessionBridgeError> {
    Uuid::parse_str(session_id).map_err(|error| SessionBridgeError::InvalidConversationId {
        id: session_id.to_owned(),
        message: format!("native imported sessions require a UUID id: {error}"),
    })?;
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, SessionBridgeError> {
    let bytes = std::fs::read(path)?;
    Ok(sha256_bytes(&bytes))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex::encode(digest)
}

fn session_ir_to_native_tasks(
    session: &SessionIr,
    target_session_id: &str,
) -> Result<Vec<api::Task>, SessionBridgeError> {
    let task_id = format!("session-bridge-root-{target_session_id}");
    let mut request_index = 0usize;
    let mut current_request_id = None::<String>;
    let mut messages = Vec::with_capacity(session.messages.len());
    let directory_context = session_bridge_directory_context(session.project_path.as_deref());

    for (index, message) in session.messages.iter().enumerate() {
        let timestamp = timestamp_to_proto(message.timestamp.as_ref())?;
        let request_id = match message.role.as_str() {
            "user" => {
                request_index += 1;
                let request_id =
                    format!("session-bridge-request-{target_session_id}-{request_index}");
                current_request_id = Some(request_id.clone());
                request_id
            }
            "assistant" => current_request_id.clone().unwrap_or_else(|| {
                request_index += 1;
                let request_id =
                    format!("session-bridge-request-{target_session_id}-{request_index}");
                current_request_id = Some(request_id.clone());
                request_id
            }),
            role => {
                return Err(SessionBridgeError::InvalidImport {
                    message: format!(
                        "unsupported message role `{role}` at index {index}; expected `user` or `assistant`"
                    ),
                });
            }
        };

        messages.push(api::Message {
            id: format!("session-bridge-message-{target_session_id}-{index}"),
            task_id: task_id.clone(),
            request_id,
            timestamp: timestamp.clone(),
            server_message_data: String::new(),
            citations: vec![],
            message: Some(match message.role.as_str() {
                "user" => api::message::Message::UserQuery(api::message::UserQuery {
                    query: message.text.clone(),
                    context: directory_context.clone(),
                    referenced_attachments: Default::default(),
                    mode: None,
                    intended_agent: api::AgentType::Primary.into(),
                }),
                "assistant" => {
                    api::message::Message::AgentOutput(api::message::AgentOutput {
                        text: message.text.clone(),
                    })
                }
                role => {
                    return Err(SessionBridgeError::InvalidImport {
                        message: format!(
                            "unsupported message role `{role}` at index {index}; expected `user` or `assistant`"
                        ),
                    });
                }
            }),
        });
    }

    Ok(vec![api::Task {
        id: task_id,
        description: session.title.clone(),
        dependencies: None,
        messages,
        summary: String::new(),
        server_data: String::new(),
    }])
}

fn session_bridge_directory_context(project_path: Option<&str>) -> Option<api::InputContext> {
    let project_path = project_path?.trim();
    if project_path.is_empty() {
        return None;
    }

    Some(api::InputContext {
        directory: Some(api::input_context::Directory {
            pwd: project_path.to_owned(),
            home: String::new(),
            pwd_file_symbols_indexed: false,
        }),
        ..Default::default()
    })
}

fn timestamp_to_proto(
    timestamp: Option<&SessionTimestamp>,
) -> Result<Option<prost_types::Timestamp>, SessionBridgeError> {
    let Some(timestamp) = timestamp else {
        return Ok(None);
    };
    let datetime = match timestamp {
        SessionTimestamp::String(value) => DateTime::parse_from_rfc3339(value)
            .map_err(|error| SessionBridgeError::InvalidImport {
                message: format!("invalid RFC3339 timestamp `{value}`: {error}"),
            })?
            .with_timezone(&Utc),
        SessionTimestamp::Integer(seconds) => {
            Utc.timestamp_opt(*seconds, 0).single().ok_or_else(|| {
                SessionBridgeError::InvalidImport {
                    message: format!("invalid unix timestamp seconds `{seconds}`"),
                }
            })?
        }
        SessionTimestamp::Float(seconds) => {
            if !seconds.is_finite() {
                return Err(SessionBridgeError::InvalidImport {
                    message: format!("invalid non-finite unix timestamp `{seconds}`"),
                });
            }
            let mut whole_seconds = seconds.floor() as i64;
            let mut nanos = ((seconds - whole_seconds as f64) * 1_000_000_000.0).round() as u32;
            if nanos == 1_000_000_000 {
                whole_seconds += 1;
                nanos = 0;
            }
            Utc.timestamp_opt(whole_seconds, nanos)
                .single()
                .ok_or_else(|| SessionBridgeError::InvalidImport {
                    message: format!("invalid unix timestamp `{seconds}`"),
                })?
        }
    };
    Ok(Some(prost_types::Timestamp {
        seconds: datetime.timestamp(),
        nanos: datetime.timestamp_subsec_nanos() as i32,
    }))
}

fn native_artifacts_json(
    session: &SessionIr,
    target_session_id: &str,
) -> Result<Option<String>, SessionBridgeError> {
    if session.artifacts.is_empty() {
        return Ok(None);
    }

    let artifacts = session
        .artifacts
        .iter()
        .enumerate()
        .map(|(index, artifact)| native_artifact_from_ir(index, artifact, target_session_id))
        .collect::<Vec<_>>();
    Ok(Some(serde_json::to_string(&artifacts)?))
}

fn native_artifact_from_ir(
    index: usize,
    artifact: &SessionArtifactIr,
    target_session_id: &str,
) -> Artifact {
    match artifact.kind.as_str() {
        "plan" => Artifact::Plan {
            document_uid: artifact
                .metadata
                .get("documentUid")
                .and_then(|value| value.as_str())
                .map(str::to_owned)
                .unwrap_or_else(|| format!("session-bridge-plan-{target_session_id}-{index}")),
            notebook_uid: None,
            title: Some(artifact.text.clone()),
        },
        "pull_request" => Artifact::PullRequest {
            url: artifact
                .metadata
                .get("url")
                .and_then(|value| value.as_str())
                .map(str::to_owned)
                .or_else(|| artifact.path.clone())
                .unwrap_or_else(|| format!("session-bridge://{target_session_id}/{index}")),
            branch: artifact
                .metadata
                .get("branch")
                .and_then(|value| value.as_str())
                .unwrap_or("session-bridge-import")
                .to_owned(),
            repo: None,
            number: None,
        },
        "screenshot" => Artifact::Screenshot {
            artifact_uid: artifact
                .metadata
                .get("artifactUid")
                .and_then(|value| value.as_str())
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    format!("session-bridge-screenshot-{target_session_id}-{index}")
                }),
            mime_type: artifact
                .metadata
                .get("mimeType")
                .and_then(|value| value.as_str())
                .unwrap_or("image/png")
                .to_owned(),
            description: Some(artifact.text.clone()),
        },
        "file" => file_artifact_from_ir(index, artifact, target_session_id),
        _ => file_artifact_from_ir(index, artifact, target_session_id),
    }
}

fn file_artifact_from_ir(
    index: usize,
    artifact: &SessionArtifactIr,
    target_session_id: &str,
) -> Artifact {
    let filepath = artifact
        .path
        .clone()
        .unwrap_or_else(|| format!("session-bridge://{target_session_id}/{index}"));
    let filename = Path::new(&filepath)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("session-bridge-artifact-{index}.txt"));
    Artifact::File {
        artifact_uid: artifact
            .metadata
            .get("artifactUid")
            .and_then(|value| value.as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("session-bridge-file-{target_session_id}-{index}")),
        filepath,
        filename,
        mime_type: artifact
            .metadata
            .get("mimeType")
            .and_then(|value| value.as_str())
            .unwrap_or("text/plain")
            .to_owned(),
        description: Some(format!(
            "SessionBridge {} artifact: {}",
            artifact.kind, artifact.text
        )),
        size_bytes: i32::try_from(artifact.text.len()).ok(),
    }
}

pub fn agent_conversation_to_session_ir(
    persisted: AgentConversation,
) -> Result<AshideSessionReadResult, SessionBridgeError> {
    let conversation_id = persisted.conversation.conversation_id.clone();
    let mut warnings = Vec::new();
    let conversation_data: AgentConversationData =
        serde_json::from_str(&persisted.conversation.conversation_data).map_err(|source| {
            SessionBridgeError::ConversationDataJson {
                id: conversation_id.clone(),
                source,
            }
        })?;

    let runtime_id = AIConversationId::try_from(conversation_id.clone()).map_err(|error| {
        SessionBridgeError::InvalidConversationId {
            id: conversation_id.clone(),
            message: format!("native sessions require a UUID id: {error}"),
        }
    })?;

    let runtime_conversation =
        AIConversation::new_restored(runtime_id, persisted.tasks, Some(conversation_data.clone()))
            .map_err(|error| SessionBridgeError::ConversationNotRestorable {
                id: conversation_id.clone(),
                message: error.to_string(),
            })?;
    let artifacts = artifacts_from_data(&conversation_id, &conversation_data, &mut warnings);
    Ok(ashide_conversation_to_session_ir(
        conversation_id,
        &runtime_conversation,
        conversation_data
            .agent_name
            .clone()
            .or_else(|| first_user_message_title(&runtime_conversation)),
        build_metadata(&conversation_data),
        artifacts,
        warnings,
    ))
}

pub fn live_ashide_conversation_to_session_ir(
    conversation: &AIConversation,
) -> AshideSessionReadResult {
    let conversation_id = conversation.id().to_string();
    let artifacts = conversation
        .artifacts()
        .iter()
        .cloned()
        .map(artifact_to_ir)
        .collect();
    ashide_conversation_to_session_ir(
        conversation_id,
        conversation,
        conversation
            .agent_name()
            .map(str::to_owned)
            .or_else(|| conversation.title())
            .or_else(|| first_user_message_title(conversation)),
        build_live_metadata(conversation),
        artifacts,
        Vec::new(),
    )
}

fn ashide_conversation_to_session_ir(
    conversation_id: String,
    runtime_conversation: &AIConversation,
    title: Option<String>,
    metadata: serde_json::Value,
    artifacts: Vec<SessionArtifactIr>,
    mut warnings: Vec<String>,
) -> AshideSessionReadResult {
    let mut session = SessionIr::new_ashide(conversation_id);
    session.title = title.unwrap_or_else(|| "Untitled".to_owned());
    session.metadata = metadata;

    let mut project_path = None;
    let mut messages = Vec::new();
    for exchange in runtime_conversation.all_exchanges() {
        if project_path.is_none() {
            project_path = exchange.working_directory.clone();
        }
        let input = exchange.format_input_for_copy();
        if !input.trim().is_empty() {
            messages.push(SessionMessageIr {
                role: "user".to_owned(),
                text: input,
                timestamp: Some(SessionTimestamp::String(exchange.start_time.to_rfc3339())),
            });
        }
        let output = exchange.format_output_for_copy(None);
        if !output.trim().is_empty() {
            messages.push(SessionMessageIr {
                role: "assistant".to_owned(),
                text: output,
                timestamp: Some(SessionTimestamp::String(
                    exchange
                        .finish_time
                        .unwrap_or(exchange.start_time)
                        .to_rfc3339(),
                )),
            });
        }
    }
    if messages.is_empty() {
        warnings
            .push("conversation restored with no exportable user or assistant messages".to_owned());
    }
    session.messages = messages;
    session.project_path = project_path;
    session.artifacts = artifacts;
    session.created_at = session
        .messages
        .first()
        .and_then(|message| message.timestamp.clone());
    session.updated_at = session
        .messages
        .last()
        .and_then(|message| message.timestamp.clone());

    AshideSessionReadResult { session, warnings }
}

fn build_metadata(data: &AgentConversationData) -> serde_json::Value {
    let mut metadata = json!({
        "serverConversationToken": data.server_conversation_token,
        "forkedFromServerConversationToken": data.forked_from_server_conversation_token,
        "runId": data.run_id,
        "parentAgentId": data.parent_agent_id,
        "agentName": data.agent_name,
        "parentConversationId": data.parent_conversation_id,
        "lastEventSequence": data.last_event_sequence,
        "hasCompactionState": data.compaction_state_json.is_some(),
        "hasByopRepairState": data.byop_repair_state_json.is_some(),
        "sessionBridgeImport": data.session_bridge_import,
    });
    if let Some(derivation_metadata) = data
        .session_bridge_import
        .as_ref()
        .and_then(|import| import.derivation_metadata.clone())
    {
        metadata["sessionBridge"] = derivation_metadata;
    }
    metadata
}

fn build_live_metadata(conversation: &AIConversation) -> serde_json::Value {
    json!({
        "source": "live",
        "agentName": conversation.agent_name(),
        "parentAgentId": conversation.parent_agent_id(),
        "parentConversationId": conversation.parent_conversation_id().map(|id| id.to_string()),
    })
}

fn artifacts_from_data(
    conversation_id: &str,
    data: &AgentConversationData,
    warnings: &mut Vec<String>,
) -> Vec<SessionArtifactIr> {
    let Some(json) = data.artifacts_json.as_deref() else {
        return Vec::new();
    };
    match serde_json::from_str::<Vec<crate::ai::artifacts::Artifact>>(json) {
        Ok(artifacts) => artifacts.into_iter().map(artifact_to_ir).collect(),
        Err(source) => {
            warnings.push(format!(
                "failed to deserialize artifacts for {conversation_id}: {source}"
            ));
            Vec::new()
        }
    }
}

fn artifact_to_ir(artifact: crate::ai::artifacts::Artifact) -> SessionArtifactIr {
    match artifact {
        crate::ai::artifacts::Artifact::Plan {
            title,
            document_uid,
            notebook_uid,
        } => SessionArtifactIr {
            kind: "plan".to_owned(),
            text: title.unwrap_or_else(|| format!("Plan {document_uid}")),
            path: None,
            metadata: json!({
                "documentUid": document_uid,
                "notebookUid": notebook_uid,
            }),
        },
        crate::ai::artifacts::Artifact::PullRequest {
            url,
            branch,
            repo,
            number,
        } => SessionArtifactIr {
            kind: "pull_request".to_owned(),
            text: format!("Pull request {url} on branch {branch}"),
            path: Some(url.clone()),
            metadata: json!({
                "url": url,
                "branch": branch,
                "repo": repo,
                "number": number,
            }),
        },
        crate::ai::artifacts::Artifact::Screenshot {
            artifact_uid,
            mime_type,
            description,
        } => SessionArtifactIr {
            kind: "screenshot".to_owned(),
            text: description.unwrap_or_else(|| format!("Screenshot {artifact_uid} ({mime_type})")),
            path: None,
            metadata: json!({
                "artifactUid": artifact_uid,
                "mimeType": mime_type,
            }),
        },
        crate::ai::artifacts::Artifact::File {
            artifact_uid,
            filepath,
            filename,
            mime_type,
            description,
            size_bytes,
        } => {
            let mut text = description.unwrap_or_else(|| filename.clone());
            if let Some(size) = size_bytes {
                text.push_str(&format!(" ({mime_type}, {size} bytes)"));
            } else {
                text.push_str(&format!(" ({mime_type})"));
            }
            SessionArtifactIr {
                kind: "file".to_owned(),
                text,
                path: Some(filepath.clone()),
                metadata: json!({
                    "artifactUid": artifact_uid,
                    "filepath": filepath,
                    "filename": filename,
                    "mimeType": mime_type,
                    "sizeBytes": size_bytes,
                }),
            }
        }
    }
}

fn first_user_message_title(conversation: &AIConversation) -> Option<String> {
    conversation
        .all_exchanges()
        .into_iter()
        .find_map(|exchange| {
            let input = exchange.format_input_for_copy();
            let line = input.lines().find(|line| !line.trim().is_empty())?.trim();
            let mut title = line.chars().take(80).collect::<String>();
            if line.chars().count() > 80 {
                title.push('…');
            }
            Some(title)
        })
}
