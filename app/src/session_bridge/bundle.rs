use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::ir::{SessionArtifactIr, SessionIr, SessionMessageIr};
use super::sanitize::clean_text;
use super::SessionBridgeError;

pub const BUNDLE_FORMAT: &str = "ashide-sessionbridge-bundle";
pub const BUNDLE_VERSION: u32 = 1;
pub const SESSION_BRIDGE_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionBridgeBundle {
    pub format: String,
    pub version: u32,
    pub exported_at: String,
    pub ashide_version: Option<String>,
    pub session_bridge_version: u32,
    pub session: SessionIr,
}

pub fn build_bundle(session: &SessionIr) -> SessionBridgeBundle {
    SessionBridgeBundle {
        format: BUNDLE_FORMAT.to_owned(),
        version: BUNDLE_VERSION,
        exported_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        ashide_version: warp_core::channel::ChannelState::app_version().map(str::to_owned),
        session_bridge_version: SESSION_BRIDGE_VERSION,
        session: sanitized_session(session),
    }
}

pub fn write_bundle(
    session: &SessionIr,
    out: Option<&Path>,
) -> Result<PathBuf, SessionBridgeError> {
    let target = bundle_output_path(session, out)?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bundle = build_bundle(session);
    let json = serde_json::to_string_pretty(&bundle)?;
    std::fs::write(&target, format!("{json}\n"))?;
    Ok(target)
}

pub fn read_bundle(path: &Path) -> Result<SessionBridgeBundle, SessionBridgeError> {
    let json = std::fs::read_to_string(path)?;
    let bundle: SessionBridgeBundle = serde_json::from_str(&json)?;
    validate_bundle(&bundle)?;
    Ok(bundle)
}

fn validate_bundle(bundle: &SessionBridgeBundle) -> Result<(), SessionBridgeError> {
    if bundle.format != BUNDLE_FORMAT {
        return Err(SessionBridgeError::InvalidBundleFormat {
            actual: bundle.format.clone(),
            expected: BUNDLE_FORMAT.to_owned(),
        });
    }
    if bundle.version != BUNDLE_VERSION {
        return Err(SessionBridgeError::UnsupportedBundleVersion {
            actual: bundle.version,
            expected: BUNDLE_VERSION,
        });
    }
    if bundle.session_bridge_version != SESSION_BRIDGE_VERSION {
        return Err(SessionBridgeError::UnsupportedSessionBridgeVersion {
            actual: bundle.session_bridge_version,
            expected: SESSION_BRIDGE_VERSION,
        });
    }
    Ok(())
}

pub fn bundle_output_path(
    session: &SessionIr,
    out: Option<&Path>,
) -> Result<PathBuf, SessionBridgeError> {
    Ok(match out {
        Some(path) if path.is_dir() => path.join(default_bundle_name(session)),
        Some(path) => path.to_path_buf(),
        None => std::env::current_dir()?.join(default_bundle_name(session)),
    })
}

pub fn default_bundle_name(session: &SessionIr) -> String {
    default_bundle_name_for(&session.source, &session.session_id)
}

pub fn default_bundle_name_for(source: &str, session_id: &str) -> String {
    let safe_id = safe_filename_component(session_id);
    format!("ashide-sessionbridge-export-{source}-{safe_id}.json")
}

pub fn safe_filename_component(value: &str) -> String {
    let mut out = String::new();
    let mut previous_dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            out.push(ch);
            previous_dash = false;
        } else if !previous_dash {
            out.push('-');
            previous_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "session".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn sanitized_session(session: &SessionIr) -> SessionIr {
    let mut clean = session.clone();
    clean.messages = session
        .messages
        .iter()
        .map(|message| SessionMessageIr {
            role: message.role.clone(),
            text: clean_text(&message.text),
            timestamp: message.timestamp.clone(),
        })
        .collect();
    clean.artifacts = session
        .artifacts
        .iter()
        .map(|artifact| SessionArtifactIr {
            kind: artifact.kind.clone(),
            text: clean_text(&artifact.text),
            path: artifact.path.clone(),
            metadata: artifact.metadata.clone(),
        })
        .collect();
    clean
}
