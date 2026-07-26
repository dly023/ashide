use std::fs;

use futures::{channel::mpsc, StreamExt as _};
use repo_metadata::repositories::DetectedRepositories;
use warp_core::ui::appearance::Appearance;
use warp_editor::render::element::VerticalExpansionBehavior;
use warp_files::FileModel;
use warp_util::content_version::ContentVersion;
use warpui::{platform::WindowStyle, App, ViewHandle};

use crate::{
    code::{
        buffer_location::EnvironmentFilePath,
        current_app_code_editor::{CurrentAppCodeEditorEvent, CurrentAppCodeEditorView},
        editor::view::{CodeEditorRenderOptions, CodeEditorView},
    },
    editor::InteractionState,
    notebooks::editor::keys::NotebookKeybindings,
    object_store::model::persistence::ObjectStoreModel,
    settings_view::keybindings::KeybindingChangedNotifier,
    test_util::settings::initialize_settings_for_tests,
    vim_registers::VimRegisters,
    workspace::{sync_inputs::SyncedInputState, ActiveSession},
    workspaces::user_workspaces::UserWorkspaces,
    AuthStateProvider,
};

use super::*;

#[derive(Debug)]
enum TestEvent {
    Loaded(FileId),
    FailedToLoad(Rc<FileLoadError>),
    Saved(FileId),
    FailedToSave(FileId),
    Other,
}

fn setup_event_channel(
    app: &mut App,
    model: &warpui::ModelHandle<GlobalBufferModel>,
) -> mpsc::UnboundedReceiver<TestEvent> {
    let (sender, receiver) = mpsc::unbounded();
    app.update(|ctx| {
        ctx.subscribe_to_model(model, move |_model, event, _ctx| {
            let event = match event {
                GlobalBufferModelEvent::BufferLoaded { file_id, .. } => TestEvent::Loaded(*file_id),
                GlobalBufferModelEvent::FailedToLoad { error, .. } => {
                    TestEvent::FailedToLoad(error.clone())
                }
                GlobalBufferModelEvent::FileSaved { file_id } => TestEvent::Saved(*file_id),
                GlobalBufferModelEvent::FailedToSave { file_id, .. } => {
                    TestEvent::FailedToSave(*file_id)
                }
                GlobalBufferModelEvent::BufferUpdatedFromFileEvent { .. }
                | GlobalBufferModelEvent::EnvironmentBufferConflict { .. }
                | GlobalBufferModelEvent::ServerCurrentAppFileSystemBufferUpdated { .. } => {
                    TestEvent::Other
                }
            };
            sender.unbounded_send(event).unwrap();
        });
    });
    receiver
}

async fn wait_until_loaded(receiver: &mut mpsc::UnboundedReceiver<TestEvent>) -> FileId {
    loop {
        match receiver.next().await.unwrap() {
            TestEvent::Loaded(file_id) => return file_id,
            TestEvent::FailedToLoad(error) => panic!("fixture failed to load: {error}"),
            TestEvent::Saved(_) | TestEvent::FailedToSave(_) | TestEvent::Other => {}
        }
    }
}

fn initialize_environment_editor_app(app: &mut App) -> warpui::ModelHandle<GlobalBufferModel> {
    initialize_settings_for_tests(app);
    app.add_singleton_model(|_| Appearance::mock());
    app.add_singleton_model(|_| SyncedInputState::mock());
    app.add_singleton_model(|_| VimRegisters::new());
    app.add_singleton_model(|_| KeybindingChangedNotifier::mock());
    app.add_singleton_model(|_| AuthStateProvider::new_for_test());
    app.add_singleton_model(ObjectStoreModel::mock);
    app.add_singleton_model(|_| ActiveSession::default());
    app.add_singleton_model(NotebookKeybindings::new);
    app.add_singleton_model(|ctx| UserWorkspaces::mock(vec![], ctx));
    app.add_singleton_model(|_| DetectedRepositories::default());
    app.add_singleton_model(FileModel::new);
    app.add_singleton_model(crate::workspace::environment_runtime::new_transport_manager);
    app.add_singleton_model(GlobalBufferModel::new)
}

