use warpui::{AppContext, Element};

use crate::{
    appearance::Appearance,
    drive::{index::DriveIndexAction, DriveObjectType},
    object_store::ids::ClientId,
    object_store::StoredObjectMetadata,
    themes::theme::Fill,
};

use super::{LocalDriveItem, LocalDriveItemId};

#[derive(Clone)]
pub struct LocalDriveMCPServerCollection {
    id: ClientId,
}

impl LocalDriveMCPServerCollection {
    pub fn new(id: ClientId) -> Self {
        Self { id }
    }

    pub fn id(&self) -> ClientId {
        self.id
    }
}

impl LocalDriveItem for LocalDriveMCPServerCollection {
    fn display_name(&self) -> Option<String> {
        Some("MCP Servers".to_string())
    }

    fn metadata(&self) -> Option<&StoredObjectMetadata> {
        None
    }

    fn object_type(&self) -> Option<DriveObjectType> {
        Some(DriveObjectType::MCPServerCollection)
    }

    fn secondary_icon(&self, _color: Option<Fill>) -> Option<Box<dyn Element>> {
        None
    }

    fn click_action(&self) -> Option<DriveIndexAction> {
        Some(DriveIndexAction::OpenMCPServerCollection)
    }

    fn preview(&self, _appearance: &Appearance) -> Option<Box<dyn Element>> {
        None
    }

    fn local_drive_id(&self) -> LocalDriveItemId {
        LocalDriveItemId::MCPServerCollection
    }

    fn action_summary(&self, _app: &AppContext) -> Option<String> {
        None
    }

    fn clone_box(&self) -> Box<dyn LocalDriveItem> {
        Box::new(self.clone())
    }
}
