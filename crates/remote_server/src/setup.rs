mod glibc;

pub use glibc::{GlibcVersion, RemoteLibc};

use std::time::Duration;

use anyhow::{anyhow, Result};
use warp_core::channel::{Channel, ChannelState};

use crate::runtime_paths::{binary_name, remote_server_dir};

/// State machine for the remote server install → launch → initialize flow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteServerSetupState {
    /// Checking if the binary exists on remote.
    Checking,
    /// Downloading and installing the binary for the first time on this host.
    Installing { progress_percent: Option<u8> },
    /// Replacing an existing install with a differently-versioned binary.
    /// Rendered as "Updating..." in the UI so the user understands this
    /// isn't a fresh install.
    Updating,
    /// Binary is launched, waiting for InitializeResponse.
    Initializing,
    /// Handshake complete. Ready.
    Ready,
    /// Something failed during setup.
    Failed { error: String },
    /// Preinstall check classified the host as unsupported by the prebuilt
    /// remote-server binary. This is distinct from `Failed`, which is rendered
    /// as a setup error.
    Unsupported { reason: UnsupportedReason },
}

impl RemoteServerSetupState {
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }

    pub fn is_failed(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }

    pub fn is_unsupported(&self) -> bool {
        matches!(self, Self::Unsupported { .. })
    }

    pub fn is_terminal(&self) -> bool {
        self.is_ready() || self.is_failed() || self.is_unsupported()
    }

    pub fn is_in_progress(&self) -> bool {
        matches!(
            self,
            Self::Checking | Self::Installing { .. } | Self::Updating | Self::Initializing
        )
    }

    pub fn is_connecting(&self) -> bool {
        matches!(
            self,
            Self::Installing { .. } | Self::Updating | Self::Initializing
        )
    }
}

/// Outcome of [`crate::transport::RemoteTransport::run_preinstall_check`].
///
/// The script runs over the existing SSH socket before any install UI
/// surfaces and reports whether the host can run the prebuilt
/// remote-server binary. The Rust side is intentionally a thin parser
/// over the script's structured stdout (see `preinstall_check.sh`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreinstallCheckResult {
    pub status: PreinstallStatus,
    pub libc: RemoteLibc,
    /// Verbatim, trimmed script stdout used to diagnose `Unknown`
    /// outcomes on exotic distros.
    pub raw: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreinstallStatus {
    Supported,
    Unsupported {
        reason: UnsupportedReason,
    },
    /// Probe ran but couldn't classify the host. Treated as supported
    /// (fail open) by [`PreinstallCheckResult::is_supported`] so we keep
    /// today's install-and-try behavior on hosts where the probe is
    /// unreliable.
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnsupportedReason {
    GlibcTooOld {
        detected: GlibcVersion,
        required: GlibcVersion,
    },
    NonGlibc {
        name: String,
    },
}

impl PreinstallCheckResult {
    /// Whether the host is supported. Both `Supported` and `Unknown` return
    /// true; only positive detection of an unsupported libc returns false.
    pub fn is_supported(&self) -> bool {
        match self.status {
            PreinstallStatus::Supported | PreinstallStatus::Unknown => true,
            PreinstallStatus::Unsupported { .. } => false,
        }
    }

    /// Parses the structured `key=value` stdout emitted by
    /// `preinstall_check.sh`. Unknown keys and malformed lines are ignored.
    pub fn parse(stdout: &str) -> Self {
        let mut status_str: Option<&str> = None;
        let mut reason_str: Option<&str> = None;
        let mut libc_family: Option<&str> = None;
        let mut libc_version: Option<&str> = None;
        let mut required_glibc: Option<&str> = None;

        for line in stdout.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key.trim() {
                "status" => status_str = Some(value.trim()),
                "reason" => reason_str = Some(value.trim()),
                "libc_family" => libc_family = Some(value.trim()),
                "libc_version" => libc_version = Some(value.trim()),
                "required_glibc" => required_glibc = Some(value.trim()),
                _ => {} // ignore unknown keys
            }
        }

        let libc = glibc::parse_libc(libc_family, libc_version);
        let status = parse_status(status_str, reason_str, &libc, required_glibc);

        Self {
            status,
            libc,
            raw: stdout.trim().to_string(),
        }
    }
}

fn parse_status(
    status: Option<&str>,
    reason: Option<&str>,
    libc: &RemoteLibc,
    required_glibc: Option<&str>,
) -> PreinstallStatus {
    match status {
        Some("supported") => PreinstallStatus::Supported,
        Some("unsupported") => match reason {
            Some("glibc_too_old") => match (libc, required_glibc.and_then(GlibcVersion::parse)) {
                (RemoteLibc::Glibc(detected), Some(required)) => PreinstallStatus::Unsupported {
                    reason: UnsupportedReason::GlibcTooOld {
                        detected: *detected,
                        required,
                    },
                },
                _ => PreinstallStatus::Unknown,
            },
            Some("non_glibc") => match libc {
                RemoteLibc::NonGlibc { name } => PreinstallStatus::Unsupported {
                    reason: UnsupportedReason::NonGlibc { name: name.clone() },
                },
                RemoteLibc::Glibc(_) | RemoteLibc::Unknown => PreinstallStatus::Unknown,
            },
            // 其他无法识别的 unsupported 理由:保守起见 fail open。
            _ => PreinstallStatus::Unknown,
        },
        // status=unknown, missing, or anything else → fail open.
        _ => PreinstallStatus::Unknown,
    }
}

