use warpui::{
    elements::{Container, Flex, ParentElement},
    fonts::Weight,
    ui_components::components::{UiComponent, UiComponentStyles},
    AppContext, Element,
};

use crate::{
    ai::facts::{AIFact, AIFactObject, AIMemory},
    appearance::Appearance,
    drive::{index::DriveIndexAction, DriveObjectType, ObjectTypeAndId},
    object_store::StoredObjectMetadata,
    themes::theme::Fill,
};

use super::{LocalDriveItem, LocalDriveItemId};

#[derive(Clone)]
pub struct LocalDriveAIFact {
    id: ObjectTypeAndId,
    ai_fact: AIFactObject,
}

impl LocalDriveAIFact {
    pub fn new(id: ObjectTypeAndId, ai_fact: AIFactObject) -> Self {
        Self { id, ai_fact }
    }
}

impl LocalDriveItem for LocalDriveAIFact {
    fn display_name(&self) -> Option<String> {
        match &self.ai_fact.model().string_model {
            AIFact::Memory(AIMemory { content, name, .. }) => {
                if let Some(name) = name {
                    if !name.is_empty() {
                        Some(name.clone())
                    } else {
                        Some(content.clone())
                    }
                } else {
                    Some(content.clone())
                }
            }
        }
    }
    fn metadata(&self) -> Option<&StoredObjectMetadata> {
        Some(&self.ai_fact.metadata)
    }

    fn object_type(&self) -> Option<DriveObjectType> {
        Some(DriveObjectType::AIFact)
    }

    fn secondary_icon(&self, _color: Option<Fill>) -> Option<Box<dyn Element>> {
        None
    }

    fn click_action(&self) -> Option<DriveIndexAction> {
        Some(DriveIndexAction::OpenAIFactCollection)
    }

    fn preview(&self, appearance: &Appearance) -> Option<Box<dyn Element>> {
        let title_to_render = match &self.ai_fact.model().string_model {
            AIFact::Memory(AIMemory { content, .. }) => content.clone(),
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

        Some(
            Flex::column()
                .with_child(Container::new(title).finish())
                .finish(),
        )
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
