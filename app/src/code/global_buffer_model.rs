#![cfg_attr(not(feature = "local_fs"), allow(dead_code))]
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use bimap::BiMap;

use futures_util::stream::AbortHandle;
use string_offset::{ByteOffset, CharOffset};
use warp_core::{features::FeatureFlag, SessionId};
use warp_editor::content::buffer::{Buffer, ToBufferCharOffset};
use warp_editor::content::diff::{text_diff, TextDiff};
use warp_util::content_version::ContentVersion;
use warp_util::file::{FileId, FileLoadError, FileSaveError};
use warpui::{Entity, ModelContext, ModelHandle, SingletonEntity, WeakModelHandle};

use super::buffer_location::{BufferLocation, SyncClock};

cfg_if::cfg_if! {
    if #[cfg(feature = "local_fs")] {
        use warp_files::{FileModelEvent, FileModel};
        use warp_editor::content::text::IndentBehavior;
        use warp_editor::content::text::IndentUnit;
    }
}

/// State for a shared buffer including the file ID and buffer handle.
#[derive(Debug, Clone)]
pub struct BufferState {
    pub file_id: FileId,
    pub buffer: ModelHandle<Buffer>,
}

impl BufferState {
    pub fn new(file_id: FileId, buffer: ModelHandle<Buffer>) -> Self {
        Self { file_id, buffer }
    }
}

/// Tracks an active background diff parsing operation.
struct PendingDiffParse {
    abort_handle: AbortHandle,
}

/// Describes the backing store for a buffer's content.
enum BufferSource {
    /// Backed by the current app filesystem.
    CurrentAppFileSystem {
        base_content_version: Option<ContentVersion>,
    },
    /// Backed by an Environment Runtime filesystem.
    EnvironmentRuntime {
        environment_file_path: super::buffer_location::EnvironmentFilePath,
        /// Stable transport binding selected when the canonical buffer is first opened.
        /// This is deliberately not part of `BufferLocation` identity.
        binding_session_id: SessionId,
        /// `None` while waiting for the `OpenBufferResponse`; `Some` once loaded.
        sync_clock: Option<SyncClock>,
    },
    /// Current-app file managed by the runtime daemon.
    /// Owns the SyncClock for version tracking. Connection tracking
    /// is handled by ServerModel, not here — the buffer is a file-level
    /// concept shared across connections.
    ServerCurrentAppFileSystem {
        sync_clock: SyncClock,
        base_content_version: Option<ContentVersion>,
        staged_server_conflict_resolution: Option<StagedServerConflictResolution>,
    },
}

struct StagedServerConflictResolution {
    acknowledged_server_version: ContentVersion,
    current_client_version: ContentVersion,
    client_content: String,
    save_version: ContentVersion,
}

struct InternalBufferState {
    buffer: WeakModelHandle<Buffer>,
    /// Number of active consumers of this canonical shared buffer.
    consumer_count: usize,
    /// Tracks any active background diff parsing for auto-reload.
    pending_diff_parse: Option<PendingDiffParse>,
    source: BufferSource,
}

impl InternalBufferState {
    /// Returns the base content version for current-app/server-managed buffers,
    /// `None` for Environment Runtime buffers.
    ///
    /// Environment Runtime buffers return `None` because they do not use the file-watcher
    /// auto-reload path. Version tracking for Environment Runtime buffers is handled by
    /// `SyncClock` instead.
    fn base_content_version(&self) -> Option<ContentVersion> {
        match &self.source {
            BufferSource::CurrentAppFileSystem {
                base_content_version,
            }
            | BufferSource::ServerCurrentAppFileSystem {
                base_content_version,
                ..
            } => *base_content_version,
            BufferSource::EnvironmentRuntime { .. } => None,
        }
    }

    /// Sets the base content version. Applicable to current-app and server-managed current-app buffers.
    fn set_base_content_version(&mut self, version: ContentVersion) {
        match &mut self.source {
            BufferSource::CurrentAppFileSystem {
                base_content_version,
            }
            | BufferSource::ServerCurrentAppFileSystem {
                base_content_version,
                ..
            } => {
                *base_content_version = Some(version);
            }
            BufferSource::EnvironmentRuntime { .. } => {}
        }
    }

    /// Whether this buffer has been loaded (has content).
    fn is_loaded(&self) -> bool {
        match &self.source {
            BufferSource::CurrentAppFileSystem {
                base_content_version,
            }
            | BufferSource::ServerCurrentAppFileSystem {
                base_content_version,
                ..
            } => base_content_version.is_some(),
            // Environment Runtime buffers are loaded once the OpenBufferResponse arrives
            // and populates the sync clock.
            BufferSource::EnvironmentRuntime { sync_clock, .. } => sync_clock.is_some(),
        }
    }
}

pub enum GlobalBufferModelEvent {
    BufferLoaded {
        file_id: FileId,
        content_version: ContentVersion,
    },
    FailedToLoad {
        file_id: FileId,
        error: Rc<FileLoadError>,
    },
    BufferUpdatedFromFileEvent {
        file_id: FileId,
        success: bool,
        content_version: ContentVersion,
    },
    FileSaved {
        file_id: FileId,
    },
    FailedToSave {
        file_id: FileId,
        error: Rc<FileSaveError>,
    },
    /// An Environment Runtime buffer update conflicted with current-app edits.
    /// The UI should present a resolution dialog.
    EnvironmentBufferConflict {
        file_id: FileId,
    },
    /// A server-managed current-app buffer was updated from a file-watcher event.
    /// Carries the incremental diff edits for the ServerModel to push
    /// to connected clients as `BufferUpdatedPush`.
    ///
    /// Note: this event is NOT emitted from `apply_client_edit`. See that
    /// method's doc comment for the V0 single-client limitation.
    ServerCurrentAppFileSystemBufferUpdated {
        file_id: FileId,
        /// Incremental edits with 1-indexed character offsets (matching `CharOffset`).
        edits: Vec<CharOffsetEdit>,
        new_server_version: ContentVersion,
        expected_client_version: ContentVersion,
    },
}

impl GlobalBufferModelEvent {
    pub fn file_id(&self) -> FileId {
        match self {
            GlobalBufferModelEvent::BufferLoaded { file_id, .. }
            | GlobalBufferModelEvent::FailedToLoad { file_id, .. }
            | GlobalBufferModelEvent::BufferUpdatedFromFileEvent { file_id, .. }
            | GlobalBufferModelEvent::FileSaved { file_id, .. }
            | GlobalBufferModelEvent::FailedToSave { file_id, .. }
            | GlobalBufferModelEvent::EnvironmentBufferConflict { file_id, .. }
            | GlobalBufferModelEvent::ServerCurrentAppFileSystemBufferUpdated { file_id, .. } => {
                *file_id
            }
        }
    }
}

/// A text edit using 1-indexed character offsets (matching `CharOffset`).
///
/// Used to carry incremental edits in `ServerCurrentAppFileSystemBufferUpdated` events
/// and `handle_buffer_updated_push` without coupling `GlobalBufferModel`
/// to proto types. Offsets use the same 1-indexed coordinate system as
/// the buffer's `CharOffset`, so no conversion is needed at the boundary.
pub struct CharOffsetEdit {
    pub start: CharOffset,
    pub end: CharOffset,
    pub text: String,
}

