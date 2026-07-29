use std::collections::HashMap;

use futures_util::stream::AbortHandle;
use std::fs;
#[cfg(feature = "local_fs")]
use std::io;

use super::super::proto::{
    abort_file_transfer_response, append_file_response, begin_file_transfer_response,
    client_message, exact_rename_response, finish_file_transfer_response, list_directory_response,
    promote_files_response, read_file_chunk_response, resolve_conflict_response,
    resolve_path_response, save_buffer_response, scan_cli_agent_sessions_response, server_message,
    write_file_chunk_response, AbortFileTransfer, AppendFile, Authenticate, BeginFileTransfer,
    ClientMessage, CreateDirectory, DeleteDirectory, DeleteDirectoryIdentity, ErrorCode,
    ExactRename, FileTransferDirection, FileTransferHandle, FinishFileTransfer, Initialize,
    ListDirectory, OpenBuffer, PromoteFiles, PromotionStatus, PromotionTarget, ReadFileChunk,
    ResolveConflict, ResolvePath, SaveBuffer, WriteFileChunk,
};
use super::super::protocol::RequestId;
#[cfg(feature = "local_fs")]
use super::super::server_buffer_tracker::ServerBufferTracker;
use super::{
    build_scan_cli_agent_sessions_response, collect_complete_directory_listing,
    decode_scan_cli_agent_wire_agents, remote_pty_user_defaults, resolve_path_failure,
    system_time_to_epoch_millis, validate_repo_metadata_directory_load_paths, ConnectionId,
    ConnectionMessageGateError, ConnectionPhase, ConnectionState, HandlerOutcome, PendingFileOps,
    ServerModel, SessionId,
};

#[cfg(feature = "local_fs")]
use super::super::cli_agent_sessions::{ScannedSession, ScannedSessionDiscovery};

#[cfg(feature = "local_fs")]
use crate::code::global_buffer_model::{GlobalBufferModel, GlobalBufferModelEvent};
#[cfg(feature = "local_fs")]
use repo_metadata::{
    repositories::DetectedRepositories, watcher::DirectoryWatcher, RepoMetadataModel,
};
#[cfg(feature = "local_fs")]
use warp_files::FileModel;
#[cfg(feature = "local_fs")]
use warp_util::content_version::ContentVersion;
#[cfg(feature = "local_fs")]
use warpui::{App, ModelHandle};

fn test_model() -> ServerModel {
    ServerModel {
        connections: HashMap::new(),
        grace_timer_cancel: None,
        host_id: "test-host-id".to_string(),
        next_pty_id: 1,
        pending_file_ops: PendingFileOps::new(),
        #[cfg(feature = "local_fs")]
        buffers: ServerBufferTracker::new(),
        auth_token: None,
    }
}

fn request_id() -> RequestId {
    RequestId::from("test-request".to_string())
}

#[cfg(all(feature = "local_fs", unix))]
fn canonical_temp_root(dir: &tempfile::TempDir) -> std::path::PathBuf {
    fs::canonicalize(dir.path()).unwrap()
}

#[cfg(feature = "local_fs")]
#[test]
fn remote_cli_agent_scan_success_uses_serialized_agent_names() {
    let response = build_scan_cli_agent_sessions_response(Ok(ScannedSessionDiscovery::Complete {
        observed_agents: vec![crate::terminal::CLIAgent::Omp],
        sessions: vec![ScannedSession {
            agent: crate::terminal::CLIAgent::Omp,
            id: "omp-session".to_owned(),
            source: "/root/.omp/agent/sessions/session.jsonl".to_owned(),
            label: Some("Remote Omp".to_owned()),
            cwd: Some("/root/project".to_owned()),
            modified_epoch_millis: Some(1),
        }],
    }));

    let Some(scan_cli_agent_sessions_response::Result::Success(success)) = response.result else {
        panic!("successful remote scan must map to an RPC Success result");
    };
    assert_eq!(
        success.observed_agents,
        vec![crate::terminal::CLIAgent::Omp.to_serialized_name()]
    );
    assert_eq!(success.records.len(), 1);
    assert_eq!(
        success.records[0].agent,
        crate::terminal::CLIAgent::Omp.to_serialized_name()
    );
    assert!(success.source_missing_agent.is_none());
}

#[cfg(feature = "local_fs")]
#[test]
fn remote_cli_agent_scan_wire_fields_round_trip_every_known_agent() {
    for agent in enum_iterator::all::<crate::terminal::CLIAgent>() {
        if matches!(agent, crate::terminal::CLIAgent::Unknown) {
            continue;
        }

        let complete =
            build_scan_cli_agent_sessions_response(Ok(ScannedSessionDiscovery::Complete {
                observed_agents: vec![agent],
                sessions: vec![ScannedSession {
                    agent,
                    id: format!("{agent:?}-session"),
                    source: format!("/tmp/{agent:?}.jsonl"),
                    label: None,
                    cwd: None,
                    modified_epoch_millis: None,
                }],
            }));
        let Some(scan_cli_agent_sessions_response::Result::Success(complete)) = complete.result
        else {
            panic!("complete remote scan must map to an RPC Success result");
        };
        assert_eq!(
            complete
                .observed_agents
                .iter()
                .map(|name| crate::terminal::CLIAgent::from_serialized_name(name))
                .collect::<Vec<_>>(),
            vec![agent],
            "observed_agents must preserve {agent:?} through the wire format"
        );
        assert_eq!(
            complete
                .records
                .iter()
                .map(|record| crate::terminal::CLIAgent::from_serialized_name(&record.agent))
                .collect::<Vec<_>>(),
            vec![agent],
            "records.agent must preserve {agent:?} through the wire format"
        );

        let missing =
            build_scan_cli_agent_sessions_response(Ok(ScannedSessionDiscovery::SourceMissing {
                agent,
            }));
        let Some(scan_cli_agent_sessions_response::Result::Success(missing)) = missing.result
        else {
            panic!("source-missing remote scan must map to an RPC Success result");
        };
        assert_eq!(
            missing
                .source_missing_agent
                .as_deref()
                .map(crate::terminal::CLIAgent::from_serialized_name),
            Some(agent),
            "source_missing_agent must preserve {agent:?} through the wire format"
        );
    }
}

#[cfg(feature = "local_fs")]
#[test]
fn remote_cli_agent_scan_source_missing_uses_serialized_agent_name() {
    let response =
        build_scan_cli_agent_sessions_response(Ok(ScannedSessionDiscovery::SourceMissing {
            agent: crate::terminal::CLIAgent::Omp,
        }));

    let Some(scan_cli_agent_sessions_response::Result::Success(success)) = response.result else {
        panic!("source-missing remote scan must map to an RPC Success result");
    };
    assert!(success.records.is_empty());
    assert!(success.observed_agents.is_empty());
    assert_eq!(
        success.source_missing_agent,
        Some(crate::terminal::CLIAgent::Omp.to_serialized_name())
    );
}

#[cfg(feature = "local_fs")]
#[test]
fn remote_cli_agent_scan_rejects_command_prefix_wire_identities() {
    let canonical = decode_scan_cli_agent_wire_agents(
        "enabled_agents",
        vec![crate::terminal::CLIAgent::Omp.to_serialized_name()],
    )
    .expect("serialized CLI agent identity must decode");
    assert_eq!(canonical, vec![crate::terminal::CLIAgent::Omp]);

    let error = decode_scan_cli_agent_wire_agents("enabled_agents", vec!["omp".to_owned()])
        .expect_err("command prefixes must never be accepted as RPC identities");
    assert!(error.contains("not a serialized CLI agent identity"));
}

#[cfg(feature = "local_fs")]
#[test]
fn remote_cli_agent_scan_failure_maps_to_rpc_error() {
    let response = build_scan_cli_agent_sessions_response(Err("traversal failed".to_owned()));

    let Some(scan_cli_agent_sessions_response::Result::Error(error)) = response.result else {
        panic!("scan failure must be an RPC Error result");
    };
    assert_eq!(error.message, "traversal failed");
}

fn test_connection_id() -> ConnectionId {
    ConnectionId::from(uuid::Uuid::new_v4())
}

fn insert_test_connection(model: &mut ServerModel) -> ConnectionId {
    let conn_id = test_connection_id();
    let (outbound_tx, _outbound_rx) = async_channel::unbounded();
    model
        .connections
        .insert(conn_id, ConnectionState::new(outbound_tx));
    conn_id
}

#[cfg(feature = "local_fs")]
fn insert_test_connection_with_receiver(
    model: &mut ServerModel,
) -> (
    ConnectionId,
    async_channel::Receiver<super::super::proto::ServerMessage>,
) {
    let conn_id = test_connection_id();
    let (outbound_tx, outbound_rx) = async_channel::unbounded();
    model
        .connections
        .insert(conn_id, ConnectionState::new(outbound_tx));
    (conn_id, outbound_rx)
}

#[cfg(feature = "local_fs")]
fn initialize_buffer_runtime_app(app: &mut App) -> ModelHandle<ServerModel> {
    app.add_singleton_model(DirectoryWatcher::new);
    app.add_singleton_model(|_| DetectedRepositories::default());
    app.add_singleton_model(RepoMetadataModel::new_with_incremental_updates);
    app.add_singleton_model(FileModel::new);
    app.add_singleton_model(GlobalBufferModel::new);
    app.add_singleton_model(ServerModel::new)
}

#[cfg(feature = "local_fs")]
async fn recv_correlated_response(
    receiver: &async_channel::Receiver<super::super::proto::ServerMessage>,
) -> super::super::proto::ServerMessage {
    loop {
        let message = receiver.recv().await.unwrap();
        if !message.request_id.is_empty() {
            return message;
        }
    }
}

#[cfg(feature = "local_fs")]
async fn register_initialized_test_connection(
    app: &mut App,
    server: &ModelHandle<ServerModel>,
    label: &str,
) -> (
    ConnectionId,
    async_channel::Receiver<super::super::proto::ServerMessage>,
) {
    let (conn_id, receiver) = server.update(app, |server, _ctx| {
        insert_test_connection_with_receiver(server)
    });
    let request_id = format!("{label}-initialize");
    server.update(app, |server, ctx| {
        server.handle_message(
            conn_id,
            ClientMessage {
                request_id: request_id.clone(),
                message: Some(client_message::Message::Initialize(Initialize {
                    auth_token: String::new(),
                    protocol_revision: remote_server::REMOTE_SERVER_PROTOCOL_REVISION,
                })),
            },
            ctx,
        );
    });
    let response = recv_correlated_response(&receiver).await;
    assert_eq!(response.request_id, request_id);
    assert!(matches!(
        response.message,
        Some(server_message::Message::InitializeResponse(_))
    ));
    (conn_id, receiver)
}

#[cfg(feature = "local_fs")]
async fn dispatch_test_request(
    app: &mut App,
    server: &ModelHandle<ServerModel>,
    conn_id: ConnectionId,
    receiver: &async_channel::Receiver<super::super::proto::ServerMessage>,
    request_id: &str,
    message: client_message::Message,
) -> super::super::proto::ServerMessage {
    server.update(app, |server, ctx| {
        server.handle_message(
            conn_id,
            ClientMessage {
                request_id: request_id.to_owned(),
                message: Some(message),
            },
            ctx,
        );
    });
    let response = recv_correlated_response(receiver).await;
    assert_eq!(response.request_id, request_id);
    response
}

