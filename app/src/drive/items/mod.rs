use warpui::{AppContext, Element};

use crate::{
    appearance::Appearance,
    object_store::{Space, StoredObjectMetadata},
    themes::theme::Fill,
    ui_components::icons::Icon,
};

use super::{
    index::{local_drive_section_header_position_id, DriveIndexAction, DriveIndexSection},
    local_drive_object_styling::local_drive_icon_color,
    DriveObjectType, ObjectTypeAndId,
};

pub mod ai_fact;
pub mod ai_fact_collection;
pub mod env_var_collection;
pub mod folder;
pub mod item;
pub mod mcp_server_collection;
pub mod notebook;
pub mod space;
pub mod workflow;

pub trait LocalDriveItem {
    /// The display name of the item. If the item is unnamed, this may return `None` - implementations
    /// should prefer this over `Some("")`, as it lets the index view use alternate styling.
    fn display_name(&self) -> Option<String>;
    fn metadata(&self) -> Option<&StoredObjectMetadata>;
    fn object_type(&self) -> Option<DriveObjectType>;
    fn secondary_icon(&self, color: Option<Fill>) -> Option<Box<dyn Element>>; // The optional icon to the right of the name
    fn click_action(&self) -> Option<DriveIndexAction>;
    fn preview(&self, appearance: &Appearance) -> Option<Box<dyn Element>>;
    fn local_drive_id(&self) -> LocalDriveItemId;
    fn icon(&self, appearance: &Appearance, color: Option<Fill>) -> Option<Box<dyn Element>> {
        let object_type = self.object_type()?;
        let icon_fill = color.unwrap_or(local_drive_icon_color(appearance, object_type).into());
        Some(Icon::from(object_type).to_warpui_icon(icon_fill).finish())
    }

    /// If implemented, returns a string that summarizes the primary action history. For example, "Run 2 times in the last week"
    fn action_summary(&self, app: &AppContext) -> Option<String>;

    /// Returns Some(true) if this is an open folder, Some(false) if closed folder, None if not a folder
    fn is_folder_open(&self) -> Option<bool> {
        None
    }

    fn clone_box(&self) -> Box<dyn LocalDriveItem>;
}

impl LocalDriveItemId {
    pub fn drive_row_position_id(&self) -> String {
        match self {
            Self::AIFactCollection => "AI_fact_collection".to_string(),
            Self::MCPServerCollection => "MCP_server_collection".to_string(),
            Self::Object(object_id) => object_id.drive_row_position_id(),
            Self::Space(space) => {
                local_drive_section_header_position_id(&DriveIndexSection::Space(*space))
            }
            Self::Trash => "Trash".to_string(),
        }
    }
}
/// This uniquely identifies an item in the local Drive index
/// Includes spaces (which ObjectTypeAndId does not entail)
#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub enum LocalDriveItemId {
    AIFactCollection,
    MCPServerCollection,
    Object(ObjectTypeAndId),
    Space(Space),
    Trash,
}

impl Clone for Box<dyn LocalDriveItem> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}
