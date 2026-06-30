//! Dev/release cross-compile + upload pipeline for the remote-server helper.
//!
//! Extracted verbatim from [`super::ssh_transport`] (tracker RR-A7 / ZAP-M3,
//! Phase 1: pure mechanical move). [`SshTransport::install_binary`] dispatches
//! into [`dev_install_local_binary`] / [`release_install_local_binary`]; the
//! rest of these functions are the dev cross-compile, freshness-stamp and
//! rsync/scp upload primitives those two entry points build on.
//!
//! This module owns no shared mutable state with `SshTransport`: every function
//! is a free function operating on a `socket_path` + `ssh_target` pair, mirroring
//! how `ssh_transport` originally hosted them.
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context as _, Result};
use sha2::{Digest as _, Sha256};
use warpui::r#async::{FutureExt as _, Timer};

use remote_server::runtime_paths;
use remote_server::setup::{parse_uname_output, RemoteArch, RemoteOs, RemotePlatform};

static DEV_REMOTE_UPLOAD_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) async fn detect_remote_platform(
    socket_path: &Path,
    ssh_target: &str,
) -> Result<RemotePlatform> {
    let output = remote_server::ssh::run_ssh_command_for_target(
        socket_path,
        ssh_target,
        "uname -sm",
        remote_server::setup::CHECK_TIMEOUT,
    )
    .await?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        return parse_uname_output(&stdout);
    }

    let code = output.status.code().unwrap_or(-1);
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(anyhow!("uname -sm exited with code {code}: {stderr}"))
}

async fn verify_installed_binary(socket_path: &Path, ssh_target: &str) -> Result<()> {
    let mut last_error = None;
    for attempt in 1..=5 {
        let output = remote_server::ssh::run_ssh_command_for_target(
            socket_path,
            ssh_target,
            &remote_server::setup::binary_check_command(),
            remote_server::setup::CHECK_TIMEOUT,
        )
        .await?;

        if output.status.success() {
            return Ok(());
        }

        let code = output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        last_error = Some((code, stderr.clone()));
        // 远端 binary 可能刚被另一个 install 原子替换或仍有 scp 写入旧路径。
        // 这种 ETXTBSY 是短暂状态,不要立刻把整个 Environment 判死。
        if stderr.contains("Text file busy") && attempt < 5 {
            Timer::after(Duration::from_millis(300 * attempt)).await;
            continue;
        }
        break;
    }

    let (code, stderr) = last_error.unwrap_or_else(|| (-1, String::new()));
    Err(anyhow!(
        "installed binary check failed with code {code}: {stderr}"
    ))
}