#[cfg(all(feature = "local_fs", unix))]
#[test]
fn production_dispatch_keeps_multi_connection_transfers_independent() {
    App::test((), |mut app| async move {
        let fixture = tempfile::tempdir().unwrap();
        let root = canonical_temp_root(&fixture);
        let first_path = root.join("first.txt");
        let second_path = root.join("second.txt");
        fs::write(&first_path, b"first-connection").unwrap();
        fs::write(&second_path, b"second-connection").unwrap();

        let server = initialize_buffer_runtime_app(&mut app);
        let (first_conn, first_receiver) =
            register_initialized_test_connection(&mut app, &server, "lr128-first").await;
        let (second_conn, second_receiver) =
            register_initialized_test_connection(&mut app, &server, "lr128-second").await;

        let first_begin = dispatch_test_request(
            &mut app,
            &server,
            first_conn,
            &first_receiver,
            "lr128-first-begin",
            client_message::Message::BeginFileTransfer(BeginFileTransfer {
                path: first_path.to_string_lossy().into_owned(),
                direction: FileTransferDirection::Read as i32,
                executable: None,
            }),
        )
        .await;
        let Some(server_message::Message::BeginFileTransferResponse(first_begin)) =
            first_begin.message
        else {
            panic!("expected first BeginFileTransferResponse");
        };
        let Some(begin_file_transfer_response::Result::Success(first_begin)) = first_begin.result
        else {
            panic!("expected first begin success");
        };
        let first_handle = first_begin.handle.expect("first transfer handle");

        let second_begin = dispatch_test_request(
            &mut app,
            &server,
            second_conn,
            &second_receiver,
            "lr128-second-begin",
            client_message::Message::BeginFileTransfer(BeginFileTransfer {
                path: second_path.to_string_lossy().into_owned(),
                direction: FileTransferDirection::Read as i32,
                executable: None,
            }),
        )
        .await;
        let Some(server_message::Message::BeginFileTransferResponse(second_begin)) =
            second_begin.message
        else {
            panic!("expected second BeginFileTransferResponse");
        };
        let Some(begin_file_transfer_response::Result::Success(second_begin)) = second_begin.result
        else {
            panic!("expected second begin success");
        };
        let second_handle = second_begin.handle.expect("second transfer handle");

        let first_read = dispatch_test_request(
            &mut app,
            &server,
            first_conn,
            &first_receiver,
            "lr128-first-read",
            client_message::Message::ReadFileChunk(ReadFileChunk {
                handle: Some(first_handle),
                max_bytes: 5,
            }),
        )
        .await;
        let Some(server_message::Message::ReadFileChunkResponse(first_read)) = first_read.message
        else {
            panic!("expected first ReadFileChunkResponse");
        };
        let Some(read_file_chunk_response::Result::Success(first_read)) = first_read.result else {
            panic!("expected first read success");
        };
        assert_eq!(first_read.bytes, b"first");
        assert_eq!(first_read.next_offset, 5);
        assert!(!first_read.eof);

        let second_read = dispatch_test_request(
            &mut app,
            &server,
            second_conn,
            &second_receiver,
            "lr128-second-read",
            client_message::Message::ReadFileChunk(ReadFileChunk {
                handle: Some(second_handle.clone()),
                max_bytes: 7,
            }),
        )
        .await;
        let Some(server_message::Message::ReadFileChunkResponse(second_read)) = second_read.message
        else {
            panic!("expected second ReadFileChunkResponse");
        };
        let Some(read_file_chunk_response::Result::Success(second_read)) = second_read.result
        else {
            panic!("expected second read success");
        };
        assert_eq!(second_read.bytes, b"second-");
        assert_eq!(second_read.next_offset, 7);
        assert!(!second_read.eof);

        server.update(&mut app, |server, ctx| {
            server.deregister_connection(first_conn, ctx);
            assert!(!server.connections.contains_key(&first_conn));
            assert!(server.connections[&second_conn]
                .file_transfers
                .contains_key(&second_handle.id));
        });

        let second_remaining = dispatch_test_request(
            &mut app,
            &server,
            second_conn,
            &second_receiver,
            "lr128-second-remaining",
            client_message::Message::ReadFileChunk(ReadFileChunk {
                handle: Some(second_handle.clone()),
                max_bytes: 1024,
            }),
        )
        .await;
        let Some(server_message::Message::ReadFileChunkResponse(second_remaining)) =
            second_remaining.message
        else {
            panic!("expected remaining ReadFileChunkResponse");
        };
        let Some(read_file_chunk_response::Result::Success(second_remaining)) =
            second_remaining.result
        else {
            panic!("expected remaining read success");
        };
        assert_eq!(second_remaining.bytes, b"connection");
        assert_eq!(second_remaining.next_offset, 17);
        assert_eq!(second_remaining.total_size, Some(17));
        assert!(second_remaining.eof);

        let second_finish = dispatch_test_request(
            &mut app,
            &server,
            second_conn,
            &second_receiver,
            "lr128-second-finish",
            client_message::Message::FinishFileTransfer(FinishFileTransfer {
                handle: Some(second_handle),
            }),
        )
        .await;
        let Some(server_message::Message::FinishFileTransferResponse(second_finish)) =
            second_finish.message
        else {
            panic!("expected FinishFileTransferResponse");
        };
        assert!(matches!(
            second_finish.result,
            Some(finish_file_transfer_response::Result::Success(_))
        ));
    });
}

#[cfg(all(feature = "local_fs", unix))]
#[test]
fn production_dispatch_enforces_pinned_growth_and_truncation_snapshots() {
    App::test((), |mut app| async move {
        let fixture = tempfile::tempdir().unwrap();
        let root = canonical_temp_root(&fixture);
        let growth_path = root.join("growth.txt");
        let truncation_path = root.join("truncation.txt");
        fs::write(&growth_path, b"base").unwrap();
        fs::write(&truncation_path, b"abcdef").unwrap();

        let server = initialize_buffer_runtime_app(&mut app);
        let (conn_id, receiver) =
            register_initialized_test_connection(&mut app, &server, "lr129").await;

        let growth_begin = dispatch_test_request(
            &mut app,
            &server,
            conn_id,
            &receiver,
            "lr129-growth-begin",
            client_message::Message::BeginFileTransfer(BeginFileTransfer {
                path: growth_path.to_string_lossy().into_owned(),
                direction: FileTransferDirection::Read as i32,
                executable: None,
            }),
        )
        .await;
        let Some(server_message::Message::BeginFileTransferResponse(growth_begin)) =
            growth_begin.message
        else {
            panic!("expected growth BeginFileTransferResponse");
        };
        let Some(begin_file_transfer_response::Result::Success(growth_begin)) = growth_begin.result
        else {
            panic!("expected growth begin success");
        };
        assert_eq!(growth_begin.total_size, Some(4));
        let growth_handle = growth_begin.handle.expect("growth transfer handle");
        fs::write(&growth_path, b"base-appended").unwrap();

        let mut growth_bytes = Vec::new();
        for (request_id, expected_offset, expected_eof) in [
            ("lr129-growth-read-1", 2, false),
            ("lr129-growth-read-2", 4, true),
        ] {
            let response = dispatch_test_request(
                &mut app,
                &server,
                conn_id,
                &receiver,
                request_id,
                client_message::Message::ReadFileChunk(ReadFileChunk {
                    handle: Some(growth_handle.clone()),
                    max_bytes: 2,
                }),
            )
            .await;
            let Some(server_message::Message::ReadFileChunkResponse(response)) = response.message
            else {
                panic!("expected growth ReadFileChunkResponse");
            };
            let Some(read_file_chunk_response::Result::Success(response)) = response.result else {
                panic!("expected growth read success");
            };
            growth_bytes.extend_from_slice(&response.bytes);
            assert_eq!(response.next_offset, expected_offset);
            assert_eq!(response.total_size, Some(4));
            assert_eq!(response.eof, expected_eof);
        }
        assert_eq!(growth_bytes, b"base");

        let growth_finish = dispatch_test_request(
            &mut app,
            &server,
            conn_id,
            &receiver,
            "lr129-growth-finish",
            client_message::Message::FinishFileTransfer(FinishFileTransfer {
                handle: Some(growth_handle),
            }),
        )
        .await;
        let Some(server_message::Message::FinishFileTransferResponse(growth_finish)) =
            growth_finish.message
        else {
            panic!("expected growth FinishFileTransferResponse");
        };
        assert!(matches!(
            growth_finish.result,
            Some(finish_file_transfer_response::Result::Success(_))
        ));

        let truncation_begin = dispatch_test_request(
            &mut app,
            &server,
            conn_id,
            &receiver,
            "lr129-truncation-begin",
            client_message::Message::BeginFileTransfer(BeginFileTransfer {
                path: truncation_path.to_string_lossy().into_owned(),
                direction: FileTransferDirection::Read as i32,
                executable: None,
            }),
        )
        .await;
        let Some(server_message::Message::BeginFileTransferResponse(truncation_begin)) =
            truncation_begin.message
        else {
            panic!("expected truncation BeginFileTransferResponse");
        };
        let Some(begin_file_transfer_response::Result::Success(truncation_begin)) =
            truncation_begin.result
        else {
            panic!("expected truncation begin success");
        };
        assert_eq!(truncation_begin.total_size, Some(6));
        let truncation_handle = truncation_begin.handle.expect("truncation transfer handle");
        fs::write(&truncation_path, b"ab").unwrap();

        let short_read = dispatch_test_request(
            &mut app,
            &server,
            conn_id,
            &receiver,
            "lr129-truncation-read-1",
            client_message::Message::ReadFileChunk(ReadFileChunk {
                handle: Some(truncation_handle.clone()),
                max_bytes: 1024,
            }),
        )
        .await;
        let Some(server_message::Message::ReadFileChunkResponse(short_read)) = short_read.message
        else {
            panic!("expected truncation ReadFileChunkResponse");
        };
        let Some(read_file_chunk_response::Result::Success(short_read)) = short_read.result else {
            panic!("first truncation read must preserve progress");
        };
        assert_eq!(short_read.bytes, b"ab");
        assert_eq!(short_read.next_offset, 2);
        assert_eq!(short_read.total_size, Some(6));
        assert!(!short_read.eof);

        let truncation_error = dispatch_test_request(
            &mut app,
            &server,
            conn_id,
            &receiver,
            "lr129-truncation-read-2",
            client_message::Message::ReadFileChunk(ReadFileChunk {
                handle: Some(truncation_handle.clone()),
                max_bytes: 1024,
            }),
        )
        .await;
        let Some(server_message::Message::ReadFileChunkResponse(truncation_error)) =
            truncation_error.message
        else {
            panic!("expected truncation error ReadFileChunkResponse");
        };
        assert!(matches!(
            truncation_error.result,
            Some(read_file_chunk_response::Result::Error(_))
        ));

        let finish_after_error = dispatch_test_request(
            &mut app,
            &server,
            conn_id,
            &receiver,
            "lr129-truncation-finish",
            client_message::Message::FinishFileTransfer(FinishFileTransfer {
                handle: Some(truncation_handle),
            }),
        )
        .await;
        let Some(server_message::Message::FinishFileTransferResponse(finish_after_error)) =
            finish_after_error.message
        else {
            panic!("expected finish-after-error response");
        };
        assert!(matches!(
            finish_after_error.result,
            Some(finish_file_transfer_response::Result::Error(_))
        ));
    });
}

