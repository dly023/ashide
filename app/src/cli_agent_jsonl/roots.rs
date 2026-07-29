//! CLI-agent store root resolution and path helpers.

#[cfg(feature = "local_fs")]
use std::fs;
use std::path::Path;
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
    pub(crate) omp_agent_home: PathBuf,
    pub(crate) opencode_data_dir: PathBuf,
    pub(crate) copilot_home: PathBuf,
    pub(crate) pi_agent_home: PathBuf,
}

impl CliAgentStoreRoots {
    pub(crate) fn for_home(home_dir: PathBuf) -> Self {
        Self {
            claude_config_dir: home_dir.join(".claude"),
            codex_home: home_dir.join(".codex"),
            omp_agent_home: home_dir.join(".omp/agent"),
            opencode_data_dir: home_dir.join(".local/share/opencode"),
            copilot_home: home_dir.join(".copilot"),
            pi_agent_home: home_dir.join(".pi/agent"),
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
        if let Some(path) = std::env::var_os("OPENCODE_CONFIG_DIR") {
            roots.opencode_data_dir = process_config_root(PathBuf::from(path));
        }
        if let Some(path) = std::env::var_os("COPILOT_HOME") {
            roots.copilot_home = process_config_root(PathBuf::from(path));
        }
        if let Some(path) = std::env::var_os("PI_CODING_AGENT_DIR") {
            roots.pi_agent_home = normalize_agent_home(PathBuf::from(path), ".pi");
        }
        if let Some(path) = std::env::var_os("OMP_CODING_AGENT_DIR") {
            roots.omp_agent_home = normalize_agent_home(PathBuf::from(path), ".omp");
        }
        roots
    }

    #[cfg(test)]
    pub(crate) fn from_explicit_target_paths(
        home_dir: PathBuf,
        claude_config_dir: PathBuf,
        codex_home: PathBuf,
    ) -> Result<Self, String> {
        let defaults = Self::for_home(home_dir.clone());
        Self::from_explicit_target_store_roots(
            home_dir,
            claude_config_dir,
            codex_home,
            defaults.opencode_data_dir,
            defaults.copilot_home,
            defaults.pi_agent_home,
            defaults.omp_agent_home,
        )
    }

    pub(crate) fn from_explicit_target_store_roots(
        home_dir: PathBuf,
        claude_config_dir: PathBuf,
        codex_home: PathBuf,
        opencode_data_dir: PathBuf,
        copilot_home: PathBuf,
        pi_agent_home: PathBuf,
        omp_agent_home: PathBuf,
    ) -> Result<Self, String> {
        for (label, path) in [
            ("home_dir", &home_dir),
            ("claude_config_dir", &claude_config_dir),
            ("codex_home", &codex_home),
            ("opencode_data_dir", &opencode_data_dir),
            ("copilot_home", &copilot_home),
            ("pi_agent_home", &pi_agent_home),
            ("omp_agent_home", &omp_agent_home),
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
        Ok(Self {
            home_dir,
            claude_config_dir,
            codex_home,
            omp_agent_home,
            opencode_data_dir,
            copilot_home,
            pi_agent_home,
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

    pub(crate) fn omp_sessions(&self) -> PathBuf {
        self.omp_agent_home.join("sessions")
    }

    pub(crate) fn droid_sessions(&self) -> PathBuf {
        self.home_dir.join(".factory/sessions")
    }

    pub(crate) fn droid_projects(&self) -> PathBuf {
        self.home_dir.join(".factory/projects")
    }

    pub(crate) fn opencode_legacy_sessions(&self) -> PathBuf {
        self.opencode_data_dir.join("storage/session")
    }

    pub(crate) fn opencode_databases_dir(&self) -> PathBuf {
        self.opencode_data_dir.clone()
    }

    pub(crate) fn copilot_sessions(&self) -> PathBuf {
        self.copilot_home.join("session-state")
    }

    pub(crate) fn pi_sessions(&self) -> PathBuf {
        self.pi_agent_home.join("sessions")
    }

    pub(crate) fn cursor_projects(&self) -> PathBuf {
        self.home_dir.join(".cursor/projects")
    }

    pub(crate) fn antigravity_brain(&self) -> PathBuf {
        self.home_dir.join(".gemini/antigravity-cli/brain")
    }
}

fn normalize_agent_home(path: PathBuf, default_dir: &str) -> PathBuf {
    if path.file_name().is_some_and(|name| name == "sessions") {
        return path.parent().map(Path::to_path_buf).unwrap_or(path);
    }
    if path.ends_with(default_dir) {
        return path.join("agent");
    }
    path
}

/// Omp 只把正本会话写为 `sessions/<project-bucket>/*.jsonl`。不能递归
/// 放宽到 tool log 等嵌套文件，否则 discovery 与 native mutation 的
/// source authority 会发生分叉。
pub(crate) fn is_omp_session_source(root: &Path, path: &Path) -> bool {
    path.parent()
        .and_then(Path::parent)
        .is_some_and(|parent| parent == root)
        && path
            .extension()
            .is_some_and(|extension| extension == "jsonl")
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

    let real_value = fs::canonicalize(&expanded).ok()?;
    for store in [
        roots.claude_config_dir.clone(),
        roots.codex_sessions(),
        roots.codex_index(),
        roots.omp_sessions(),
        roots.droid_sessions(),
        roots.droid_projects(),
        roots.opencode_legacy_sessions(),
        roots.opencode_databases_dir(),
        roots.copilot_sessions(),
        roots.pi_sessions(),
        roots.cursor_projects(),
        roots.antigravity_brain(),
    ] {
        let Ok(real_store) = fs::canonicalize(&store) else {
            continue;
        };
        if real_value == real_store || real_value.starts_with(&real_store) {
            return None;
        }
    }
    Some(real_value.to_string_lossy().into_owned())
}
