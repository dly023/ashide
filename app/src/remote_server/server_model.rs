use crate::terminal::shell::ShellType;
use futures_util::stream::AbortHandle;
use remote_server::session_execution_context::{
    validate_marked_target_session_snapshot, AUTHORITATIVE_SESSION_ENVIRONMENT_VARIABLES,
};
use repo_metadata::repositories::{DetectedRepositories, RepoDetectionSource};
use repo_metadata::{RepoMetadataEvent, RepoMetadataModel, RepositoryIdentifier};
use std::collections::{HashMap, HashSet};
use std::future::Future;
#[cfg(feature = "local_fs")]
use std::io;
#[cfg(unix)]
use std::io::{Read as _, Write as _};
#[cfg(unix)]
use std::os::fd::{AsRawFd as _, FromRawFd as _};
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::Stdio;
use std::sync::Arc;
#[cfg(unix)]
use std::thread;
use warp_core::channel::ChannelState;
use warp_core::SessionId;
use warp_util::standardized_path::StandardizedPath;
use warpui::platform::TerminationMode;
use warpui::r#async::{Spawnable, SpawnableOutput, SpawnedFutureHandle};
use warpui::{Entity, ModelContext, SingletonEntity};

#[cfg(feature = "local_fs")]
use string_offset::CharOffset;
use warp_files::{FileModel, FileModelEvent};
use warp_util::content_version::ContentVersion;
use warp_util::file::FileId;

use super::proto::{
    client_message, create_pty_response, delete_file_response, run_command_response,
    server_message, write_file_response, Abort, Authenticate, ClientMessage, ClosePty, CreatePty,
    CreatePtyError, CreatePtyResponse, CreatePtySuccess, DeleteFile, DeleteFileResponse,
    DeleteFileSuccess, ErrorCode, ErrorResponse, FailedFileRead, FileContextProto,
    FileOperationError, Initialize, InitializeResponse, NavigatedToDirectory,
    NavigatedToDirectoryResponse, PtyExitedPush, PtyOutputPush, ReadFileContextResponse, ResizePty,
    RunCommandError, RunCommandErrorCode, RunCommandRequest, RunCommandResponse, RunCommandSuccess,
    ServerMessage, SessionBootstrapped, SessionExecutionContextDeregistered, WriteFile,
    WriteFileResponse, WriteFileSuccess, WritePty,
};

// Buffer-sync 相关:依赖 GlobalBufferModel,后者的 server-managed current-app 操作只在
// `local_fs` 下可用,因此整套服务端 buffer 处理都按 `local_fs` 门控。
#[cfg(feature = "local_fs")]
use super::proto::{
    abort_file_transfer_response, append_file_response, begin_file_transfer_response,
    create_directory_response, delete_directory_response, exact_rename_response,
    finish_file_transfer_response, get_cli_agent_session_user_state_response,
    list_directory_response, mutate_cli_agent_session_response,
    mutate_cli_agent_session_user_state_response, promote_files_response,
    read_cli_agent_session_response, read_file_chunk_response, resolve_conflict_response,
    resolve_path_response, save_buffer_response, scan_cli_agent_sessions_response,
    write_file_chunk_response, AbortFileTransfer, AbortFileTransferResponse,
    AbortFileTransferSuccess, AppendFile, AppendFileResponse, AppendFileSuccess, BeginFileTransfer,
    BeginFileTransferResponse, BeginFileTransferSuccess, BufferEdit, BufferUpdatedPush,
    CliAgentSessionMutation, CliAgentSessionRecord, CliAgentSessionStoreRoots,
    CliAgentSessionUserState, CliAgentSessionUserStateMutation, CloseBuffer, CreateDirectory,
    CreateDirectoryResponse, CreateDirectorySuccess, DeleteDirectory, DeleteDirectoryIdentity,
    DeleteDirectoryResponse, DeleteDirectorySuccess, DirEntry, ExactRename, ExactRenameConflict,
    ExactRenameResponse, ExactRenameSuccess, FileSystemEntryKind, FileTransferDirection,
    FileTransferHandle, FinishFileTransfer, FinishFileTransferResponse, FinishFileTransferSuccess,
    GetCliAgentSessionUserState, GetCliAgentSessionUserStateResponse,
    GetCliAgentSessionUserStateSuccess, ListDirectory, ListDirectoryResponse, ListDirectorySuccess,
    MutateCliAgentSession, MutateCliAgentSessionResponse, MutateCliAgentSessionSuccess,
    MutateCliAgentSessionUserState, MutateCliAgentSessionUserStateResponse,
    MutateCliAgentSessionUserStateSuccess, OpenBuffer, OpenBufferResponse, PromoteFiles,
    PromoteFilesResponse, PromoteFilesSuccess, PromotionResult, PromotionStatus,
    ReadCliAgentSession, ReadCliAgentSessionResponse, ReadCliAgentSessionSuccess, ReadFileChunk,
    ReadFileChunkResponse, ReadFileChunkSuccess, ResolveConflict, ResolveConflictResponse,
    ResolveConflictSuccess, ResolvePath, ResolvePathNotFound, ResolvePathResponse,
    ResolvePathSuccess, SaveBuffer, SaveBufferResponse, SaveBufferSuccess, ScanCliAgentSessions,
    ScanCliAgentSessionsResponse, ScanCliAgentSessionsSuccess, TextEdit, WriteFileChunk,
    WriteFileChunkResponse, WriteFileChunkSuccess,
};
#[cfg(feature = "local_fs")]
use super::server_buffer_tracker::{
    BufferWriterAccessError, PendingBufferMutation, PendingBufferMutationKind, ServerBufferTracker,
};
#[cfg(feature = "local_fs")]
use crate::code::global_buffer_model::{CharOffsetEdit, GlobalBufferModel, GlobalBufferModelEvent};

/// How long the daemon waits with no connections before exiting.
pub const GRACE_PERIOD: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Unique identifier for a connected proxy session in daemon mode.
pub type ConnectionId = uuid::Uuid;
use super::protocol::RequestId;
use crate::ai::agent::FileLocations;
use crate::ai::blocklist::{read_current_app_file_context, ReadFileContextResult};
use crate::terminal::capability_environment::terminal_capability_environment_variables_to_remove;
use crate::terminal::model::session::command_executor::{
    ExecuteCommandOptions, LocalCommandExecutionContext, LocalCommandExecutor,
};
#[cfg(unix)]
use command::blocking::Command;
#[cfg(unix)]
use libc::winsize;
#[cfg(unix)]
use nix::pty::openpty;

#[cfg(unix)]
#[derive(Clone, Debug, PartialEq, Eq)]
struct RemotePtyUserDefaults {
    home: PathBuf,
    shell: PathBuf,
}

#[cfg(unix)]
fn remote_pty_user_defaults() -> Result<RemotePtyUserDefaults, String> {
    let user = nix::unistd::User::from_uid(nix::unistd::getuid())
        .map_err(|error| format!("failed to read target passwd entry: {error}"))?
        .ok_or_else(|| "target user has no passwd entry".to_owned())?;
    if user.dir.as_os_str().is_empty() || user.shell.as_os_str().is_empty() {
        return Err("target passwd entry has an empty home or shell".to_owned());
    }
    Ok(RemotePtyUserDefaults {
        home: user.dir,
        shell: user.shell,
    })
}

/// Outcome of dispatching a request-style `ClientMessage`.
///
/// Notifications (fire-and-forget messages like `SessionBootstrapped` and
/// `Abort`) do not produce a `HandlerOutcome`; they are dispatched inline in
/// `handle_message` and return early.
enum HandlerOutcome {
    /// The response is ready synchronously — the caller sends it immediately.
    Sync(server_message::Message),
    /// The handler initiated async work whose response will be sent later.
    ///
    /// When the handle is `Some`, the caller inserts it into `in_progress`
    /// so the request can be cancelled via `Abort`. Removal on
    /// completion/abort is arranged by [`ServerModel::spawn_request_handler`].
    ///
    /// `None` is used for async work whose completion is delivered through
    /// a separate event subscription and is not currently cancellable via
    /// `Abort` (e.g. `FileModel` events for file writes and deletes, which
    /// are tracked by `FileId` in `pending_file_ops` rather than by
    /// `RequestId` in `in_progress`).
    Async(Option<SpawnedFutureHandle>),
}

#[cfg(test)]
impl HandlerOutcome {
    fn into_message(self) -> server_message::Message {
        match self {
            HandlerOutcome::Sync(message) => message,
            HandlerOutcome::Async(_) => panic!("expected synchronous handler outcome"),
        }
    }
}

/// Tracks an in-flight file write or delete so the async completion
/// event can be correlated back to the originating client request.
enum FileOpKind {
    Write,
    Delete,
}

struct PendingFileOp {
    request_id: RequestId,
    conn_id: ConnectionId,
    kind: FileOpKind,
}

/// Manages pending file operations and ensures that the corresponding
/// `FileModel` entry is always cleaned up when an operation completes
/// or fails, preventing `FileState` leaks.
struct PendingFileOps {
    ops: HashMap<FileId, PendingFileOp>,
}

impl PendingFileOps {
    fn new() -> Self {
        Self {
            ops: HashMap::new(),
        }
    }

    /// Registers a file path with `FileModel`, sets the initial version,
    /// and tracks the pending operation. Returns the `FileId` and
    /// `ContentVersion` for the caller to initiate the actual I/O.
    fn insert(
        &mut self,
        path: &Path,
        request_id: RequestId,
        conn_id: ConnectionId,
        kind: FileOpKind,
        ctx: &mut ModelContext<ServerModel>,
    ) -> (FileId, ContentVersion) {
        let file_model = FileModel::handle(ctx);
        let file_id = file_model.update(ctx, |m, ctx| m.register_file_path(path, false, ctx));
        let version = ContentVersion::new();
        file_model.update(ctx, |m, _| m.set_version(file_id, version));
        self.ops.insert(
            file_id,
            PendingFileOp {
                request_id,
                conn_id,
                kind,
            },
        );
        (file_id, version)
    }

    fn get(&self, file_id: &FileId) -> Option<&PendingFileOp> {
        self.ops.get(file_id)
    }

    /// Removes a pending operation and unsubscribes the file from `FileModel`,
    /// preventing the `FileState` entry from leaking.
    fn remove(
        &mut self,
        file_id: FileId,
        ctx: &mut ModelContext<ServerModel>,
    ) -> Option<PendingFileOp> {
        let op = self.ops.remove(&file_id)?;
        FileModel::handle(ctx).update(ctx, |m, ctx| m.unsubscribe(file_id, ctx));
        Some(op)
    }
}

// `libc::ioctl` 的 request ABI 取决于目标 libc：Linux musl 使用 c_int，
// Linux glibc 与 macOS 使用 c_ulong。remote helper 固定构建为 musl，不能用
// 本机 macOS 能编译的类型掩盖交叉编译错误。
#[cfg(all(target_os = "linux", target_env = "musl"))]
type IoctlRequest = libc::c_int;
#[cfg(all(unix, not(all(target_os = "linux", target_env = "musl"))))]
type IoctlRequest = libc::c_ulong;

#[cfg(unix)]
const TIOCSCTTY_REQUEST: IoctlRequest = libc::TIOCSCTTY as IoctlRequest;

#[cfg(unix)]
const TIOCSWINSZ_REQUEST: IoctlRequest = libc::TIOCSWINSZ as IoctlRequest;