#[cfg(feature = "local_fs")]
#[test]
fn production_buffer_mutation_pipeline_serializes_resolve_then_save() {
    App::test((), |mut app| async move {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("buffer.txt");
        fs::write(&path, "committed").unwrap();
        let wire_path = path.to_string_lossy().into_owned();

        let server = initialize_buffer_runtime_app(&mut app);
        let (conn_id, receiver) = server.update(&mut app, |server, _ctx| {
            insert_test_connection_with_receiver(server)
        });

        let open_request = RequestId::from("lr127-open".to_string());
        let open_outcome = server.update(&mut app, |server, ctx| {
            server.handle_open_buffer(
                OpenBuffer {
                    path: wire_path.clone(),
                },
                &open_request,
                conn_id,
                ctx,
            )
        });
        assert!(matches!(open_outcome, HandlerOutcome::Async(None)));
        let open_response = recv_correlated_response(&receiver).await;
        assert_eq!(open_response.request_id, "lr127-open");
        let Some(server_message::Message::OpenBufferResponse(open_response)) =
            open_response.message
        else {
            panic!("expected OpenBufferResponse");
        };

        let resolved_client_version = ContentVersion::new().as_u64();
        let resolve_request = RequestId::from("lr127-resolve".to_string());
        let save_request = RequestId::from("lr127-save".to_string());
        server.update(&mut app, |server, ctx| {
            let resolve_outcome = server.handle_resolve_conflict(
                ResolveConflict {
                    path: wire_path.clone(),
                    acknowledged_server_version: open_response.server_version,
                    current_client_version: resolved_client_version,
                    client_content: "resolved-content".to_string(),
                },
                &resolve_request,
                conn_id,
                ctx,
            );
            assert!(matches!(resolve_outcome, HandlerOutcome::Async(None)));

            let save_outcome = server.handle_save_buffer(
                SaveBuffer {
                    path: wire_path.clone(),
                },
                &save_request,
                conn_id,
                ctx,
            );
            assert!(matches!(save_outcome, HandlerOutcome::Async(None)));

            let file_id = server.buffers.file_id_for_path(&wire_path).unwrap();
            assert!(matches!(
                server.buffers.active_mutation(&file_id).map(|mutation| &mutation.kind),
                Some(super::super::server_buffer_tracker::PendingBufferMutationKind::ResolveConflict { .. })
            ));
        });

        let resolve_response = recv_correlated_response(&receiver).await;
        assert_eq!(resolve_response.request_id, "lr127-resolve");
        let Some(server_message::Message::ResolveConflictResponse(resolve_response)) =
            resolve_response.message
        else {
            panic!("expected ResolveConflictResponse first");
        };
        assert!(matches!(
            resolve_response.result,
            Some(resolve_conflict_response::Result::Success(_))
        ));

        let save_response = recv_correlated_response(&receiver).await;
        assert_eq!(save_response.request_id, "lr127-save");
        let Some(server_message::Message::SaveBufferResponse(save_response)) =
            save_response.message
        else {
            panic!("expected SaveBufferResponse second");
        };
        assert!(matches!(
            save_response.result,
            Some(save_buffer_response::Result::Success(_))
        ));
        assert_eq!(fs::read_to_string(path).unwrap(), "resolved-content");
    });
}

#[cfg(feature = "local_fs")]
#[test]
fn production_failed_resolve_rolls_back_before_queued_save() {
    use std::{cell::Cell, rc::Rc};

    App::test((), |mut app| async move {
        let fixture = tempfile::tempdir().unwrap();
        let parent = fixture.path().join("container");
        fs::create_dir(&parent).unwrap();
        let path = parent.join("buffer.txt");
        fs::write(&path, "committed").unwrap();
        let wire_path = path.to_string_lossy().into_owned();

        app.add_singleton_model(DirectoryWatcher::new);
        app.add_singleton_model(|_| DetectedRepositories::default());
        app.add_singleton_model(RepoMetadataModel::new_with_incremental_updates);
        app.add_singleton_model(FileModel::new);
        let global_buffers = app.add_singleton_model(GlobalBufferModel::new);

        let restored_parent = Rc::new(Cell::new(false));
        let restored_parent_for_event = restored_parent.clone();
        let parent_for_event = parent.clone();
        app.update(|ctx| {
            ctx.subscribe_to_model(&global_buffers, move |_model, event, _ctx| {
                if matches!(event, GlobalBufferModelEvent::FailedToSave { .. })
                    && !restored_parent_for_event.replace(true)
                {
                    fs::remove_file(&parent_for_event).unwrap();
                    fs::create_dir(&parent_for_event).unwrap();
                }
            });
        });
        let server = app.add_singleton_model(ServerModel::new);
        let (conn_id, receiver) = server.update(&mut app, |server, _ctx| {
            insert_test_connection_with_receiver(server)
        });

        let open_request = RequestId::from("lr130-open".to_string());
        let open_outcome = server.update(&mut app, |server, ctx| {
            server.handle_open_buffer(
                OpenBuffer {
                    path: wire_path.clone(),
                },
                &open_request,
                conn_id,
                ctx,
            )
        });
        assert!(matches!(open_outcome, HandlerOutcome::Async(None)));
        let open_response = recv_correlated_response(&receiver).await;
        let Some(server_message::Message::OpenBufferResponse(open_response)) =
            open_response.message
        else {
            panic!("expected OpenBufferResponse");
        };
        let file_id = server.read(&app, |server, _ctx| {
            server.buffers.file_id_for_path(&wire_path).unwrap()
        });
        let initial_clock = global_buffers.read(&app, |buffers, _ctx| {
            buffers
                .sync_clock_for_server_current_app(file_id)
                .cloned()
                .unwrap()
        });

        fs::remove_file(&path).unwrap();
        fs::remove_dir(&parent).unwrap();
        fs::write(&parent, "not-a-directory").unwrap();

        let resolve_request = RequestId::from("lr130-resolve".to_string());
        let save_request = RequestId::from("lr130-save".to_string());
        server.update(&mut app, |server, ctx| {
            let resolve_outcome = server.handle_resolve_conflict(
                ResolveConflict {
                    path: wire_path.clone(),
                    acknowledged_server_version: open_response.server_version,
                    current_client_version: ContentVersion::new().as_u64(),
                    client_content: "must-not-commit".to_string(),
                },
                &resolve_request,
                conn_id,
                ctx,
            );
            assert!(matches!(resolve_outcome, HandlerOutcome::Async(None)));
            let save_outcome = server.handle_save_buffer(
                SaveBuffer {
                    path: wire_path.clone(),
                },
                &save_request,
                conn_id,
                ctx,
            );
            assert!(matches!(save_outcome, HandlerOutcome::Async(None)));
        });

        let resolve_response = recv_correlated_response(&receiver).await;
        assert_eq!(resolve_response.request_id, "lr130-resolve");
        let Some(server_message::Message::ResolveConflictResponse(resolve_response)) =
            resolve_response.message
        else {
            panic!("expected failed ResolveConflictResponse first");
        };
        assert!(matches!(
            resolve_response.result,
            Some(resolve_conflict_response::Result::Error(_))
        ));

        let save_response = recv_correlated_response(&receiver).await;
        assert_eq!(save_response.request_id, "lr130-save");
        let Some(server_message::Message::SaveBufferResponse(save_response)) =
            save_response.message
        else {
            panic!("expected queued SaveBufferResponse second");
        };
        assert!(matches!(
            save_response.result,
            Some(save_buffer_response::Result::Success(_))
        ));
        assert!(restored_parent.get());
        assert_eq!(fs::read_to_string(&path).unwrap(), "committed");
        global_buffers.read(&app, |buffers, ctx| {
            assert_eq!(
                buffers.content_for_file(file_id, ctx).as_deref(),
                Some("committed")
            );
            let clock = buffers.sync_clock_for_server_current_app(file_id).unwrap();
            assert_eq!(clock.server_version, initial_clock.server_version);
            assert_eq!(clock.client_version, initial_clock.client_version);
        });
    });
}

fn valid_session_bootstrapped(
    session_id: u64,
    shell_type: &str,
    shell_path: &str,
) -> super::super::proto::SessionBootstrapped {
    super::super::proto::SessionBootstrapped {
        session_id,
        shell_type: shell_type.to_owned(),
        shell_path: Some(shell_path.to_owned()),
        working_directory: Some(std::env::temp_dir().to_string_lossy().into_owned()),
        environment_variables: HashMap::from([
            (
                "HOME".to_owned(),
                std::env::temp_dir().to_string_lossy().into_owned(),
            ),
            ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
            (
                "ASHIDE_SESSION_EXECUTION_CONTEXT".to_owned(),
                "1".to_owned(),
            ),
        ]),
    }
}

#[test]
fn connection_starts_awaiting_initialize() {
    let (outbound_tx, _outbound_rx) = async_channel::unbounded();
    let connection = ConnectionState::new(outbound_tx);
    assert_eq!(connection.phase, ConnectionPhase::AwaitingInitialize);
    assert!(connection.snapshot_sent_roots.is_empty());
}

#[test]
fn pre_initialize_business_message_is_rejected_without_side_effects() {
    let mut model = test_model();
    let conn_id = insert_test_connection(&mut model);
    let message = Some(client_message::Message::Authenticate(Authenticate {
        auth_token: "must-not-apply".to_string(),
    }));

    assert_eq!(
        model.validate_connection_message(conn_id, &message),
        Err(ConnectionMessageGateError::InitializeRequired)
    );
    assert_eq!(model.auth_token(), None);
    let connection = model.connections.get(&conn_id).unwrap();
    assert!(connection.executors.is_empty());
    assert!(connection.ptys.is_empty());
}

#[test]
fn failed_initialize_cannot_be_bypassed_by_later_business_message() {
    let mut model = test_model();
    let conn_id = insert_test_connection(&mut model);
    let outcome = model.handle_initialize(
        conn_id,
        Initialize {
            auth_token: "must-not-apply".to_string(),
            protocol_revision: remote_server::REMOTE_SERVER_PROTOCOL_REVISION + 1,
        },
        &request_id(),
    );
    assert!(matches!(
        outcome.into_message(),
        server_message::Message::Error(_)
    ));

    let business = Some(client_message::Message::Authenticate(Authenticate {
        auth_token: "still-must-not-apply".to_string(),
    }));
    assert_eq!(
        model.validate_connection_message(conn_id, &business),
        Err(ConnectionMessageGateError::InitializeRequired)
    );
    assert_eq!(model.auth_token(), None);
}

#[test]
fn duplicate_initialize_is_rejected() {
    let mut model = test_model();
    let conn_id = insert_test_connection(&mut model);
    let first = model.handle_initialize(
        conn_id,
        Initialize {
            auth_token: String::new(),
            protocol_revision: remote_server::REMOTE_SERVER_PROTOCOL_REVISION,
        },
        &request_id(),
    );
    assert!(matches!(
        first.into_message(),
        server_message::Message::InitializeResponse(_)
    ));

    let duplicate = Some(client_message::Message::Initialize(Initialize {
        auth_token: String::new(),
        protocol_revision: remote_server::REMOTE_SERVER_PROTOCOL_REVISION,
    }));
    assert_eq!(
        model.validate_connection_message(conn_id, &duplicate),
        Err(ConnectionMessageGateError::AlreadyInitialized)
    );
}