/// `handle_buffer_updated_push` 的入参集合：把一次 Environment Runtime buffer
/// push 的语义字段收进一个借用的 params struct，避免函数参数过多。
pub struct BufferUpdatedPush<'a> {
    pub session_id: SessionId,
    pub host_id: &'a warp_core::HostId,
    pub path: &'a str,
    pub new_server_version: u64,
    pub expected_client_version: u64,
    pub edits: &'a [CharOffsetEdit],
}

/// Global singleton model for managing shared buffers across editors.
///
/// This allows multiple editors to share the same buffer when editing the same file,
/// enabling consistent content synchronization and more efficient memory usage.
pub struct GlobalBufferModel {
    location_to_id: BiMap<BufferLocation, FileId>,
    buffers: HashMap<FileId, InternalBufferState>,
}

impl GlobalBufferModel {
    pub fn new(_ctx: &mut ModelContext<Self>) -> Self {
        #[cfg(feature = "local_fs")]
        _ctx.subscribe_to_model(&FileModel::handle(_ctx), Self::handle_file_model_events);

        Self {
            location_to_id: BiMap::new(),
            buffers: HashMap::new(),
        }
    }

    /// 客户端 app 专用:订阅 Environment Runtime 的 buffer push 事件,把 runtime
    /// daemon 推来的 buffer update 应用到 current-app buffer。
    ///
    /// 必须由客户端 app 在注册 `GlobalBufferModel` 时显式调用 —— **不能**放进
    /// `new()`:runtime daemon 同样会注册 `GlobalBufferModel`(用于
    /// server-managed current-app buffer 的服务端同步),但 daemon 不注册 Environment Runtime
    /// transport manager,若在 `new()` 里订阅会 panic「never registered」
    /// 导致 daemon 一启动就崩。
    #[cfg(feature = "local_tty")]
    pub fn subscribe_to_environment_runtime_buffer_updates(ctx: &mut ModelContext<Self>) {
        crate::workspace::environment_runtime::subscribe_to_buffer_updates(
            ctx,
            |me, update, ctx| {
                // wire offset 是 1-indexed char offset(对齐 CharOffset)。
                // 用饱和转换避免 32-bit 平台 `as usize` 截断高位;native 64-bit 上等价。
                let char_edits: Vec<_> = update
                    .edits
                    .iter()
                    .map(|e| CharOffsetEdit {
                        start: CharOffset::from(
                            usize::try_from(e.start_offset).unwrap_or(usize::MAX),
                        ),
                        end: CharOffset::from(usize::try_from(e.end_offset).unwrap_or(usize::MAX)),
                        text: e.text.clone(),
                    })
                    .collect();
                me.handle_buffer_updated_push(
                    BufferUpdatedPush {
                        session_id: update.session_id,
                        host_id: &update.host_id,
                        path: &update.path,
                        new_server_version: update.new_server_version,
                        expected_client_version: update.expected_client_version,
                        edits: &char_edits,
                    },
                    ctx,
                );
            },
        );
    }

    /// Scan through all buffers and deallocate any that are no longer in use.
    pub fn remove_deallocated_buffers(&mut self, ctx: &mut ModelContext<Self>) {
        let ids_to_remove: HashSet<FileId> = self
            .buffers
            .iter()
            .filter_map(|(id, state)| {
                if state.buffer.upgrade(ctx).is_none() {
                    Some(*id)
                } else {
                    None
                }
            })
            .collect();

        if ids_to_remove.is_empty() {
            return;
        }

        for id in &ids_to_remove {
            self.location_to_id.remove_by_right(id);
        }

        for id in ids_to_remove {
            self.buffers.remove(&id);

            #[cfg(feature = "local_fs")]
            {
                let file_model = FileModel::handle(ctx);
                file_model.update(ctx, |file_model, ctx| {
                    file_model.cancel(id);
                    file_model.unsubscribe(id, ctx);
                });
            }
        }
    }

    pub fn buffer_loaded(&self, file_id: FileId) -> bool {
        self.buffers
            .get(&file_id)
            .map(|state| state.is_loaded())
            .unwrap_or(false)
    }

    fn cleanup_file_id(&mut self, file_id: FileId, _ctx: &mut ModelContext<Self>) {
        self.location_to_id.remove_by_right(&file_id);

        self.buffers.remove(&file_id);

        #[cfg(feature = "local_fs")]
        {
            let file_model = FileModel::handle(_ctx);
            file_model.update(_ctx, |file_model, ctx| {
                file_model.cancel(file_id);
                file_model.unsubscribe(file_id, ctx);
            });
        }
    }

    /// 主动关闭一个 buffer:从客户端 map 中移除,Environment Runtime buffer 额外发
    /// `CloseBuffer` 让 daemon 释放内存 buffer。
    ///
    /// 不依赖 `WeakHandle` 是否失效——`remove_deallocated_buffers` 仅在 handle
    /// 已被 drop 时清理,而 tab 关闭路径里 buffer 通常仍有强引用(`TabData`
    /// 持有 `CurrentAppCodeEditorView` 间接强引用 `Buffer`)。如果不主动清理,
    /// 关 tab 后 `InternalBufferState` 残留,下次打开同一Environment Runtime文件会走
    /// `open_environment_buffer` 的 "Return existing buffer if already open" 分支
    /// 复用包含未保存编辑的旧 buffer,造成"看着已保存"的假象。
    #[cfg_attr(not(feature = "local_tty"), allow(unused_variables))]
    pub fn close_buffer(&mut self, file_id: FileId, ctx: &mut ModelContext<Self>) {
        let Some(state) = self.buffers.get_mut(&file_id) else {
            return;
        };
        if state.consumer_count > 1 {
            state.consumer_count -= 1;
            return;
        }
        debug_assert_eq!(
            state.consumer_count, 1,
            "tracked buffer must have a consumer"
        );

        // Environment Runtime buffer:发 CloseBuffer 让 daemon 释放内存 buffer。
        #[cfg(feature = "local_tty")]
        if let Some(state) = self.buffers.get(&file_id) {
            if let BufferSource::EnvironmentRuntime {
                environment_file_path,
                binding_session_id,
                ..
            } = &state.source
            {
                let path_str = environment_file_path.path.as_str().to_string();
                if let Some(client) = crate::workspace::environment_runtime::client_for_session(
                    *binding_session_id,
                    ctx,
                ) {
                    client.close_buffer(path_str);
                }
            }
        }

        self.cleanup_file_id(file_id, ctx);
    }

    /// Returns the buffer handle if it is 1) still exists + active 2) loaded.
    fn buffer_handle_for_id(
        &mut self,
        file_id: FileId,
        ctx: &mut ModelContext<Self>,
    ) -> Option<ModelHandle<Buffer>> {
        let state = self.buffers.get(&file_id)?;

        // If the buffer hasn't been loaded yet, don't return a model handle.
        if !state.is_loaded() {
            log::info!("Cannot return handle for unloaded buffers");
            return None;
        }

        match state.buffer.upgrade(ctx) {
            Some(handle) => Some(handle),
            None => {
                // Clean up deallocated buffers.
                self.cleanup_file_id(file_id, ctx);
                None
            }
        }
    }

