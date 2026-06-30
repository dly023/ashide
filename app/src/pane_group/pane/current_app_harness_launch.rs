use std::{collections::HashMap, ffi::OsString, path::PathBuf};

use shell_words::quote as shell_quote;
use uuid::Uuid;
use warp_cli::agent::Harness;
use warp_managed_secrets::ManagedSecretValue;

use crate::ai::{
    agent_sdk::{
        driver::AgentDriverError, task_env_vars, validate_cli_installed, ClaudeHarness,
        ThirdPartyHarness,
    },
    ambient_agents::{task::HarnessConfig, AgentConfigSnapshot, AmbientAgentTaskId},
};
use crate::terminal::cli_agent_sessions::plugin_manager::plugin_manager_for;
use crate::terminal::shell::ShellType;

#[derive(Clone)]
pub(super) struct PreparedCurrentAppHarnessLaunch {
    pub command: String,
    pub env_vars: HashMap<OsString, OsString>,
    pub run_id: String,
    pub task_id: AmbientAgentTaskId,
}

pub(super) fn normalize_current_app_child_harness(harness_type: &str) -> Option<Harness> {
    Harness::parse_current_app_child_harness(harness_type)
}

pub(super) fn validate_current_app_harness_shell(
    shell_type: Option<ShellType>,
) -> Result<(), String> {
    match shell_type {
        Some(ShellType::Bash) | Some(ShellType::Zsh) | Some(ShellType::Fish) => Ok(()),
        Some(ShellType::PowerShell) => Err(
            "Current-app child harnesses currently require bash, zsh, or fish; PowerShell is not supported."
                .to_string(),
        ),
        None => Err(
            "Current-app child harnesses currently require a detected bash, zsh, or fish session."
                .to_string(),
        ),
    }
}

pub(super) fn build_current_app_claude_child_command(prompt: &str) -> String {
    let session_id = Uuid::new_v4();
    let quoted_prompt = shell_quote(prompt);
    // Current-app child harness panes are launched off-screen. We intentionally skip
    // Claude's own permission prompts here so the child can start unattended
    // instead of hanging on an approval UI the user cannot see in that hidden
    // pane.
    format!("claude --session-id {session_id} --dangerously-skip-permissions {quoted_prompt}")
}

pub(super) fn build_current_app_opencode_child_command(prompt: &str) -> String {
    let quoted_prompt = shell_quote(prompt);
    format!("opencode --prompt {quoted_prompt}")
}

fn current_app_child_task_config(harness: Harness) -> Option<AgentConfigSnapshot> {
    match harness {
        Harness::Oz | Harness::OpenCode | Harness::Gemini | Harness::Unknown => None,
        Harness::Claude => Some(AgentConfigSnapshot {
            harness: Some(HarnessConfig::from_harness_type(harness)),
            ..Default::default()
        }),
    }
}

pub(super) async fn prepare_current_app_harness_child_launch(
    prompt: String,
    harness_type: String,
    parent_run_id: Option<String>,
    shell_type: Option<ShellType>,
    startup_directory: Option<PathBuf>,
) -> Result<PreparedCurrentAppHarnessLaunch, String> {
    let Some(harness) = normalize_current_app_child_harness(&harness_type) else {
        let harness_name = harness_type.trim();
        return Err(if harness_name.is_empty() {
            "Current-app child harness type is missing.".to_string()
        } else {
            format!("Unsupported current-app child harness '{harness_name}'.")
        });
    };
    validate_current_app_harness_shell(shell_type)?;
    let command = match harness {
        Harness::Oz => unreachable!("normalize_current_app_child_harness filters out Oz"),
        Harness::Unknown => unreachable!("normalize_current_app_child_harness filters out Unknown"),
        Harness::Claude => {
            let working_dir = startup_directory
                .or_else(|| std::env::current_dir().ok())
                .ok_or_else(|| {
                    "Could not resolve a working directory for the current-app Claude child."
                        .to_string()
                })?;
            let claude_harness = ClaudeHarness;
            claude_harness
                .validate()
                .map_err(|error: AgentDriverError| error.to_string())?;
            // Current-app child harness panes inherit the user's existing Claude
            // auth/session state from the current app context. We still prepare
            // Claude's config files here, but there are no Ashide-managed secrets
            // to materialize into the hidden child pane.
            let managed_secrets: HashMap<String, ManagedSecretValue> = HashMap::new();
            claude_harness
                .prepare_environment_config(&working_dir, None, &managed_secrets)
                .map_err(|error: AgentDriverError| error.to_string())?;
            if let Some(manager) = plugin_manager_for(claude_harness.cli_agent()) {
                if let Err(error) = manager.install().await {
                    log::warn!("Claude plugin installation failed for child harness: {error}");
                }
            }

            build_current_app_claude_child_command(&prompt)
        }
        Harness::OpenCode => {
            validate_cli_installed("opencode", Some("https://opencode.ai/docs"))
                .map_err(|error: AgentDriverError| error.to_string())?;
            build_current_app_opencode_child_command(&prompt)
        }
        Harness::Gemini => unreachable!("normalize_current_app_child_harness filters out Gemini"),
    };

    // Ashide: current-app harness 启动子 task 在本地直接生成 UUID v4 作为 task_id。
    // 保留 `current_app_child_task_config(harness)` 调用以匹配签名;当前未使用其返回值。
    let _ = current_app_child_task_config(harness);
    let task_id = AmbientAgentTaskId::new_local();

    Ok(PreparedCurrentAppHarnessLaunch {
        command,
        env_vars: task_env_vars(Some(&task_id), parent_run_id.as_deref(), harness),
        run_id: task_id.to_string(),
        task_id,
    })
}

#[cfg(test)]
#[path = "current_app_harness_launch_tests.rs"]
mod tests;
