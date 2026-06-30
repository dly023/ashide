//! 本地对象存储抽象。
//!
//! 这里统一描述 Notebook / Workflow / EnvVar / Fact / MCP / ExecutionProfile /
//! AIDocument 等需要进入 Ashide Drive、SQLite 和搜索索引的本地对象类型。
//!
//! - `StoredObject` trait 承载 metadata / permissions / versions / display_name /
//!   upsert_event / as_any / clone_box。
//! - `GenericStoredObject<K, M>` 是本地对象的泛型承载结构。
//! - `StoredObjectModel` 是本地对象类型描述。
//! - `ObjectStoreModel` 是进程内本地对象全局存储 + SQLite 背存。
//! - `ObjectStoreEvent` 是本地模型变更事件总线,被本地 UI 视图订阅。
//! - `ObjectTypeAndId` 是本地 ID 判别式,被 Drive UI / search 等入口使用。

use self::{breadcrumbs::ContainingObject, model::persistence::ObjectStoreModel};
use crate::{
    appearance::Appearance,
    auth::UserUid,
    drive::{items::LocalDriveItem, ObjectTypeAndId},
    object_store::ids::{
        ClientId, HashableId, HashedSqliteId, ObjectStoreId, ObjectUid, StableObjectId,
        ToStableObjectId,
    },
    persistence::ModelEvent,
    server_time::ServerTimestamp,
    util::time_format::format_approx_duration_from_now_utc,
    workflows::WorkflowSource,
    workspaces::{user_profiles::UserProfiles, user_workspaces::UserWorkspaces},
};
use chrono::{Duration, Utc};
use derivative::Derivative;
use std::{any::Any, collections::HashSet, fmt::Debug, sync::Arc};
use warpui::{AppContext, SingletonEntity};

pub mod breadcrumbs;
pub mod grab_edit_access_modal;
pub mod ids;
pub mod model;
mod server_types;
pub mod toast_message;
pub mod update_manager;

pub use server_types::*;

/// 包装一个 model 序列化后字符串的 newtype。
///
/// 多个 object-store model 的 `serialized()` 返回它,用于本地 SQLite 写入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerializedModel(String);

impl SerializedModel {
    pub fn new(s: String) -> Self {
        Self(s)
    }

    pub fn model_as_str(&self) -> &str {
        &self.0
    }

    pub fn take(self) -> String {
        self.0
    }
}

impl From<String> for SerializedModel {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for SerializedModel {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// A StoredObject represents a local object that can be edited and persisted in
/// Ashide's local object store. Objects keep local revision numbers so stale
/// edits can be detected and resolved deterministically.
///
/// Note that this trait must be object-safe and non-generic.  The reason for this
/// is that (a) we need to be able to store instances of it as trait objects in
/// ObjectStoreModel and (b) we need to be able to support mixed collections of different
/// instances of it (e.g. in the map of id -> StoredObject in ObjectStoreModel).
///
/// There are two closely related types to this:
/// 1) GenericStoredObject: This is the concrete generic implementation of StoredObject that
///    holds onto a model of type StoredObjectModel and an id of type ObjectStoreId.
/// 2) StoredObjectModel: This is a trait that defines the model type for a StoredObject -
///    this is what implementors of new stored object types typically have to implement.
///
/// These types are tightly coupled.  In an ideal world, rust would allow a mechanism
/// for us having a single interface that new model types could implement that could
/// be generic on id and model types, but as far as I (zach) can tell, that's not currently
/// possible.
///
/// The typical usage pattern for these types is to use dyn StoredObject whenever you
/// don't need access to a model or id, and to downcast to a GenericStoredObject whenever you do.
///
/// 当前所有本地 StoredObject 都需要实现 GenericStoredObject。
///
/// Additionally, they support the local edit-access UX used by Notebook and
/// Workflow views.
pub trait StoredObject: Debug {
    /// Returns the name of this model type (e.g. Workflow, Folder, Notebook)
    fn model_type_name(&self) -> &'static str;

    /// Returns the  uid for this object.
    fn uid(&self) -> ObjectUid;

    /// Returns the [`ObjectStoreId`] that currently identifies this object.
    fn object_store_id(&self) -> ObjectStoreId;

