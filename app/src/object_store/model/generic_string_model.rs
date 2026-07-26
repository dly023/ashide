use std::fmt::Debug;

use crate::{
    appearance::Appearance,
    drive::{items::LocalDriveItem, ObjectTypeAndId},
    object_store::ids::{ObjectStoreId, ObjectUid, StableObjectId},
    object_store::{
        GenericStoredObject, GenericStringObjectFormat, GenericStringObjectUniqueKey, ObjectType,
        SerializedModel, StoredObject, StoredObjectModel,
    },
    persistence::ModelEvent,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};

/// A trait that generic string-based objects should implement.
pub trait StoredStringObject: StoredObject + Send + Sync {
    /// Returns the object format for this object.
    fn generic_string_object_format(&self) -> GenericStringObjectFormat;

    /// Returns the id for this specific object.
    fn id(&self) -> ObjectStoreId;

    /// Returns a serialized model from this string object.
    fn serialized(&self) -> SerializedModel;

    /// 返回这个 stored object 的 boxed clone。
    /// 不能直接要求该 trait derive Clone,否则 trait 不再 object safe。
    fn clone_box(&self) -> Box<dyn StoredStringObject>;
}

/// A `StringModel` is a model that can be serialized and deserialized as a simple string.
///
/// Any model that has a simple string representation (e.g. JSON, markdown, yaml) that can be atomically updated
/// 实现这个 trait 后即可复用大多数本地 object-store 行为。
///
/// 实现这个类型的对象共享同一套本地存储行为。
pub trait StringModel: Clone + Debug + PartialEq + Send + Sync + 'static {
    type StoredObjectType: StoredObject + 'static;

    /// Returns the name of this model type (e.g. Workflow, Folder, Notebook)
    fn model_type_name(&self) -> &'static str;

    /// Whether we should enforce revisions for this model type.
    /// If revisions are not enforced, updates will have last-write-wins semantics.
    /// If revisions are enforced, the object will need to add logic to
    /// the update manager for how conflicts are resolved.
    fn should_enforce_revisions() -> bool;

    /// Returns the serialization format for this model.
    fn model_format() -> GenericStringObjectFormat;

    /// Whether to show update toasts for this type of model.
    fn should_show_activity_toasts() -> bool;

    /// Whether to show a warning if this type of model is unsaved at quit time
    /// (which typically blocks the user from quitting)
    fn warn_if_unsaved_at_quit() -> bool;

    /// Returns the display name for this model.
    fn display_name(&self) -> String;

    /// Returns whether to render this model as a LocalDriveItem.
    fn renders_in_local_drive(&self) -> bool {
        false
    }

    /// Returns whether this model can be exported to a file
    fn can_export(&self) -> bool {
        false
    }

    /// Sets the display name for this model
    fn set_display_name(&mut self, _name: &str) {}

    /// Creates a new local Drive item for this model type. Returns None
    /// if this object does not render in Local Drive.
    fn to_local_drive_item(
        &self,
        _id: ObjectStoreId,
        _appearance: &Appearance,
        _object: &Self::StoredObjectType,
    ) -> Option<Box<dyn LocalDriveItem>> {
        None
    }

    /// Returns whether this model type should clear on a unique key conflict.
    fn should_clear_on_unique_key_conflict(&self) -> bool {
        false
    }

    /// Returns a unique key for this object, if one exists. Unique keys are used
    /// to enforce that only one object with a given key can exist in the generic string
    /// object server database.
    fn uniqueness_key(&self) -> Option<GenericStringObjectUniqueKey>;
}

/// A serializer goes from a model to a string and back.
pub trait Serializer<M>: Debug + Clone + 'static {
    fn serialize(model: &M) -> SerializedModel;
    fn deserialize_owned(serialized: &str) -> Result<M>
    where
        Self: Sized;
}

/// A `GenericStringModel` is a generic implementation of model types that can serialize to/from string.
/// given a particular serializer.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct GenericStringModel<M, S>
where
    M: StringModel<
        StoredObjectType = GenericStoredObject<GenericStringObjectId, GenericStringModel<M, S>>,
    >,
    S: Serializer<M>,
{
    pub string_model: M,
}

impl<M, S> StoredStringObject
    for GenericStoredObject<GenericStringObjectId, GenericStringModel<M, S>>