#[cfg(unix)]
fn duplicate_pty_slave(slave_fd: std::os::fd::RawFd, label: &str) -> Result<std::fs::File, String> {
    let fd = unsafe { libc::dup(slave_fd) };
    if fd < 0 {
        return Err(format!(
            "failed to dup PTY slave for {label}: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(unsafe { std::fs::File::from_raw_fd(fd) })
}

#[cfg(unix)]
struct RemotePty {
    master: std::fs::File,
    child: std::process::Child,
}

#[cfg(unix)]
impl Drop for RemotePty {
    fn drop(&mut self) {
        if let Err(e) = self.child.kill() {
            log::debug!("Remote PTY child already exited or could not be killed: {e}");
        }
        let _ = self.child.wait();
    }
}

#[cfg(not(unix))]
struct RemotePty {}

#[cfg(all(feature = "local_fs", unix))]
enum FileTransferState {
    Read {
        file: std::fs::File,
        total_size: u64,
        offset: u64,
    },
    Write {
        file: std::fs::File,
        staging_name: std::ffi::CString,
        final_name: std::ffi::CString,
        parent: std::os::fd::OwnedFd,
        parent_path: PathBuf,
        parent_identity: DeleteDirectoryIdentity,
        final_path: PathBuf,
        executable: Option<bool>,
        offset: u64,
    },
}

#[cfg(all(feature = "local_fs", not(unix)))]
struct FileTransferState;

#[cfg(all(feature = "local_fs", unix))]
impl Drop for FileTransferState {
    fn drop(&mut self) {
        if let Self::Write {
            parent,
            staging_name,
            ..
        } = self
        {
            unsafe {
                libc::unlinkat(parent.as_raw_fd(), staging_name.as_ptr(), 0);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConnectionPhase {
    AwaitingInitialize,
    Ready,
}

struct ConnectionState {
    outbound_tx: async_channel::Sender<ServerMessage>,
    phase: ConnectionPhase,
    snapshot_sent_roots: HashSet<StandardizedPath>,
    executors: HashMap<SessionId, Arc<LocalCommandExecutor>>,
    in_progress: HashMap<RequestId, AbortHandle>,
    ptys: HashMap<u64, RemotePty>,
    #[cfg(feature = "local_fs")]
    file_transfers: HashMap<String, FileTransferState>,
}

impl ConnectionState {
    fn new(outbound_tx: async_channel::Sender<ServerMessage>) -> Self {
        Self {
            outbound_tx,
            phase: ConnectionPhase::AwaitingInitialize,
            snapshot_sent_roots: HashSet::new(),
            executors: HashMap::new(),
            in_progress: HashMap::new(),
            ptys: HashMap::new(),
            #[cfg(feature = "local_fs")]
            file_transfers: HashMap::new(),
        }
    }
}

impl Drop for ConnectionState {
    fn drop(&mut self) {
        for handle in self.in_progress.values() {
            handle.abort();
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConnectionMessageGateError {
    MissingConnection,
    InitializeRequired,
    AlreadyInitialized,
}

/// The top-level server-side orchestrator model.
///
/// Receives `ClientMessage`s from connected proxy sessions and routes
/// `ServerMessage` responses and push notifications back through each
/// connection's dedicated sender channel.
pub struct ServerModel {
    /// Complete per-connection lifecycle state keyed by `ConnectionId`.
    ///
    /// Sender, Initialize eligibility and snapshot-delivery state are created
    /// and removed together so no business request can outlive its handshake.
    connections: HashMap<ConnectionId, ConnectionState>,
    /// Abort handle for the active grace timer, if any.
    /// Calling `.abort()` cancels the timer before it fires.
    grace_timer_cancel: Option<SpawnedFutureHandle>,
    /// Stable host identifier generated once at process startup.
    /// Returned in every `InitializeResponse` so clients can deduplicate
    /// host-scoped models.
    host_id: String,
    next_pty_id: u64,
    /// Tracks in-flight file write/delete operations and handles cleanup.
    pending_file_ops: PendingFileOps,
    /// Tracks open server-managed current-app buffers, their connections, and pending
    /// buffer requests (OpenBuffer, SaveBuffer, ResolveConflict).
    #[cfg(feature = "local_fs")]
    buffers: ServerBufferTracker,
    /// Daemon-wide bearer credential for the identity-scoped daemon.
    ///
    /// The token is written by Initialize when the client supplies a
    /// non-empty credential, or by Authenticate during token rotation. It is
    /// intentionally retained across proxy connection teardown and cleared
    /// only by daemon process exit.
    auth_token: Option<String>,
}

#[cfg(feature = "local_fs")]
fn build_scan_cli_agent_sessions_response(
    result: Result<super::cli_agent_sessions::ScannedSessionDiscovery, String>,
) -> ScanCliAgentSessionsResponse {
    let result = match result {
        Ok(super::cli_agent_sessions::ScannedSessionDiscovery::Complete {
            observed_agents,
            sessions,
        }) => {
            let records = sessions
                .into_iter()
                .map(|session| CliAgentSessionRecord {
                    agent: session.agent.to_serialized_name(),
                    id: session.id,
                    source: session.source,
                    label: session.label,
                    cwd: session.cwd,
                    modified_epoch_millis: session.modified_epoch_millis,
                })
                .collect();
            scan_cli_agent_sessions_response::Result::Success(ScanCliAgentSessionsSuccess {
                records,
                observed_agents: observed_agents
                    .into_iter()
                    .map(|agent| agent.to_serialized_name())
                    .collect(),
                source_missing_agent: None,
            })
        }
        Ok(super::cli_agent_sessions::ScannedSessionDiscovery::SourceMissing { agent }) => {
            scan_cli_agent_sessions_response::Result::Success(ScanCliAgentSessionsSuccess {
                records: Vec::new(),
                observed_agents: Vec::new(),
                source_missing_agent: Some(agent.to_serialized_name()),
            })
        }
        Err(message) => {
            scan_cli_agent_sessions_response::Result::Error(FileOperationError { message })
        }
    };
    ScanCliAgentSessionsResponse {
        result: Some(result),
    }
}

#[cfg(feature = "local_fs")]
fn decode_scan_cli_agent_wire_agents(
    field: &str,
    names: impl IntoIterator<Item = String>,
) -> Result<Vec<crate::terminal::CLIAgent>, String> {
    names
        .into_iter()
        .enumerate()
        .map(|(index, name)| {
            let agent = crate::terminal::CLIAgent::from_serialized_name(&name);
            if matches!(agent, crate::terminal::CLIAgent::Unknown) {
                Err(format!(
                    "ScanCliAgentSessions {field}[{index}] is not a serialized CLI agent identity: {name:?}"
                ))
            } else {
                Ok(agent)
            }
        })
        .collect()
}

#[cfg(feature = "local_fs")]
fn cli_agent_store_roots_from_request(
    roots: Option<CliAgentSessionStoreRoots>,
) -> Result<crate::cli_agent_jsonl::CliAgentStoreRoots, String> {
    let roots =
        roots.ok_or_else(|| "CLI-agent request is missing target store roots".to_owned())?;
    crate::cli_agent_jsonl::CliAgentStoreRoots::from_explicit_target_paths(
        PathBuf::from(roots.home_dir),
        PathBuf::from(roots.claude_config_dir),
        PathBuf::from(roots.codex_home),
    )
}

impl Entity for ServerModel {
    type Event = ();
}

impl SingletonEntity for ServerModel {}

impl ServerModel {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        let host_id = uuid::Uuid::new_v4().to_string();
        log::info!(
            "Daemon started: PID={}, host_id={}",
            std::process::id(),
            host_id
        );
        let mut model = Self {
            connections: HashMap::new(),
            grace_timer_cancel: None,
            host_id,
            next_pty_id: 1,
            pending_file_ops: PendingFileOps::new(),
            #[cfg(feature = "local_fs")]
            buffers: ServerBufferTracker::new(),
            auth_token: None,
        };
        // Subscribe to FileModel and RepoMetadataModel events
        // file operation results and repo metadata pushes are forwarded to all
        // connected proxy sessions.
        {
            let file_model = FileModel::handle(ctx);
            ctx.subscribe_to_model(&file_model, |me, event, ctx| {
                let file_id = event.file_id();
                let Some(pending_kind) = me.pending_file_ops.get(&file_id).map(|op| &op.kind)
                else {
                    return; // Not a file op we're tracking.
                };
                let response_message = match (event, pending_kind) {
                    (FileModelEvent::FileSaved { .. }, FileOpKind::Write) => {
                        server_message::Message::WriteFileResponse(WriteFileResponse {
                            result: Some(write_file_response::Result::Success(WriteFileSuccess {})),
                        })
                    }
                    (FileModelEvent::FileSaved { .. }, FileOpKind::Delete) => {
                        server_message::Message::DeleteFileResponse(DeleteFileResponse {
                            result: Some(delete_file_response::Result::Success(
                                DeleteFileSuccess {},
                            )),
                        })
                    }
                    (FileModelEvent::FailedToSave { error, .. }, FileOpKind::Write) => {
                        server_message::Message::WriteFileResponse(WriteFileResponse {
                            result: Some(write_file_response::Result::Error(FileOperationError {
                                message: format!("{error}"),
                            })),
                        })
                    }
                    (FileModelEvent::FailedToSave { error, .. }, FileOpKind::Delete) => {
                        server_message::Message::DeleteFileResponse(DeleteFileResponse {
                            result: Some(delete_file_response::Result::Error(FileOperationError {
                                message: format!("{error}"),
                            })),
                        })
                    }
                    (FileModelEvent::FileLoaded { .. }, _)
                    | (FileModelEvent::FailedToLoad { .. }, _)
                    | (FileModelEvent::FileUpdated { .. }, _) => return,
                };
                // Remove the pending op and unsubscribe from FileModel.
                let pending = me
                    .pending_file_ops
                    .remove(file_id, ctx)
                    .expect("pending op was confirmed present");
                me.send_server_message(
                    Some(pending.conn_id),
                    Some(&pending.request_id),
                    response_message,
                );
            });
        }
        {
            let repo_model = RepoMetadataModel::handle(ctx);
            ctx.subscribe_to_model(&repo_model, |me, event, ctx| match event {
                RepoMetadataEvent::IncrementalUpdateReady { update } => {
                    me.send_server_message(
                        None,
                        None,
                        server_message::Message::RepoMetadataUpdate(update.into()),
                    );
                }
                RepoMetadataEvent::RepositoryUpdated {
                    id: RepositoryIdentifier::Local(path),
                } => {
                    // A repo finished indexing — push the full tree as a snapshot.
                    let id = RepositoryIdentifier::local(path.clone());
                    let repo_model = RepoMetadataModel::handle(ctx);
                    if let Some(state) = repo_model.as_ref(ctx).get_repository(&id, ctx) {
                        let entries = super::repo_metadata_proto::file_tree_entry_to_snapshot_proto(
                            &state.entry,
                        );
                        me.send_server_message(
                            None,
                            None,
                            server_message::Message::RepoMetadataSnapshot(
                                super::proto::RepoMetadataSnapshot {
                                    repo_path: path.to_string(),
                                    entries,
                                    sync_complete: true,
                                },
                            ),
                        );
                        // Mark this root as snapshot-sent for all active connections
                        // so subsequent NavigatedToDirectory calls skip re-sending.
                        for connection in me.connections.values_mut() {
                            if connection.phase == ConnectionPhase::Ready {
                                connection.snapshot_sent_roots.insert(path.clone());
                            }
                        }
                    }
                }
                RepoMetadataEvent::RepositoryRemoved { .. }
                | RepoMetadataEvent::FileTreeUpdated { .. }
                | RepoMetadataEvent::FileTreeEntryUpdated { .. }
                | RepoMetadataEvent::DirectoryLoadFinished { .. }
                | RepoMetadataEvent::UpdatingRepositoryFailed { .. }
                | RepoMetadataEvent::RepositoryUpdated {
                    id: RepositoryIdentifier::Remote(_),
                } => {}
            });
        }
        // Subscribe to GlobalBufferModel events for server-managed current-app buffers.
        #[cfg(feature = "local_fs")]
        {
            let gbm = GlobalBufferModel::handle(ctx);
            ctx.subscribe_to_model(&gbm, |me, event, ctx| match event {
                GlobalBufferModelEvent::BufferLoaded { file_id, .. } => {
                    // Complete all pending OpenBuffer requests for this file.
                    let pending = me
                        .buffers
                        .take_pending_open(file_id);
                    if !pending.is_empty() {
                        let gbm = GlobalBufferModel::handle(ctx);
                        let content = gbm.as_ref(ctx).content_for_file(*file_id, ctx);
                        let server_version = gbm
                            .as_ref(ctx)
                            .sync_clock_for_server_current_app(*file_id)
                            .map(|c| c.server_version.as_u64());

                        for (request_id, conn_id) in pending {
                            let message = match (&content, server_version) {
                                (Some(content), Some(sv)) => {
                                    server_message::Message::OpenBufferResponse(
                                        OpenBufferResponse {
                                            content: content.clone(),
                                            server_version: sv,
                                        },
                                    )
                                }
                                _ => server_message::Message::Error(ErrorResponse {
                                    code: ErrorCode::Internal.into(),
                                    message: format!(
                                        "Buffer loaded but content or sync clock unavailable for file {file_id:?}"
                                    ),
                                }),
                            };
                            me.send_server_message(Some(conn_id), Some(&request_id), message);
                        }
                    }
                }
                GlobalBufferModelEvent::ServerCurrentAppFileSystemBufferUpdated {
                    file_id,
                    edits,
                    new_server_version,
                    expected_client_version,
                } => {
                    // A canonical buffer has exactly one writable connection owner.
                    let Some(conn_id) = me.buffers.connection_for_buffer(file_id) else {
                        return;
                    };
                    // Find the path for this file_id; abort the push if tracker
                    // state is inconsistent (空 path 会破坏 path↔buffer 契约)。
                    let Some(path) = me.buffers.path_for_file_id(*file_id) else {
                        log::error!(
                            "Missing path mapping for server-managed current-app buffer file_id={file_id:?}"
                        );
                        return;
                    };

                    let proto_edits: Vec<TextEdit> = edits
                        .iter()
                        .map(|edit| TextEdit {
                            start_offset: edit.start.as_usize() as u64,
                            end_offset: edit.end.as_usize() as u64,
                            text: edit.text.clone(),
                        })
                        .collect();

                    me.send_server_message(
                        Some(conn_id),
                        None,
                        server_message::Message::BufferUpdated(BufferUpdatedPush {
                            path,
                            new_server_version: new_server_version.as_u64(),
                            expected_client_version: expected_client_version.as_u64(),
                            edits: proto_edits,
                        }),
                    );
                }
                GlobalBufferModelEvent::FileSaved { file_id } => {
                    me.complete_active_buffer_mutation(*file_id, Ok(()), ctx);
                }
                GlobalBufferModelEvent::FailedToSave { file_id, error } => {
                    me.complete_active_buffer_mutation(*file_id, Err(format!("{error}")), ctx);
                }
                GlobalBufferModelEvent::FailedToLoad { file_id, error } => {
                    me.fail_server_current_app_buffer(
                        *file_id,
                        format!("Failed to load buffer: {error}"),
                        ctx,
                    );
                }
                GlobalBufferModelEvent::BufferUpdatedFromFileEvent { .. }
                | GlobalBufferModelEvent::EnvironmentBufferConflict { .. } => {
                    // Not relevant for server-managed current-app buffers.
                }
            });
        }
        // Start the grace timer immediately so the daemon exits if no proxy
        // connects within GRACE_PERIOD. In practice the spawning proxy connects
        // within milliseconds, so the risk of premature shutdown is negligible;
        // register_connection will cancel the timer the moment the first proxy
        // arrives.
        model.start_grace_timer(ctx);
        model
    }

    /// Called when a proxy connects.  Inserts `conn_tx` into the connection
    /// map so `send_server_message` can route responses to this proxy, and
    /// cancels the grace timer if it was running.
    pub fn register_connection(
        &mut self,
        conn_id: ConnectionId,
        conn_tx: async_channel::Sender<ServerMessage>,
        ctx: &mut ModelContext<Self>,
    ) {
        log::info!(
            "Daemon: connection {conn_id} registered — {} active, host_id={}",
            self.connections.len() + 1,
            self.host_id
        );
        if let Some(handle) = self.grace_timer_cancel.take() {
            handle.abort();
        }
        self.connections
            .insert(conn_id, ConnectionState::new(conn_tx));
        ctx.notify();
    }

    fn remove_connection_state(&mut self, conn_id: ConnectionId) -> Option<ConnectionState> {
        self.connections.remove(&conn_id)
    }

    /// Called when a proxy disconnects.  Removes it from the connection map
    /// and starts the grace timer if no connections remain.
    pub fn deregister_connection(&mut self, conn_id: ConnectionId, ctx: &mut ModelContext<Self>) {
        // Guard against double-deregister (reader and writer tasks both call
        // this on connection close; the second call must be a safe no-op).
        let Some(connection) = self.remove_connection_state(conn_id) else {
            return;
        };
        let removed_executors = connection.executors.len();
        let aborted_requests = connection.in_progress.len();
        let removed_ptys = connection.ptys.len();
        drop(connection);
        // Drop this connection from all open server-managed current-app buffers; orphaned
        // buffers (no remaining connections) are deallocated by the tracker.
        #[cfg(feature = "local_fs")]
        self.buffers.remove_connection(conn_id, ctx);
        if removed_executors + aborted_requests + removed_ptys > 0 {
            log::info!(
                "Daemon: dropped connection-owned resources for {conn_id}: \
                 executors={removed_executors}, aborted_requests={aborted_requests}, \
                 ptys={removed_ptys}"
            );
        }
        let remaining = self.connections.len();
        log::info!("Daemon: connection {conn_id} deregistered — {remaining} active remaining");
        if remaining == 0 {
            log::info!("Daemon: grace timer started ({GRACE_PERIOD:?})");
            self.start_grace_timer(ctx);
        }
        ctx.notify();
    }

    /// Starts (or restarts) a timer that shuts the daemon down after
    /// [`GRACE_PERIOD`] with no connected proxies.  If a timer is already
    /// running its abort handle is cancelled before the new one is stored.
    /// When a proxy connects, `register_connection` aborts the handle,
    /// preventing the shutdown.
    fn start_grace_timer(&mut self, ctx: &mut ModelContext<Self>) {
        if let Some(handle) = self.grace_timer_cancel.take() {
            handle.abort();
        }
        let handle = ctx.spawn_abortable(
            async_io::Timer::after(GRACE_PERIOD),
            |_, _, ctx| {
                log::info!("Daemon: grace period expired, shutting down");
                ctx.terminate_app(TerminationMode::ForceTerminate, None);
            },
            |_, _| {
                log::debug!("Daemon: grace timer cancelled");
            },
        );
        self.grace_timer_cancel = Some(handle);
    }

    fn validate_connection_message(
        &self,
        conn_id: ConnectionId,
        message: &Option<client_message::Message>,
    ) -> Result<(), ConnectionMessageGateError> {
        let phase = self
            .connections
            .get(&conn_id)
            .map(|connection| connection.phase)
            .ok_or(ConnectionMessageGateError::MissingConnection)?;
        let is_initialize = matches!(message, Some(client_message::Message::Initialize(_)));
        match (phase, is_initialize) {
            (ConnectionPhase::AwaitingInitialize, true) | (ConnectionPhase::Ready, false) => Ok(()),
            (ConnectionPhase::AwaitingInitialize, false) => {
                Err(ConnectionMessageGateError::InitializeRequired)
            }
            (ConnectionPhase::Ready, true) => Err(ConnectionMessageGateError::AlreadyInitialized),
        }
    }

    fn reject_message_before_initialize(
        &self,
        conn_id: ConnectionId,
        request_id: &RequestId,
        error: ConnectionMessageGateError,
    ) {
        let message = match error {
            ConnectionMessageGateError::MissingConnection => {
                log::warn!("Ignoring message for unregistered connection {conn_id}");
                return;
            }
            ConnectionMessageGateError::InitializeRequired => {
                "remote helper connection must complete Initialize before business messages"
            }
            ConnectionMessageGateError::AlreadyInitialized => {
                "remote helper connection has already completed Initialize"
            }
        };
        self.send_server_message(
            Some(conn_id),
            Some(request_id),
            server_message::Message::Error(ErrorResponse {
                code: ErrorCode::InvalidRequest.into(),
                message: message.to_string(),
            }),
        );
    }

    /// Called by the background stdin reader task via `ModelSpawner`.
    ///
    /// Dispatches on the `oneof message` variant. Notifications are handled
    /// inline; request-style messages return a `HandlerOutcome` that is
    /// centrally acted on here: `Sync` responses are sent immediately and
    /// `Async` handles are tracked in `in_progress` so they can be aborted.
    pub fn handle_message(
        &mut self,
        conn_id: ConnectionId,
        msg: ClientMessage,
        ctx: &mut ModelContext<Self>,
    ) {
        let request_id = RequestId::from(msg.request_id);
        if let Err(error) = self.validate_connection_message(conn_id, &msg.message) {
            self.reject_message_before_initialize(conn_id, &request_id, error);
            return;
        }

        let outcome = match msg.message {
            Some(client_message::Message::Initialize(msg)) => {
                self.handle_initialize(conn_id, msg, &request_id)
            }
            Some(client_message::Message::Authenticate(msg)) => {
                self.handle_authenticate(msg);
                return;
            }
            Some(client_message::Message::SessionBootstrapped(msg)) => {
                self.handle_session_bootstrapped(conn_id, msg);
                return;
            }
            Some(client_message::Message::SessionExecutionContextDeregistered(msg)) => {
                self.handle_session_execution_context_deregistered(conn_id, msg);
                return;
            }
            Some(client_message::Message::Abort(abort)) => {
                self.handle_abort(conn_id, abort, &request_id);
                return;
            }
            Some(client_message::Message::RunCommand(req)) => {
                self.handle_run_command(req, &request_id, conn_id, ctx)
            }
            Some(client_message::Message::CreatePty(req)) => {
                self.handle_create_pty(req, &request_id, conn_id)
            }
            Some(client_message::Message::WritePty(req)) => {
                self.handle_write_pty(conn_id, req);
                return;
            }
            Some(client_message::Message::ResizePty(req)) => {
                self.handle_resize_pty(conn_id, req);
                return;
            }
            Some(client_message::Message::ClosePty(req)) => {
                self.handle_close_pty(conn_id, req);
                return;
            }
            Some(client_message::Message::NavigatedToDirectory(msg)) => {
                self.handle_navigated_to_directory(msg, &request_id, conn_id, ctx)
            }
            Some(client_message::Message::LoadRepoMetadataDirectory(msg)) => {
                self.handle_load_repo_metadata_directory(msg, &request_id, ctx)
            }
            Some(client_message::Message::WriteFile(msg)) => {
                self.handle_write_file(msg, &request_id, conn_id, ctx)
            }
            Some(client_message::Message::DeleteFile(msg)) => {
                self.handle_delete_file(msg, &request_id, conn_id, ctx)
            }
            Some(client_message::Message::ReadFileContext(msg)) => {
                self.handle_read_file_context(msg, &request_id, conn_id, ctx)
            }
            #[cfg(feature = "local_fs")]
            Some(client_message::Message::OpenBuffer(msg)) => {
                self.handle_open_buffer(msg, &request_id, conn_id, ctx)
            }
            #[cfg(feature = "local_fs")]
            Some(client_message::Message::BufferEdit(msg)) => {
                self.handle_buffer_edit(msg, conn_id, ctx);
                return; // fire-and-forget notification
            }
            #[cfg(feature = "local_fs")]
            Some(client_message::Message::CloseBuffer(msg)) => {
                self.handle_close_buffer(msg, conn_id, ctx);
                return; // fire-and-forget notification
            }
            #[cfg(feature = "local_fs")]
            Some(client_message::Message::SaveBuffer(msg)) => {
                self.handle_save_buffer(msg, &request_id, conn_id, ctx)
            }
            #[cfg(feature = "local_fs")]
            Some(client_message::Message::ResolveConflict(msg)) => {
                self.handle_resolve_conflict(msg, &request_id, conn_id, ctx)
            }
            // Ashide:远端终端文件链接的目录列举(校验路径形态用)。
            #[cfg(feature = "local_fs")]
            Some(client_message::Message::ListDirectory(msg)) => self.handle_list_directory(msg),
            #[cfg(feature = "local_fs")]
            Some(client_message::Message::ResolvePath(msg)) => self.handle_resolve_path(msg),
            #[cfg(feature = "local_fs")]
            Some(client_message::Message::CreateDirectory(msg)) => {
                self.handle_create_directory(msg)
            }
            #[cfg(feature = "local_fs")]
            Some(client_message::Message::DeleteDirectory(msg)) => {
                self.handle_delete_directory(msg)
            }
            #[cfg(feature = "local_fs")]
            Some(client_message::Message::ExactRename(msg)) => self.handle_exact_rename(msg),
            #[cfg(feature = "local_fs")]
            Some(client_message::Message::AppendFile(msg)) => self.handle_append_file(msg),
            #[cfg(feature = "local_fs")]
            Some(client_message::Message::BeginFileTransfer(msg)) => {
                self.handle_begin_file_transfer(msg, conn_id)
            }
            #[cfg(feature = "local_fs")]
            Some(client_message::Message::ReadFileChunk(msg)) => {
                self.handle_read_file_chunk(msg, conn_id)
            }
            #[cfg(feature = "local_fs")]
            Some(client_message::Message::WriteFileChunk(msg)) => {
                self.handle_write_file_chunk(msg, conn_id)
            }
            #[cfg(feature = "local_fs")]
            Some(client_message::Message::FinishFileTransfer(msg)) => {
                self.handle_finish_file_transfer(msg, conn_id)
            }
            #[cfg(feature = "local_fs")]
            Some(client_message::Message::AbortFileTransfer(msg)) => {
                self.handle_abort_file_transfer(msg, conn_id)
            }
            #[cfg(feature = "local_fs")]
            Some(client_message::Message::PromoteFiles(msg)) => self.handle_promote_files(msg),
            #[cfg(feature = "local_fs")]
            Some(client_message::Message::ScanCliAgentSessions(msg)) => {
                self.handle_scan_cli_agent_sessions(msg)
            }
            #[cfg(feature = "local_fs")]
            Some(client_message::Message::ReadCliAgentSession(msg)) => {
                self.handle_read_cli_agent_session(msg)
            }
            #[cfg(feature = "local_fs")]
            Some(client_message::Message::MutateCliAgentSession(msg)) => {
                self.handle_mutate_cli_agent_session(msg)
            }
            #[cfg(feature = "local_fs")]
            Some(client_message::Message::GetCliAgentSessionUserState(msg)) => {
                self.handle_get_cli_agent_session_user_state(msg)
            }
            #[cfg(feature = "local_fs")]
            Some(client_message::Message::MutateCliAgentSessionUserState(msg)) => {
                self.handle_mutate_cli_agent_session_user_state(msg)
            }
            #[cfg(not(feature = "local_fs"))]
            Some(
                client_message::Message::OpenBuffer(_)
                | client_message::Message::BufferEdit(_)
                | client_message::Message::CloseBuffer(_)
                | client_message::Message::SaveBuffer(_)
                | client_message::Message::ResolveConflict(_)
                | client_message::Message::ListDirectory(_)
                | client_message::Message::ResolvePath(_)
                | client_message::Message::CreateDirectory(_)
                | client_message::Message::ExactRename(_)
                | client_message::Message::AppendFile(_)
                | client_message::Message::BeginFileTransfer(_)
                | client_message::Message::ReadFileChunk(_)
                | client_message::Message::WriteFileChunk(_)
                | client_message::Message::FinishFileTransfer(_)
                | client_message::Message::AbortFileTransfer(_)
                | client_message::Message::PromoteFiles(_)
                | client_message::Message::ScanCliAgentSessions(_)
                | client_message::Message::ReadCliAgentSession(_)
                | client_message::Message::MutateCliAgentSession(_)
                | client_message::Message::GetCliAgentSessionUserState(_)
                | client_message::Message::MutateCliAgentSessionUserState(_),
            ) => HandlerOutcome::Sync(server_message::Message::Error(ErrorResponse {
                code: ErrorCode::InvalidRequest.into(),
                message: "Buffer syncing requires the local_fs feature".to_string(),
            })),
            None => {
                log::warn!(
                    "Received ClientMessage with no message variant (request_id={request_id})"
                );
                HandlerOutcome::Sync(server_message::Message::Error(ErrorResponse {
                    code: ErrorCode::InvalidRequest.into(),
                    message: "ClientMessage had no message variant set".to_string(),
                }))
            }
        };

        match outcome {
            HandlerOutcome::Sync(message) => {
                self.send_server_message(Some(conn_id), Some(&request_id), message);
            }
            HandlerOutcome::Async(Some(handle)) => {
                self.register_in_progress_request(conn_id, request_id, handle);
            }
            HandlerOutcome::Async(None) => {
                // Async work tracked elsewhere (e.g. `pending_file_ops`);
                // the response will be sent via an event subscription.
            }
        }
    }

    /// Routes a server message to its destination.
    ///
    /// - `conn_id = Some(id)` — sends only to the connection that originated
    ///   the request (used for all request/response pairs).
    /// - `conn_id = None` — broadcasts to every connected proxy (used for
    ///   server-initiated push notifications such as repo metadata updates).
    fn send_server_message(
        &self,
        conn_id: Option<ConnectionId>,
        request_id: Option<&RequestId>,
        message: server_message::Message,
    ) {
        let msg = ServerMessage {
            request_id: request_id.map(|id| id.clone().into()).unwrap_or_default(),
            message: Some(message),
        };
        if let Some(target) = conn_id {
            if let Some(conn_tx) = self
                .connections
                .get(&target)
                .map(|connection| &connection.outbound_tx)
            {
                if let Err(e) = conn_tx.try_send(msg) {
                    log::warn!("Daemon: failed to send to conn {target}: {e}");
                }
            } else {
                log::debug!("Daemon: no sender for conn {target} (already disconnected)");
            }
        } else {
            // Push notification — broadcast to all connections.
            for (id, connection) in &self.connections {
                if connection.phase != ConnectionPhase::Ready {
                    continue;
                }
                if let Err(e) = connection.outbound_tx.try_send(msg.clone()) {
                    log::warn!("Daemon: failed to send to conn {id}: {e}");
                }
            }
        }
    }

    /// Spawns an abortable future tied to `request_id` and wires up automatic
    /// removal from `in_progress` on completion or abort.
    ///
    /// The returned handle is intended to be returned from a handler as
    /// `HandlerOutcome::Async(Some(handle))`; the caller (`handle_message`)
    /// inserts it into `in_progress`.
    fn spawn_request_handler<S, F>(
        &mut self,
        conn_id: ConnectionId,
        request_id: RequestId,
        future: S,
        on_resolve: F,
        ctx: &mut ModelContext<Self>,
    ) -> SpawnedFutureHandle
    where
        S: Spawnable,
        <S as Future>::Output: SpawnableOutput,
        F: 'static + FnOnce(&mut Self, <S as Future>::Output, &mut ModelContext<Self>),
    {
        let resolve_id = request_id.clone();
        let abort_id = request_id;
        ctx.spawn_abortable(
            future,
            move |me, output, ctx| {
                me.remove_in_progress_request(conn_id, &resolve_id);
                on_resolve(me, output, ctx);
            },
            move |me, _ctx| {
                log::info!("Request cancelled (request_id={abort_id})");
                me.remove_in_progress_request(conn_id, &abort_id);
            },
        )
    }

    fn register_in_progress_request(
        &mut self,
        conn_id: ConnectionId,
        request_id: RequestId,
        handle: SpawnedFutureHandle,
    ) {
        let Some(connection) = self.connections.get_mut(&conn_id) else {
            handle.abort();
            return;
        };
        if connection.in_progress.contains_key(&request_id) {
            log::error!(
                "Refusing duplicate in-progress request id {request_id} for connection {conn_id}"
            );
            handle.abort();
            return;
        }
        connection
            .in_progress
            .insert(request_id, handle.abort_handle());
    }

    fn remove_in_progress_request(
        &mut self,
        conn_id: ConnectionId,
        request_id: &RequestId,
    ) -> bool {
        self.connections
            .get_mut(&conn_id)
            .and_then(|connection| connection.in_progress.remove(request_id))
            .is_some()
    }

    /// Handles `Initialize` by returning the server version and host id.
    ///
    /// `server_version` is the release tag the daemon was built from
    /// (`GIT_RELEASE_TAG`) or the empty string for `cargo run` / locally
    /// deployed builds. The client treats an empty version as "unknown" and
    /// skips strict version enforcement, which keeps the
    /// `script/deploy_remote_server` developer workflow functional.
    fn handle_initialize(
        &mut self,
        conn_id: ConnectionId,
        msg: Initialize,
        request_id: &RequestId,
    ) -> HandlerOutcome {
        log::info!("Handling Initialize (request_id={request_id})");
        let expected_revision = remote_server::REMOTE_SERVER_PROTOCOL_REVISION;
        if msg.protocol_revision != expected_revision {
            return HandlerOutcome::Sync(server_message::Message::Error(ErrorResponse {
                code: ErrorCode::InvalidRequest.into(),
                message: format!(
                    "remote helper protocol revision mismatch: expected {expected_revision}, received {}",
                    msg.protocol_revision
                ),
            }));
        }
        let Some(connection) = self.connections.get_mut(&conn_id) else {
            return HandlerOutcome::Sync(server_message::Message::Error(ErrorResponse {
                code: ErrorCode::InvalidRequest.into(),
                message: "Initialize received for an unregistered connection".to_string(),
            }));
        };
        if connection.phase != ConnectionPhase::AwaitingInitialize {
            return HandlerOutcome::Sync(server_message::Message::Error(ErrorResponse {
                code: ErrorCode::InvalidRequest.into(),
                message: "remote helper connection has already completed Initialize".to_string(),
            }));
        }
        connection.phase = ConnectionPhase::Ready;
        if !msg.auth_token.is_empty() {
            self.auth_token = Some(msg.auth_token);
        }
        let server_version = ChannelState::app_version().unwrap_or("").to_string();
        HandlerOutcome::Sync(server_message::Message::InitializeResponse(
            InitializeResponse {
                server_version,
                host_id: self.host_id.clone(),
                protocol_revision: remote_server::REMOTE_SERVER_PROTOCOL_REVISION,
            },
        ))
    }

    /// Handles `Authenticate` by replacing the daemon-wide credential.
    /// This is a notification — no response is sent.
    fn handle_authenticate(&mut self, msg: Authenticate) {
        if msg.auth_token.is_empty() {
            log::warn!("Received Authenticate notification with empty auth token; ignoring");
            return;
        }
        self.auth_token = Some(msg.auth_token);
    }

    pub fn auth_token(&self) -> Option<&str> {
        self.auth_token.as_deref()
    }

    /// Handles `Abort` by cancelling the in-progress request it targets.
    /// This is a notification — no response is sent.
    fn handle_abort(&mut self, conn_id: ConnectionId, abort: Abort, request_id: &RequestId) {
        let target_id = RequestId::from(abort.request_id_to_abort);
        let handle = self
            .connections
            .get_mut(&conn_id)
            .and_then(|connection| connection.in_progress.remove(&target_id));
        if let Some(handle) = handle {
            log::info!(
                "Aborting in-progress request (request_id={target_id}, \
                 abort_request_id={request_id})"
            );
            handle.abort();
        } else {
            log::info!(
                "Abort for unknown/completed request (request_id={target_id}, \
                 abort_request_id={request_id})"
            );
        }
    }

    /// Handles `SessionBootstrapped` by creating a `LocalCommandExecutor` for
    /// the session. This is a notification — no response is sent.
    fn handle_session_bootstrapped(&mut self, conn_id: ConnectionId, msg: SessionBootstrapped) {
        let session_id = SessionId::from(msg.session_id);
        log::info!(
            "Handling SessionBootstrapped: session_id={session_id:?}, \
             shell_type={:?}, shell_path={:?}",
            msg.shell_type,
            msg.shell_path,
        );

        let Some(shell_type) = ShellType::from_name(&msg.shell_type) else {
            log::error!(
                "Unknown shell_type {:?} in SessionBootstrapped for session {session_id:?}",
                msg.shell_type,
            );
            return;
        };

        if let Err(error) = validate_marked_target_session_snapshot(
            msg.shell_path.as_deref(),
            msg.working_directory.as_deref(),
            &msg.environment_variables,
        ) {
            log::error!("Rejecting SessionBootstrapped for session {session_id:?}: {error}");
            return;
        }
        let shell_path = msg.shell_path.map(PathBuf::from);
        let Some(connection) = self.connections.get_mut(&conn_id) else {
            log::error!("Connection disappeared before SessionBootstrapped for {session_id:?}");
            return;
        };
        let execution_context = LocalCommandExecutionContext {
            working_directory: msg.working_directory.map(PathBuf::from),
            environment_variables: msg.environment_variables,
            authoritative_environment_variable_names: AUTHORITATIVE_SESSION_ENVIRONMENT_VARIABLES
                .into_iter()
                .map(str::to_owned)
                .collect(),
        };
        let executor = Arc::new(LocalCommandExecutor::new(
            shell_path,
            shell_type,
            execution_context,
        ));
        if connection.executors.insert(session_id, executor).is_some() {
            log::warn!(
                "Overwriting existing executor for session {session_id:?} \
                 (re-SessionBootstrapped with shell_type={:?})",
                msg.shell_type,
            );
        }
    }

    fn handle_session_execution_context_deregistered(
        &mut self,
        conn_id: ConnectionId,
        msg: SessionExecutionContextDeregistered,
    ) {
        let session_id = SessionId::from(msg.session_id);
        let Some(connection) = self.connections.get_mut(&conn_id) else {
            log::warn!(
                "Ignoring execution-context teardown for {session_id:?} on missing connection {conn_id}"
            );
            return;
        };
        if connection.executors.remove(&session_id).is_none() {
            log::debug!(
                "Execution-context teardown for absent session {session_id:?} on connection {conn_id}"
            );
        }
    }

    fn create_pty_error(message: impl Into<String>) -> server_message::Message {
        server_message::Message::CreatePtyResponse(CreatePtyResponse {
            result: Some(create_pty_response::Result::Error(CreatePtyError {
                message: message.into(),
            })),
        })
    }

    /// Handles `CreatePty` by spawning a long-lived shell PTY in the daemon runtime.
    fn handle_create_pty(
        &mut self,
        req: CreatePty,
        request_id: &RequestId,
        conn_id: ConnectionId,
    ) -> HandlerOutcome {
        log::info!(
            "Handling CreatePty (request_id={request_id}): cwd={:?}, shell={:?}, rows={}, cols={}",
            req.working_directory,
            req.shell,
            req.rows,
            req.cols,
        );

        let pty_id = self.next_pty_id;
        self.next_pty_id = self.next_pty_id.saturating_add(1);

        match self.spawn_remote_pty(pty_id, req, conn_id) {
            Ok(shell_type) => HandlerOutcome::Sync(server_message::Message::CreatePtyResponse(
                CreatePtyResponse {
                    result: Some(create_pty_response::Result::Success(CreatePtySuccess {
                        pty_id,
                        shell_type,
                    })),
                },
            )),
            Err(message) => HandlerOutcome::Sync(Self::create_pty_error(message)),
        }
    }

    #[cfg(unix)]
    fn spawn_remote_pty(
        &mut self,
        pty_id: u64,
        req: CreatePty,
        conn_id: ConnectionId,
    ) -> Result<String, String> {
        let Some(conn_tx) = self
            .connections
            .get(&conn_id)
            .map(|connection| connection.outbound_tx.clone())
        else {
            return Err("connection disappeared before PTY could be registered".to_string());
        };

        let rows = if req.rows == 0 { 24 } else { req.rows };
        let cols = if req.cols == 0 { 80 } else { req.cols };
        let size = winsize {
            ws_row: rows as u16,
            ws_col: cols as u16,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let ends = openpty(Some(&size), None).map_err(|e| format!("openpty failed: {e}"))?;
        let master = unsafe { std::fs::File::from_raw_fd(ends.master) };
        let slave = unsafe { std::fs::File::from_raw_fd(ends.slave) };
        let slave_fd = slave.as_raw_fd();

        let user_defaults = remote_pty_user_defaults()?;
        let shell = if req.shell.is_empty() {
            user_defaults.shell.to_string_lossy().into_owned()
        } else {
            req.shell
        };
        let shell_type = ShellType::from_name(&shell)
            .map(|shell_type| format!("{shell_type:?}").to_lowercase())
            .or_else(|| {
                Path::new(&shell)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(ToOwned::to_owned)
            })
            .unwrap_or_else(|| "sh".to_string());

        let mut cmd = Command::new(&shell);
        if matches!(
            ShellType::from_name(&shell),
            Some(ShellType::Bash | ShellType::Zsh | ShellType::Fish)
        ) {
            cmd.arg("-l");
        }
        let working_directory = if req.working_directory.is_empty() {
            user_defaults.home
        } else {
            PathBuf::from(req.working_directory)
        };
        cmd.current_dir(&working_directory);
        for key in terminal_capability_environment_variables_to_remove() {
            cmd.env_remove(*key);
        }
        for (key, value) in req.environment_variables {
            cmd.env(key, value);
        }
        cmd.env("TERM", "xterm-256color");

        let stdin = duplicate_pty_slave(slave_fd, "stdin")?;
        let stdout = duplicate_pty_slave(slave_fd, "stdout")?;
        let stderr = duplicate_pty_slave(slave_fd, "stderr")?;
        cmd.stdin(Stdio::from(stdin));
        cmd.stdout(Stdio::from(stdout));
        cmd.stderr(Stdio::from(stderr));
        unsafe {
            cmd.pre_exec(move || {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::ioctl(slave_fd, TIOCSCTTY_REQUEST, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn PTY shell {shell:?}: {e}"))?;
        drop(slave);

        let mut reader = match master.try_clone() {
            Ok(reader) => reader,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("failed to clone PTY master: {e}"));
            }
        };
        thread::spawn(move || {
            let mut buf = [0_u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let msg = ServerMessage {
                            request_id: String::new(),
                            message: Some(server_message::Message::PtyOutput(PtyOutputPush {
                                pty_id,
                                bytes: buf[..n].to_vec(),
                            })),
                        };
                        if conn_tx.try_send(msg).is_err() {
                            break;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => {
                        log::debug!("Remote PTY read loop ended for {pty_id}: {e}");
                        break;
                    }
                }
            }
            let _ = conn_tx.try_send(ServerMessage {
                request_id: String::new(),
                message: Some(server_message::Message::PtyExited(PtyExitedPush {
                    pty_id,
                    exit_code: None,
                })),
            });
        });

        let Some(connection) = self.connections.get_mut(&conn_id) else {
            drop(RemotePty { master, child });
            return Err("connection disappeared before PTY could be registered".to_string());
        };
        connection.ptys.insert(pty_id, RemotePty { master, child });
        Ok(shell_type)
    }

    #[cfg(not(unix))]
    fn spawn_remote_pty(
        &mut self,
        _pty_id: u64,
        _req: CreatePty,
        _conn_id: ConnectionId,
    ) -> Result<String, String> {
        // Hard-cut: not a "yet" placeholder. A non-unix host can never be a
        // remote daemon target — the uname gate only accepts Linux/Darwin
        // (remote_server::setup::parse_uname_output), so this branch exists
        // solely to keep the app crate compiling on non-unix clients.
        Err("native remote PTY is unsupported on non-unix platforms".to_string())
    }

    fn handle_write_pty(&mut self, conn_id: ConnectionId, req: WritePty) {
        #[cfg(unix)]
        if let Some(pty) = self
            .connections
            .get_mut(&conn_id)
            .and_then(|connection| connection.ptys.get_mut(&req.pty_id))
        {
            if let Err(e) = pty.master.write_all(&req.bytes) {
                log::warn!("Failed to write to remote PTY {}: {e}", req.pty_id);
            }
        } else {
            log::warn!(
                "Ignoring WritePty for unknown PTY {} on connection {conn_id}",
                req.pty_id
            );
        }

        #[cfg(not(unix))]
        let _ = (conn_id, req);
    }

    fn handle_resize_pty(&mut self, conn_id: ConnectionId, req: ResizePty) {
        #[cfg(unix)]
        if let Some(pty) = self
            .connections
            .get(&conn_id)
            .and_then(|connection| connection.ptys.get(&req.pty_id))
        {
            let size = winsize {
                ws_row: req.rows as u16,
                ws_col: req.cols as u16,
                ws_xpixel: req.width_px as u16,
                ws_ypixel: req.height_px as u16,
            };
            let result = unsafe { libc::ioctl(pty.master.as_raw_fd(), TIOCSWINSZ_REQUEST, &size) };
            if result < 0 {
                log::warn!(
                    "Failed to resize remote PTY {}: {}",
                    req.pty_id,
                    std::io::Error::last_os_error()
                );
            }
        } else {
            log::warn!(
                "Ignoring ResizePty for unknown PTY {} on connection {conn_id}",
                req.pty_id
            );
        }

        #[cfg(not(unix))]
        let _ = (conn_id, req);
    }

    fn handle_close_pty(&mut self, conn_id: ConnectionId, req: ClosePty) {
        let removed = self
            .connections
            .get_mut(&conn_id)
            .and_then(|connection| connection.ptys.remove(&req.pty_id));
        if removed.is_none() {
            log::debug!(
                "Ignoring ClosePty for unknown PTY {} on connection {conn_id}",
                req.pty_id
            );
        }
    }

    fn session_executor_for_connection(
        &self,
        session_id: SessionId,
        conn_id: ConnectionId,
    ) -> Option<Arc<LocalCommandExecutor>> {
        self.connections
            .get(&conn_id)
            .and_then(|connection| connection.executors.get(&session_id))
            .map(Arc::clone)
    }

    /// Handles `RunCommand` by delegating to the session's `LocalCommandExecutor`.
    ///
    /// On success, returns a `HandlerOutcome::Async` whose task resolves the
    /// request with a `RunCommandResponse`. On validation failure (missing
    /// executor), returns a `HandlerOutcome::Sync` error response.
    fn handle_run_command(
        &mut self,
        req: RunCommandRequest,
        request_id: &RequestId,
        conn_id: ConnectionId,
        ctx: &mut ModelContext<Self>,
    ) -> HandlerOutcome {
        let session_id = SessionId::from(req.session_id);
        log::info!(
            "Handling RunCommand (request_id={request_id}, session_id={session_id:?}): \
             command={:?}, cwd={:?}",
            req.command,
            req.working_directory,
        );

        let command = req.command;
        let cwd = req.working_directory;
        let env_vars = if req.environment_variables.is_empty() {
            None
        } else {
            Some(req.environment_variables)
        };

        let Some(executor) = self.session_executor_for_connection(session_id, conn_id) else {
            log::error!("No executor for session {session_id:?}, session was never initialized");
            return HandlerOutcome::Sync(server_message::Message::RunCommandResponse(
                RunCommandResponse {
                    result: Some(run_command_response::Result::Error(RunCommandError {
                        code: RunCommandErrorCode::SessionNotFound.into(),
                        message: format!("No executor for session {session_id:?}"),
                    })),
                },
            ));
        };

        // Call `execute_local_command` directly because the
        // `CommandExecutor::execute_command` trait method requires
        // a `&Shell` (version, options, plugins from bootstrap).
        let request_id_for_response = request_id.clone();
        let conn_id_for_response = conn_id;
        let handle = self.spawn_request_handler(
            conn_id,
            request_id.clone(),
            async move {
                executor
                    .execute_local_command(
                        &command,
                        cwd.as_deref(),
                        env_vars,
                        ExecuteCommandOptions::default(),
                    )
                    .await
            },
            move |me, result, _ctx| {
                let result_oneof = match result {
                    Ok(output) => {
                        log::info!(
                            "RunCommand completed (request_id={request_id_for_response}): \
                             exit_code={:?}, stdout_len={}, stderr_len={}",
                            output.exit_code,
                            output.stdout.len(),
                            output.stderr.len(),
                        );
                        run_command_response::Result::Success(RunCommandSuccess {
                            stdout: output.stdout.clone(),
                            stderr: output.stderr.clone(),
                            exit_code: output.exit_code.map(|c| c.value()),
                        })
                    }
                    Err(e) => {
                        log::warn!("RunCommand failed (request_id={request_id_for_response}): {e}");
                        run_command_response::Result::Error(RunCommandError {
                            code: RunCommandErrorCode::ExecutionFailed.into(),
                            message: format!("Failed to execute command: {e}"),
                        })
                    }
                };
                me.send_server_message(
                    Some(conn_id_for_response),
                    Some(&request_id_for_response),
                    server_message::Message::RunCommandResponse(RunCommandResponse {
                        result: Some(result_oneof),
                    }),
                );
            },
            ctx,
        );
        HandlerOutcome::Async(Some(handle))
    }

    /// Handles `NavigatedToDirectory` by running git detection first, then
    /// responding. On validation failure returns a `HandlerOutcome::Sync` error;
    /// otherwise spawns a task and returns a `HandlerOutcome::Async(Some(_))`
    /// handle.
    fn handle_navigated_to_directory(
        &mut self,
        msg: NavigatedToDirectory,
        request_id: &RequestId,
        conn_id: ConnectionId,
        ctx: &mut ModelContext<Self>,
    ) -> HandlerOutcome {
        log::info!(
            "Handling NavigatedToDirectory path={} (request_id={request_id})",
            msg.path
        );

        let std_path = match StandardizedPath::from_local_canonicalized(Path::new(&msg.path)) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("Invalid path for NavigatedToDirectory: {e}");
                return HandlerOutcome::Sync(server_message::Message::Error(ErrorResponse {
                    code: ErrorCode::InvalidRequest.into(),
                    message: format!("Invalid path: {e}"),
                }));
            }
        };

        // Kick off git detection. The returned future resolves with the git
        // root path (Some) or None if no git repo was found.
        let path_str = msg.path.clone();
        let git_future = DetectedRepositories::handle(ctx).update(ctx, |repos, ctx| {
            repos.detect_possible_git_repo(&path_str, RepoDetectionSource::TerminalNavigation, ctx)
        });

        let request_id_for_response = request_id.clone();
        let conn_id_for_response = conn_id;
        let handle = self.spawn_request_handler(
            conn_id,
            request_id.clone(),
            git_future,
            move |me, git_root, ctx| {
                let (indexed_path, is_git) = if let Some(root) = git_root {
                    // Git repo found. Full indexing was already triggered by
                    // DetectedGitRepo → CurrentAppRepoMetadataModel. The client
                    // waits for RepositoryIndexedPush before FetchFileTree.
                    let root_str = root.to_string_lossy().to_string();
                    log::info!("Git repo detected at {root_str} for path {}", std_path);
                    (root_str, true)
                } else {
                    // No git repo. Lazy-load the directory for first-level data,
                    // then push the snapshot immediately.
                    RepoMetadataModel::handle(ctx).update(ctx, |repo_model, ctx| {
                        if let Err(e) = repo_model.index_lazy_loaded_path(&std_path, ctx) {
                            log::warn!("Failed to lazy-load directory {std_path}: {e}");
                        }
                    });
                    (std_path.to_string(), false)
                };

                me.send_server_message(
                    Some(conn_id_for_response),
                    Some(&request_id_for_response),
                    server_message::Message::NavigatedToDirectoryResponse(
                        NavigatedToDirectoryResponse {
                            indexed_path: indexed_path.clone(),
                            is_git,
                        },
                    ),
                );

                // After responding, push a snapshot if metadata is available.
                //
                // For git repos this is an opportunistic push for the case
                // where the repo was already indexed and RepositoryUpdated
                // won't fire again (which would otherwise leave the client
                // with only a placeholder root). We skip if a snapshot was
                // already sent for this connection+root.
                //
                // For non-git directories the lazy-loaded tree is always
                // broadcast to all connections.
                if let Ok(root_path) =
                    StandardizedPath::from_local_canonicalized(Path::new(&indexed_path))
                {
                    if is_git {
                        let already_sent =
                            me.connections
                                .get(&conn_id_for_response)
                                .is_some_and(|connection| {
                                    connection.snapshot_sent_roots.contains(&root_path)
                                });
                        if already_sent {
                            log::debug!(
                                "Snapshot already sent for repo {indexed_path} \
                                 to conn {conn_id_for_response}, skipping"
                            );
                            return;
                        }
                    }

                    let id = RepositoryIdentifier::local(root_path.clone());
                    let repo_model = RepoMetadataModel::handle(ctx);
                    if let Some(state) = repo_model.as_ref(ctx).get_repository(&id, ctx) {
                        let entries = super::repo_metadata_proto::file_tree_entry_to_snapshot_proto(
                            &state.entry,
                        );
                        // Git snapshots target the requesting connection;
                        // non-git snapshots broadcast to all.
                        let target = if is_git {
                            Some(conn_id_for_response)
                        } else {
                            None
                        };
                        me.send_server_message(
                            target,
                            None,
                            server_message::Message::RepoMetadataSnapshot(
                                super::proto::RepoMetadataSnapshot {
                                    repo_path: indexed_path,
                                    entries,
                                    sync_complete: true,
                                },
                            ),
                        );
                        if is_git {
                            if let Some(sent_roots) = me
                                .connections
                                .get_mut(&conn_id_for_response)
                                .map(|connection| &mut connection.snapshot_sent_roots)
                            {
                                sent_roots.insert(root_path);
                            }
                        }
                    }
                }
            },
            ctx,
        );
        HandlerOutcome::Async(Some(handle))
    }

    /// Handles `LoadRepoMetadataDirectory` by loading a subdirectory on the
    /// server's local model and returning the children synchronously.
    fn handle_load_repo_metadata_directory(
        &mut self,
        msg: super::proto::LoadRepoMetadataDirectory,
        request_id: &RequestId,
        ctx: &mut ModelContext<Self>,
    ) -> HandlerOutcome {
        log::info!(
            "Handling LoadRepoMetadataDirectory repo_path={} dir_path={} (request_id={request_id})",
            msg.repo_path,
            msg.dir_path
        );

        let (repo_path, dir_path) =
            match validate_repo_metadata_directory_load_paths(&msg.repo_path, &msg.dir_path) {
                Ok(paths) => paths,
                Err(message) => {
                    return HandlerOutcome::Sync(server_message::Message::Error(ErrorResponse {
                        code: ErrorCode::InvalidRequest.into(),
                        message,
                    }));
                }
            };

        // Load the directory on the server's local model.
        let load_result = RepoMetadataModel::handle(ctx).update(ctx, |model, ctx| {
            model.load_directory(&repo_path, &dir_path, ctx)
        });

        if let Err(e) = load_result {
            log::warn!("LoadRepoMetadataDirectory failed: {e}");
            return HandlerOutcome::Sync(server_message::Message::Error(ErrorResponse {
                code: ErrorCode::Internal.into(),
                message: format!("Failed to load directory: {e}"),
            }));
        }

        // Read back the loaded children and serialize them.
        let id = RepositoryIdentifier::local(repo_path.clone());
        let entries = RepoMetadataModel::handle(ctx)
            .as_ref(ctx)
            .get_repository(&id, ctx)
            .map(|state| {
                super::repo_metadata_proto::file_tree_children_to_proto_entries(
                    &state.entry,
                    &dir_path,
                )
            })
            .unwrap_or_default();

        HandlerOutcome::Sync(server_message::Message::LoadRepoMetadataDirectoryResponse(
            super::proto::LoadRepoMetadataDirectoryResponse {
                repo_path: msg.repo_path,
                dir_path: msg.dir_path,
                entries,
            },
        ))
    }

    /// Handles `WriteFile` by registering the path and triggering an async
    /// write via `FileModel`. On a successful dispatch, returns
    /// `HandlerOutcome::Async(None)` — the response is sent later by the
    /// `FileModel` event subscription, and the op is not cancellable via
    /// `Abort`. On failure to dispatch, returns a `HandlerOutcome::Sync`
    /// error response.
    fn handle_write_file(
        &mut self,
        msg: WriteFile,
        request_id: &RequestId,
        conn_id: ConnectionId,
        ctx: &mut ModelContext<Self>,
    ) -> HandlerOutcome {
        log::info!(
            "Handling WriteFile path={} (request_id={request_id})",
            msg.path
        );
        let path = Path::new(&msg.path);

        let (file_id, version) =
            self.pending_file_ops
                .insert(path, request_id.clone(), conn_id, FileOpKind::Write, ctx);

        let file_model = FileModel::handle(ctx);
        if let Err(err) =
            file_model.update(ctx, |m, ctx| m.save(file_id, msg.content, version, ctx))
        {
            self.pending_file_ops.remove(file_id, ctx);
            return HandlerOutcome::Sync(server_message::Message::WriteFileResponse(
                WriteFileResponse {
                    result: Some(write_file_response::Result::Error(FileOperationError {
                        message: format!("Failed to initiate write: {err}"),
                    })),
                },
            ));
        }

        // Response sent asynchronously via the event subscription.
        HandlerOutcome::Async(None)
    }

    /// Handles `DeleteFile` by registering the path and triggering an async
    /// delete via `FileModel`. On a successful dispatch, returns
    /// `HandlerOutcome::Async(None)` — the response is sent later by the
    /// `FileModel` event subscription, and the op is not cancellable via
    /// `Abort`. On failure to dispatch, returns a `HandlerOutcome::Sync`
    /// error response.
    fn handle_delete_file(
        &mut self,
        msg: DeleteFile,
        request_id: &RequestId,
        conn_id: ConnectionId,
        ctx: &mut ModelContext<Self>,
    ) -> HandlerOutcome {
        log::info!(
            "Handling DeleteFile path={} (request_id={request_id})",
            msg.path
        );
        let path = Path::new(&msg.path);

        let (file_id, version) = self.pending_file_ops.insert(
            path,
            request_id.clone(),
            conn_id,
            FileOpKind::Delete,
            ctx,
        );

        let file_model = FileModel::handle(ctx);
        if let Err(err) = file_model.update(ctx, |m, ctx| m.delete(file_id, version, ctx)) {
            self.pending_file_ops.remove(file_id, ctx);
            return HandlerOutcome::Sync(server_message::Message::DeleteFileResponse(
                DeleteFileResponse {
                    result: Some(delete_file_response::Result::Error(FileOperationError {
                        message: format!("Failed to initiate delete: {err}"),
                    })),
                },
            ));
        }

        // Response sent asynchronously via the event subscription.
        HandlerOutcome::Async(None)
    }

    /// Handles `ReadFileContext` by spawning an async batch file read on the
    /// background executor. Returns `HandlerOutcome::Async` with the spawned
    /// handle so the request can be cancelled via `Abort`.
    fn handle_read_file_context(
        &mut self,
        msg: super::proto::ReadFileContextRequest,
        request_id: &RequestId,
        conn_id: ConnectionId,
        ctx: &mut ModelContext<Self>,
    ) -> HandlerOutcome {
        log::info!(
            "Handling ReadFileContext ({} files, request_id={request_id})",
            msg.files.len()
        );

        let max_file_bytes = msg.max_file_bytes.map(|b| b as usize);
        let max_batch_bytes = msg.max_batch_bytes.map(|b| b as usize);
        let file_locations: Vec<FileLocations> = msg
            .files
            .into_iter()
            .map(|f| FileLocations {
                name: f.path,
                lines: f
                    .line_ranges
                    .into_iter()
                    .map(|r| r.start as usize..r.end as usize)
                    .collect(),
            })
            .collect();
        let request_id_for_response = request_id.clone();

        let handle = self.spawn_request_handler(
            conn_id,
            request_id.clone(),
            async move {
                read_current_app_file_context(
                    &file_locations,
                    None,
                    None,
                    max_file_bytes,
                    max_batch_bytes,
                )
                .await
            },
            move |me, result: anyhow::Result<ReadFileContextResult>, _ctx| {
                let response = match result {
                    Ok(result) => file_context_result_to_proto(result),
                    Err(err) => ReadFileContextResponse {
                        file_contexts: vec![],
                        failed_files: vec![FailedFileRead {
                            path: String::new(),
                            error: Some(FileOperationError {
                                message: format!("{err:#}"),
                            }),
                        }],
                    },
                };
                me.send_server_message(
                    Some(conn_id),
                    Some(&request_id_for_response),
                    server_message::Message::ReadFileContextResponse(response),
                );
            },
            ctx,
        );

        HandlerOutcome::Async(Some(handle))
    }

    /// Handles `OpenBuffer` by opening the file via `GlobalBufferModel`.
    /// The response is sent asynchronously when `BufferLoaded` fires.
    #[cfg(feature = "local_fs")]
    fn fail_server_current_app_buffer(
        &mut self,
        file_id: FileId,
        message: String,
        ctx: &mut ModelContext<Self>,
    ) {
        let (pending_opens, pending_mutations) = self.buffers.fail_open_buffer(&file_id);
        GlobalBufferModel::handle(ctx).update(ctx, |gbm, ctx| gbm.remove(file_id, ctx));
        for (request_id, conn_id) in pending_opens {
            self.send_server_message(
                Some(conn_id),
                Some(&request_id),
                server_message::Message::Error(ErrorResponse {
                    code: ErrorCode::Internal.into(),
                    message: message.clone(),
                }),
            );
        }
        for mutation in pending_mutations {
            self.send_buffer_mutation_response(mutation, Err(message.clone()));
        }
    }

    #[cfg(feature = "local_fs")]
    fn handle_open_buffer(
        &mut self,
        msg: OpenBuffer,
        request_id: &RequestId,
        conn_id: ConnectionId,
        ctx: &mut ModelContext<Self>,
    ) -> HandlerOutcome {
        log::info!(
            "Handling OpenBuffer path={path} (request_id={request_id})",
            path = msg.path
        );

        let gbm = GlobalBufferModel::handle(ctx);
        let file_id = if let Some(file_id) = self.buffers.file_id_for_path(&msg.path) {
            if !self.buffers.add_connection(file_id, conn_id) {
                return HandlerOutcome::Sync(server_message::Message::Error(ErrorResponse {
                    code: ErrorCode::InvalidRequest.into(),
                    message: format!(
                        "Buffer already has a writable owner: {path}",
                        path = msg.path
                    ),
                }));
            }
            file_id
        } else {
            let path = PathBuf::from(&msg.path);
            let buffer_state = gbm.update(ctx, |gbm, ctx| gbm.open_server_current_app(path, ctx));
            let file_id = buffer_state.file_id;

            // Track path → FileId mapping and hold the daemon-side strong buffer reference.
            self.buffers
                .track_open_buffer(msg.path.clone(), file_id, buffer_state.buffer);
            assert!(
                self.buffers.add_connection(file_id, conn_id),
                "new canonical buffer must not already have a writer"
            );
            file_id
        };

        // If already loaded, respond immediately.
        if gbm.as_ref(ctx).buffer_loaded(file_id) {
            let content = gbm.as_ref(ctx).content_for_file(file_id, ctx);
            let server_version = gbm
                .as_ref(ctx)
                .sync_clock_for_server_current_app(file_id)
                .map(|clock| clock.server_version.as_u64());
            let (Some(content), Some(server_version)) = (content, server_version) else {
                let message = format!(
                    "Loaded buffer is missing canonical content or sync clock: {path}",
                    path = msg.path
                );
                self.fail_server_current_app_buffer(file_id, message.clone(), ctx);
                return HandlerOutcome::Sync(server_message::Message::Error(ErrorResponse {
                    code: ErrorCode::Internal.into(),
                    message,
                }));
            };
            return HandlerOutcome::Sync(server_message::Message::OpenBufferResponse(
                OpenBufferResponse {
                    content,
                    server_version,
                },
            ));
        }

        // Not yet loaded — stash request info so the GlobalBufferModelEvent
        // subscription can send the response when content arrives.
        self.buffers
            .insert_pending_open(file_id, request_id.clone(), conn_id);
        HandlerOutcome::Async(None)
    }

    /// Handles `BufferEdit` notification (fire-and-forget).
    /// Delegates to `GlobalBufferModel::apply_client_edit`. On rejection
    /// (stale server version), the edit is silently dropped.
    #[cfg(feature = "local_fs")]
    fn handle_buffer_edit(
        &mut self,
        msg: BufferEdit,
        conn_id: ConnectionId,
        ctx: &mut ModelContext<Self>,
    ) {
        let file_id = match self.buffers.require_writer(&msg.path, conn_id) {
            Ok(file_id) => file_id,
            Err(BufferWriterAccessError::NotOpen) => {
                log::warn!("BufferEdit for unknown buffer: {path}", path = msg.path);
                return;
            }
            Err(BufferWriterAccessError::NotOwner) => {
                log::warn!(
                    "Ignoring BufferEdit from non-owner connection {conn_id:?} for path {path}",
                    path = msg.path
                );
                return;
            }
        };

        let expected_sv = ContentVersion::from_wire_u64(msg.expected_server_version);
        let new_cv = ContentVersion::from_wire_u64(msg.new_client_version);

        // Per spec: if the edit is rejected (stale server version),
        // the server silently drops it.
        let edits = msg
            .edits
            .into_iter()
            .map(|edit| CharOffsetEdit {
                start: CharOffset::from(usize::try_from(edit.start_offset).unwrap_or(usize::MAX)),
                end: CharOffset::from(usize::try_from(edit.end_offset).unwrap_or(usize::MAX)),
                text: edit.text,
            })
            .collect::<Vec<_>>();
        GlobalBufferModel::handle(ctx).update(ctx, |gbm, ctx| {
            gbm.apply_client_edit(file_id, &edits, expected_sv, new_cv, ctx);
        });
    }

    #[cfg(feature = "local_fs")]
    fn start_active_buffer_mutation(&mut self, file_id: FileId, ctx: &mut ModelContext<Self>) {
        loop {
            let Some(mutation) = self.buffers.active_mutation(&file_id).cloned() else {
                self.buffers.cleanup_orphaned_if_idle(file_id, ctx);
                return;
            };
            let result = match &mutation.kind {
                PendingBufferMutationKind::SaveBuffer => GlobalBufferModel::handle(ctx)
                    .update(ctx, |gbm, ctx| gbm.save_server_current_app(file_id, ctx)),
                PendingBufferMutationKind::ResolveConflict {
                    acknowledged_server_version,
                    current_client_version,
                    client_content,
                } => GlobalBufferModel::handle(ctx).update(ctx, |gbm, ctx| {
                    gbm.resolve_conflict(
                        file_id,
                        *acknowledged_server_version,
                        *current_client_version,
                        client_content,
                        ctx,
                    )
                }),
            };
            match result {
                Ok(()) => return,
                Err(error) => {
                    let failed = self
                        .buffers
                        .complete_active_mutation(&file_id)
                        .expect("active mutation was confirmed present");
                    self.send_buffer_mutation_response(
                        failed,
                        Err(format!("Failed to initiate buffer mutation: {error}")),
                    );
                }
            }
        }
    }

    #[cfg(feature = "local_fs")]
    fn complete_active_buffer_mutation(
        &mut self,
        file_id: FileId,
        result: Result<(), String>,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(completed) = self.buffers.complete_active_mutation(&file_id) else {
            return;
        };
        self.send_buffer_mutation_response(completed, result);
        self.start_active_buffer_mutation(file_id, ctx);
    }

    #[cfg(feature = "local_fs")]
    fn send_buffer_mutation_response(
        &self,
        mutation: PendingBufferMutation,
        result: Result<(), String>,
    ) {
        let message = match mutation.kind {
            PendingBufferMutationKind::SaveBuffer => {
                let result = match result {
                    Ok(()) => save_buffer_response::Result::Success(SaveBufferSuccess {}),
                    Err(message) => {
                        save_buffer_response::Result::Error(FileOperationError { message })
                    }
                };
                server_message::Message::SaveBufferResponse(SaveBufferResponse {
                    result: Some(result),
                })
            }
            PendingBufferMutationKind::ResolveConflict { .. } => {
                let result = match result {
                    Ok(()) => resolve_conflict_response::Result::Success(ResolveConflictSuccess {}),
                    Err(message) => {
                        resolve_conflict_response::Result::Error(FileOperationError { message })
                    }
                };
                server_message::Message::ResolveConflictResponse(ResolveConflictResponse {
                    result: Some(result),
                })
            }
        };
        self.send_server_message(Some(mutation.conn_id), Some(&mutation.request_id), message);
    }

    /// Handles `SaveBuffer` by persisting the buffer to disk.
    #[cfg(feature = "local_fs")]
    fn handle_save_buffer(
        &mut self,
        msg: SaveBuffer,
        request_id: &RequestId,
        conn_id: ConnectionId,
        ctx: &mut ModelContext<Self>,
    ) -> HandlerOutcome {
        log::info!(
            "Handling SaveBuffer path={path} (request_id={request_id})",
            path = msg.path
        );

        let file_id = match self.buffers.require_writer(&msg.path, conn_id) {
            Ok(file_id) => file_id,
            Err(error) => {
                return HandlerOutcome::Sync(server_message::Message::SaveBufferResponse(
                    SaveBufferResponse {
                        result: Some(save_buffer_response::Result::Error(FileOperationError {
                            message: error.message(&msg.path),
                        })),
                    },
                ));
            }
        };

        let should_start = self.buffers.enqueue_mutation(
            file_id,
            PendingBufferMutation {
                request_id: request_id.clone(),
                conn_id,
                kind: PendingBufferMutationKind::SaveBuffer,
            },
        );
        if should_start {
            self.start_active_buffer_mutation(file_id, ctx);
        }
        HandlerOutcome::Async(None)
    }

    /// Handles `ResolveConflict` by replacing the server buffer with the
    /// client's content and persisting to disk. Returns an async
    /// `HandlerOutcome` — the response is sent when `FileSaved` or
    /// `FailedToSave` fires.
    #[cfg(feature = "local_fs")]
    fn handle_resolve_conflict(
        &mut self,
        msg: ResolveConflict,
        request_id: &RequestId,
        conn_id: ConnectionId,
        ctx: &mut ModelContext<Self>,
    ) -> HandlerOutcome {
        log::info!(
            "Handling ResolveConflict path={path} (request_id={request_id})",
            path = msg.path
        );

        let file_id = match self.buffers.require_writer(&msg.path, conn_id) {
            Ok(file_id) => file_id,
            Err(error) => {
                return HandlerOutcome::Sync(server_message::Message::ResolveConflictResponse(
                    ResolveConflictResponse {
                        result: Some(resolve_conflict_response::Result::Error(
                            FileOperationError {
                                message: error.message(&msg.path),
                            },
                        )),
                    },
                ));
            }
        };

        let should_start = self.buffers.enqueue_mutation(
            file_id,
            PendingBufferMutation {
                request_id: request_id.clone(),
                conn_id,
                kind: PendingBufferMutationKind::ResolveConflict {
                    acknowledged_server_version: ContentVersion::from_wire_u64(
                        msg.acknowledged_server_version,
                    ),
                    current_client_version: ContentVersion::from_wire_u64(
                        msg.current_client_version,
                    ),
                    client_content: msg.client_content,
                },
            },
        );
        if should_start {
            self.start_active_buffer_mutation(file_id, ctx);
        }
        HandlerOutcome::Async(None)
    }

    /// Ashide:处理 `ListDirectory` —— 同步列举一个目录下的直接子项。
    ///
    /// 给远端终端文件链接检测做精确校验用:客户端缓存某个 cwd 下的
    /// 真实目录项,链接检测器据此从 `ls -l` 整行里切出正确的文件名。
    /// `std::fs::read_dir` 在 daemon 端是廉价的同步调用,故直接返回
    /// `HandlerOutcome::Sync`,不走异步 spawn。
    #[cfg(feature = "local_fs")]
    fn handle_list_directory(&self, msg: ListDirectory) -> HandlerOutcome {
        log::info!("Handling ListDirectory path={}", msg.path);

        let path = expand_user_path(&msg.path);
        let result = match std::fs::read_dir(&path).and_then(collect_complete_directory_listing) {
            Ok(entries) => {
                let request_path = path.to_string_lossy().to_string();
                list_directory_response::Result::Success(ListDirectorySuccess {
                    entries,
                    path: request_path,
                })
            }
            Err(err) => list_directory_response::Result::Error(FileOperationError {
                message: format!("Failed to list directory {}: {err}", msg.path),
            }),
        };

        HandlerOutcome::Sync(server_message::Message::ListDirectoryResponse(
            ListDirectoryResponse {
                result: Some(result),
            },
        ))
    }

    #[cfg(feature = "local_fs")]
    fn handle_resolve_path(&self, msg: ResolvePath) -> HandlerOutcome {
        let path = expand_user_path(&msg.path);
        let result = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => {
                let file_type = metadata.file_type();
                let kind = entry_kind(Some(&file_type), Some(&metadata));
                let target_kind = entry_target_kind(Some(&file_type), path.as_path());
                let request_path = path.to_string_lossy().to_string();
                let resolved_path = path
                    .canonicalize()
                    .ok()
                    .map(|path| path.to_string_lossy().to_string());
                resolve_path_response::Result::Success(ResolvePathSuccess {
                    path: request_path,
                    kind,
                    target_kind,
                    size_bytes: metadata.is_file().then_some(metadata.len()),
                    resolved_path,
                })
            }
            Err(err) => resolve_path_failure(&msg.path, err),
        };

        HandlerOutcome::Sync(server_message::Message::ResolvePathResponse(
            ResolvePathResponse {
                result: Some(result),
            },
        ))
    }

    #[cfg(feature = "local_fs")]
    fn handle_create_directory(&self, msg: CreateDirectory) -> HandlerOutcome {
        let path = expand_user_path(&msg.path);
        let result = match std::fs::create_dir_all(&path) {
            Ok(()) => create_directory_response::Result::Success(CreateDirectorySuccess {}),
            Err(err) => create_directory_response::Result::Error(FileOperationError {
                message: format!("Failed to create directory {}: {err}", msg.path),
            }),
        };

        HandlerOutcome::Sync(server_message::Message::CreateDirectoryResponse(
            CreateDirectoryResponse {
                result: Some(result),
            },
        ))
    }

    #[cfg(feature = "local_fs")]
    fn handle_delete_directory(&self, msg: DeleteDirectory) -> HandlerOutcome {
        let path = expand_user_path(&msg.path);
        let result = msg
            .identity
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "missing directory identity")
            })
            .and_then(|identity| delete_directory_identity_bound(&path, &identity));
        let result = match result {
            Ok(()) => delete_directory_response::Result::Success(DeleteDirectorySuccess {}),
            Err(err) => delete_directory_response::Result::Error(FileOperationError {
                message: format!("Failed to delete directory {}: {err}", path.display()),
            }),
        };
        HandlerOutcome::Sync(server_message::Message::DeleteDirectoryResponse(
            DeleteDirectoryResponse {
                result: Some(result),
            },
        ))
    }

    #[cfg(feature = "local_fs")]
    fn handle_exact_rename(&self, msg: ExactRename) -> HandlerOutcome {
        let from = expand_user_path(&msg.from_path);
        let to = expand_user_path(&msg.to_path);
        let result = match exact_rename_path(&from, &to, false) {
            Ok(()) => exact_rename_response::Result::Success(ExactRenameSuccess {
                committed_path: msg.to_path,
            }),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                exact_rename_response::Result::Conflict(ExactRenameConflict {
                    requested_path: msg.to_path,
                })
            }
            Err(error) => exact_rename_response::Result::Error(FileOperationError {
                message: format!(
                    "Failed to rename {} -> {}: {error}",
                    msg.from_path, msg.to_path
                ),
            }),
        };
        HandlerOutcome::Sync(server_message::Message::ExactRenameResponse(
            ExactRenameResponse {
                result: Some(result),
            },
        ))
    }

    #[cfg(feature = "local_fs")]
    fn handle_append_file(&self, msg: AppendFile) -> HandlerOutcome {
        let path = expand_user_path(&msg.path);
        let result = append_file_nofollow(&path, &msg.bytes);
        let result = match result {
            Ok(file_size) => append_file_response::Result::Success(AppendFileSuccess { file_size }),
            Err(error) => append_file_response::Result::Error(FileOperationError {
                message: format!("Failed to append file {}: {error}", msg.path),
            }),
        };
        HandlerOutcome::Sync(server_message::Message::AppendFileResponse(
            AppendFileResponse {
                result: Some(result),
            },
        ))
    }

    #[cfg(feature = "local_fs")]
    fn file_transfers_for_connection_mut(
        &mut self,
        conn_id: ConnectionId,
    ) -> io::Result<&mut HashMap<String, FileTransferState>> {
        self.connections
            .get_mut(&conn_id)
            .map(|connection| &mut connection.file_transfers)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "connection is not registered"))
    }

    #[cfg(feature = "local_fs")]
    fn handle_begin_file_transfer(
        &mut self,
        msg: BeginFileTransfer,
        conn_id: ConnectionId,
    ) -> HandlerOutcome {
        let path = expand_user_path(&msg.path);
        let result = begin_file_transfer_state(&path, msg.direction, msg.executable);
        let result = match result {
            Ok((state, total_size)) => {
                let handle = uuid::Uuid::new_v4().to_string();
                match self.file_transfers_for_connection_mut(conn_id) {
                    Ok(transfers) => {
                        transfers.insert(handle.clone(), state);
                        begin_file_transfer_response::Result::Success(BeginFileTransferSuccess {
                            handle: Some(FileTransferHandle { id: handle }),
                            total_size,
                        })
                    }
                    Err(error) => begin_file_transfer_response::Result::Error(FileOperationError {
                        message: format!("Failed to begin file transfer {}: {error}", msg.path),
                    }),
                }
            }
            Err(error) => begin_file_transfer_response::Result::Error(FileOperationError {
                message: format!("Failed to begin file transfer {}: {error}", msg.path),
            }),
        };
        HandlerOutcome::Sync(server_message::Message::BeginFileTransferResponse(
            BeginFileTransferResponse {
                result: Some(result),
            },
        ))
    }

    #[cfg(feature = "local_fs")]
    fn handle_read_file_chunk(
        &mut self,
        msg: ReadFileChunk,
        conn_id: ConnectionId,
    ) -> HandlerOutcome {
        let result = transfer_handle_id(msg.handle).and_then(|handle| {
            let transfers = self.file_transfers_for_connection_mut(conn_id)?;
            read_transfer_chunk(transfers, &handle, msg.max_bytes)
        });
        let result = match result {
            Ok(success) => read_file_chunk_response::Result::Success(success),
            Err(error) => read_file_chunk_response::Result::Error(FileOperationError {
                message: format!("Failed to read file chunk: {error}"),
            }),
        };
        HandlerOutcome::Sync(server_message::Message::ReadFileChunkResponse(
            ReadFileChunkResponse {
                result: Some(result),
            },
        ))
    }

    #[cfg(feature = "local_fs")]
    fn handle_write_file_chunk(
        &mut self,
        msg: WriteFileChunk,
        conn_id: ConnectionId,
    ) -> HandlerOutcome {
        let result = transfer_handle_id(msg.handle).and_then(|handle| {
            let transfers = self.file_transfers_for_connection_mut(conn_id)?;
            write_transfer_chunk(transfers, &handle, &msg.bytes)
        });
        let result = match result {
            Ok(success) => write_file_chunk_response::Result::Success(success),
            Err(error) => write_file_chunk_response::Result::Error(FileOperationError {
                message: format!("Failed to write file chunk: {error}"),
            }),
        };
        HandlerOutcome::Sync(server_message::Message::WriteFileChunkResponse(
            WriteFileChunkResponse {
                result: Some(result),
            },
        ))
    }

    #[cfg(feature = "local_fs")]
    fn handle_finish_file_transfer(
        &mut self,
        msg: FinishFileTransfer,
        conn_id: ConnectionId,
    ) -> HandlerOutcome {
        let result = transfer_handle_id(msg.handle).and_then(|handle| {
            let transfers = self.file_transfers_for_connection_mut(conn_id)?;
            finish_transfer(transfers, &handle)
        });
        let result = match result {
            Ok(committed_path) => {
                finish_file_transfer_response::Result::Success(FinishFileTransferSuccess {
                    committed_path: committed_path.map(|path| path.to_string_lossy().into_owned()),
                })
            }
            Err(error) => finish_file_transfer_response::Result::Error(FileOperationError {
                message: format!("Failed to finish file transfer: {error}"),
            }),
        };
        HandlerOutcome::Sync(server_message::Message::FinishFileTransferResponse(
            FinishFileTransferResponse {
                result: Some(result),
            },
        ))
    }

    #[cfg(feature = "local_fs")]
    fn handle_abort_file_transfer(
        &mut self,
        msg: AbortFileTransfer,
        conn_id: ConnectionId,
    ) -> HandlerOutcome {
        let result = transfer_handle_id(msg.handle).and_then(|handle| {
            let transfers = self.file_transfers_for_connection_mut(conn_id)?;
            abort_transfer(transfers, &handle)
        });
        let result = match result {
            Ok(()) => abort_file_transfer_response::Result::Success(AbortFileTransferSuccess {}),
            Err(error) => abort_file_transfer_response::Result::Error(FileOperationError {
                message: format!("Failed to abort file transfer: {error}"),
            }),
        };
        HandlerOutcome::Sync(server_message::Message::AbortFileTransferResponse(
            AbortFileTransferResponse {
                result: Some(result),
            },
        ))
    }

    #[cfg(feature = "local_fs")]
    fn handle_promote_files(&self, msg: PromoteFiles) -> HandlerOutcome {
        let result = promote_files_exact(msg);
        let result = match result {
            Ok(results) => promote_files_response::Result::Success(PromoteFilesSuccess { results }),
            Err(error) => promote_files_response::Result::Error(FileOperationError {
                message: format!("Failed to promote staging files: {error}"),
            }),
        };
        HandlerOutcome::Sync(server_message::Message::PromoteFilesResponse(
            PromoteFilesResponse {
                result: Some(result),
            },
        ))
    }

    /// Scans Claude/Codex session history natively on the daemon. Replaces the
    /// former remote-Python scan heredoc with one round trip over `std::fs`.
    #[cfg(feature = "local_fs")]
    fn handle_scan_cli_agent_sessions(&self, msg: ScanCliAgentSessions) -> HandlerOutcome {
        let ScanCliAgentSessions {
            limit,
            roots,
            enabled_agents,
            previously_observed_agents,
        } = msg;
        let result = decode_scan_cli_agent_wire_agents("enabled_agents", enabled_agents)
            .and_then(|enabled_agents| {
                decode_scan_cli_agent_wire_agents(
                    "previously_observed_agents",
                    previously_observed_agents,
                )
                .map(|previously_observed_agents| (enabled_agents, previously_observed_agents))
            })
            .and_then(|(enabled_agents, previously_observed_agents)| {
                cli_agent_store_roots_from_request(roots).and_then(|roots| {
                    super::cli_agent_sessions::scan_sessions(
                        &roots,
                        limit as usize,
                        enabled_agents,
                        previously_observed_agents,
                    )
                    .map_err(|error| error.to_string())
                })
            });
        let response = build_scan_cli_agent_sessions_response(result);
        HandlerOutcome::Sync(server_message::Message::ScanCliAgentSessionsResponse(
            response,
        ))
    }

    /// Resolves a session source (including codex index → transcript) and reads
    /// its bytes natively on the daemon.
    #[cfg(feature = "local_fs")]
    fn handle_read_cli_agent_session(&self, msg: ReadCliAgentSession) -> HandlerOutcome {
        let result = match cli_agent_store_roots_from_request(msg.roots)
            .and_then(|roots| super::cli_agent_sessions::read_session(&msg.source, &roots))
        {
            Ok(session) => {
                read_cli_agent_session_response::Result::Success(ReadCliAgentSessionSuccess {
                    reference: session.reference,
                    sha256: session.sha256,
                    content: session.content,
                })
            }
            Err(message) => {
                read_cli_agent_session_response::Result::Error(FileOperationError { message })
            }
        };
        HandlerOutcome::Sync(server_message::Message::ReadCliAgentSessionResponse(
            ReadCliAgentSessionResponse {
                result: Some(result),
            },
        ))
    }

    /// Archives or deletes a session source natively on the daemon.
    #[cfg(feature = "local_fs")]
    fn handle_mutate_cli_agent_session(&self, msg: MutateCliAgentSession) -> HandlerOutcome {
        let mutation = match msg.mutation() {
            CliAgentSessionMutation::Delete => super::cli_agent_sessions::Mutation::Delete,
            // Archive and the unspecified default both archive (the client
            // never sends Unspecified, but archiving is the safe fallback).
            CliAgentSessionMutation::Archive | CliAgentSessionMutation::Unspecified => {
                super::cli_agent_sessions::Mutation::Archive
            }
        };
        let result = match cli_agent_store_roots_from_request(msg.roots).and_then(|roots| {
            super::cli_agent_sessions::mutate_session(&msg.source, mutation, &roots)
        }) {
            Ok(()) => {
                mutate_cli_agent_session_response::Result::Success(MutateCliAgentSessionSuccess {})
            }
            Err(message) => {
                mutate_cli_agent_session_response::Result::Error(FileOperationError { message })
            }
        };
        HandlerOutcome::Sync(server_message::Message::MutateCliAgentSessionResponse(
            MutateCliAgentSessionResponse {
                result: Some(result),
            },
        ))
    }

    /// Reads Ashide-owned Session Navigator state from the daemon user's
    /// environment config directory.
    #[cfg(feature = "local_fs")]
    fn handle_get_cli_agent_session_user_state(
        &self,
        _msg: GetCliAgentSessionUserState,
    ) -> HandlerOutcome {
        let result = match super::cli_agent_session_user_state::read_state() {
            Ok(state) => get_cli_agent_session_user_state_response::Result::Success(
                GetCliAgentSessionUserStateSuccess {
                    state: Some(cli_agent_session_user_state_to_proto(state)),
                },
            ),
            Err(message) => {
                get_cli_agent_session_user_state_response::Result::Error(FileOperationError {
                    message,
                })
            }
        };
        HandlerOutcome::Sync(
            server_message::Message::GetCliAgentSessionUserStateResponse(
                GetCliAgentSessionUserStateResponse {
                    result: Some(result),
                },
            ),
        )
    }

    /// Mutates Ashide-owned Session Navigator state on the daemon host.
    #[cfg(feature = "local_fs")]
    fn handle_mutate_cli_agent_session_user_state(
        &self,
        msg: MutateCliAgentSessionUserState,
    ) -> HandlerOutcome {
        let mutation = match msg.mutation() {
            CliAgentSessionUserStateMutation::SetAlias => {
                super::cli_agent_session_user_state::SessionUserStateMutation::SetAlias(
                    msg.alias.unwrap_or_default(),
                )
            }
            CliAgentSessionUserStateMutation::ClearAlias
            | CliAgentSessionUserStateMutation::Unspecified => {
                super::cli_agent_session_user_state::SessionUserStateMutation::ClearAlias
            }
            CliAgentSessionUserStateMutation::SetPinned => {
                super::cli_agent_session_user_state::SessionUserStateMutation::SetPinned
            }
            CliAgentSessionUserStateMutation::ClearPinned => {
                super::cli_agent_session_user_state::SessionUserStateMutation::ClearPinned
            }
        };
        let result =
            match super::cli_agent_session_user_state::mutate_state(msg.keys, mutation) {
                Ok(state) => mutate_cli_agent_session_user_state_response::Result::Success(
                    MutateCliAgentSessionUserStateSuccess {
                        state: Some(cli_agent_session_user_state_to_proto(state)),
                    },
                ),
                Err(message) => mutate_cli_agent_session_user_state_response::Result::Error(
                    FileOperationError { message },
                ),
            };
        HandlerOutcome::Sync(
            server_message::Message::MutateCliAgentSessionUserStateResponse(
                MutateCliAgentSessionUserStateResponse {
                    result: Some(result),
                },
            ),
        )
    }

    /// Handles `CloseBuffer` notification (fire-and-forget).
    /// Removes the connection from the buffer's connection set.
    /// Deallocates the buffer if no connections remain.
    #[cfg(feature = "local_fs")]
    fn handle_close_buffer(
        &mut self,
        msg: CloseBuffer,
        conn_id: ConnectionId,
        ctx: &mut ModelContext<Self>,
    ) {
        log::info!(
            "Handling CloseBuffer path={path} conn={conn_id}",
            path = msg.path
        );
        self.buffers.close_buffer(&msg.path, conn_id, ctx);
    }
}

#[cfg(feature = "local_fs")]
fn cli_agent_session_user_state_to_proto(
    state: super::cli_agent_session_user_state::SessionUserState,
) -> CliAgentSessionUserState {
    let mut pinned = state.pinned.into_iter().collect::<Vec<_>>();
    pinned.sort();
    CliAgentSessionUserState {
        aliases: state.aliases,
        pinned,
    }
}

#[cfg(feature = "local_fs")]
fn expand_user_path(path: &str) -> PathBuf {
    if path == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(stripped);
        }
    }
    PathBuf::from(path)
}

#[cfg(all(feature = "local_fs", unix))]
fn transfer_parent_and_name(path: &Path) -> io::Result<(&Path, std::ffi::CString)> {
    use std::os::unix::ffi::OsStrExt;

    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "transfer path has no parent")
    })?;
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "transfer path has no file name",
        )
    })?;
    let name = std::ffi::CString::new(name.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))?;
    Ok((parent, name))
}

