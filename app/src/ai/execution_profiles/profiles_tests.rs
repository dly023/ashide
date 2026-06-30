use warpui::{App, SingletonEntity};

use crate::ai::execution_profiles::profiles::AIExecutionProfilesModel;
use crate::ai::execution_profiles::{
    AIExecutionProfile, AIExecutionProfileObject, AIExecutionProfileObjectModel, ActionPermission,
};
use crate::ai::mcp::TemplatableMCPServerManager;
use crate::auth::AuthStateProvider;
use crate::network::NetworkStatus;
use crate::object_store::ids::{ObjectStoreId, StableObjectId};
use crate::object_store::model::persistence::{ObjectStoreEvent, ObjectStoreModel};
use crate::object_store::update_manager::UpdateManager;
use crate::object_store::{StoredObjectMetadata, StoredObjectPermissions};
use crate::settings::PrivacySettings;
use crate::test_util::settings::initialize_settings_for_tests;
use crate::workspaces::user_workspaces::UserWorkspaces;
use crate::LaunchMode;

/// Install the minimal singleton graph needed to construct an
/// `AIExecutionProfilesModel` and exercise its ObjectStoreModel interactions.
fn install_singletons(app: &mut App, auth_state: AuthStateProvider) {
    initialize_settings_for_tests(app);
    app.add_singleton_model(|_| auth_state);
    app.add_singleton_model(|_| NetworkStatus::new());
    app.add_singleton_model(UpdateManager::mock);
    app.add_singleton_model(ObjectStoreModel::mock);
    app.add_singleton_model(|_| TemplatableMCPServerManager::default());
    app.add_singleton_model(PrivacySettings::mock);
    app.add_singleton_model(UserWorkspaces::default_mock);
}

/// Regression test for the onboarding autonomy bug where
/// `edit_profile_internal` would silently drop edits made to an `InlineLocal`
/// default profile whenever `personal_drive` returned `None` (logged-out
/// users). `apply_agent_settings` calls `set_*` on the default profile the
/// moment onboarding completes, which can happen before the user logs in
/// (e.g. `LoginSlideEvent::LoginLaterConfirmed`), so those edits must
/// persist on the local `InlineLocal` state rather than being dropped.
#[test]
fn edits_persist_on_inline_local_default_profile_when_logged_out() {
    App::test((), |mut app| async move {
        install_singletons(&mut app, AuthStateProvider::new_logged_out_for_test());
        let profile_model = app.add_singleton_model(|ctx| {
            AIExecutionProfilesModel::new(&LaunchMode::new_for_unit_test(), ctx)
        });

        let default_profile_id = profile_model.read(&app, |model, _ctx| model.default_profile_id());

        // Sanity-check the precondition: the baseline `apply_code_diffs`
        // on a fresh default profile is the enum default (`AgentDecides`).
        profile_model.read(&app, |model, ctx| {
            assert!(
                matches!(
                    model.default_profile(ctx).data().apply_code_diffs,
                    ActionPermission::AgentDecides
                ),
                "unexpected baseline apply_code_diffs"
            );
        });

        // Apply the edit that onboarding would make for the Full autonomy
        // preset. Before the fix, this call no-ops because
        // `personal_drive` is `None` while the profile is `InlineLocal` — the
        // `set_apply_code_diffs` value was cloned, mutated, then dropped
        // without being written back to `default_profile_state`.
        profile_model.update(&mut app, |model, ctx| {
            model.set_apply_code_diffs(default_profile_id, &ActionPermission::AlwaysAllow, ctx);
        });

        profile_model.read(&app, |model, ctx| {
            assert_eq!(
                model.default_profile(ctx).data().apply_code_diffs,
                ActionPermission::AlwaysAllow,
                "edit was dropped: default profile still has the baseline \
                 apply_code_diffs value after an edit made while logged out",
            );
        });
    })
}