    /// Once we finish reading the file's content from the disk, populate the buffer with the content.
    /// For initial load (is_loaded_from_file_system == true), this is synchronous.
    /// For auto-reload (is_loaded_from_file_system == false), this spawns a background task for diff computation.
    fn populate_buffer_with_read_content(
        &mut self,
        file_id: FileId,
        content: &str,
        base_version: ContentVersion,
        new_version: ContentVersion,
        is_initial_load: bool,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(state) = self.buffers.get_mut(&file_id) else {
            return;
        };

        let Some(buffer) = state.buffer.upgrade(ctx) else {
            self.cleanup_file_id(file_id, ctx);
            log::warn!("Cannot populate buffer with content due to deallocated model handle");
            return;
        };

        if is_initial_load {
            // Initial load: use synchronous replace_all since there's nothing to preserve
            buffer.update(ctx, |buffer, ctx| {
                buffer.replace_all(content, ctx);
                buffer.set_version(new_version);
            });

            state.set_base_content_version(new_version);

            ctx.emit(GlobalBufferModelEvent::BufferLoaded {
                file_id,
                content_version: new_version,
            });
        } else if FeatureFlag::IncrementalAutoReload.is_enabled() {
            // Auto-reload: spawn background task for diff computation
            Self::start_background_diff_parse(
                file_id,
                state,
                buffer,
                content,
                base_version,
                new_version,
                ctx,
            );
        } else {
            // Fallback: synchronous replace_all (non-incremental)
            buffer.update(ctx, |buffer, ctx| {
                buffer.replace_all(content, ctx);
                buffer.set_version(new_version);
            });

            state.set_base_content_version(new_version);

            ctx.emit(GlobalBufferModelEvent::BufferUpdatedFromFileEvent {
                file_id,
                success: true,
                content_version: new_version,
            });
        }
    }

    /// Spawns a background task to compute the diff between current buffer content and new content.
    /// On completion, applies the diff edits to the buffer.
    fn start_background_diff_parse(
        file_id: FileId,
        state: &mut InternalBufferState,
        buffer: ModelHandle<Buffer>,
        new_content: &str,
        base_version: ContentVersion,
        new_version: ContentVersion,
        ctx: &mut ModelContext<Self>,
    ) {
        // Abort any existing diff parse for this file
        if let Some(pending) = state.pending_diff_parse.take() {
            pending.abort_handle.abort();
        }

        // Move owned strings to the background thread
        let old_text = buffer.as_ref(ctx).text().into_string();
        let new_content_owned = new_content.to_string();

        let handle = ctx.spawn(
            async move { text_diff(&old_text, &new_content_owned).await },
            move |me, diff: TextDiff, ctx| {
                me.apply_diff_result(file_id, diff, base_version, new_version, ctx);
            },
        );

        // Store the abort handle so we can cancel if a newer update arrives
        state.pending_diff_parse = Some(PendingDiffParse {
            abort_handle: handle.abort_handle(),
        });
    }

    /// Called when background diff parsing completes. Applies the diff edits to the buffer.
    fn apply_diff_result(
        &mut self,
        file_id: FileId,
        diff: TextDiff,
        base_version: ContentVersion,
        new_version: ContentVersion,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(state) = self.buffers.get_mut(&file_id) else {
            return;
        };

        // Clear the pending diff parse state
        state.pending_diff_parse = None;

        let Some(buffer) = state.buffer.upgrade(ctx) else {
            self.cleanup_file_id(file_id, ctx);
            return;
        };

        // Verify the buffer still matches the expected base version.
        // This also correctly handles the case where a client edit arrives
        // during the background diff parse: apply_client_edit modifies the
        // buffer version, so this check will fail and we discard the stale
        // diff rather than incorrectly bumping the server version.
        if !buffer.as_ref(ctx).version_match(&base_version) {
            log::info!("Buffer version changed during diff parsing, aborting apply");
            ctx.emit(GlobalBufferModelEvent::BufferUpdatedFromFileEvent {
                file_id,
                success: false,
                content_version: base_version,
            });
            return;
        }

        let use_server_current_app_file_system = matches!(
            state.source,
            BufferSource::ServerCurrentAppFileSystem { .. }
        );

        // For server-managed current-app buffers, convert byte-range edits to 1-indexed
        // char-offset edits BEFORE applying the diff, because the byte
        // ranges in diff.edits reference the old (pre-edit) buffer content.
        // Uses the buffer's native byte→char offset conversion.
        let char_offset_edits: Option<Vec<CharOffsetEdit>> = if use_server_current_app_file_system {
            let buffer_ref = buffer.as_ref(ctx);
            Some(
                diff.edits
                    .iter()
                    .map(|(range, text)| {
                        // +1: 0-indexed text byte offset → 1-indexed buffer byte offset
                        let start =
                            ByteOffset::from(range.start + 1).to_buffer_char_offset(buffer_ref);
                        let end = ByteOffset::from(range.end + 1).to_buffer_char_offset(buffer_ref);
                        CharOffsetEdit {
                            start,
                            end,
                            text: text.clone(),
                        }
                    })
                    .collect(),
            )
        } else {
            None
        };

        // Apply the diff edits
        buffer.update(ctx, |buffer, ctx| {
            if diff.is_empty() {
                // No actual changes to content, but still need to update version
                buffer.set_version(new_version);
                return;
            }
            let char_edits = diff.to_char_offset_edits(buffer);
            buffer.insert_at_char_offset_ranges(char_edits, new_version, ctx);
        });

        state.set_base_content_version(new_version);

        if let Some(char_offset_edits) = char_offset_edits {
            if let BufferSource::ServerCurrentAppFileSystem { sync_clock, .. } = &mut state.source {
                let new_sv = sync_clock.bump_server();
                ctx.emit(
                    GlobalBufferModelEvent::ServerCurrentAppFileSystemBufferUpdated {
                        file_id,
                        edits: char_offset_edits,
                        new_server_version: new_sv,
                        expected_client_version: sync_clock.client_version,
                    },
                );
            }
        } else {
            ctx.emit(GlobalBufferModelEvent::BufferUpdatedFromFileEvent {
                file_id,
                success: true,
                content_version: new_version,
            });
        }
    }

