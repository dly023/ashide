#[cfg(not(target_family = "wasm"))]
use crate::ai::mcp::templatable::{TemplatableMCPServer, TemplatableMCPServerObjectModel};
use crate::{
    ai::{
        execution_profiles::{AIExecutionProfile, AIExecutionProfileObjectModel},
        facts::{AIFact, AIFactObjectModel},
    },
    auth::TEST_USER_UID,
    drive::{folders::FolderObjectModel, ObjectTypeAndId},
    env_vars::{EnvVarCollection, EnvVarCollectionObjectModel},
    notebooks::{NotebookId, NotebookObjectModel},
    object_store::ids::{
        ClientId, HashableId, ObjectStoreId, ObjectUid, StableObjectId, ToStableObjectId,
    },
    object_store::{
        model::{
            actions::{ObjectAction, ObjectActionHistory, ObjectActionType, ObjectActions},
            generic_string_model::{
                GenericStringModel, GenericStringObjectId, Serializer, StringModel,
            },
            persistence::{ObjectStoreEvent, ObjectStoreModel, UpdateSource},
        },
        GenericStoredObject, GenericStringObjectFormat, JsonObjectType, ObjectIdType, ObjectType,
        Owner, Revision, Space, StoredObject, StoredObjectEventEntrypoint, StoredObjectLocation,
        StoredObjectModel,
    },
    persistence::ModelEvent,
    server_time::ServerTimestamp,
    workflows::{
        workflow::Workflow,
        workflow_enum::{WorkflowEnum, WorkflowEnumObject, WorkflowEnumObjectModel},
        WorkflowId, WorkflowObjectModel,
    },
    workspaces::user_workspaces::UserWorkspaces,
};
use chrono::{DateTime, Utc};
use lazy_static::lazy_static;
use regex::Regex;
use std::collections::HashSet;
use std::sync::{mpsc::SyncSender, Arc};
use warpui::r#async::FutureId;
use warpui::AppContext;
use warpui::{Entity, ModelContext, SingletonEntity};

lazy_static! {
    static ref DUPLICATE_OBJECT_NAME_REGEX: Regex =
        Regex::new(r" \((\d+)\)$").expect("regex should not fail to compile");
}

#[derive(Debug, PartialEq)]
pub enum OperationSuccessType {
    Success,
    Failure,
    Rejection,
    Denied(String),
    FeatureNotAvailable,
}

#[derive(Debug, PartialEq)]
pub enum ObjectOperation {
    Create { initiated_by: InitiatedBy },
    Update,
    MoveToFolder,
    MoveToDrive,
    Trash,
    TakeEditAccess,
    Untrash,
    Delete { initiated_by: InitiatedBy },
    EmptyTrash,
    UpdatePermissions,
    Leave,
}

#[derive(Debug)]
pub struct ObjectOperationResult {
    pub success_type: OperationSuccessType,
    pub operation: ObjectOperation,
    pub client_id: Option<ClientId>,
    pub stable_id: Option<StableObjectId>,
    pub num_objects: Option<i32>, // counts number of objects (including descendants) deleted for permadeletion
}

#[derive(Debug)]
pub enum UpdateManagerEvent {
    ObjectOperationComplete { result: ObjectOperationResult },
    AmbientTaskUpdated { timestamp: DateTime<Utc> },
}

/// An enum that defines whether the action was initiated by the user or the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitiatedBy {
    User,
    System,
}
#[derive(Debug)]
pub struct GenericStringObjectInput<T, S>
where
    T: StringModel<
            StoredObjectType = GenericStoredObject<GenericStringObjectId, GenericStringModel<T, S>>,
        > + 'static,
    S: Serializer<T> + 'static,
{
    pub id: ClientId,
    pub model: GenericStringModel<T, S>,
    pub initial_folder_id: Option<ObjectStoreId>,
    pub entrypoint: StoredObjectEventEntrypoint,
}

/// The UpdateManager is responsible for delegating work
/// when there is an update to an object (e.g. via a user interaction or
/// a local persistence event). Specifically, it will
/// - write to SQLite
/// - interact with the ObjectStoreModel to update the in-memory state used by the object views
pub struct UpdateManager {
    model_event_sender: Option<SyncSender<ModelEvent>>,
    spawned_futures: Vec<FutureId>,
}

impl UpdateManager {
    pub fn new(
        model_event_sender: Option<SyncSender<ModelEvent>>,
        _ctx: &mut ModelContext<Self>,
    ) -> Self {
        Self {
            model_event_sender,
            spawned_futures: Default::default(),
        }
    }

    #[cfg(test)]
    pub fn mock(ctx: &mut ModelContext<Self>) -> Self {
        Self::new(None, ctx)
    }

    #[cfg(any(test, feature = "integration_tests"))]
    pub fn spawned_futures(&self) -> &[FutureId] {
        &self.spawned_futures
    }

    fn save_to_db(&self, events: impl IntoIterator<Item = ModelEvent>) {
        let model_event_sender = self.model_event_sender.clone();
        if let Some(model_event_sender) = &model_event_sender {
            for event in events {
                if let Err(e) = model_event_sender.send(event) {
                    log::error!("Error saving to database: {e:?}");
                }
            }
        }
    }