#[test]
fn broadcasts_only_reach_ready_connections() {
    let mut model = test_model();
    let awaiting_id = test_connection_id();
    let (awaiting_tx, awaiting_rx) = async_channel::unbounded();
    model
        .connections
        .insert(awaiting_id, ConnectionState::new(awaiting_tx));

    let ready_id = test_connection_id();
    let (ready_tx, ready_rx) = async_channel::unbounded();
    let mut ready = ConnectionState::new(ready_tx);
    ready.phase = ConnectionPhase::Ready;
    model.connections.insert(ready_id, ready);

    model.send_server_message(
        None,
        None,
        server_message::Message::Error(super::super::proto::ErrorResponse {
            code: ErrorCode::Internal.into(),
            message: "push".to_string(),
        }),
    );

    assert!(awaiting_rx.try_recv().is_err());
    assert!(ready_rx.try_recv().is_ok());
}

#[test]
fn deregister_removes_complete_connection_state() {
    let mut model = test_model();
    let conn_id = insert_test_connection(&mut model);
    assert!(model.remove_connection_state(conn_id).is_some());
    assert!(!model.connections.contains_key(&conn_id));
}

#[test]
fn session_bootstrapped_rejects_missing_required_execution_context() {
    let mut model = test_model();
    let conn_id = insert_test_connection(&mut model);
    let session_id = SessionId::from(41_u64);

    model.handle_session_bootstrapped(
        conn_id,
        super::super::proto::SessionBootstrapped {
            session_id: session_id.as_u64(),
            shell_type: "bash".to_owned(),
            shell_path: None,
            working_directory: None,
            environment_variables: HashMap::new(),
        },
    );

    assert!(
        model
            .session_executor_for_connection(session_id, conn_id)
            .is_none(),
        "malformed SessionBootstrapped must not create a helper executor"
    );
}

#[test]
fn alias_deregister_removes_current_helper_executor() {
    let mut model = test_model();
    let conn_id = insert_test_connection(&mut model);
    let session_id = SessionId::from(45_u64);
    let working_directory = tempfile::tempdir().unwrap();
    model.handle_session_bootstrapped(
        conn_id,
        super::super::proto::SessionBootstrapped {
            session_id: session_id.as_u64(),
            shell_type: "bash".to_owned(),
            shell_path: Some("/bin/bash".to_owned()),
            working_directory: Some(working_directory.path().to_string_lossy().into_owned()),
            environment_variables: HashMap::from([
                ("HOME".to_owned(), "/home/test".to_owned()),
                ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
                (
                    "ASHIDE_SESSION_EXECUTION_CONTEXT".to_owned(),
                    "1".to_owned(),
                ),
            ]),
        },
    );
    assert!(model
        .session_executor_for_connection(session_id, conn_id)
        .is_some());

    model.handle_session_execution_context_deregistered(
        conn_id,
        super::super::proto::SessionExecutionContextDeregistered {
            session_id: session_id.as_u64(),
        },
    );

    assert!(model
        .session_executor_for_connection(session_id, conn_id)
        .is_none());
}

#[test]
fn same_session_id_is_independent_across_connections() {
    let mut model = test_model();
    let owner = insert_test_connection(&mut model);
    let other = insert_test_connection(&mut model);
    let session_id = 42_u64;
    model.handle_session_bootstrapped(
        owner,
        valid_session_bootstrapped(session_id, "bash", "/bin/bash"),
    );
    model.handle_session_bootstrapped(
        other,
        valid_session_bootstrapped(session_id, "zsh", "/bin/zsh"),
    );

    let session_id = SessionId::from(session_id);
    let owner_executor = model
        .session_executor_for_connection(session_id, owner)
        .expect("owner executor must exist");
    let other_executor = model
        .session_executor_for_connection(session_id, other)
        .expect("same SessionId in another connection must be independent");
    assert!(!std::sync::Arc::ptr_eq(&owner_executor, &other_executor));
}

#[test]
fn disconnect_removes_only_owned_executors() {
    let mut model = test_model();
    let owner = insert_test_connection(&mut model);
    let other = insert_test_connection(&mut model);
    let session_id = 43_u64;
    for conn_id in [owner, other] {
        model.handle_session_bootstrapped(
            conn_id,
            valid_session_bootstrapped(session_id, "bash", "/bin/bash"),
        );
    }
    model.remove_connection_state(owner);

    assert!(model
        .session_executor_for_connection(SessionId::from(session_id), owner)
        .is_none());
    assert!(model
        .session_executor_for_connection(SessionId::from(session_id), other)
        .is_some());
}

fn insert_in_progress_request(
    model: &mut ServerModel,
    conn_id: ConnectionId,
    request_id: RequestId,
) -> AbortHandle {
    let (handle, _registration) = AbortHandle::new_pair();
    model
        .connections
        .get_mut(&conn_id)
        .unwrap()
        .in_progress
        .insert(request_id, handle.clone());
    handle
}

#[test]
fn same_request_id_is_independent_across_connections() {
    let mut model = test_model();
    let owner = insert_test_connection(&mut model);
    let other = insert_test_connection(&mut model);
    let request_id = request_id();
    let owner_handle = insert_in_progress_request(&mut model, owner, request_id.clone());
    let other_handle = insert_in_progress_request(&mut model, other, request_id.clone());

    model.handle_abort(
        owner,
        super::super::proto::Abort {
            request_id_to_abort: request_id.clone().into(),
        },
        &RequestId::from("abort-owner".to_string()),
    );

    assert!(owner_handle.is_aborted());
    assert!(!other_handle.is_aborted());
    assert!(!model.connections[&owner]
        .in_progress
        .contains_key(&request_id));
    assert!(model.connections[&other]
        .in_progress
        .contains_key(&request_id));
}

#[test]
fn cross_connection_abort_is_rejected() {
    let mut model = test_model();
    let owner = insert_test_connection(&mut model);
    let other = insert_test_connection(&mut model);
    let request_id = request_id();
    let owner_handle = insert_in_progress_request(&mut model, owner, request_id.clone());

    model.handle_abort(
        other,
        super::super::proto::Abort {
            request_id_to_abort: request_id.clone().into(),
        },
        &RequestId::from("foreign-abort".to_string()),
    );

    assert!(!owner_handle.is_aborted());
    assert!(model.connections[&owner]
        .in_progress
        .contains_key(&request_id));
}

#[test]
fn request_completion_removes_only_owner_entry() {
    let mut model = test_model();
    let owner = insert_test_connection(&mut model);
    let other = insert_test_connection(&mut model);
    let request_id = request_id();
    insert_in_progress_request(&mut model, owner, request_id.clone());
    insert_in_progress_request(&mut model, other, request_id.clone());

    assert!(model.remove_in_progress_request(owner, &request_id));
    assert!(!model.connections[&owner]
        .in_progress
        .contains_key(&request_id));
    assert!(model.connections[&other]
        .in_progress
        .contains_key(&request_id));
}

#[test]
fn disconnect_aborts_only_owned_in_progress_requests() {
    let mut model = test_model();
    let owner = insert_test_connection(&mut model);
    let other = insert_test_connection(&mut model);
    let shared_id = request_id();
    let owner_handle = insert_in_progress_request(&mut model, owner, shared_id.clone());
    let owner_unrelated = insert_in_progress_request(
        &mut model,
        owner,
        RequestId::from("owner-unrelated".to_string()),
    );
    let other_handle = insert_in_progress_request(&mut model, other, shared_id);

    model.remove_connection_state(owner);

    assert!(owner_handle.is_aborted());
    assert!(owner_unrelated.is_aborted());
    assert!(!other_handle.is_aborted());
    assert_eq!(model.connections[&other].in_progress.len(), 1);
}

#[test]
fn session_executor_rebootstrap_updates_only_owner_namespace() {
    let mut model = test_model();
    let owner = insert_test_connection(&mut model);
    let other = insert_test_connection(&mut model);
    let session_id = 44_u64;
    model.handle_session_bootstrapped(
        other,
        valid_session_bootstrapped(session_id, "bash", "/bin/bash"),
    );
    let other_before = model
        .session_executor_for_connection(SessionId::from(session_id), other)
        .unwrap();
    model.handle_session_bootstrapped(
        owner,
        valid_session_bootstrapped(session_id, "zsh", "/bin/zsh"),
    );

    let other_after = model
        .session_executor_for_connection(SessionId::from(session_id), other)
        .unwrap();
    assert!(std::sync::Arc::ptr_eq(&other_before, &other_after));
}

#[tokio::test]
async fn two_remote_sessions_keep_distinct_execution_contexts() {
    let mut model = test_model();
    let conn_id = insert_test_connection(&mut model);
    let first_cwd = tempfile::tempdir().unwrap();
    let second_cwd = tempfile::tempdir().unwrap();

    for (session_id, cwd, codex_home) in [
        (61_u64, first_cwd.path(), "/session/one/codex"),
        (62_u64, second_cwd.path(), "/session/two/codex"),
    ] {
        model.handle_session_bootstrapped(
            conn_id,
            super::super::proto::SessionBootstrapped {
                session_id,
                shell_type: "bash".to_owned(),
                shell_path: Some("/bin/bash".to_owned()),
                working_directory: Some(cwd.to_string_lossy().into_owned()),
                environment_variables: HashMap::from([
                    ("HOME".to_owned(), format!("/session/{session_id}/home")),
                    ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
                    ("CODEX_HOME".to_owned(), codex_home.to_owned()),
                    (
                        "ASHIDE_SESSION_EXECUTION_CONTEXT".to_owned(),
                        "1".to_owned(),
                    ),
                ]),
            },
        );
    }

    for (session_id, cwd, codex_home) in [
        (61_u64, first_cwd.path(), "/session/one/codex"),
        (62_u64, second_cwd.path(), "/session/two/codex"),
    ] {
        let executor = model
            .session_executor_for_connection(SessionId::from(session_id), conn_id)
            .unwrap();
        let output = executor
            .execute_local_command(
                "printf '%s\\n%s\\n' \"$PWD\" \"$CODEX_HOME\"",
                None,
                None,
                Default::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!(
                "{}\n{codex_home}\n",
                std::fs::canonicalize(cwd).unwrap().to_string_lossy()
            )
        );
    }
}

#[test]
fn fresh_model_starts_without_auth_token() {
    let model = test_model();

    assert_eq!(model.auth_token(), None);
}

#[test]
fn initialize_with_auth_token_stores_token() {
    let mut model = test_model();
    let conn_id = insert_test_connection(&mut model);

    model.handle_initialize(
        conn_id,
        Initialize {
            auth_token: "initial-token".to_string(),
            protocol_revision: remote_server::REMOTE_SERVER_PROTOCOL_REVISION,
        },
        &request_id(),
    );

    assert_eq!(model.auth_token(), Some("initial-token"));
}

#[test]
fn initialize_marks_only_matching_revision_ready() {
    let mut model = test_model();
    let conn_id = insert_test_connection(&mut model);
    model.handle_initialize(
        conn_id,
        Initialize {
            auth_token: String::new(),
            protocol_revision: remote_server::REMOTE_SERVER_PROTOCOL_REVISION,
        },
        &request_id(),
    );

    assert_eq!(model.connections[&conn_id].phase, ConnectionPhase::Ready);
    let business = Some(client_message::Message::Authenticate(Authenticate {
        auth_token: "rotated".to_string(),
    }));
    assert_eq!(
        model.validate_connection_message(conn_id, &business),
        Ok(())
    );
}