#[cfg(all(feature = "local_fs", unix))]
fn path_component(component: &std::ffi::OsStr) -> io::Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;

    std::ffi::CString::new(component.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))
}

#[cfg(all(feature = "local_fs", unix))]
fn open_transfer_directory_fd(
    path: &Path,
    create_missing: bool,
) -> io::Result<std::os::fd::OwnedFd> {
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::path::Component;

    let initial = if path.is_absolute() { "/" } else { "." };
    let initial = std::ffi::CString::new(initial).expect("static path cannot contain NUL");
    let fd = unsafe {
        libc::open(
            initial.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut directory = unsafe { OwnedFd::from_raw_fd(fd) };
    for component in path.components() {
        let name = match component {
            Component::RootDir | Component::CurDir => continue,
            Component::Normal(name) => path_component(name)?,
            Component::ParentDir | Component::Prefix(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "path must not contain parent or platform-prefix components",
                ));
            }
        };
        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW;
        let mut next_fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        if next_fd < 0
            && create_missing
            && io::Error::last_os_error().kind() == io::ErrorKind::NotFound
        {
            let result = unsafe { libc::mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o755) };
            if result < 0 && io::Error::last_os_error().kind() != io::ErrorKind::AlreadyExists {
                return Err(io::Error::last_os_error());
            }
            next_fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        }
        if next_fd < 0 {
            return Err(io::Error::last_os_error());
        }
        directory = unsafe { OwnedFd::from_raw_fd(next_fd) };
    }
    Ok(directory)
}