fn add_environment_editor(
    app: &mut App,
    environment_file_path: EnvironmentFilePath,
    binding_session_id: warp_core::SessionId,
) -> ViewHandle<CurrentAppCodeEditorView> {
    let (_window, editor) = app.add_window(WindowStyle::NotStealFocus, move |ctx| {
        CurrentAppCodeEditorView::new_with_environment_buffer(
            environment_file_path,
            binding_session_id,
            |buffer_state, ctx| {
                ctx.add_typed_action_view(|ctx| {
                    CodeEditorView::new(
                        None,
                        Some(buffer_state.buffer),
                        CodeEditorRenderOptions::new(VerticalExpansionBehavior::FillMaxHeight),
                        ctx,
                    )
                })
            },
            false,
            None,
            ctx,
        )
    });
    editor
}

fn assert_committed_state_unchanged(
    app: &App,
    model: &warpui::ModelHandle<GlobalBufferModel>,
    file_id: FileId,
    expected_content: &str,
    expected_server_version: ContentVersion,
    expected_client_version: ContentVersion,
) {
    model.read(app, |model, ctx| {
        assert_eq!(
            model.content_for_file(file_id, ctx).as_deref(),
            Some(expected_content)
        );
        let clock = model
            .sync_clock_for_server_current_app(file_id)
            .expect("server buffer must retain its sync clock");
        assert_eq!(clock.server_version, expected_server_version);
        assert_eq!(clock.client_version, expected_client_version);
    });
}

#[test]
fn resolve_conflict_keeps_canonical_state_until_file_saved() {
    App::test((), |mut app| async move {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("note.txt");
        fs::write(&path, "committed").unwrap();

        app.add_singleton_model(|_| DetectedRepositories::default());
        app.add_singleton_model(FileModel::new);
        let model = app.add_singleton_model(GlobalBufferModel::new);
        let mut events = setup_event_channel(&mut app, &model);
        let _buffer_state = model.update(&mut app, |model, ctx| {
            model.open_server_current_app(path, ctx)
        });
        let file_id = wait_until_loaded(&mut events).await;
        let (server_version, client_version) = model.read(&app, |model, _ctx| {
            let clock = model
                .sync_clock_for_server_current_app(file_id)
                .expect("server buffer must be loaded");
            (clock.server_version, clock.client_version)
        });
        let resolved_client_version = ContentVersion::new();

        model
            .update(&mut app, |model, ctx| {
                model.resolve_conflict(
                    file_id,
                    server_version,
                    resolved_client_version,
                    "client-wins",
                    ctx,
                )
            })
            .unwrap();

        assert_committed_state_unchanged(
            &app,
            &model,
            file_id,
            "committed",
            server_version,
            client_version,
        );

        loop {
            if matches!(
                events.next().await.unwrap(),
                TestEvent::Saved(saved_id) if saved_id == file_id
            ) {
                break;
            }
        }
        model.read(&app, |model, ctx| {
            assert_eq!(
                model.content_for_file(file_id, ctx).as_deref(),
                Some("client-wins")
            );
            let clock = model
                .sync_clock_for_server_current_app(file_id)
                .expect("server buffer must retain its sync clock");
            assert_eq!(clock.server_version, server_version);
            assert_eq!(clock.client_version, resolved_client_version);
        });
    });
}