where
    M: StringModel<
        StoredObjectType = GenericStoredObject<GenericStringObjectId, GenericStringModel<M, S>>,
    >,
    S: Serializer<M>,
{
    fn generic_string_object_format(&self) -> GenericStringObjectFormat {
        M::model_format()
    }

    fn id(&self) -> ObjectStoreId {
        self.id
    }

    fn serialized(&self) -> SerializedModel {
        self.model.serialized()
    }

    fn clone_box(&self) -> Box<dyn StoredStringObject> {
        Box::new(self.clone())
    }
}

/// Implements the StoredObjectModel trait for all generic string models.
///
/// This has common logic for storing string models to SQLite and updating from the
/// server -- basically for anything not specific to the contents
/// of the string model.
impl<M, S> StoredObjectModel for GenericStringModel<M, S>
where
    M: StringModel<
        StoredObjectType = GenericStoredObject<GenericStringObjectId, GenericStringModel<M, S>>,
    >,
    S: Serializer<M>,
{
    type StoredObjectType = GenericStoredObject<GenericStringObjectId, Self>;
    type IdType = GenericStringObjectId;

    fn model_type_name(&self) -> &'static str {
        self.string_model.model_type_name()
    }

    fn object_type(&self) -> ObjectType {
        ObjectType::GenericStringObject(M::model_format())
    }

    fn object_type_and_id(&self, id: ObjectStoreId) -> ObjectTypeAndId {
        ObjectTypeAndId::GenericStringObject {
            object_type: M::model_format(),
            id,
        }
    }

    fn display_name(&self) -> String {
        self.string_model.display_name()
    }

    fn set_display_name(&mut self, name: &str) {
        self.string_model.set_display_name(name);
    }

    fn upsert_event(
        &self,
        object: &GenericStoredObject<GenericStringObjectId, Self>,
    ) -> ModelEvent {
        let object = object as &dyn StoredStringObject;
        ModelEvent::UpsertGenericStringObject {
            object: StoredStringObject::clone_box(object),
        }
    }

    fn should_show_activity_toasts(&self) -> bool {
        M::should_show_activity_toasts()
    }

    fn warn_if_unsaved_at_quit(&self) -> bool {
        M::warn_if_unsaved_at_quit()
    }

    fn can_export(&self) -> bool {
        self.string_model.can_export()
    }

    fn bulk_upsert_event(
        objects: &[GenericStoredObject<GenericStringObjectId, Self>],
    ) -> ModelEvent {
        ModelEvent::UpsertGenericStringObjects(
            objects.iter().map(StoredStringObject::clone_box).collect(),
        )
    }

    fn should_clear_on_unique_key_conflict(&self) -> bool {
        self.string_model.should_clear_on_unique_key_conflict()
    }

    fn should_update_after_stored_revision_conflict(&self) -> bool {
        true
    }

    fn serialized(&self) -> SerializedModel {
        S::serialize(&self.string_model)
    }

    fn renders_in_local_drive(&self) -> bool {
        self.string_model.renders_in_local_drive()
    }

    fn to_local_drive_item(
        &self,
        id: ObjectStoreId,
        appearance: &Appearance,
        object: &GenericStoredObject<GenericStringObjectId, Self>,
    ) -> Option<Box<dyn LocalDriveItem>> {
        self.string_model
            .to_local_drive_item(id, appearance, object)
    }
}

impl<M, S> GenericStringModel<M, S>
where
    M: StringModel<
        StoredObjectType = GenericStoredObject<GenericStringObjectId, GenericStringModel<M, S>>,
    >,
    S: Serializer<M>,
{
    pub fn deserialize_owned(serialized: &str) -> Result<Self> {
        S::deserialize_owned(serialized).map(Self::new)
    }

    pub fn new(model: M) -> Self {
        Self {
            string_model: model,
        }
    }

    pub fn json_model(&self) -> &M {
        &self.string_model
    }
}

/// Object id type that is common for all generic string objects.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct GenericStringObjectId(StableObjectId);
crate::stable_object_id_traits! { GenericStringObjectId, "GenericStringObject" }

impl From<GenericStringObjectId> for ObjectStoreId {
    fn from(id: GenericStringObjectId) -> Self {
        Self::StableId(id.into())
    }
}

impl GenericStringObjectId {
    pub fn uid(&self) -> ObjectUid {
        self.0.uid()
    }
}
