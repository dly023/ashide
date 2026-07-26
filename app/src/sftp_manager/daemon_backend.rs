//! Daemon-backed SFTP backend
//!
//! Implements the [`SftpBackend`] trait over the remote helper daemon's native
//! file RPCs (`list_directory` / `resolve_path` / `create_directory` /
//! `delete_file` / `rename_file` / `read_file_chunk` / `write_file_chunk`),
//! reusing the single ControlMaster SSH connection the daemon already holds for
//! the host. When a daemon helper is present for the host this is preferred over
//! the zero-install libssh2 SFTP backend; otherwise the libssh2 backend remains
//! the fallback (see `browser.rs` backend selection).
//!
//! ## Sync/async bridge
//!
//! The [`SftpBackend`] trait is synchronous, but the daemon
//! [`EnvironmentRuntimeClient`] exposes async request/response methods. The
//! file browser always drives these calls from inside `tokio::task::spawn_blocking`
//! (see `SftpBrowserView::run_blocking`), i.e. on a Tokio blocking thread. We
//! capture a `tokio::runtime::Handle` at construction time and bridge each sync
//! method with `handle.block_on(..)`, which is valid on a blocking thread and
//! never blocks an async worker.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::runtime::Handle;

use crate::workspace::environment_runtime::{
    self, EnvironmentRuntimeClient, EnvironmentRuntimeFileKind,
};

use super::sftp_backend::{
    create_unique_transfer_staging, ensure_safe_transfer_destination, open_sftp_transfer_source,
    SftpBackend,
};
use super::sftp_ops::{ProgressCallback, SftpOpsError};
use super::types::{DeleteDirectoryIdentity, FileEntry, FileEntryType, SymlinkTargetType};

/// Chunk size for streaming uploads/downloads over the daemon RPCs.
const CHUNK_SIZE: u64 = 512 * 1024;

/// Daemon-backed SFTP backend that routes file operations through the remote
/// helper daemon's native file RPCs over the shared ControlMaster connection.
pub struct DaemonSftpBackend {
    client: Arc<EnvironmentRuntimeClient>,
    handle: Handle,
}

impl DaemonSftpBackend {
    /// Creates a daemon backend from a connected runtime client.
    ///
    /// `handle` must be a Tokio runtime handle; the trait's sync methods are
    /// always invoked from `spawn_blocking` threads, so `handle.block_on(..)`
    /// is safe.
    pub fn new(client: Arc<EnvironmentRuntimeClient>, handle: Handle) -> Self {
        Self { client, handle }
    }

    /// Wraps an `Arc<dyn SftpBackend>` for the file browser.
    pub fn into_backend(self) -> Arc<dyn SftpBackend> {
        Arc::new(self)
    }

    /// Bridges an async daemon call onto the current blocking thread.
    fn block_on<F, T>(&self, fut: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        self.handle.block_on(fut)
    }

    fn path_string(path: &Path) -> String {
        // Remote hosts are POSIX; normalize any Windows separators that may have
        // crept in from local PathBuf::join calls.
        path.to_string_lossy().replace('\\', "/")
    }

    fn kind_to_entry_type(kind: EnvironmentRuntimeFileKind, is_dir: bool) -> FileEntryType {
        match kind {
            EnvironmentRuntimeFileKind::Directory => FileEntryType::Directory,
            EnvironmentRuntimeFileKind::File => FileEntryType::File,
            EnvironmentRuntimeFileKind::Symlink => FileEntryType::Symlink,
            EnvironmentRuntimeFileKind::Missing | EnvironmentRuntimeFileKind::Other => {
                FileEntryType::Other
            }
            EnvironmentRuntimeFileKind::Unspecified => {
                if is_dir {
                    FileEntryType::Directory
                } else {
                    FileEntryType::File
                }
            }
        }
    }

    fn symlink_target_type(
        kind: EnvironmentRuntimeFileKind,
        target_kind: EnvironmentRuntimeFileKind,
    ) -> Option<SymlinkTargetType> {
        if kind != EnvironmentRuntimeFileKind::Symlink {
            return None;
        }
        Some(match target_kind {
            EnvironmentRuntimeFileKind::Directory => SymlinkTargetType::Directory,
            EnvironmentRuntimeFileKind::File => SymlinkTargetType::File,
            EnvironmentRuntimeFileKind::Missing => SymlinkTargetType::Missing,
            EnvironmentRuntimeFileKind::Other
            | EnvironmentRuntimeFileKind::Unspecified
            | EnvironmentRuntimeFileKind::Symlink => SymlinkTargetType::Other,
        })
    }