    fn save_in_memory_object_to_sqlite(
        &mut self,
        object_store_model: &ObjectStoreModel,
        uid: &ObjectUid,
    ) {
        if let Some(stored_object) = object_store_model.get_by_uid(uid) {
            self.save_to_db([stored_object.upsert_event()]);
        }
    }

    fn save_in_memory_object_metadata_to_sqlite(
        &mut self,
        object_store_model: &ObjectStoreModel,
        uid: &ObjectUid,
        hashed_sqlite_id: &str,
    ) {
        if let Some(stored_object) = object_store_model.get_by_uid(uid) {
            let metadata = stored_object.metadata().clone();
            let event = ModelEvent::UpdateObjectMetadata {
                id: hashed_sqlite_id.to_string(),
                metadata,
            };
            self.save_to_db([event]);
        }
    }

    fn object_store_id_event_parts(
        object_store_id: ObjectStoreId,
    ) -> (Option<ClientId>, Option<StableObjectId>) {
        match object_store_id {
            ObjectStoreId::ClientId(client_id) => (Some(client_id), None),
            ObjectStoreId::StableId(stable_id) => (None, Some(stable_id)),
        }
    }

    fn emit_object_operation_success(
        &self,
        operation: ObjectOperation,
        object_store_id: ObjectStoreId,
        num_objects: Option<i32>,
        ctx: &mut ModelContext<Self>,
    ) {
        let (client_id, stable_id) = Self::object_store_id_event_parts(object_store_id);
        ctx.emit(UpdateManagerEvent::ObjectOperationComplete {
            result: ObjectOperationResult {
                success_type: OperationSuccessType::Success,
                operation,
                client_id,
                stable_id,
                num_objects,
            },
        });
    }

    /// Replace an object's data with its conflicting version. If the object does not have a
    /// conflict, this has no effect.
    pub fn replace_object_with_conflict(&mut self, uid: &ObjectUid, ctx: &mut ModelContext<Self>) {
        let object_store_model_handle = ObjectStoreModel::handle(ctx);

        // Update the in-memory model first, and check for conflicts.
        let had_conflicts =
            object_store_model_handle.update(
                ctx,
                |object_store_model, ctx| match object_store_model.get_mut_by_uid(uid) {
                    Some(object) if object.has_conflicting_changes() => {
                        object.replace_object_with_conflict();
                        ctx.emit(ObjectStoreEvent::ObjectUpdated {
                            type_and_id: object.object_type_and_id(),
                            source: UpdateSource::External,
                        });
                        true
                    }
                    _ => false,
                },
            );

        // Update SQLite, but only if the in-memory model was updated.
        if had_conflicts {
            self.save_in_memory_object_to_sqlite(object_store_model_handle.as_ref(ctx), uid);
        }
    }

    pub fn update_ai_fact(
        &mut self,
        ai_fact: AIFact,
        ai_fact_id: ObjectStoreId,
        revision_ts: Option<Revision>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.update_object(
            AIFactObjectModel::new(ai_fact),
            ai_fact_id,
            revision_ts,
            ctx,
        );
    }

    #[cfg(not(target_family = "wasm"))]
    pub fn update_templatable_mcp_server(
        &mut self,
        templatable_mcp_server: TemplatableMCPServer,
        templatable_mcp_object_id: ObjectStoreId,
        revision_ts: Option<Revision>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.update_object(
            TemplatableMCPServerObjectModel::new(templatable_mcp_server),
            templatable_mcp_object_id,
            revision_ts,
            ctx,
        );
    }

    pub fn update_workflow(
        &mut self,
        workflow: Workflow,
        workflow_id: ObjectStoreId,
        revision_ts: Option<Revision>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.update_object(
            WorkflowObjectModel::new(workflow),
            workflow_id,
            revision_ts,
            ctx,
        );
    }

    pub fn update_workflow_enum(
        &mut self,
        workflow_enum: WorkflowEnum,
        workflow_enum_id: ObjectStoreId,
        revision_ts: Option<Revision>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.update_object(
            WorkflowEnumObjectModel::new(workflow_enum),
            workflow_enum_id,
            revision_ts,
            ctx,
        );
    }

    pub fn update_env_var_collection(
        &mut self,
        env_var_collection: EnvVarCollection,
        env_var_collection_id: ObjectStoreId,
        revision_ts: Option<Revision>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.update_object(
            EnvVarCollectionObjectModel::new(env_var_collection),
            env_var_collection_id,
            revision_ts,
            ctx,
        );
    }

