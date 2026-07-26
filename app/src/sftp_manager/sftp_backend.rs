//! SFTP 后端操作抽象层
//!
//! 定义 SftpBackend trait，将 UI 层与传输层解耦。
//! 生产实现为 `daemon_backend::DaemonSftpBackend`（远程 helper daemon 文件 RPC）；
//! `InMemorySftpBackend` 使用本地文件系统用于测试。

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

#[cfg(any(test, feature = "integration_tests"))]
use dunce;
#[cfg(any(test, feature = "integration_tests"))]
use std::fs;
#[cfg(any(test, feature = "integration_tests"))]
use std::io::{Read, Write};

use super::sftp_ops::{ProgressCallback, SftpOpsError};
#[cfg(any(test, feature = "integration_tests"))]
use super::types::SymlinkTargetType;
use super::types::{DeleteDirectoryIdentity, FileEntry, FileEntryType};

pub(super) fn ensure_transferable_entry_kind(
    file_type: FileEntryType,
    path: &Path,
) -> Result<(), SftpOpsError> {
    if file_type == FileEntryType::File {
        return Ok(());
    }
    Err(SftpOpsError::Operation(format!(
        "symbolic links and non-regular files cannot be transferred: {}",
        path.display()
    )))
}

pub(super) fn ensure_transferable_local_file(path: &Path) -> Result<u64, SftpOpsError> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| SftpOpsError::LocalIo(error.to_string()))?;
    let file_type = if metadata.file_type().is_symlink() {
        FileEntryType::Symlink
    } else if metadata.is_file() {
        FileEntryType::File
    } else if metadata.is_dir() {
        FileEntryType::Directory
    } else {
        FileEntryType::Other
    };
    ensure_transferable_entry_kind(file_type, path)?;
    Ok(metadata.len())
}

pub(super) fn ensure_safe_transfer_destination(path: &Path) -> Result<(), SftpOpsError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            let file_type = if metadata.file_type().is_symlink() {
                FileEntryType::Symlink
            } else if metadata.is_file() {
                FileEntryType::File
            } else if metadata.is_dir() {
                FileEntryType::Directory
            } else {
                FileEntryType::Other
            };
            ensure_transferable_entry_kind(file_type, path)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(SftpOpsError::LocalIo(error.to_string())),
    }
}

/// Opens the actual transfer source through a component-by-component nofollow walk.
pub(crate) fn open_sftp_transfer_source(path: &Path) -> std::io::Result<std::fs::File> {
    #[cfg(unix)]
    {
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
            return Err(std::io::Error::last_os_error());
        }
        let file = unsafe { std::fs::File::from_raw_fd(fd) };
        if !file.metadata()?.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "transfer source is not a regular file",
            ));
        }
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        reject_existing_symlink_components(path)?;
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "transfer source is not a regular lexical file",
            ));
        }
        std::fs::File::open(path)
    }
}

pub(crate) fn create_sftp_transfer_directory(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        open_transfer_directory_fd(path, true).map(drop)
    }
    #[cfg(not(unix))]
    {
        reject_existing_symlink_components(path)?;
        std::fs::create_dir_all(path)?;
        reject_existing_symlink_components(path)
    }
}

pub(crate) struct UniqueTransferStaging {
    file: Option<std::fs::File>,
    staging_path: PathBuf,
    #[cfg(not(unix))]
    final_path: PathBuf,
    identity: (u64, u64),
    #[cfg(unix)]
    parent: std::os::fd::OwnedFd,
    #[cfg(unix)]
    staging_name: std::ffi::CString,
    #[cfg(unix)]
    final_name: std::ffi::CString,
    committed: bool,
}

impl UniqueTransferStaging {
    pub(crate) fn take_file(&mut self) -> std::fs::File {
        self.file
            .take()
            .expect("transfer staging file already taken")
    }

    pub(crate) fn commit(mut self) -> std::io::Result<()> {
        self.file.take();
        ensure_staging_identity(&self.staging_path, self.identity)?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            ensure_replaceable_transfer_destination(&self.parent, &self.final_name)?;
            if unsafe {
                libc::renameat(
                    self.parent.as_raw_fd(),
                    self.staging_name.as_ptr(),
                    self.parent.as_raw_fd(),
                    self.final_name.as_ptr(),
                )
            } < 0
            {
                return Err(std::io::Error::last_os_error());
            }
        }
        #[cfg(not(unix))]
        std::fs::rename(&self.staging_path, &self.final_path)?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for UniqueTransferStaging {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        if ensure_staging_identity(&self.staging_path, self.identity).is_ok() {
            let _ = std::fs::remove_file(&self.staging_path);
        }
    }
}

