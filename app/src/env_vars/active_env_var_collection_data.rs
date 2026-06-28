use crate::{
    drive::access::{ContentEditability, LocalObjectAccessLevel},
    env_vars::EnvVarCollectionObject,
    object_store::ids::{ClientId, ObjectStoreId},
    object_store::{
        breadcrumbs::ContainingObject,
        model::{persistence::ObjectStoreEvent, view::ObjectStoreViewModel},
        Owner, Revision, Space, StoredObject,
    },
    AppContext, ObjectStoreModel,
};

use warpui::{Entity, ModelContext, SingletonEntity};

use super::EnvVarCollectionObjectModel;

#[derive(Default, Clone)]
pub enum ActiveEnvVarCollection {
    #[default]
    None,
    // An EnvVarCollection already stored in ObjectStoreModel, all relevant data should be queried
    // from ObjectStoreModel directly
    CommittedEnvVarCollection(ObjectStoreId),
    // An EnvVarCollection that has been created and displayed in the view, but is not yet
    // committed to ObjectStoreModel
    NewEnvVarCollection(Box<EnvVarCollectionObject>),
}

#[derive(Default, PartialEq, Debug)]
pub enum SavingStatus {
    #[default]
    Saved,
    Unsaved,
    New,
}

#[derive(Default)]
pub struct ActiveEnvVarCollectionData {
    pub saving_status: SavingStatus,
    pub active_env_var_collection: ActiveEnvVarCollection,
    pub revision_ts: Option<Revision>,
}

impl ActiveEnvVarCollectionData {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        // The old remote operation completion subscription was removed with the
        // local-only object store. `ObjectStoreModel` subscription remains because
        // local object moves still refresh breadcrumbs.
        let object_store_model = ObjectStoreModel::handle(ctx);

        ctx.subscribe_to_model(&object_store_model, |me, event, ctx| {
            me.handle_object_store_event(event, ctx);
        });