    pub fn update_notebook_data(
        &mut self,
        data: Arc<String>,
        notebook_id: ObjectStoreId,
        ctx: &mut ModelContext<Self>,
    ) {
        let object_store_model = ObjectStoreModel::as_ref(ctx);
        let revision = object_store_model.current_revision(&notebook_id).cloned();
        if let Some(notebook) = object_store_model.get_notebook(&notebook_id) {
            let new_notebook = NotebookObjectModel {
                title: notebook.model().title.to_owned(),
                data: data.to_string(),
                ai_document_id: notebook.model().ai_document_id,
                conversation_id: notebook.model().conversation_id.clone(),
            };
            self.update_object(new_notebook, notebook_id, revision, ctx);
        } else {
            log::warn!("Expected notebook to be in model with id {notebook_id:?}");
        }
    }

    pub fn update_notebook_title(
        &mut self,
        title: Arc<String>,
        notebook_id: ObjectStoreId,
        ctx: &mut ModelContext<Self>,
    ) {
        let object_store_model = ObjectStoreModel::as_ref(ctx);
        let revision = object_store_model.current_revision(&notebook_id).cloned();
        if let Some(notebook) = object_store_model.get_notebook(&notebook_id) {
            let new_notebook = NotebookObjectModel {
                title: title.to_string(),
                data: notebook.model().data.to_owned(),
                ai_document_id: notebook.model().ai_document_id,
                conversation_id: notebook.model().conversation_id.clone(),
            };
            self.update_object(new_notebook, notebook_id, revision, ctx);
        } else {
            log::warn!("Expected notebook to be in model with id {notebook_id:?}");
        }
    }

    /// Applies a move directly to the local object store and persists it.
    fn apply_local_object_move(
        &mut self,
        uid: &ObjectUid,
        new_owner: Option<Owner>,
        new_folder: Option<ObjectStoreId>,
        clear_permissions_change: bool,
        ctx: &mut ModelContext<Self>,
    ) {
        ObjectStoreModel::handle(ctx).update(ctx, |object_store_model, ctx| {
            object_store_model.update_object_location(uid, new_owner, new_folder, ctx);
            if let Some(object) = object_store_model.get_mut_by_uid(uid) {
                object
                    .metadata_mut()
                    .pending_changes_statuses
                    .has_pending_metadata_change = false;
                if clear_permissions_change {
                    object
                        .metadata_mut()
                        .pending_changes_statuses
                        .has_pending_permissions_change = false;
                }
            }
        });
        self.save_in_memory_object_to_sqlite(ObjectStoreModel::as_ref(ctx), uid);
        ctx.notify();
    }

    /// Given a workflow_id and a destination owner, make a local copy of all referenced workflow enums.
    fn copy_workflow_enums_to_drive(
        &mut self,
        workflow_id: ObjectStoreId,
        owner: Owner,
        ctx: &mut ModelContext<Self>,
    ) -> Option<Workflow> {
        let workflow = ObjectStoreModel::as_ref(ctx).get_workflow(&workflow_id);

        if let Some(workflow) = workflow {
            let original_workflow = workflow.model().data.clone();
            let mut workflow_model = original_workflow.clone();

            // Duplicate all enums associated with the workflow
            let enums = workflow_model.get_enum_ids();
            for enum_id in enums.iter() {
                let object_store_model = ObjectStoreModel::as_ref(ctx);
                let object: Option<&WorkflowEnumObject> =
                    object_store_model.get_object_of_type(enum_id);
                let Some(object) = object else {
                    log::error!("Could not find referenced workflow enum to copy over to the new space, skipping");
                    continue;
                };

                let client_id = ClientId::new();

                // Create a duplicate enum in the new space with a new client ID
                self.create_object(
                    object.model().clone(),
                    owner,
                    client_id,
                    StoredObjectEventEntrypoint::Unknown,
                    true,
                    None,
                    // When adding the initiated_by parameter to this function call, InitiatedBy::User was set as a default value.
                    // This can be changed to InitiatedBy::System if this action was automatically kicked off by the system and we do not want a user facing toast.
                    InitiatedBy::User,
                    ctx,
                );

                workflow_model.replace_object_id(*enum_id, ObjectStoreId::ClientId(client_id));
            }

            // Update the workflow with the new enum IDs, if there are any
            if !enums.is_empty() {
                self.update_workflow(workflow_model, workflow_id, None, ctx);
                Some(original_workflow)
            } else {
                None
            }
        } else {
            log::error!(
                "Tried to move workflow enums to new space but could not find associated workflow",
            );
            None
        }
    }