/// The bundled preinstall check script. Loaded as a string so the SSH
/// transport can pipe it through the existing ControlMaster socket via
/// [`crate::ssh::run_ssh_script_for_target`].
///
/// The script is intentionally self-contained — the supported-glibc
/// floor is hardcoded inside the script (see `preinstall_check.sh`)
/// rather than templated from Rust.
pub const PREINSTALL_CHECK_SCRIPT: &str = include_str!("preinstall_check.sh");

/// Detected remote platform from `uname -sm` output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemotePlatform {
    pub os: RemoteOs,
    pub arch: RemoteArch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteOs {
    Linux,
    MacOs,
}

impl RemoteOs {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::MacOs => "macos",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteArch {
    X86_64,
    Aarch64,
}

impl RemoteArch {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
        }
    }
}

/// Parse `uname -sm` output into a `RemotePlatform`.
///
/// The expected format is `<os> <arch>`, e.g. `Linux x86_64` or `Darwin arm64`.
/// Takes the last line to skip any shell initialization output.
pub fn parse_uname_output(output: &str) -> Result<RemotePlatform> {
    let line = output
        .lines()
        .last()
        .ok_or_else(|| anyhow!("empty uname output"))?
        .trim();

    let mut parts = line.split_whitespace();
    let os_str = parts
        .next()
        .ok_or_else(|| anyhow!("missing OS in uname output: {line}"))?;
    let arch_str = parts
        .next()
        .ok_or_else(|| anyhow!("missing arch in uname output: {line}"))?;

    let os = match os_str {
        "Linux" => RemoteOs::Linux,
        "Darwin" => RemoteOs::MacOs,
        other => return Err(anyhow!("unsupported OS: {other}")),
    };

    let arch = match arch_str {
        "x86_64" => RemoteArch::X86_64,
        "aarch64" | "arm64" | "armv8l" => RemoteArch::Aarch64,
        other => return Err(anyhow!("unsupported arch: {other}")),
    };

    Ok(RemotePlatform { os, arch })
}

/// Dev/source-build remote-server install slot.
///
/// Source builds do not have a release tag, so they use this explicit protocol
/// slot instead of an unversioned compatibility path. Bump this slot when the
/// local dev protocol becomes incompatible with an already-uploaded
/// remote-server binary.
const DEV_REMOTE_SERVER_SLOT: &str = "dev-pty-v1";

/// 返回当前 channel 和客户端版本对应的远端二进制完整路径。
///
/// Local channel 由 `script/deploy_remote_server` 覆盖同一个开发 slot。
/// OSS 源码构建没有 release tag,因此使用 dev protocol slot 后缀,避免
/// 客户端协议升级后继续复用远端旧二进制。
/// Release 构建带 `GIT_RELEASE_TAG` 时使用版本后缀,这样新版本会自然触发重新安装。
pub fn remote_server_binary() -> String {
    let dir = remote_server_dir();
    match ChannelState::channel() {
        Channel::Local => format!("{dir}/{}", binary_name()),
        Channel::Stable | Channel::Preview | Channel::Dev | Channel::Integration | Channel::Oss
            if ChannelState::app_version().is_none() =>
        {
            format!("{dir}/{}{}", source_build_binary_name(), version_suffix())
        }
        Channel::Stable | Channel::Preview | Channel::Dev | Channel::Integration | Channel::Oss => {
            format!("{dir}/{}-{}", binary_name(), pinned_version())
        }
    }
}

/// 源码构建的 cargo bin 固定是 `ashide`,不随 channel 改成 `ashide-dev`。
fn source_build_binary_name() -> &'static str {
    "ashide"
}

/// 返回检查远端 remote-server 二进制存在、可执行且支持当前客户端协议的 shell 命令。
///
/// 只运行 `--version` 不够:开发/OSS 构建会复用同一个 protocol slot 路径,
/// 旧二进制可能仍能打印版本,但不支持当前启动面(例如
/// `environment-runtime-proxy --identity-key`)。这里额外检查 proxy help 中是否
/// 暴露 `--identity-key`,不满足时退出非 0,让调用方把它当成需要更新。
pub fn binary_check_command() -> String {
    let binary = remote_server_binary();
    format!(
        "{binary} --version >/dev/null 2>&1 && {binary} environment-runtime-proxy --help 2>&1 | grep -q -- --identity-key"
    )
}

/// 返回用于版本化安装路径的版本号。优先使用编译时注入的
/// `GIT_RELEASE_TAG`;没有 release tag 时回退到 `CARGO_PKG_VERSION`,
/// 让需要版本化路径的 channel 保持确定性,并在缺少对应 release 资产时
/// 清晰失败,而不是误用无版本路径。
fn pinned_version() -> &'static str {
    ChannelState::app_version().unwrap_or(env!("CARGO_PKG_VERSION"))
}

