//! Supporting types for persisting local object-store records to SQLite.

use diesel::{result::Error, SqliteConnection};

use crate::{
    object_store::{
        ObjectIdType, ObjectType, Owner, StoredObjectMetadata, StoredObjectPermissions,
    },
    persistence::{model::ObjectMetadata, schema},
};
use persistence::model::{NewObjectMetadata, NewObjectPermissions};

pub type StoredObjectId = i32;
pub type CreateStoredObjectFn =
    Box<dyn FnOnce(&mut SqliteConnection) -> Result<StoredObjectId, Error>>;
pub type UpdateStoredObjectFn =
    Box<dyn FnOnce(&mut SqliteConnection, StoredObjectId) -> Result<(), Error>>;

pub fn upsert_stored_object(
    conn: &mut SqliteConnection,
    stored_object_type: ObjectType,
    object_store_id: crate::object_store::ids::ObjectStoreId,
    stored_object_metadata: StoredObjectMetadata,
    stored_object_permissions: StoredObjectPermissions,
    create_object_fn: CreateStoredObjectFn,
    update_object_fn: UpdateStoredObjectFn,
) -> Result<(), Error> {
    use schema::object_metadata::dsl::{
        client_id, current_editor, folder_id, is_pending, last_editor_uid,
        metadata_last_updated_ts, object_metadata, revision_ts, server_id, trashed_ts,
    };
    use schema::object_permissions::dsl::{
        anyone_with_link_access_level, anyone_with_link_source, object_guests, object_metadata_id,
        object_permissions, permissions_last_updated_at, subject_id, subject_type, subject_uid,
    };

    use diesel::prelude::*;

    let Owner::User { user_uid } = stored_object_permissions.owner;
    let (subject_type_value, subject_id_value, subject_uid_value) =
        ("USER", Some(user_uid.to_string()), user_uid.to_string());
    let permissions_ts = stored_object_permissions
        .permissions_last_updated_ts
        .map(|ts| ts.timestamp_micros());

    let revision = stored_object_metadata
        .revision
        .as_ref()
        .map(|r| r.timestamp_micros());
    let has_pending_content_changes = stored_object_metadata.has_pending_content_changes();
    let hashed_object_store_id = object_store_id.sqlite_uid_hash(stored_object_type.into());
    let metadata_filter = object_metadata
        .filter(client_id.eq(Some(hashed_object_store_id.as_str())))
        .or_filter(server_id.eq(Some(hashed_object_store_id.as_str())));
    let metadata: Option<ObjectMetadata> = metadata_filter.first(conn).ok();

    match metadata {
        Some(metadata) => {
            update_object_fn(conn, metadata.shareable_object_id)?;

            let metadata_last_updated_at = stored_object_metadata
                .metadata_last_updated_ts
                .map(|ts| ts.timestamp_micros());
            let trashed_timestamp = stored_object_metadata
                .trashed_ts
                .map(|ts| ts.timestamp_micros());
            let folder_id_str = stored_object_metadata
                .folder_id
                .map(|folder_object_store_id| {
                    folder_object_store_id.sqlite_uid_hash(ObjectIdType::Folder)
                });

            diesel::update(metadata_filter)
                .set((
                    revision_ts.eq(revision),
                    is_pending.eq(has_pending_content_changes),
                    last_editor_uid.eq(stored_object_metadata.last_editor_uid),
                ))
                .execute(conn)?;

            if !stored_object_metadata
                .pending_changes_statuses
                .has_pending_metadata_change
            {
                diesel::update(metadata_filter)
                    .set((
                        metadata_last_updated_ts.eq(metadata_last_updated_at),
                        trashed_ts.eq(trashed_timestamp),
                        folder_id.eq(folder_id_str),
                        current_editor.eq(stored_object_metadata.current_editor_uid),
                    ))
                    .execute(conn)?;
            }

            if !stored_object_metadata
                .pending_changes_statuses
                .has_pending_permissions_change
            {
                let permissions_filter =
                    object_permissions.filter(object_metadata_id.eq(metadata.id));
                diesel::update(permissions_filter)
                    .set((
                        subject_type.eq(subject_type_value),
                        subject_id.eq(subject_id_value),
                        subject_uid.eq(subject_uid_value),
                        permissions_last_updated_at.eq(permissions_ts),
                        object_guests.eq(Option::<Vec<u8>>::None),
                        anyone_with_link_access_level.eq(Option::<&str>::None),
                        anyone_with_link_source.eq(Option::<Vec<u8>>::None),
                    ))
                    .execute(conn)?;
            }
        }
        None => {
            let object_id = create_object_fn(conn)?;
            let mut new_object_metadata = NewObjectMetadata {
                object_type: stored_object_type.sqlite_object_type_as_str().to_string(),
                revision_ts: revision,
                shareable_object_id: object_id,
                is_pending: has_pending_content_changes,
                retry_count: 0,
                author_id: None,
                client_id: None,
                server_id: None,
                metadata_last_updated_ts: stored_object_metadata
                    .metadata_last_updated_ts
                    .map(|ts| ts.timestamp_micros()),
                trashed_ts: stored_object_metadata
                    .trashed_ts
                    .map(|ts| ts.timestamp_micros()),
                folder_id: stored_object_metadata
                    .folder_id
                    .map(|object_store_id| object_store_id.sqlite_uid_hash(ObjectIdType::Folder)),
                is_welcome_object: stored_object_metadata.is_welcome_object,
                creator_uid: stored_object_metadata.creator_uid,
                last_editor_uid: stored_object_metadata.last_editor_uid,
                current_editor: stored_object_metadata.current_editor_uid,
            };

            match object_store_id {
                crate::object_store::ids::ObjectStoreId::ClientId(_) => {
                    new_object_metadata.client_id = Some(hashed_object_store_id);
                }
                crate::object_store::ids::ObjectStoreId::StableId(_) => {
                    new_object_metadata.server_id = Some(hashed_object_store_id);
                }
            }
            diesel::insert_into(schema::object_metadata::dsl::object_metadata)
                .values(new_object_metadata)
                .execute(conn)?;

            let metadata_id: i32 = schema::object_metadata::dsl::object_metadata
                .select(schema::object_metadata::dsl::id)
                .order(schema::object_metadata::dsl::id.desc())
                .first(conn)?;

            let new_object_permissions = NewObjectPermissions {
                object_metadata_id: metadata_id,
                subject_type: subject_type_value.to_owned(),
                subject_id: subject_id_value,
                subject_uid: subject_uid_value,
                permissions_last_updated_at: permissions_ts,
                object_guests: None,
                anyone_with_link_access_level: None,
                anyone_with_link_source: None,
            };
            diesel::insert_into(schema::object_permissions::dsl::object_permissions)
                .values(new_object_permissions)
                .execute(conn)?;
        }
    }

    Ok(())
}
