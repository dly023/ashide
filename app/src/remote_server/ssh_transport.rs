//! SSH-specific implementation of [`RemoteTransport`].
//!
//! [`SshTransport`] uses an existing SSH ControlMaster socket to check/install
//! the remote server binary and to launch the `environment-runtime-proxy` process
//! whose stdin/stdout become the protocol channel.
use std::fmt;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;
use warpui::r#async::executor;

use remote_server::auth::RemoteServerAuthContext;
use remote_server::client::RemoteServerClient;
use remote_server::runtime_paths;
use remote_server::setup::{PreinstallCheckResult, RemotePlatform};
use remote_server::ssh::ssh_args_for_target;
use remote_server::transport::{Connection, RemoteTransport};

use super::dev_remote_install::{
    detect_remote_platform, dev_install_local_binary, dev_musl_target_for_platform,
    dev_remote_source_bin_name, expected_dev_remote_build_stamp, release_install_local_binary,
    remote_dev_build_stamp_matches, workspace_root,
};

/// SSH transport: connects via a ControlMaster socket.
///
/// `socket_path` is the local Unix socket created by the ControlMaster
/// process (`ssh -N -o ControlMaster=yes -o ControlPath=<path>`). All SSH
/// commands (binary check, install, proxy launch) are multiplexed through
/// this socket without re-authenticating.
#[derive(Clone)]
pub struct SshTransport {
    socket_path: PathBuf,
    ssh_target: String,
    auth_context: Arc<RemoteServerAuthContext>,
}

impl fmt::Debug for SshTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SshTransport")
            .field("socket_path", &self.socket_path)
            .finish_non_exhaustive()
    }
}

impl SshTransport {
    /// Builds a transport bound to a ControlMaster socket only (see
    /// `environment_runtime::transport_for_control_path`). The placeholder host
    /// is intentional and harmless: this constructor is used solely for
    /// socket-scoped commands where the remote host is irrelevant (the same
    /// pattern as `remote_server::ssh::ssh_args`). Host-sensitive operations
    /// (scp / proxy launch) go through [`Self::new_with_target`] with a real
    /// `user@host`.
    pub fn new(socket_path: PathBuf, auth_context: Arc<RemoteServerAuthContext>) -> Self {
        Self::new_with_target(
            socket_path,
            "placeholder@placeholder".to_owned(),
            auth_context,
        )
    }

    pub fn new_with_target(
        socket_path: PathBuf,
        ssh_target: String,
        auth_context: Arc<RemoteServerAuthContext>,
    ) -> Self {
        Self {
            socket_path,
            ssh_target,
            auth_context,
        }
    }

    pub fn socket_path(&self) -> &PathBuf {
        &self.socket_path
    }

    pub fn remote_daemon_socket_path(&self) -> String {
        format!(
            "{}/server.sock",
            runtime_paths::remote_server_daemon_dir(
                &self.auth_context.remote_server_identity_key()
            )
        )
    }

    pub fn remote_daemon_pid_path(&self) -> String {
        format!(
            "{}/server.pid",
            runtime_paths::remote_server_daemon_dir(
                &self.auth_context.remote_server_identity_key()
            )
        )
    }

    fn ssh_target(&self) -> &str {
        &self.ssh_target
    }

    fn remote_proxy_command(&self) -> String {
        let binary = remote_server::setup::remote_server_binary();
        let identity_key = self.auth_context.remote_server_identity_key();
        let quoted_identity_key = shell_words::quote(&identity_key);
        format!("{binary} environment-runtime-proxy --identity-key {quoted_identity_key}")
    }
}