    #[cfg(feature = "local_fs")]
    fn handle_file_model_events(&mut self, event: &FileModelEvent, ctx: &mut ModelContext<Self>) {
        match event {
            FileModelEvent::FileLoaded {
                content,
                id,
                version,
            } => {
                // For initial load, base_version and new_version are the same
                self.populate_buffer_with_read_content(*id, content, *version, *version, true, ctx);
            }
            FileModelEvent::FailedToLoad { id, error } => {
                ctx.emit(GlobalBufferModelEvent::FailedToLoad {
                    file_id: *id,
                    error: error.clone(),
                });
            }
            FileModelEvent::FileUpdated {
                id,
                content,
                base_version,
                new_version,
            } => {
                if let Some(buffer) = self.buffer_handle_for_id(*id, ctx) {
                    if buffer.as_ref(ctx).version_match(base_version) {
                        self.populate_buffer_with_read_content(
                            *id,
                            content,
                            *base_version,
                            *new_version,
                            false,
                            ctx,
                        );
                    } else {
                        // Buffer version doesn't match the event's base_version.
                        // Check if the buffer has no user edits (matches our internal
                        // base_content_version). If so, it's safe to start a fresh
                        // diff parse from the actual buffer version to the new content.
                        let internal_base_version = self
                            .buffers
                            .get(id)
                            .and_then(|state| state.base_content_version());
                        let has_no_user_edits = internal_base_version
                            .is_some_and(|v| buffer.as_ref(ctx).version_match(&v));

                        if has_no_user_edits {
                            // No user edits: safe to reload from the actual buffer
                            // version. This handles both:
                            log::info!(
                                "Starting fresh diff parse for file update (no user edits, \
                                 internal base {:?}, event base {:?})",
                                internal_base_version,
                                *base_version
                            );
                            let actual_version = buffer.as_ref(ctx).version();
                            self.populate_buffer_with_read_content(
                                *id,
                                content,
                                actual_version,
                                *new_version,
                                false,
                                ctx,
                            );
                        } else {
                            log::info!("Not updating global buffer due to version conflict");

                            // Abort any pending diff parse since the buffer has
                            // user edits that we must not overwrite.
                            if let Some(state) = self.buffers.get_mut(id) {
                                if let Some(pending) = state.pending_diff_parse.take() {
                                    pending.abort_handle.abort();
                                }
                            }

                            if internal_base_version != Some(*base_version) {
                                log::warn!(
                                    "Internal global buffer base version {:?} mismatches file model base version {:?}",
                                    internal_base_version,
                                    *base_version
                                );
                            }

                            ctx.emit(GlobalBufferModelEvent::BufferUpdatedFromFileEvent {
                                file_id: *id,
                                success: false,
                                content_version: *base_version,
                            });
                        }
                    }
                }
            }
            FileModelEvent::FileSaved { id, version } => {
                // FileSaved 是 server conflict resolution 的唯一 commit boundary。
                // save initiation 期间 canonical buffer/clock 保持最后已提交状态；只有
                // 与 staging 的 save_version 对应的事件才能原子发布 staged 内容。
                let staged_resolution = self.buffers.get_mut(id).and_then(|state| {
                    let BufferSource::ServerCurrentAppFileSystem {
                        staged_server_conflict_resolution,
                        ..
                    } = &mut state.source
                    else {
                        return None;
                    };
                    let staged = staged_server_conflict_resolution.take()?;
                    if staged.save_version == *version {
                        Some(staged)
                    } else {
                        log::error!(
                            "Ignoring FileSaved with version {version:?} while conflict resolution expects {:?}",
                            staged.save_version
                        );
                        *staged_server_conflict_resolution = Some(staged);
                        None
                    }
                });

                let has_pending_resolution = self.buffers.get(id).is_some_and(|state| {
                    matches!(
                        &state.source,
                        BufferSource::ServerCurrentAppFileSystem {
                            staged_server_conflict_resolution: Some(_),
                            ..
                        }
                    )
                });
                if staged_resolution.is_none() && has_pending_resolution {
                    // A different save operation must not complete the active conflict
                    // resolution mutation. Keep the staged transaction pending and fail
                    // closed instead of emitting FileSaved to ServerModel.
                    return;
                }

                if let Some(staged) = staged_resolution {
                    if let Some(state) = self.buffers.get_mut(id) {
                        let BufferSource::ServerCurrentAppFileSystem {
                            sync_clock,
                            base_content_version,
                            ..
                        } = &mut state.source
                        else {
                            unreachable!("staging only exists for server current-app buffers");
                        };
                        sync_clock.server_version = staged.acknowledged_server_version;
                        sync_clock.client_version = staged.current_client_version;
                        *base_content_version = Some(*version);

                        if let Some(buffer) = state.buffer.upgrade(ctx) {
                            buffer.update(ctx, |buffer, ctx| {
                                buffer.replace_all(&staged.client_content, ctx);
                                buffer.set_version(staged.save_version);
                            });
                        }
                    }
                } else if let Some(state) = self.buffers.get_mut(id) {
                    // Normal current-app save path.
                    state.set_base_content_version(*version);
                }
                ctx.emit(GlobalBufferModelEvent::FileSaved { file_id: *id });
            }
            FileModelEvent::FailedToSave { id, error } => {
                if let Some(state) = self.buffers.get_mut(id) {
                    if let BufferSource::ServerCurrentAppFileSystem {
                        staged_server_conflict_resolution,
                        ..
                    } = &mut state.source
                    {
                        staged_server_conflict_resolution.take();
                    }
                }
                ctx.emit(GlobalBufferModelEvent::FailedToSave {
                    file_id: *id,
                    error: error.clone(),
                });
            }
        }
    }

    /// Save the content of a tracked buffer to disk via FileModel.
    #[cfg(feature = "local_fs")]
    pub fn save(
        &self,
        file_id: FileId,
        content: String,
        version: ContentVersion,
        ctx: &mut ModelContext<Self>,
    ) -> Result<(), FileSaveError> {
        FileModel::handle(ctx).update(ctx, |file_model, ctx| {
            file_model.save(file_id, content, version, ctx)
        })
    }

    /// Rename a file and save its content via FileModel.
    #[cfg(feature = "local_fs")]
    pub fn rename_and_save(
        &self,
        file_id: FileId,
        new_path: PathBuf,
        content: String,
        version: ContentVersion,
        ctx: &mut ModelContext<Self>,
    ) -> Result<(), FileSaveError> {
        FileModel::handle(ctx).update(ctx, |file_model, ctx| {
            file_model.rename_and_save(file_id, new_path, content, version, ctx)
        })
    }

    /// Delete a file via FileModel.
    #[cfg(feature = "local_fs")]
    pub fn delete(
        &self,
        file_id: FileId,
        version: ContentVersion,
        ctx: &mut ModelContext<Self>,
    ) -> Result<(), FileSaveError> {
        FileModel::handle(ctx).update(ctx, |file_model, ctx| {
            file_model.delete(file_id, version, ctx)
        })
    }

    /// Remove a tracked buffer, cleaning up FileModel state.
    /// Used when a new file is deleted before ever being saved to a permanent location.
    pub fn remove(&mut self, file_id: FileId, ctx: &mut ModelContext<Self>) {
        self.cleanup_file_id(file_id, ctx);
    }

    /// Look up the file path for a tracked current-app file-system buffer.
    pub fn file_path(&self, file_id: FileId) -> Option<&Path> {
        match self.location_to_id.get_by_right(&file_id) {
            Some(BufferLocation::CurrentAppFileSystem(path)) => Some(path.as_path()),
            _ => None,
        }
    }

    /// Get the base content version (last known on-disk version) for a tracked buffer.
    pub fn base_version(&self, file_id: FileId) -> Option<ContentVersion> {
        self.buffers
            .get(&file_id)
            .and_then(|state| state.base_content_version())
    }