async fn stop_remote_environment_daemons(socket_path: &Path, ssh_target: &str) -> Result<()> {
    let remote_server_dir = runtime_paths::remote_server_dir();
    let quoted_dir = shell_words::quote(&remote_server_dir);
    let command = format!(
        r#"dir={quoted_dir}
if [ -d "$dir" ]; then
  for pid_file in "$dir"/*/server.pid; do
    [ -f "$pid_file" ] || continue
    pid="$(cat "$pid_file" 2>/dev/null || true)"
    case "$pid" in
      ''|*[!0-9]*) rm -f "$(dirname "$pid_file")/server.sock" "$pid_file"; continue ;;
    esac
    if kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null || true
      i=0
      while kill -0 "$pid" 2>/dev/null && [ "$i" -lt 10 ]; do
        i=$((i + 1))
        sleep 0.2
      done
      kill -0 "$pid" 2>/dev/null && kill -KILL "$pid" 2>/dev/null || true
    fi
    rm -f "$(dirname "$pid_file")/server.sock" "$pid_file"
  done
fi"#
    );

    let output = remote_server::ssh::run_ssh_command_for_target(
        socket_path,
        ssh_target,
        &command,
        remote_server::setup::CHECK_TIMEOUT,
    )
    .await?;

    if output.status.success() {
        return Ok(());
    }

    let code = output.status.code().unwrap_or(-1);
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(anyhow!(
        "remote environment daemon restart failed with code {code}: {stderr}"
    ))
}

// ===========================================================================
// Ashide fork:开发模式 remote-server 安装路径
//
// 上游 / release 构建会让远端安装脚本从 GitHub releases 下载预编译的
// remote-server 二进制。但在本地源码构建(`cargo run`)时,这会下载到
// 「最新已发布」的陈旧二进制,而不是开发者刚改过的代码,导致根本无法
// 调试 remote-server 的改动。
//
// 因此在 DEBUG 且无 release tag 的源码构建下(见
// `remote_server::setup::is_dev_source_build()`),`install_binary()` 改为:
//   1. 本地把 `warp` 二进制交叉编译到 x86_64 musl(profile/features 与
//      `script/deploy_remote_server` 完全一致);
//   2. 通过已有的 SSH ControlMaster socket,优先用 `rsync` 把产物增量上传到
//      `remote_server::setup::remote_server_binary()` 解析出的远端路径;
//   3. 完全跳过 GitHub 下载安装脚本。
//
// 如果交叉编译前置条件缺失(没装 musl target、没有 musl 链接器),直接失败并
// 暴露清晰错误；dev/source build 不能回退到 GitHub release 旧 helper。
// ===========================================================================

/// 开发模式交叉编译可能用到的 musl 链接器候选(按优先级)。
/// macOS 上一般是 `x86_64-linux-musl-gcc`(filosottile/musl-cross),
/// Linux 上常见为 `musl-gcc`。
const DEV_MUSL_LINKER_CANDIDATES: &[&str] = &["x86_64-linux-musl-gcc", "musl-gcc"];
const DEV_REMOTE_BUILD_STAMP_VERSION: &str = "1";
const DEV_REMOTE_BUILD_STAMP_FILE: &str = ".ashide-remote-helper-build.stamp";
const DEV_REMOTE_INSTALLED_STAMP_SUFFIX: &str = ".stamp";
const DEV_REMOTE_SOURCE_BIN_NAME: &str = "ashide";
// Freshness 只覆盖会实际影响远端 helper(proxy/daemon)行为的入口和依赖。
// 之前这里直接纳入 `app/` + `crates/` + `resources/`,导致任意 UI / i18n /
// 文档式源码调整都会触发 x86_64-musl helper 重编,让环境长时间停在“准备运行时”。
//
// 注意:dev helper 的发布形态是 remote-runtime capable `ashide` bin,cargo 编译时会
// 看到完整 app crate。这里的职责是 freshness 判定:只有远端 daemon/proxy/PTY/file
// 行为相关输入变化时才允许触发该昂贵编译。不要把整个 crate / 子目录放进来,否则
// UI、测试、文档、客户端抽象改动会再次把远程环境卡在“准备运行时”。
//
// 审计留痕(ZAP-M3 Phase 2 / backlog RR-A7):曾有「改用 git ls-files 自动推导输入集、
// 去掉这份硬编码白名单」的建议。经核实**驳回**:那等同于上面已被回退过的 app/+crates/
// 广撒网方案,会重新引发重编风暴。这份 per-file 白名单是刻意为之,rot 风险由下方
// scope 测试兜底——若担心遗漏,加强测试,**不要**扩大输入集。
const DEV_REMOTE_BUILD_INPUT_SCOPES: &[&str] = &[
    "Cargo.toml",
    "Cargo.lock",
    "app/Cargo.toml",
    "app/src/bin/ashide.rs",
    "app/src/remote_server/mod.rs",
    "app/src/remote_server/server_buffer_tracker.rs",
    "app/src/remote_server/server_model.rs",
    "app/src/remote_server/unix/mod.rs",
    "app/src/remote_server/unix/proxy.rs",
    "app/src/code/global_buffer_model.rs",
    "app/src/terminal/model/session/command_executor.rs",
    "app/src/terminal/model/session/command_executor/local_command_executor.rs",
    "app/src/terminal/local_tty/shell.rs",
    "app/src/ai/blocklist/action_model/execute.rs",
    "crates/ai/src/agent/action_result/mod.rs",
    "crates/ai/src/agent/file_locations.rs",
    "crates/command/Cargo.toml",
    "crates/command/src/async.rs",
    "crates/command/src/blocking.rs",
    "crates/command/src/lib.rs",
    "crates/command/src/unix.rs",
    "crates/command/src/windows.rs",
    "crates/remote_server/Cargo.toml",
    "crates/remote_server/proto/remote_server.proto",
    "crates/remote_server/src/lib.rs",
    "crates/remote_server/src/protocol.rs",
    "crates/remote_server/src/repo_metadata_proto.rs",
    "crates/remote_server/src/runtime_paths.rs",
    "crates/repo_metadata/Cargo.toml",
    "crates/repo_metadata/src/current_app_model.rs",
    "crates/repo_metadata/src/entry.rs",
    "crates/repo_metadata/src/file_tree_store.rs",
    "crates/repo_metadata/src/file_tree_store/file_tree_state.rs",
    "crates/repo_metadata/src/file_tree_update.rs",
    "crates/repo_metadata/src/lib.rs",
    "crates/repo_metadata/src/remote_model.rs",
    "crates/repo_metadata/src/repositories.rs",
    "crates/repo_metadata/src/repository.rs",
    "crates/repo_metadata/src/repository_identifier.rs",
    "crates/repo_metadata/src/watcher.rs",
    "crates/repo_metadata/src/wrapper_model.rs",
    "crates/warp_cli/Cargo.toml",
    "crates/warp_cli/src/lib.rs",
    "crates/warp_core/Cargo.toml",
    "crates/warp_core/src/app_id.rs",
    "crates/warp_core/src/channel/config.rs",
    "crates/warp_core/src/channel/mod.rs",
    "crates/warp_core/src/channel/state.rs",
    "crates/warp_core/src/errors.rs",
    "crates/warp_core/src/execution_mode.rs",
    "crates/warp_core/src/features.rs",
    "crates/warp_core/src/host_id.rs",
    "crates/warp_core/src/lib.rs",
    "crates/warp_core/src/paths.rs",
    "crates/warp_core/src/platform.rs",
    "crates/warp_core/src/safe_log.rs",
    "crates/warp_core/src/session_id.rs",
    "crates/warp_core/src/sync_queue.rs",
    "crates/warp_core/src/user_preferences.rs",
    "crates/warp_files/Cargo.toml",
    "crates/warp_files/src/lib.rs",
    "crates/warp_files/src/text_file_reader.rs",
    "crates/warp_terminal/Cargo.toml",
    "crates/warp_terminal/src/shell/mod.rs",
    "crates/warp_terminal/src/shell/unescape.rs",
    "crates/warp_util/Cargo.toml",
    "crates/warp_util/src/assets.rs",
    "crates/warp_util/src/content_version.rs",
    "crates/warp_util/src/file.rs",
    "crates/warp_util/src/file_type.rs",
    "crates/warp_util/src/lib.rs",
    "crates/warp_util/src/on_cancel.rs",
    "crates/warp_util/src/path.rs",
    "crates/warp_util/src/standardized_path.rs",
    "crates/warp_util/src/user_input.rs",
    "crates/warp_util/src/windows.rs",
    "crates/warp_util/src/worktree_names.rs",
];

/// 返回当前 workspace 根目录。
///
/// `ssh_transport.rs` 属于 `app` crate,`CARGO_MANIFEST_DIR` 指向
/// `<workspace>/app`,其父目录即 workspace 根。
pub(crate) fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        // 理论上 `app` 一定有父目录;万一没有就退回 manifest 目录本身。
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}

/// dev remote-server 交叉编译默认使用独立 target 目录,避免 GUI / app 的
/// macOS debug cache 与 x86_64 Linux helper cache 混在同一个 `target/` 下。
///
/// 可用 `ASHIDE_DEV_REMOTE_TARGET_DIR` 显式覆盖,方便开发者把 helper cache
/// 放到外部磁盘或临时目录。
fn dev_remote_target_root(root: &Path) -> PathBuf {
    dev_remote_target_root_from_env(root, std::env::var_os("ASHIDE_DEV_REMOTE_TARGET_DIR"))
}

fn dev_remote_target_root_from_env(root: &Path, override_dir: Option<OsString>) -> PathBuf {
    override_dir
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target").join("dev-remote"))
}

pub(crate) fn dev_musl_target_for_platform(platform: &RemotePlatform) -> Result<&'static str> {
    match (&platform.os, &platform.arch) {
        (RemoteOs::Linux, RemoteArch::X86_64) => Ok(remote_server::setup::DEV_MUSL_TARGET),
        (RemoteOs::Linux, RemoteArch::Aarch64) => Ok(remote_server::setup::DEV_AARCH64_MUSL_TARGET),
        (os, arch) => Err(anyhow!(
            "dev remote-server 本地交叉编译暂不支持远端平台 {} {}",
            os.as_str(),
            arch.as_str()
        )),
    }
}

fn dev_remote_binary_path(target_root: &Path, musl_target: &str, bin_name: &str) -> PathBuf {
    target_root
        .join(musl_target)
        .join(remote_server::setup::DEV_REMOTE_PROFILE)
        .join(bin_name)
}

fn dev_remote_build_stamp_path(target_root: &Path, musl_target: &str) -> PathBuf {
    target_root
        .join(musl_target)
        .join(DEV_REMOTE_BUILD_STAMP_FILE)
}

fn dev_remote_installed_stamp_path(remote_binary: &str) -> String {
    format!("{remote_binary}{DEV_REMOTE_INSTALLED_STAMP_SUFFIX}")
}

pub(crate) fn dev_remote_source_bin_name() -> &'static str {
    DEV_REMOTE_SOURCE_BIN_NAME
}

fn git_input_paths(root: &Path, include_untracked: bool) -> Result<Vec<PathBuf>> {
    let mut cmd = command::blocking::Command::new("git");
    cmd.current_dir(root);
    cmd.arg("ls-files");
    if include_untracked {
        cmd.arg("--others").arg("--exclude-standard");
    }
    cmd.arg("-z").arg("--");
    for scope in DEV_REMOTE_BUILD_INPUT_SCOPES {
        cmd.arg(scope);
    }
    let output = cmd.output().context("无法执行 git ls-files")?;
    if !output.status.success() {
        let code = output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("git ls-files 失败(exit {code}): {stderr}"));
    }

    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| root.join(String::from_utf8_lossy(entry).into_owned()))
        .collect())
}

fn dev_remote_build_input_paths(root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = BTreeSet::new();
    for path in git_input_paths(root, false)? {
        paths.insert(path);
    }
    for path in git_input_paths(root, true)? {
        paths.insert(path);
    }
    Ok(paths.into_iter().collect())
}

fn hash_dev_remote_input_file(hasher: &mut Sha256, root: &Path, path: &Path) -> Result<()> {
    let relative = path.strip_prefix(root).unwrap_or(path);
    hasher.update(b"path\0");
    hasher.update(relative.to_string_lossy().as_bytes());
    hasher.update(b"\0");

    match fs::File::open(path) {
        Ok(mut file) => {
            hasher.update(b"file\0");
            let mut buffer = [0; 64 * 1024];
            loop {
                let bytes_read = file
                    .read(&mut buffer)
                    .with_context(|| format!("读取 {} 失败", path.display()))?;
                if bytes_read == 0 {
                    break;
                }
                hasher.update(&buffer[..bytes_read]);
            }
            hasher.update(b"\0");
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            hasher.update(b"missing\0");
        }
        Err(error) => {
            return Err(anyhow!("读取 {} 失败: {error}", path.display()));
        }
    }

    Ok(())
}

fn dev_remote_input_digest(root: &Path) -> Result<String> {
    let mut hasher = Sha256::new();
    for path in dev_remote_build_input_paths(root)? {
        hash_dev_remote_input_file(&mut hasher, root, &path)?;
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub(crate) fn expected_dev_remote_build_stamp(
    root: &Path,
    musl_target: &str,
    bin_name: &str,
) -> Result<String> {
    let input_digest = dev_remote_input_digest(root)?;
    Ok(format!(
        "version={}\n\
         target={}\n\
         profile={}\n\
         features={}\n\
         bin={}\n\
         input_digest={}\n",
        DEV_REMOTE_BUILD_STAMP_VERSION,
        musl_target,
        remote_server::setup::DEV_REMOTE_PROFILE,
        remote_server::setup::DEV_REMOTE_FEATURES,
        bin_name,
        input_digest,
    ))
}

fn dev_remote_build_is_fresh(binary: &Path, stamp_path: &Path, expected_stamp: &str) -> bool {
    binary.is_file()
        && fs::read_to_string(stamp_path)
            .map(|actual_stamp| actual_stamp == expected_stamp)
            .unwrap_or(false)
}

fn dev_remote_binary_covers_paths(binary: &Path, paths: &[PathBuf]) -> Result<bool> {
    if !binary.is_file() {
        return Ok(false);
    }

    let binary_modified = fs::metadata(binary)
        .with_context(|| format!("读取 helper 产物元数据 {} 失败", binary.display()))?
        .modified()
        .with_context(|| format!("读取 helper 产物修改时间 {} 失败", binary.display()))?;

    for path in paths {
        let modified = match fs::metadata(&path) {
            Ok(metadata) => metadata
                .modified()
                .with_context(|| format!("读取 freshness 输入修改时间 {} 失败", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(anyhow!(
                    "读取 freshness 输入元数据 {} 失败: {error}",
                    path.display()
                ));
            }
        };

        if modified > binary_modified {
            return Ok(false);
        }
    }

    Ok(true)
}

fn dev_remote_binary_covers_current_inputs(binary: &Path, root: &Path) -> Result<bool> {
    let paths = dev_remote_build_input_paths(root)?;
    dev_remote_binary_covers_paths(binary, &paths)
}

fn local_file_sha256(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)
        .with_context(|| format!("打开本地 helper 产物 {} 失败", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let bytes_read = file
            .read(&mut buffer)
            .with_context(|| format!("读取本地 helper 产物 {} 失败", path.display()))?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn write_dev_remote_build_stamp(stamp_path: &Path, stamp: &str) -> Result<()> {
    if let Some(parent) = stamp_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建 stamp 目录 {} 失败", parent.display()))?;
    }
    let temp_path = stamp_path.with_extension("stamp.tmp");
    fs::write(&temp_path, stamp)
        .with_context(|| format!("写入临时 stamp {} 失败", temp_path.display()))?;
    fs::rename(&temp_path, stamp_path)
        .with_context(|| format!("更新 stamp {} 失败", stamp_path.display()))?;
    Ok(())
}

/// 返回追加了 `~/.cargo/bin`(及 `$CARGO_HOME/bin`)的 PATH。
///
/// warp 进程常由桌面环境或系统 `cargo` 拉起,其 PATH 可能只含 `/usr/bin`
/// 而不含 `~/.cargo/bin`。这会导致:
///   - `cargo zigbuild` 找不到 `cargo-zigbuild` 子命令 → 回退到 musl-gcc;
///   - cargo-zigbuild 自身找不到 `cargo` / `rustc`。
/// 交叉编译相关的子进程统一用这里返回的 PATH,保证两者都能解析到。
/// 若无需调整(无 HOME / 无法拼接)返回 `None`,调用方沿用继承的 PATH。
fn dev_build_path_env() -> Option<std::ffi::OsString> {
    let mut extra: Vec<PathBuf> = Vec::new();
    if let Some(cargo_home) = std::env::var_os("CARGO_HOME") {
        extra.push(PathBuf::from(cargo_home).join("bin"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        extra.push(PathBuf::from(home).join(".cargo").join("bin"));
    }
    // macOS GUI / app bundle 启动时 PATH 经常缺 Homebrew / local bin。
    // cargo-zigbuild 会在运行时调用 `zig`;只把 ~/.cargo/bin 注入进去还不够,
    // 否则探测能过、真正构建时仍可能因为找不到 zig 失败。
    extra.push(PathBuf::from("/opt/homebrew/bin"));
    extra.push(PathBuf::from("/usr/local/bin"));

    let current = std::env::var_os("PATH").unwrap_or_default();
    extra.extend(std::env::split_paths(&current));
    std::env::join_paths(extra).ok()
}

/// 在 `PATH` 中查找首个可用的 musl 链接器,找不到返回 `None`。
fn find_musl_linker() -> Option<&'static str> {
    DEV_MUSL_LINKER_CANDIDATES.iter().copied().find(|linker| {
        let mut cmd = command::blocking::Command::new(linker);
        if let Some(path) = dev_build_path_env() {
            cmd.env("PATH", path);
        }
        cmd.arg("--version")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    })
}

/// dev 交叉编译使用的构建后端。
enum DevBuildBackend {
    /// `cargo zigbuild`:zig 充当完整的 C/C++ musl 交叉工具链,无需单独安装
    /// `*-musl-gcc` / `*-musl-g++`,能正确编译 `freetype-sys` 等带 C/C++ 源码
    /// 的依赖。这是首选后端。
    Zigbuild,
    /// 原生 `cargo build` + musl 链接器。仅当系统装有完整的 musl C/C++ 交叉
    /// 工具链时才可靠 —— 只有 `*-musl-gcc`、缺 `*-musl-g++` 时,`freetype-sys`
    /// 之类的 C++ 依赖会编译失败。
    MuslGcc(&'static str),
}

/// 检测 `cargo-zigbuild` 是否可用。
///
/// 直接探测 `cargo-zigbuild --version`(二进制本身),而不是
/// `cargo zigbuild --version` —— 后者会被 `zigbuild` 子命令解析为未知参数
/// 而失败。探测用的 PATH 与实际构建一致(注入 `~/.cargo/bin`)。
fn cargo_zigbuild_available() -> bool {
    let mut cmd = command::blocking::Command::new("cargo-zigbuild");
    cmd.arg("--version");
    if let Some(path) = dev_build_path_env() {
        cmd.env("PATH", path);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// 选择 dev 交叉编译后端:优先 `cargo zigbuild`。x86_64 远端允许使用
/// 原生 `cargo build` + musl 链接器；其他目标必须使用 zigbuild。
fn select_dev_build_backend(musl_target: &str) -> Option<DevBuildBackend> {
    if cargo_zigbuild_available() {
        return Some(DevBuildBackend::Zigbuild);
    }
    if musl_target != remote_server::setup::DEV_MUSL_TARGET {
        return None;
    }
    find_musl_linker().map(DevBuildBackend::MuslGcc)
}

/// 检查对应 musl target 是否已通过 rustup 安装。
async fn musl_target_installed(musl_target: &str) -> bool {
    let mut cmd = command::r#async::Command::new("rustup");
    if let Some(path) = dev_build_path_env() {
        cmd.env("PATH", path);
    }
    let output = cmd
        .arg("target")
        .arg("list")
        .arg("--installed")
        .kill_on_drop(true)
        .output()
        .await;
    match output {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line.trim() == musl_target),
        // 拿不到 rustup 输出时保守地认为未安装,从而触发回退。
        _ => false,
    }
}

/// 交叉编译本地 `warp` 二进制到 musl,返回产物路径。
///
/// profile / features 与 `script/deploy_remote_server` 对齐。

fn quote_remote_shell_path(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        format!("\"$HOME\"/{}", shell_words::quote(rest))
    } else {
        shell_words::quote(path).into_owned()
    }
}

fn remote_upload_temp_path(remote_binary: &str) -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let counter = DEV_REMOTE_UPLOAD_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(
        "{remote_binary}.upload-{}-{millis}-{counter}",
        std::process::id(),
    )
}

async fn remote_transfer_path(
    socket_path: &Path,
    ssh_target: &str,
    remote_path: &str,
) -> Result<String> {
    let Some(rest) = remote_path.strip_prefix("~/") else {
        return Ok(remote_path.to_owned());
    };

    let output = remote_server::ssh::run_ssh_command_for_target(
        socket_path,
        ssh_target,
        r#"printf %s "$HOME""#,
        remote_server::setup::CHECK_TIMEOUT,
    )
    .await?;
    if !output.status.success() {
        let code = output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("远端 HOME 解析失败(exit {code}): {stderr}"));
    }

    let home = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if home.is_empty() {
        return Err(anyhow!("远端 HOME 解析为空"));
    }
    Ok(format!("{}/{}", home.trim_end_matches('/'), rest))
}

fn local_rsync_available() -> bool {
    command::blocking::Command::new("rsync")
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

async fn remote_rsync_available(socket_path: &Path, ssh_target: &str) -> Result<bool> {
    let output = remote_server::ssh::run_ssh_command_for_target(
        socket_path,
        ssh_target,
        "command -v rsync >/dev/null 2>&1",
        remote_server::setup::CHECK_TIMEOUT,
    )
    .await?;
    Ok(output.status.success())
}

fn rsync_ssh_command(socket_path: &Path) -> String {
    format!(
        "ssh -o ClearAllForwardings=yes -o ControlPath={} -o ControlMaster=no \
         -o PasswordAuthentication=no -o ForwardX11=no -o ServerAliveInterval=30 \
         -o ServerAliveCountMax=6 -o TCPKeepAlive=yes -o ConnectTimeout=15",
        socket_path.display()
    )
}

async fn rsync_upload_for_target(
    socket_path: &Path,
    ssh_target: &str,
    local_path: &Path,
    remote_path: &str,
    timeout: Duration,
) -> Result<bool> {
    if !local_rsync_available() {
        log::warn!("dev remote-server: 本机缺少 rsync,无法使用增量上传");
        return Ok(false);
    }
    if !remote_rsync_available(socket_path, ssh_target).await? {
        log::warn!("dev remote-server: 远端缺少 rsync,无法使用增量上传");
        return Ok(false);
    }

    let remote_path = remote_transfer_path(socket_path, ssh_target, remote_path).await?;
    let output = async {
        command::r#async::Command::new("rsync")
            .arg("-z")
            .arg("-t")
            .arg("--partial")
            .arg("-e")
            .arg(rsync_ssh_command(socket_path))
            .arg(local_path.as_os_str())
            .arg(format!("{ssh_target}:{remote_path}"))
            .kill_on_drop(true)
            .output()
            .await
    }
    .with_timeout(timeout)
    .await
    .map_err(|_| anyhow!("rsync upload timed out after {timeout:?}"))?
    .map_err(|error| anyhow!("rsync upload failed to execute: {error}"))?;

    if output.status.success() {
        return Ok(true);
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(anyhow!("rsync upload failed: {stderr}"))
}

async fn chmod_remote_binary(
    socket_path: &Path,
    ssh_target: &str,
    remote_binary: &str,
) -> Result<()> {
    let quoted_binary = quote_remote_shell_path(remote_binary);
    let output = remote_server::ssh::run_ssh_command_for_target(
        socket_path,
        ssh_target,
        &format!("chmod 755 {quoted_binary}"),
        remote_server::setup::CHECK_TIMEOUT,
    )
    .await?;
    if output.status.success() {
        return Ok(());
    }

    let code = output.status.code().unwrap_or(-1);
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(anyhow!(
        "远端 remote-server chmod 失败(exit {code}): {stderr}"
    ))
}

async fn cleanup_stale_dev_uploads(
    socket_path: &Path,
    ssh_target: &str,
    remote_binary: &str,
) -> Result<()> {
    let quoted_binary = quote_remote_shell_path(remote_binary);
    let command = format!(
        r#"binary={quoted_binary}
dir="${{binary%/*}}"
base="${{binary##*/}}"
if [ -d "$dir" ]; then
  find "$dir" -maxdepth 1 -type f -name "$base.upload-*" -mmin +30 -exec rm -f -- {{}} +