/// Creates one operation-owned O_EXCL staging file in the final directory.
pub(crate) fn create_unique_transfer_staging(
    final_path: &Path,
) -> std::io::Result<UniqueTransferStaging> {
    #[cfg(unix)]
    {
        use std::os::fd::{AsRawFd, FromRawFd};
        use std::os::unix::fs::MetadataExt;

        let (parent_path, final_name) = transfer_parent_and_name(final_path)?;
        let parent = open_transfer_directory_fd(parent_path, true)?;
        ensure_replaceable_transfer_destination(&parent, &final_name)?;
        for _ in 0..64 {
            let staging_name = std::ffi::CString::new(format!(
                ".ashide-transfer-{}.staging",
                uuid::Uuid::new_v4()
            ))
            .expect("uuid staging name cannot contain NUL");
            let fd = unsafe {
                libc::openat(
                    parent.as_raw_fd(),
                    staging_name.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_CLOEXEC
                        | libc::O_NOFOLLOW,
                    0o600,
                )
            };
            if fd >= 0 {
                let file = unsafe { std::fs::File::from_raw_fd(fd) };
                let metadata = file.metadata()?;
                let staging_path = parent_path.join(staging_name.to_string_lossy().into_owned());
                return Ok(UniqueTransferStaging {
                    file: Some(file),
                    staging_path,
                    identity: (metadata.dev(), metadata.ino()),
                    parent,
                    staging_name,
                    final_name,
                    committed: false,
                });
            }
            if std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists {
                return Err(std::io::Error::last_os_error());
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not allocate unique transfer staging file",
        ))
    }
    #[cfg(not(unix))]
    {
        if let Some(parent) = final_path.parent() {
            std::fs::create_dir_all(parent)?;
            reject_existing_symlink_components(parent)?;
        }
        for _ in 0..64 {
            let staging_path = final_path
                .with_file_name(format!(".ashide-transfer-{}.staging", uuid::Uuid::new_v4()));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&staging_path)
            {
                Ok(file) => {
                    let identity = windows_file_identity(&file)?;
                    return Ok(UniqueTransferStaging {
                        file: Some(file),
                        staging_path,
                        final_path: final_path.to_path_buf(),
                        identity,
                        committed: false,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not allocate unique transfer staging file",
        ))
    }
}

#[cfg(unix)]
fn ensure_replaceable_transfer_destination(
    parent: &impl std::os::fd::AsRawFd,
    final_name: &std::ffi::CStr,
) -> std::io::Result<()> {
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
        let error = std::io::Error::last_os_error();
        return if error.kind() == std::io::ErrorKind::NotFound {
            Ok(())
        } else {
            Err(error)
        };
    }
    let stat = unsafe { stat.assume_init() };
    if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "transfer destination is not a replaceable regular file",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn ensure_staging_identity(path: &Path, expected: (u64, u64)) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || (metadata.dev(), metadata.ino()) != expected {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "transfer staging identity changed",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn windows_file_identity(file: &std::fs::File) -> std::io::Result<(u64, u64)> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };

    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut information) }
        .map_err(std::io::Error::other)?;
    Ok((
        information.dwVolumeSerialNumber as u64,
        ((information.nFileIndexHigh as u64) << 32) | information.nFileIndexLow as u64,
    ))
}

#[cfg(not(unix))]
fn ensure_staging_identity(path: &Path, expected: (u64, u64)) -> std::io::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;

    reject_existing_symlink_components(path)?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)?;
    let actual = windows_file_identity(&file)?;
    if actual != expected {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "transfer staging identity changed",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn transfer_parent_and_name(path: &Path) -> std::io::Result<(&Path, std::ffi::CString)> {
    use std::os::unix::ffi::OsStrExt;
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "transfer path has no parent",
        )
    })?;
    let name = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "transfer path has no file name",
        )
    })?;
    Ok((
        parent,
        std::ffi::CString::new(name.as_bytes()).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "path contains NUL")
        })?,
    ))
}