    /// Discard any in progress changes and reload the buffer with the canonical version from the file system.
    #[cfg(feature = "local_fs")]
    pub fn discard_unsaved_changes(&mut self, path: &Path, ctx: &mut ModelContext<Self>) {
        if let Some(id) = self
            .location_to_id
            .get_by_left(&BufferLocation::CurrentAppFileSystem(path.to_path_buf()))
            .cloned()
        {
            let path_clone = path.to_path_buf();
            ctx.spawn(
                async move { FileModel::read_content_for_file(&path_clone).await },
                move |me, content, ctx| match content {
                    Ok(content) => {
                        // Consider this reload as a "new" version. This prevents any race condition when there is another
                        // auto-reload while we are reading out the latest content.
                        let new_version = ContentVersion::new();
                        // For discard, we get the current base version from the buffer state
                        let base_version = me
                            .buffers
                            .get(&id)
                            .and_then(|state| {
                                state.buffer.upgrade(ctx).map(|b| b.as_ref(ctx).version())
                            })
                            .unwrap_or(new_version);
                        FileModel::handle(ctx).update(ctx, |file_model, _ctx| {
                            file_model.set_version(id, new_version);
                        });
                        me.populate_buffer_with_read_content(
                            id,
                            &content,
                            base_version,
                            new_version,
                            false,
                            ctx,
                        );
                    }
                    Err(e) => ctx.emit(GlobalBufferModelEvent::FailedToLoad {
                        file_id: id,
                        error: e.into(),
                    }),
                },
            );
        }
    }

    /// Remap an existing buffer from `old_file_id` to a new path, preserving the buffer
    /// content and unsaved edits. Re-registers the new path with FileModel.
    ///
    /// Used for file rename.
    #[cfg(feature = "local_fs")]
    pub fn rename(
        &mut self,
        old_file_id: FileId,
        new_path: PathBuf,
        ctx: &mut ModelContext<Self>,
    ) -> Option<BufferState> {
        let old_state = self.buffers.remove(&old_file_id)?;
        let buffer = old_state.buffer.upgrade(ctx)?;

        self.location_to_id.remove_by_right(&old_file_id);

        // Cancel + unsubscribe old FileId from FileModel.
        let file_model = FileModel::handle(ctx);
        file_model.update(ctx, |file_model, ctx| {
            file_model.cancel(old_file_id);
            file_model.unsubscribe(old_file_id, ctx);
        });

        Some(self.register_buffer_for_path(new_path, buffer, old_state.base_content_version(), ctx))
    }

    /// Adopt an existing buffer under a new path without reading from disk.
    /// Used by `save_as` to register a newly-created file with GlobalBufferModel.
    #[cfg(feature = "local_fs")]
    pub fn register(
        &mut self,
        path: PathBuf,
        buffer: ModelHandle<Buffer>,
        ctx: &mut ModelContext<Self>,
    ) -> BufferState {
        let buffer_version = buffer.as_ref(ctx).version();
        self.register_buffer_for_path(path, buffer, Some(buffer_version), ctx)
    }

    /// Shared helper: register `buffer` under `path` with FileModel and store internal state.
    /// LSP 下线后不再试图跟 LSP 同步 buffer 变更。
    #[cfg(feature = "local_fs")]
    fn register_buffer_for_path(
        &mut self,
        path: PathBuf,
        buffer: ModelHandle<Buffer>,
        base_content_version: Option<ContentVersion>,
        ctx: &mut ModelContext<Self>,
    ) -> BufferState {
        // If a buffer is already registered for this path, clean up the old entry
        // to avoid orphaning the previous FileId in `self.buffers`.
        if let Some(old_file_id) = self
            .location_to_id
            .get_by_left(&BufferLocation::CurrentAppFileSystem(path.clone()))
            .copied()
        {
            self.cleanup_file_id(old_file_id, ctx);
        }

        let buffer_version = buffer.as_ref(ctx).version();
        let file_id = FileModel::handle(ctx).update(ctx, |file_model, ctx| {
            let id = file_model.register_file_path(&path, true, ctx);
            file_model.set_version(id, buffer_version);
            id
        });

        self.location_to_id
            .insert(BufferLocation::CurrentAppFileSystem(path.clone()), file_id);
        self.buffers.insert(
            file_id,
            InternalBufferState {
                buffer: buffer.downgrade(),
                consumer_count: 1,
                pending_diff_parse: None,
                source: BufferSource::CurrentAppFileSystem {
                    base_content_version,
                },
            },
        );

        BufferState::new(file_id, buffer)
    }

    /// Open a buffer at the given location.
    ///
    /// Dispatches to the appropriate private opener based on the location variant.
    /// If a buffer already exists for this location and is loaded, returns the
    /// existing `BufferState`.
    #[cfg(feature = "local_fs")]
    pub fn open_current_app(&mut self, path: PathBuf, ctx: &mut ModelContext<Self>) -> BufferState {
        self.open_current_app_file_system(path, false, ctx)
    }

    /// Open a current-app file-system buffer for the given file path.
    ///
    /// If a buffer already exists for this path and is loaded, returns the existing BufferState.
    /// If no buffer exists, creates a new Buffer and BufferState using FileModel.
    /// File system updates are automatically subscribed to for all buffers.
    ///
    /// When `use_server_current_app_file_system` is true, the buffer is created with a `ServerCurrentAppFileSystem`
    /// source (with a `SyncClock`) instead of a plain current-app file source.
    #[cfg(feature = "local_fs")]
    fn open_current_app_file_system(
        &mut self,
        path: PathBuf,
        use_server_current_app_file_system: bool,
        ctx: &mut ModelContext<Self>,
    ) -> BufferState {
        if let Some(id) = self
            .location_to_id
            .get_by_left(&BufferLocation::CurrentAppFileSystem(path.clone()))
            .cloned()
        {
            debug_assert!(self.buffers.contains_key(&id));
            if let Some(state) = self.buffers.get_mut(&id) {
                if let Some(handle) = state.buffer.upgrade(ctx) {
                    state.consumer_count += 1;
                    // Only emit buffer loaded if the base content version is set.
                    if state.is_loaded() {
                        ctx.emit(GlobalBufferModelEvent::BufferLoaded {
                            file_id: id,
                            content_version: handle.as_ref(ctx).version(),
                        });
                    }
                    return BufferState::new(id, handle);
                }
            }
        }

        self.create_new_buffer(&path, use_server_current_app_file_system, ctx)
    }

    #[cfg(feature = "local_fs")]
    fn create_new_buffer(
        &mut self,
        path: &Path,
        use_server_current_app_file_system: bool,
        ctx: &mut ModelContext<Self>,
    ) -> BufferState {
        // Open file through FileModel to get FileId
        // Always subscribe to updates for GlobalBufferModel created buffers
        let file_id =
            FileModel::handle(ctx).update(ctx, |file_model, ctx| file_model.open(path, true, ctx));

        // Create new buffer
        let buffer = ctx.add_model(|_| {
            // This sets the default indentation behavior. The editor will override this if it can load the grammar config
            // for the given file path.
            Buffer::new(Box::new(|_, _| {
                IndentBehavior::TabIndent(IndentUnit::Space(4))
            }))
        });

        self.location_to_id.insert(
            BufferLocation::CurrentAppFileSystem(path.to_path_buf()),
            file_id,
        );
        let source = if use_server_current_app_file_system {
            BufferSource::ServerCurrentAppFileSystem {
                sync_clock: SyncClock::new(),
                base_content_version: None,
                staged_server_conflict_resolution: None,
            }
        } else {
            BufferSource::CurrentAppFileSystem {
                base_content_version: None,
            }
        };
        self.buffers.insert(
            file_id,
            InternalBufferState {
                buffer: buffer.downgrade(),
                consumer_count: 1,
                pending_diff_parse: None,
                source,
            },
        );

        BufferState::new(file_id, buffer)
    }

