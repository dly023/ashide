//! SFTP 后端操作抽象层
//!
//! 定义 SftpBackend trait，将 UI 层与传输层解耦。
//! 生产实现为 `daemon_backend::DaemonSftpBackend`（远程 helper daemon 文件 RPC）；
//! `InMemorySftpBackend` 使用本地文件系统用于测试。

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use dunce;

use super::sftp_ops::{ProgressCallback, SftpOpsError};
use super::types::{FileEntry, FileEntryType};

/// SFTP 后端操作抽象，用于解耦 UI 层与协议层
pub trait SftpBackend: Send + Sync {
    /// 列出目录内容，返回文件条目列表
    fn list_dir(&self, path: &Path) -> Result<Vec<FileEntry>, SftpOpsError>;

    /// 删除远程文件
    fn delete_file(&self, path: &Path) -> Result<(), SftpOpsError>;

    /// 递归删除远程目录
    fn delete_dir_recursive(&self, path: &Path) -> Result<(), SftpOpsError>;

    /// 创建远程目录
    fn create_dir(&self, path: &Path) -> Result<(), SftpOpsError>;

    /// 重命名远程文件或目录
    fn rename(&self, old_path: &Path, new_path: &Path) -> Result<(), SftpOpsError>;

    /// 解析真实路径
    fn realpath(&self, path: &Path) -> Result<PathBuf, SftpOpsError>;

    /// 获取文件/目录详情
    fn stat(&self, path: &Path) -> Result<FileEntry, SftpOpsError>;

    /// 流式上传本地文件到远程
    fn upload_file(
        &self,
        local_path: &Path,
        remote_path: &Path,
        progress_cb: Option<&ProgressCallback>,
        cancel_flag: Option<&AtomicBool>,
    ) -> Result<(), SftpOpsError>;

    /// 流式下载远程文件到本地
    fn download_file(
        &self,
        remote_path: &Path,
        local_path: &Path,
        progress_cb: Option<&ProgressCallback>,
        cancel_flag: Option<&AtomicBool>,
    ) -> Result<(), SftpOpsError>;
}

// ============================================================
// InMemorySftpBackend — 基于本地文件系统的测试实现
// ============================================================

/// 基于内存（本地临时目录）的 SFTP 后端，用于测试
pub struct InMemorySftpBackend {
    /// 根目录，模拟远程文件系统的根
    root: PathBuf,
}

impl InMemorySftpBackend {
    /// 创建新的内存后端，使用指定目录作为根
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// 获取根目录路径
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 将"远程"路径映射到本地绝对路径
    ///
    /// 远程路径以 / 开头，映射到 root 下的相对路径。
    fn to_local(&self, remote_path: &Path) -> PathBuf {
        let relative = remote_path.strip_prefix("/").unwrap_or(remote_path);
        self.root.join(relative)
    }

    /// 将本地路径转换为"远程"路径
    fn to_remote(&self, local_path: &Path) -> PathBuf {
        match local_path.strip_prefix(&self.root) {
            Ok(rel) => {
                if rel.as_os_str().is_empty() {
                    PathBuf::from("/")
                } else {
                    PathBuf::from("/").join(rel)
                }
            }
            Err(_) => PathBuf::from("/").join(local_path),
        }
    }

    /// 从 std::fs::Metadata 构建 FileEntry
    fn metadata_to_entry(
        &self,
        name: String,
        local_path: &Path,
        meta: &std::fs::Metadata,
    ) -> FileEntry {
        let file_type = if meta.is_symlink() {
            FileEntryType::Symlink
        } else if meta.is_dir() {
            FileEntryType::Directory
        } else {
            FileEntryType::File
        };
        let modified = meta.modified().ok().map(|t| {
            let datetime: chrono::DateTime<chrono::Local> = t.into();
            datetime.format("%Y-%m-%d %H:%M").to_string()
        });
        FileEntry {
            name,
            path: self.to_remote(local_path),
            file_type,
            size: if meta.is_dir() { 0 } else { meta.len() },
            modified,
            permissions: None,
        }
    }
}

impl SftpBackend for InMemorySftpBackend {
    fn list_dir(&self, path: &Path) -> Result<Vec<FileEntry>, SftpOpsError> {
        let local = self.to_local(path);
        let p = path.display();
        let entries = fs::read_dir(&local)
            .map_err(|e| SftpOpsError::Operation(format!("列出目录失败 {p}: {e}")))?;

        let mut result = Vec::new();
        for entry in entries {
            let entry =
                entry.map_err(|e| SftpOpsError::Operation(format!("读取目录条目失败: {e}")))?;
            let name = entry.file_name().to_string_lossy().to_string();
            // 过滤 . 和 ..
            if name == "." || name == ".." {
                continue;
            }
            let meta = fs::symlink_metadata(entry.path())
                .map_err(|e| SftpOpsError::Operation(format!("读取元数据失败: {e}")))?;
            result.push(self.metadata_to_entry(name, &entry.path(), &meta));
        }

        Ok(result)
    }

