use warp_core::ui::appearance::Appearance;
use warpui::{
    platform::WindowStyle, AddSingletonModel, App, SingletonEntity, TypedActionView, ViewHandle,
};

use crate::{
    ai::blocklist::BlocklistAIHistoryModel,
    auth::{AuthManager, AuthStateProvider},
    drive::{
        folders::{FolderObject, FolderObjectModel},
        items::LocalDriveItemId,
        ObjectTypeAndId,
    },
    menu::MenuItem,
    network::NetworkStatus,
    notebooks::{NotebookObject, NotebookObjectModel},
    object_store::ids::{ClientId, ObjectStoreId},
    object_store::{
        model::{
            actions::ObjectActions, persistence::ObjectStoreModel, view::ObjectStoreViewModel,
        },
        update_manager::UpdateManager,
        ObjectType, Owner, Space, StoredObjectLocation, StoredObjectSyncStatus,
    },
    settings_view::keybindings::KeybindingChangedNotifier,
    test_util::settings::initialize_settings_for_tests,
    workflows::{workflow::Workflow, WorkflowObject, WorkflowObjectModel},
    workspaces::{user_profiles::UserProfiles, user_workspaces::UserWorkspaces},
    Assets,
};

use super::{DriveIndex, DriveIndexAction};

fn initialize_app(app: &mut App) {
    initialize_settings_for_tests(app);

    app.add_singleton_model(ObjectStoreModel::mock);
    app.add_singleton_model(UserWorkspaces::default_mock);
    app.add_singleton_model(|_| NetworkStatus::new());
    app.add_singleton_model(|_| Appearance::mock());
    app.add_singleton_model(|_| AuthStateProvider::new_for_test());
    app.add_singleton_model(AuthManager::new_for_test);
    app.add_singleton_model(UpdateManager::mock);
    app.add_singleton_model(ObjectStoreViewModel::mock);
    app.add_singleton_model(|_| ObjectActions::new(Vec::new()));
    app.add_singleton_model(|_| UserProfiles::new(Vec::new()));
    app.add_singleton_model(|_| KeybindingChangedNotifier::mock());
    app.add_singleton_model(|_| BlocklistAIHistoryModel::new_for_test());
    #[cfg(feature = "voice_input")]
    app.add_singleton_model(voice_input::VoiceInput::new);
}

fn create_index(app: &mut App) -> ViewHandle<DriveIndex> {
    let (_, index) = app.add_window(WindowStyle::NotStealFocus, DriveIndex::new);
    index
}

fn create_workflow(app: &mut App) -> ObjectStoreId {
    ObjectStoreModel::handle(app).update(app, |object_store_model, ctx| {
        let client_id = ClientId::new();
        let object_store_id = ObjectStoreId::ClientId(client_id);
        let workflow = Workflow::new("my workflow", "my command");
        object_store_model.create_object(
            object_store_id,
            WorkflowObject::new_local(
                WorkflowObjectModel::new(workflow),
                Owner::mock_current_user(),
                None,
                client_id,
            ),
            ctx,
        );
        object_store_id
    })
}

fn create_notebook(app: &mut App) -> ObjectStoreId {
    ObjectStoreModel::handle(app).update(app, |object_store_model, ctx| {
        let client_id = ClientId::new();
        let object_store_id = ObjectStoreId::ClientId(client_id);
        object_store_model.create_object(
            object_store_id,
            NotebookObject::new_local(
                NotebookObjectModel::default(),
                Owner::mock_current_user(),
                None,
                client_id,
            ),
            ctx,
        );
        object_store_id
    })
}

fn create_folder(app: &mut App) -> ObjectStoreId {
    ObjectStoreModel::handle(app).update(app, |object_store_model, ctx| {
        let client_id = ClientId::new();
        let object_store_id = ObjectStoreId::ClientId(client_id);
        object_store_model.create_object(
            object_store_id,
            FolderObject::new_local(
                FolderObjectModel::new("local folder", false),
                Owner::mock_current_user(),
                None,
                client_id,
            ),
            ctx,
        );
        object_store_id
    })
}

fn set_object_in_error(app: &mut App, object_type_and_id: &ObjectTypeAndId) {
    ObjectStoreModel::handle(app).update(
        app,
        |object_store_model, _ctx: &mut warpui::ModelContext<'_, ObjectStoreModel>| {
            if let Some(object) = object_store_model.get_mut_by_uid(&object_type_and_id.uid()) {
                object.set_pending_content_changes_status(StoredObjectSyncStatus::Errored);
            }
        },
    );
}

fn label_for_menu_item(item: &MenuItem<DriveIndexAction>) -> &str {
    if let MenuItem::Item(item) = item {
        item.label()
    } else {
        panic!("item provided wasn't of type MenuItem::Item")
    }
}

