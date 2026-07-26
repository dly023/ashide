use std::collections::{HashMap, VecDeque};

use bimap::BiMap;
use warp_editor::content::buffer::Buffer;
use warp_util::content_version::ContentVersion;
use warp_util::file::FileId;
use warpui::{ModelContext, ModelHandle, SingletonEntity as _};

use super::server_model::{ConnectionId, ServerModel};
use crate::code::global_buffer_model::GlobalBufferModel;
use crate::environment_runtime_transport::protocol::RequestId;

#[derive(Clone, Debug)]
pub enum PendingBufferMutationKind {
    SaveBuffer,
    ResolveConflict {
        acknowledged_server_version: ContentVersion,
        current_client_version: ContentVersion,
        client_content: String,
    },
}

#[derive(Clone, Debug)]
pub struct PendingBufferMutation {
    pub request_id: RequestId,
    pub conn_id: ConnectionId,
    pub kind: PendingBufferMutationKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferWriterAccessError {
    NotOpen,
    NotOwner,
}

impl BufferWriterAccessError {
    pub fn message(self, path: &str) -> String {
        match self {
            Self::NotOpen => format!("Buffer not open: {path}"),
            Self::NotOwner => format!("Connection does not own buffer: {path}"),
        }
    }
}

#[derive(Default)]
struct BufferMutationQueue {
    active: Option<PendingBufferMutation>,
    queued: VecDeque<PendingBufferMutation>,
}

/// Bridges the ServerModel's per-connection state with the GlobalBufferModel's
/// tracked buffers. Manages:
/// - Wire path → FileId mappings for open server-local buffers
/// - Per-buffer connection sets (which connections have each buffer open)
/// - Pending async requests (OpenBuffer, SaveBuffer, ResolveConflict) awaiting events
pub struct ServerBufferTracker {
    /// 双向映射 wire path ↔ FileId,两侧查找都 O(1)。
    /// `path_for_file_id` 在每次 `ServerLocalBufferUpdated` 都会被调用,
    /// 用 BiMap 避免线性扫描。
    open_buffers: BiMap<String, FileId>,
    /// 持有每个已打开 server-local buffer 的**强引用** `ModelHandle<Buffer>`。
    ///
    /// `GlobalBufferModel` 内部只存 `WeakModelHandle`,客户端靠编辑器 view 持有
    /// 强引用让 buffer 存活;但 daemon 没有 view —— 若不在这里持有强引用,
    /// `handle_open_buffer` 返回后 buffer 引用计数归零,会被 WarpUI 的
    /// `flush_effects` 回收,导致随后 `FileModel` 异步加载完成时 weak handle 已
    /// 失效(日志「Cannot populate buffer with content due to deallocated model
    /// handle」)。buffer 关闭(无连接)时一并 drop。
    buffer_handles: HashMap<FileId, ModelHandle<Buffer>>,
    /// Single writable owner for each canonical buffer. The daemon SyncClock has one
    /// client_version, so allowing a second connection would create two writers for
    /// one clock and corrupt edit/save/close ownership.
    writer_connections: HashMap<FileId, ConnectionId>,
    /// OpenBuffer is a read fan-out: one BufferLoaded event may satisfy every
    /// waiter for the same canonical buffer.
    pending_open_requests: HashMap<FileId, Vec<(RequestId, ConnectionId)>>,
    /// SaveBuffer and ResolveConflict are ordered mutations. Exactly one
    /// mutation per FileId may be dispatched to GlobalBufferModel at a time;
    /// one FileSaved/FailedToSave event completes only the active head.
    mutation_queues: HashMap<FileId, BufferMutationQueue>,
}

impl ServerBufferTracker {
    pub fn new() -> Self {
        Self {
            open_buffers: BiMap::new(),
            buffer_handles: HashMap::new(),
            writer_connections: HashMap::new(),
            pending_open_requests: HashMap::new(),
            mutation_queues: HashMap::new(),
        }
    }

    // ── Path ↔ FileId mapping ─────────────────────────────────────