#[cfg(unix)]
fn open_transfer_directory_fd(
    path: &Path,
    create_missing: bool,
) -> std::io::Result<std::os::fd::OwnedFd> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Component;

    let initial = std::ffi::CString::new(if path.is_absolute() { "/" } else { "." }).unwrap();
    let fd = unsafe {
        libc::open(
            initial.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut directory = unsafe { OwnedFd::from_raw_fd(fd) };
    for component in path.components() {
        let name = match component {
            Component::RootDir | Component::CurDir => continue,
            Component::Normal(name) => std::ffi::CString::new(name.as_bytes()).map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "path contains NUL")
            })?,
            Component::ParentDir | Component::Prefix(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "path contains forbidden component",
                ));
            }
        };
        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW;
        let mut next = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        if next < 0
            && create_missing
            && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound
        {
            if unsafe { libc::mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o755) } < 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
            {
                return Err(std::io::Error::last_os_error());
            }
            next = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        }
        if next < 0 {
            return Err(std::io::Error::last_os_error());
        }
        directory = unsafe { OwnedFd::from_raw_fd(next) };
    }
    Ok(directory)
}

#[cfg(all(unix, any(test, feature = "integration_tests")))]
fn recursive_delete_directory_fd(directory: &impl std::os::fd::AsRawFd) -> std::io::Result<()> {
    use std::ffi::CStr;
    use std::os::fd::{FromRawFd, OwnedFd};

    let duplicate = unsafe { libc::dup(directory.as_raw_fd()) };
    if duplicate < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let stream = unsafe { libc::fdopendir(duplicate) };
    if stream.is_null() {
        unsafe { libc::close(duplicate) };
        return Err(std::io::Error::last_os_error());
    }
    let result = (|| loop {
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            return Ok(());
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
            return Err(std::io::Error::last_os_error());
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
                return Err(std::io::Error::last_os_error());
            }
            let child = unsafe { OwnedFd::from_raw_fd(child) };
            recursive_delete_directory_fd(&child)?;
            if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) }
                < 0
            {
                return Err(std::io::Error::last_os_error());
            }
        } else if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
    })();
    unsafe { libc::closedir(stream) };
    result
}

#[cfg(all(unix, any(test, feature = "integration_tests")))]
fn delete_directory_identity_bound(
    path: &Path,
    expected: &DeleteDirectoryIdentity,
) -> std::io::Result<()> {
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
        return Err(std::io::Error::last_os_error());
    }
    let directory = unsafe { OwnedFd::from_raw_fd(fd) };
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(directory.as_raw_fd(), stat.as_mut_ptr()) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let stat = unsafe { stat.assume_init() };
    if (stat.st_dev as u64, stat.st_ino as u64) != (expected.device, expected.inode) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "directory identity changed before recursive delete",
        ));
    }
    recursive_delete_directory_fd(&directory)?;
    if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(all(not(unix), any(test, feature = "integration_tests")))]
fn delete_directory_identity_bound(
    _path: &Path,
    _expected: &DeleteDirectoryIdentity,
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "identity-bound recursive delete requires directory descriptors",
    ))
}

#[cfg(not(unix))]
fn reject_existing_symlink_components(path: &Path) -> std::io::Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "transfer path contains a symlink component",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// SFTP 后端操作抽象，用于解耦 UI 层与协议层
pub trait SftpBackend: Send + Sync {
    /// 列出目录内容，返回文件条目列表
    fn list_dir(&self, path: &Path) -> Result<Vec<FileEntry>, SftpOpsError>;

    /// 删除远程文件
    fn delete_file(&self, path: &Path) -> Result<(), SftpOpsError>;

    /// 递归删除远程目录
    fn delete_dir_recursive(
        &self,
        path: &Path,
        identity: &DeleteDirectoryIdentity,
    ) -> Result<(), SftpOpsError>;

    /// 创建远程目录
    fn create_dir(&self, path: &Path) -> Result<(), SftpOpsError>;

    /// 重命名远程文件或目录
    fn rename(&self, old_path: &Path, new_path: &Path) -> Result<(), SftpOpsError>;