    fn modified_to_string(modified_epoch_millis: Option<u64>) -> Option<String> {
        modified_epoch_millis.and_then(|ms| {
            let secs = (ms / 1000) as i64;
            let nsecs = ((ms % 1000) * 1_000_000) as u32;
            chrono::DateTime::<chrono::Utc>::from_timestamp(secs, nsecs).map(|utc| {
                let local: chrono::DateTime<chrono::Local> = utc.into();
                local.format("%Y-%m-%d %H:%M").to_string()
            })
        })
    }
}

impl SftpBackend for DaemonSftpBackend {
    fn list_dir(&self, path: &Path) -> Result<Vec<FileEntry>, SftpOpsError> {
        let path_str = Self::path_string(path);
        let listing = self
            .block_on(environment_runtime::list_directory(&self.client, path_str))
            .map_err(SftpOpsError::Operation)?;

        // SFTP UI 的条目身份来自调用路径；daemon 返回路径只用于协议校验，
        // 不能用 canonical target 重写符号链接命名空间。
        let base = path.to_path_buf();
        let result = listing
            .entries
            .into_iter()
            .map(|entry| {
                let file_type = Self::kind_to_entry_type(entry.kind, entry.is_dir);
                let symlink_target_type = Self::symlink_target_type(entry.kind, entry.target_kind);
                FileEntry {
                    path: base.join(&entry.name),
                    name: entry.name,
                    file_type,
                    symlink_target_type,
                    size: entry.size_bytes.unwrap_or(0),
                    modified: Self::modified_to_string(entry.modified_epoch_millis),
                    permissions: None,
                    directory_identity: entry.directory_identity.map(|identity| {
                        DeleteDirectoryIdentity {
                            device: identity.device,
                            inode: identity.inode,
                        }
                    }),
                }
            })
            .collect();
        Ok(result)
    }

    fn delete_file(&self, path: &Path) -> Result<(), SftpOpsError> {
        let path_str = Self::path_string(path);
        self.block_on(async {
            self.client
                .delete_file(path_str)
                .await
                .map_err(|e| SftpOpsError::Operation(e.to_string()))
        })
    }

    fn delete_dir_recursive(
        &self,
        path: &Path,
        identity: &DeleteDirectoryIdentity,
    ) -> Result<(), SftpOpsError> {
        let path_str = Self::path_string(path);
        self.block_on(environment_runtime::delete_directory(
            &self.client,
            path_str,
            crate::environment_runtime_transport::proto::DeleteDirectoryIdentity {
                device: identity.device,
                inode: identity.inode,
            },
        ))
        .map_err(SftpOpsError::Operation)
    }

    fn create_dir(&self, path: &Path) -> Result<(), SftpOpsError> {
        let path_str = Self::path_string(path);
        self.block_on(environment_runtime::create_directory(
            &self.client,
            path_str,
        ))
        .map_err(SftpOpsError::Operation)
    }

    fn rename(&self, old_path: &Path, new_path: &Path) -> Result<(), SftpOpsError> {
        let from = Self::path_string(old_path);
        let to = Self::path_string(new_path);
        let committed = self
            .block_on(environment_runtime::exact_rename(
                &self.client,
                from,
                to.clone(),
            ))
            .map_err(SftpOpsError::Operation)?;
        if committed != to {
            return Err(SftpOpsError::Operation(format!(
                "rename committed unexpected path: requested={to}, committed={committed}"
            )));
        }
        Ok(())
    }

    fn realpath(&self, path: &Path) -> Result<PathBuf, SftpOpsError> {
        let path_str = Self::path_string(path);
        let resolved = self
            .block_on(environment_runtime::resolve_path(&self.client, path_str))
            .map_err(SftpOpsError::Operation)?;
        Ok(PathBuf::from(
            resolved.resolved_path.unwrap_or(resolved.path),
        ))
    }