fn version_suffix() -> String {
    match ChannelState::channel() {
        Channel::Local => String::new(),
        Channel::Stable | Channel::Preview | Channel::Dev | Channel::Integration | Channel::Oss
            if ChannelState::app_version().is_none() =>
        {
            format!("-{DEV_REMOTE_SERVER_SLOT}")
        }
        Channel::Stable | Channel::Preview | Channel::Dev | Channel::Integration | Channel::Oss => {
            format!("-{}", pinned_version())
        }
    }
}

/// 构造 Ashide CLI release 资产下载基址。
///
/// local-first 交付:这个 URL 由**本地 app** 拉取(本机有网),helper 资产落到
/// 本地缓存后再通过既有 SSH ControlMaster 推给远端。远端不再访问 GitHub,
/// 因此离线 / 内网远端也能用。
fn download_base_url() -> String {
    let release_path = match ChannelState::app_version() {
        Some(tag) => format!("download/{tag}"),
        None => "latest/download".to_string(),
    };
    format!("https://github.com/dly023/ashide/releases/{release_path}")
}

/// 远端平台对应的 release helper 资产 URL。
///
/// 命名与历史安装脚本保持一致:`ashide-<os>-<arch>.tar.gz`。由本地 app 下载,
/// 解包出 [`release_helper_archive_member`] 指向的二进制后上传给远端。
pub fn release_helper_asset_url(platform: &RemotePlatform) -> String {
    format!(
        "{}/ashide-{}-{}.tar.gz",
        download_base_url(),
        platform.os.as_str(),
        platform.arch.as_str(),
    )
}

/// release helper tarball 内的二进制成员名(与远端安装文件名同源)。
pub fn release_helper_archive_member() -> &'static str {
    binary_name()
}

/// Ashide 开发模式(DEBUG 源码构建,无 release tag)下,
/// SSH transport 不再从 GitHub 下载陈旧的发行版,而是本地交叉编译
/// 当前 `ashide` 二进制并上传。下面这些常量集中描述该交叉编译产物,
/// 与 `script/deploy_remote_server` 保持一致(同 profile / 同 features /
/// 同 target),避免两处分叉。
///
/// 交叉编译目标三元组。
pub const DEV_MUSL_TARGET: &str = "x86_64-unknown-linux-musl";

/// ARM64 Linux 远端的开发模式交叉编译目标三元组。
pub const DEV_AARCH64_MUSL_TARGET: &str = "aarch64-unknown-linux-musl";

/// 交叉编译使用的 cargo profile。对应 `Cargo.toml` 的 `[profile.dev-remote]`,
/// 它继承 release-size CLI profile 并 strip 符号,避免把完整 debug app 的
/// 300MB+ 静态二进制上传到远端。
pub const DEV_REMOTE_PROFILE: &str = "dev-remote";

/// 交叉编译启用的 features,与 `script/deploy_remote_server` 一致。
pub const DEV_REMOTE_FEATURES: &str = "release_bundle,crash_reporting,standalone,agent_mode_debug";

/// 判断当前是否处于「开发模式 remote-server 安装」路径。
///
/// 默认条件:DEBUG 构建(`debug_assertions`)且没有注入 `GIT_RELEASE_TAG`
/// (`app_version().is_none()`,即源码本地构建,非发行版)。这与
/// `remote_server_binary()` / `download_url()` 中对「无 release tag」的
/// 判定保持同一标准。release 构建恒为 `false`,行为完全不变。
///
/// 显式覆盖:设置 `ASHIDE_REMOTE_SERVER_FROM_LOCAL=1` 强制走本地交叉编译路径
/// (`0`/未设视为关闭)。用于 release 构建里临时联调本地 remote-server。
pub fn is_dev_source_build() -> bool {
    if let Some(raw) = std::env::var_os("ASHIDE_REMOTE_SERVER_FROM_LOCAL") {
        let lossy = raw.to_string_lossy();
        let trimmed = lossy.trim();
        let disabled =
            trimmed.is_empty() || trimmed == "0" || trimmed.eq_ignore_ascii_case("false");
        if !disabled {
            return true;
        }
    }
    cfg!(debug_assertions) && ChannelState::app_version().is_none()
}

/// 检查二进制是否存在的超时。
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// 本地下载 release helper 资产的超时。helper 是完整静态二进制(数十 MB),
/// 跨公网下载给一个宽松上限。
pub const RELEASE_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);

/// 开发模式交叉编译可能要从头编译整个 crate 图,给它一个很宽松的超时。
pub const DEV_CROSS_COMPILE_TIMEOUT: Duration = Duration::from_secs(900);

/// 开发模式上传本地交叉编译产物的超时。即便禁用 debuginfo,静态 helper
/// 仍明显大于 release 包；跨公网上传可能要数分钟,因此给一个宽松上限。
pub const DEV_UPLOAD_TIMEOUT: Duration = Duration::from_secs(1800);

#[cfg(test)]
#[path = "setup_tests.rs"]
mod tests;
