//! CLI-agent store root resolution and path helpers.

#[cfg(feature = "local_fs")]
use std::fs;
use std::path::Path;
use std::path::PathBuf;

#[cfg(feature = "local_fs")]
use super::error::CliAgentSessionScanError;
use crate::terminal::CLIAgent;

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

    pub(crate) fn cursor_chats(&self) -> PathBuf {
        self.home_dir.join(".cursor/chats")
    }

    pub(crate) fn antigravity_brain(&self) -> PathBuf {
        self.home_dir.join(".gemini/antigravity-cli/brain")
    }

    /// 单个 provider 在目标 installation 下可参与会话发现的物理根。
    /// discovery、read 与 mutation 必须从这里派生，禁止各自维护 provider 子集。
    pub(crate) fn provider_session_discovery_roots(&self, agent: CLIAgent) -> Vec<PathBuf> {
        match agent {
            CLIAgent::Claude => vec![self.claude_projects()],
            CLIAgent::Codex => vec![self.codex_sessions(), self.codex_index()],
            CLIAgent::Droid => vec![self.droid_sessions(), self.droid_projects()],
            CLIAgent::OpenCode => vec![
                self.opencode_legacy_sessions(),
                self.opencode_databases_dir(),
            ],
            CLIAgent::Copilot => vec![self.copilot_sessions()],
            CLIAgent::Pi => vec![self.pi_sessions()],
            CLIAgent::CursorCli => vec![self.cursor_chats(), self.cursor_projects()],
            CLIAgent::Antigravity => vec![self.antigravity_brain()],
            CLIAgent::Omp => vec![self.omp_sessions()],
            CLIAgent::Amp
            | CLIAgent::Auggie
            | CLIAgent::Goose
            | CLIAgent::DeepSeek
            | CLIAgent::Unknown => Vec::new(),
        }
    }

    /// 普通文件 mutation 可触达的 transcript 根。数据库/index entry 不在此列，
    /// 它们必须走 provider-native mutation，不能被当成整个文件删除。
    pub(crate) fn provider_session_transcript_roots(&self, agent: CLIAgent) -> Vec<PathBuf> {
        match agent {
            CLIAgent::Codex => vec![self.codex_sessions()],
            CLIAgent::OpenCode => vec![self.opencode_legacy_sessions()],
            CLIAgent::CursorCli => vec![self.cursor_projects()],
            CLIAgent::Claude
            | CLIAgent::Droid
            | CLIAgent::Copilot
            | CLIAgent::Pi
            | CLIAgent::Antigravity
            | CLIAgent::Omp => self.provider_session_discovery_roots(agent),
            CLIAgent::Amp
            | CLIAgent::Auggie
            | CLIAgent::Goose
            | CLIAgent::DeepSeek
            | CLIAgent::Unknown => Vec::new(),
        }
    }

    #[cfg(feature = "local_fs")]
    pub(crate) fn is_authoritative_session_transcript(
        &self,
        agent: CLIAgent,
        resolved_path: &Path,
    ) -> bool {
        let expected_extension = match agent {
            CLIAgent::OpenCode => "json",
            CLIAgent::Claude
            | CLIAgent::Codex
            | CLIAgent::Droid
            | CLIAgent::Copilot
            | CLIAgent::Pi
            | CLIAgent::CursorCli
            | CLIAgent::Antigravity
            | CLIAgent::Omp => "jsonl",
            CLIAgent::Amp
            | CLIAgent::Auggie
            | CLIAgent::Goose
            | CLIAgent::DeepSeek
            | CLIAgent::Unknown => return false,
        };
        if resolved_path.extension().and_then(|value| value.to_str()) != Some(expected_extension) {
            return false;
        }

        self.provider_session_transcript_roots(agent)
            .into_iter()
            .map(|root| fs::canonicalize(&root).unwrap_or(root))
            .any(|root| {
                if matches!(agent, CLIAgent::Omp) {
                    is_omp_session_source(&root, resolved_path)
                } else if matches!(agent, CLIAgent::Antigravity) {
                    resolved_path.starts_with(root)
                        && resolved_path.file_name().and_then(|name| name.to_str())
                            == Some("transcript.jsonl")
                } else if matches!(agent, CLIAgent::CursorCli) {
                    resolved_path.starts_with(root)
                        && resolved_path
                            .components()
                            .any(|component| component.as_os_str() == "agent-transcripts")
                        && !resolved_path
                            .components()
                            .any(|component| component.as_os_str() == "subagents")
                } else {
                    resolved_path.starts_with(root)
                }
            })
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
    let mut stores = enum_iterator::all::<CLIAgent>()
        .flat_map(|agent| roots.provider_session_discovery_roots(agent))
        .collect::<Vec<_>>();
    stores.push(roots.claude_config_dir.clone());
    for store in stores {
        let Ok(real_store) = fs::canonicalize(&store) else {
            continue;
        };
        if real_value == real_store || real_value.starts_with(&real_store) {
            return None;
        }
    }
    Some(real_value.to_string_lossy().into_owned())
}