    /// Returns the id used to index into sqlite, this is the object's UID with its type
    /// prefixed, such as "Workflow-{UID}"
    fn hashed_sqlite_id(&self) -> HashedSqliteId;

    /// Returns the StoredObjectMetadata struct associated with this object.
    fn metadata(&self) -> &StoredObjectMetadata;

    /// Returns a mutable reference to the StoredObjectMetadata struct associated with this object.
    fn metadata_mut(&mut self) -> &mut StoredObjectMetadata;

    /// Returns the StoredObjectPermissions struct associated with this object.
    fn permissions(&self) -> &StoredObjectPermissions;

    /// Returnsa mutable reference to the StoredObjectPermissions struct associated with this object.
    fn permissions_mut(&mut self) -> &mut StoredObjectPermissions;

    /// Returns the ObjectType i.e. 'Workflow' or 'Notebook'
    fn object_type(&self) -> ObjectType;

    /// Returns the ObjectTypeAndId for this object.
    fn object_type_and_id(&self) -> ObjectTypeAndId;

    /// Sets the legacy server-style id on this object.
    fn set_stable_id(&mut self, stable_id: StableObjectId);

    /// Returns whether this object can be moved to the given space.
    fn can_move_to_space(&self, _space: Space, _app: &AppContext) -> bool {
        true
    }

    // Whether to clear this object from the local SQLite DB on a unique key conflict.
    fn should_clear_on_unique_key_conflict(&self) -> bool {
        false
    }

    /// Whether to show a warning if this object is unsaved at quit time
    /// (which typically blocks the user from quitting)
    fn warn_if_unsaved_at_quit(&self) -> bool {
        true
    }

    /// Returns the "upsert" event for inserting / updating this object in the SQLite DB.
    fn upsert_event(&self) -> ModelEvent;

    // Returns the name of the object.
    fn display_name(&self) -> String;

    /// Returns whether this model type should render as a local Drive item.
    fn renders_in_local_drive(&self) -> bool;

    /// Returns whether this model type should show update toasts in the UI.
    fn should_show_activity_toasts(&self) -> bool {
        true
    }

    /// Creates a new local Drive item for this object.  Returns None if this
    /// object is not rendered in Local Drive.
    fn to_local_drive_item(&self, appearance: &Appearance) -> Option<Box<dyn LocalDriveItem>>;

    /// The local space containing this object.
    fn space(&self, app: &AppContext) -> Space {
        UserWorkspaces::as_ref(app).owner_to_space(self.permissions().owner, app)
    }

    /// Local Drive has no shared-object leave action.
    fn can_leave(&self, app: &AppContext) -> bool {
        let _ = app;
        false
    }

    /// Returns the name of the containing "object" for this object.
    /// This could be a folder, or in the case of top-level objects,
    /// the name of the space it belongs to.
    fn containing_object_name(&self, app: &AppContext) -> String {
        self.containing_objects_path(app)
            .into_iter()
            .next_back()
            .expect("Object should have at least one ancestor")
            .name
    }

    // Returns the path of all the containing "objects" for this object.
    // This could include folders or spaces.
    fn containing_objects_path(&self, app: &AppContext) -> Vec<ContainingObject> {
        let space = self.space(app);

        match self.metadata().folder_id {
            Some(folder_id) => {
                let object_store_model = ObjectStoreModel::as_ref(app);
                if let Some(folder) = object_store_model.get_folder_by_uid(&folder_id.uid()) {
                    let mut path = vec![];
                    let ancestors = folder.containing_objects_path(app);
                    path.extend(ancestors);
                    path.push(folder.into());
                    path
                } else {
                    // if for whatever reason the folder id is messed up,
                    // just default to showing the top-level space it wound up in
                    vec![space.into_containing_object(app)]
                }
            }
            None => vec![space.into_containing_object(app)],
        }
    }

    fn breadcrumbs(&self, app: &AppContext) -> String {
        self.containing_objects_path(app)
            .into_iter()
            .map(|object| object.name)
            .collect::<Vec<String>>()
            .join(" / ")
    }

    /// Returns whether this StoredObject is in the given space
    fn is_in_space(&self, space: Space, app: &AppContext) -> bool {
        self.space(app) == space
    }