#[test]
fn failed_resolve_conflict_preserves_committed_buffer_and_clock() {
    App::test((), |mut app| async move {
        let fixture = tempfile::tempdir().unwrap();
        let parent = fixture.path().join("container");
        fs::create_dir(&parent).unwrap();
        let path = parent.join("note.txt");
        fs::write(&path, "committed").unwrap();

        app.add_singleton_model(|_| DetectedRepositories::default());
        app.add_singleton_model(FileModel::new);
        let model = app.add_singleton_model(GlobalBufferModel::new);
        let mut events = setup_event_channel(&mut app, &model);
        let _buffer_state = model.update(&mut app, |model, ctx| {
            model.open_server_current_app(path, ctx)
        });
        let file_id = wait_until_loaded(&mut events).await;
        let (server_version, client_version) = model.read(&app, |model, _ctx| {
            let clock = model
                .sync_clock_for_server_current_app(file_id)
                .expect("server buffer must be loaded");
            (clock.server_version, clock.client_version)
        });

        fs::remove_file(parent.join("note.txt")).unwrap();
        fs::remove_dir(&parent).unwrap();
        fs::write(&parent, "not a directory").unwrap();

        model
            .update(&mut app, |model, ctx| {
                model.resolve_conflict(
                    file_id,
                    server_version,
                    ContentVersion::new(),
                    "must-not-commit",
                    ctx,
                )
            })
            .unwrap();

        loop {
            if matches!(
                events.next().await.unwrap(),
                TestEvent::FailedToSave(failed_id) if failed_id == file_id
            ) {
                break;
            }
        }

        assert_committed_state_unchanged(
            &app,
            &model,
            file_id,
            "committed",
            server_version,
            client_version,
        );
    });
}

#[test]
fn queued_save_after_failed_resolve_uses_committed_state() {
    App::test((), |mut app| async move {
        let fixture = tempfile::tempdir().unwrap();
        let parent = fixture.path().join("container");
        fs::create_dir(&parent).unwrap();
        let path = parent.join("note.txt");
        fs::write(&path, "committed").unwrap();

        app.add_singleton_model(|_| DetectedRepositories::default());
        app.add_singleton_model(FileModel::new);
        let model = app.add_singleton_model(GlobalBufferModel::new);
        let mut events = setup_event_channel(&mut app, &model);
        let _buffer_state = model.update(&mut app, |model, ctx| {
            model.open_server_current_app(path.clone(), ctx)
        });
        let file_id = wait_until_loaded(&mut events).await;
        let (server_version, client_version) = model.read(&app, |model, _ctx| {
            let clock = model
                .sync_clock_for_server_current_app(file_id)
                .expect("server buffer must be loaded");
            (clock.server_version, clock.client_version)
        });

        fs::remove_file(&path).unwrap();
        fs::remove_dir(&parent).unwrap();
        fs::write(&parent, "not a directory").unwrap();
        model
            .update(&mut app, |model, ctx| {
                model.resolve_conflict(
                    file_id,
                    server_version,
                    ContentVersion::new(),
                    "failed-client-content",
                    ctx,
                )
            })
            .unwrap();
        loop {
            if matches!(
                events.next().await.unwrap(),
                TestEvent::FailedToSave(failed_id) if failed_id == file_id
            ) {
                break;
            }
        }
        assert_committed_state_unchanged(
            &app,
            &model,
            file_id,
            "committed",
            server_version,
            client_version,
        );

        fs::remove_file(&parent).unwrap();
        fs::create_dir(&parent).unwrap();
        model
            .update(&mut app, |model, ctx| {
                model.save_server_current_app(file_id, ctx)
            })
            .unwrap();
        loop {
            if matches!(
                events.next().await.unwrap(),
                TestEvent::Saved(saved_id) if saved_id == file_id
            ) {
                break;
            }
        }

        assert_eq!(fs::read_to_string(&path).unwrap(), "committed");
        assert_committed_state_unchanged(
            &app,
            &model,
            file_id,
            "committed",
            server_version,
            client_version,
        );
    });
}