    /// 解析真实路径
    fn realpath(&self, path: &Path) -> Result<PathBuf, SftpOpsError>;

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
#[cfg(any(test, feature = "integration_tests"))]
pub struct InMemorySftpBackend {
    /// 根目录，模拟远程文件系统的根
    root: PathBuf,
}

#[cfg(any(test, feature = "integration_tests"))]
impl InMemorySftpBackend {
    /// 创建新的内存后端，使用指定目录作为根
    pub fn new(root: PathBuf) -> Self {
        let root = fs::canonicalize(&root).expect("SFTP test backend root must already exist");
        Self { root }
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
    ) -> Result<FileEntry, SftpOpsError> {
        let file_type = if meta.is_symlink() {
            FileEntryType::Symlink
        } else if meta.is_dir() {
            FileEntryType::Directory
        } else {
            FileEntryType::File
        };
        let symlink_target_type = if meta.file_type().is_symlink() {
            match fs::metadata(local_path) {
                Ok(target) if target.is_dir() => Some(SymlinkTargetType::Directory),
                Ok(target) if target.is_file() => Some(SymlinkTargetType::File),
                Ok(_) => Some(SymlinkTargetType::Other),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    Some(SymlinkTargetType::Missing)
                }
                Err(error) => {
                    return Err(SftpOpsError::Operation(format!(
                        "读取符号链接目标元数据失败 {}: {error}",
                        local_path.display()
                    )));
                }
            }
        } else {
            None
        };
        let modified = Some(
            meta.modified()
                .map_err(|error| {
                    SftpOpsError::Operation(format!(
                        "读取修改时间失败 {}: {error}",
                        local_path.display()
                    ))
                })
                .map(|t| {
                    let datetime: chrono::DateTime<chrono::Local> = t.into();
                    datetime.format("%Y-%m-%d %H:%M").to_string()
                })?,
        );
        #[cfg(unix)]
        let directory_identity = if meta.is_dir() {
            use std::os::unix::fs::MetadataExt;
            Some(DeleteDirectoryIdentity {
                device: meta.dev(),
                inode: meta.ino(),
            })
        } else {
            None
        };
        #[cfg(not(unix))]
        let directory_identity = None;
        Ok(FileEntry {
            name,
            path: self.to_remote(local_path),
            file_type,
            symlink_target_type,
            size: if meta.is_dir() { 0 } else { meta.len() },
            modified,
            permissions: None,
            directory_identity,
        })
    }
}