        Self {
            ..Default::default()
        }
    }

    fn handle_object_store_event(
        &mut self,
        event: &ObjectStoreEvent,
        ctx: &mut ModelContext<Self>,
    ) {
        if let ObjectStoreEvent::ObjectMoved { type_and_id, .. } = event {
            if let Some(env_var_collection_id) = type_and_id.as_generic_string_object_id() {
                if self.is_active_env_var_collection(env_var_collection_id) {
                    ctx.emit(ActiveEnvVarCollectionDataEvent::BreadcrumbsChanged)
                }
            }
        }
    }

    pub fn reset(&mut self) {
        self.active_env_var_collection = ActiveEnvVarCollection::None;
    }

    pub fn open_new(
        &mut self,
        owner: Owner,
        initial_folder_id: Option<ObjectStoreId>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.reset();

        let new_id = ClientId::default();

        // Set the active env var collection to be an uncommitted collection
        self.active_env_var_collection = ActiveEnvVarCollection::NewEnvVarCollection(Box::new(
            EnvVarCollectionObject::new_local(
                EnvVarCollectionObjectModel::default(),
                owner,
                initial_folder_id,
                new_id,
            ),
        ));

        ctx.emit(ActiveEnvVarCollectionDataEvent::BreadcrumbsChanged);
        ctx.notify();
    }

    pub fn open_existing(
        &mut self,
        env_var_collection_id: ObjectStoreId,
        ctx: &mut ModelContext<Self>,
    ) {
        self.reset();
        self.saving_status = SavingStatus::Saved;
        self.active_env_var_collection =
            ActiveEnvVarCollection::CommittedEnvVarCollection(env_var_collection_id);

        ctx.emit(ActiveEnvVarCollectionDataEvent::BreadcrumbsChanged);
        ctx.notify();
    }

    pub fn id(&self) -> Option<ObjectStoreId> {
        match &self.active_env_var_collection {
            ActiveEnvVarCollection::None => None,
            ActiveEnvVarCollection::CommittedEnvVarCollection(id) => Some(*id),
            ActiveEnvVarCollection::NewEnvVarCollection(env_var_collection) => {
                Some(env_var_collection.id)
            }
        }
    }

    /// The current user's access level on this env var collection.
    pub fn access_level(&self, app: &AppContext) -> LocalObjectAccessLevel {
        match &self.active_env_var_collection {
            ActiveEnvVarCollection::CommittedEnvVarCollection(object_store_id) => {
                ObjectStoreViewModel::as_ref(app).access_level(&object_store_id.uid(), app)
            }
            ActiveEnvVarCollection::None | ActiveEnvVarCollection::NewEnvVarCollection(_) => {
                LocalObjectAccessLevel::Full
            }
        }
    }

    pub fn editability(&self, app: &AppContext) -> ContentEditability {
        match &self.active_env_var_collection {
            ActiveEnvVarCollection::CommittedEnvVarCollection(object_store_id) => {
                ObjectStoreViewModel::as_ref(app).object_editability(&object_store_id.uid(), app)
            }
            ActiveEnvVarCollection::None | ActiveEnvVarCollection::NewEnvVarCollection(_) => {
                ContentEditability::Editable
            }
        }
    }

    /// The space that this env var collection is in.
    pub fn space(&self, app: &AppContext) -> Option<Space> {
        match &self.active_env_var_collection {
            ActiveEnvVarCollection::None => None,
            ActiveEnvVarCollection::CommittedEnvVarCollection(object_store_id) => {
                ObjectStoreViewModel::as_ref(app).object_space(&object_store_id.uid(), app)
            }
            ActiveEnvVarCollection::NewEnvVarCollection(env_var_collection) => {
                Some(env_var_collection.space(app))
            }
        }
    }

    pub fn active_env_var_collection(&self) -> ActiveEnvVarCollection {
        self.active_env_var_collection.clone()
    }

    pub fn is_active_env_var_collection(&self, env_var_collection_id: ObjectStoreId) -> bool {
        self.id() == Some(env_var_collection_id)
    }

    pub fn breadcrumbs(&self, ctx: &AppContext) -> Option<Vec<ContainingObject>> {
        let local_env_var_collection = match &self.active_env_var_collection {
            ActiveEnvVarCollection::None => None,
            ActiveEnvVarCollection::CommittedEnvVarCollection(id) => {
                ObjectStoreModel::as_ref(ctx).get_env_var_collection(id)
            }
            ActiveEnvVarCollection::NewEnvVarCollection(env_var_collection) => {
                Some(env_var_collection.as_ref())
            }
        };

        local_env_var_collection
            .map(|env_var_collection| env_var_collection.containing_objects_path(ctx))
    }

    pub fn trash_status(&self, ctx: &AppContext) -> TrashStatus {
        match &self.active_env_var_collection {
            ActiveEnvVarCollection::None | ActiveEnvVarCollection::NewEnvVarCollection(_) => {
                TrashStatus::Active
            }
            ActiveEnvVarCollection::CommittedEnvVarCollection(id) => {
                let object_store_model = ObjectStoreModel::as_ref(ctx);
                match object_store_model.get_env_var_collection(id) {
                    Some(env_var_collection) => {
                        if env_var_collection.is_trashed(object_store_model) {
                            TrashStatus::Trashed
                        } else {
                            TrashStatus::Active
                        }
                    }
                    None => TrashStatus::Deleted,
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TrashStatus {
    Active,
    Trashed,
    Deleted,
}

pub enum ActiveEnvVarCollectionDataEvent {
    /// The EVC's breadcrumbs were updated.
    BreadcrumbsChanged,
    /// The EVC was trashed or untrashed
    /// (used for refreshing the pane overflow items)
    TrashStatusChanged,
}

impl Entity for ActiveEnvVarCollectionData {
    type Event = ActiveEnvVarCollectionDataEvent;
}