    fn is_welcome_object(&self) -> bool {
        self.metadata().is_welcome_object
    }

    /// Returns the direct location of the object. If the object
    /// is not in a folder, this will be the object's space. Otherwise, it will
    /// be the folder the object is placed in directly, even if that folder is nested.
    fn location(
        &self,
        object_store_model: &ObjectStoreModel,
        app: &AppContext,
    ) -> StoredObjectLocation {
        if let Some(folder_id) = self.metadata().folder_id {
            if object_store_model.get_folder(&folder_id).is_some() {
                return StoredObjectLocation::Folder(folder_id);
            }
        }

        StoredObjectLocation::Space(self.space(app))
    }

    /// Return true is this object or any of its ancestors are trashed. Also returns true
    /// if a cycle is detected.
    fn is_trashed(&self, object_store_model: &ObjectStoreModel) -> bool {
        self.is_trashed_internal(object_store_model, &mut HashSet::new())
    }

    /// Helper function for is_trashed.
    fn is_trashed_internal(
        &self,
        object_store_model: &ObjectStoreModel,
        ancestors: &mut HashSet<String>,
    ) -> bool {
        // Base case: If the object is trashed, return true.
        if self.metadata().trashed_ts.is_some() {
            return true;
        }

        // Else: return true if the object's parent is trashed. Return false if the object has no parent.
        match self.metadata().folder_id.map(|parent_id| parent_id.uid()) {
            Some(hashed_parent_id) => {
                // We need to check for cycles to avoid causing a stack overflow. If a cycle is detected, return that the object is trashed.
                if ancestors.contains(&hashed_parent_id) {
                    return true;
                }
                ancestors.insert(hashed_parent_id.clone());

                match object_store_model.get_by_uid(&hashed_parent_id) {
                    Some(parent) => parent.is_trashed_internal(object_store_model, ancestors),
                    None => {
                        // Local Drive keeps orphaned legacy objects visible in Personal instead
                        // of hiding them behind the removed non-local sharing surface.
                        false
                    }
                }
            }
            None => false,
        }
    }

    /// Returns whether this object has conflicting local object-store changes.
    fn has_conflicting_changes(&self) -> bool;

    /// Returns the revision of the conflicting object, if any.
    /// This is used for object-safe access to conflict information.
    fn conflicting_object_revision(&self) -> Option<Revision>;

    /// Clears the conflict status back to NoConflicts.
    fn clear_conflict_status(&mut self);

    /// Updates the object to deal with any conflict status.
    fn replace_object_with_conflict(&mut self);

    /// Sets the content persistence status of this object to `InFlight` (if it
    /// wasn't already) and increments the number of in-flight requests tracked
    /// in the `InFlight` enum.
    fn increment_in_flight_request_count(&mut self) {
        let new_reqs = match &self.metadata().pending_changes_statuses.content_sync_status {
            StoredObjectSyncStatus::InFlight(reqs) => reqs.0 + 1,
            _ => 1,
        };

        self.set_pending_content_changes_status(StoredObjectSyncStatus::InFlight(
            NumInFlightRequests(new_reqs),
        ))
    }

    /// Decrements the number of in flight requests tracked in this object's `InFlight` enum. If
    /// that number becomes 0, it's no longer in flight, so it will be set to `status_if_no_reqs`.
    /// Returns true if the object is no longer in flight.
    fn decrement_in_flight_request_count(
        &mut self,
        status_if_no_reqs: StoredObjectSyncStatus,
    ) -> bool {
        match &self.metadata().pending_changes_statuses.content_sync_status {
            StoredObjectSyncStatus::InFlight(reqs) => {
                if reqs.0 - 1 == 0 {
                    self.set_pending_content_changes_status(status_if_no_reqs);
                    return true;
                } else {
                    self.set_pending_content_changes_status(StoredObjectSyncStatus::InFlight(
                        NumInFlightRequests(reqs.0 - 1),
                    ));
                    return false;
                }
            }
            _ => log::error!(
                "called decrement_in_flight_request_count with a non-`InFlight` stored-object status"
            ),
        }

        true
    }