#[cfg(all(feature = "local_fs", unix))]
fn ensure_replaceable_transfer_destination(
    parent: &impl std::os::fd::AsRawFd,
    final_name: &std::ffi::CStr,
) -> io::Result<()> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            final_name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } < 0
    {
        let error = io::Error::last_os_error();
        return if error.kind() == io::ErrorKind::NotFound {
            Ok(())
        } else {
            Err(error)
        };
    }
    let stat = unsafe { stat.assume_init() };
    if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "transfer destination is not a replaceable regular file",
        ));
    }
    Ok(())
}

#[cfg(all(feature = "local_fs", unix))]
fn identity_from_metadata(metadata: &std::fs::Metadata) -> DeleteDirectoryIdentity {
    use std::os::unix::fs::MetadataExt;

    DeleteDirectoryIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    }
}

#[cfg(all(feature = "local_fs", unix))]
fn identity_from_fd(fd: &impl std::os::fd::AsRawFd) -> io::Result<DeleteDirectoryIdentity> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(fd.as_raw_fd(), stat.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let stat = unsafe { stat.assume_init() };
    Ok(DeleteDirectoryIdentity {
        device: stat.st_dev as u64,
        inode: stat.st_ino as u64,
    })
}

#[cfg(all(feature = "local_fs", unix))]
fn ensure_identity(
    actual: &DeleteDirectoryIdentity,
    expected: &DeleteDirectoryIdentity,
) -> io::Result<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "directory identity changed before mutation",
        ))
    }
}