#[test]
fn rejected_duplicate_initialize_preserves_existing_auth_token() {
    let mut model = test_model();
    let conn_id = insert_test_connection(&mut model);
    model.handle_initialize(
        conn_id,
        Initialize {
            auth_token: "initial-token".to_string(),
            protocol_revision: remote_server::REMOTE_SERVER_PROTOCOL_REVISION,
        },
        &request_id(),
    );

    let outcome = model.handle_initialize(
        conn_id,
        Initialize {
            auth_token: "must-not-replace".to_string(),
            protocol_revision: remote_server::REMOTE_SERVER_PROTOCOL_REVISION,
        },
        &request_id(),
    );

    assert!(matches!(
        outcome.into_message(),
        server_message::Message::Error(_)
    ));
    assert_eq!(model.auth_token(), Some("initial-token"));
}

#[test]
fn initialize_rejects_wrong_client_protocol_revision_before_auth_mutation() {
    let mut model = test_model();
    let conn_id = insert_test_connection(&mut model);

    let outcome = model.handle_initialize(
        conn_id,
        Initialize {
            auth_token: "must-not-be-stored".to_string(),
            protocol_revision: remote_server::REMOTE_SERVER_PROTOCOL_REVISION + 1,
        },
        &request_id(),
    );

    let server_message::Message::Error(error) = outcome.into_message() else {
        panic!("expected protocol mismatch error");
    };
    assert_eq!(error.code(), ErrorCode::InvalidRequest);
    assert!(error.message.contains("protocol revision mismatch"));
    assert_eq!(model.auth_token(), None);
}

#[test]
fn authenticate_with_auth_token_replaces_auth_token() {
    let mut model = test_model();
    let conn_id = insert_test_connection(&mut model);
    model.handle_initialize(
        conn_id,
        Initialize {
            auth_token: "initial-token".to_string(),
            protocol_revision: remote_server::REMOTE_SERVER_PROTOCOL_REVISION,
        },
        &request_id(),
    );

    model.handle_authenticate(Authenticate {
        auth_token: "rotated-token".to_string(),
    });

    assert_eq!(model.auth_token(), Some("rotated-token"));
}

#[test]
fn empty_authenticate_preserves_existing_auth_token() {
    let mut model = test_model();
    let conn_id = insert_test_connection(&mut model);
    model.handle_initialize(
        conn_id,
        Initialize {
            auth_token: "initial-token".to_string(),
            protocol_revision: remote_server::REMOTE_SERVER_PROTOCOL_REVISION,
        },
        &request_id(),
    );

    model.handle_authenticate(Authenticate {
        auth_token: String::new(),
    });

    assert_eq!(model.auth_token(), Some("initial-token"));
}

#[cfg(feature = "local_fs")]
#[test]
fn resolve_path_reports_file_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("note.txt");
    fs::write(&file_path, "hello").unwrap();
    let model = test_model();

    let response = model.handle_resolve_path(ResolvePath {
        path: file_path.to_string_lossy().to_string(),
    });

    let server_message::Message::ResolvePathResponse(response) = response.into_message() else {
        panic!("expected ResolvePathResponse");
    };
    let Some(resolve_path_response::Result::Success(success)) = response.result else {
        panic!("expected resolve path success");
    };
    assert_eq!(success.path, file_path.to_string_lossy());
    assert_eq!(
        success.resolved_path.as_deref(),
        Some(
            fs::canonicalize(&file_path)
                .unwrap()
                .to_string_lossy()
                .as_ref()
        )
    );
    assert_eq!(
        success.kind,
        super::super::proto::FileSystemEntryKind::File as i32
    );
    assert_eq!(success.size_bytes, Some(5));
}

#[cfg(feature = "local_fs")]
#[test]
fn resolve_path_not_found_is_distinct_from_remote_error() {
    assert!(matches!(
        resolve_path_failure("/missing", io::Error::from(io::ErrorKind::NotFound),),
        resolve_path_response::Result::NotFound(_)
    ));
    let resolve_path_response::Result::Error(error) = resolve_path_failure(
        "/forbidden",
        io::Error::from(io::ErrorKind::PermissionDenied),
    ) else {
        panic!("permission failure must remain an explicit remote error");
    };
    assert!(error.message.contains("/forbidden"));
    assert!(!error.message.is_empty());
}

#[cfg(feature = "local_fs")]
#[test]
fn list_directory_returns_sorted_metadata() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("b.txt"), "b").unwrap();
    fs::create_dir(dir.path().join("a-dir")).unwrap();
    let model = test_model();

    let response = model.handle_list_directory(ListDirectory {
        path: dir.path().to_string_lossy().to_string(),
    });

    let server_message::Message::ListDirectoryResponse(response) = response.into_message() else {
        panic!("expected ListDirectoryResponse");
    };
    let Some(list_directory_response::Result::Success(success)) = response.result else {
        panic!("expected list directory success");
    };
    assert_eq!(success.path, dir.path().to_string_lossy());
    assert_eq!(success.entries.len(), 2);
    assert_eq!(success.entries[0].name, "a-dir");
    assert_eq!(
        success.entries[0].kind,
        super::super::proto::FileSystemEntryKind::Directory as i32
    );
    assert_eq!(success.entries[1].name, "b.txt");
    assert_eq!(
        success.entries[1].kind,
        super::super::proto::FileSystemEntryKind::File as i32
    );
    assert_eq!(success.entries[1].size_bytes, Some(1));
}

#[cfg(feature = "local_fs")]
#[test]
fn list_directory_entry_error_is_not_silently_dropped() {
    let entries = std::iter::once(Err::<fs::DirEntry, _>(io::Error::new(
        io::ErrorKind::PermissionDenied,
        "injected directory entry failure",
    )));

    let error = collect_complete_directory_listing(entries).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
}

#[cfg(feature = "local_fs")]
#[test]
fn list_directory_modified_time_error_is_not_silently_dropped() {
    let before_epoch = std::time::UNIX_EPOCH - std::time::Duration::from_secs(1);
    assert!(system_time_to_epoch_millis(before_epoch).is_err());
}

#[cfg(all(feature = "local_fs", unix))]
#[test]
fn list_directory_broken_symlink_preserves_lexical_row() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let link = dir.path().join("broken-link");
    symlink(dir.path().join("missing-target"), &link).unwrap();

    let entries = collect_complete_directory_listing(fs::read_dir(dir.path()).unwrap()).unwrap();
    let entry = entries
        .iter()
        .find(|entry| entry.name == "broken-link")
        .expect("broken symlink lexical row must remain visible");
    assert_eq!(
        entry.kind,
        super::super::proto::FileSystemEntryKind::Symlink as i32
    );
    assert_eq!(
        entry.target_kind,
        super::super::proto::FileSystemEntryKind::Missing as i32
    );
    assert_eq!(
        entry.size_bytes,
        Some(fs::symlink_metadata(&link).unwrap().len())
    );
}

#[cfg(all(feature = "local_fs", unix))]
fn begin_transfer(
    model: &mut ServerModel,
    path: &std::path::Path,
    direction: FileTransferDirection,
    conn_id: uuid::Uuid,
) -> FileTransferHandle {
    model.connections.entry(conn_id).or_insert_with(|| {
        let (outbound_tx, _outbound_rx) = async_channel::unbounded();
        ConnectionState::new(outbound_tx)
    });
    let response = model.handle_begin_file_transfer(
        BeginFileTransfer {
            path: path.to_string_lossy().into_owned(),
            direction: direction as i32,
            executable: None,
        },
        conn_id,
    );
    let server_message::Message::BeginFileTransferResponse(response) = response.into_message()
    else {
        panic!("expected BeginFileTransferResponse");
    };
    let Some(begin_file_transfer_response::Result::Success(success)) = response.result else {
        panic!("expected begin transfer success: {response:?}");
    };
    success.handle.expect("transfer handle")
}

#[cfg(all(feature = "local_fs", unix))]
fn assert_transfer_error(outcome: super::HandlerOutcome) {
    match outcome.into_message() {
        server_message::Message::ReadFileChunkResponse(response) => assert!(matches!(
            response.result,
            Some(read_file_chunk_response::Result::Error(_))
        )),
        server_message::Message::WriteFileChunkResponse(response) => assert!(matches!(
            response.result,
            Some(write_file_chunk_response::Result::Error(_))
        )),
        server_message::Message::FinishFileTransferResponse(response) => assert!(matches!(
            response.result,
            Some(finish_file_transfer_response::Result::Error(_))
        )),
        server_message::Message::AbortFileTransferResponse(response) => assert!(matches!(
            response.result,
            Some(abort_file_transfer_response::Result::Error(_))
        )),
        message => panic!("expected file transfer error response, got {message:?}"),
    }
}

#[cfg(all(feature = "local_fs", unix))]
#[test]
fn file_transfer_handle_is_connection_scoped() {
    let dir = tempfile::tempdir().unwrap();
    let path = canonical_temp_root(&dir).join("source.txt");
    fs::write(&path, b"owner bytes").unwrap();
    let mut model = test_model();
    let owner = test_connection_id();
    let foreign = insert_test_connection(&mut model);
    let handle = begin_transfer(&mut model, &path, FileTransferDirection::Read, owner);

    assert_transfer_error(model.handle_read_file_chunk(
        ReadFileChunk {
            handle: Some(handle.clone()),
            max_bytes: 1024,
        },
        foreign,
    ));

    assert!(model.connections[&owner]
        .file_transfers
        .contains_key(&handle.id));
    let owner_response = model.handle_read_file_chunk(
        ReadFileChunk {
            handle: Some(handle.clone()),
            max_bytes: 1024,
        },
        owner,
    );
    let server_message::Message::ReadFileChunkResponse(owner_response) =
        owner_response.into_message()
    else {
        panic!("expected ReadFileChunkResponse");
    };
    let Some(read_file_chunk_response::Result::Success(success)) = owner_response.result else {
        panic!("owner must retain access to its transfer");
    };
    assert_eq!(success.bytes, b"owner bytes");
}

#[cfg(all(feature = "local_fs", unix))]
#[test]
fn same_transfer_handle_locator_is_independent_across_connections() {
    let dir = tempfile::tempdir().unwrap();
    let root = canonical_temp_root(&dir);
    let first_path = root.join("first.txt");
    let second_path = root.join("second.txt");
    let mut model = test_model();
    let first_conn = test_connection_id();
    let second_conn = test_connection_id();
    let first_handle = begin_transfer(
        &mut model,
        &first_path,
        FileTransferDirection::Write,
        first_conn,
    );
    let second_handle = begin_transfer(
        &mut model,
        &second_path,
        FileTransferDirection::Write,
        second_conn,
    );
    let shared_handle = "same-connection-local-locator".to_owned();
    for (conn_id, original_handle) in [
        (first_conn, first_handle.id),
        (second_conn, second_handle.id),
    ] {
        let transfers = &mut model.connections.get_mut(&conn_id).unwrap().file_transfers;
        let state = transfers.remove(&original_handle).unwrap();
        assert!(transfers.insert(shared_handle.clone(), state).is_none());
    }
    let shared_handle = FileTransferHandle { id: shared_handle };

    for (conn_id, bytes) in [(first_conn, b"first".as_slice()), (second_conn, b"second")] {
        let response = model.handle_write_file_chunk(
            WriteFileChunk {
                handle: Some(shared_handle.clone()),
                bytes: bytes.to_vec(),
            },
            conn_id,
        );
        let server_message::Message::WriteFileChunkResponse(response) = response.into_message()
        else {
            panic!("expected WriteFileChunkResponse");
        };
        assert!(matches!(
            response.result,
            Some(write_file_chunk_response::Result::Success(_))
        ));
        finish_transfer_for_test(&mut model, shared_handle.clone(), conn_id);
    }

    assert_eq!(fs::read(&first_path).unwrap(), b"first");
    assert_eq!(fs::read(&second_path).unwrap(), b"second");
}