    /// Sets the content change status on this object's metadata
    fn set_pending_content_changes_status(
        &mut self,
        pending_content_changes_status: StoredObjectSyncStatus,
    ) {
        self.metadata_mut()
            .pending_changes_statuses
            .content_sync_status = pending_content_changes_status;
    }

    /// Whether or not this object can be exported.
    fn can_export(&self) -> bool;

    /// Returns this object as a ref to the Any type.  Needed for typecasts.
    fn as_any(&self) -> &dyn Any;

    /// Returns this object as a mut ref to Any type.  Needed for typecasts.
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// Returns the trait object as a concrete type reference by downcasting it.
    /// Returns None if the downcast fails.
    fn as_model_type<K, M>(stored_object: &dyn StoredObject) -> Option<&GenericStoredObject<K, M>>
    where
        Self: Sized,
        K: HashableId + ToStableObjectId + Debug + Into<String> + Clone + 'static,
        M: StoredObjectModel<IdType = K, StoredObjectType = GenericStoredObject<K, M>> + 'static,
    {
        stored_object
            .as_any()
            .downcast_ref::<GenericStoredObject<K, M>>()
    }

    /// Returns the trait object as a concrete mutable type reference by downcasting it.
    /// Returns None if the downcast fails.
    fn as_model_type_mut<K, M>(
        stored_object: &mut dyn StoredObject,
    ) -> Option<&mut GenericStoredObject<K, M>>
    where
        Self: Sized,
        K: HashableId + ToStableObjectId + Debug + Into<String> + Clone + 'static,
        M: StoredObjectModel<IdType = K, StoredObjectType = GenericStoredObject<K, M>> + 'static,
    {
        stored_object
            .as_any_mut()
            .downcast_mut::<GenericStoredObject<K, M>>()
    }

    /// 返回这个 stored object 的 boxed clone。
    /// 不能直接要求 StoredObject trait derive Clone,否则 trait 不再 object safe。
    fn clone_box(&self) -> Box<dyn StoredObject>;
}

/// Defines a common trait for object store models to implement.
/// "model" 是 stored object 的领域数据,例如 notebook/workflow/folder 的具体内容;
/// metadata、permissions、persistence status 逻辑不放在这里。
///
/// See the comments for StoredObject to understand the relationship between
/// this trait, StoredObject and GenericStoredObject.  They are tightly coupled.
///
/// When building new model types (e.g. for settings or launch configs) we should just
/// have to implement this trait, and not the entire StoredObject trait.
pub trait StoredObjectModel: Debug + Clone + Send + Sync {
    /// The associated StoredObject type for this model (e.g. NotebookObject, WorkflowObject, etc)
    type StoredObjectType: StoredObject + 'static;
    // TODO: Remove after the local object id refactor.
    type IdType: HashableId + ToStableObjectId + Debug + Into<String> + Clone + 'static;

    /// Returns the name of this model type (e.g. Workflow, Folder, Notebook)
    fn model_type_name(&self) -> &'static str;

    /// Returns the ObjectTypeAndId for this object.
    fn object_type_and_id(&self, id: ObjectStoreId) -> ObjectTypeAndId;

    /// Returns the ObjectType for this model.
    fn object_type(&self) -> ObjectType;

    /// Returns whether this model type should render as a local Drive item.
    fn renders_in_local_drive(&self) -> bool;

    /// Returns whether this model type should show update toasts in the UI.
    fn should_show_activity_toasts(&self) -> bool {
        true
    }

    /// Whether to show a warning if this model is unsaved at quit time
    /// (which typically blocks the user from quitting)
    fn warn_if_unsaved_at_quit(&self) -> bool {
        true
    }

    /// Creates a new local Drive item for this model type. Returns None
    /// if this object does not render in Local Drive.
    fn to_local_drive_item(
        &self,
        id: ObjectStoreId,
        appearance: &Appearance,
        object: &Self::StoredObjectType,
    ) -> Option<Box<dyn LocalDriveItem>>;

    /// Returns the display name for this model (e.g. to show in the local Drive index)
    fn display_name(&self) -> String;

    /// Sets the display name to show in the Ashide Drive Index.  Setting the name
    /// is not currently supported by all object types, hence the default empty
    /// implementation.
    fn set_display_name(&mut self, _name: &str) {}

