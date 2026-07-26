use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use warp_completer::completer::{CommandExitStatus, CommandOutput};
use warp_core::command::ExitCode;
use warp_core::SessionId;

use crate::terminal::model::session::command_executor::{CommandExecutor, ExecuteCommandOptions};
use crate::terminal::shell::Shell;
use crate::workspace::environment_runtime::{self, EnvironmentRuntimeClient};

/// `CommandExecutor` implementation that executes commands via a persistent
/// `environment runtime` helper running on the environment host.
///
/// The executor is always constructed with a live environment runtime client
/// after the session reached the `Connected` state. The runtime registry owns
/// the authoritative per-session client; this executor holds a cloned `Arc` to
/// the same underlying channels and transitively keeps them alive as long as
/// the `Session` is alive.
///
/// If the underlying environment transport is torn down mid-session,
/// [`EnvironmentRuntimeClient::run_command`] will fail naturally and
/// [`execute_command`] surfaces that as an `Err`. We deliberately do *not*
/// silently synthesize an empty `Ok(CommandOutput)` for the disconnected
/// case, because callers (notably the completions/syntax-highlighting
/// pipeline) treat `Ok(empty)` as "there are zero top-level commands" and
/// produce incorrect results.
pub struct EnvironmentRuntimeCommandExecutor {
    session_id: SessionId,
    client: Arc<EnvironmentRuntimeClient>,
}

impl std::fmt::Debug for EnvironmentRuntimeCommandExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnvironmentRuntimeCommandExecutor")
            .field("session_id", &self.session_id)
            .finish()
    }
}

impl EnvironmentRuntimeCommandExecutor {
    /// Creates a new executor backed by an already-connected
    /// [`EnvironmentRuntimeClient`].
    pub fn new(session_id: SessionId, client: Arc<EnvironmentRuntimeClient>) -> Self {
        Self { session_id, client }
    }
}

#[async_trait]
impl CommandExecutor for EnvironmentRuntimeCommandExecutor {
    async fn execute_command(
        &self,
        command: &str,
        _shell: &Shell,
        current_directory_path: Option<&str>,
        environment_variables: Option<HashMap<String, String>>,
        _execute_command_options: ExecuteCommandOptions,
    ) -> Result<CommandOutput> {
        let output = environment_runtime::run_command_output(
            &self.client,
            self.session_id,
            command.to_owned(),
            current_directory_path.map(ToOwned::to_owned),
            environment_variables.unwrap_or_default(),
        )
        .await
        .map_err(|e| {
            anyhow!(
                "Environment runtime command failed (session={:?}): {e}",
                self.session_id
            )
        })?;

        let status = match output.exit_code {
            Some(0) => CommandExitStatus::Success,
            _ => CommandExitStatus::Failure,
        };
        Ok(CommandOutput {
            stdout: output.stdout,
            stderr: output.stderr,
            status,
            exit_code: output.exit_code.map(ExitCode::from),
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    /// Environment Runtime multiplexes commands over one managed transport connection,
    /// so parallel execution is safe (unlike the legacy SSH fallback executor which
    /// opens a new SSH session per command and is limited by `MaxSessions`).
    fn supports_parallel_command_execution(&self) -> bool {
        true
    }
}