    /// Register a wire path → FileId mapping,并持有 buffer 的强引用让它在
    /// daemon 端存活(见 `buffer_handles` 字段说明)。
    pub fn track_open_buffer(
        &mut self,
        path: String,
        file_id: FileId,
        buffer: ModelHandle<Buffer>,
    ) {
        self.open_buffers.insert(path, file_id);
        self.buffer_handles.insert(file_id, buffer);
    }

    /// Look up a FileId by its wire path. O(1)。
    pub fn file_id_for_path(&self, path: &str) -> Option<FileId> {
        self.open_buffers.get_by_left(path).copied()
    }

    /// Look up the wire path for a given FileId. O(1) via BiMap。
    /// 返回 owned `String` 而非 `&str`,让调用方在持有结果的同时还能借用
    /// 其它 `&mut self`(典型场景:在事件 handler 里拿到 path 然后回头
    /// `send_server_message(...)`)。push 频率不高,clone 开销可忽略。
    pub fn path_for_file_id(&self, file_id: FileId) -> Option<String> {
        self.open_buffers.get_by_right(&file_id).cloned()
    }

    // ── Connection tracking ───────────────────────────────────────

    /// Claims the single writable connection for a canonical buffer.
    /// Repeated opens from the same connection are idempotent; a foreign connection fails closed.
    pub fn add_connection(&mut self, file_id: FileId, conn_id: ConnectionId) -> bool {
        match self.writer_connections.get(&file_id) {
            Some(owner) => *owner == conn_id,
            None => {
                self.writer_connections.insert(file_id, conn_id);
                true
            }
        }
    }

    pub fn connection_for_buffer(&self, file_id: &FileId) -> Option<ConnectionId> {
        self.writer_connections.get(file_id).copied()
    }

    pub fn is_writer(&self, file_id: FileId, conn_id: ConnectionId) -> bool {
        self.writer_connections.get(&file_id) == Some(&conn_id)
    }

    /// Resolves the canonical buffer only when the requesting connection owns
    /// its single-writer lease. All mutation handlers use this one guard.
    pub fn require_writer(
        &self,
        path: &str,
        conn_id: ConnectionId,
    ) -> Result<FileId, BufferWriterAccessError> {
        let file_id = self
            .file_id_for_path(path)
            .ok_or(BufferWriterAccessError::NotOpen)?;
        if !self.is_writer(file_id, conn_id) {
            return Err(BufferWriterAccessError::NotOwner);
        }
        Ok(file_id)
    }

    fn release_connection_ownership(&mut self, conn_id: ConnectionId) -> Vec<FileId> {
        let owned = self
            .writer_connections
            .iter()
            .filter_map(|(file_id, owner)| (*owner == conn_id).then_some(*file_id))
            .collect::<Vec<_>>();
        for file_id in &owned {
            self.writer_connections.remove(file_id);
        }
        owned
    }

    fn release_writer(&mut self, path: &str, conn_id: ConnectionId) -> Option<FileId> {
        let file_id = self.file_id_for_path(path)?;
        if !self.is_writer(file_id, conn_id) {
            return None;
        }
        self.writer_connections.remove(&file_id);
        Some(file_id)
    }

    /// Remove a connection from all buffer subscription sets.
    /// Returns the list of FileIds that have no remaining connections
    /// (orphaned buffers that should be deallocated).
    pub fn remove_connection(
        &mut self,
        conn_id: ConnectionId,
        ctx: &mut ModelContext<ServerModel>,
    ) -> Vec<FileId> {
        self.remove_connection_pending_requests(conn_id);

        let orphaned = self.release_connection_ownership(conn_id);

        for &file_id in &orphaned {
            self.cleanup_orphaned_if_idle(file_id, ctx);
        }

        orphaned
    }