    /// Returns the upsert event for putting this model into the SQLite database.
    fn upsert_event(&self, object: &Self::StoredObjectType) -> ModelEvent;

    /// Returns a bulk upsert event for putting a list of this model into the SQLite database.
    fn bulk_upsert_event(objects: &[Self::StoredObjectType]) -> ModelEvent;

    /// Returns a serialized model.
    fn serialized(&self) -> SerializedModel;

    /// Returns whether this model type supports being moved to the given space.
    fn can_move_to_space(&self, _current_space: Space, _new_space: Space) -> bool {
        true
    }

    /// Returns whether this model type should clear on a unique key conflict.
    fn should_clear_on_unique_key_conflict(&self) -> bool {
        false
    }

    /// Returns whether this model type should be updated after a stored revision conflict.
    /// Note that for now the only model type that this is relevant for is Notebooks,
    /// where we show a banner in case of conflicts and ask users to manually take action.
    /// For other types we typically just want to replace the local object with the newer
    /// stored revision, which doesn't go through this code path.
    fn should_update_after_stored_revision_conflict(&self) -> bool;

    /// Whether this model type can be exported.
    fn can_export(&self) -> bool {
        false
    }
}

/// GenericStoredObject 是本地对象通用实现。
///
/// 新对象可以直接使用 GenericStoredObject<K, M>,其中 K 是 id type,M 是 model type。
///
/// For example, NotebookObject becomes:
///
///   pub type NotebookObject = GenericStoredObject<NotebookId, NotebookObjectModel>
///
/// The advantage of using the generic model is you get common implementations
/// of StoredObject methods like ```versions``` for free.
///
/// See the comments for StoredObject to understand the relationship between
/// this trait, StoredObject and StoredObjectModel.  They are tightly coupled.
#[derive(Clone, Debug)]
pub struct GenericStoredObject<K, M>
where
    K: HashableId + ToStableObjectId + Debug + Into<String> + Clone + 'static,
    M: StoredObjectModel<IdType = K> + 'static,
{
    pub id: ObjectStoreId,
    pub metadata: StoredObjectMetadata,
    pub permissions: StoredObjectPermissions,
    /// Tracks whether this object has a conflict with local object-store state.
    /// This is runtime state (not persisted) - conflicts are always NoConflicts when loaded from SQLite.
    pub conflict_status: ConflictStatus,

    // Intentionally not public to prevent users of this class from holding
    // onto references to the model outside of this struct.
    //
    // This is an Arc in order to support clone-on-write semantics for the model.
    // By wrapping the model in an Arc, clones become cheap, and we can avoid
    // doing deep clones of the model whenever the containing object is cloned.
    //
    // Callers who want to update the model need to call set_model to update the
    // entire model atomically.
    model: Arc<M>,
}

impl<K, M> StoredObject for GenericStoredObject<K, M>
where
    K: HashableId + ToStableObjectId + Debug + Into<String> + Clone + 'static,
    M: StoredObjectModel<IdType = K, StoredObjectType = GenericStoredObject<K, M>> + 'static,
{
    fn model_type_name(&self) -> &'static str {
        self.model.model_type_name()
    }

    fn uid(&self) -> ObjectUid {
        self.id.uid()
    }

    fn hashed_sqlite_id(&self) -> HashedSqliteId {
        self.id.sqlite_uid_hash(self.object_type().into())
    }

    fn object_store_id(&self) -> ObjectStoreId {
        self.id
    }

    fn should_show_activity_toasts(&self) -> bool {
        self.model.should_show_activity_toasts()
    }

    fn warn_if_unsaved_at_quit(&self) -> bool {
        self.model.warn_if_unsaved_at_quit()
    }

    fn metadata(&self) -> &StoredObjectMetadata {
        &self.metadata
    }

    fn metadata_mut(&mut self) -> &mut StoredObjectMetadata {
        &mut self.metadata
    }

    fn permissions(&self) -> &StoredObjectPermissions {
        &self.permissions
    }

    fn permissions_mut(&mut self) -> &mut StoredObjectPermissions {
        &mut self.permissions
    }

