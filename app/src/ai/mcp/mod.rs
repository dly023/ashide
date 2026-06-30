use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use warp_core::ui::Icon;

pub mod templatable_manager;
#[cfg(not(target_family = "wasm"))]
pub use templatable_manager::McpIntegration;
pub use templatable_manager::TemplatableMCPServerManager;

cfg_if::cfg_if! {
    if #[cfg(not(feature = "local_fs"))] {
        mod dummy_file_based_manager;
        pub use dummy_file_based_manager::FileBasedMCPManager;
        mod dummy_file_mcp_watcher;
        pub use dummy_file_mcp_watcher::FileMCPWatcher;
    }
}

pub(crate) fn home_config_file_path(provider: MCPProvider) -> Option<PathBuf> {
    match provider {
        MCPProvider::Ashide => warp_core::paths::warp_home_mcp_config_file_path(),
        _ => dirs::home_dir().map(|home_dir| home_dir.join(provider.home_config_path())),
    }
}

cfg_if::cfg_if! {
    if #[cfg(feature = "local_fs")] {
        pub mod file_based_manager;
        pub use file_based_manager::FileBasedMCPManager;
        pub mod file_mcp_watcher;
        pub use file_mcp_watcher::{FileMCPWatcher, FileMCPWatcherEvent};
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter)]
pub enum MCPProvider {
    Ashide,
    Claude,
    Codex,
    Agents,
}

impl MCPProvider {
    pub fn display_name(&self) -> &str {
        match self {
            MCPProvider::Ashide => "Ashide",
            MCPProvider::Claude => "Claude",
            MCPProvider::Codex => "Codex",
            MCPProvider::Agents => "Other Agents",
        }
    }

    pub fn icon(&self) -> Icon {
        match self {
            MCPProvider::Ashide => Icon::WarpLogoLight,
            MCPProvider::Claude => Icon::ClaudeLogo,
            MCPProvider::Codex => Icon::OpenAILogo,
            MCPProvider::Agents => Icon::WarpLogoLight,
        }
    }

    /// Returns the path of the provider's config file relative to the home directory.
    pub fn home_config_path(&self) -> &'static Path {
        match self {
            MCPProvider::Ashide => Path::new(".ashide/.mcp.json"),
            MCPProvider::Claude => Path::new(".claude.json"),
            MCPProvider::Codex => Path::new(".codex/config.toml"),
            MCPProvider::Agents => Path::new(".agents/.mcp.json"),
        }
    }

    /// Returns the path of the provider's config file relative to a project root.
    pub fn project_config_path(&self) -> &'static Path {
        match self {
            MCPProvider::Ashide => Path::new(".ashide/.mcp.json"),
            MCPProvider::Claude => Path::new(".mcp.json"),
            MCPProvider::Codex => Path::new(".codex/config.toml"),
            MCPProvider::Agents => Path::new(".agents/.mcp.json"),
        }
    }
}

/// Returns the [`MCPProvider`] that owns `file_path` as a config file, if any.
///
/// Matches against both home-level configs (e.g. `~/.claude.json`) and
/// project-level configs (e.g. `.mcp.json` anywhere in the path).
pub fn mcp_provider_from_file_path(file_path: &Path) -> Option<MCPProvider> {
    // Try exact home-config match first (unambiguous).
    for provider in MCPProvider::iter() {
        if home_config_file_path(provider)
            .as_ref()
            .is_some_and(|home_config_path| file_path == home_config_path)
        {
            return Some(provider);
        }
    }
    // Fall back to project-config suffix match, preferring the longest
    // (most-specific) suffix.
    // This avoids `.mcp.json` shadowing `.ashide/.mcp.json`, for example.
    let mut best: Option<(MCPProvider, usize)> = None;
    for provider in MCPProvider::iter() {
        let cfg = provider.project_config_path();
        if file_path.ends_with(cfg) {
            let len = cfg.as_os_str().len();
            if best.is_none_or(|(_, best_len)| len > best_len) {
                best = Some((provider, len));
            }
        }
    }
    best.map(|(p, _)| p)
}

#[cfg(test)]
mod tests {
    use super::{mcp_provider_from_file_path, MCPProvider};