#[cfg(all(feature = "local_fs", unix))]
#[test]
fn foreign_finish_and_abort_do_not_remove_owner_transfer() {
    let dir = tempfile::tempdir().unwrap();
    let root = canonical_temp_root(&dir);
    let finish_path = root.join("finish.txt");
    let abort_path = root.join("abort.txt");
    let mut model = test_model();
    let owner = test_connection_id();
    let foreign = insert_test_connection(&mut model);
    let finish_handle = begin_transfer(
        &mut model,
        &finish_path,
        FileTransferDirection::Write,
        owner,
    );
    let abort_handle = begin_transfer(&mut model, &abort_path, FileTransferDirection::Write, owner);

    assert_transfer_error(model.handle_finish_file_transfer(
        FinishFileTransfer {
            handle: Some(finish_handle.clone()),
        },
        foreign,
    ));
    assert_transfer_error(model.handle_abort_file_transfer(
        AbortFileTransfer {
            handle: Some(abort_handle.clone()),
        },
        foreign,
    ));

    let owner_transfers = &model.connections[&owner].file_transfers;
    assert!(owner_transfers.contains_key(&finish_handle.id));
    assert!(owner_transfers.contains_key(&abort_handle.id));
    finish_transfer_for_test(&mut model, finish_handle, owner);
    let response = model.handle_abort_file_transfer(
        AbortFileTransfer {
            handle: Some(abort_handle),
        },
        owner,
    );
    let server_message::Message::AbortFileTransferResponse(response) = response.into_message()
    else {
        panic!("expected AbortFileTransferResponse");
    };
    assert!(matches!(
        response.result,
        Some(abort_file_transfer_response::Result::Success(_))
    ));
}

#[cfg(all(feature = "local_fs", unix))]
#[test]
fn disconnect_drops_only_owned_file_transfers() {
    let dir = tempfile::tempdir().unwrap();
    let root = canonical_temp_root(&dir);
    let owner_path = root.join("owner.txt");
    let other_path = root.join("other.txt");
    let mut model = test_model();
    let owner = test_connection_id();
    let other = test_connection_id();
    let owner_handle = begin_transfer(&mut model, &owner_path, FileTransferDirection::Write, owner);
    let other_handle = begin_transfer(&mut model, &other_path, FileTransferDirection::Write, other);
    let staging_path = |conn_id, handle: &FileTransferHandle| {
        let super::FileTransferState::Write {
            parent_path,
            staging_name,
            ..
        } = &model.connections[&conn_id].file_transfers[&handle.id]
        else {
            panic!("expected write transfer");
        };
        parent_path.join(staging_name.to_str().unwrap())
    };
    let owner_staging = staging_path(owner, &owner_handle);
    let other_staging = staging_path(other, &other_handle);
    assert!(owner_staging.exists());
    assert!(other_staging.exists());

    model.remove_connection_state(owner);

    assert!(!model.connections.contains_key(&owner));
    assert!(model.connections[&other]
        .file_transfers
        .contains_key(&other_handle.id));
    assert!(!owner_staging.exists());
    assert!(other_staging.exists());
}

#[cfg(all(feature = "local_fs", unix))]
fn finish_transfer_for_test(
    model: &mut ServerModel,
    handle: FileTransferHandle,
    conn_id: uuid::Uuid,
) -> Option<String> {
    let response = model.handle_finish_file_transfer(
        FinishFileTransfer {
            handle: Some(handle),
        },
        conn_id,
    );
    let server_message::Message::FinishFileTransferResponse(response) = response.into_message()
    else {
        panic!("expected FinishFileTransferResponse");
    };
    let Some(finish_file_transfer_response::Result::Success(success)) = response.result else {
        panic!("expected finish transfer success: {response:?}");
    };
    success.committed_path
}

#[cfg(all(feature = "local_fs", unix))]
#[test]
fn read_and_write_file_chunks_round_trip_binary_data() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = canonical_temp_root(&dir).join("blob.bin");
    let mut model = test_model();
    let conn_id = uuid::Uuid::new_v4();

    let write_handle = begin_transfer(
        &mut model,
        &file_path,
        FileTransferDirection::Write,
        conn_id,
    );
    let write_response = model.handle_write_file_chunk(
        WriteFileChunk {
            handle: Some(write_handle.clone()),
            bytes: vec![0, 1, 2, 3],
        },
        conn_id,
    );
    let server_message::Message::WriteFileChunkResponse(write_response) =
        write_response.into_message()
    else {
        panic!("expected WriteFileChunkResponse");
    };
    let Some(write_file_chunk_response::Result::Success(write_success)) = write_response.result
    else {
        panic!("expected write chunk success");
    };
    assert_eq!(write_success.next_offset, 4);
    assert_eq!(
        finish_transfer_for_test(&mut model, write_handle, conn_id).as_deref(),
        Some(file_path.to_string_lossy().as_ref())
    );

    let read_handle = begin_transfer(&mut model, &file_path, FileTransferDirection::Read, conn_id);
    let read_response = model.handle_read_file_chunk(
        ReadFileChunk {
            handle: Some(read_handle.clone()),
            max_bytes: 2,
        },
        conn_id,
    );
    let server_message::Message::ReadFileChunkResponse(read_response) =
        read_response.into_message()
    else {
        panic!("expected ReadFileChunkResponse");
    };
    let Some(read_file_chunk_response::Result::Success(read_success)) = read_response.result else {
        panic!("expected read chunk success");
    };
    assert_eq!(read_success.bytes, vec![0, 1]);
    assert_eq!(read_success.next_offset, 2);
    assert_eq!(read_success.total_size, Some(4));
    assert!(!read_success.eof);

    let read_response = model.handle_read_file_chunk(
        ReadFileChunk {
            handle: Some(read_handle.clone()),
            max_bytes: 2,
        },
        conn_id,
    );
    let server_message::Message::ReadFileChunkResponse(read_response) =
        read_response.into_message()
    else {
        panic!("expected ReadFileChunkResponse");
    };
    let Some(read_file_chunk_response::Result::Success(read_success)) = read_response.result else {
        panic!("expected read chunk success");
    };
    assert_eq!(read_success.bytes, vec![2, 3]);
    assert_eq!(read_success.next_offset, 4);
    assert_eq!(read_success.total_size, Some(4));
    assert!(read_success.eof);
    finish_transfer_for_test(&mut model, read_handle, conn_id);
}

#[cfg(all(feature = "local_fs", unix))]
#[test]
fn file_chunk_rpc_rejects_symlink_instead_of_reading_or_overwriting_target() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let root = canonical_temp_root(&dir);
    let target = root.join("target.txt");
    let link = root.join("link.txt");
    fs::write(&target, "secret target").unwrap();
    symlink(&target, &link).unwrap();
    let mut model = test_model();
    let conn_id = uuid::Uuid::new_v4();

    for direction in [FileTransferDirection::Read, FileTransferDirection::Write] {
        let response = model.handle_begin_file_transfer(
            BeginFileTransfer {
                path: link.to_string_lossy().into_owned(),
                direction: direction as i32,
                executable: None,
            },
            conn_id,
        );
        let server_message::Message::BeginFileTransferResponse(response) = response.into_message()
        else {
            panic!("expected BeginFileTransferResponse");
        };
        assert!(matches!(
            response.result,
            Some(begin_file_transfer_response::Result::Error(_))
        ));
    }
    assert_eq!(fs::read_to_string(&target).unwrap(), "secret target");
}

#[cfg(all(feature = "local_fs", unix))]
#[test]
fn read_chunk_transfer_pins_inode_across_path_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let root = canonical_temp_root(&dir);
    let path = root.join("source.txt");
    let moved = root.join("source.old");
    fs::write(&path, b"original inode").unwrap();
    let mut model = test_model();
    let conn_id = uuid::Uuid::new_v4();
    let handle = begin_transfer(&mut model, &path, FileTransferDirection::Read, conn_id);

    fs::rename(&path, &moved).unwrap();
    fs::write(&path, b"replacement path").unwrap();
    let response = model.handle_read_file_chunk(
        ReadFileChunk {
            handle: Some(handle.clone()),
            max_bytes: 1024,
        },
        conn_id,
    );
    let server_message::Message::ReadFileChunkResponse(response) = response.into_message() else {
        panic!("expected ReadFileChunkResponse");
    };
    let Some(read_file_chunk_response::Result::Success(success)) = response.result else {
        panic!("expected read success");
    };
    assert_eq!(success.bytes, b"original inode");
    finish_transfer_for_test(&mut model, handle, conn_id);
}

#[cfg(all(feature = "local_fs", unix))]
#[test]
fn read_transfer_does_not_include_bytes_appended_after_begin() {
    use std::io::Write as _;

    let dir = tempfile::tempdir().unwrap();
    let path = canonical_temp_root(&dir).join("growing.txt");
    fs::write(&path, b"base").unwrap();
    let mut model = test_model();
    let conn_id = test_connection_id();
    let handle = begin_transfer(&mut model, &path, FileTransferDirection::Read, conn_id);
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"-later")
        .unwrap();

    let response = model.handle_read_file_chunk(
        ReadFileChunk {
            handle: Some(handle),
            max_bytes: 1024,
        },
        conn_id,
    );
    let server_message::Message::ReadFileChunkResponse(response) = response.into_message() else {
        panic!("expected ReadFileChunkResponse");
    };
    let Some(read_file_chunk_response::Result::Success(success)) = response.result else {
        panic!("expected read success");
    };
    assert_eq!(success.bytes, b"base");
    assert_eq!(success.next_offset, 4);
    assert_eq!(success.total_size, Some(4));
    assert!(success.eof);
}

#[cfg(all(feature = "local_fs", unix))]
#[test]
fn read_transfer_reports_truncation_instead_of_successful_eof() {
    let dir = tempfile::tempdir().unwrap();
    let path = canonical_temp_root(&dir).join("truncated.txt");
    fs::write(&path, b"abcdef").unwrap();
    let mut model = test_model();
    let conn_id = test_connection_id();
    let handle = begin_transfer(&mut model, &path, FileTransferDirection::Read, conn_id);
    fs::write(&path, b"ab").unwrap();

    let first = model.handle_read_file_chunk(
        ReadFileChunk {
            handle: Some(handle.clone()),
            max_bytes: 1024,
        },
        conn_id,
    );
    let server_message::Message::ReadFileChunkResponse(first) = first.into_message() else {
        panic!("expected ReadFileChunkResponse");
    };
    let Some(read_file_chunk_response::Result::Success(first)) = first.result else {
        panic!("first short read may still make progress");
    };
    assert_eq!(first.bytes, b"ab");
    assert!(!first.eof);

    assert_transfer_error(model.handle_read_file_chunk(
        ReadFileChunk {
            handle: Some(handle.clone()),
            max_bytes: 1024,
        },
        conn_id,
    ));
    assert_transfer_error(model.handle_read_file_chunk(
        ReadFileChunk {
            handle: Some(handle),
            max_bytes: 1024,
        },
        conn_id,
    ));
}