#[cfg(all(feature = "local_fs", unix))]
fn open_regular_source_nofollow(path: &Path) -> io::Result<std::fs::File> {
    use std::os::fd::{AsRawFd, FromRawFd};

    let (parent_path, name) = transfer_parent_and_name(path)?;
    let parent = open_transfer_directory_fd(parent_path, false)?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "transfer source is not a regular file",
        ));
    }
    Ok(file)
}

#[cfg(all(feature = "local_fs", unix))]
fn create_unique_transfer_staging(
    parent: &impl std::os::fd::AsRawFd,
) -> io::Result<(std::ffi::CString, std::fs::File)> {
    use std::os::fd::FromRawFd;

    for _ in 0..64 {
        let name = format!(".ashide-transfer-{}.staging", uuid::Uuid::new_v4());
        let name = std::ffi::CString::new(name).expect("uuid staging name cannot contain NUL");
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0o600,
            )
        };
        if fd >= 0 {
            return Ok((name, unsafe { std::fs::File::from_raw_fd(fd) }));
        }
        if io::Error::last_os_error().kind() != io::ErrorKind::AlreadyExists {
            return Err(io::Error::last_os_error());
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate unique transfer staging file",
    ))
}

#[cfg(all(feature = "local_fs", unix))]
fn begin_file_transfer_state(
    path: &Path,
    direction: i32,
    executable: Option<bool>,
) -> io::Result<(FileTransferState, Option<u64>)> {
    match FileTransferDirection::try_from(direction).unwrap_or(FileTransferDirection::Unspecified) {
        FileTransferDirection::Read => {
            let file = open_regular_source_nofollow(path)?;
            let total_size = file.metadata()?.len();
            Ok((
                FileTransferState::Read {
                    file,
                    total_size,
                    offset: 0,
                },
                Some(total_size),
            ))
        }
        FileTransferDirection::Write => {
            let (parent_path, final_name) = transfer_parent_and_name(path)?;
            let parent = open_transfer_directory_fd(parent_path, true)?;
            ensure_replaceable_transfer_destination(&parent, &final_name)?;
            let parent_identity = identity_from_fd(&parent)?;
            let (staging_name, file) = create_unique_transfer_staging(&parent)?;
            Ok((
                FileTransferState::Write {
                    file,
                    staging_name,
                    final_name,
                    parent,
                    parent_path: parent_path.to_path_buf(),
                    parent_identity,
                    final_path: path.to_path_buf(),
                    executable,
                    offset: 0,
                },
                None,
            ))
        }
        FileTransferDirection::Unspecified => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "file transfer direction is unspecified",
        )),
    }
}