#[cfg(any(test, feature = "integration_tests"))]
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
            result.push(self.metadata_to_entry(name, &entry.path(), &meta)?);
        }

        Ok(result)
    }

    fn delete_file(&self, path: &Path) -> Result<(), SftpOpsError> {
        let local = self.to_local(path);
        let p = path.display();
        fs::remove_file(&local)
            .map_err(|e| SftpOpsError::Operation(format!("删除文件失败 {p}: {e}")))
    }

    fn delete_dir_recursive(
        &self,
        path: &Path,
        identity: &DeleteDirectoryIdentity,
    ) -> Result<(), SftpOpsError> {
        let local = self.to_local(path);
        let p = path.display();
        delete_directory_identity_bound(&local, identity)
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

    fn upload_file(
        &self,
        local_path: &Path,
        remote_path: &Path,
        _progress_cb: Option<&ProgressCallback>,
        _cancel_flag: Option<&AtomicBool>,
    ) -> Result<(), SftpOpsError> {
        let dest = self.to_local(remote_path);
        ensure_transferable_local_file(local_path)?;
        let mut source = open_sftp_transfer_source(local_path)
            .map_err(|e| SftpOpsError::LocalIo(e.to_string()))?;
        ensure_safe_transfer_destination(&dest)?;
        // 确保父目录存在
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| SftpOpsError::LocalIo(format!("创建目录失败: {e}")))?;
        }
        let mut staging = create_unique_transfer_staging(&dest)
            .map_err(|e| SftpOpsError::LocalIo(format!("创建上传 staging 失败: {e}")))?;
        let mut output = staging.take_file();
        std::io::copy(&mut source, &mut output)
            .map_err(|e| SftpOpsError::LocalIo(format!("上传文件失败: {e}")))?;
        output
            .sync_all()
            .map_err(|e| SftpOpsError::LocalIo(format!("刷新上传 staging 失败: {e}")))?;
        drop(output);
        staging
            .commit()
            .map_err(|e| SftpOpsError::LocalIo(format!("提交上传 staging 失败: {e}")))?;
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
        ensure_transferable_local_file(&src)?;
        ensure_safe_transfer_destination(local_path)?;
        // 确保本地父目录存在
        if let Some(parent) = local_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| SftpOpsError::LocalIo(format!("创建目录失败: {e}")))?;
        }
        let mut src_file = open_sftp_transfer_source(&src)
            .map_err(|e| SftpOpsError::LocalIo(format!("打开远程文件失败: {e}")))?;
        let mut staging = create_unique_transfer_staging(local_path)
            .map_err(|e| SftpOpsError::LocalIo(format!("创建下载 staging 失败: {e}")))?;
        let mut dest_file = staging.take_file();

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
            .sync_all()
            .map_err(|e| SftpOpsError::LocalIo(format!("刷新失败: {e}")))?;
        drop(dest_file);
        staging
            .commit()
            .map_err(|e| SftpOpsError::LocalIo(format!("提交下载 staging 失败: {e}")))?;
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;
    use crate::sftp_manager::types::{FileEntryType, SymlinkTargetType};

    fn canonical_temp_root(dir: &tempfile::TempDir) -> PathBuf {
        fs::canonicalize(dir.path()).unwrap()
    }

    #[test]
    fn symlink_metadata_preserves_entry_and_target_types() {
        let root = tempfile::tempdir().unwrap();
        let target_dir = root.path().join("target-dir");
        fs::create_dir(&target_dir).unwrap();
        let target_file = root.path().join("target-file");
        fs::write(&target_file, "hello").unwrap();
        symlink(&target_dir, root.path().join("dir-link")).unwrap();
        symlink(&target_file, root.path().join("file-link")).unwrap();
        symlink(root.path().join("missing"), root.path().join("broken-link")).unwrap();
        let backend = InMemorySftpBackend::new(root.path().to_path_buf());

        let entries = backend.list_dir(Path::new("/")).unwrap();
        let dir_link = entries
            .iter()
            .find(|entry| entry.name == "dir-link")
            .unwrap();
        assert_eq!(dir_link.file_type, FileEntryType::Symlink);
        assert_eq!(
            dir_link.symlink_target_type,
            Some(SymlinkTargetType::Directory)
        );
        assert!(dir_link.is_directory_like());
        assert!(!dir_link.is_directory_entry());

        let file_link = entries
            .iter()
            .find(|entry| entry.name == "file-link")
            .unwrap();
        assert_eq!(file_link.file_type, FileEntryType::Symlink);
        assert_eq!(file_link.symlink_target_type, Some(SymlinkTargetType::File));
        assert!(!file_link.is_transferable());

        let broken_link = entries
            .iter()
            .find(|entry| entry.name == "broken-link")
            .unwrap();
        assert_eq!(broken_link.file_type, FileEntryType::Symlink);
        assert_eq!(
            broken_link.symlink_target_type,
            Some(SymlinkTargetType::Missing)
        );
        assert!(!broken_link.is_directory_like());
        assert!(!broken_link.is_transferable());
    }

    #[test]
    fn deleting_symlink_to_directory_does_not_delete_target() {
        let root = tempfile::tempdir().unwrap();
        let target_dir = root.path().join("target-dir");
        fs::create_dir(&target_dir).unwrap();
        fs::write(target_dir.join("keep.txt"), "keep").unwrap();
        symlink(&target_dir, root.path().join("dir-link")).unwrap();
        let backend = InMemorySftpBackend::new(root.path().to_path_buf());

        backend.delete_file(Path::new("/dir-link")).unwrap();

        assert!(!root.path().join("dir-link").exists());
        assert!(target_dir.join("keep.txt").exists());
    }

    #[test]
    fn sftp_backend_rejects_symlink_upload_and_download() {
        let root = tempfile::tempdir().unwrap();
        let root_path = canonical_temp_root(&root);
        let backend = InMemorySftpBackend::new(root_path.clone());

        let local = tempfile::tempdir().unwrap();
        let local_path = canonical_temp_root(&local);
        let local_target = local_path.join("source-target.txt");
        let local_link = local_path.join("source-link.txt");
        fs::write(&local_target, "source target").unwrap();
        symlink(&local_target, &local_link).unwrap();
        assert!(backend
            .upload_file(&local_link, Path::new("/uploaded.txt"), None, None)
            .is_err());
        assert!(!root_path.join("uploaded.txt").exists());

        let remote_target = root_path.join("remote-target.txt");
        let remote_link = root_path.join("remote-link.txt");
        fs::write(&remote_target, "remote target").unwrap();
        symlink(&remote_target, &remote_link).unwrap();
        let downloaded = local_path.join("downloaded.txt");
        assert!(backend
            .download_file(Path::new("/remote-link.txt"), &downloaded, None, None)
            .is_err());
        assert!(!downloaded.exists());

        let regular_source = local_path.join("regular.txt");
        fs::write(&regular_source, "replacement").unwrap();
        assert!(backend
            .upload_file(&regular_source, Path::new("/remote-link.txt"), None, None)
            .is_err());
        assert_eq!(fs::read_to_string(&remote_target).unwrap(), "remote target");
    }

    #[test]
    fn sftp_recursive_delete_rejects_directory_replaced_by_symlink_before_confirmation() {
        let root = tempfile::tempdir().unwrap();
        let root_path = canonical_temp_root(&root);
        let selected = root_path.join("selected");
        let target = root_path.join("target");
        fs::create_dir(&selected).unwrap();
        fs::create_dir(&target).unwrap();
        fs::write(target.join("keep.txt"), "keep").unwrap();
        let backend = InMemorySftpBackend::new(root_path);
        let identity = backend
            .list_dir(Path::new("/"))
            .unwrap()
            .into_iter()
            .find(|entry| entry.name == "selected")
            .unwrap()
            .directory_identity
            .unwrap();

        fs::remove_dir(&selected).unwrap();
        symlink(&target, &selected).unwrap();
        assert!(backend
            .delete_dir_recursive(Path::new("/selected"), &identity)
            .is_err());
        assert_eq!(fs::read_to_string(target.join("keep.txt")).unwrap(), "keep");
    }

    #[test]
    fn sftp_upload_actual_open_rejects_source_symlink_swap() {
        let dir = tempfile::tempdir().unwrap();
        let root = canonical_temp_root(&dir);
        let source = root.join("source.txt");
        let target = root.join("target.txt");
        fs::write(&source, "planned").unwrap();
        fs::write(&target, "target").unwrap();
        ensure_transferable_local_file(&source).unwrap();
        fs::remove_file(&source).unwrap();
        symlink(&target, &source).unwrap();

        assert!(open_sftp_transfer_source(&source).is_err());
    }

    #[test]
    fn sftp_download_rejects_symlink_destination_ancestor() {
        let dir = tempfile::tempdir().unwrap();
        let root = canonical_temp_root(&dir);
        let target = root.join("target-directory");
        let linked = root.join("linked-directory");
        fs::create_dir(&target).unwrap();
        symlink(&target, &linked).unwrap();

        assert!(create_unique_transfer_staging(&linked.join("download.txt")).is_err());
        assert!(!target.join("download.txt").exists());
    }

    #[test]
    fn sftp_upload_failure_preserves_destination() {
        let dir = tempfile::tempdir().unwrap();
        let destination = canonical_temp_root(&dir).join("destination.txt");
        fs::write(&destination, "original").unwrap();
        let mut staging = create_unique_transfer_staging(&destination).unwrap();
        let mut file = staging.take_file();
        file.write_all(b"partial upload").unwrap();
        drop(file);
        drop(staging);

        assert_eq!(fs::read_to_string(destination).unwrap(), "original");
    }

    #[test]
    fn sftp_download_preserves_preexisting_partial_sibling() {
        let root = tempfile::tempdir().unwrap();
        let root_path = canonical_temp_root(&root);
        fs::write(root_path.join("remote.txt"), "downloaded").unwrap();
        let backend = InMemorySftpBackend::new(root_path);
        let local = tempfile::tempdir().unwrap();
        let local_path = canonical_temp_root(&local);
        let destination = local_path.join("destination.txt");
        let old_partial = local_path.join("destination.txt.partial");
        fs::write(&old_partial, "preexisting partial").unwrap();

        backend
            .download_file(Path::new("/remote.txt"), &destination, None, None)
            .unwrap();
        assert_eq!(fs::read_to_string(destination).unwrap(), "downloaded");
        assert_eq!(
            fs::read_to_string(old_partial).unwrap(),
            "preexisting partial"
        );
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    #[test]
    fn windows_staging_identity_rejects_path_replacement() {
        let root = tempfile::tempdir().unwrap();
        let staging_path = root.path().join("staging");
        let replacement_path = root.path().join("replacement");
        std::fs::write(&staging_path, "original").unwrap();
        std::fs::write(&replacement_path, "replacement").unwrap();

        let staging_file = std::fs::File::open(&staging_path).unwrap();
        let replacement_file = std::fs::File::open(&replacement_path).unwrap();
        let expected = windows_file_identity(&staging_file).unwrap();
        let replacement_identity = windows_file_identity(&replacement_file).unwrap();
        assert_ne!(expected, replacement_identity);
        drop(staging_file);
        drop(replacement_file);

        std::fs::remove_file(&staging_path).unwrap();
        std::fs::rename(&replacement_path, &staging_path).unwrap();

        let error = ensure_staging_identity(&staging_path, expected).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(error.to_string(), "transfer staging identity changed");
    }
}
