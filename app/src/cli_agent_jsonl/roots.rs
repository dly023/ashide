//! CLI-agent store root resolution and path helpers.

#[cfg(feature = "local_fs")]
use std::fs;
use std::path::PathBuf;

#[cfg(feature = "local_fs")]
use super::error::CliAgentSessionScanError;

#[cfg(feature = "local_fs")]
pub(crate) fn require_cli_agent_home(
    home: Option<PathBuf>,
) -> Result<PathBuf, CliAgentSessionScanError> {
    home.ok_or_else(CliAgentSessionScanError::home_directory_unavailable)
}

#[cfg(feature = "local_fs")]
pub(crate) fn current_cli_agent_home() -> Result<PathBuf, CliAgentSessionScanError> {
    require_cli_agent_home(dirs::home_dir())
}

#[cfg(feature = "local_fs")]
pub(crate) fn resolve_current_process_cli_agent_store_roots(
) -> Result<CliAgentStoreRoots, CliAgentSessionScanError> {
    Ok(CliAgentStoreRoots::for_current_process(
        current_cli_agent_home()?,
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CliAgentStoreRoots {
    pub(crate) home_dir: PathBuf,
    pub(crate) claude_config_dir: PathBuf,
    pub(crate) codex_home: PathBuf,
    pub(crate) jcode_home: PathBuf,
    pub(crate) omp_agent_home: PathBuf,
}

impl CliAgentStoreRoots {
    pub(crate) fn for_home(home_dir: PathBuf) -> Self {
        Self {
            claude_config_dir: home_dir.join(".claude"),
            codex_home: home_dir.join(".codex"),
            jcode_home: home_dir.join(".jcode"),
            omp_agent_home: home_dir.join(".omp/agent"),
            home_dir,
        }
    }

    pub(crate) fn for_current_process(home_dir: PathBuf) -> Self {
        let mut roots = Self::for_home(home_dir);
        if let Some(path) = std::env::var_os("CLAUDE_CONFIG_DIR") {
            roots.claude_config_dir = process_config_root(PathBuf::from(path));
        }
        if let Some(path) = std::env::var_os("CODEX_HOME") {
            roots.codex_home = process_config_root(PathBuf::from(path));
        }
        roots
    }

    pub(crate) fn from_explicit_target_paths(
        home_dir: PathBuf,
        claude_config_dir: PathBuf,
        codex_home: PathBuf,
    ) -> Result<Self, String> {
        for (label, path) in [
            ("home_dir", &home_dir),
            ("claude_config_dir", &claude_config_dir),
            ("codex_home", &codex_home),
        ] {
            if path.as_os_str().is_empty() {
                return Err(format!("CLI-agent target store root {label} is empty"));
            }
            if !path.is_absolute() {
                return Err(format!(
                    "CLI-agent target store root {label} is not absolute: {}",
                    path.display()
                ));
            }
        }
        let jcode_home = home_dir.join(".jcode");
        let omp_agent_home = home_dir.join(".omp/agent");
        Ok(Self {
            home_dir,
            claude_config_dir,
            codex_home,
            jcode_home,
            omp_agent_home,
        })
    }

    pub(crate) fn claude_projects(&self) -> PathBuf {
        self.claude_config_dir.join("projects")
    }

    pub(crate) fn codex_sessions(&self) -> PathBuf {
        self.codex_home.join("sessions")
    }

    pub(crate) fn codex_index(&self) -> PathBuf {
        self.codex_home.join("session_index.jsonl")
    }

    pub(crate) fn jcode_sessions(&self) -> PathBuf {
        self.jcode_home.join("sessions")
    }

    pub(crate) fn omp_sessions(&self) -> PathBuf {
        self.omp_agent_home.join("sessions")
    }
}

fn process_config_root(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        return path;
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(&path))
        .unwrap_or(path)
}

#[cfg(feature = "local_fs")]
pub(crate) fn normalize_cli_agent_session_cwd(
    value: Option<&str>,
    roots: &CliAgentStoreRoots,
) -> Option<String> {
    let home = &roots.home_dir;
    let value = value?.trim();
    if value.is_empty() || value.starts_with("remote:") || value.starts_with("ssh:") {
        return None;
    }
    let expanded = if value == "~" {
        home.to_path_buf()
    } else if let Some(relative) = value.strip_prefix("~/") {
        home.join(relative)
    } else {
        PathBuf::from(value)
    };
    if !expanded.is_absolute() || !expanded.is_dir() {
        return None;
    }

    let real_value = fs::canonicalize(&expanded).unwrap_or_else(|_| expanded.clone());
    for store in [
        roots.claude_config_dir.clone(),
        roots.codex_sessions(),
        roots.codex_index(),
        roots.jcode_sessions(),
        roots.omp_sessions(),
    ] {
        let real_store = fs::canonicalize(&store).unwrap_or(store);
        if real_value == real_store || real_value.starts_with(&real_store) {
            return None;
        }
    }
    Some(expanded.to_string_lossy().into_owned())
}