fi"#
    );
    let output = remote_server::ssh::run_ssh_command_for_target(
        socket_path,
        ssh_target,
        &command,
        remote_server::setup::CHECK_TIMEOUT,
    )
    .await?;
    if output.status.success() {
        return Ok(());
    }

    let code = output.status.code().unwrap_or(-1);
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(anyhow!(
        "远端 remote-server 临时上传残留清理失败(exit {code}): {stderr}"
    ))
}

async fn promote_uploaded_binary(
    socket_path: &Path,
    ssh_target: &str,
    remote_temp_binary: &str,
    remote_binary: &str,
) -> Result<()> {
    let quoted_temp = quote_remote_shell_path(remote_temp_binary);
    let quoted_binary = quote_remote_shell_path(remote_binary);
    let promote_cmd = format!("chmod 755 {quoted_temp} && mv -f {quoted_temp} {quoted_binary}");
    let output = remote_server::ssh::run_ssh_command_for_target(
        socket_path,
        ssh_target,
        &promote_cmd,
        remote_server::setup::CHECK_TIMEOUT,
    )
    .await?;
    if output.status.success() {
        return Ok(());
    }

    let code = output.status.code().unwrap_or(-1);
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(anyhow!(
        "远端 remote-server 原子替换失败(exit {code}): {stderr}"
    ))
}

