//! CLI-agent session scan errors.

#[cfg(feature = "local_fs")]
use std::fmt;
#[cfg(feature = "local_fs")]
use std::io;
#[cfg(feature = "local_fs")]
use std::path::{Path, PathBuf};

#[cfg(feature = "local_fs")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CliAgentSessionScanError {
    path: Option<PathBuf>,
    operation: &'static str,
    message: String,
}

#[cfg(feature = "local_fs")]
impl CliAgentSessionScanError {
    pub(crate) fn io(path: &Path, operation: &'static str, error: io::Error) -> Self {
        Self {
            path: Some(path.to_path_buf()),
            operation,
            message: error.to_string(),
        }
    }

    pub(super) fn walk(root: &Path, error: walkdir::Error) -> Self {
        Self {
            path: error
                .path()
                .map(Path::to_path_buf)
                .or_else(|| Some(root.to_path_buf())),
            operation: "遍历 CLI-agent 会话目录",
            message: error.to_string(),
        }
    }

    pub(super) fn expected_directory(path: &Path) -> Self {
        Self {
            path: Some(path.to_path_buf()),
            operation: "读取 CLI-agent 会话目录",
            message: "路径存在但不是目录".to_owned(),
        }
    }

    pub(super) fn discovery_candidate_limit(path: &Path, limit: usize) -> Self {
        Self {
            path: Some(path.to_path_buf()),
            operation: "扫描 CLI-agent 会话候选项",
            message: format!(
                "候选会话超过安全上限 {limit}；已保留当前会话列表，缩小 provider store 后再刷新"
            ),
        }
    }

    pub(super) fn home_directory_unavailable() -> Self {
        Self {
            path: None,
            operation: "解析 CLI-agent home directory",
            message: "当前用户 home directory 不可用".to_owned(),
        }
    }

    pub(super) fn parse(path: &Path, operation: &'static str, message: String) -> Self {
        Self {
            path: Some(path.to_path_buf()),
            operation,
            message,
        }
    }

    #[cfg(test)]
    pub(super) fn operation(&self) -> &'static str {
        self.operation
    }

    #[cfg(test)]
    pub(super) fn message(&self) -> &str {
        &self.message
    }

    #[cfg(test)]
    pub(crate) fn source_missing() -> Self {
        Self {
            path: None,
            operation: "扫描 CLI-agent session discovery source",
            message: "provider stores are temporarily unavailable".to_owned(),
        }
    }
}

#[cfg(feature = "local_fs")]
impl fmt::Display for CliAgentSessionScanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(path) = &self.path {
            write!(
                formatter,
                "{} {} 失败：{}",
                self.operation,
                path.display(),
                self.message
            )
        } else {
            write!(formatter, "{}失败：{}", self.operation, self.message)
        }
    }
}

#[cfg(feature = "local_fs")]
impl std::error::Error for CliAgentSessionScanError {}