/// Regression test for the "log in to an existing user after onboarding"
/// bug. Objects restored from local storage can already exist in `ObjectStoreModel`
/// before `AIExecutionProfilesModel` observes per-object `ObjectCreated` events.
/// The model reconciles when it receives `ObjectStoreEvent::InitialLoadCompleted`.
/// Without the reconciliation handler for `InitialLoadCompleted`, the
/// existing user's default profile sits in `ObjectStoreModel` but
/// `AIExecutionProfilesModel` stays in `InlineLocal`, so a subsequent
/// onboarding edit creates a duplicate object-store default profile instead of
/// editing the existing one. This test drives that sequence and asserts
/// the model adopts the stored profile's ObjectStore ID.
#[test]
fn reconciles_inline_local_default_profile_with_object_store_after_initial_load() {
    App::test((), |mut app| async move {
        install_singletons(&mut app, AuthStateProvider::new_for_test());
        let profile_model = app.add_singleton_model(|ctx| {
            AIExecutionProfilesModel::new(&LaunchMode::new_for_unit_test(), ctx)
        });

        // Baseline: ObjectStoreModel is empty, so the model starts InlineLocal and
        // object-store ID is `None`.
        profile_model.read(&app, |model, ctx| {
            assert!(
                model.default_profile(ctx).object_store_id().is_none(),
                "default profile should be InlineLocal at startup"
            );
        });

        // Simulate the user's existing default profile object arriving via
        // initial bulk load. We construct the existing profile with
        // `apply_code_diffs = AlwaysAllow` so we can verify the model is
        // reading that stored object after reconciliation.
        let stored_profile_server_id = StableObjectId::from(42);
        let object_store_id = ObjectStoreId::StableId(stored_profile_server_id);
        let local_profile = AIExecutionProfile {
            name: "Default".to_string(),
            is_default_profile: true,
            apply_code_diffs: ActionPermission::AlwaysAllow,
            ..Default::default()
        };
        let profile_object = AIExecutionProfileObject::new(
            object_store_id,
            AIExecutionProfileObjectModel::new(local_profile),
            StoredObjectMetadata::mock(),
            StoredObjectPermissions::mock_personal(),
        );

        // Insert the object into ObjectStoreModel without per-object events and then
        // emit `InitialLoadCompleted` so the reconciliation handler fires.
        ObjectStoreModel::handle(&app).update(&mut app, move |object_store_model, ctx| {
            object_store_model.add_object(object_store_id, profile_object);
            ctx.emit(ObjectStoreEvent::InitialLoadCompleted);
        });

        // The model should now be ObjectBacked with the stored profile object's object_store_id,
        // and `default_profile` should read values from the existing local
        // object (proving we're not backed by a fresh client-side default).
        profile_model.read(&app, |model, ctx| {
            let info = model.default_profile(ctx);
            assert_eq!(
                info.object_store_id(),
                Some(object_store_id),
                "model did not adopt the existing default profile object's ObjectStore ID"
            );
            assert_eq!(
                info.data().apply_code_diffs,
                ActionPermission::AlwaysAllow,
                "default profile should now surface the existing stored value"
            );
        });

        // Further edits should now target the existing profile object in
        // place, rather than falling through the `InlineLocal` branch and
        // creating a duplicate.
        let default_profile_id = profile_model.read(&app, |model, _ctx| model.default_profile_id());
        profile_model.update(&mut app, |model, ctx| {
            model.set_apply_code_diffs(default_profile_id, &ActionPermission::AlwaysAsk, ctx);
        });
        profile_model.read(&app, |model, ctx| {
            let info = model.default_profile(ctx);
            assert_eq!(
                info.object_store_id(),
                Some(object_store_id),
                "edit should target the same ObjectStore object, not create a duplicate"
            );
            assert_eq!(
                info.data().apply_code_diffs,
                ActionPermission::AlwaysAsk,
                "edit should be reflected on the existing profile object"
            );
        });
    })
}
