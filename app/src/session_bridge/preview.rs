use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::bundle::default_bundle_name;
use super::ir::{SessionIr, SessionTimestamp};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionBridgePreview {
    pub session_id: String,
    pub title: String,
    pub source: String,
    pub project_path: Option<String>,
    pub message_count: usize,
    pub artifact_count: usize,
    pub created_at: Option<SessionTimestamp>,
    pub updated_at: Option<SessionTimestamp>,
    pub output_path: Option<PathBuf>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl SessionBridgePreview {
    pub fn from_session(
        session: &SessionIr,
        output_path: Option<PathBuf>,
        warnings: Vec<String>,
    ) -> Self {
        Self {
            session_id: session.session_id.clone(),
            title: session.title.clone(),
            source: session.source.clone(),
            project_path: session.project_path.clone(),
            message_count: session.messages.len(),
            artifact_count: session.artifacts.len(),
            created_at: session.created_at.clone(),
            updated_at: session.updated_at.clone(),
            output_path,
            warnings,
        }
    }

    pub fn dry_run_text(&self) -> String {
        let output = self
            .output_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| {
                default_bundle_name(&SessionIr {
                    source: self.source.clone(),
                    session_id: self.session_id.clone(),
                    title: self.title.clone(),
                    project_path: self.project_path.clone(),
                    created_at: self.created_at.clone(),
                    updated_at: self.updated_at.clone(),
                    messages: Vec::new(),
                    artifacts: Vec::new(),
                    metadata: serde_json::Value::Object(Default::default()),
                })
            });
        let mut lines = vec![
            format!(
                "DRY RUN: would export {} session {} to {}",
                self.source, self.session_id, output
            ),
            format!("Title: {}", self.title),
            format!(
                "Project: {}",
                self.project_path.as_deref().unwrap_or("unknown")
            ),
            format!("Messages: {}", self.message_count),
            format!("Artifacts: {}", self.artifact_count),
        ];
        if !self.warnings.is_empty() {
            lines.push("Warnings:".to_owned());
            lines.extend(self.warnings.iter().map(|warning| format!("- {warning}")));
        }
        lines.join("\n")
    }
}