impl RemoteTransport for SshTransport {
    fn detect_platform(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<RemotePlatform, String>> + Send>> {
        let socket_path = self.socket_path.clone();
        let ssh_target = self.ssh_target().to_owned();
        Box::pin(async move {
            detect_remote_platform(&socket_path, &ssh_target)
                .await
                .map_err(|e| format!("{e:#}"))
        })
    }

    fn run_preinstall_check(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<PreinstallCheckResult, String>> + Send>> {
        let socket_path = self.socket_path.clone();
        let ssh_target = self.ssh_target().to_owned();
        Box::pin(async move {
            match remote_server::ssh::run_ssh_script_for_target(
                &socket_path,
                &ssh_target,
                remote_server::setup::PREINSTALL_CHECK_SCRIPT,
                remote_server::setup::CHECK_TIMEOUT,
            )
            .await
            {
                Ok(output) if output.status.success() => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    Ok(PreinstallCheckResult::parse(&stdout))
                }
                Ok(output) => {
                    let code = output.status.code().unwrap_or(-1);
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    Err(format!(
                        "Preinstall check exited with code {code}: {stderr}"
                    ))
                }
                Err(e) => Err(format!("{e:#}")),
            }
        })
    }

    fn check_binary(&self) -> Pin<Box<dyn Future<Output = Result<bool, String>> + Send>> {
        let socket_path = self.socket_path.clone();
        let ssh_target = self.ssh_target().to_owned();
        Box::pin(async move {
            let bin_path = remote_server::setup::remote_server_binary();
            log::info!("Checking for remote server binary at {bin_path}");
            match remote_server::ssh::run_ssh_command_for_target(
                &socket_path,
                &ssh_target,
                &remote_server::setup::binary_check_command(),
                remote_server::setup::CHECK_TIMEOUT,
            )
            .await
            {
                // binary_check_command 既检查存在/可执行,也检查当前客户端协议能力。
                // 0 表示可复用;126/127 表示缺失或不可执行;1/2 常见于 grep 未匹配
                // 或旧 clap 二进制不认识当前子命令/参数,都应触发 update/install。
                Ok(output) => match output.status.code() {
                    Some(0) => {
                        if !remote_server::setup::is_dev_source_build() {
                            return Ok(true);
                        }

                        let musl_target = match detect_remote_platform(&socket_path, &ssh_target)
                            .await
                            .and_then(|platform| dev_musl_target_for_platform(&platform))
                        {
                            Ok(target) => target,
                            Err(error) => {
                                return Err(format!(
                                    "dev remote-server: 无法判断远端 helper target: {error:#}"
                                ));
                            }
                        };
                        let expected_stamp = match expected_dev_remote_build_stamp(
                            &workspace_root(),
                            musl_target,
                            dev_remote_source_bin_name(),
                        ) {
                            Ok(stamp) => stamp,
                            Err(error) => {
                                return Err(format!(
                                    "dev remote-server: 无法计算远端 freshness stamp: {error:#}"
                                ));
                            }
                        };
                        match remote_dev_build_stamp_matches(
                            &socket_path,
                            &ssh_target,
                            &bin_path,
                            &expected_stamp,
                        )
                        .await
                        {
                            Ok(true) => Ok(true),
                            Ok(false) => {
                                log::info!(
                                    "dev remote-server: 远端 helper stamp 缺失或已过期,需要安装"
                                );
                                Ok(false)
                            }
                            Err(error) => Err(format!(
                                "dev remote-server: 远端 helper stamp 检查失败: {error:#}"
                            )),
                        }
                    }
                    Some(1) | Some(2) | Some(126) | Some(127) => Ok(false),
                    Some(code) => {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        Err(format!("binary check exited with code {code}: {stderr}"))
                    }
                    None => Err("binary check terminated by signal".into()),
                },
                Err(e) => Err(format!("{e:#}")),
            }
        })
    }

    fn check_has_old_binary(&self) -> Pin<Box<dyn Future<Output = anyhow::Result<bool>> + Send>> {
        let socket_path = self.socket_path.clone();
        let ssh_target = self.ssh_target().to_owned();
        Box::pin(async move {
            // Treat the existence of the remote-server install directory
            // itself as evidence of a prior install. If `~/.ashide-XX/remote-server`
            // exists, something was installed there before, so any mismatch
            // with the client's expected binary path should be auto-updated
            // rather than surfaced as a first-time install prompt.
            let cmd = format!("test -d {}", runtime_paths::remote_server_dir());
            let output = remote_server::ssh::run_ssh_command_for_target(
                &socket_path,
                &ssh_target,
                &cmd,
                remote_server::setup::CHECK_TIMEOUT,
            )
            .await?;
            // `test -d` exits 0 when present, 1 when missing.
            // Anything else is treated as a check failure.
            match output.status.code() {
                Some(0) => Ok(true),
                Some(1) => Ok(false),
                Some(code) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    Err(anyhow::anyhow!(
                        "remote-server dir check exited with code {code}: {stderr}"
                    ))
                }
                None => Err(anyhow::anyhow!(
                    "remote-server dir check terminated by signal"
                )),
            }
        })
    }

