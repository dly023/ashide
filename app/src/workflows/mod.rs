use serde::{Deserialize, Serialize};
use warp_core::context_flag::ContextFlag;
use warpui::AppContext;

pub mod categories;
use workflow::Workflow;

pub mod aliases;
pub mod command_parser;
pub mod export_workflow;
pub mod info_box;
pub mod local_workflows;
pub mod manager;
pub mod workflow;
pub mod workflow_enum;
pub mod workflow_view;

use crate::appearance::Appearance;
use crate::object_store::{GenericStoredObject, ObjectType, StoredObjectModel};

use crate::drive::items::workflow::LocalDriveWorkflow;
use crate::drive::items::LocalDriveItem;
use crate::drive::ObjectTypeAndId;
use crate::notebooks::{NotebookId, NotebookLocation};
use crate::object_store::ids::{ObjectStoreId, StableObjectId};
use crate::object_store::SerializedModel;
use crate::persistence::ModelEvent;
pub use categories::{CategoriesView, CategoriesViewEvent, WorkflowsViewAction};

pub fn init(app: &mut AppContext) {
    categories::init(app);
    self::workflow_view::init(app);
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize, Eq, PartialEq, Hash)]
pub enum WorkflowSource {
    Global,
    Local,
    Project,
    PersonalDrive,
    AiAssistant,
    Notebook {
        notebook_id: Option<NotebookId>,
        location: NotebookLocation,
    },

    /// A hardcoded workflow type that allows Ashide to surface features as Workflows (e.g.
    /// a command to see our network log)
    App,
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize, Eq, PartialEq, Hash, PartialOrd)]
pub enum WorkflowSelectionSource {
    LocalDrive,
    CommandPalette,
    UniversalSearch,
    Voltron,
    AiAssistant,
    Notebook,
    SlashMenu,
    UpArrowHistory,
    WorkflowView,
    AgentMode,
    Undefined,
    Alias,
}

#[derive(Debug, Clone, Copy)]
pub enum WorkflowViewMode {
    View,
    Edit,
    Create,
}

impl WorkflowViewMode {
    pub fn supported_edit_mode() -> Self {
        Self::Edit
    }

    pub fn supported_view_mode() -> Self {
        if ContextFlag::RunWorkflow.is_enabled() {
            Self::Edit
        } else {
            Self::View
        }
    }

    fn is_editable(&self) -> bool {
        match self {
            Self::View => false,
            Self::Edit | Self::Create => true,
        }
    }
}

#[derive(Clone, Debug, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct WorkflowId(StableObjectId);
crate::stable_object_id_traits! { WorkflowId, "Workflow" }

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AIWorkflowOrigin {
    CommandSearch,
    AgentMode,
    LegacyAiAssistant,
}

/// Wrapper type for a workflow that may be saved locally or in the object store.
#[derive(Clone, Debug, PartialEq)]
pub enum WorkflowType {
    /// Saved workflows sourced from local, global, project, app collections, saved locally.
    Local(Workflow),
    /// Saved workflows from the local object store.
    ObjectStore(Box<WorkflowObject>),
    /// Ephemeral/transient workflows created from Ashide AI output
    AIGenerated {
        workflow: Workflow,
        origin: AIWorkflowOrigin,
    },
    /// A workflow that's part of an object-store notebook.
    Notebook(Workflow),
}

impl WorkflowType {
    pub fn as_workflow(&self) -> &Workflow {
        match self {
            WorkflowType::Local(workflow) => workflow,
            WorkflowType::AIGenerated { workflow, .. } => workflow,
            WorkflowType::ObjectStore(workflow) => &workflow.model().data,
            WorkflowType::Notebook(workflow) => workflow,
        }
    }

    /// Returns the contained [`Workflow`], consuming `self`.
    pub fn take_workflow(self) -> Workflow {
        match self {
            WorkflowType::Local(workflow) => workflow,
            WorkflowType::AIGenerated { workflow, .. } => workflow,
            WorkflowType::ObjectStore(workflow) => workflow.model().data.clone(),
            WorkflowType::Notebook(workflow) => workflow,
        }
    }

