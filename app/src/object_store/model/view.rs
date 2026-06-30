use std::{cell::RefCell, collections::HashMap};

use warpui::{AppContext, Entity, ModelContext, SingletonEntity};

use crate::{
    drive::{
        access::{ContentEditability, LocalObjectAccessLevel},
        folders::FolderObject,
    },
    object_store::ids::{ObjectStoreId, ObjectUid},
    object_store::{
        update_manager::{
            ObjectOperation, OperationSuccessType, UpdateManager, UpdateManagerEvent,
        },
        Space, StoredObject, StoredObjectLocation,
    },
    server_time::ServerTimestamp,
};

use super::persistence::{ObjectStoreEvent, ObjectStoreModel};

#[derive(Default, Clone, Debug, PartialEq)]
pub enum EditorState {
    #[default]
    None,
    CurrentUser,
    OtherUserActive,
    OtherUserIdle,
}

/// Stores information about the current editor of
/// a particular notebook, for display purposes.
/// For now, this just includes the state and
/// an email, but will eventually hold more information
/// about the user.
#[derive(Default, Clone, Debug, PartialEq)]
pub struct Editor {
    pub state: EditorState,
    pub email: Option<String>,
}

impl Editor {
    pub fn no_editor() -> Self {
        Self {
            state: EditorState::None,
            email: None,
        }
    }
}

/// Singleton model for storing and querying the data and logic logic needed by various view, based on the information
/// stored in [ObjectStoreModel]. As a general, rule, any new API that requires logic beyond just retrieving the raw value
/// in [ObjectStoreModel], should be stored here. This includes logic such as object trashed status, the object current editor,
/// and object location.
///
/// Any API added to this model should be unit tested in model_test.rs
pub struct ObjectStoreViewModel {
    folder_timestamp_cache: FolderTimestampCache,
}

type FolderTimestampCache = RefCell<HashMap<ObjectStoreId, ServerTimestamp>>;

pub enum ObjectStoreViewModelEvent {
    /// A model change has invalidated object sort timestamps.
    SortTimestampsChanged,
}