#[test]
fn shared_buffer_close_releases_only_last_consumer() {
    App::test((), |mut app| async move {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("shared.txt");
        fs::write(&path, "shared").unwrap();

        app.add_singleton_model(|_| DetectedRepositories::default());
        app.add_singleton_model(FileModel::new);
        let model = app.add_singleton_model(GlobalBufferModel::new);
        let mut events = setup_event_channel(&mut app, &model);
        let first = model.update(&mut app, |model, ctx| {
            model.open_server_current_app(path.clone(), ctx)
        });
        let file_id = wait_until_loaded(&mut events).await;
        let second = model.update(&mut app, |model, ctx| {
            model.open_server_current_app(path, ctx)
        });
        assert_eq!(first.file_id, second.file_id);

        model.update(&mut app, |model, ctx| model.close_buffer(file_id, ctx));
        model.read(&app, |model, ctx| {
            assert_eq!(
                model.content_for_file(file_id, ctx).as_deref(),
                Some("shared")
            );
        });

        model.update(&mut app, |model, ctx| model.close_buffer(file_id, ctx));
        model.read(&app, |model, ctx| {
            assert_eq!(model.content_for_file(file_id, ctx), None);
        });

        drop((first, second));
    });
}

#[test]
fn environment_buffer_operations_keep_opening_session_binding() {
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| DetectedRepositories::default());
        app.add_singleton_model(FileModel::new);
        app.add_singleton_model(crate::workspace::environment_runtime::new_transport_manager);
        let model = app.add_singleton_model(GlobalBufferModel::new);
        let host_id = warp_core::HostId::new("stable-binding-host".to_owned());
        let path =
            warp_util::standardized_path::StandardizedPath::try_new("/tmp/stable-binding.txt")
                .unwrap();
        let environment_file_path = EnvironmentFilePath::new(host_id, path);
        let opening_session_id = warp_core::SessionId::from(61);
        let later_session_id = warp_core::SessionId::from(62);
        let file_id = FileId::new();
        let buffer = app.update(|ctx| ctx.add_model(|_| Buffer::default()));

        model.update(&mut app, |model, _ctx| {
            model.location_to_id.insert(
                BufferLocation::EnvironmentRuntime(environment_file_path.clone()),
                file_id,
            );
            model.buffers.insert(
                file_id,
                InternalBufferState {
                    buffer: buffer.downgrade(),
                    consumer_count: 1,
                    pending_diff_parse: None,
                    source: BufferSource::EnvironmentRuntime {
                        environment_file_path: environment_file_path.clone(),
                        binding_session_id: opening_session_id,
                        sync_clock: Some(SyncClock::from_wire(7, 0)),
                    },
                },
            );
        });

        let reopened = model.update(&mut app, |model, ctx| {
            model.open_environment_buffer(environment_file_path.clone(), later_session_id, ctx)
        });
        assert_eq!(reopened.file_id, file_id);
        model.read(&app, |model, _ctx| {
            let state = model.buffers.get(&file_id).unwrap();
            assert_eq!(state.consumer_count, 2);
            let BufferSource::EnvironmentRuntime {
                binding_session_id, ..
            } = &state.source
            else {
                panic!("expected environment buffer");
            };
            assert_eq!(*binding_session_id, opening_session_id);
        });

        let (sender, mut receiver) = mpsc::unbounded();
        app.update(|ctx| {
            ctx.subscribe_to_model(&model, move |_model, event, _ctx| {
                if let GlobalBufferModelEvent::FailedToSave { file_id, error } = event {
                    sender
                        .unbounded_send((*file_id, error.to_string()))
                        .unwrap();
                }
            });
        });
        model.update(&mut app, |model, ctx| {
            model.save_environment_buffer(file_id, ctx)
        });
        let (failed_file_id, error) = receiver.next().await.unwrap();
        assert_eq!(failed_file_id, file_id);
        assert!(
            error.contains("61"),
            "save must resolve the opening session binding, got: {error}"
        );
        assert!(
            !error.contains("62"),
            "later same-host sessions must not replace the binding: {error}"
        );

        model.update(&mut app, |model, ctx| model.close_buffer(file_id, ctx));
        model.read(&app, |model, _ctx| {
            let state = model.buffers.get(&file_id).unwrap();
            assert_eq!(state.consumer_count, 1);
            let BufferSource::EnvironmentRuntime {
                binding_session_id, ..
            } = &state.source
            else {
                panic!("expected environment buffer");
            };
            assert_eq!(*binding_session_id, opening_session_id);
        });

        model.update(&mut app, |model, ctx| model.close_buffer(file_id, ctx));
        model.read(&app, |model, _ctx| {
            assert!(!model.buffers.contains_key(&file_id));
            assert!(model.location_to_id.get_by_right(&file_id).is_none());
        });

        drop(reopened);
    });
}