async fn upload_dev_remote_build_stamp(
    socket_path: &Path,
    ssh_target: &str,
    local_stamp_path: &Path,
    remote_binary: &str,
) -> Result<()> {
    let remote_stamp = dev_remote_installed_stamp_path(remote_binary);
    let remote_temp_stamp = remote_upload_temp_path(&remote_stamp);
    remote_server::ssh::scp_upload_for_target(
        socket_path,
        ssh_target,
        local_stamp_path,
        &remote_temp_stamp,
        remote_server::setup::CHECK_TIMEOUT,
    )
    .await?;

    let quoted_temp = quote_remote_shell_path(&remote_temp_stamp);
    let quoted_stamp = quote_remote_shell_path(&remote_stamp);
    let promote_cmd = format!("mv -f {quoted_temp} {quoted_stamp}");
    let output = remote_server::ssh::run_ssh_command_for_target(
        socket_path,
        ssh_target,
        &promote_cmd,
        remote_server::setup::CHECK_TIMEOUT,
    )
    .await?;
    if output.status.success() {
        return Ok(());
    }

    let code = output.status.code().unwrap_or(-1);
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(anyhow!(
        "远端 remote-server stamp 更新失败(exit {code}): {stderr}"
    ))
}

pub(crate) async fn remote_dev_build_stamp_matches(
    socket_path: &Path,
    ssh_target: &str,
    remote_binary: &str,
    expected_stamp: &str,
) -> Result<bool> {
    let remote_stamp = dev_remote_installed_stamp_path(remote_binary);
    let quoted_stamp = quote_remote_shell_path(&remote_stamp);
    let output = remote_server::ssh::run_ssh_command_for_target(
        socket_path,
        ssh_target,
        &format!("cat {quoted_stamp} 2>/dev/null"),
        remote_server::setup::CHECK_TIMEOUT,
    )
    .await?;

    Ok(output.status.success() && output.stdout == expected_stamp.as_bytes())
}