#[cfg(all(feature = "local_fs", not(unix)))]
fn begin_file_transfer_state(
    _path: &Path,
    _direction: i32,
    _executable: Option<bool>,
) -> io::Result<(FileTransferState, Option<u64>)> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "inode-pinned file transfers require Unix descriptor semantics",
    ))
}

#[cfg(feature = "local_fs")]
fn transfer_handle_id(handle: Option<FileTransferHandle>) -> io::Result<String> {
    let id = handle
        .map(|handle| handle.id)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing transfer handle"))?;
    Ok(id)
}

#[cfg(all(feature = "local_fs", unix))]
fn read_transfer_chunk(
    transfers: &mut HashMap<String, FileTransferState>,
    handle: &str,
    max_bytes: u64,
) -> io::Result<ReadFileChunkSuccess> {
    use std::io::Read;

    let mut transfer = transfers
        .remove(handle)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "unknown read transfer handle"))?;
    let result = match &mut transfer {
        FileTransferState::Read {
            file,
            total_size,
            offset,
        } => {
            if *offset == *total_size {
                Ok(ReadFileChunkSuccess {
                    bytes: Vec::new(),
                    next_offset: *offset,
                    total_size: Some(*total_size),
                    eof: true,
                })
            } else if max_bytes == 0 {
                Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "read transfer chunk size must be greater than zero before EOF",
                ))
            } else {
                let remaining = *total_size - *offset;
                let read_budget = max_bytes.min(8 * 1024 * 1024).min(remaining);
                let mut bytes = vec![0; read_budget as usize];
                let read = file.read(&mut bytes)?;
                if read == 0 {
                    Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        format!(
                            "transfer source truncated at offset {} before captured size {}",
                            *offset, *total_size
                        ),
                    ))
                } else {
                    bytes.truncate(read);
                    *offset += read as u64;
                    Ok(ReadFileChunkSuccess {
                        bytes,
                        next_offset: *offset,
                        total_size: Some(*total_size),
                        eof: *offset == *total_size,
                    })
                }
            }
        }
        FileTransferState::Write { .. } => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "transfer handle is not readable",
        )),
    };
    if result.is_ok() {
        transfers.insert(handle.to_owned(), transfer);
    }
    result
}