    #[test]
    fn mcp_provider_from_file_path_recognizes_ashide_home_path() {
        if let Some(ashide_home_mcp_config_file_path) =
            warp_core::paths::warp_home_mcp_config_file_path()
        {
            assert_eq!(
                mcp_provider_from_file_path(&ashide_home_mcp_config_file_path),
                Some(MCPProvider::Ashide)
            );
        }
    }
}

pub mod gallery;
pub use gallery::MCPGalleryManager;
pub mod templatable;
pub use templatable::JsonTemplate;
pub use templatable::{TemplatableMCPServer, TemplateVariable};
pub mod logs;
pub mod templatable_installation;
pub use templatable_installation::TemplatableMCPServerInstallation;
pub mod parsing;
pub use parsing::ParsedTemplatableMCPServerResult;
#[cfg(not(target_family = "wasm"))]
pub mod http_client;
#[cfg(not(target_family = "wasm"))]
pub mod reconnecting_peer;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(target_family = "wasm", expect(dead_code))]
pub struct JSONMCPServer {
    #[serde(flatten)]
    pub transport_type: JSONTransportType,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JSONTransportType {
    CLIServer {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: HashMap<String, String>,
        #[serde(default)]
        working_directory: Option<String>,
    },
    SSEServer {
        #[serde(alias = "serverUrl")]
        url: String,
        #[serde(default)]
        headers: HashMap<String, String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MCPServer {
    pub transport_type: TransportType,
    pub name: String,
    #[serde(default)]
    pub uuid: uuid::Uuid,
}

#[derive(Debug, Clone, Copy)]
#[cfg_attr(target_family = "wasm", allow(dead_code))]
pub enum MCPServerState {
    NotRunning,
    Starting,
    Authenticating,
    Running,
    ShuttingDown,
    FailedToStart,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransportType {
    CLIServer(CLIServer),
    ServerSentEvents(ServerSentEvents),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CLIServer {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd_parameter: Option<String>,
    /// Static env vars added via editor inputs.
    pub static_env_vars: Vec<StaticEnvVar>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaticEnvVar {
    pub name: String,
    /// To avoid leaking environment variables, we ensure that values are not
    /// serialized before being sent to our servers
    #[serde(skip_serializing, default)]
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaticHeader {
    pub name: String,
    /// To avoid leaking header values (which may contain secrets), we ensure that values are not
    /// serialized before being sent to our servers
    #[serde(skip_serializing, default)]
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerSentEvents {
    pub url: String,
    /// Static headers added via editor inputs.
    #[serde(default)]
    pub headers: Vec<StaticHeader>,
}

/// Trait for types that have a name and value field.
/// Used for shared operations on `StaticEnvVar` and `StaticHeader`.
#[cfg(not(target_family = "wasm"))]
trait NameValuePair {
    fn name(&self) -> &str;
    fn value(&self) -> &str;
    fn new(name: String, value: String) -> Self;
}

#[cfg(not(target_family = "wasm"))]
impl NameValuePair for StaticEnvVar {
    fn name(&self) -> &str {
        &self.name
    }
    fn value(&self) -> &str {
        &self.value
    }
    fn new(name: String, value: String) -> Self {
        Self { name, value }
    }
}
#[cfg(not(target_family = "wasm"))]
impl NameValuePair for StaticHeader {
    fn name(&self) -> &str {
        &self.name
    }
    fn value(&self) -> &str {
        &self.value
    }
    fn new(name: String, value: String) -> Self {
        Self { name, value }
    }
}

/// Converts a HashMap to a Vec of name/value pair items.
#[cfg(not(target_family = "wasm"))]
fn items_from_hashmap<T: NameValuePair>(map: &HashMap<String, String>) -> Vec<T> {
    map.iter()
        .map(|(name, value)| T::new(name.to_owned(), value.to_owned()))
        .collect()
}

/// Converts a slice of name/value pair items to a HashMap.
#[cfg(not(target_family = "wasm"))]
fn items_to_hashmap<T: NameValuePair>(items: &[T]) -> HashMap<String, String> {
    items
        .iter()
        .map(|item| (item.name().to_owned(), item.value().to_owned()))
        .collect()
}

#[cfg(not(target_family = "wasm"))]
impl MCPServer {
    fn find_server_map(
        config: serde_json::Value,
    ) -> serde_json::Result<HashMap<String, JSONMCPServer>> {
        // We want to be quite permissive in parsing user input. They may specify more than one
        // server. They might paste things in Claude Desktop style or VSCode style. All are
        // accepted here.
        //
        // VSCode:
        // {
        //   "mcp": {
        //     "servers": {
        //          [map of mcp servers]
        //     }
        //   }
        // }
        //   ---  OR  ---
        // {
        //   "servers": {
        //     [map of mcp servers]
        //   }
        // }
        //
        // Claude Desktop:
        // {
        //   "mcpServers": {
        //     [map of mcp servers]
        //   }
        // }
        // Also allowed:
        // {
        //   [map of mcp servers]
        // }

        let pointers = ["/mcp/servers", "/servers", "/mcpServers"];
        for pointer in pointers.into_iter() {
            if let Some(value) = config.pointer(pointer) {
                if let Ok(servers) =
                    serde_json::from_value::<HashMap<String, JSONMCPServer>>(value.clone())
                {
                    return Ok(servers);
                }
            }
        }
        serde_json::from_value::<HashMap<String, JSONMCPServer>>(config)
    }
    pub fn from_user_json(json: &str) -> serde_json::Result<Vec<MCPServer>> {
        // Some docs don't show curly braces around the json object, so add them if necessary.
        let json = json.trim();
        let json = if json.starts_with("{") {
            json.to_owned()
        } else {
            format!("{{{json}}}")
        };

        let config: serde_json::Value = serde_json::from_str(&json)?;

        let servers = Self::find_server_map(config)?;
        Ok(servers
            .iter()
            .map(|(name, server)| {
                let transport_type = match &server.transport_type {
                    JSONTransportType::CLIServer {
                        command,
                        args,
                        env,
                        working_directory,
                    } => TransportType::CLIServer(CLIServer {
                        command: command.clone(),
                        args: args.clone(),
                        cwd_parameter: working_directory.to_owned(),
                        static_env_vars: items_from_hashmap(env),
                    }),
                    JSONTransportType::SSEServer { url, headers } => {
                        TransportType::ServerSentEvents(ServerSentEvents {
                            url: url.to_owned(),
                            headers: items_from_hashmap(headers),
                        })
                    }
                };
                MCPServer {
                    name: name.to_owned(),
                    transport_type,
                    uuid: uuid::Uuid::new_v4(),
                }
            })
            .collect())
    }

    /// Includes the environment variable values, should only be shown to users,
    /// not sent to our servers.
    pub fn to_user_json(&self) -> String {
        let transport_type = match &self.transport_type {
            TransportType::CLIServer(cli_server) => JSONTransportType::CLIServer {
                command: cli_server.command.clone(),
                args: cli_server.args.clone(),
                env: items_to_hashmap(&cli_server.static_env_vars),
                working_directory: cli_server.cwd_parameter.to_owned(),
            },
            TransportType::ServerSentEvents(sse_server) => JSONTransportType::SSEServer {
                url: sse_server.url.to_owned(),
                headers: items_to_hashmap(&sse_server.headers),
            },
        };
        serde_json::to_string_pretty(
            &std::iter::once((self.name.to_owned(), JSONMCPServer { transport_type }))
                .collect::<HashMap<_, _>>(),
        )
        // serde_json::to_string_pretty should never fail on our JSONMCPServer type, but better to
        // not crash the app if it does.
        .unwrap_or_else(|err| {
            log::error!("Could not serialize MCP server to user json: {err:?}");
            Default::default()
        })
    }
}

#[cfg(target_family = "wasm")]
impl MCPServer {
    pub fn from_user_json(_json: &str) -> serde_json::Result<Vec<MCPServer>> {
        Ok(Vec::new())
    }

    pub fn to_user_json(&self) -> String {
        Default::default()
    }
}

#[derive(Debug, Clone)]
pub enum Author {
    CurrentUser,
    OtherUser { name: String },
    Unknown,
}

#[derive(Debug, Clone)]
pub enum MCPServerUpdate {
    TemplateObject {
        publisher: Author,
        new_version_ts: i64,
        json_template: JsonTemplate,
    },
    Gallery {
        name: String,
        new_version: i32,
        json_template: JsonTemplate,
    },
}

#[cfg(test)]
mod mod_test;
