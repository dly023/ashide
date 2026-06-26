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

use super::sftp_backend::SftpBackend;
use super::sftp_ops::{ProgressCallback, SftpOpsError};
use super::types::{FileEntry, FileEntryType};

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
            EnvironmentRuntimeFileKind::Other => FileEntryType::Other,
            EnvironmentRuntimeFileKind::Unspecified => {
                if is_dir {
                    FileEntryType::Directory
                } else {
                    FileEntryType::File
                }
            }
        }
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

        let base = PathBuf::from(&listing.canonical_path);
        let result = listing
            .entries
            .into_iter()
            .map(|entry| {
                let file_type = Self::kind_to_entry_type(entry.kind, entry.is_dir);
                FileEntry {
                    path: base.join(&entry.name),
                    name: entry.name,
                    file_type,
                    size: entry.size_bytes.unwrap_or(0),
                    modified: Self::modified_to_string(entry.modified_epoch_millis),
                    permissions: None,
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

    fn delete_dir_recursive(&self, path: &Path) -> Result<(), SftpOpsError> {
        // The daemon has no recursive-delete RPC; recurse client-side using
        // list_dir + per-entry delete, mirroring the libssh2 backend.
        let entries = self.list_dir(path)?;
        for entry in entries {
            match entry.file_type {
                FileEntryType::Directory => self.delete_dir_recursive(&entry.path)?,
                FileEntryType::File | FileEntryType::Symlink | FileEntryType::Other => {
                    self.delete_file(&entry.path)?;
                }
            }
        }
        // Remove the now-empty directory itself.
        self.delete_file(path)
    }

    fn create_dir(&self, path: &Path) -> Result<(), SftpOpsError> {
        let path_str = Self::path_string(path);
        self.block_on(environment_runtime::create_directory(&self.client, path_str))
            .map_err(SftpOpsError::Operation)
    }

    fn rename(&self, old_path: &Path, new_path: &Path) -> Result<(), SftpOpsError> {
        let from = Self::path_string(old_path);
        let to = Self::path_string(new_path);
        self.block_on(environment_runtime::rename_file(&self.client, from, to))
            .map_err(SftpOpsError::Operation)
    }

    fn realpath(&self, path: &Path) -> Result<PathBuf, SftpOpsError> {
        let path_str = Self::path_string(path);
        let resolved = self
            .block_on(environment_runtime::resolve_path(&self.client, path_str))
            .map_err(SftpOpsError::Operation)?;
        Ok(PathBuf::from(resolved.canonical_path))
    }

    fn stat(&self, path: &Path) -> Result<FileEntry, SftpOpsError> {
        let path_str = Self::path_string(path);
        let resolved = self
            .block_on(environment_runtime::resolve_path(&self.client, path_str))
            .map_err(SftpOpsError::Operation)?;
        let file_type = Self::kind_to_entry_type(resolved.kind, false);
        let canonical = PathBuf::from(&resolved.canonical_path);
        let name = canonical
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        Ok(FileEntry {
            name,
            path: canonical,
            file_type,
            size: resolved.size_bytes.unwrap_or(0),
            modified: None,
            permissions: None,
        })
    }

    fn upload_file(
        &self,
        local_path: &Path,
        remote_path: &Path,
        progress_cb: Option<&ProgressCallback>,
        cancel_flag: Option<&AtomicBool>,
    ) -> Result<(), SftpOpsError> {
        use std::io::Read;

        let mut local_file = std::fs::File::open(local_path)
            .map_err(|e| SftpOpsError::LocalIo(e.to_string()))?;
        let total_size = local_file.metadata().map(|m| m.len()).unwrap_or(0);
        let remote = Self::path_string(remote_path);

        let mut buf = vec![0u8; CHUNK_SIZE as usize];
        let mut offset: u64 = 0;
        let mut first = true;
        loop {
            if cancel_flag.is_some_and(|f| f.load(Ordering::SeqCst)) {
                return Err(SftpOpsError::Cancelled);
            }
            let n = local_file
                .read(&mut buf)
                .map_err(|e| SftpOpsError::LocalIo(e.to_string()))?;
            if n == 0 {
                break;
            }
            let success = self
                .block_on(environment_runtime::write_file_chunk(
                    &self.client,
                    remote.clone(),
                    offset,
                    buf[..n].to_vec(),
                    // Truncate on the first chunk so a re-upload replaces the file.
                    first,
                    None,
                ))
                .map_err(SftpOpsError::Operation)?;
            if success.next_offset <= offset {
                return Err(SftpOpsError::Operation(format!(
                    "remote write made no progress at offset {offset}: {remote}"
                )));
            }
            offset = success.next_offset;
            first = false;
            if let Some(cb) = progress_cb {
                cb(offset, total_size);
            }
        }

        // Empty file: still create/truncate it on the remote.
        if first {
            self.block_on(environment_runtime::write_file_chunk(
                &self.client,
                remote,
                0,
                Vec::new(),
                true,
                None,
            ))
            .map_err(SftpOpsError::Operation)?;
            if let Some(cb) = progress_cb {
                cb(0, total_size);
            }
        }
        Ok(())
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
        // Total size for progress reporting (best-effort).
        let total_size = self
            .block_on(environment_runtime::resolve_path(&self.client, remote.clone()))
            .ok()
            .and_then(|resolved| resolved.size_bytes)
            .unwrap_or(0);

        if let Some(parent) = local_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| SftpOpsError::LocalIo(e.to_string()))?;
        }

        // Write to a temp file, then rename, to avoid truncating an existing
        // local file if the transfer is cancelled or fails.
        let local_display = local_path.display();
        let temp_local_path = PathBuf::from(format!("{local_display}.sftp_partial"));
        let mut local_file = std::fs::File::create(&temp_local_path)
            .map_err(|e| SftpOpsError::LocalIo(e.to_string()))?;

        let result = (|| -> Result<(), SftpOpsError> {
            let mut offset: u64 = 0;
            loop {
                if cancel_flag.is_some_and(|f| f.load(Ordering::SeqCst)) {
                    return Err(SftpOpsError::Cancelled);
                }
                let chunk = self
                    .block_on(environment_runtime::read_file_chunk(
                        &self.client,
                        remote.clone(),
                        offset,
                        CHUNK_SIZE,
                    ))
                    .map_err(SftpOpsError::Operation)?;
                if !chunk.bytes.is_empty() {
                    local_file
                        .write_all(&chunk.bytes)
                        .map_err(|e| SftpOpsError::LocalIo(e.to_string()))?;
                    offset += chunk.bytes.len() as u64;
                    if let Some(cb) = progress_cb {
                        cb(offset, total_size);
                    }
                }
                if chunk.eof {
                    break;
                }
                if chunk.next_offset <= offset && chunk.bytes.is_empty() {
                    return Err(SftpOpsError::Operation(format!(
                        "remote read made no progress at offset {offset}: {remote}"
                    )));
                }
                offset = chunk.next_offset.max(offset);
            }
            local_file
                .flush()
                .map_err(|e| SftpOpsError::LocalIo(e.to_string()))?;
            Ok(())
        })();

        match &result {
            Ok(()) => {
                if let Err(e) = std::fs::rename(&temp_local_path, local_path) {
                    let temp_display = temp_local_path.display();
                    return Err(SftpOpsError::LocalIo(format!(
                        "rename failed: {e}. partial download kept at: {temp_display}"
                    )));
                }
            }
            Err(_) => {
                let _ = std::fs::remove_file(&temp_local_path);
            }
        }
        result
    }
}
