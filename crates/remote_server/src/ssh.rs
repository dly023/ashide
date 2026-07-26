use std::path::Path;
use std::process::{Output, Stdio};
use std::time::Duration;

use anyhow::{anyhow, Result};
use command::r#async::Command;
use warpui::r#async::FutureExt as _;

/// Timeout for `ssh -O exit`. The command only talks to the local
/// ControlMaster over a Unix socket, so it should return almost
/// immediately; if it doesn't, we'd rather give up than block
/// teardown.
const STOP_CONTROL_MASTER_TIMEOUT: Duration = Duration::from_secs(5);

/// Builds the common SSH argument list for multiplexed connections through
/// an existing ControlMaster socket.
pub fn ssh_args(socket_path: &Path) -> Vec<String> {
    ssh_args_for_target(socket_path, "placeholder@placeholder")
}

pub fn ssh_args_for_target(socket_path: &Path, target: &str) -> Vec<String> {
    vec![
        "-q".to_string(),
        "-o".to_string(),
        "PasswordAuthentication=no".to_string(),
        "-o".to_string(),
        "ForwardX11=no".to_string(),
        "-o".to_string(),
        "ServerAliveInterval=30".to_string(),
        "-o".to_string(),
        "ServerAliveCountMax=6".to_string(),
        "-o".to_string(),
        "TCPKeepAlive=yes".to_string(),
        "-o".to_string(),
        format!("ControlPath={}", socket_path.display()),
        target.to_string(),
    ]
}

/// Runs `ssh -O exit -o ControlPath=<socket_path>` to force the local
/// SSH `ControlMaster` managing `socket_path` to exit immediately,
/// without waiting for multiplexed channels to finish draining.
///
/// The user's interactive ssh is spawned with `-o ControlMaster=yes` by
/// `warp_ssh_helper`, so it is both the interactive session and the
/// multiplex master. When the user's remote shell exits, that ssh can
/// hang waiting for half-closed slave channels (e.g. from
/// `ssh ... remote-server-proxy`) to finish cleanup on the remote
/// side. Sending `-O exit` bypasses that wait.
///
/// **Only safe to call once the user's shell has already exited** --
/// this tears down the interactive ssh outright. In practice it is
/// invoked from the `ExitShell` teardown path on the client.
///
/// Fire-and-forget. Errors are logged but not propagated: at teardown
/// time there is nothing useful to do with them.
pub async fn stop_control_master(socket_path: &Path) {
    let args = ssh_args(socket_path);
    let result = async {
        Command::new("ssh")
            .arg("-O")
            .arg("exit")
            .args(&args)
            .kill_on_drop(true)
            .output()
            .await
    }
    .with_timeout(STOP_CONTROL_MASTER_TIMEOUT)
    .await;

    match result {
        Ok(Ok(output)) if output.status.success() => {
            log::info!(
                "stop_control_master: `ssh -O exit` succeeded for {}",
                socket_path.display()
            );
        }
        Ok(Ok(output)) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            log::info!(
                "stop_control_master: `ssh -O exit` for {} exited with {:?}: {stderr}",
                socket_path.display(),
                output.status.code(),
            );
        }
        Ok(Err(e)) => {
            log::info!(
                "stop_control_master: failed to spawn `ssh -O exit` for {}: {e}",
                socket_path.display()
            );
        }
        Err(_) => {
            log::warn!(
                "stop_control_master: `ssh -O exit` for {} timed out after {:?}",
                socket_path.display(),
                STOP_CONTROL_MASTER_TIMEOUT,
            );
        }
    }
}

