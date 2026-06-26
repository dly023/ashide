use crate::{notebooks::NotebookId, workflows::WorkflowId};

use super::{ClientId, ObjectStoreId, StableObjectId};

#[test]
pub fn test_client_object_store_id_serialization() {
    let id: ObjectStoreId = ObjectStoreId::ClientId(ClientId::new());
    let serialized = serde_json::to_string(&id).expect("failed to serialize");
    assert_eq!(serialized, format!("\"{}\"", id.uid()));
    let deserialized: ObjectStoreId =
        serde_json::from_str(serialized.as_str()).expect("failed to deserialize");
    assert_eq!(id, deserialized);
}

#[test]
pub fn test_server_object_store_id_serialization() {
    let id = ObjectStoreId::StableId(WorkflowId::from(StableObjectId::from(123)).into());
    let serialized = serde_json::to_string(&id).expect("failed to serialize");
    assert_eq!(serialized, format!("\"{}\"", StableObjectId::from(123)));
    let deserialized: ObjectStoreId =
        serde_json::from_str(serialized.as_str()).expect("failed to deserialize");
    assert_eq!(id, deserialized);
}

#[test]
pub fn test_server_object_store_id_uid_serialization() {
    let id =
        ObjectStoreId::StableId(NotebookId::from(String::from("Ymgrzu0nh2HwDNeYEtXF1x")).into());
    let serialized = serde_json::to_string(&id).expect("failed to serialize");
    assert_eq!(
        serialized,
        format!("\"{}\"", String::from("Ymgrzu0nh2HwDNeYEtXF1x"))
    );
    let deserialized: ObjectStoreId =
        serde_json::from_str(serialized.as_str()).expect("failed to deserialize");
    assert_eq!(id, deserialized);
}