    fn upload_file(
        &self,
        local_path: &Path,
        remote_path: &Path,
        progress_cb: Option<&ProgressCallback>,
        cancel_flag: Option<&AtomicBool>,
    ) -> Result<(), SftpOpsError> {
        use std::io::Read;

        let mut local_file = open_sftp_transfer_source(local_path)
            .map_err(|error| SftpOpsError::LocalIo(error.to_string()))?;
        let total_size = local_file
            .metadata()
            .map_err(|error| SftpOpsError::LocalIo(error.to_string()))?
            .len();
        let remote = Self::path_string(remote_path);
        let mut transfer = self
            .block_on(environment_runtime::begin_write_file_transfer(
                &self.client,
                remote.clone(),
                None,
            ))
            .map_err(SftpOpsError::Operation)?;
        let handle = transfer.handle.clone();

        let result = (|| -> Result<(), SftpOpsError> {
            let mut buffer = vec![0u8; CHUNK_SIZE as usize];
            while transfer.next_offset() < total_size {
                if cancel_flag.is_some_and(|flag| flag.load(Ordering::SeqCst)) {
                    return Err(SftpOpsError::Cancelled);
                }
                let remaining = total_size - transfer.next_offset();
                let budget = buffer.len().min(remaining as usize);
                let read = local_file
                    .read(&mut buffer[..budget])
                    .map_err(|error| SftpOpsError::LocalIo(error.to_string()))?;
                if read == 0 {
                    return Err(SftpOpsError::LocalIo(format!(
                        "local upload source ended before captured size: offset={}, total={total_size}: {}",
                        transfer.next_offset(),
                        local_path.display()
                    )));
                }
                let success = self
                    .block_on(environment_runtime::write_file_chunk(
                        &self.client,
                        handle.clone(),
                        buffer[..read].to_vec(),
                    ))
                    .map_err(SftpOpsError::Operation)?;
                transfer
                    .accept_chunk(read, &success)
                    .map_err(SftpOpsError::Operation)?;
                if let Some(callback) = progress_cb {
                    callback(transfer.next_offset(), total_size);
                }
            }
            let committed = self
                .block_on(environment_runtime::finish_file_transfer(
                    &self.client,
                    handle.clone(),
                ))
                .map_err(SftpOpsError::Operation)?;
            if committed.as_deref() != Some(remote.as_str()) {
                return Err(SftpOpsError::Operation(format!(
                    "upload committed unexpected path: requested={remote}, committed={committed:?}"
                )));
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = self.block_on(environment_runtime::abort_file_transfer(
                &self.client,
                handle,
            ));
        }
        result
    }

    fn download_file(
        &self,
        remote_path: &Path,
        local_path: &Path,
        progress_cb: Option<&ProgressCallback>,
        cancel_flag: Option<&AtomicBool>,
    ) -> Result<(), SftpOpsError> {
        use std::io::Write;

        let remote = Self::path_string(remote_path);
        let mut transfer = self
            .block_on(environment_runtime::begin_read_file_transfer(
                &self.client,
                remote.clone(),
            ))
            .map_err(SftpOpsError::Operation)?;
        let handle = transfer.handle.clone();
        let total_size = transfer.total_size();
        ensure_safe_transfer_destination(local_path)?;
        let mut staging = create_unique_transfer_staging(local_path)
            .map_err(|error| SftpOpsError::LocalIo(error.to_string()))?;
        let mut local_file = staging.take_file();

        let result = (|| -> Result<(), SftpOpsError> {
            loop {
                if cancel_flag.is_some_and(|flag| flag.load(Ordering::SeqCst)) {
                    return Err(SftpOpsError::Cancelled);
                }
                let chunk = self
                    .block_on(environment_runtime::read_file_chunk(
                        &self.client,
                        handle.clone(),
                        CHUNK_SIZE,
                    ))
                    .map_err(SftpOpsError::Operation)?;
                local_file
                    .write_all(&chunk.bytes)
                    .map_err(|error| SftpOpsError::LocalIo(error.to_string()))?;
                transfer
                    .accept_chunk(&chunk)
                    .map_err(SftpOpsError::Operation)?;
                if let Some(callback) = progress_cb {
                    callback(transfer.next_offset(), total_size);
                }
                if chunk.eof {
                    break;
                }
            }
            let committed = self
                .block_on(environment_runtime::finish_file_transfer(
                    &self.client,
                    handle.clone(),
                ))
                .map_err(SftpOpsError::Operation)?;
            if committed.is_some() {
                return Err(SftpOpsError::Operation(format!(
                    "download unexpectedly committed a path: {committed:?}"
                )));
            }
            local_file
                .sync_all()
                .map_err(|error| SftpOpsError::LocalIo(error.to_string()))?;
            drop(local_file);
            staging
                .commit()
                .map_err(|error| SftpOpsError::LocalIo(error.to_string()))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = self.block_on(environment_runtime::abort_file_transfer(
                &self.client,
                handle,
            ));
        }
        result
    }
}