/// Run a single SSH command through the ControlMaster socket and return a result where:
/// - `Err` for transport-level failures (e.g. couldn't spawn `ssh`, or timeout).
/// - `Ok(output)` callers should check `output.status` to distinguish a successful remote command from a non-zero remote exit.
pub async fn run_ssh_command_for_target(
    socket_path: &Path,
    target: &str,
    remote_command: &str,
    timeout: Duration,
) -> Result<Output> {
    async {
        Command::new("ssh")
            .args(ssh_args_for_target(socket_path, target))
            .arg(remote_command)
            .kill_on_drop(true)
            .output()
            .await
    }
    .with_timeout(timeout)
    .await
    .map_err(|_| anyhow!("SSH command timed out after {timeout:?}"))?
    .map_err(|e| anyhow!("SSH command failed to execute: {e}"))
}

/// Pipe a script into `bash -s` on the remote host via the ControlMaster
/// socket. Returns a result where:
/// - `Err` for transport-level failures (e.g. couldn't spawn `ssh`, or timeout).
/// - `Ok(output)` callers should check `output.status` to distinguish a successful remote script from a non-zero remote exit.
///
/// We pipe via stdin rather than passing the script as an SSH command-line
/// argument because the install script is multi-line and contains shell
/// constructs (case statements, variable expansions, single/double quotes)
/// that would require complex, fragile escaping if passed as an argument.
/// The `bash -s` + stdin approach avoids all escaping issues and has no
/// argument length limits.
pub async fn run_ssh_script_for_target(
    socket_path: &Path,
    target: &str,
    script: &str,
    timeout: Duration,
) -> Result<Output> {
    let mut child = Command::new("ssh")
        .args(ssh_args_for_target(socket_path, target))
        .arg("bash -s")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| anyhow!("Failed to spawn SSH for script: {e}"))?;

    // Write the script to stdin.
    if let Some(mut stdin) = child.stdin.take() {
        use futures_lite::io::AsyncWriteExt;
        stdin
            .write_all(script.as_bytes())
            .await
            .map_err(|e| anyhow!("Failed to write script to stdin: {e}"))?;
        // Close stdin so the remote bash exits after reading the script.
        drop(stdin);
    }

    child
        .output()
        .with_timeout(timeout)
        .await
        .map_err(|_| anyhow!("Script timed out after {timeout:?}"))?
        .map_err(|e| anyhow!("Script failed: {e}"))
}

fn quote_posix_shell_argument(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// 通过已有 ControlMaster 的 SSH command channel 流式上传单个文件。
///
/// 远端只需要 POSIX shell 与 `cat`，不依赖 rsync、SCP legacy protocol 或
/// SFTP subsystem。调用方必须传入唯一临时路径，并在成功后自行原子 promote。
pub async fn ssh_upload_file_for_target(
    socket_path: &Path,
    target: &str,
    local_path: &Path,
    remote_path: &str,
    timeout: Duration,
) -> Result<()> {
    let output = async {
        let mut source = async_fs::File::open(local_path)
            .await
            .map_err(|error| anyhow!("Failed to open {}: {error}", local_path.display()))?;
        let remote_command = format!("cat > {}", quote_posix_shell_argument(remote_path));
        let mut child = Command::new("ssh")
            .args(ssh_args_for_target(socket_path, target))
            .arg(remote_command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| anyhow!("Failed to spawn SSH upload: {error}"))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("SSH upload stdin is unavailable"))?;
        futures_lite::io::copy(&mut source, &mut stdin)
            .await
            .map_err(|error| {
                anyhow!(
                    "Failed to stream {} over SSH: {error}",
                    local_path.display()
                )
            })?;
        drop(stdin);
        child
            .output()
            .await
            .map_err(|error| anyhow!("SSH upload failed: {error}"))
    }
    .with_timeout(timeout)
    .await
    .map_err(|_| anyhow!("SSH upload timed out after {timeout:?}"))??;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(anyhow!("SSH upload failed: {stderr}"))
}

#[cfg(test)]
mod tests {
    use super::quote_posix_shell_argument;

    #[test]
    fn ssh_stdin_upload_quotes_remote_path() {
        assert_eq!(
            quote_posix_shell_argument("/tmp/helper upload'one"),
            "'/tmp/helper upload'\\''one'"
        );
    }
}
