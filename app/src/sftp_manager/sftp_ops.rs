//! SFTP 操作辅助类型
//!
//! 文件传输/浏览的传输无关辅助类型与工具函数。
//! 实际的远程文件操作由 `daemon_backend::DaemonSftpBackend` 通过远程 helper
//! daemon 的原生文件 RPC（共享 ControlMaster 连接）完成。
//! author: logic
//! date: 2026-05-26

use std::path::PathBuf;

/// SFTP 操作错误
#[derive(Debug)]
pub enum SftpOpsError {
    /// 连接错误
    Connection(String),
    /// 操作错误
    Operation(String),
    /// 本地 IO 错误
    LocalIo(String),
    /// 未找到凭据
    NoCredentials(String),
    /// 传输已取消
    Cancelled,
}

impl std::fmt::Display for SftpOpsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SftpOpsError::Connection(msg) => write!(f, "连接错误: {msg}"),
            SftpOpsError::Operation(msg) => write!(f, "操作错误: {msg}"),
            SftpOpsError::LocalIo(msg) => write!(f, "本地 IO 错误: {msg}"),
            SftpOpsError::NoCredentials(msg) => write!(f, "未找到凭据: {msg}"),
            SftpOpsError::Cancelled => write!(f, "传输已取消"),
        }
    }
}

impl From<std::io::Error> for SftpOpsError {
    fn from(e: std::io::Error) -> Self {
        SftpOpsError::LocalIo(e.to_string())
    }
}

/// 进度回调类型
pub type ProgressCallback = Box<dyn Fn(u64, u64) + Send>;

/// 规范化远程路径，将 Windows 反斜杠替换为正斜杠
///
/// 远程服务器（Linux）只接受正斜杠路径分隔符，
/// 在 Windows 上 PathBuf::join 会产生反斜杠，必须转换。
pub(crate) fn normalize_remote_path(path: &PathBuf) -> PathBuf {
    PathBuf::from(path.to_string_lossy().replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试 SftpOpsError::Connection Display 输出
    #[test]
    fn test_sftp_ops_error_display_connection() {
        assert_eq!(
            SftpOpsError::Connection("refused".into()).to_string(),
            "连接错误: refused"
        );
    }

    /// 测试 SftpOpsError::Operation Display 输出
    #[test]
    fn test_sftp_ops_error_display_operation() {
        assert_eq!(
            SftpOpsError::Operation("not found".into()).to_string(),
            "操作错误: not found"
        );
    }

    /// 测试 SftpOpsError::LocalIo Display 输出
    #[test]
    fn test_sftp_ops_error_display_local_io() {
        assert_eq!(
            SftpOpsError::LocalIo("disk full".into()).to_string(),
            "本地 IO 错误: disk full"
        );
    }

    /// 测试 SftpOpsError::NoCredentials Display 输出
    #[test]
    fn test_sftp_ops_error_display_no_credentials() {
        assert_eq!(
            SftpOpsError::NoCredentials("no key".into()).to_string(),
            "未找到凭据: no key"
        );
    }

    /// 测试 SftpOpsError::Cancelled Display 输出
    #[test]
    fn test_sftp_ops_error_display_cancelled() {
        assert_eq!(SftpOpsError::Cancelled.to_string(), "传输已取消");
    }

    /// 测试从 std::io::Error 转换为 SftpOpsError
    #[test]
    fn test_sftp_ops_error_from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let ops_err: SftpOpsError = io_err.into();
        assert!(matches!(ops_err, SftpOpsError::LocalIo(_)));
    }

    // ==================== SftpOpsError 边界场景测试 ====================

    /// 测试 SftpOpsError::Connection 空消息
    #[test]
    fn test_sftp_ops_error_connection_empty() {
        assert_eq!(
            SftpOpsError::Connection(String::new()).to_string(),
            "连接错误: "
        );
    }

    /// 测试 SftpOpsError::Operation 空消息
    #[test]
    fn test_sftp_ops_error_operation_empty() {
        assert_eq!(
            SftpOpsError::Operation(String::new()).to_string(),
            "操作错误: "
        );
    }

    /// 测试 SftpOpsError::LocalIo 空消息
    #[test]
    fn test_sftp_ops_error_local_io_empty() {
        assert_eq!(
            SftpOpsError::LocalIo(String::new()).to_string(),
            "本地 IO 错误: "
        );
    }

    /// 测试 SftpOpsError::NoCredentials 空消息
    #[test]
    fn test_sftp_ops_error_no_credentials_empty() {
        assert_eq!(
            SftpOpsError::NoCredentials(String::new()).to_string(),
            "未找到凭据: "
        );
    }

    /// 测试 SftpOpsError::Cancelled 始终为固定文本
    #[test]
    fn test_sftp_ops_error_cancelled_consistent() {
        let s1 = SftpOpsError::Cancelled.to_string();
        let s2 = SftpOpsError::Cancelled.to_string();
        assert_eq!(s1, s2);
        assert_eq!(s1, "传输已取消");
    }

    /// 测试 normalize_remote_path 反斜杠转换
    #[test]
    fn test_normalize_remote_path_backslash() {
        let path = PathBuf::from("\\home\\user");
        assert_eq!(normalize_remote_path(&path), PathBuf::from("/home/user"));
    }
}