async fn remote_file_sha256(
    socket_path: &Path,
    ssh_target: &str,
    remote_path: &str,
) -> Result<Option<String>> {
    let quoted_path = quote_remote_shell_path(remote_path);
    let command = format!(
        "if [ -f {quoted_path} ]; then \
         (sha256sum {quoted_path} 2>/dev/null || shasum -a 256 {quoted_path} 2>/dev/null) | \
         awk '{{print $1}}'; \
         fi"
    );
    let output = remote_server::ssh::run_ssh_command_for_target(
        socket_path,
        ssh_target,
        &command,
        remote_server::setup::CHECK_TIMEOUT,
    )
    .await?;
    if !output.status.success() {
        let code = output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("远端 helper hash 检查失败(exit {code}): {stderr}"));
    }

    let hash = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned);
    Ok(hash)
}

async fn skip_dev_upload_if_remote_binary_matches(
    socket_path: &Path,
    ssh_target: &str,
    local_binary: &Path,
    local_stamp_path: &Path,
    remote_binary: &str,
) -> Result<bool> {
    if !local_stamp_path.is_file() {
        return Ok(false);
    }

    let local_hash = local_file_sha256(local_binary)?;
    let Some(remote_hash) = remote_file_sha256(socket_path, ssh_target, remote_binary).await?
    else {
        return Ok(false);
    };
    if remote_hash != local_hash {
        return Ok(false);
    }

    log::info!("dev remote-server: 远端 helper 内容已与本地产物一致,跳过大文件上传");
    if let Err(error) =
        upload_dev_remote_build_stamp(socket_path, ssh_target, local_stamp_path, remote_binary)
            .await
    {
        log::warn!(
            "dev remote-server: 跳过上传后刷新远端 freshness stamp 失败: {error:#};\
             当前连接继续使用已匹配的 helper"
        );
    }
    stop_remote_environment_daemons(socket_path, ssh_target).await?;
    verify_installed_binary(socket_path, ssh_target).await?;
    Ok(true)
}

async fn cross_compile_remote_server(
    backend: &DevBuildBackend,
    musl_target: &str,
) -> Result<PathBuf> {
    let root = workspace_root();
    // Cargo `[[bin]]` 名固定是 `ashide`(见 app/Cargo.toml)。Channel 只影响
    // 远端安装路径,例如 Dev channel 会上传到 `ashide-dev-...`;不能把
    // `runtime_paths::binary_name()` 当 cargo bin 名,否则 AshideDev 会尝试构建
    // 不存在的 `--bin ashide-dev` 并立即 exit 101。
    let bin_name = dev_remote_source_bin_name();
    let backend_desc = match backend {
        DevBuildBackend::Zigbuild => "cargo-zigbuild".to_string(),
        DevBuildBackend::MuslGcc(linker) => format!("cargo-build/{linker}"),
    };
    log::info!(
        "dev remote-server: 交叉编译 {bin_name} -> {} (profile={}, backend={backend_desc})",
        musl_target,
        remote_server::setup::DEV_REMOTE_PROFILE,
    );
    let target_root = dev_remote_target_root(&root);
    log::info!(
        "dev remote-server: 使用独立 cargo target dir {}",
        target_root.display()
    );
    let binary = dev_remote_binary_path(&target_root, musl_target, bin_name);
    let stamp_path = dev_remote_build_stamp_path(&target_root, musl_target);
    let expected_stamp = match expected_dev_remote_build_stamp(&root, musl_target, bin_name) {
        Ok(stamp) => Some(stamp),
        Err(error) => {
            log::warn!("dev remote-server: 无法计算构建 freshness stamp,本次不跳过编译: {error:#}");
            None
        }
    };

    if let Some(stamp) = expected_stamp.as_deref() {
        if dev_remote_build_is_fresh(&binary, &stamp_path, stamp) {
            log::info!(
                "dev remote-server: 输入未变化,复用已有 helper 产物 {}",
                binary.display()
            );
            return Ok(binary);
        }

        if dev_remote_binary_covers_current_inputs(&binary, &root)? {
            write_dev_remote_build_stamp(&stamp_path, stamp)?;
            log::info!(
                "dev remote-server: helper 产物晚于 freshness 输入,刷新 stamp 后复用 {}",
                binary.display()
            );
            return Ok(binary);
        }

        log::info!(
            "dev remote-server: helper 产物缺失或输入已变化,需要重新编译 ({})",
            stamp_path.display()
        );
    }

    // 首次会编译整个 warp,耗时通常数分钟。stdout/stderr 直接 inherit 到运行
    // Ashide 的终端,这样开发者能看到 cargo 的实时编译进度(否则全程静默,
    // 容易误以为卡死)。
    log::info!(
        "dev remote-server: 正在交叉编译,首次通常需数分钟 —— cargo 进度会打印到\
         运行 Ashide 的终端"
    );

    let status = async {
        let mut cmd = command::r#async::Command::new("cargo");
        cmd.current_dir(&root);
        cmd.env("CARGO_TARGET_DIR", &target_root);
        // 注入 `~/.cargo/bin`,确保 `cargo zigbuild` 能解析 `cargo-zigbuild`
        // 子命令,且 cargo-zigbuild 能找到 `cargo` / `rustc`。
        if let Some(path) = dev_build_path_env() {
            cmd.env("PATH", path);
        }
        match backend {
            // zigbuild 是 cargo 子命令,自带 zig 链接器与 C/C++ 交叉编译器,
            // 无需再设 LINKER env。
            DevBuildBackend::Zigbuild => {
                cmd.arg("zigbuild");
            }
            // 原生 cargo build:通过 env 指定 musl 链接器并覆盖 rustflags,
            // 避免 .cargo/config.toml 里 macOS 专用 flag 污染交叉编译。
            DevBuildBackend::MuslGcc(linker) => {
                cmd.arg("build")
                    .env("CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER", *linker)
                    .env(
                        "CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_RUSTFLAGS",
                        "-C symbol-mangling-version=v0",
                    );
            }
        }
        cmd.arg("-p")
            .arg("warp")
            .arg("--bin")
            .arg(bin_name)
            .arg("--target")
            .arg(musl_target)
            .arg("--profile")
            .arg(remote_server::setup::DEV_REMOTE_PROFILE)
            .arg("--features")
            .arg(remote_server::setup::DEV_REMOTE_FEATURES)
            // inherit:把 cargo 实时进度透到终端,而不是全程静默缓冲。
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true)
            .status()
            .await
    }
    .with_timeout(remote_server::setup::DEV_CROSS_COMPILE_TIMEOUT)
    .await
    .map_err(|_| {
        anyhow!(
            "dev remote-server 交叉编译超时(>{:?})",
            remote_server::setup::DEV_CROSS_COMPILE_TIMEOUT
        )
    })?
    .map_err(|e| anyhow!("无法启动 cargo 构建: {e}"))?;

    if !status.success() {
        let code = status.code().unwrap_or(-1);
        return Err(anyhow!(
            "cargo 交叉编译失败(exit {code}),详见运行 Ashide 的终端的 cargo 输出"
        ));
    }

    if !binary.is_file() {
        return Err(anyhow!(
            "交叉编译完成但未在 {} 找到产物(若设置了 ASHIDE_DEV_REMOTE_TARGET_DIR 请确认路径)",
            binary.display()
        ));
    }
    if let Some(stamp) = expected_stamp.as_deref() {
        if let Err(error) = write_dev_remote_build_stamp(&stamp_path, stamp) {
            log::warn!(
                "dev remote-server: helper 编译已完成,但写入 freshness stamp 失败: {error:#}"
            );
        }
    }
    Ok(binary)
}