    fn install_binary(&self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> {
        let socket_path = self.socket_path.clone();
        let ssh_target = self.ssh_target().to_owned();
        Box::pin(async move {
            log::info!(
                "Installing remote server binary to {}",
                remote_server::setup::remote_server_binary()
            );

            // Ashide fork:DEBUG 源码构建(无 release tag)走开发模式,
            // 交叉编译本地 `warp` 并上传,而不是下载陈旧的 GitHub release。
            // dev/source build 必须上传当前源码对应的 remote-server。回退到
            // GitHub latest 会把客户端新协议连到旧 binary,出现 unknown oneof /
            // “ClientMessage had no message variant set” 这类假连接成功。
            // 因此 dev 安装失败时直接把清晰错误暴露给 Environment Runtime,
            // 不再伪装成 release 下载安装。release 构建跳过整段逻辑,行为不变。
            if remote_server::setup::is_dev_source_build() {
                log::info!("dev remote-server: 检测到 DEBUG 源码构建,改用本地交叉编译安装");
                return dev_install_local_binary(&socket_path, &ssh_target)
                    .await
                    .map_err(|error| format!("dev remote-server 本地交叉编译安装失败: {error:#}"));
            }

            // local-first 交付:release 构建也由**本地** app 获取 helper 再上传给
            // 远端,远端不访问 GitHub(内网 / 离线远端也能用)。复用 dev 路径已验证的
            // 上传原语(rsync 增量,或 scp 临时文件 + 原子替换)。
            log::info!(
                "release remote-server: local-first 交付,本地拉取 helper 并上传(远端不访问 GitHub)"
            );
            release_install_local_binary(&socket_path, &ssh_target)
                .await
                .map_err(|error| format!("release remote-server 本地上传安装失败: {error:#}"))
        })
    }

    fn connect(
        &self,
        executor: Arc<executor::Background>,
    ) -> Pin<Box<dyn Future<Output = Result<Connection>> + Send>> {
        let socket_path = self.socket_path.clone();
        let ssh_target = self.ssh_target().to_owned();
        let remote_proxy_command = self.remote_proxy_command();
        Box::pin(async move {
            let mut args = ssh_args_for_target(&socket_path, &ssh_target);
            args.push(remote_proxy_command);

            // `kill_on_drop(true)` pairs with ownership of the `Child` being
            // returned in the [`Connection`] below: the
            // [`RemoteServerManager`] holds the `Child` on its per-session
            // state, and dropping that state (on explicit teardown or
            // spontaneous disconnect) sends SIGKILL to this ssh process.
            let mut child = command::r#async::Command::new("ssh")
                .args(&args)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn()?;

            let stdin = child
                .stdin
                .take()
                .ok_or_else(|| anyhow::anyhow!("Failed to capture child stdin"))?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| anyhow::anyhow!("Failed to capture child stdout"))?;
            let stderr = child
                .stderr
                .take()
                .ok_or_else(|| anyhow::anyhow!("Failed to capture child stderr"))?;

            let (client, event_rx) =
                RemoteServerClient::from_child_streams(stdin, stdout, stderr, &executor);
            Ok(Connection {
                client,
                event_rx,
                child,
                control_path: Some(socket_path),
            })
        })
    }

    fn remove_remote_server_binary(
        &self,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>> {
        let socket_path = self.socket_path.clone();
        let ssh_target = self.ssh_target().to_owned();
        Box::pin(async move {
            let cmd = format!("rm -f {}", remote_server::setup::remote_server_binary());
            log::info!("Removing stale remote server binary: {cmd}");
            let output = remote_server::ssh::run_ssh_command_for_target(
                &socket_path,
                &ssh_target,
                &cmd,
                remote_server::setup::CHECK_TIMEOUT,
            )
            .await?;
            if output.status.success() {
                Ok(())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Err(anyhow::anyhow!("Failed to remove binary: {stderr}"))
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use warpui::r#async::BoxFuture;
    fn static_auth_context() -> Arc<RemoteServerAuthContext> {
        Arc::new(RemoteServerAuthContext::new(
            || -> BoxFuture<'static, Option<String>> { Box::pin(async { None }) },
            || "user id/with spaces".to_string(),
        ))
    }

    #[test]
    fn remote_proxy_command_quotes_identity_key() {
        let transport = SshTransport::new(
            PathBuf::from("/tmp/control-master.sock"),
            static_auth_context(),
        );

        let command = transport.remote_proxy_command();

        assert!(command.contains("environment-runtime-proxy --identity-key"));
        assert!(command.contains("'user id/with spaces'"));
    }
}