#[test]
fn edit_and_save_are_rejected_while_conflict_resolution_is_staged() {
    App::test((), |mut app| async move {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("note.txt");
        fs::write(&path, "committed").unwrap();

        app.add_singleton_model(|_| DetectedRepositories::default());
        app.add_singleton_model(FileModel::new);
        let model = app.add_singleton_model(GlobalBufferModel::new);
        let mut events = setup_event_channel(&mut app, &model);
        let _buffer_state = model.update(&mut app, |model, ctx| {
            model.open_server_current_app(path, ctx)
        });
        let file_id = wait_until_loaded(&mut events).await;
        let (server_version, client_version) = model.read(&app, |model, _ctx| {
            let clock = model
                .sync_clock_for_server_current_app(file_id)
                .expect("server buffer must be loaded");
            (clock.server_version, clock.client_version)
        });

        let (edit_accepted, save_result) = model.update(&mut app, |model, ctx| {
            model
                .resolve_conflict(
                    file_id,
                    server_version,
                    ContentVersion::new(),
                    "client-wins",
                    ctx,
                )
                .unwrap();
            let edit_accepted = model.apply_client_edit(
                file_id,
                &[CharOffsetEdit {
                    start: CharOffset::from(1),
                    end: CharOffset::from(1),
                    text: "must-not-apply".to_string(),
                }],
                server_version,
                ContentVersion::new(),
                ctx,
            );
            let save_result = model.save_server_current_app(file_id, ctx);
            (edit_accepted, save_result)
        });

        assert!(!edit_accepted);
        assert!(save_result.is_err());
        assert_committed_state_unchanged(
            &app,
            &model,
            file_id,
            "committed",
            server_version,
            client_version,
        );

        loop {
            if matches!(
                events.next().await.unwrap(),
                TestEvent::Saved(saved_id) if saved_id == file_id
            ) {
                break;
            }
        }
    });
}

#[test]
fn mismatched_file_saved_does_not_complete_staged_conflict_resolution() {
    App::test((), |mut app| async move {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("note.txt");
        fs::write(&path, "committed").unwrap();

        app.add_singleton_model(|_| DetectedRepositories::default());
        app.add_singleton_model(FileModel::new);
        let model = app.add_singleton_model(GlobalBufferModel::new);
        let mut events = setup_event_channel(&mut app, &model);
        let _buffer_state = model.update(&mut app, |model, ctx| {
            model.open_server_current_app(path, ctx)
        });
        let file_id = wait_until_loaded(&mut events).await;
        let (server_version, client_version) = model.read(&app, |model, _ctx| {
            let clock = model
                .sync_clock_for_server_current_app(file_id)
                .expect("server buffer must be loaded");
            (clock.server_version, clock.client_version)
        });

        model.update(&mut app, |model, ctx| {
            model
                .resolve_conflict(
                    file_id,
                    server_version,
                    ContentVersion::new(),
                    "client-wins",
                    ctx,
                )
                .unwrap();
            model.handle_file_model_events(
                &FileModelEvent::FileSaved {
                    id: file_id,
                    version: ContentVersion::new(),
                },
                ctx,
            );
            assert!(model.save_server_current_app(file_id, ctx).is_err());
        });

        assert_committed_state_unchanged(
            &app,
            &model,
            file_id,
            "committed",
            server_version,
            client_version,
        );

        loop {
            if matches!(
                events.next().await.unwrap(),
                TestEvent::Saved(saved_id) if saved_id == file_id
            ) {
                break;
            }
        }
    });
}