fn assert_local_only_workflow_menu(menu_items: &[MenuItem<DriveIndexAction>]) {
    let labels = menu_items
        .iter()
        .map(label_for_menu_item)
        .collect::<Vec<_>>();
    assert_eq!(
        labels,
        vec![
            "drive-edit",
            "drive-copy-workflow-text",
            "drive-duplicate",
            "drive-export",
            "drive-trash-menu",
        ]
    );
    let server_retry_label = ["drive", "retry"].join("-");
    let server_revert_label = ["drive", "revert", "to", "server"].join("-");
    assert!(!labels.iter().any(|label| *label == server_retry_label));
    assert!(!labels.iter().any(|label| *label == server_revert_label));
    assert!(!labels.contains(&"drive-share"));
}

#[test]
fn test_local_client_object_moves_into_local_folder_offline() {
    App::test(Assets, |mut app| async move {
        initialize_app(&mut app);
        let notebook_id = create_notebook(&mut app);
        let folder_id = create_folder(&mut app);
        let object_type_and_id =
            ObjectTypeAndId::from_id_and_type(notebook_id, ObjectType::Notebook);

        NetworkStatus::handle(&app).update(&mut app, |network_status, ctx| {
            network_status.reachability_changed(false, ctx);
        });

        ObjectStoreModel::handle(&app).read(&app, |object_store_model, ctx| {
            assert_eq!(
                object_store_model.object_location(&object_type_and_id.uid(), ctx),
                Some(StoredObjectLocation::Space(Space::Personal))
            );
        });

        UpdateManager::handle(&app).update(&mut app, |update_manager, ctx| {
            update_manager.move_object_to_location(
                object_type_and_id,
                StoredObjectLocation::Folder(folder_id),
                ctx,
            );
        });

        ObjectStoreModel::handle(&app).read(&app, |object_store_model, ctx| {
            assert_eq!(
                object_store_model.object_location(&object_type_and_id.uid(), ctx),
                Some(StoredObjectLocation::Folder(folder_id))
            );
        });
    })
}

#[test]
fn test_failed_sync_retry_menu_items_are_removed() {
    App::test(Assets, |mut app| async move {
        initialize_app(&mut app);
        let index = create_index(&mut app);
        let object_store_id = create_workflow(&mut app);
        let object_type_and_id: ObjectTypeAndId =
            ObjectTypeAndId::from_id_and_type(object_store_id, ObjectType::Workflow);
        let local_drive_item_id = LocalDriveItemId::Object(object_type_and_id);

        // by default, it doesn't show up
        index.update(&mut app, |index, ctx| {
            let menu_items = index.menu_items(&Space::Personal, &local_drive_item_id, ctx);
            assert_local_only_workflow_menu(&menu_items);
        });

        // Ashide Drive is local-only; sync error metadata must not surface
        // server retry/revert actions in the active product UI.
        set_object_in_error(&mut app, &object_type_and_id);
        index.update(&mut app, |index, ctx| {
            let menu_items = index.menu_items(&Space::Personal, &local_drive_item_id, ctx);
            assert_local_only_workflow_menu(&menu_items);
        });

        // Offline state should not change the local-only menu shape.
        NetworkStatus::handle(&app).update(&mut app, |network_status, ctx| {
            network_status.reachability_changed(false, ctx);
        });
        index.update(&mut app, |index, ctx| {
            let menu_items = index.menu_items(&Space::Personal, &local_drive_item_id, ctx);
            assert_local_only_workflow_menu(&menu_items);
        });
    })
}

#[test]
fn test_local_client_delete_forever_removes_object() {
    App::test(Assets, |mut app| async move {
        initialize_app(&mut app);
        let object_store_id = create_workflow(&mut app);
        let object_type_and_id: ObjectTypeAndId =
            ObjectTypeAndId::from_id_and_type(object_store_id, ObjectType::Workflow);

        UpdateManager::handle(&app).update(&mut app, |update_manager, ctx| {
            update_manager.trash_object(object_type_and_id, ctx);
            update_manager.delete_object_by_user(object_type_and_id, ctx);
        });

        ObjectStoreModel::handle(&app).read(&app, |object_store_model, _| {
            assert!(
                object_store_model
                    .get_by_uid(&object_type_and_id.uid())
                    .is_none(),
                "local ClientId object should be permanently deleted without a server id",
            );
        });
    })
}

#[test]
fn test_local_drive_navigation_states() {
    use crate::drive::index::DriveIndexAction;
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let index = create_index(&mut app);
        let object_store_id = create_notebook(&mut app);
        let object_type_and_id: ObjectTypeAndId =
            ObjectTypeAndId::from_id_and_type(object_store_id, ObjectType::Notebook);

        index.read(&app, |index, _| {
            assert_eq!(index.selected, None, "Expect selected to be None");
            assert_eq!(
                index.focused_index,
                Some(0),
                "Expect focused_index to be initialized"
            );
        });

        index.update(&mut app, |index, ctx| {
            index.handle_action(&DriveIndexAction::OpenObject(object_type_and_id), ctx);
        });

        index.read(&app, |index, _| {
            assert_eq!(
                index.selected,
                Some(LocalDriveItemId::Object(object_type_and_id)),
                "Expect selected to have correct value"
            );
        });
    });
}