#[cfg(all(feature = "local_fs", unix))]
#[test]
fn read_transfer_rejects_zero_budget_before_eof() {
    let dir = tempfile::tempdir().unwrap();
    let path = canonical_temp_root(&dir).join("source.txt");
    fs::write(&path, b"x").unwrap();
    let mut model = test_model();
    let conn_id = test_connection_id();
    let handle = begin_transfer(&mut model, &path, FileTransferDirection::Read, conn_id);

    assert_transfer_error(model.handle_read_file_chunk(
        ReadFileChunk {
            handle: Some(handle.clone()),
            max_bytes: 0,
        },
        conn_id,
    ));
    assert_transfer_error(model.handle_read_file_chunk(
        ReadFileChunk {
            handle: Some(handle),
            max_bytes: 1,
        },
        conn_id,
    ));
}

#[cfg(all(feature = "local_fs", unix))]
#[test]
fn partial_read_finish_fails_and_removes_handle() {
    let dir = tempfile::tempdir().unwrap();
    let path = canonical_temp_root(&dir).join("source.txt");
    fs::write(&path, b"abcdef").unwrap();
    let mut model = test_model();
    let conn_id = test_connection_id();
    let handle = begin_transfer(&mut model, &path, FileTransferDirection::Read, conn_id);

    let response = model.handle_read_file_chunk(
        ReadFileChunk {
            handle: Some(handle.clone()),
            max_bytes: 2,
        },
        conn_id,
    );
    let server_message::Message::ReadFileChunkResponse(response) = response.into_message() else {
        panic!("expected ReadFileChunkResponse");
    };
    assert!(matches!(
        response.result,
        Some(read_file_chunk_response::Result::Success(_))
    ));

    let response = model.handle_finish_file_transfer(
        FinishFileTransfer {
            handle: Some(handle.clone()),
        },
        conn_id,
    );
    let server_message::Message::FinishFileTransferResponse(response) = response.into_message()
    else {
        panic!("expected FinishFileTransferResponse");
    };
    assert!(matches!(
        response.result,
        Some(finish_file_transfer_response::Result::Error(_))
    ));
    assert_transfer_error(model.handle_read_file_chunk(
        ReadFileChunk {
            handle: Some(handle),
            max_bytes: 2,
        },
        conn_id,
    ));
}

#[cfg(all(feature = "local_fs", unix))]
#[test]
fn write_chunk_transfer_never_splices_inodes() {
    let dir = tempfile::tempdir().unwrap();
    let root = canonical_temp_root(&dir);
    let path = root.join("destination.txt");
    let replaced = root.join("destination.replaced");
    fs::write(&path, b"first destination").unwrap();
    let mut model = test_model();
    let conn_id = uuid::Uuid::new_v4();
    let handle = begin_transfer(&mut model, &path, FileTransferDirection::Write, conn_id);

    for bytes in [b"first ".as_slice(), b"second".as_slice()] {
        let response = model.handle_write_file_chunk(
            WriteFileChunk {
                handle: Some(handle.clone()),
                bytes: bytes.to_vec(),
            },
            conn_id,
        );
        let server_message::Message::WriteFileChunkResponse(response) = response.into_message()
        else {
            panic!("expected WriteFileChunkResponse");
        };
        assert!(matches!(
            response.result,
            Some(write_file_chunk_response::Result::Success(_))
        ));
        if bytes == b"first " {
            fs::rename(&path, &replaced).unwrap();
            fs::write(&path, b"second destination").unwrap();
        }
    }
    assert_eq!(fs::read(&replaced).unwrap(), b"first destination");
    assert_eq!(fs::read(&path).unwrap(), b"second destination");
    finish_transfer_for_test(&mut model, handle, conn_id);
    assert_eq!(fs::read(&path).unwrap(), b"first second");
    assert_eq!(fs::read(&replaced).unwrap(), b"first destination");
}

#[cfg(feature = "local_fs")]
#[test]
fn create_directory_creates_nested_directories() {
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("a/b/c");
    let model = test_model();

    let response = model.handle_create_directory(CreateDirectory {
        path: nested.to_string_lossy().to_string(),
    });

    let server_message::Message::CreateDirectoryResponse(response) = response.into_message() else {
        panic!("expected CreateDirectoryResponse");
    };
    assert!(matches!(
        response.result,
        Some(super::super::proto::create_directory_response::Result::Success(_))
    ));
    assert!(nested.is_dir());
}

#[cfg(all(feature = "local_fs", unix))]
#[test]
fn delete_directory_has_explicit_empty_directory_semantics() {
    use std::os::unix::fs::MetadataExt;

    let dir = tempfile::tempdir().unwrap();
    let root = canonical_temp_root(&dir);
    let empty = root.join("empty");
    let nonempty = root.join("nonempty");
    fs::create_dir(&empty).unwrap();
    fs::create_dir(&nonempty).unwrap();
    fs::write(nonempty.join("child.txt"), "child").unwrap();
    let model = test_model();

    let metadata = fs::symlink_metadata(&empty).unwrap();
    let response = model.handle_delete_directory(DeleteDirectory {
        path: empty.to_string_lossy().to_string(),
        identity: Some(DeleteDirectoryIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        }),
    });
    let server_message::Message::DeleteDirectoryResponse(response) = response.into_message() else {
        panic!("expected DeleteDirectoryResponse");
    };
    assert!(matches!(
        response.result,
        Some(super::super::proto::delete_directory_response::Result::Success(_))
    ));
    assert!(!empty.exists());

    let metadata = fs::symlink_metadata(&nonempty).unwrap();
    let response = model.handle_delete_directory(DeleteDirectory {
        path: nonempty.to_string_lossy().to_string(),
        identity: Some(DeleteDirectoryIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        }),
    });
    let server_message::Message::DeleteDirectoryResponse(response) = response.into_message() else {
        panic!("expected DeleteDirectoryResponse");
    };
    assert!(matches!(
        response.result,
        Some(super::super::proto::delete_directory_response::Result::Success(_))
    ));
    assert!(!nonempty.exists());
}

#[cfg(all(feature = "local_fs", unix))]
#[test]
fn remote_native_append_preserves_concurrent_provider_write() {
    use std::io::Write;
    use std::sync::{Arc, Barrier};

    let dir = tempfile::tempdir().unwrap();
    let path = canonical_temp_root(&dir).join("events.jsonl");
    fs::write(&path, b"seed\n").unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let provider_path = path.clone();
    let provider_barrier = barrier.clone();
    let provider = std::thread::spawn(move || {
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(provider_path)
            .unwrap();
        provider_barrier.wait();
        for index in 0..200 {
            let record = format!("provider-{index:03}\n");
            assert_eq!(file.write(record.as_bytes()).unwrap(), record.len());
        }
    });
    let helper_path = path.clone();
    let helper = std::thread::spawn(move || {
        barrier.wait();
        for index in 0..200 {
            super::append_file_nofollow(&helper_path, format!("ashide-{index:03}\n").as_bytes())
                .unwrap();
        }
    });
    provider.join().unwrap();
    helper.join().unwrap();

    let contents = fs::read_to_string(path).unwrap();
    assert_eq!(contents.lines().count(), 401);
    for index in 0..200 {
        assert!(contents.contains(&format!("provider-{index:03}\n")));
        assert!(contents.contains(&format!("ashide-{index:03}\n")));
    }
}

#[cfg(all(feature = "local_fs", unix))]
#[test]
fn remote_native_append_does_not_rewrite_existing_file() {
    use std::os::unix::fs::MetadataExt;

    let dir = tempfile::tempdir().unwrap();
    let path = canonical_temp_root(&dir).join("events.jsonl");
    fs::write(&path, b"existing\n").unwrap();
    let inode = fs::metadata(&path).unwrap().ino();
    let model = test_model();
    let response = model.handle_append_file(AppendFile {
        path: path.to_string_lossy().into_owned(),
        bytes: b"appended\n".to_vec(),
    });
    let server_message::Message::AppendFileResponse(response) = response.into_message() else {
        panic!("expected AppendFileResponse");
    };
    assert!(matches!(
        response.result,
        Some(append_file_response::Result::Success(_))
    ));
    assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
    assert_eq!(fs::read(&path).unwrap(), b"existing\nappended\n");
}

#[cfg(all(feature = "local_fs", unix))]
#[test]
fn workspace_remote_rename_to_existing_directory_does_not_move_inside() {
    let dir = tempfile::tempdir().unwrap();
    let root = canonical_temp_root(&dir);
    let source = root.join("source.txt");
    let destination = root.join("existing-directory");
    fs::write(&source, b"source").unwrap();
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("keep.txt"), b"keep").unwrap();
    let model = test_model();

    let response = model.handle_exact_rename(ExactRename {
        from_path: source.to_string_lossy().into_owned(),
        to_path: destination.to_string_lossy().into_owned(),
    });
    let server_message::Message::ExactRenameResponse(response) = response.into_message() else {
        panic!("expected ExactRenameResponse");
    };
    assert!(matches!(
        response.result,
        Some(exact_rename_response::Result::Conflict(_))
            | Some(exact_rename_response::Result::Error(_))
    ));
    assert!(source.is_file());
    assert_eq!(fs::read(destination.join("keep.txt")).unwrap(), b"keep");
    assert!(!destination.join("source.txt").exists());
}

#[cfg(all(feature = "local_fs", unix))]
#[test]
fn workspace_upload_promotion_rechecks_remote_symlink() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let root = canonical_temp_root(&dir);
    let staging = root.join("staging.txt");
    let final_path = root.join("final.txt");
    let target = root.join("target.txt");
    fs::write(&staging, b"staging").unwrap();
    fs::write(&target, b"target").unwrap();
    symlink(&target, &final_path).unwrap();
    let model = test_model();

    let response = model.handle_promote_files(PromoteFiles {
        targets: vec![PromotionTarget {
            staging_path: staging.to_string_lossy().into_owned(),
            final_path: final_path.to_string_lossy().into_owned(),
        }],
        overwrite: true,
        directory_overwrite_roots: Vec::new(),
    });
    let server_message::Message::PromoteFilesResponse(response) = response.into_message() else {
        panic!("expected PromoteFilesResponse");
    };
    let Some(promote_files_response::Result::Success(success)) = response.result else {
        panic!("expected typed per-target promotion result");
    };
    assert_eq!(success.results.len(), 1);
    assert_eq!(
        PromotionStatus::try_from(success.results[0].status).unwrap(),
        PromotionStatus::Conflict
    );
    assert!(staging.is_file());
    assert_eq!(fs::read(&target).unwrap(), b"target");
    assert!(fs::symlink_metadata(&final_path)
        .unwrap()
        .file_type()
        .is_symlink());
}