    fn object_type(&self) -> ObjectType {
        self.model.object_type()
    }

    fn object_type_and_id(&self) -> ObjectTypeAndId {
        self.model.object_type_and_id(self.id)
    }

    fn should_clear_on_unique_key_conflict(&self) -> bool {
        self.model.should_clear_on_unique_key_conflict()
    }

    fn can_move_to_space(&self, space: Space, app: &AppContext) -> bool {
        self.model.can_move_to_space(self.space(app), space)
    }

    fn has_conflicting_changes(&self) -> bool {
        self.conflict_status.has_conflicts()
    }

    fn conflicting_object_revision(&self) -> Option<Revision> {
        match &self.conflict_status {
            ConflictStatus::ConflictingChanges { revision } => Some(revision.clone()),
            ConflictStatus::NoConflicts => None,
        }
    }

    fn clear_conflict_status(&mut self) {
        self.conflict_status = ConflictStatus::NoConflicts;
    }

    fn replace_object_with_conflict(&mut self) {
        let mut new_conflict = ConflictStatus::NoConflicts;
        std::mem::swap(&mut new_conflict, &mut self.conflict_status);

        self.set_pending_content_changes_status(StoredObjectSyncStatus::NoLocalChanges);

        let _ = new_conflict;
        self.conflict_status = ConflictStatus::NoConflicts;
    }

    fn set_stable_id(&mut self, stable_id: StableObjectId) {
        self.id = ObjectStoreId::StableId(stable_id);
    }

    fn upsert_event(&self) -> ModelEvent {
        self.model.upsert_event(self)
    }

    fn display_name(&self) -> String {
        self.model.display_name()
    }

    fn renders_in_local_drive(&self) -> bool {
        self.model.renders_in_local_drive()
    }

    fn to_local_drive_item(&self, appearance: &Appearance) -> Option<Box<dyn LocalDriveItem>> {
        self.model.to_local_drive_item(self.id, appearance, self)
    }