#[test]
fn same_host_session_churn_does_not_switch_buffer_connection() {
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| DetectedRepositories::default());
        app.add_singleton_model(FileModel::new);
        let model = app.add_singleton_model(GlobalBufferModel::new);
        let host_id = warp_core::HostId::new("shared-host".to_string());
        let path =
            warp_util::standardized_path::StandardizedPath::try_new("/tmp/note.txt").unwrap();
        let environment_file_path = EnvironmentFilePath::new(host_id.clone(), path);
        let bound_session_id = warp_core::SessionId::from(41);
        let foreign_session_id = warp_core::SessionId::from(42);
        let file_id = FileId::new();
        let initial_server_version = ContentVersion::from_raw(10);
        let buffer = app.update(|ctx| {
            let buffer = ctx.add_model(|_| Buffer::default());
            buffer.update(ctx, |buffer, ctx| {
                buffer.replace_all("committed", ctx);
                buffer.set_version(ContentVersion::new());
            });
            buffer
        });

        model.update(&mut app, |model, _ctx| {
            model.location_to_id.insert(
                BufferLocation::EnvironmentRuntime(environment_file_path.clone()),
                file_id,
            );
            model.buffers.insert(
                file_id,
                InternalBufferState {
                    buffer: buffer.downgrade(),
                    consumer_count: 1,
                    pending_diff_parse: None,
                    source: BufferSource::EnvironmentRuntime {
                        environment_file_path: environment_file_path.clone(),
                        binding_session_id: bound_session_id,
                        sync_clock: Some(SyncClock {
                            server_version: initial_server_version,
                            client_version: ContentVersion::from_raw(0),
                        }),
                    },
                },
            );
        });

        let reopened = model.update(&mut app, |model, ctx| {
            model.open_environment_buffer(environment_file_path.clone(), foreign_session_id, ctx)
        });
        assert_eq!(reopened.file_id, file_id);
        model.read(&app, |model, _ctx| {
            let state = model.buffers.get(&file_id).unwrap();
            let BufferSource::EnvironmentRuntime {
                binding_session_id, ..
            } = &state.source
            else {
                panic!("expected environment buffer");
            };
            assert_eq!(*binding_session_id, bound_session_id);
        });

        let pushed_server_version = ContentVersion::from_raw(11);
        model.update(&mut app, |model, ctx| {
            model.handle_buffer_updated_push(
                foreign_session_id,
                &host_id,
                environment_file_path.path.as_str(),
                pushed_server_version.as_u64(),
                0,
                &[],
                ctx,
            );
        });
        model.read(&app, |model, _ctx| {
            let state = model.buffers.get(&file_id).unwrap();
            let BufferSource::EnvironmentRuntime { sync_clock, .. } = &state.source else {
                panic!("expected environment buffer");
            };
            assert_eq!(
                sync_clock.as_ref().unwrap().server_version,
                initial_server_version
            );
        });

        model.update(&mut app, |model, ctx| {
            model.handle_buffer_updated_push(
                bound_session_id,
                &host_id,
                environment_file_path.path.as_str(),
                pushed_server_version.as_u64(),
                0,
                &[],
                ctx,
            );
        });
        model.read(&app, |model, _ctx| {
            let state = model.buffers.get(&file_id).unwrap();
            let BufferSource::EnvironmentRuntime { sync_clock, .. } = &state.source else {
                panic!("expected environment buffer");
            };
            assert_eq!(
                sync_clock.as_ref().unwrap().server_version,
                pushed_server_version
            );
        });

        drop(reopened);
    });
}