    /// The object type and ID for the object containing this workflow, if there is
    /// one. This is currently only supported for object-backed workflows, not workflows within notebooks.
    pub fn object_id(&self) -> Option<ObjectTypeAndId> {
        match self {
            WorkflowType::ObjectStore(workflow) => Some(ObjectTypeAndId::Workflow(workflow.id)),
            _ => None,
        }
    }

    pub fn object_store_id(&self) -> Option<ObjectStoreId> {
        match self {
            WorkflowType::ObjectStore(workflow) => Some(workflow.id),
            _ => None,
        }
    }

    /// We don't show env var selection for Agent Mode suggested commands.
    pub(super) fn should_show_env_var_selection(&self) -> bool {
        !matches!(self, WorkflowType::AIGenerated { .. },)
    }
}

/// The model for a `WorkflowObject`.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkflowObjectModel {
    pub data: Workflow,
}

impl WorkflowObjectModel {
    pub fn new(workflow: Workflow) -> Self {
        Self { data: workflow }
    }
}

/// `WorkflowObject` is an object-store backed workflow.
pub type WorkflowObject = GenericStoredObject<WorkflowId, WorkflowObjectModel>;

impl StoredObjectModel for WorkflowObjectModel {
    type StoredObjectType = WorkflowObject;
    type IdType = WorkflowId;

    fn model_type_name(&self) -> &'static str {
        if self.data.is_agent_mode_workflow() {
            "Prompt"
        } else {
            "Workflow"
        }
    }

    fn object_type(&self) -> ObjectType {
        ObjectType::Workflow
    }

    fn object_type_and_id(&self, id: ObjectStoreId) -> ObjectTypeAndId {
        ObjectTypeAndId::Workflow(id)
    }

    fn display_name(&self) -> String {
        self.data.name().to_string()
    }

    fn set_display_name(&mut self, name: &str) {
        self.data.set_name(name);
    }

    fn upsert_event(&self, workflow: &WorkflowObject) -> ModelEvent {
        ModelEvent::UpsertWorkflow {
            workflow: workflow.clone(),
        }
    }

    fn bulk_upsert_event(objects: &[WorkflowObject]) -> ModelEvent {
        ModelEvent::UpsertWorkflows(objects.to_vec())
    }

    fn should_update_after_stored_revision_conflict(&self) -> bool {
        true
    }

    fn serialized(&self) -> SerializedModel {
        SerializedModel::new(
            serde_json::to_string(&self.data).expect("failed to serialize workflow"),
        )
    }

    fn renders_in_local_drive(&self) -> bool {
        true
    }

    fn to_local_drive_item(
        &self,
        id: ObjectStoreId,
        _appearance: &Appearance,
        workflow: &WorkflowObject,
    ) -> Option<Box<dyn LocalDriveItem>> {
        Some(Box::new(LocalDriveWorkflow::new(
            self.object_type_and_id(id),
            workflow.clone(),
        )))
    }

    fn can_export(&self) -> bool {
        true
    }
}

impl PartialEq<Workflow> for WorkflowObject {
    fn eq(&self, other: &Workflow) -> bool {
        self.model().data == *other
    }
}

impl PartialEq<WorkflowObject> for WorkflowObject {
    fn eq(&self, other: &WorkflowObject) -> bool {
        self.model().data == other.model().data && self.id == other.id
    }
}

impl From<WorkflowObject> for Workflow {
    fn from(workflow: WorkflowObject) -> Self {
        workflow.model().data.clone()
    }
}

impl From<&WorkflowObject> for Workflow {
    fn from(workflow: &WorkflowObject) -> Self {
        workflow.model().data.to_owned()
    }
}

#[cfg(test)]
#[path = "mod_test.rs"]
mod tests;