    fn can_export(&self) -> bool {
        self.model.can_export()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn clone_box(&self) -> Box<dyn StoredObject> {
        Box::new(self.clone())
    }
}

impl<K, M> GenericStoredObject<K, M>
where
    K: HashableId + ToStableObjectId + Debug + Into<String> + Clone + 'static,
    M: StoredObjectModel<IdType = K, StoredObjectType = GenericStoredObject<K, M>> + 'static,
{
    /// Gets a reference to the model held by the object.
    pub fn model(&self) -> &M {
        &self.model
    }

    /// Returns a shared handle to the model.
    pub fn shared_model(&self) -> Arc<M> {
        self.model.clone()
    }

    /// Sets a new version of the model on the object, replacing the old version.
    pub fn set_model(&mut self, model: M) {
        self.model = model.into();
    }

    /// Returns a bulk upsert event for putting a list of this model into the SQLite database.
    pub fn bulk_upsert_event(objects: &[Self]) -> ModelEvent {
        M::bulk_upsert_event(objects)
    }

    /// Constructs a new instance of this model with the given id, model, metadata and permissions.
    pub fn new(
        id: ObjectStoreId,
        model: M,
        metadata: StoredObjectMetadata,
        permissions: StoredObjectPermissions,
    ) -> Self {
        Self {
            id,
            model: model.into(),
            metadata,
            permissions,
            conflict_status: ConflictStatus::NoConflicts,
        }
    }

    /// Creates a new GenericStoredObject with the given model, owner, and initial folder id.
    /// This is for the direct local creation flow, as opposed to applying an
    /// existing object-store update.
    pub fn new_local(
        model: M,
        owner: Owner,
        initial_folder_id: Option<ObjectStoreId>,
        client_id: ClientId,
    ) -> Self {
        Self {
            id: ObjectStoreId::ClientId(client_id),
            model: model.into(),
            metadata: StoredObjectMetadata {
                pending_changes_statuses: StoredObjectStatuses {
                    content_sync_status: StoredObjectSyncStatus::InFlight(NumInFlightRequests(1)),
                    has_pending_metadata_change: false,
                    has_pending_permissions_change: false,
                    pending_untrash: false,
                    pending_delete: false,
                },
                folder_id: initial_folder_id,
                revision: Default::default(),
                metadata_last_updated_ts: Default::default(),
                current_editor_uid: Default::default(),
                trashed_ts: Default::default(),
                // Objects created from the client are never welcome objects.
                is_welcome_object: false,
                creator_uid: None,
                last_editor_uid: None,
                last_task_run_ts: None,
            },
            permissions: StoredObjectPermissions {
                owner,
                permissions_last_updated_ts: None,
            },
            conflict_status: ConflictStatus::NoConflicts,
        }
    }
}

impl<'a, K, M> From<&'a dyn StoredObject> for Option<&'a GenericStoredObject<K, M>>
where
    K: HashableId + ToStableObjectId + Debug + Into<String> + Clone + 'static,
    M: StoredObjectModel<IdType = K, StoredObjectType = GenericStoredObject<K, M>> + 'static,
{
    fn from(value: &'a dyn StoredObject) -> Self {
        <GenericStoredObject<K, M> as StoredObject>::as_model_type::<K, M>(value)
    }
}

impl<'a, K, M> From<&'a Box<dyn StoredObject>> for Option<&'a GenericStoredObject<K, M>>
where
    K: HashableId + ToStableObjectId + Debug + Into<String> + Clone + 'static,
    M: StoredObjectModel<IdType = K, StoredObjectType = GenericStoredObject<K, M>> + 'static,
{
    fn from(value: &'a Box<dyn StoredObject>) -> Self {
        <GenericStoredObject<K, M> as StoredObject>::as_model_type::<K, M>(value.as_ref())
    }
}

impl<'a, K, M> From<&'a mut Box<dyn StoredObject>> for Option<&'a mut GenericStoredObject<K, M>>
where
    K: HashableId + ToStableObjectId + Debug + Into<String> + Clone + 'static,
    M: StoredObjectModel<IdType = K, StoredObjectType = GenericStoredObject<K, M>> + 'static,
{
    fn from(value: &'a mut Box<dyn StoredObject>) -> Self {
        <GenericStoredObject<K, M> as StoredObject>::as_model_type_mut::<K, M>(value.as_mut())
    }
}

impl Clone for Box<dyn StoredObject> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

#[derive(Clone, Debug, Default)]
pub enum ConflictStatus {
    #[default]
    NoConflicts,
    ConflictingChanges {
        revision: Revision,
    },
}

impl ConflictStatus {
    /// Utility function that allows for a more ergonomic way of figuring out whether there is a
    /// conflict (for cases where we don't care about the conflict details).
    pub fn has_conflicts(&self) -> bool {
        matches!(self, ConflictStatus::ConflictingChanges { .. })
    }
}

/// Represents a unique key for a generic string object. Local persistence
/// enforces that no two generic string objects have the same key.
#[derive(PartialEq, Eq, Debug, Clone)]
pub struct GenericStringObjectUniqueKey {
    /// The unique key.  E.g. for local prefs this is the storage_key of the pref
    pub key: String,

    /// Whether this key is unique for all generic string objects, or unique per user.
    pub unique_per: UniquePer,
}

#[derive(PartialEq, Eq, Debug, Clone)]
pub enum UniquePer {
    User,
}

impl From<&dyn StoredObject> for ObjectType {
    fn from(value: &dyn StoredObject) -> Self {
        value.object_type()
    }
}

impl From<&Box<dyn StoredObject>> for ObjectType {
    fn from(value: &Box<dyn StoredObject>) -> Self {
        <ObjectType as From<&dyn StoredObject>>::from(value.as_ref())
    }
}

/// Extension trait for StoredObjectMetadata with methods that require AppContext.
pub trait StoredObjectMetadataExt {
    /// Returns a semantic summary of the last edit to the object. For example, "Alice edited 4 weeks ago".
    /// Returns None if the revision and last_editor are None.
    fn semantic_editing_history(&self, app: &AppContext) -> Option<String>;

    /// Returns a semantic summary of the object's creator. For example, "Alice" or "joan@warp.dev".
    #[cfg_attr(target_family = "wasm", expect(dead_code))]
    fn semantic_creator(&self, app: &AppContext) -> Option<String>;

