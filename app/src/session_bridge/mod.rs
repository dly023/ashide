pub mod adapter_registry;
pub mod bundle;
#[cfg(feature = "local_fs")]
pub mod cli_agent_reader;
pub mod ir;
#[cfg(feature = "local_fs")]
pub mod native_writer;
pub mod preview;
pub mod sanitize;
pub mod source;
pub mod transform;

#[cfg(feature = "local_fs")]
pub mod ashide_store;

#[derive(thiserror::Error, Debug)]
pub enum SessionBridgeError {
    #[error("conversation not found: {id}")]
    ConversationNotFound { id: String },
    #[error("invalid conversation id {id}: {message}")]
    InvalidConversationId { id: String, message: String },
    #[error("conversation {id} is not restorable: {message}")]
    ConversationNotRestorable { id: String, message: String },
    #[error("invalid session edit: {message}")]
    InvalidEdit { message: String },
    #[error("invalid session import: {message}")]
    InvalidImport { message: String },
    #[error("invalid SessionBridge bundle format {actual}: expected {expected}")]
    InvalidBundleFormat { actual: String, expected: String },
    #[error("unsupported SessionBridge bundle version {actual}: expected {expected}")]
    UnsupportedBundleVersion { actual: u32, expected: u32 },
    #[error("unsupported SessionBridge runtime version {actual}: expected {expected}")]
    UnsupportedSessionBridgeVersion { actual: u32, expected: u32 },
    #[error("conversation already exists: {id}")]
    ConversationAlreadyExists { id: String },
    #[error("failed to deserialize conversation data for {id}: {source}")]
    ConversationDataJson {
        id: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to deserialize artifacts for {id}: {source}")]
    ArtifactJson {
        id: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("persistence error: {0}")]
    Persistence(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests;