// ===========================================================================
// Ashide local-first:release remote-server 交付路径
//
// release 构建过去让远端跑安装脚本,从 GitHub 下载预编译 helper(远端必须能访问
// github.com)。local-first 方向下翻转为:**本地** app 拉取 helper 资产(本机有网),
// 缓存到本地,再通过既有 SSH ControlMaster 把它推给远端(rsync 增量,或 scp 临时
// 文件 + chmod + 原子 mv)。远端不再访问外网,内网 / 离线远端也能用,且复用 dev
// 路径已验证的上传原语。
// ===========================================================================

/// 本机 HOME(用于定位 release helper 本地缓存)。
fn local_home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| !home.as_os_str().is_empty())
        .ok_or_else(|| anyhow!("无法解析本机 HOME,无法定位 release helper 缓存目录"))
}

/// release helper 本地缓存根目录,与远端安装目录同源(`~/.ashide` 系列),
/// 落在 `<channel>/remote-server-cache` 下,保证不同 channel 互不污染。
fn local_release_helper_cache_dir() -> Result<PathBuf> {
    // remote_server_dir() 形如 `~/.ashide/remote-server`;本地缓存复用同一 channel
    // 目录,把 `remote-server` 换成 `remote-server-cache`。
    let remote_dir = runtime_paths::remote_server_dir();
    let relative = remote_dir.trim_start_matches("~/");
    let cache_relative = match relative.strip_suffix("/remote-server") {
        Some(channel_dir) => format!("{channel_dir}/remote-server-cache"),
        None => format!("{relative}-cache"),
    };
    Ok(local_home_dir()?.join(cache_relative))
}

/// 远端平台对应的 release helper 本地缓存文件路径。
///
/// 缓存文件名用 `remote_server_binary()` 的文件名(已含版本 / 协议 slot 后缀),
/// 客户端协议或版本变化时自然换新缓存,绝不复用旧 helper。
fn local_release_helper_path(platform: &RemotePlatform) -> Result<PathBuf> {
    let basename = remote_server::setup::remote_server_binary()
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("ashide")
        .to_owned();
    Ok(local_release_helper_cache_dir()?
        .join(format!(
            "{}-{}",
            platform.os.as_str(),
            platform.arch.as_str()
        ))
        .join(basename))
}

/// 本机是否有某个命令(curl/wget/tar)。
fn local_command_available(program: &str) -> bool {
    command::blocking::Command::new(program)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

async fn curl_download_to_local_file(
    url: &str,
    dest: &Path,
    timeout: Duration,
    force_ipv4: bool,
    resume: bool,
) -> Result<()> {
    let mut command = command::r#async::Command::new("curl");
    command.arg("-fSL");
    if force_ipv4 {
        command.arg("-4");
    }
    if resume {
        command.arg("-C").arg("-");
    }
    let output = command
        .arg("--connect-timeout")
        .arg("15")
        .arg("-o")
        .arg(dest)
        .arg("--")
        .arg(url)
        .kill_on_drop(true)
        .output()
        .with_timeout(timeout)
        .await
        .map_err(|_| anyhow!("下载 release helper 超时 {timeout:?}: {url}"))?
        .map_err(|error| anyhow!("curl 启动失败: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mode = if force_ipv4 { "curl -4" } else { "curl" };
    Err(anyhow!(
        "{mode} 下载 release helper 失败({}): {stderr}",
        output.status
    ))
}

/// 本地把 URL 下载到文件。优先 curl,回退 wget(macOS 自带 curl)。
async fn download_to_local_file(url: &str, dest: &Path, timeout: Duration) -> Result<()> {
    if local_command_available("curl") {
        let error = match curl_download_to_local_file(url, dest, timeout, false, false).await {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };
        log::warn!("curl 下载 release helper 失败,改用 curl -4 断点续传重试: {error:#}");
        let mut last_error = error;
        for attempt in 1..=5 {
            let resume = dest.is_file();
            match curl_download_to_local_file(url, dest, timeout, true, resume).await {
                Ok(()) => return Ok(()),
                Err(error) => {
                    last_error = error;
                    log::warn!("curl -4 下载 release helper 第 {attempt}/5 次失败: {last_error:#}");
                    if attempt < 5 {
                        Timer::after(Duration::from_secs(2)).await;
                    }
                }
            }
        }
        return Err(anyhow!(
            "curl IPv4 fallback 重试 5 次仍失败;最后错误: {last_error:#}"
        ));
    }
    if local_command_available("wget") {
        let output = command::r#async::Command::new("wget")
            .arg("-q")
            .arg("-O")
            .arg(dest)
            .arg(url)
            .kill_on_drop(true)
            .output()
            .with_timeout(timeout)
            .await
            .map_err(|_| anyhow!("下载 release helper 超时 {timeout:?}: {url}"))?
            .map_err(|error| anyhow!("wget 启动失败: {error}"))?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "wget 下载 release helper 失败({}): {stderr}",
            output.status
        ));
    }
    Err(anyhow!(
        "本机缺少 curl/wget,无法本地拉取 release helper(local-first 交付要求本机有网): {url}"
    ))
}

/// 本地解包 tar.gz 到目录(系统 tar,macOS/Linux 自带)。
async fn extract_local_tarball(tarball: &Path, dest_dir: &Path) -> Result<()> {
    if !local_command_available("tar") {
        return Err(anyhow!("本机缺少 tar,无法解包 release helper"));
    }
    let output = command::r#async::Command::new("tar")
        .arg("-xzf")
        .arg(tarball)
        .arg("-C")
        .arg(dest_dir)
        .kill_on_drop(true)
        .output()
        .with_timeout(Duration::from_secs(120))
        .await
        .map_err(|_| anyhow!("解包 release helper 超时"))?
        .map_err(|error| anyhow!("tar 启动失败: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(anyhow!(
        "tar 解包 release helper 失败({}): {stderr}",
        output.status
    ))
}

/// 确保本地缓存里有远端平台对应的 release helper;没有就下载 + 解包。
async fn ensure_local_release_helper(platform: &RemotePlatform) -> Result<PathBuf> {
    let dest = local_release_helper_path(platform)?;
    if dest.is_file() {
        log::info!(
            "release remote-server: 命中本地 helper 缓存 {}",
            dest.display()
        );
        return Ok(dest);
    }
    let parent = dest
        .parent()
        .ok_or_else(|| anyhow!("release helper 缓存路径无父目录: {}", dest.display()))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("创建 release helper 缓存目录失败: {}", parent.display()))?;

    // 下载 + 解包都在唯一 staging 目录里完成,成功后原子 rename 到缓存路径,
    // 避免并发安装 / 中断留下半成品被当成有效缓存。
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let staging = parent.join(format!(".download-{}-{millis}", std::process::id()));
    fs::create_dir_all(&staging)
        .with_context(|| format!("创建 release helper 临时目录失败: {}", staging.display()))?;

    let result = async {
        let url = remote_server::setup::release_helper_asset_url(platform);
        log::info!("release remote-server: 本地拉取 helper 资产 {url}");
        let tarball = staging.join("helper.tar.gz");
        download_to_local_file(
            &url,
            &tarball,
            remote_server::setup::RELEASE_DOWNLOAD_TIMEOUT,
        )
        .await?;
        extract_local_tarball(&tarball, &staging).await?;

        let member = remote_server::setup::release_helper_archive_member();
        let extracted = staging.join(member);
        if !extracted.is_file() {
            return Err(anyhow!(
                "release helper tarball 缺少二进制成员 `{member}`: {}",
                extracted.display()
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&extracted, std::fs::Permissions::from_mode(0o755))
                .with_context(|| format!("设置 helper 可执行位失败: {}", extracted.display()))?;
        }
        // rename 到最终缓存路径(同一文件系统,原子)。
        fs::rename(&extracted, &dest).with_context(|| {
            format!(
                "移动 helper 到缓存路径失败: {} -> {}",
                extracted.display(),
                dest.display()
            )
        })?;
        Ok::<(), anyhow::Error>(())
    }
    .await;

    let _ = fs::remove_dir_all(&staging);
    result?;
    Ok(dest)
}

/// release 安装:本地准备好 helper 后,复用 dev 路径的上传原语推给远端。
pub(crate) async fn release_install_local_binary(
    socket_path: &Path,
    ssh_target: &str,
) -> Result<()> {
    let platform = detect_remote_platform(socket_path, ssh_target).await?;
    log::info!(
        "release remote-server: 远端平台 {} {},本地准备 helper 后上传(local-first)",
        platform.os.as_str(),
        platform.arch.as_str()
    );
    let local_binary = ensure_local_release_helper(&platform).await?;

    let remote_binary = remote_server::setup::remote_server_binary();
    let remote_dir = runtime_paths::remote_server_dir();
    let mkdir_output = remote_server::ssh::run_ssh_command_for_target(
        socket_path,
        ssh_target,
        &format!("mkdir -p {remote_dir}"),
        remote_server::setup::CHECK_TIMEOUT,
    )
    .await?;
    if !mkdir_output.status.success() {
        let code = mkdir_output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&mkdir_output.stderr);
        return Err(anyhow!(
            "远端 remote-server 目录创建失败(exit {code}): {stderr}"
        ));
    }

    let remote_temp_binary = remote_upload_temp_path(&remote_binary);
    if let Err(error) = cleanup_stale_dev_uploads(socket_path, ssh_target, &remote_binary).await {
        log::warn!("release remote-server: 清理旧上传临时文件失败: {error:#}");
    }

    let uploaded_with_rsync = rsync_upload_for_target(
        socket_path,
        ssh_target,
        &local_binary,
        &remote_binary,
        remote_server::setup::DEV_UPLOAD_TIMEOUT,
    )
    .await?;
    if uploaded_with_rsync {
        chmod_remote_binary(socket_path, ssh_target, &remote_binary).await?;
    } else {
        // 不能直接覆盖最终 binary(运行中或并发上传会 ETXTBSY):先传唯一临时文件,
        // 再 chmod + mv -f 原子替换。
        remote_server::ssh::scp_upload_for_target(
            socket_path,
            ssh_target,
            &local_binary,
            &remote_temp_binary,
            remote_server::setup::DEV_UPLOAD_TIMEOUT,
        )
        .await?;
        promote_uploaded_binary(socket_path, ssh_target, &remote_temp_binary, &remote_binary)
            .await?;
    }

    stop_remote_environment_daemons(socket_path, ssh_target).await?;
    verify_installed_binary(socket_path, ssh_target).await
}

