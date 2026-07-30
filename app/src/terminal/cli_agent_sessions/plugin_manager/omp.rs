use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::LazyLock;

use async_trait::async_trait;

use super::{CliAgentPluginManager, PluginInstructionStep, PluginInstructions};

/// The Ashide-bundled Omp extension source. Keep in sync with
/// `app/src/terminal/cli_agent_sessions/event/v1.rs` field names.
const BUNDLED_EXTENSION_SOURCE: &str = include_str!("ashide-omp.ts");

/// The filename Omp auto-discovery expects under ~/.omp/agent/extensions/.
const EXTENSION_FILENAME: &str = "ashide-omp.ts";

pub(super) struct OmpPluginManager;

impl OmpPluginManager {
    fn extensions_dir() -> io::Result<PathBuf> {
        let home_dir = dirs::home_dir().ok_or_else(|| {
            io::Error::new(io::ErrorKind::Other, "could not determine home directory")
        })?;
        Ok(home_dir.join(".omp/agent/extensions"))
    }

    fn extension_path() -> io::Result<PathBuf> {
        Ok(Self::extensions_dir()?.join(EXTENSION_FILENAME))
    }
}

#[async_trait]
impl CliAgentPluginManager for OmpPluginManager {
    fn minimum_plugin_version(&self) -> &'static str {
        "1.0.0"
    }

    fn can_auto_install(&self) -> bool {
        true
    }

    fn is_installed(&self) -> bool {
        Self::extension_path()
            .map(|path| path.exists())
            .unwrap_or(false)
    }

    fn needs_update(&self) -> bool {
        // The extension is bundled in the Ashide binary. If the file exists
        // but its content differs from the bundled source, it's stale.
        match Self::extension_path() {
            Ok(path) => {
                if !path.exists() {
                    return true;
                }
                fs::read_to_string(&path)
                    .map(|content| content != BUNDLED_EXTENSION_SOURCE)
                    .unwrap_or(true)
            }
            Err(_) => true,
        }
    }

    async fn install(&self) -> Result<(), super::PluginInstallError> {
        let mut log = String::new();
        let dir = Self::extensions_dir().map_err(|e| super::PluginInstallError {
            message: format!("failed to resolve Omp extensions dir: {e}"),
            log: log.clone(),
        })?;
        log.push_str(&format!("extensions dir: {}\n", dir.display()));

        fs::create_dir_all(&dir).map_err(|e| super::PluginInstallError {
            message: format!("failed to create Omp extensions dir: {e}"),
            log: log.clone(),
        })?;

        let path = dir.join(EXTENSION_FILENAME);
        fs::write(&path, BUNDLED_EXTENSION_SOURCE).map_err(|e| super::PluginInstallError {
            message: format!("failed to write Omp extension: {e}"),
            log: format!("{log}$ write {}\nerror: {e}", path.display()),
        })?;

        log.push_str(&format!("wrote {}\n", path.display()));
        Ok(())
    }

    async fn update(&self) -> Result<(), super::PluginInstallError> {
        // Update is the same as install — overwrite with the current bundled source.
        self.install().await
    }

    fn install_success_message(&self) -> &'static str {
        "Ashide Omp extension installed. Please restart the Omp session to activate."
    }

    fn update_success_message(&self) -> &'static str {
        "Ashide Omp extension updated. Please restart the Omp session to activate."
    }

    fn install_instructions(&self) -> &'static PluginInstructions {
        &INSTALL_INSTRUCTIONS
    }

    fn update_instructions(&self) -> &'static PluginInstructions {
        &UPDATE_INSTRUCTIONS
    }
}

static INSTALL_INSTRUCTIONS: LazyLock<PluginInstructions> = LazyLock::new(|| PluginInstructions {
    title: crate::t_static!("cli-agent-plugin-omp-install-title"),
    subtitle: crate::t_static!("cli-agent-plugin-omp-install-subtitle"),
    steps: vec![PluginInstructionStep {
        description: crate::t_static!("cli-agent-plugin-omp-install-step"),
        command: "",
        executable: false,
        link: None,
    }],
    post_install_notes: vec![crate::t_static!("cli-agent-plugin-omp-restart-note")],
});

static UPDATE_INSTRUCTIONS: LazyLock<PluginInstructions> = LazyLock::new(|| PluginInstructions {
    title: crate::t_static!("cli-agent-plugin-omp-update-title"),
    subtitle: crate::t_static!("cli-agent-plugin-omp-update-subtitle"),
    steps: vec![PluginInstructionStep {
        description: crate::t_static!("cli-agent-plugin-omp-update-step"),
        command: "",
        executable: false,
        link: None,
    }],
    post_install_notes: vec![crate::t_static!("cli-agent-plugin-omp-restart-update-note")],
});

#[cfg(test)]
#[path = "omp_tests.rs"]
mod tests;