#[cfg(unix)]
#[test]
fn native_remote_pty_round_trips_output() {
    use super::super::proto::{create_pty_response, CreatePty, PtyOutputPush, WritePty};
    use std::time::Duration;

    let mut model = test_model();
    let conn_id = uuid::Uuid::new_v4();
    let (tx, rx) = async_channel::unbounded();
    model.connections.insert(conn_id, ConnectionState::new(tx));

    let response = model
        .handle_create_pty(
            CreatePty {
                working_directory: "/".to_string(),
                shell: "/bin/sh".to_string(),
                rows: 24,
                cols: 80,
                environment_variables: HashMap::new(),
            },
            &request_id(),
            conn_id,
        )
        .into_message();

    let server_message::Message::CreatePtyResponse(response) = response else {
        panic!("expected CreatePtyResponse");
    };
    let Some(create_pty_response::Result::Success(success)) = response.result else {
        panic!("expected CreatePtySuccess");
    };

    model.handle_write_pty(
        conn_id,
        WritePty {
            pty_id: success.pty_id,
            bytes: b"printf ASHIDE_PTY_OK\\n; exit\n".to_vec(),
        },
    );

    let (seen_tx, seen_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut output = Vec::new();
        while let Ok(msg) = rx.recv_blocking() {
            match msg.message {
                Some(server_message::Message::PtyOutput(PtyOutputPush { bytes, .. })) => {
                    output.extend(bytes);
                    if output
                        .windows(b"ASHIDE_PTY_OK".len())
                        .any(|w| w == b"ASHIDE_PTY_OK")
                    {
                        let _ = seen_tx.send(Ok(()));
                        return;
                    }
                }
                Some(server_message::Message::PtyExited(_)) => break,
                _ => {}
            }
        }
        let _ = seen_tx.send(Err(String::from_utf8_lossy(&output).to_string()));
    });

    match seen_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => {}
        Ok(Err(output)) => panic!("did not receive PTY output marker; output={output}"),
        Err(error) => panic!("timed out waiting for PTY output marker: {error}"),
    }
}

#[cfg(unix)]
#[test]
fn owner_close_pty_removes_resource() {
    use super::super::proto::{create_pty_response, ClosePty, CreatePty};

    let mut model = test_model();
    let conn_id = uuid::Uuid::new_v4();
    let (tx, _rx) = async_channel::unbounded();
    model.connections.insert(conn_id, ConnectionState::new(tx));

    let response = model
        .handle_create_pty(
            CreatePty {
                working_directory: "/".to_string(),
                shell: "/bin/sh".to_string(),
                rows: 24,
                cols: 80,
                environment_variables: HashMap::new(),
            },
            &request_id(),
            conn_id,
        )
        .into_message();

    let server_message::Message::CreatePtyResponse(response) = response else {
        panic!("expected CreatePtyResponse");
    };
    let Some(create_pty_response::Result::Success(success)) = response.result else {
        panic!("expected CreatePtySuccess");
    };
    assert!(model.connections[&conn_id]
        .ptys
        .contains_key(&success.pty_id));

    model.handle_close_pty(
        conn_id,
        ClosePty {
            pty_id: success.pty_id,
        },
    );

    assert!(!model.connections[&conn_id]
        .ptys
        .contains_key(&success.pty_id));
}

#[cfg(unix)]
#[test]
fn foreign_close_pty_does_not_remove_owner_resource() {
    use super::super::proto::{create_pty_response, ClosePty, CreatePty};

    let mut model = test_model();
    let owner = insert_test_connection(&mut model);
    let other = insert_test_connection(&mut model);
    let response = model
        .handle_create_pty(
            CreatePty {
                working_directory: "/".to_string(),
                shell: "/bin/sh".to_string(),
                rows: 24,
                cols: 80,
                environment_variables: HashMap::new(),
            },
            &request_id(),
            owner,
        )
        .into_message();
    let server_message::Message::CreatePtyResponse(response) = response else {
        panic!("expected CreatePtyResponse");
    };
    let Some(create_pty_response::Result::Success(success)) = response.result else {
        panic!("expected CreatePtySuccess");
    };

    model.handle_close_pty(
        other,
        ClosePty {
            pty_id: success.pty_id,
        },
    );

    assert!(model.connections[&owner].ptys.contains_key(&success.pty_id));
}

#[cfg(unix)]
#[test]
fn pty_mutation_handlers_require_connection_owner() {
    use super::super::proto::{create_pty_response, CreatePty, ResizePty};
    use std::os::fd::AsRawFd as _;

    let mut model = test_model();
    let owner = insert_test_connection(&mut model);
    let other = insert_test_connection(&mut model);
    let response = model
        .handle_create_pty(
            CreatePty {
                working_directory: "/".to_string(),
                shell: "/bin/sh".to_string(),
                rows: 24,
                cols: 80,
                environment_variables: HashMap::new(),
            },
            &request_id(),
            owner,
        )
        .into_message();
    let server_message::Message::CreatePtyResponse(response) = response else {
        panic!("expected CreatePtyResponse");
    };
    let Some(create_pty_response::Result::Success(success)) = response.result else {
        panic!("expected CreatePtySuccess");
    };
    let master_fd = model.connections[&owner].ptys[&success.pty_id]
        .master
        .as_raw_fd();

    model.handle_resize_pty(
        other,
        ResizePty {
            pty_id: success.pty_id,
            rows: 40,
            cols: 120,
            width_px: 0,
            height_px: 0,
        },
    );
    let mut size = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    assert_eq!(
        unsafe {
            libc::ioctl(
                master_fd,
                libc::TIOCGWINSZ as super::IoctlRequest,
                &mut size,
            )
        },
        0
    );
    assert_eq!((size.ws_row, size.ws_col), (24, 80));

    model.handle_resize_pty(
        owner,
        ResizePty {
            pty_id: success.pty_id,
            rows: 40,
            cols: 120,
            width_px: 0,
            height_px: 0,
        },
    );
    assert_eq!(
        unsafe {
            libc::ioctl(
                master_fd,
                libc::TIOCGWINSZ as super::IoctlRequest,
                &mut size,
            )
        },
        0
    );
    assert_eq!((size.ws_row, size.ws_col), (40, 120));
}

#[cfg(unix)]
#[test]
fn disconnect_removes_only_owned_ptys() {
    use super::super::proto::{create_pty_response, CreatePty};

    let mut model = test_model();
    let owner = insert_test_connection(&mut model);
    let other = insert_test_connection(&mut model);
    let mut pty_ids = HashMap::new();
    for conn_id in [owner, other] {
        let response = model
            .handle_create_pty(
                CreatePty {
                    working_directory: "/".to_string(),
                    shell: "/bin/sh".to_string(),
                    rows: 24,
                    cols: 80,
                    environment_variables: HashMap::new(),
                },
                &request_id(),
                conn_id,
            )
            .into_message();
        let server_message::Message::CreatePtyResponse(response) = response else {
            panic!("expected CreatePtyResponse");
        };
        let Some(create_pty_response::Result::Success(success)) = response.result else {
            panic!("expected CreatePtySuccess");
        };
        pty_ids.insert(conn_id, success.pty_id);
    }

    model.remove_connection_state(owner);

    assert!(!model.connections.contains_key(&owner));
    assert!(model.connections[&other]
        .ptys
        .contains_key(&pty_ids[&other]));
}

#[cfg(all(feature = "local_fs", unix))]
#[test]
fn repo_metadata_directory_load_keeps_external_symlink_lexical_identity() {
    use std::os::unix::fs::symlink;

    let fixture = tempfile::tempdir().unwrap();
    let repo = fixture.path().join("repo");
    let external = fixture.path().join("external");
    fs::create_dir(&repo).unwrap();
    fs::create_dir(&external).unwrap();
    let link = repo.join("dir-link");
    symlink(&external, &link).unwrap();

    let (validated_repo, validated_dir) = validate_repo_metadata_directory_load_paths(
        &repo.to_string_lossy(),
        &link.to_string_lossy(),
    )
    .expect("an in-repo lexical symlink path must be accepted");

    assert_eq!(validated_repo.to_string(), repo.to_string_lossy());
    assert_eq!(validated_dir.to_string(), link.to_string_lossy());
    assert_eq!(
        fs::canonicalize(&link).unwrap(),
        fs::canonicalize(&external).unwrap()
    );
    assert!(validated_dir.starts_with(&validated_repo));
}

#[cfg(all(feature = "local_fs", unix))]
#[test]
fn symlink_directory_listing_preserves_link_path_identity() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("child.txt"), "hello").unwrap();
    let link = dir.path().join("link");
    symlink(&target, &link).unwrap();
    let model = test_model();

    let parent_response = model.handle_list_directory(ListDirectory {
        path: dir.path().to_string_lossy().to_string(),
    });
    let server_message::Message::ListDirectoryResponse(parent_response) =
        parent_response.into_message()
    else {
        panic!("expected ListDirectoryResponse");
    };
    let Some(list_directory_response::Result::Success(parent)) = parent_response.result else {
        panic!("expected parent listing success");
    };
    let link_entry = parent
        .entries
        .iter()
        .find(|entry| entry.name == "link")
        .expect("missing symlink entry");
    assert_eq!(
        link_entry.kind,
        super::super::proto::FileSystemEntryKind::Symlink as i32
    );
    assert_eq!(
        link_entry.target_kind,
        super::super::proto::FileSystemEntryKind::Directory as i32
    );

    let link_response = model.handle_list_directory(ListDirectory {
        path: link.to_string_lossy().to_string(),
    });
    let server_message::Message::ListDirectoryResponse(link_response) =
        link_response.into_message()
    else {
        panic!("expected ListDirectoryResponse");
    };
    let Some(list_directory_response::Result::Success(listing)) = link_response.result else {
        panic!("expected symlink directory listing success");
    };
    assert_eq!(listing.path, link.to_string_lossy());
    assert_eq!(listing.entries[0].name, "child.txt");
}

#[cfg(all(feature = "local_fs", unix))]
#[test]
fn create_directory_inside_symlink_parent_keeps_caller_owned_identity() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    fs::create_dir(&target).unwrap();
    let link = dir.path().join("link");
    symlink(&target, &link).unwrap();
    let requested = link.join("New Folder");
    let model = test_model();

    let response = model.handle_create_directory(CreateDirectory {
        path: requested.to_string_lossy().to_string(),
    });
    let server_message::Message::CreateDirectoryResponse(response) = response.into_message() else {
        panic!("expected CreateDirectoryResponse");
    };
    assert!(matches!(
        response.result,
        Some(super::super::proto::create_directory_response::Result::Success(_))
    ));
    assert!(target.join("New Folder").is_dir());
}

#[cfg(all(feature = "local_fs", unix))]
#[test]
fn resolve_broken_symlink_keeps_link_identity() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let link = dir.path().join("broken");
    symlink(dir.path().join("missing"), &link).unwrap();
    let model = test_model();

    let response = model.handle_resolve_path(ResolvePath {
        path: link.to_string_lossy().to_string(),
    });
    let server_message::Message::ResolvePathResponse(response) = response.into_message() else {
        panic!("expected ResolvePathResponse");
    };
    let Some(resolve_path_response::Result::Success(success)) = response.result else {
        panic!("expected broken symlink resolve success");
    };
    assert_eq!(success.path, link.to_string_lossy());
    assert_eq!(success.resolved_path, None);
    assert_eq!(
        success.kind,
        super::super::proto::FileSystemEntryKind::Symlink as i32
    );
    assert_eq!(
        success.target_kind,
        super::super::proto::FileSystemEntryKind::Missing as i32
    );
}

#[cfg(unix)]
#[test]
fn remote_pty_default_context_uses_target_passwd_record() {
    let defaults = remote_pty_user_defaults().expect("current target user must have passwd data");
    let user = nix::unistd::User::from_uid(nix::unistd::getuid())
        .expect("passwd lookup must succeed")
        .expect("current target user must exist");

    assert_eq!(defaults.home, user.dir);
    assert_eq!(defaults.shell, user.shell);
}