#[test]
fn environment_buffer_is_not_editable_until_loaded() {
    App::test((), |mut app| async move {
        let model = initialize_environment_editor_app(&mut app);
        let host_id = warp_core::HostId::new("loading-host".to_owned());
        let path =
            warp_util::standardized_path::StandardizedPath::try_new("/tmp/loading.txt").unwrap();
        let environment_file_path = EnvironmentFilePath::new(host_id, path);
        let binding_session_id = warp_core::SessionId::from(51);
        let file_id = FileId::new();
        let buffer = app.update(|ctx| ctx.add_model(|_| Buffer::default()));

        model.update(&mut app, |model, _ctx| {
            model.location_to_id.insert(
                BufferLocation::EnvironmentRuntime(environment_file_path.clone()),
                file_id,
            );
            model.buffers.insert(
                file_id,
                InternalBufferState {
                    buffer: buffer.downgrade(),
                    consumer_count: 1,
                    pending_diff_parse: None,
                    source: BufferSource::EnvironmentRuntime {
                        environment_file_path: environment_file_path.clone(),
                        binding_session_id,
                        sync_clock: None,
                    },
                },
            );
        });

        let editor = add_environment_editor(&mut app, environment_file_path, binding_session_id);
        let interaction_state = editor.update(&mut app, |view, ctx| {
            view.editor().as_ref(ctx).interaction_state(ctx)
        });
        assert_eq!(interaction_state, InteractionState::Selectable);
    });
}

#[test]
fn shared_loading_consumers_unlock_together() {
    App::test((), |mut app| async move {
        let model = initialize_environment_editor_app(&mut app);
        let host_id = warp_core::HostId::new("shared-loading-host".to_owned());
        let path =
            warp_util::standardized_path::StandardizedPath::try_new("/tmp/shared-loading.txt")
                .unwrap();
        let environment_file_path = EnvironmentFilePath::new(host_id, path);
        let binding_session_id = warp_core::SessionId::from(55);
        let file_id = FileId::new();
        let buffer = app.update(|ctx| ctx.add_model(|_| Buffer::default()));

        model.update(&mut app, |model, _ctx| {
            model.location_to_id.insert(
                BufferLocation::EnvironmentRuntime(environment_file_path.clone()),
                file_id,
            );
            model.buffers.insert(
                file_id,
                InternalBufferState {
                    buffer: buffer.downgrade(),
                    consumer_count: 0,
                    pending_diff_parse: None,
                    source: BufferSource::EnvironmentRuntime {
                        environment_file_path: environment_file_path.clone(),
                        binding_session_id,
                        sync_clock: None,
                    },
                },
            );
        });

        let first =
            add_environment_editor(&mut app, environment_file_path.clone(), binding_session_id);
        let second = add_environment_editor(&mut app, environment_file_path, binding_session_id);
        for editor in [&first, &second] {
            assert_eq!(
                editor.update(&mut app, |view, ctx| {
                    view.editor().as_ref(ctx).interaction_state(ctx)
                }),
                InteractionState::Selectable
            );
        }

        model.update(&mut app, |model, ctx| {
            model.finish_environment_buffer_materialization(
                file_id,
                Ok(("materialized together".to_owned(), 8)),
                ctx,
            );
        });

        for editor in [&first, &second] {
            let (interaction_state, text) = editor.update(&mut app, |view, ctx| {
                let editor = view.editor().as_ref(ctx);
                (
                    editor.interaction_state(ctx),
                    editor.text(ctx).into_string(),
                )
            });
            assert_eq!(interaction_state, InteractionState::Editable);
            assert_eq!(text, "materialized together");
        }
    });
}

#[test]
fn already_loaded_environment_buffer_second_consumer_is_editable() {
    App::test((), |mut app| async move {
        let model = initialize_environment_editor_app(&mut app);
        let host_id = warp_core::HostId::new("loaded-host".to_owned());
        let path =
            warp_util::standardized_path::StandardizedPath::try_new("/tmp/loaded.txt").unwrap();
        let environment_file_path = EnvironmentFilePath::new(host_id, path);
        let binding_session_id = warp_core::SessionId::from(52);
        let file_id = FileId::new();
        let buffer = app.update(|ctx| {
            let buffer = ctx.add_model(|_| Buffer::default());
            buffer.update(ctx, |buffer, ctx| {
                buffer.replace_all("materialized", ctx);
                buffer.set_version(ContentVersion::new());
            });
            buffer
        });

        model.update(&mut app, |model, _ctx| {
            model.location_to_id.insert(
                BufferLocation::EnvironmentRuntime(environment_file_path.clone()),
                file_id,
            );
            model.buffers.insert(
                file_id,
                InternalBufferState {
                    buffer: buffer.downgrade(),
                    consumer_count: 1,
                    pending_diff_parse: None,
                    source: BufferSource::EnvironmentRuntime {
                        environment_file_path: environment_file_path.clone(),
                        binding_session_id,
                        sync_clock: Some(SyncClock::from_wire(7, 0)),
                    },
                },
            );
        });

        let editor = add_environment_editor(&mut app, environment_file_path, binding_session_id);
        let (interaction_state, text) = editor.update(&mut app, |view, ctx| {
            let editor = view.editor().as_ref(ctx);
            (
                editor.interaction_state(ctx),
                editor.text(ctx).into_string(),
            )
        });
        assert_eq!(interaction_state, InteractionState::Editable);
        assert_eq!(text, "materialized");
    });
}