#[cfg(all(feature = "local_fs", not(unix)))]
fn read_transfer_chunk(
    _transfers: &mut HashMap<String, FileTransferState>,
    _handle: &str,
    _max_bytes: u64,
) -> io::Result<ReadFileChunkSuccess> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "unsupported platform",
    ))
}

#[cfg(all(feature = "local_fs", unix))]
fn write_transfer_chunk(
    transfers: &mut HashMap<String, FileTransferState>,
    handle: &str,
    bytes: &[u8],
) -> io::Result<WriteFileChunkSuccess> {
    use std::io::Write;

    let Some(FileTransferState::Write { file, offset, .. }) = transfers.get_mut(handle) else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "unknown write transfer handle",
        ));
    };
    file.write_all(bytes)?;
    *offset += bytes.len() as u64;
    Ok(WriteFileChunkSuccess {
        next_offset: *offset,
    })
}

#[cfg(all(feature = "local_fs", not(unix)))]
fn write_transfer_chunk(
    _transfers: &mut HashMap<String, FileTransferState>,
    _handle: &str,
    _bytes: &[u8],
) -> io::Result<WriteFileChunkSuccess> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "unsupported platform",
    ))
}

#[cfg(all(feature = "local_fs", unix))]
fn finish_transfer(
    transfers: &mut HashMap<String, FileTransferState>,
    handle: &str,
) -> io::Result<Option<PathBuf>> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::PermissionsExt;

    let mut transfer = transfers
        .remove(handle)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "unknown transfer handle"))?;
    match &mut transfer {
        FileTransferState::Read {
            total_size, offset, ..
        } => {
            if *offset != *total_size {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!(
                        "cannot finish partial read transfer at offset {} before captured size {}",
                        *offset, *total_size
                    ),
                ));
            }
            Ok(None)
        }
        FileTransferState::Write {
            file,
            staging_name,
            final_name,
            parent,
            parent_path,
            parent_identity,
            final_path,
            executable,
            ..
        } => {
            file.sync_all()?;
            if let Some(executable) = *executable {
                file.set_permissions(std::fs::Permissions::from_mode(if executable {
                    0o755
                } else {
                    0o644
                }))?;
            }
            ensure_identity(
                &identity_from_fd(&open_transfer_directory_fd(&parent_path, false)?)?,
                &parent_identity,
            )?;
            ensure_replaceable_transfer_destination(parent, final_name)?;
            if unsafe {
                libc::renameat(
                    parent.as_raw_fd(),
                    staging_name.as_ptr(),
                    parent.as_raw_fd(),
                    final_name.as_ptr(),
                )
            } < 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(Some(final_path.clone()))
        }
    }
}