    /// Moves an object in the local object store and persists the new location.
    pub fn move_object_to_location(
        &mut self,
        object_id: ObjectTypeAndId,
        new_location: StoredObjectLocation,
        ctx: &mut ModelContext<Self>,
    ) {
        // If we are moving into the trash, we really mean to trash the object
        if let StoredObjectLocation::Trash = new_location {
            return self.trash_object(object_id, ctx);
        }

        let uid = object_id.uid();
        let object_store_id = object_id.object_store_id();

        let Some((object_current_owner, object_type)) =
            ObjectStoreModel::handle(ctx).read(ctx, |model, _| {
                let object = model.get_by_uid(&uid)?;
                Some((object.permissions().owner, object.object_type()))
            })
        else {
            return;
        };

        match new_location {
            StoredObjectLocation::Space(destination_space) => {
                let Some(destination_owner) =
                    UserWorkspaces::as_ref(ctx).space_to_owner(destination_space, ctx)
                else {
                    return;
                };

                if destination_owner != object_current_owner && object_type == ObjectType::Workflow
                {
                    let _ =
                        self.copy_workflow_enums_to_drive(object_store_id, destination_owner, ctx);
                }

                let clear_permissions_change = destination_owner != object_current_owner;
                self.apply_local_object_move(
                    &uid,
                    Some(destination_owner),
                    None,
                    clear_permissions_change,
                    ctx,
                );
                let operation = if clear_permissions_change {
                    ObjectOperation::MoveToDrive
                } else {
                    ObjectOperation::MoveToFolder
                };
                self.emit_object_operation_success(operation, object_store_id, None, ctx);
            }
            StoredObjectLocation::Folder(destination_folder_id) => {
                self.apply_local_object_move(&uid, None, Some(destination_folder_id), false, ctx);
                self.emit_object_operation_success(
                    ObjectOperation::MoveToFolder,
                    object_store_id,
                    None,
                    ctx,
                );
            }
            StoredObjectLocation::Trash => unreachable!("trash handled before move dispatch"),
        }
    }

    pub fn duplicate_object(
        &mut self,
        object_type_and_id: &ObjectTypeAndId,
        ctx: &mut ModelContext<Self>,
    ) {
        match object_type_and_id {
            ObjectTypeAndId::Notebook(notebook_id) => {
                self.duplicate_object_internal::<NotebookId, NotebookObjectModel>(notebook_id, ctx);
            }
            ObjectTypeAndId::Workflow(workflow_id) => {
                self.duplicate_object_internal::<WorkflowId, WorkflowObjectModel>(workflow_id, ctx);
            }
            ObjectTypeAndId::GenericStringObject { object_type, id } => {
                if let GenericStringObjectFormat::Json(JsonObjectType::EnvVarCollection) =
                    object_type
                {
                    self.duplicate_object_internal::<GenericStringObjectId, EnvVarCollectionObjectModel>(
                        id, ctx,
                    );
                } else {
                    log::error!("Tried to duplicate an unsupported type: json object");
                    debug_assert!(false, "Tried to duplicate an unsupported type: json object");
                }
            }
            ObjectTypeAndId::Folder(_) => {
                // Duplicating folders not currently supported.
                log::error!("Tried to duplicate an unsupported type: folder");
                debug_assert!(false, "Tried to duplicate an unsupported type: folder");
            }
        }
    }

    fn duplicate_object_internal<K, M>(&mut self, id: &ObjectStoreId, ctx: &mut ModelContext<Self>)
    where
        K: HashableId
            + ToStableObjectId
            + std::fmt::Debug
            + Into<String>
            + Clone
            + Copy
            + Send
            + Sync
            + 'static,
        M: StoredObjectModel<IdType = K, StoredObjectType = GenericStoredObject<K, M>> + 'static,
    {
        let (duplicate_model, client_id, owner, initial_folder_id, entrypoint) = {
            let object_store_model = ObjectStoreModel::as_ref(ctx);
            let object: GenericStoredObject<K, M> = object_store_model
                .get_object_of_type(id)
                .expect("object should exist in order to be duplicated")
                .clone();
            let client_id = ClientId::new();
            let owner = object.permissions.owner;
            let initial_folder_id = object.metadata.folder_id;
            let entrypoint = StoredObjectEventEntrypoint::Unknown;
            let mut duplicate_model = object.model().clone();
            let duplicate_name = self.get_next_duplicate_object_name(
                &object as &dyn StoredObject,
                object_store_model,
                ctx,
            );
            duplicate_model.set_display_name(&duplicate_name);
            (
                duplicate_model,
                client_id,
                owner,
                initial_folder_id,
                entrypoint,
            )
        };
        self.create_object(
            duplicate_model,
            owner,
            client_id,
            entrypoint,
            true,
            initial_folder_id,
            // When adding the initiated_by parameter to this function call, InitiatedBy::User was set as a default value.
            // This can be changed to InitiatedBy::System if this action was automatically kicked off by the system and we do not want a user facing toast.
            InitiatedBy::User,
            ctx,
        );
    }

    pub fn create_ai_fact(
        &mut self,
        ai_fact: AIFact,
        client_id: ClientId,
        owner: Owner,
        ctx: &mut ModelContext<Self>,
    ) {
        self.create_object(
            AIFactObjectModel::new(ai_fact),
            owner,
            client_id,
            Default::default(),
            false,
            None,
            // When adding the initiated_by parameter to this function call, InitiatedBy::User was set as a default value.
            // This can be changed to InitiatedBy::System if this action was automatically kicked off by the system and we do not want a user facing toast.
            InitiatedBy::User,
            ctx,
        );
    }

    #[cfg(not(target_family = "wasm"))]
    pub fn create_templatable_mcp_server(
        &mut self,
        templatable_mcp_server: TemplatableMCPServer,
        client_id: ClientId,
        owner: Owner,
        initiated_by: InitiatedBy,
        ctx: &mut ModelContext<Self>,
    ) {
        self.create_object(
            TemplatableMCPServerObjectModel::new(templatable_mcp_server),
            owner,
            client_id,
            Default::default(),
            false,
            None,
            initiated_by,
            ctx,
        );
    }

