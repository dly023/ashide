mod assertion;

pub use assertion::*;
use itertools::Itertools;
use std::future::Future;
use std::pin::Pin;
use warpui::{App, SingletonEntity};

use crate::{
    object_store::update_manager::UpdateManager,
    object_store::{model::persistence::ObjectStoreModel, Space},
};

/// Clears the object store of all non-welcome objects in the user's personal space.
/// Returns a future that resolves when the object store is cleared.
pub fn clear_object_store_model(app: &mut App) -> Pin<Box<dyn Future<Output = ()> + Send>> {
    let object_ids_to_delete =
        ObjectStoreModel::handle(app).read(app, |object_store_model, ctx| {
            object_store_model
                .active_non_welcome_stored_objects_in_space(Space::Personal, ctx)
                .map(|object| object.object_type_and_id())
                .collect_vec()
        });

    UpdateManager::handle(app).update(app, |update_manager, ctx| {
        for object_id in object_ids_to_delete {
            update_manager.delete_object_by_user(object_id, ctx);
        }
    });

    Box::pin(async {})
}