#[cfg(all(feature = "local_fs", not(unix)))]
fn finish_transfer(
    _transfers: &mut HashMap<String, FileTransferState>,
    _handle: &str,
) -> io::Result<Option<PathBuf>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "unsupported platform",
    ))
}

#[cfg(feature = "local_fs")]
fn abort_transfer(
    transfers: &mut HashMap<String, FileTransferState>,
    handle: &str,
) -> io::Result<()> {
    transfers
        .remove(handle)
        .map(|_| ())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "unknown transfer handle"))
}

#[cfg(all(feature = "local_fs", unix))]
fn append_file_nofollow(path: &Path, bytes: &[u8]) -> io::Result<u64> {
    use std::io::Write;
    use std::os::fd::{AsRawFd, FromRawFd};

    let (parent_path, name) = transfer_parent_and_name(path)?;
    let parent = open_transfer_directory_fd(parent_path, true)?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_APPEND | libc::O_CREAT | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0o644,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    if !bytes.is_empty() {
        let written = file.write(bytes)?;
        if written != bytes.len() {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "atomic append was partial",
            ));
        }
    }
    Ok(file.metadata()?.len())
}

#[cfg(all(feature = "local_fs", not(unix)))]
fn append_file_nofollow(_path: &Path, _bytes: &[u8]) -> io::Result<u64> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "owning-host append requires Unix O_APPEND",
    ))
}

#[cfg(all(feature = "local_fs", unix))]
fn recursive_delete_fd(directory: &impl std::os::fd::AsRawFd) -> io::Result<()> {
    use std::ffi::CStr;
    use std::os::fd::{FromRawFd, OwnedFd};

    let duplicate = unsafe { libc::dup(directory.as_raw_fd()) };
    if duplicate < 0 {
        return Err(io::Error::last_os_error());
    }
    let stream = unsafe { libc::fdopendir(duplicate) };
    if stream.is_null() {
        unsafe { libc::close(duplicate) };
        return Err(io::Error::last_os_error());
    }
    loop {
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            break;
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        if name.to_bytes() == b"." || name.to_bytes() == b".." {
            continue;
        }
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe {
            libc::fstatat(
                directory.as_raw_fd(),
                name.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } < 0
        {
            unsafe { libc::closedir(stream) };
            return Err(io::Error::last_os_error());
        }
        let stat = unsafe { stat.assume_init() };
        if stat.st_mode & libc::S_IFMT == libc::S_IFDIR {
            let child = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                )
            };
            if child < 0 {
                unsafe { libc::closedir(stream) };
                return Err(io::Error::last_os_error());
            }
            let child = unsafe { OwnedFd::from_raw_fd(child) };
            recursive_delete_fd(&child)?;
            if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) }
                < 0
            {
                unsafe { libc::closedir(stream) };
                return Err(io::Error::last_os_error());
            }
        } else if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } < 0 {
            unsafe { libc::closedir(stream) };
            return Err(io::Error::last_os_error());
        }
    }
    if unsafe { libc::closedir(stream) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(all(feature = "local_fs", unix))]
fn delete_directory_identity_bound(
    path: &Path,
    expected: &DeleteDirectoryIdentity,
) -> io::Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    let (parent_path, name) = transfer_parent_and_name(path)?;
    let parent = open_transfer_directory_fd(parent_path, false)?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let directory = unsafe { OwnedFd::from_raw_fd(fd) };
    ensure_identity(&identity_from_fd(&directory)?, expected)?;
    recursive_delete_fd(&directory)?;
    if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(all(feature = "local_fs", not(unix)))]
fn delete_directory_identity_bound(
    _path: &Path,
    _expected: &DeleteDirectoryIdentity,
) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "identity-bound recursive delete requires Unix dirfd semantics",
    ))
}

#[cfg(all(feature = "local_fs", target_os = "linux"))]
fn renameat_noreplace(
    from_parent: &impl std::os::fd::AsRawFd,
    from_name: &std::ffi::CStr,
    to_parent: &impl std::os::fd::AsRawFd,
    to_name: &std::ffi::CStr,
) -> io::Result<()> {
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            from_parent.as_raw_fd(),
            from_name.as_ptr(),
            to_parent.as_raw_fd(),
            to_name.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(all(feature = "local_fs", target_os = "macos"))]
fn renameat_noreplace(
    from_parent: &impl std::os::fd::AsRawFd,
    from_name: &std::ffi::CStr,
    to_parent: &impl std::os::fd::AsRawFd,
    to_name: &std::ffi::CStr,
) -> io::Result<()> {
    let result = unsafe {
        libc::renameatx_np(
            from_parent.as_raw_fd(),
            from_name.as_ptr(),
            to_parent.as_raw_fd(),
            to_name.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(all(
    feature = "local_fs",
    unix,
    not(any(target_os = "linux", target_os = "macos"))
))]
fn renameat_noreplace(
    _from_parent: &impl std::os::fd::AsRawFd,
    _from_name: &std::ffi::CStr,
    _to_parent: &impl std::os::fd::AsRawFd,
    _to_name: &std::ffi::CStr,
) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic no-replace rename is unsupported on this Unix target",
    ))
}

#[cfg(all(feature = "local_fs", unix))]
fn exact_rename_path(from: &Path, to: &Path, overwrite: bool) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    let (from_parent_path, from_name) = transfer_parent_and_name(from)?;
    let (to_parent_path, to_name) = transfer_parent_and_name(to)?;
    let from_parent = open_transfer_directory_fd(from_parent_path, false)?;
    let to_parent = open_transfer_directory_fd(to_parent_path, true)?;
    if overwrite {
        ensure_replaceable_transfer_destination(&to_parent, &to_name)?;
        if unsafe {
            libc::renameat(
                from_parent.as_raw_fd(),
                from_name.as_ptr(),
                to_parent.as_raw_fd(),
                to_name.as_ptr(),
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    } else {
        renameat_noreplace(&from_parent, &from_name, &to_parent, &to_name)
    }
}

#[cfg(all(feature = "local_fs", not(unix)))]
fn exact_rename_path(_from: &Path, _to: &Path, _overwrite: bool) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "exact rename requires atomic no-replace support",
    ))
}

#[cfg(feature = "local_fs")]
fn promote_files_exact(msg: PromoteFiles) -> io::Result<Vec<PromotionResult>> {
    if msg.overwrite {
        let mut roots = msg.directory_overwrite_roots;
        roots.sort_by_key(|root| std::cmp::Reverse(root.len()));
        for root in roots {
            let path = expand_user_path(&root);
            match std::fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("refusing to overwrite symlink directory root: {root}"),
                    ));
                }
                Ok(metadata) if metadata.is_dir() => {
                    #[cfg(unix)]
                    delete_directory_identity_bound(&path, &identity_from_metadata(&metadata))?;
                    #[cfg(not(unix))]
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "directory overwrite requires Unix dirfd semantics",
                    ));
                }
                Ok(_) => std::fs::remove_file(&path)?,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
    }

    Ok(msg
        .targets
        .into_iter()
        .map(|target| {
            let staging = expand_user_path(&target.staging_path);
            let final_path = expand_user_path(&target.final_path);
            match exact_rename_path(&staging, &final_path, msg.overwrite) {
                Ok(()) => PromotionResult {
                    staging_path: target.staging_path,
                    requested_path: target.final_path.clone(),
                    committed_path: Some(target.final_path),
                    status: PromotionStatus::Committed as i32,
                    error: None,
                },
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => PromotionResult {
                    staging_path: target.staging_path,
                    requested_path: target.final_path,
                    committed_path: None,
                    status: PromotionStatus::Conflict as i32,
                    error: Some(error.to_string()),
                },
                Err(error) => PromotionResult {
                    staging_path: target.staging_path,
                    requested_path: target.final_path,
                    committed_path: None,
                    status: PromotionStatus::Failed as i32,
                    error: Some(error.to_string()),
                },
            }
        })
        .collect())
}

#[cfg(feature = "local_fs")]
fn reject_symlink_transfer_path(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "symbolic link byte transfer is unsupported: {}",
                path.display()
            ),
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(feature = "local_fs")]
fn resolve_path_failure(path: &str, error: io::Error) -> resolve_path_response::Result {
    if error.kind() == io::ErrorKind::NotFound {
        resolve_path_response::Result::NotFound(ResolvePathNotFound {})
    } else {
        resolve_path_response::Result::Error(FileOperationError {
            message: format!("Failed to resolve path {path}: {error}"),
        })
    }
}

#[cfg(feature = "local_fs")]
fn collect_complete_directory_listing(
    entries: impl IntoIterator<Item = io::Result<std::fs::DirEntry>>,
) -> io::Result<Vec<DirEntry>> {
    let mut listing = Vec::new();
    let directory_path = entries.into_iter().collect::<Vec<_>>();
    let directory_root = directory_path
        .iter()
        .find_map(|entry| entry.as_ref().ok())
        .and_then(|entry| entry.path().parent().map(Path::to_path_buf));
    let gitignores = directory_root
        .as_deref()
        .map(repo_metadata::gitignores_for_directory)
        .unwrap_or_default();
    for entry in directory_path {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        let metadata = std::fs::symlink_metadata(&path)?;
        let kind = entry_kind(Some(&file_type), Some(&metadata));
        let target_kind = complete_listing_target_kind(&file_type, &path)?;
        let is_dir = kind == FileSystemEntryKind::Directory as i32
            || target_kind == FileSystemEntryKind::Directory as i32;
        let size_bytes = (!metadata.is_dir()).then(|| metadata.len());
        let modified_epoch_millis = Some(system_time_to_epoch_millis(metadata.modified()?)?);
        let platform_hidden = repo_metadata::platform_hidden(&metadata);
        let ignored = repo_metadata::matches_gitignores(&path, is_dir, &gitignores, true);
        #[cfg(unix)]
        let directory_identity = metadata.is_dir().then(|| identity_from_metadata(&metadata));
        #[cfg(not(unix))]
        let directory_identity = None;
        listing.push(DirEntry {
            name: entry.file_name().to_string_lossy().into_owned(),
            is_dir,
            kind,
            target_kind,
            size_bytes,
            modified_epoch_millis,
            directory_identity,
            platform_hidden,
            ignored,
        });
    }
    listing.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(listing)
}

#[cfg(feature = "local_fs")]
fn complete_listing_target_kind(
    file_type: &std::fs::FileType,
    path: &std::path::Path,
) -> io::Result<i32> {
    if !file_type.is_symlink() {
        return Ok(FileSystemEntryKind::Unspecified as i32);
    }
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(FileSystemEntryKind::Directory as i32),
        Ok(metadata) if metadata.is_file() => Ok(FileSystemEntryKind::File as i32),
        Ok(_) => Ok(FileSystemEntryKind::Other as i32),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Ok(FileSystemEntryKind::Missing as i32)
        }
        Err(error) => Err(error),
    }
}

#[cfg(feature = "local_fs")]
fn entry_kind(file_type: Option<&std::fs::FileType>, metadata: Option<&std::fs::Metadata>) -> i32 {
    if file_type.is_some_and(|ft| ft.is_symlink()) {
        return FileSystemEntryKind::Symlink as i32;
    }
    if metadata.is_some_and(|metadata| metadata.is_dir()) {
        return FileSystemEntryKind::Directory as i32;
    }
    if metadata.is_some_and(|metadata| metadata.is_file()) {
        return FileSystemEntryKind::File as i32;
    }
    FileSystemEntryKind::Other as i32
}

/// For a symlink entry, follows the link and classifies the resolved target.
/// Returns `MISSING` for broken symlinks. Returns `UNSPECIFIED` for non-symlink
/// entries (target kind is not applicable).
fn entry_target_kind(file_type: Option<&std::fs::FileType>, path: &std::path::Path) -> i32 {
    if !file_type.is_some_and(|ft| ft.is_symlink()) {
        return FileSystemEntryKind::Unspecified as i32;
    }
    match std::fs::metadata(path) {
        Ok(metadata) => {
            if metadata.is_dir() {
                FileSystemEntryKind::Directory as i32
            } else if metadata.is_file() {
                FileSystemEntryKind::File as i32
            } else {
                FileSystemEntryKind::Other as i32
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            FileSystemEntryKind::Missing as i32
        }
        Err(_) => FileSystemEntryKind::Other as i32,
    }
}

#[cfg(feature = "local_fs")]
fn system_time_to_epoch_millis(time: std::time::SystemTime) -> io::Result<u64> {
    time.duration_since(std::time::UNIX_EPOCH)
        .map_err(io::Error::other)
        .map(|duration| duration.as_millis() as u64)
}

/// Converts a [`ReadFileContextResult`] into its protobuf equivalent.
fn file_context_result_to_proto(result: ReadFileContextResult) -> ReadFileContextResponse {
    use crate::ai::agent::AnyFileContent;

    let file_contexts = result
        .file_contexts
        .into_iter()
        .map(|fc| {
            let content = match fc.content {
                AnyFileContent::StringContent(text) => {
                    super::proto::file_context_proto::Content::TextContent(text)
                }
                AnyFileContent::BinaryContent(bytes) => {
                    super::proto::file_context_proto::Content::BinaryContent(bytes)
                }
            };
            let last_modified_epoch_millis = fc
                .last_modified
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64);
            FileContextProto {
                file_name: fc.file_name,
                content: Some(content),
                line_range_start: fc.line_range.as_ref().map(|r| r.start as u32),
                line_range_end: fc.line_range.as_ref().map(|r| r.end as u32),
                last_modified_epoch_millis,
                line_count: fc.line_count as u32,
            }
        })
        .collect();

    let failed_files = result
        .missing_files
        .into_iter()
        .map(|path| FailedFileRead {
            path,
            error: Some(FileOperationError {
                message: "File not found or could not be read".to_string(),
            }),
        })
        .collect();

    ReadFileContextResponse {
        file_contexts,
        failed_files,
    }
}

/// Validates protocol path identities without touching the filesystem.
///
/// `dir_path` is intentionally lexical: canonicalizing it would resolve a
/// directory symlink outside the repository and then reject the request as an
/// escape, even though the user selected a valid in-tree link path.
fn validate_repo_metadata_directory_load_paths(
    repo_path: &str,
    dir_path: &str,
) -> Result<(StandardizedPath, StandardizedPath), String> {
    let repo_path = StandardizedPath::try_new(repo_path)
        .map_err(|error| format!("Invalid repo_path: {error}"))?;
    let dir_path = StandardizedPath::try_new(dir_path)
        .map_err(|error| format!("Invalid dir_path: {error}"))?;

    if !dir_path.starts_with(&repo_path) {
        return Err(format!(
            "dir_path {dir_path} is not a descendant of repo_path {repo_path}"
        ));
    }

    Ok((repo_path, dir_path))
}

#[cfg(test)]
#[path = "server_model_tests.rs"]
mod tests;