    pub fn create_ai_execution_profile(
        &mut self,
        ai_execution_profile: AIExecutionProfile,
        client_id: ClientId,
        owner: Owner,
        ctx: &mut ModelContext<Self>,
    ) {
        self.create_object(
            AIExecutionProfileObjectModel::new(ai_execution_profile),
            owner,
            client_id,
            Default::default(),
            false,
            None,
            // When adding the initiated_by parameter to this function call, InitiatedBy::User was set as a default value.
            // This can be changed to InitiatedBy::System if this action was automatically kicked off by the system and we do not want a user facing toast.
            InitiatedBy::User,
            ctx,
        );
    }

    pub fn update_ai_execution_profile(
        &mut self,
        ai_execution_profile: AIExecutionProfile,
        ai_execution_profile_id: ObjectStoreId,
        revision_ts: Option<Revision>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.update_object(
            AIExecutionProfileObjectModel::new(ai_execution_profile),
            ai_execution_profile_id,
            revision_ts,
            ctx,
        );
    }

    pub fn delete_ai_execution_profile(
        &mut self,
        ai_execution_profile_id: ObjectStoreId,
        ctx: &mut ModelContext<Self>,
    ) {
        self.delete_object_by_user(
            ObjectTypeAndId::GenericStringObject {
                object_type: GenericStringObjectFormat::Json(JsonObjectType::AIExecutionProfile),
                id: ai_execution_profile_id,
            },
            ctx,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_notebook(
        &mut self,
        client_id: ClientId,
        owner: Owner,
        initial_folder_id: Option<ObjectStoreId>,
        model: NotebookObjectModel,
        entrypoint: StoredObjectEventEntrypoint,
        force_expand: bool,
        ctx: &mut ModelContext<Self>,
    ) {
        self.create_object(
            model,
            owner,
            client_id,
            entrypoint,
            force_expand,
            initial_folder_id,
            // When adding the initiated_by parameter to this function call, InitiatedBy::User was set as a default value.
            // This can be changed to InitiatedBy::System if this action was automatically kicked off by the system and we do not want a user facing toast.
            InitiatedBy::User,
            ctx,
        );
    }

    fn get_next_duplicate_object_name(
        &self,
        original_stored_object: &dyn StoredObject,
        object_store_model: &ObjectStoreModel,
        app: &AppContext,
    ) -> String {
        let original_name = original_stored_object.display_name();

        // Iterate through items in the same folder as the original object that are of the
        // same type, and populate a hashset with those names.
        let same_type_and_folder_names = object_store_model
            .active_stored_objects_in_location_without_descendents(
                original_stored_object.location(object_store_model, app),
                app,
            )
            .filter(|&object| object.object_type() == original_stored_object.object_type())
            .map(|object| object.display_name())
            .collect::<HashSet<String>>();

        // Start with "{original_object_name} ({original_object_name's count + 1})".
        // Keep incrementing by one if there already exists an object of the same type in
        // the same folder (using the hashset generated above).
        let mut duplicate_name = get_duplicate_object_name(&original_name);
        while same_type_and_folder_names.contains(&duplicate_name) {
            duplicate_name = get_duplicate_object_name(&duplicate_name);
        }
        duplicate_name
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_workflow(
        &mut self,
        workflow: Workflow,
        owner: Owner,
        initial_folder_id: Option<ObjectStoreId>,
        client_id: ClientId,
        entrypoint: StoredObjectEventEntrypoint,
        force_expand: bool,
        ctx: &mut ModelContext<Self>,
    ) {
        self.create_object(
            WorkflowObjectModel::new(workflow),
            owner,
            client_id,
            entrypoint,
            force_expand,
            initial_folder_id,
            // When adding the initiated_by parameter to this function call, InitiatedBy::User was set as a default value.
            // This can be changed to InitiatedBy::System if this action was automatically kicked off by the system and we do not want a user facing toast.
            InitiatedBy::User,
            ctx,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_workflow_enum(
        &mut self,
        workflow_enum: WorkflowEnum,
        owner: Owner,
        client_id: ClientId,
        entrypoint: StoredObjectEventEntrypoint,
        force_expand: bool,
        ctx: &mut ModelContext<Self>,
    ) {
        self.create_object(
            WorkflowEnumObjectModel::new(workflow_enum),
            owner,
            client_id,
            entrypoint,
            force_expand,
            None,
            // When adding the initiated_by parameter to this function call, InitiatedBy::User was set as a default value.
            // This can be changed to InitiatedBy::System if this action was automatically kicked off by the system and we do not want a user facing toast.
            InitiatedBy::User,
            ctx,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_env_var_collection(
        &mut self,
        client_id: ClientId,
        owner: Owner,
        initial_folder_id: Option<ObjectStoreId>,
        model: EnvVarCollectionObjectModel,
        entrypoint: StoredObjectEventEntrypoint,
        force_expand: bool,
        ctx: &mut ModelContext<Self>,
    ) {
        self.create_object(
            model,
            owner,
            client_id,
            entrypoint,
            force_expand,
            initial_folder_id,
            // When adding the initiated_by parameter to this function call, InitiatedBy::User was set as a default value.
            // This can be changed to InitiatedBy::System if this action was automatically kicked off by the system and we do not want a user facing toast.
            InitiatedBy::User,
            ctx,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_folder(
        &mut self,
        name: String,
        owner: Owner,
        client_id: ClientId,
        initial_folder_id: Option<ObjectStoreId>,
        force_expand: bool,
        initiated_by: InitiatedBy,
        ctx: &mut ModelContext<Self>,
    ) {
        self.create_object(
            // TODO(INT-789): support creating folders as warp packs
            FolderObjectModel::new(&name, false),
            owner,
            client_id,
            Default::default(),
            force_expand,
            initial_folder_id,
            initiated_by,
            ctx,
        );
    }

    /// Creates a local stored object and persists it to SQLite.
    #[allow(clippy::too_many_arguments)]
    pub fn create_object<K, M>(
        &mut self,
        model: M,
        owner: Owner,
        client_id: ClientId,
        entrypoint: StoredObjectEventEntrypoint,
        force_expand: bool,
        initial_folder_id: Option<ObjectStoreId>,
        initiated_by: InitiatedBy,
        ctx: &mut ModelContext<Self>,
    ) where
        K: HashableId
            + ToStableObjectId
            + std::fmt::Debug
            + Into<String>
            + Clone
            + Copy
            + Send
            + Sync
            + 'static,
        M: StoredObjectModel<IdType = K, StoredObjectType = GenericStoredObject<K, M>> + 'static,
    {
        let _ = entrypoint;
        let _ = initiated_by;

        let object_id = ObjectStoreId::ClientId(client_id);
        let initial_editor_uid = TEST_USER_UID.to_string();

        // Update in-memory model.
        ObjectStoreModel::handle(ctx).update(ctx, move |object_store_model, ctx| {
            let mut object = GenericStoredObject::<K, M>::new_local(
                model.clone(),
                owner,
                initial_folder_id,
                client_id,
            );
            object.metadata.current_editor_uid = Some(initial_editor_uid.clone());
            object_store_model.create_object(object_id, object, ctx);

            if force_expand {
                object_store_model.force_expand_object_and_ancestors(object_id, ctx);
            }
        });

        // Update sqlite.
        let object_store_model = ObjectStoreModel::as_ref(ctx);
        if let Some(object) = object_store_model.get_object_of_type::<K, M>(&object_id) {
            self.save_to_db([object.upsert_event()]);
        }
    }

    /// Updates a local stored object and persists it to SQLite.
    pub fn update_object<K, M>(
        &mut self,
        model: M,
        object_id: ObjectStoreId,
        revision_ts: Option<Revision>,
        ctx: &mut ModelContext<Self>,
    ) where
        K: HashableId
            + ToStableObjectId
            + std::fmt::Debug
            + Into<String>
            + Clone
            + Copy
            + Send
            + Sync
            + 'static,
        M: StoredObjectModel<IdType = K, StoredObjectType = GenericStoredObject<K, M>> + 'static,
    {
        let _ = revision_ts;

        // Update in-memory model.
        ObjectStoreModel::handle(ctx).update(ctx, |object_store_model, ctx| {
            object_store_model.update_object_from_edit(model.clone(), object_id, ctx);
            ctx.notify();
        });

        // Update sqlite.
        let object_store_model = ObjectStoreModel::as_ref(ctx);
        if let Some(object) = object_store_model.get_object_of_type::<K, M>(&object_id) {
            self.save_to_db([object.upsert_event()]);
        };
    }

    // Takes a generic ObjectStoreId and records the action.
    pub fn record_object_action(
        &mut self,
        id_and_type: ObjectTypeAndId,
        action_type: ObjectActionType,
        data: Option<String>,
        ctx: &mut ModelContext<Self>,
    ) {
        // Take the action timestamp from the client.
        let action_timestamp = Utc::now();

        // Update in-memory model.
        let object_action = ObjectActions::handle(ctx).update(ctx, |object_actions_model, ctx| {
            object_actions_model.insert_action(
                id_and_type.uid(),
                id_and_type.sqlite_uid_hash(),
                action_type.clone(),
                data.clone(),
                action_timestamp,
                ctx,
            )
        });

        // Update sqlite.
        self.save_to_db([ModelEvent::InsertObjectAction { object_action }]);

        let _ = (id_and_type, action_type, data, action_timestamp);
    }

    fn maybe_overwrite_object_action_history(
        &mut self,
        history: &ObjectActionHistory,
        ctx: &mut ModelContext<Self>,
    ) {
        ObjectActions::handle(ctx).update(ctx, |object_actions_model, ctx| {
            // Accept this action history if we don't have any actions for this object OR the server's latest action
            // for this object is at least as recent as our latest synced action for this object
            let latest_processed_at_ts =
                object_actions_model.get_latest_processed_at_ts(&history.uid);
            if latest_processed_at_ts
                .is_none_or(|client_ts| client_ts <= history.latest_processed_at_timestamp)
            {
                // Overwrite the history for this object.
                object_actions_model.overwrite_action_history_for_object(
                    &history.uid,
                    history.actions.clone(),
                    ctx,
                );
            }
        });
    }

    /// Overwrites the actions in SQLite for a specified set of objects with the actions that
    /// are currently in the ObjectActions singleton model.
    fn sync_actions_for_objects_to_sqlite(
        &mut self,
        object_uids: Vec<&ObjectUid>,
        ctx: &mut ModelContext<Self>,
    ) {
        // Retrieve the objects from the ObjectActions model
        let actions = ObjectActions::handle(ctx).read(ctx, |object_actions_model, _ctx| {
            object_actions_model.get_actions_for_objects(object_uids)
        });

        // Overwrite the actions for those objects in sqlite
        let actions_to_sync: Vec<ObjectAction> = actions.values().flatten().cloned().collect();
        self.save_to_db([ModelEvent::SyncObjectActions { actions_to_sync }]);
    }

    /// Sets the notebook's current editor in local memory.
    fn set_notebook_current_editor(
        &self,
        notebook_id: &ObjectStoreId,
        editor_uid: Option<String>,
        ctx: &mut ModelContext<Self>,
    ) {
        ObjectStoreModel::handle(ctx).update(ctx, |object_store_model, ctx| {
            if let Some(notebook) = object_store_model.get_notebook_mut(notebook_id) {
                notebook.metadata.set_current_editor(editor_uid);
                ctx.notify();
            }
        });
    }

    /// Grants local notebook edit access.
    pub fn grab_notebook_edit_access(
        &mut self,
        notebook_id: ObjectStoreId,
        _optimistically_grant_access: bool,
        ctx: &mut ModelContext<Self>,
    ) {
        self.set_notebook_current_editor(&notebook_id, Some(TEST_USER_UID.to_string()), ctx);
    }

    /// Gives up local notebook edit access.
    pub fn give_up_notebook_edit_access(
        &mut self,
        notebook_id: ObjectStoreId,
        ctx: &mut ModelContext<Self>,
    ) {
        self.set_notebook_current_editor(&notebook_id, None, ctx);
    }

    fn mark_object_trashed(&self, uid: &ObjectUid, ctx: &mut ModelContext<Self>) {
        let timestamp = ServerTimestamp::new(Utc::now());
        ObjectStoreModel::handle(ctx).update(ctx, |object_store_model, ctx| {
            if let Some(object) = object_store_model.get_mut_by_uid(uid) {
                object.metadata_mut().trashed_ts = Some(timestamp);
                object
                    .metadata_mut()
                    .pending_changes_statuses
                    .has_pending_metadata_change = false;
                ctx.emit(ObjectStoreEvent::ObjectTrashed {
                    type_and_id: object.object_type_and_id(),
                    source: UpdateSource::Local,
                });
                ctx.notify();
            }
        });
    }

    pub fn trash_object(&mut self, id: ObjectTypeAndId, ctx: &mut ModelContext<Self>) {
        let uid = id.uid();
        let object_store_id = id.object_store_id();
        self.mark_object_trashed(&uid, ctx);
        ObjectStoreModel::handle(ctx).update(ctx, |object_store_model, _| {
            self.save_in_memory_object_to_sqlite(object_store_model, &uid);
        });
        self.emit_object_operation_success(ObjectOperation::Trash, object_store_id, None, ctx);
        ctx.notify();
    }

    pub fn untrash_object(&mut self, id: ObjectTypeAndId, ctx: &mut ModelContext<Self>) {
        let uid = id.uid();
        let object_store_id = id.object_store_id();
        ObjectStoreModel::handle(ctx).update(ctx, |object_store_model, ctx| {
            if let Some(object) = object_store_model.get_mut_by_uid(&uid) {
                object.metadata_mut().trashed_ts = None;
                object
                    .metadata_mut()
                    .pending_changes_statuses
                    .has_pending_metadata_change = false;
                object
                    .metadata_mut()
                    .pending_changes_statuses
                    .pending_untrash = false;
                ctx.emit(ObjectStoreEvent::ObjectUntrashed {
                    type_and_id: object.object_type_and_id(),
                    source: UpdateSource::Local,
                });
            }
            self.save_in_memory_object_to_sqlite(object_store_model, &uid);
        });
        self.emit_object_operation_success(ObjectOperation::Untrash, object_store_id, None, ctx);
        ctx.notify();
    }

    pub fn delete_object_by_user(&mut self, id: ObjectTypeAndId, ctx: &mut ModelContext<Self>) {
        self.delete_object_with_initiated_by(id, InitiatedBy::User, ctx);
    }

    pub fn delete_object_with_initiated_by(
        &mut self,
        id: ObjectTypeAndId,
        initiated_by: InitiatedBy,
        ctx: &mut ModelContext<Self>,
    ) {
        let uid = id.uid();
        let Some(object_store_id) = ObjectStoreModel::handle(ctx).read(ctx, |model, _| {
            model
                .get_by_uid(&uid)
                .map(|object| object.object_store_id())
        }) else {
            return;
        };

        let num_deleted_objects = self.on_object_delete_success(vec![object_store_id], ctx);
        let (client_id, stable_id) = Self::object_store_id_event_parts(object_store_id);
        ctx.emit(UpdateManagerEvent::ObjectOperationComplete {
            result: ObjectOperationResult {
                success_type: OperationSuccessType::Success,
                operation: ObjectOperation::Delete { initiated_by },
                client_id,
                stable_id,
                num_objects: Some(num_deleted_objects),
            },
        });
        ctx.notify();
    }

    pub fn empty_trash(&mut self, space: Space, ctx: &mut ModelContext<Self>) {
        let owner = match UserWorkspaces::as_ref(ctx).space_to_owner(space, ctx) {
            Some(owner) => owner,
            None => return,
        };

        let object_store_model_handle = ObjectStoreModel::handle(ctx);
        let deleted_ids: Vec<ObjectStoreId> =
            object_store_model_handle.read(ctx, |object_store_model, _| {
                object_store_model
                    .stored_objects()
                    .filter(|object| {
                        object.permissions().owner == owner && object.is_trashed(object_store_model)
                    })
                    .map(|object| object.object_store_id())
                    .collect()
            });

        let num_deleted_objects = self.on_object_delete_success(deleted_ids, ctx);

        let success_type = if num_deleted_objects == 0 {
            OperationSuccessType::Rejection
        } else {
            OperationSuccessType::Success
        };

        ctx.emit(UpdateManagerEvent::ObjectOperationComplete {
            result: ObjectOperationResult {
                success_type,
                operation: ObjectOperation::EmptyTrash,
                client_id: None,
                stable_id: None,
                num_objects: Some(num_deleted_objects),
            },
        });
        ctx.notify();
    }

    pub fn on_object_delete_success(
        &mut self,
        deleted_ids: Vec<ObjectStoreId>,
        ctx: &mut ModelContext<'_, UpdateManager>,
    ) -> i32 {
        let object_store_model_handle = ObjectStoreModel::handle(ctx);
        let all_object_uids: Vec<ObjectUid> = deleted_ids.iter().map(|&id| id.uid()).collect();

        // This variable counts the number of objects deleted in each Empty Trash action.
        let mut num_deleted_objects = 0;
        let mut object_store_ids_and_types: Vec<(ObjectStoreId, ObjectIdType)> = Vec::new();
        object_store_model_handle.update(ctx, |object_store_model, ctx| {
            (object_store_ids_and_types, num_deleted_objects) =
                object_store_model.delete_objects_by_id(all_object_uids.clone(), ctx);
        });

        // Deleted the actions associated with these objects too.
        ObjectActions::handle(ctx).update(ctx, |object_actions, ctx| {
            for uid in all_object_uids.clone() {
                object_actions.delete_actions_for_object(&uid, ctx);
            }
        });

        // Return early if empty
        if num_deleted_objects == 0 {
            return num_deleted_objects;
        }

        // Delete objects from sqlite. This will also delete their actions.
        self.save_to_db([ModelEvent::DeleteObjects {
            ids: object_store_ids_and_types,
        }]);

        num_deleted_objects
    }

    pub fn rename_folder(
        &mut self,
        folder_id: ObjectStoreId,
        new_name: String,
        ctx: &mut ModelContext<Self>,
    ) {
        let object_store_model = ObjectStoreModel::as_ref(ctx);
        let revision = object_store_model.current_revision(&folder_id).cloned();
        if let Some(folder) = object_store_model.get_folder(&folder_id) {
            let new_folder = FolderObjectModel {
                name: new_name,
                is_open: folder.model().is_open,
                is_warp_pack: folder.model().is_warp_pack,
            };
            self.update_object(new_folder, folder_id, revision, ctx);
        } else {
            log::warn!("Attempted to rename folder that doesn't exist with id: {folder_id:?}");
        }
    }
}

/// Return the newly duplicated object's name based on the original object's name. E.g.:
/// - "my object name" -> "my object name (1)"
pub fn get_duplicate_object_name(original_name: &str) -> String {
    match DUPLICATE_OBJECT_NAME_REGEX
        .captures(original_name)
        .and_then(|caps| caps.get(1))
        .and_then(|num| num.as_str().parse::<usize>().ok())
    {
        Some(num) => {
            let new_num = num.saturating_add(1);

            // edge case check for when the duplicate number is usize::MAX
            if new_num == usize::MAX {
                format!("{original_name} (1)")
            } else {
                DUPLICATE_OBJECT_NAME_REGEX
                    .replace(original_name, format!(" ({new_num})"))
                    .to_string()
            }
        }
        None => format!("{original_name} (1)"),
    }
}

impl Entity for UpdateManager {
    type Event = UpdateManagerEvent;
}

impl SingletonEntity for UpdateManager {}
