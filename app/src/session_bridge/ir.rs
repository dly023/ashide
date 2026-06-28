use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SessionTimestamp {
    String(String),
    Integer(i64),
    Float(f64),
}

impl From<String> for SessionTimestamp {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for SessionTimestamp {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMessageIr {
    pub role: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<SessionTimestamp>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionArtifactIr {
    pub kind: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionIr {
    pub source: String,
    pub session_id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<SessionTimestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<SessionTimestamp>,
    #[serde(default)]
    pub messages: Vec<SessionMessageIr>,
    #[serde(default)]
    pub artifacts: Vec<SessionArtifactIr>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

impl SessionIr {
    pub fn new_ashide(session_id: impl Into<String>) -> Self {
        let session_id = session_id.into();
        Self {
            source: "ashide".to_owned(),
            title: "Untitled".to_owned(),
            session_id,
            project_path: None,
            created_at: None,
            updated_at: None,
            messages: Vec::new(),
            artifacts: Vec::new(),
            metadata: serde_json::Value::Object(Default::default()),
        }
    }
}