impl ObjectStoreViewModel {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        ctx.subscribe_to_model(
            &ObjectStoreModel::handle(ctx),
            Self::handle_object_store_event,
        );
        ctx.subscribe_to_model(
            &UpdateManager::handle(ctx),
            Self::handle_update_manager_event,
        );
        Self {
            folder_timestamp_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub fn mock(ctx: &mut ModelContext<Self>) -> Self {
        Self::new(ctx)
    }

    /// Returns local editor state for the object.
    ///
    /// Ashide Drive is local-only, so legacy collaborator metadata must not
    /// reintroduce account/profile lookups or remote baton-grabbing UI.
    pub fn object_current_editor(&self, uid: &ObjectUid, ctx: &AppContext) -> Option<Editor> {
        ObjectStoreModel::as_ref(ctx)
            .get_by_uid(uid)
            .map(|_| Editor::no_editor())
    }

    /// Get the [`Space`] that contains an object.
    pub fn object_space(&self, id: &ObjectUid, app: &AppContext) -> Option<Space> {
        ObjectStoreModel::as_ref(app)
            .get_by_uid(id)
            .map(|object| object.space(app))
    }

    /// Get the current user's access level on a Ashide Drive object.
    pub fn access_level(&self, object_uid: &ObjectUid, app: &AppContext) -> LocalObjectAccessLevel {
        let _ = (object_uid, app);
        LocalObjectAccessLevel::Full
    }

    /// Get the current user's editability state for a Ashide Drive object.
    pub fn object_editability(
        &self,
        object_uid: &ObjectUid,
        app: &AppContext,
    ) -> ContentEditability {
        let _ = (object_uid, app);
        ContentEditability::Editable
    }

    /// Get the timestamp to sort `object` according to `timestamp_kind`.
    pub fn object_sorting_timestamp(
        &self,
        object: &dyn StoredObject,
        timestamp_kind: UpdateTimestamp,
        app: &AppContext,
    ) -> Option<ServerTimestamp> {
        match timestamp_kind {
            // When sorting in the trash, we only ever consider the object's own trashed timestamp.
            // For trashed folders, their indirectly-trashed children will not have a trashed_ts,
            // so there's no need to recurse.
            UpdateTimestamp::Trashed => object.metadata().trashed_ts,
            // When sorting in the main index, we consider all of the children of a folder. This
            // can be expensive, so it's cached.
            UpdateTimestamp::Revision => {
                self.sorting_timestamp_rec(object, ObjectStoreModel::as_ref(app), app)
            }
        }
    }

    /// Calculate the sorting timestamp for `object`:
    /// * For a folder, this is the max of the folder's timestamp and all of its children's timestamps
    ///   (recursively, for sub-folders).
    /// * For other objects, this is the object's own timestamp.
    fn sorting_timestamp_rec(
        &self,
        object: &dyn StoredObject,
        object_store_model: &ObjectStoreModel,
        app: &AppContext,
    ) -> Option<ServerTimestamp> {
        let folder: Option<&FolderObject> = object.into();
        match folder {
            // For non-folder objects, always use the object's own timestamp.
            None => object.metadata().revision.clone().map(Into::into),
            Some(folder) => self
                .folder_timestamp_cache
                // Skip the cache if it's already mutably borrowed. This should not happen in practice,
                // because the UI framework is single-threaded.
                .try_borrow()
                .ok()
                .and_then(|cache| cache.get(&folder.id).cloned())
                .or_else(|| {
                    let max_child_timestamp = object_store_model
                        .active_stored_objects_in_location_without_descendents(
                            StoredObjectLocation::Folder(folder.id),
                            app,
                        )
                        // TODO(ben): This check won't be needed soon.
                        .filter(|child| child.permissions().owner == folder.permissions().owner)
                        .filter_map(|child| {
                            self.sorting_timestamp_rec(child, object_store_model, app)
                        })
                        .max();
                    // The `Ord` implementation of `Option` always considers `None` less than
                    // `Some`.
                    let folder_timestamp = folder.metadata().revision.clone().map(Into::into);
                    let timestamp = max_child_timestamp.max(folder_timestamp);

                    if let Some(timestamp) = timestamp {
                        if let Ok(mut cache) = self.folder_timestamp_cache.try_borrow_mut() {
                            cache.insert(folder.id, timestamp);
                        }
                    }

                    timestamp
                }),
        }
    }

    fn handle_object_store_event(
        &mut self,
        event: &ObjectStoreEvent,
        ctx: &mut ModelContext<Self>,
    ) {
        match event {
            ObjectStoreEvent::ObjectUpdated { type_and_id, .. }
            | ObjectStoreEvent::ObjectTrashed { type_and_id, .. }
            | ObjectStoreEvent::ObjectUntrashed { type_and_id, .. }
            | ObjectStoreEvent::ObjectPermissionsUpdated { type_and_id, .. } => {
                // If an object is updated, we need to recompute the timestamps of its parents.
                if self
                    .invalidate_object_timestamps(&type_and_id.uid(), ObjectStoreModel::as_ref(ctx))
                {
                    ctx.emit(ObjectStoreViewModelEvent::SortTimestampsChanged);
                }
            }
            ObjectStoreEvent::ObjectMoved {
                from_folder,
                to_folder,
                ..
            } => {
                // Both the old parent and the new parent need to be invalidated, since this object
                // could affect the sort timestamp of both. Even if the moved object were a folder,
                // its own sort timestamp isn't affected.
                let object_store_model = ObjectStoreModel::as_ref(ctx);
                let old_parent_changed = from_folder.is_some_and(|folder_id| {
                    self.invalidate_folder_timestamps(&folder_id, object_store_model)
                });
                let new_parent_changed = to_folder.is_some_and(|folder_id| {
                    self.invalidate_folder_timestamps(&folder_id, object_store_model)
                });
                if old_parent_changed || new_parent_changed {
                    ctx.emit(ObjectStoreViewModelEvent::SortTimestampsChanged);
                }
            }
            ObjectStoreEvent::ObjectCreated { type_and_id } => {
                if self
                    .invalidate_object_timestamps(&type_and_id.uid(), ObjectStoreModel::as_ref(ctx))
                {
                    ctx.emit(ObjectStoreViewModelEvent::SortTimestampsChanged);
                }
            }
            ObjectStoreEvent::ObjectDeleted { folder_id, .. } => {
                if let Some(folder_id) = folder_id {
                    if self.invalidate_folder_timestamps(folder_id, ObjectStoreModel::as_ref(ctx)) {
                        ctx.emit(ObjectStoreViewModelEvent::SortTimestampsChanged);
                    }
                }
            }
            ObjectStoreEvent::NotebookEditorChangedExternally { .. }
            | ObjectStoreEvent::ObjectForceExpanded { .. }
            | ObjectStoreEvent::InitialLoadCompleted => (),
        }
    }

    fn handle_update_manager_event(
        &mut self,
        event: &UpdateManagerEvent,
        ctx: &mut ModelContext<Self>,
    ) {
        let UpdateManagerEvent::ObjectOperationComplete { result } = event else {
            return;
        };

        if result.success_type != OperationSuccessType::Success {
            return;
        }

        let object_store_model = ObjectStoreModel::as_ref(ctx);
        if let ObjectOperation::Create { .. } = result.operation {
            let created_id = result
                .stable_id
                .map(ObjectStoreId::StableId)
                .or_else(|| result.client_id.map(ObjectStoreId::ClientId));
            let Some(created_id) = created_id else {
                return;
            };

            if object_store_model
                .get_folder_by_uid(&created_id.uid())
                .is_some()
            {
                if let Some(client_id) = result.client_id {
                    let object_store_id = ObjectStoreId::ClientId(client_id);
                    self.folder_timestamp_cache
                        .borrow_mut()
                        .remove(&object_store_id);
                }
            }

            // For any new object, we need to recalculate its ancestors' timestamp with their
            // new child.
            if let Some(parent_id) = object_store_model
                .get_by_uid(&created_id.uid())
                .and_then(|object| object.metadata().folder_id)
            {
                if self.invalidate_folder_timestamps(&parent_id, object_store_model) {
                    ctx.emit(ObjectStoreViewModelEvent::SortTimestampsChanged);
                }
            }
        }
    }

    /// Invalidate all cached timestamps for the object with the given ID, and its parents.
    fn invalidate_object_timestamps(
        &mut self,
        uid: &ObjectUid,
        object_store_model: &ObjectStoreModel,
    ) -> bool {
        let Some(object) = object_store_model.get_by_uid(uid) else {
            return false;
        };
        let folder: Option<&FolderObject> = object.into();
        match folder {
            Some(folder) => self.invalidate_folder_timestamps(&folder.id, object_store_model),
            None => {
                if let Some(parent_id) = object.metadata().folder_id {
                    self.invalidate_folder_timestamps(&parent_id, object_store_model)
                } else {
                    false
                }
            }
        }
    }

    /// Invalidate all cached timestamps for the given folder and its parents.
    fn invalidate_folder_timestamps(
        &mut self,
        folder_id: &ObjectStoreId,
        object_store_model: &ObjectStoreModel,
    ) -> bool {
        let had_revision_ts = self
            .folder_timestamp_cache
            .borrow_mut()
            .remove(folder_id)
            .is_some();

        let had_parent_ts = object_store_model
            .get_folder(folder_id)
            .and_then(|folder| folder.metadata().folder_id.as_ref())
            .is_some_and(|parent| self.invalidate_folder_timestamps(parent, object_store_model));
        had_revision_ts || had_parent_ts
    }
}

impl Entity for ObjectStoreViewModel {
    type Event = ObjectStoreViewModelEvent;
}

/// Mark ObjectStoreViewModel as global application state.
impl SingletonEntity for ObjectStoreViewModel {}

/// The timestamp to use when sorting objects by their last updated time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UpdateTimestamp {
    /// Sort objects by their revision timestamp, when they were last edited.
    #[default]
    Revision,
    /// Sort objects by their trashed timestamp.
    Trashed,
}