/// 开发模式安装:交叉编译本地 `warp` 并上传到远端 remote-server 路径。
///
/// 上传目标与 `remote_server_binary()` 完全一致,确保随后的
/// `check_binary()` / proxy 启动能找到它。
pub(crate) async fn dev_install_local_binary(socket_path: &Path, ssh_target: &str) -> Result<()> {
    let platform = detect_remote_platform(socket_path, ssh_target).await?;
    let musl_target = dev_musl_target_for_platform(&platform)?;
    log::info!(
        "dev remote-server: 远端平台 {} {},使用本地 helper target {musl_target}",
        platform.os.as_str(),
        platform.arch.as_str()
    );

    // rustup target 探测在 GUI 启动环境里可能误判:PATH/RUSTUP_HOME/CARGO_HOME
    // 与开发 shell 不一致。这里不能再把探测失败当硬错误,否则所有远程环境
    // 都会在安装前直接失败。真正是否缺 target 交给 cargo zigbuild/build 输出
    // 决定,这样至少能利用已可用的工具链继续安装。
    if !musl_target_installed(musl_target).await {
        log::warn!(
            "dev remote-server: rustup 未报告已安装 target {},继续尝试交叉编译；\
             若确实缺失,cargo 会返回明确错误",
            musl_target,
        );
    }
    // 选择交叉编译后端:优先 `cargo zigbuild`(zig 自带完整 C/C++ musl 工具链,
    // 能编译 freetype-sys 等 C++ 依赖),否则回退到 musl-gcc。两者皆无则报错。
    let backend = select_dev_build_backend(musl_target).ok_or_else(|| {
        anyhow!(
            "未找到可用的 musl 交叉编译后端。建议安装 cargo-zigbuild + zig\
             (`cargo install cargo-zigbuild`,并用包管理器安装 `zig`),\
             或安装完整的 musl C/C++ 交叉工具链({})",
            DEV_MUSL_LINKER_CANDIDATES.join(" / ")
        )
    })?;

    let local_binary = cross_compile_remote_server(&backend, musl_target).await?;
    let local_stamp_path =
        dev_remote_build_stamp_path(&dev_remote_target_root(&workspace_root()), musl_target);

    // 上传到 `remote_server_binary()` 解析出的精确路径,先建好父目录。
    let remote_binary = remote_server::setup::remote_server_binary();
    let remote_dir = runtime_paths::remote_server_dir();
    let mkdir_output = remote_server::ssh::run_ssh_command_for_target(
        socket_path,
        ssh_target,
        &format!("mkdir -p {remote_dir}"),
        remote_server::setup::CHECK_TIMEOUT,
    )
    .await?;
    if !mkdir_output.status.success() {
        let code = mkdir_output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&mkdir_output.stderr);
        return Err(anyhow!(
            "远端 remote-server 目录创建失败(exit {code}): {stderr}"
        ));
    }

    let remote_temp_binary = remote_upload_temp_path(&remote_binary);
    if let Err(error) = cleanup_stale_dev_uploads(socket_path, ssh_target, &remote_binary).await {
        log::warn!("dev remote-server: 清理旧上传临时文件失败: {error:#}");
    }

    if skip_dev_upload_if_remote_binary_matches(
        socket_path,
        ssh_target,
        &local_binary,
        &local_stamp_path,
        &remote_binary,
    )
    .await?
    {
        return Ok(());
    }

    log::info!("dev remote-server: 通过 rsync 增量上传本地 helper 到 {remote_binary}");
    let uploaded_with_rsync = rsync_upload_for_target(
        socket_path,
        ssh_target,
        &local_binary,
        &remote_binary,
        remote_server::setup::DEV_UPLOAD_TIMEOUT,
    )
    .await?;

    if uploaded_with_rsync {
        chmod_remote_binary(socket_path, ssh_target, &remote_binary).await?;
    } else {
        log::warn!(
            "dev remote-server: rsync 不可用,退回 scp 完整上传到临时文件 {remote_temp_binary}"
        );
        // dev 产物有数百 MB,用 DEV_UPLOAD_TIMEOUT。
        // 不能直接覆盖最终 binary:若旧 remote-server 正在执行,或多个 reconnect
        // 并发安装同时 scp 同一路径,Linux 会返回 ETXTBSY/Text file busy。
        // 先上传唯一临时文件,再 chmod + mv -f 原子替换。
        remote_server::ssh::scp_upload_for_target(
            socket_path,
            ssh_target,
            &local_binary,
            &remote_temp_binary,
            remote_server::setup::DEV_UPLOAD_TIMEOUT,
        )
        .await?;
        promote_uploaded_binary(socket_path, ssh_target, &remote_temp_binary, &remote_binary)
            .await?;
    }

    if local_stamp_path.is_file() {
        if let Err(error) = upload_dev_remote_build_stamp(
            socket_path,
            ssh_target,
            &local_stamp_path,
            &remote_binary,
        )
        .await
        {
            log::warn!(
                "dev remote-server: 本地产物已上传,但更新远端 freshness stamp 失败: {error:#};\
                 下次检查会重新安装"
            );
        }
    } else {
        log::warn!(
            "dev remote-server: 本地产物已上传,但缺少 freshness stamp {};下次检查会重新安装",
            local_stamp_path.display()
        );
    }
    stop_remote_environment_daemons(socket_path, ssh_target).await?;

    // 复用既有校验逻辑确认上传的二进制可运行。
    verify_installed_binary(socket_path, ssh_target).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_remote_target_root_defaults_to_isolated_cache() {
        let root = Path::new("/workspace/ashide");

        let target_root = dev_remote_target_root_from_env(root, None);

        assert_eq!(
            target_root,
            PathBuf::from("/workspace/ashide/target/dev-remote")
        );
    }

    #[test]
    fn dev_remote_target_root_honors_explicit_override() {
        let root = Path::new("/workspace/ashide");

        let target_root =
            dev_remote_target_root_from_env(root, Some(OsString::from("/tmp/ashide-remote-cache")));

        assert_eq!(target_root, PathBuf::from("/tmp/ashide-remote-cache"));
    }

    #[test]
    fn dev_remote_target_selection_matches_linux_arch() {
        assert_eq!(
            dev_musl_target_for_platform(&RemotePlatform {
                os: RemoteOs::Linux,
                arch: RemoteArch::X86_64,
            })
            .unwrap(),
            remote_server::setup::DEV_MUSL_TARGET
        );
        assert_eq!(
            dev_musl_target_for_platform(&RemotePlatform {
                os: RemoteOs::Linux,
                arch: RemoteArch::Aarch64,
            })
            .unwrap(),
            remote_server::setup::DEV_AARCH64_MUSL_TARGET
        );
    }

    #[test]
    fn dev_remote_binary_path_uses_isolated_target_profile() {
        let target_root = Path::new("/workspace/ashide/target/dev-remote");

        let binary = dev_remote_binary_path(
            target_root,
            remote_server::setup::DEV_AARCH64_MUSL_TARGET,
            "ashide",
        );

        assert_eq!(
            binary,
            PathBuf::from(
                "/workspace/ashide/target/dev-remote/aarch64-unknown-linux-musl/dev-remote/ashide"
            )
        );
    }

    #[test]
    fn dev_remote_source_bin_name_is_cargo_target_not_channel_cli_name() {
        assert_eq!(dev_remote_source_bin_name(), "ashide");
        assert_ne!(dev_remote_source_bin_name(), "ashide-dev");
    }

    #[test]
    fn dev_remote_installed_stamp_path_sits_next_to_remote_binary() {
        assert_eq!(
            dev_remote_installed_stamp_path("~/.ashide-dev/remote-server/ashide-dev-pty-v1"),
            "~/.ashide-dev/remote-server/ashide-dev-pty-v1.stamp"
        );
    }

    #[test]
    fn dev_remote_build_freshness_requires_binary_and_matching_stamp() {
        let tempdir = tempfile::tempdir().unwrap();
        let binary = tempdir.path().join("ashide");
        let stamp = tempdir.path().join(DEV_REMOTE_BUILD_STAMP_FILE);

        assert!(!dev_remote_build_is_fresh(&binary, &stamp, "expected\n"));

        fs::write(&binary, "binary").unwrap();
        fs::write(&stamp, "old\n").unwrap();
        assert!(!dev_remote_build_is_fresh(&binary, &stamp, "expected\n"));

        fs::write(&stamp, "expected\n").unwrap();
        assert!(dev_remote_build_is_fresh(&binary, &stamp, "expected\n"));
    }

    #[test]
    fn dev_remote_binary_can_refresh_stale_stamp_when_newer_than_inputs() {
        let tempdir = tempfile::tempdir().unwrap();
        let root = tempdir.path();
        let input = root.join("input.txt");
        let binary = root.join("ashide");

        fs::write(&input, "input").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        fs::write(&binary, "binary").unwrap();

        assert!(dev_remote_binary_covers_paths(&binary, &[input.clone()]).unwrap());

        std::thread::sleep(std::time::Duration::from_millis(5));
        fs::write(&input, "changed").unwrap();
        assert!(!dev_remote_binary_covers_paths(&binary, &[input]).unwrap());
    }

    #[test]
    fn dev_remote_build_input_scopes_do_not_cover_all_app_sources() {
        for forbidden in [
            "app",
            "app/src/lib.rs",
            "app/src/workspace/environment_runtime.rs",
            "app/src/terminal/model/session/command_executor",
            "app/src/remote_server/unix",
            "crates",
            "resources",
            "crates/command",
            "crates/remote_server",
            "crates/repo_metadata",
            "crates/warp_cli",
            "crates/warp_core",
            "crates/warp_files",
            "crates/warp_terminal/src/shell",
            "crates/warp_util",
            "crates/warpui",
            "crates/warpui_core",
        ] {
            assert!(!DEV_REMOTE_BUILD_INPUT_SCOPES.contains(&forbidden));
        }
        assert!(DEV_REMOTE_BUILD_INPUT_SCOPES.contains(&"app/src/remote_server/server_model.rs"));
        assert!(DEV_REMOTE_BUILD_INPUT_SCOPES.contains(&"app/src/remote_server/unix/proxy.rs"));
        assert!(DEV_REMOTE_BUILD_INPUT_SCOPES.contains(
            &"app/src/terminal/model/session/command_executor/local_command_executor.rs"
        ));
        assert!(DEV_REMOTE_BUILD_INPUT_SCOPES
            .contains(&"crates/remote_server/proto/remote_server.proto"));
        assert!(
            DEV_REMOTE_BUILD_INPUT_SCOPES.contains(&"crates/remote_server/src/runtime_paths.rs")
        );
        assert!(DEV_REMOTE_BUILD_INPUT_SCOPES.contains(&"crates/warp_cli/src/lib.rs"));
        assert!(DEV_REMOTE_BUILD_INPUT_SCOPES.contains(&"crates/warp_terminal/src/shell/mod.rs"));
    }

    #[test]
    fn write_dev_remote_build_stamp_replaces_existing_stamp() {
        let tempdir = tempfile::tempdir().unwrap();
        let stamp = tempdir
            .path()
            .join("nested")
            .join(DEV_REMOTE_BUILD_STAMP_FILE);

        write_dev_remote_build_stamp(&stamp, "first\n").unwrap();
        write_dev_remote_build_stamp(&stamp, "second\n").unwrap();

        assert_eq!(fs::read_to_string(stamp).unwrap(), "second\n");
    }
}
