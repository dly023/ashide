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
pub struct LocalDriveAIFactCollection {
    id: ClientId,
}

impl LocalDriveAIFactCollection {
    pub fn new(id: ClientId) -> Self {
        Self { id }
    }

    pub fn id(&self) -> ClientId {
        self.id
    }
}

impl LocalDriveItem for LocalDriveAIFactCollection {
    fn display_name(&self) -> Option<String> {
        Some(crate::t!("rules-collection-name"))
    }

    fn metadata(&self) -> Option<&StoredObjectMetadata> {
        None
    }

    fn object_type(&self) -> Option<DriveObjectType> {
        Some(DriveObjectType::AIFactCollection)
    }

    fn secondary_icon(&self, _color: Option<Fill>) -> Option<Box<dyn Element>> {
        None
    }

    fn click_action(&self) -> Option<DriveIndexAction> {
        Some(DriveIndexAction::OpenAIFactCollection)
    }

    fn preview(&self, _appearance: &Appearance) -> Option<Box<dyn Element>> {
        None
    }

    fn local_drive_id(&self) -> LocalDriveItemId {
        LocalDriveItemId::AIFactCollection
    }

    fn action_summary(&self, _app: &AppContext) -> Option<String> {
        None
    }

    fn clone_box(&self) -> Box<dyn LocalDriveItem> {
        Box::new(self.clone())
    }
}
