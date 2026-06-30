use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use uuid::Uuid;

use super::ir::{SessionArtifactIr, SessionIr, SessionMessageIr};
use super::SessionBridgeError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEditSpec {
    pub redactions: Vec<String>,
    pub trim_after: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDerivationReceipt {
    pub operation: String,
    pub source_session_id: String,
    pub derived_session_id: String,
    pub created_at: String,
    pub original_message_count: usize,
    pub message_count: usize,
    pub artifact_count: usize,
    pub redaction_replacement_count: usize,
    pub trimmed_message_count: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionDerivation {
    pub session: SessionIr,
    pub receipt: SessionDerivationReceipt,
}

pub fn fork_session(source: &SessionIr, new_session_id: Option<String>) -> SessionDerivation {
    let derived_session_id = new_session_id.unwrap_or_else(generated_session_id);
    let created_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let mut session = source.clone();
    session.session_id = derived_session_id.clone();
    session.title = derived_title(&source.title, "fork");
    attach_derivation_metadata(
        &mut session,
        "fork",
        &source.session_id,
        &derived_session_id,
        &created_at,
        json!({
            "forkedFromSessionId": source.session_id,
            "parentSessionId": source.session_id,
        }),
    );
    let receipt = SessionDerivationReceipt {
        operation: "fork".to_owned(),
        source_session_id: source.session_id.clone(),
        derived_session_id,
        created_at,
        original_message_count: source.messages.len(),
        message_count: session.messages.len(),
        artifact_count: session.artifacts.len(),
        redaction_replacement_count: 0,
        trimmed_message_count: 0,
    };
    SessionDerivation { session, receipt }
}

pub fn edit_session(
    source: &SessionIr,
    spec: SessionEditSpec,
    new_session_id: Option<String>,
) -> Result<SessionDerivation, SessionBridgeError> {
    let redactions: Vec<String> = spec
        .redactions
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect();
    if redactions.is_empty() && spec.trim_after.is_none() {
        return Err(SessionBridgeError::InvalidEdit {
            message: "provide at least one --redact value or --trim-after".to_owned(),
        });
    }

    let derived_session_id = new_session_id.unwrap_or_else(generated_session_id);
    let created_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let mut session = source.clone();
    session.session_id = derived_session_id.clone();
    session.title = derived_title(&source.title, "edited");

    let original_message_count = source.messages.len();
    let trimmed_message_count = if let Some(keep_count) = spec.trim_after {
        if keep_count < session.messages.len() {
            let trimmed = session.messages.len() - keep_count;
            session.messages.truncate(keep_count);
            trimmed
        } else {
            0
        }
    } else {
        0
    };

    let mut redaction_replacement_count = 0;
    session.messages = session
        .messages
        .into_iter()
        .map(|message| redact_message(message, &redactions, &mut redaction_replacement_count))
        .collect();
    session.artifacts = session
        .artifacts
        .into_iter()
        .map(|artifact| redact_artifact(artifact, &redactions, &mut redaction_replacement_count))
        .collect();

    attach_derivation_metadata(
        &mut session,
        "edit",
        &source.session_id,
        &derived_session_id,
        &created_at,
        json!({
            "redactionLiteralCount": redactions.len(),
            "redactionReplacementCount": redaction_replacement_count,
            "trimAfter": spec.trim_after,
            "trimmedMessageCount": trimmed_message_count,
        }),
    );

    let receipt = SessionDerivationReceipt {
        operation: "edit".to_owned(),
        source_session_id: source.session_id.clone(),
        derived_session_id,
        created_at,
        original_message_count,
        message_count: session.messages.len(),
        artifact_count: session.artifacts.len(),
        redaction_replacement_count,
        trimmed_message_count,
    };
    Ok(SessionDerivation { session, receipt })
}

pub fn edit_session_messages(
    source: &SessionIr,
    messages: Vec<SessionMessageIr>,
    new_session_id: Option<String>,
) -> Result<SessionDerivation, SessionBridgeError> {
    if messages.is_empty() {
        return Err(SessionBridgeError::InvalidEdit {
            message: "provide at least one edited message".to_owned(),
        });
    }

    let derived_session_id = new_session_id.unwrap_or_else(generated_session_id);
    let created_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let original_message_count = source.messages.len();
    let mut session = source.clone();
    session.session_id = derived_session_id.clone();
    session.title = derived_title(&source.title, "edited");
    session.messages = messages;
    let message_count = session.messages.len();
    let trimmed_message_count = original_message_count.saturating_sub(message_count);

    attach_derivation_metadata(
        &mut session,
        "edit",
        &source.session_id,
        &derived_session_id,
        &created_at,
        json!({
            "messageEditor": true,
            "originalMessageCount": original_message_count,
            "messageCount": message_count,
            "trimmedMessageCount": trimmed_message_count,
        }),
    );

    let receipt = SessionDerivationReceipt {
        operation: "edit".to_owned(),
        source_session_id: source.session_id.clone(),
        derived_session_id,
        created_at,
        original_message_count,
        message_count,
        artifact_count: session.artifacts.len(),
        redaction_replacement_count: 0,
        trimmed_message_count,
    };
    Ok(SessionDerivation { session, receipt })
}

fn redact_message(
    mut message: SessionMessageIr,
    redactions: &[String],
    replacement_count: &mut usize,
) -> SessionMessageIr {
    message.text = redact_literals(&message.text, redactions, replacement_count);
    message
}

fn redact_artifact(
    mut artifact: SessionArtifactIr,
    redactions: &[String],
    replacement_count: &mut usize,
) -> SessionArtifactIr {
    artifact.text = redact_literals(&artifact.text, redactions, replacement_count);
    artifact
}

fn redact_literals(text: &str, redactions: &[String], replacement_count: &mut usize) -> String {
    let mut out = text.to_owned();
    for needle in redactions {
        let count = out.matches(needle).count();
        if count > 0 {
            *replacement_count += count;
            out = out.replace(needle, "[REDACTED_BY_SESSION_BRIDGE]");
        }
    }
    out
}

fn attach_derivation_metadata(
    session: &mut SessionIr,
    operation: &str,
    source_session_id: &str,
    derived_session_id: &str,
    created_at: &str,
    operation_metadata: Value,
) {
    let root = metadata_object(&mut session.metadata);
    root.insert(
        "sessionBridge".to_owned(),
        json!({
            "operation": operation,
            "sourceSessionId": source_session_id,
            "derivedSessionId": derived_session_id,
            "createdAt": created_at,
            "operationMetadata": operation_metadata,
        }),
    );
}

fn metadata_object(value: &mut Value) -> &mut Map<String, Value> {
    if !value.is_object() {
        *value = Value::Object(Map::new());
    }
    value.as_object_mut().expect("metadata is an object")
}

fn derived_title(title: &str, suffix: &str) -> String {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        format!("Untitled ({suffix})")
    } else {
        format!("{trimmed} ({suffix})")
    }
}

fn generated_session_id() -> String {
    Uuid::new_v4().to_string()
}
