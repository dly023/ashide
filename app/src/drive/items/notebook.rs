use warpui::{
    elements::{Flex, ParentElement},
    fonts::Weight,
    ui_components::components::{UiComponent, UiComponentStyles},
    AppContext, Element,
};

use crate::{
    appearance::Appearance,
    drive::{index::DriveIndexAction, DriveObjectType, ObjectTypeAndId},
    notebooks::NotebookObject,
    object_store::StoredObjectMetadata,
    themes::theme::Fill,
};

use super::{LocalDriveItem, LocalDriveItemId};

#[derive(Clone)]
pub struct LocalDriveNotebook {
    id: ObjectTypeAndId,
    notebook: NotebookObject,
    is_ai_document: bool,
}

impl LocalDriveNotebook {
    pub fn new(id: ObjectTypeAndId, notebook: NotebookObject, is_ai_document: bool) -> Self {
        Self {
            id,
            notebook,
            is_ai_document,
        }
    }
}

impl LocalDriveItem for LocalDriveNotebook {
    fn display_name(&self) -> Option<String> {
        if self.notebook.model().title.is_empty() {
            None
        } else {
            Some(self.notebook.model().title.clone())
        }
    }

    fn metadata(&self) -> Option<&StoredObjectMetadata> {
        Some(&self.notebook.metadata)
    }

    fn object_type(&self) -> Option<DriveObjectType> {
        Some(DriveObjectType::Notebook {
            is_ai_document: self.is_ai_document,
        })
    }

    fn secondary_icon(&self, _color: Option<Fill>) -> Option<Box<dyn Element>> {
        None
    }

    fn click_action(&self) -> Option<DriveIndexAction> {
        Some(DriveIndexAction::OpenObject(self.id))
    }

    fn preview(&self, appearance: &Appearance) -> Option<Box<dyn Element>> {
        let title_text = self.notebook.model().title.clone();
        let title_to_render = if title_text.is_empty() {
            "Untitled".to_string()
        } else {
            title_text
        };
        let title = appearance
            .ui_builder()
            .wrappable_text(title_to_render, true)
            .with_style(UiComponentStyles {
                font_color: Some(
                    appearance
                        .theme()
                        .main_text_color(appearance.theme().background())
                        .into(),
                ),
                font_size: Some(14.),
                font_weight: Some(Weight::Bold),
                ..Default::default()
            })
            .build()
            .finish();

        Some(Flex::column().with_child(title).finish())
    }

    fn local_drive_id(&self) -> LocalDriveItemId {
        LocalDriveItemId::Object(self.id)
    }

    fn action_summary(&self, _app: &AppContext) -> Option<String> {
        None
    }

    fn clone_box(&self) -> Box<dyn LocalDriveItem> {
        Box::new(self.clone())
    }
}