    /// Remove a single connection from a buffer's subscriber set.
    /// If no connections remain, deallocates the buffer entirely.
    pub fn close_buffer(
        &mut self,
        path: &str,
        conn_id: ConnectionId,
        ctx: &mut ModelContext<ServerModel>,
    ) {
        let Some(file_id) = self.release_writer(path, conn_id) else {
            return;
        };

        // No connection remains. Active mutation state keeps the canonical
        // buffer alive until its real completion event; idle buffers deallocate now.
        self.cleanup_orphaned_if_idle(file_id, ctx);
    }

    // ── Pending request tracking ──────────────────────────────────

    pub fn insert_pending_open(
        &mut self,
        file_id: FileId,
        request_id: RequestId,
        conn_id: ConnectionId,
    ) {
        self.pending_open_requests
            .entry(file_id)
            .or_default()
            .push((request_id, conn_id));
    }

    pub fn take_pending_open(&mut self, file_id: &FileId) -> Vec<(RequestId, ConnectionId)> {
        self.pending_open_requests
            .remove(file_id)
            .unwrap_or_default()
    }

    pub fn fail_open_buffer(
        &mut self,
        file_id: &FileId,
    ) -> (Vec<(RequestId, ConnectionId)>, Vec<PendingBufferMutation>) {
        let pending_opens = self.take_pending_open(file_id);
        let mut pending_mutations = Vec::new();
        if let Some(mut queue) = self.mutation_queues.remove(file_id) {
            if let Some(active) = queue.active.take() {
                pending_mutations.push(active);
            }
            pending_mutations.extend(queue.queued);
        }
        self.writer_connections.remove(file_id);
        self.open_buffers.remove_by_right(file_id);
        self.buffer_handles.remove(file_id);
        (pending_opens, pending_mutations)
    }

    /// Enqueue an ordered mutation. Returns true only when this intent became
    /// the active head and therefore needs to be dispatched by ServerModel.
    pub fn enqueue_mutation(&mut self, file_id: FileId, mutation: PendingBufferMutation) -> bool {
        let queue = self.mutation_queues.entry(file_id).or_default();
        if queue.active.is_none() {
            queue.active = Some(mutation);
            true
        } else {
            queue.queued.push_back(mutation);
            false
        }
    }

    pub fn active_mutation(&self, file_id: &FileId) -> Option<&PendingBufferMutation> {
        self.mutation_queues
            .get(file_id)
            .and_then(|queue| queue.active.as_ref())
    }

    /// Complete exactly one active mutation and promote at most one queued
    /// intent to active. The caller owns dispatching the promoted head.
    pub fn complete_active_mutation(&mut self, file_id: &FileId) -> Option<PendingBufferMutation> {
        let queue = self.mutation_queues.get_mut(file_id)?;
        let completed = queue.active.take()?;
        queue.active = queue.queued.pop_front();
        if queue.active.is_none() {
            self.mutation_queues.remove(file_id);
        }
        Some(completed)
    }

    fn remove_connection_pending_requests(&mut self, conn_id: ConnectionId) {
        for entries in self.pending_open_requests.values_mut() {
            entries.retain(|(_, pending_conn_id)| *pending_conn_id != conn_id);
        }
        self.pending_open_requests
            .retain(|_, entries| !entries.is_empty());

        for queue in self.mutation_queues.values_mut() {
            // Active work already reached the filesystem boundary. Losing its
            // response carrier must not release the serialization slot early.
            queue.queued.retain(|mutation| mutation.conn_id != conn_id);
        }
    }

    pub fn cleanup_orphaned_if_idle(
        &mut self,
        file_id: FileId,
        ctx: &mut ModelContext<ServerModel>,
    ) {
        let has_connections = self.writer_connections.contains_key(&file_id);
        if has_connections || self.mutation_queues.contains_key(&file_id) {
            return;
        }
        self.writer_connections.remove(&file_id);
        self.open_buffers.remove_by_right(&file_id);
        self.pending_open_requests.remove(&file_id);
        self.buffer_handles.remove(&file_id);
        GlobalBufferModel::handle(ctx).update(ctx, |gbm, ctx| gbm.remove(file_id, ctx));
    }
}

#[cfg(test)]
#[path = "server_buffer_tracker_tests.rs"]
mod tests;