    fn delete_file(&self, path: &Path) -> Result<(), SftpOpsError> {
        let local = self.to_local(path);
        let p = path.display();
        fs::remove_file(&local)
            .map_err(|e| SftpOpsError::Operation(format!("删除文件失败 {p}: {e}")))
    }

    fn delete_dir_recursive(&self, path: &Path) -> Result<(), SftpOpsError> {
        let local = self.to_local(path);
        let p = path.display();
        fs::remove_dir_all(&local)
            .map_err(|e| SftpOpsError::Operation(format!("递归删除目录失败 {p}: {e}")))
    }

    fn create_dir(&self, path: &Path) -> Result<(), SftpOpsError> {
        let local = self.to_local(path);
        let p = path.display();
        fs::create_dir(&local)
            .map_err(|e| SftpOpsError::Operation(format!("创建目录失败 {p}: {e}")))
    }

    fn rename(&self, old_path: &Path, new_path: &Path) -> Result<(), SftpOpsError> {
        let old_local = self.to_local(old_path);
        let new_local = self.to_local(new_path);
        fs::rename(&old_local, &new_local).map_err(|e| {
            SftpOpsError::Operation(format!(
                "重命名失败 {} -> {}: {e}",
                old_path.display(),
                new_path.display()
            ))
        })
    }

    fn realpath(&self, path: &Path) -> Result<PathBuf, SftpOpsError> {
        let local = self.to_local(path);
        let p = path.display();
        let canonical = dunce::canonicalize(&local)
            .map_err(|e| SftpOpsError::Operation(format!("解析路径失败 {p}: {e}")))?;
        Ok(self.to_remote(&canonical))
    }

    fn stat(&self, path: &Path) -> Result<FileEntry, SftpOpsError> {
        let local = self.to_local(path);
        let p = path.display();
        let meta = fs::symlink_metadata(&local)
            .map_err(|e| SftpOpsError::Operation(format!("获取文件信息失败 {p}: {e}")))?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        Ok(self.metadata_to_entry(name, &local, &meta))
    }

    fn upload_file(
        &self,
        local_path: &Path,
        remote_path: &Path,
        _progress_cb: Option<&ProgressCallback>,
        _cancel_flag: Option<&AtomicBool>,
    ) -> Result<(), SftpOpsError> {
        let dest = self.to_local(remote_path);
        // 确保父目录存在
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| SftpOpsError::LocalIo(format!("创建目录失败: {e}")))?;
        }
        fs::copy(local_path, &dest)
            .map_err(|e| SftpOpsError::LocalIo(format!("上传文件失败: {e}")))?;
        Ok(())
    }

    fn download_file(
        &self,
        remote_path: &Path,
        local_path: &Path,
        _progress_cb: Option<&ProgressCallback>,
        _cancel_flag: Option<&AtomicBool>,
    ) -> Result<(), SftpOpsError> {
        let src = self.to_local(remote_path);
        // 确保本地父目录存在
        if let Some(parent) = local_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| SftpOpsError::LocalIo(format!("创建目录失败: {e}")))?;
        }
        let mut src_file = fs::File::open(&src)
            .map_err(|e| SftpOpsError::LocalIo(format!("打开远程文件失败: {e}")))?;
        let mut dest_file = fs::File::create(local_path)
            .map_err(|e| SftpOpsError::LocalIo(format!("创建本地文件失败: {e}")))?;

        // 分块复制以模拟流式传输
        const CHUNK_SIZE: usize = 32 * 1024;
        let mut buf = vec![0u8; CHUNK_SIZE];
        loop {
            let n = src_file
                .read(&mut buf)
                .map_err(|e| SftpOpsError::LocalIo(format!("读取失败: {e}")))?;
            if n == 0 {
                break;
            }
            dest_file
                .write_all(&buf[..n])
                .map_err(|e| SftpOpsError::LocalIo(format!("写入失败: {e}")))?;
        }
        dest_file
            .flush()
            .map_err(|e| SftpOpsError::LocalIo(format!("刷新失败: {e}")))?;
        Ok(())
    }
}

/// 创建 Arc<dyn SftpBackend> 的便捷方法
impl InMemorySftpBackend {
    /// 创建并包装为 Arc<dyn SftpBackend>
    pub fn into_backend(self) -> Arc<dyn SftpBackend> {
        Arc::new(self)
    }
}