#[test]
fn failed_environment_buffer_never_becomes_editable() {
    App::test((), |mut app| async move {
        initialize_environment_editor_app(&mut app);
        let host_id = warp_core::HostId::new("disconnected-host".to_owned());
        let path =
            warp_util::standardized_path::StandardizedPath::try_new("/tmp/missing.txt").unwrap();
        let editor = add_environment_editor(
            &mut app,
            EnvironmentFilePath::new(host_id, path),
            warp_core::SessionId::from(53),
        );
        let (sender, mut receiver) = mpsc::unbounded();
        app.update(|ctx| {
            ctx.subscribe_to_view(&editor, move |_view, event, _ctx| {
                if matches!(event, CurrentAppCodeEditorEvent::FailedToLoad { .. }) {
                    sender.unbounded_send(()).unwrap();
                }
            });
        });

        receiver.next().await.unwrap();
        let (interaction_state, load_completion_reached) = editor.update(&mut app, |view, ctx| {
            (
                view.editor().as_ref(ctx).interaction_state(ctx),
                view.load_completion_reached(),
            )
        });
        assert_eq!(interaction_state, InteractionState::Selectable);
        assert!(!load_completion_reached);
    });
}

#[test]
fn late_open_response_does_not_revive_closed_buffer() {
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| DetectedRepositories::default());
        app.add_singleton_model(FileModel::new);
        app.add_singleton_model(crate::workspace::environment_runtime::new_transport_manager);
        let model = app.add_singleton_model(GlobalBufferModel::new);
        let host_id = warp_core::HostId::new("late-response-host".to_owned());
        let path =
            warp_util::standardized_path::StandardizedPath::try_new("/tmp/late.txt").unwrap();
        let environment_file_path = EnvironmentFilePath::new(host_id, path);
        let binding_session_id = warp_core::SessionId::from(54);
        let file_id = FileId::new();
        let buffer = app.update(|ctx| ctx.add_model(|_| Buffer::default()));

        model.update(&mut app, |model, _ctx| {
            model.location_to_id.insert(
                BufferLocation::EnvironmentRuntime(environment_file_path.clone()),
                file_id,
            );
            model.buffers.insert(
                file_id,
                InternalBufferState {
                    buffer: buffer.downgrade(),
                    consumer_count: 1,
                    pending_diff_parse: None,
                    source: BufferSource::EnvironmentRuntime {
                        environment_file_path,
                        binding_session_id,
                        sync_clock: None,
                    },
                },
            );
        });

        model.update(&mut app, |model, ctx| model.close_buffer(file_id, ctx));
        model.update(&mut app, |model, ctx| {
            model.finish_environment_buffer_materialization(
                file_id,
                Ok(("late content".to_owned(), 9)),
                ctx,
            );
        });

        model.read(&app, |model, _ctx| {
            assert!(!model.buffers.contains_key(&file_id));
            assert!(model.location_to_id.get_by_right(&file_id).is_none());
        });
        assert_eq!(
            buffer.read(&app, |buffer, _ctx| buffer.text().into_string()),
            ""
        );
    });
}