    // ── Environment Runtime buffer operations (client side) ─────────────

    /// Open an Environment Runtime buffer identified by an `EnvironmentFilePath`.
    ///
    /// Sends `OpenBuffer` to the environment runtime, creates a current-app `Buffer` model,
    /// and sets up bidirectional sync via `BufferEvent` → `BufferEdit`.
    ///
    /// Returns a `BufferState` immediately (buffer content is populated asynchronously).
    #[cfg_attr(not(feature = "local_tty"), allow(unused_variables, unused_mut))]
    pub fn open_environment_buffer(
        &mut self,
        environment_file_path: super::buffer_location::EnvironmentFilePath,
        binding_session_id: SessionId,
        ctx: &mut ModelContext<Self>,
    ) -> BufferState {
        let location = BufferLocation::EnvironmentRuntime(environment_file_path.clone());

        // Return existing buffer if already open.
        if let Some(id) = self.location_to_id.get_by_left(&location).cloned() {
            if let Some(state) = self.buffers.get_mut(&id) {
                if let Some(handle) = state.buffer.upgrade(ctx) {
                    state.consumer_count += 1;
                    if state.is_loaded() {
                        ctx.emit(GlobalBufferModelEvent::BufferLoaded {
                            file_id: id,
                            content_version: handle.as_ref(ctx).version(),
                        });
                    }
                    return BufferState::new(id, handle);
                }
            }
        }

        let file_id = FileId::new();
        let buffer = ctx.add_model(|_| Buffer::default());

        // Store state with sync_clock = None (set to Some on OpenBufferResponse).
        self.location_to_id.insert(location, file_id);
        self.buffers.insert(
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

        #[cfg(feature = "local_tty")]
        {
            use warp_editor::content::buffer::BufferEvent;

            // Extract fields before moving environment_file_path into the buffer source.
            let path_str = environment_file_path.path.as_str().to_string();
            // Subscribe to buffer content changes so edits are sent back to the daemon.
            let path_for_edit = path_str.clone();
            ctx.subscribe_to_model(&buffer, move |me, event, ctx| {
                    if let BufferEvent::ContentChanged { delta, origin, .. } = event {
                        // Skip server-originated changes to prevent echo loop.
                        // Server pushes applied via insert_at_char_offset_ranges
                        // emit ContentChanged with SystemEdit origin.
                        if !origin.from_user() {
                            return;
                        }

                        // Look up the sync clock to get the expected server version
                        // and bump the client version.
                        let Some(state) = me.buffers.get_mut(&file_id) else {
                            return;
                        };
                        let BufferSource::EnvironmentRuntime {
                            binding_session_id,
                            sync_clock,
                            ..
                        } = &mut state.source else {
                            return;
                        };
                        let Some(sync_clock) = sync_clock.as_mut() else {
                            return;
                        };
                        let expected_sv = sync_clock.server_version.as_u64();
                        let new_cv = ContentVersion::new();
                        sync_clock.client_version = new_cv;
                        let binding_session_id = *binding_session_id;

                        // Build incremental edits from the ContentChanged delta.
                        let Some(buffer) = state.buffer.upgrade(ctx) else {
                            return;
                        };
                        let edits: Vec<crate::workspace::environment_runtime::EnvironmentRuntimeBufferEdit> = delta
                            .precise_deltas
                            .iter()
                            .map(|d| {
                                // Wire offsets are 1-indexed (matching CharOffset).
                                let text = buffer
                                    .as_ref(ctx)
                                    .text_in_range(d.resolved_range.clone())
                                    .into_string();
                                crate::workspace::environment_runtime::EnvironmentRuntimeBufferEdit {
                                    start_offset: d.replaced_range.start.as_usize() as u64,
                                    end_offset: d.replaced_range.end.as_usize() as u64,
                                    text,
                                }
                            })
                            .collect();
                        // 投递失败说明连接已死,daemon 收不到这次编辑而 current-app
                        // buffer 已推进 —— 标记为冲突,触发 UI 重新同步。
                        let Some(client) = crate::workspace::environment_runtime::client_for_session(
                            binding_session_id,
                            ctx,
                        ) else {
                            log::error!(
                                "Environment Runtime session {binding_session_id:?} disconnected while editing {path_for_edit}"
                            );
                            ctx.emit(GlobalBufferModelEvent::EnvironmentBufferConflict { file_id });
                            return;
                        };
                        if let Err(e) = crate::workspace::environment_runtime::send_buffer_edit(
                            &client,
                            path_for_edit.clone(),
                            expected_sv,
                            new_cv.as_u64(),
                            edits,
                        ) {
                            log::error!(
                                "Failed to send Environment Runtime buffer edit for {path_for_edit}: {e}"
                            );
                            ctx.emit(GlobalBufferModelEvent::EnvironmentBufferConflict { file_id });
                        }
                    }
            });

            // Materialization always completes asynchronously, including the
            // disconnected case. This gives every consumer time to install its
            // model/view subscriptions before Loaded or FailedToLoad is emitted.
            let client =
                crate::workspace::environment_runtime::client_for_session(binding_session_id, ctx);

            ctx.spawn(
                async move {
                    let Some(client) = client else {
                        return Err(format!(
                            "No environment runtime client for bound session {binding_session_id:?}"
                        ));
                    };
                    crate::workspace::environment_runtime::open_buffer(&client, path_str)
                        .await
                        .map(|response| (response.content, response.server_version))
                        .map_err(|error| error.to_string())
                },
                move |me, result, ctx| {
                    me.finish_environment_buffer_materialization(file_id, result, ctx)
                },
            );
        }

        BufferState::new(file_id, buffer)
    }

    fn finish_environment_buffer_materialization(
        &mut self,
        file_id: FileId,
        result: Result<(String, u64), String>,
        ctx: &mut ModelContext<Self>,
    ) {
        match result {
            Ok((content, server_version)) => {
                let Some(state) = self.buffers.get_mut(&file_id) else {
                    return;
                };
                let BufferSource::EnvironmentRuntime { sync_clock, .. } = &mut state.source else {
                    return;
                };
                *sync_clock = Some(SyncClock::from_wire(server_version, 0));
                let Some(buffer) = state.buffer.upgrade(ctx) else {
                    return;
                };
                let version = ContentVersion::new();
                buffer.update(ctx, |buffer, ctx| {
                    buffer.replace_all(&content, ctx);
                    buffer.set_version(version);
                });
                ctx.emit(GlobalBufferModelEvent::BufferLoaded {
                    file_id,
                    content_version: version,
                });
            }
            Err(error) => {
                if !self.buffers.contains_key(&file_id) {
                    return;
                }
                log::warn!("Failed to open Environment Runtime buffer: {error}");
                // 清理失败的 file_id,使后续重试能重新发送 OpenBuffer。
                self.cleanup_file_id(file_id, ctx);
                ctx.emit(GlobalBufferModelEvent::FailedToLoad {
                    file_id,
                    error: Rc::new(FileLoadError::DoesNotExist),
                });
            }
        }
    }

    /// Handle an incoming `BufferUpdatedPush` from the environment runtime.
    ///
    /// Accepts incremental edits (1-indexed char offsets matching `CharOffset`)
    /// and applies them to the in-memory buffer via `insert_at_char_offset_ranges`.
    /// If the expected client version doesn't match, a conflict event is emitted.
    #[cfg_attr(not(feature = "local_tty"), allow(dead_code))]
    pub fn handle_buffer_updated_push(
        &mut self,
        push: BufferUpdatedPush,
        ctx: &mut ModelContext<Self>,
    ) {
        let BufferUpdatedPush {
            session_id,
            host_id,
            path,
            new_server_version,
            expected_client_version,
            edits,
        } = push;
        // Find the buffer by scanning for an Environment Runtime source with matching host+path.
        let file_id = self.buffers.iter().find_map(|(id, state)| {
            if let BufferSource::EnvironmentRuntime {
                environment_file_path,
                binding_session_id,
                ..
            } = &state.source
            {
                if *binding_session_id == session_id
                    && environment_file_path.host_id == *host_id
                    && environment_file_path.path.as_str() == path
                {
                    return Some(*id);
                }
            }
            None
        });

        let Some(file_id) = file_id else {
            log::warn!("BufferUpdatedPush for unknown Environment Runtime buffer: {path}");
            return;
        };

        let Some(state) = self.buffers.get_mut(&file_id) else {
            return;
        };

        let BufferSource::EnvironmentRuntime { sync_clock, .. } = &mut state.source else {
            return;
        };
        let Some(sync_clock) = sync_clock.as_mut() else {
            return;
        };

        let expected_cv = ContentVersion::from_wire_u64(expected_client_version);
        if sync_clock.server_push_matches(expected_cv) {
            // Accept the update — apply edits incrementally.
            sync_clock.server_version = ContentVersion::from_wire_u64(new_server_version);

            let Some(buffer) = state.buffer.upgrade(ctx) else {
                return;
            };

            let new_version = ContentVersion::new();
            buffer.update(ctx, |buffer, ctx| {
                let max_offset = buffer.max_charoffset();
                let char_edits: Vec<(std::ops::Range<CharOffset>, String)> = edits
                    .iter()
                    .map(|edit| {
                        let start = std::cmp::min(edit.start, max_offset);
                        let end = std::cmp::min(edit.end, max_offset);
                        (start..end, edit.text.clone())
                    })
                    .collect();

                buffer.insert_at_char_offset_ranges(char_edits, new_version, ctx);
            });
        } else {
            // Conflict — client edits diverged from server.
            log::info!(
                "Environment Runtime buffer conflict for {path}: expected C={expected_client_version}, \
                 client C={:?}",
                sync_clock.client_version
            );
            ctx.emit(GlobalBufferModelEvent::EnvironmentBufferConflict { file_id });
        }
    }

    // ── Server current-app buffer operations (daemon side) ────────────────

    /// Open a server-managed current-app buffer for the given file path on the daemon.
    ///
    /// Delegates to `open_current_app_file_system` with `use_server_current_app_file_system = true` so the buffer
    /// is created directly with a `ServerCurrentAppFileSystem` source and `SyncClock`.
    #[cfg(feature = "local_fs")]
    pub fn open_server_current_app(
        &mut self,
        path: PathBuf,
        ctx: &mut ModelContext<Self>,
    ) -> BufferState {
        self.open_current_app_file_system(path, true, ctx)
    }

    /// Apply a client edit to a server-managed current-app buffer.
    ///
    /// If `expected_server_version` matches the buffer's current server version,
    /// the edits are applied to the in-memory buffer (no disk write) and the
    /// client version is updated. Returns `true` if accepted, `false` if rejected
    /// (stale edit — silently discarded, per `BufferEdit` proto spec).
    ///
    /// V0 limitation (single-client per buffer):
    /// this intentionally does NOT emit `ServerCurrentAppFileSystemBufferUpdated`. That event
    /// would broadcast the edit to every other connection that has the buffer
    /// open, but `SyncClock.client_version` is daemon-wide rather than
    /// per-connection, so there is no safe `expected_client_version` to put
    /// in a `BufferUpdatedPush` targeted at peer connection C (its
    /// `client_version` is independent of A's). Until `SyncClock` becomes
    /// per-connection, only one client should hold a writable view of a
    /// given Environment Runtime buffer at a time.
    ///
    /// TODO(environment-runtime, multi-client): make `SyncClock.client_version` a
    /// `HashMap<ConnectionId, ContentVersion>` and forward A's edits to
    /// peers with the per-peer expected `client_version`.
    #[cfg(feature = "local_fs")]
    pub fn apply_client_edit(
        &mut self,
        file_id: FileId,
        edits: &[CharOffsetEdit],
        expected_server_version: ContentVersion,
        new_client_version: ContentVersion,
        ctx: &mut ModelContext<Self>,
    ) -> bool {
        let Some(state) = self.buffers.get_mut(&file_id) else {
            return false;
        };

        let BufferSource::ServerCurrentAppFileSystem {
            sync_clock,
            staged_server_conflict_resolution,
            ..
        } = &mut state.source
        else {
            return false;
        };

        if staged_server_conflict_resolution.is_some() {
            log::warn!("Rejected client edit while conflict resolution is pending");
            return false;
        }

        if !sync_clock.client_edit_matches(expected_server_version) {
            log::info!(
                "Rejected client edit: expected S={:?}, actual S={:?}",
                expected_server_version,
                sync_clock.server_version
            );
            return false;
        }

        sync_clock.client_version = new_client_version;

        let Some(buffer) = state.buffer.upgrade(ctx) else {
            return false;
        };

        // Wire offsets are 1-indexed (matching CharOffset), so no conversion needed.
        let new_version = ContentVersion::new();
        buffer.update(ctx, |buffer, ctx| {
            let max_offset = buffer.max_charoffset();
            // wire offset 饱和转换 + clamp 到 buffer 末尾,双重防御。
            let char_edits: Vec<(std::ops::Range<CharOffset>, String)> = edits
                .iter()
                .map(|edit| {
                    let start = edit.start.min(max_offset);
                    let end = edit.end.min(max_offset);
                    (start..end, edit.text.clone())
                })
                .collect();

            buffer.insert_at_char_offset_ranges(char_edits, new_version, ctx);
        });

        true
    }

    /// Save a server-managed current-app buffer to disk.
    #[cfg(feature = "local_fs")]
    pub fn save_server_current_app(
        &mut self,
        file_id: FileId,
        ctx: &mut ModelContext<Self>,
    ) -> Result<(), FileSaveError> {
        let Some(state) = self.buffers.get(&file_id) else {
            return Err(FileSaveError::RemoteError("Buffer not found".to_string()));
        };
        if matches!(
            &state.source,
            BufferSource::ServerCurrentAppFileSystem {
                staged_server_conflict_resolution: Some(_),
                ..
            }
        ) {
            return Err(FileSaveError::RemoteError(
                "Conflict resolution save already pending".to_string(),
            ));
        }
        let Some(buffer) = state.buffer.upgrade(ctx) else {
            return Err(FileSaveError::RemoteError("Buffer deallocated".to_string()));
        };
        let content = buffer.as_ref(ctx).text().into_string();
        // 使用 buffer 当前版本,避免与 daemon 的版本同步错位。
        let version = buffer.as_ref(ctx).version();
        FileModel::handle(ctx).update(ctx, |file_model, ctx| {
            file_model.save(file_id, content, version, ctx)
        })
    }

    /// Resolve a conflict by accepting the client's content.
    /// Replaces the buffer content, updates the sync clock, and saves to disk.
    #[cfg(feature = "local_fs")]
    pub fn resolve_conflict(
        &mut self,
        file_id: FileId,
        acknowledged_server_version: ContentVersion,
        current_client_version: ContentVersion,
        client_content: &str,
        ctx: &mut ModelContext<Self>,
    ) -> Result<(), FileSaveError> {
        let Some(state) = self.buffers.get_mut(&file_id) else {
            return Err(FileSaveError::RemoteError("Buffer not found".to_string()));
        };

        let BufferSource::ServerCurrentAppFileSystem {
            sync_clock,
            staged_server_conflict_resolution,
            ..
        } = &mut state.source
        else {
            return Err(FileSaveError::RemoteError(
                "Buffer is not server-managed current-app".to_string(),
            ));
        };

        // 拒绝过期或重叠的冲突解决。mutation queue 正常情况下会保证单 active，
        // 这里仍由 canonical owner fail closed，避免其它调用面绕过队列。
        if sync_clock.server_version != acknowledged_server_version {
            return Err(FileSaveError::RemoteError(
                "Stale conflict resolution".to_string(),
            ));
        }
        if staged_server_conflict_resolution.is_some() {
            return Err(FileSaveError::RemoteError(
                "Conflict resolution already pending".to_string(),
            ));
        }
        if state.buffer.upgrade(ctx).is_none() {
            return Err(FileSaveError::RemoteError("Buffer deallocated".to_string()));
        }

        let save_version = ContentVersion::new();
        *staged_server_conflict_resolution = Some(StagedServerConflictResolution {
            acknowledged_server_version,
            current_client_version,
            client_content: client_content.to_string(),
            save_version,
        });

        let save_result = FileModel::handle(ctx).update(ctx, |file_model, ctx| {
            file_model.save(file_id, client_content.to_string(), save_version, ctx)
        });
        if save_result.is_err() {
            let Some(state) = self.buffers.get_mut(&file_id) else {
                return save_result;
            };
            let BufferSource::ServerCurrentAppFileSystem {
                staged_server_conflict_resolution,
                ..
            } = &mut state.source
            else {
                return save_result;
            };
            staged_server_conflict_resolution.take();
        }
        save_result
    }

    // ── Public accessors ──────────────────────────────────────────────

    /// Returns the buffer text content for a given `FileId`.
    pub fn content_for_file(&self, file_id: FileId, ctx: &warpui::AppContext) -> Option<String> {
        let state = self.buffers.get(&file_id)?;
        let buffer = state.buffer.upgrade(ctx)?;
        Some(buffer.as_ref(ctx).text().into_string())
    }

    /// Returns a reference to the `SyncClock` for a server-managed current-app buffer.
    pub fn sync_clock_for_server_current_app(&self, file_id: FileId) -> Option<&SyncClock> {
        let state = self.buffers.get(&file_id)?;
        match &state.source {
            BufferSource::ServerCurrentAppFileSystem { sync_clock, .. } => Some(sync_clock),
            BufferSource::CurrentAppFileSystem { .. } | BufferSource::EnvironmentRuntime { .. } => {
                None
            }
        }
    }
    /// 该 buffer 是否由 Environment Runtime 文件系统提供。
    ///
    /// 编辑器保存时用它判断:Environment Runtime 文件不能走当前 app 的 `FileModel`(无 current-app 路径,
    /// 会得到 `NoFilePath`),必须走 buffer-sync 的 `SaveBuffer` 协议。
    #[cfg(feature = "local_tty")]
    pub fn is_environment_runtime_buffer(&self, file_id: FileId) -> bool {
        self.buffers
            .get(&file_id)
            .is_some_and(|state| matches!(state.source, BufferSource::EnvironmentRuntime { .. }))
    }

    /// 客户端:把 Environment Runtime buffer 的当前内容持久化到 daemon 端磁盘。
    ///
    /// daemon 的内存 buffer 已经通过 `BufferEdit`(见 `open_environment_buffer` 里对
    /// `ContentChanged` 的订阅)实时同步过用户编辑,这里只需发一个 `SaveBuffer`
    /// 触发 daemon 落盘。请求成功后 emit `FileSaved`,让编辑器/标签更新已保存状态。
    #[cfg(feature = "local_tty")]
    pub fn save_environment_buffer(&self, file_id: FileId, ctx: &mut ModelContext<Self>) {
        let Some(BufferLocation::EnvironmentRuntime(environment_file_path)) =
            self.location_to_id.get_by_right(&file_id).cloned()
        else {
            log::warn!(
                "save_environment_buffer: file_id {file_id:?} 不是 Environment Runtime buffer"
            );
            return;
        };
        let path_str = environment_file_path.path.as_str().to_string();
        let Some(binding_session_id) = self.buffers.get(&file_id).and_then(|state| {
            let BufferSource::EnvironmentRuntime {
                binding_session_id, ..
            } = &state.source
            else {
                return None;
            };
            Some(*binding_session_id)
        }) else {
            return;
        };

        let Some(client) =
            crate::workspace::environment_runtime::client_for_session(binding_session_id, ctx)
        else {
            log::warn!(
                "save_environment_buffer: bound session {binding_session_id:?} 无 environment runtime client"
            );
            // 通知编辑器保存失败,避免停留在虚假的“已保存”状态。
            ctx.emit(GlobalBufferModelEvent::FailedToSave {
                file_id,
                error: Rc::new(FileSaveError::RemoteError(format!(
                    "Environment session {binding_session_id:?} is not connected"
                ))),
            });
            return;
        };

        ctx.spawn(
            async move {
                crate::workspace::environment_runtime::save_buffer(&client, path_str)
                    .await
                    .map_err(|e| format!("{e}"))
            },
            move |_me, result, ctx| match result {
                Ok(response) => {
                    match response {
                        crate::workspace::environment_runtime::EnvironmentRuntimeSaveBufferResponse::Saved => {
                            ctx.emit(GlobalBufferModelEvent::FileSaved { file_id });
                        }
                        crate::workspace::environment_runtime::EnvironmentRuntimeSaveBufferResponse::Failed(message) => {
                            // 把Environment Runtime保存失败上抛给编辑器,显示失败提示。
                            ctx.emit(GlobalBufferModelEvent::FailedToSave {
                                file_id,
                                error: Rc::new(FileSaveError::RemoteError(message)),
                            });
                        }
                    }
                }
                Err(error) => {
                    // 传输/协议错误同样上抛给编辑器。
                    ctx.emit(GlobalBufferModelEvent::FailedToSave {
                        file_id,
                        error: Rc::new(FileSaveError::RemoteError(format!(
                            "SaveBuffer request failed: {error}"
                        ))),
                    });
                }
            },
        );
    }
}

impl Entity for GlobalBufferModel {
    type Event = GlobalBufferModelEvent;
}

impl SingletonEntity for GlobalBufferModel {}

#[cfg(test)]
#[path = "global_buffer_model_tests.rs"]
mod tests;
