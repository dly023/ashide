use warpui::Element;

use crate::{
    appearance::Appearance,
    drive::{index::DriveIndexAction, DriveObjectType},
    object_store::{Space, StoredObjectMetadata},
    themes::theme::Fill,
};

use super::{LocalDriveItem, LocalDriveItemId};

#[derive(Clone)]
pub struct LocalDriveSpace {
    space: Space,
}

impl LocalDriveSpace {
    #[allow(dead_code)]
    pub fn new(space: Space) -> Self {
        Self { space }
    }
}

impl LocalDriveItem for LocalDriveSpace {
    fn display_name(&self) -> Option<String> {
        None
    }

    fn metadata(&self) -> Option<&StoredObjectMetadata> {
        None
    }

    fn object_type(&self) -> Option<DriveObjectType> {
        None
    }

    fn secondary_icon(&self, _color: Option<Fill>) -> Option<Box<dyn Element>> {
        None
    }

    fn click_action(&self) -> Option<DriveIndexAction> {
        None
    }

    fn preview(&self, _appearance: &Appearance) -> Option<Box<dyn Element>> {
        None
    }

    fn local_drive_id(&self) -> LocalDriveItemId {
        LocalDriveItemId::Space(self.space)
    }

    fn clone_box(&self) -> Box<dyn LocalDriveItem> {
        Box::new(self.clone())
    }

    fn action_summary(&self, _app: &warpui::AppContext) -> Option<String> {
        None
    }
}