    /// Returns semantic summary of countdown of days until permadeletion.
    /// Ex: "27 days until permanent deletion"
    fn semantic_permadeletion_countdown(&self, app: &AppContext) -> Option<String>;
}

impl StoredObjectMetadataExt for StoredObjectMetadata {
    fn semantic_editing_history(&self, app: &AppContext) -> Option<String> {
        let user_profiles = UserProfiles::as_ref(app);

        // First, the editor. For example, "Joan Didion" or "joan@warp.dev".
        let editor_string = self
            .last_editor_uid
            .as_ref()
            .and_then(|uid| user_profiles.displayable_identifier_for_uid(UserUid::new(uid)));

        // Second, the time elapsed since the edit. For example, "just now" or "3 months ago".
        let time_ago_string = self
            .revision
            .clone()
            .map(|r| format_approx_duration_from_now_utc(r.utc()));

        let full_string = match (editor_string, time_ago_string) {
            (Some(name), Some(time_ago)) if name.is_empty() => format!("Edited {time_ago}"),
            (Some(name), Some(time_ago)) => format!("{name} edited {time_ago}"),
            (None, Some(time_ago)) => format!("Edited {time_ago}"),
            (Some(name), None) => format!("Last edited by {name}"),
            _ => return None,
        };

        Some(full_string)
    }

    fn semantic_creator(&self, app: &AppContext) -> Option<String> {
        // Todo(Jack): add creation ts.
        let user_profiles = UserProfiles::as_ref(app);
        self.creator_uid
            .as_ref()
            .and_then(|uid| user_profiles.displayable_identifier_for_uid(UserUid::new(uid)))
    }

    fn semantic_permadeletion_countdown(&self, app: &AppContext) -> Option<String> {
        // 2 cases:
        // 1) Either the object is a root level object.
        // 2) Or the object is inside folder(s), call recursive function to get trashed_ts of top level folder.
        if let Some(trashed_ts) = self
            .trashed_ts
            .or_else(|| get_top_folder_trashed_ts(self.folder_id, app))
        {
            let deletion_time = trashed_ts.utc() + Duration::days(31);
            let current_time = Utc::now();
            let days_left = deletion_time.signed_duration_since(current_time).num_days();

            let full_string = match days_left {
                0 | 1 => "1 day until permanent deletion".to_string(),
                _ => format!("{days_left} days until permanent deletion"),
            };
            Some(full_string)
        } else {
            None
        }
    }
}

/// Helper function to retrieve trashed_ts of top level folder given a folder_id of an object.
fn get_top_folder_trashed_ts(
    folder_id: Option<ObjectStoreId>,
    app: &AppContext,
) -> Option<ServerTimestamp> {
    let mut folder_id = folder_id;
    let object_store_model = ObjectStoreModel::as_ref(app);
    while let Some(current_folder_id) = folder_id {
        // If the parent folder isn't in ObjectStoreModel, short-circuit so we don't loop forever.
        let folder = object_store_model.get_folder_by_uid(&current_folder_id.uid())?;

        if let Some(_parent_folder_id) = folder.metadata.folder_id {
            folder_id = folder.metadata.folder_id
        } else {
            return folder.metadata.trashed_ts;
        }
    }
    None
}

#[derive(Default, Clone, Copy, Debug, Eq, Derivative)]
#[derivative(PartialEq, Hash)]
pub enum Space {
    /// The current user's local personal drive.
    #[default]
    Personal,
}

impl Space {
    pub fn name(&self, app: &AppContext) -> String {
        let _ = (self, app);
        "Personal".to_string()
    }
}

/// Enum for specifying the location of a Ashide Drive object.
/// Objects can live in top level spaces, or a specific folder.
#[derive(Eq, PartialEq, Copy, Clone, Debug, Hash)]
pub enum StoredObjectLocation {
    Space(Space),
    Folder(ObjectStoreId),
    Trash,
}

impl From<Space> for WorkflowSource {
    fn from(space: Space) -> Self {
        let _ = space;
        WorkflowSource::PersonalDrive
    }
}

impl From<Owner> for WorkflowSource {
    fn from(owner: Owner) -> WorkflowSource {
        let _ = owner;
        Self::PersonalDrive
    }
}
