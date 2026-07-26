use warpui::{ViewContext, ViewHandle};

use crate::app_state::{EnvironmentKind, EnvironmentLifecycleState, EnvironmentSnapshot};
use crate::environment_authority::saved_ssh_authority;
use crate::ssh_manager::{SshTargetCatalog, SshTargetCatalogEntry};
use crate::workspace::environment_provider::{
    EnvironmentProviderCandidate, EnvironmentProviderCandidateLoadOutcome,
    EnvironmentProviderCandidateLoadResult, EnvironmentProviderManagerEvent,
    EnvironmentProviderSearchTarget, EnvironmentProviderTarget, EnvironmentTransportDescriptor,
};

const CONFIG_ENVIRONMENT_NODE_PREFIX: &str = "ssh-config:";

pub(crate) type ProviderManagerView = crate::ssh_manager::SshManagerPanel;
pub(crate) type ProviderManagerEvent = crate::ssh_manager::SshManagerPanelEvent;

pub(crate) fn new_provider_manager_view<V: warpui::View>(
    ctx: &mut ViewContext<V>,
) -> ViewHandle<ProviderManagerView> {
    ctx.add_typed_action_view(crate::ssh_manager::SshManagerPanel::new)
}

pub(crate) fn provider_manager_event(
    event: &ProviderManagerEvent,
) -> EnvironmentProviderManagerEvent {
    match event {
        crate::ssh_manager::SshManagerPanelEvent::OpenServerEditor { node_id } => {
            EnvironmentProviderManagerEvent::OpenEditor {
                connection_ref: node_id.clone(),
            }
        }
        crate::ssh_manager::SshManagerPanelEvent::OpenEnvironmentTerminal { node_id, server } => {
            EnvironmentProviderManagerEvent::OpenRuntimeTerminal {
                target: target_from_server(node_id.clone(), server.clone()),
            }
        }
        crate::ssh_manager::SshManagerPanelEvent::OpenEnvironment { node_id, server } => {
            EnvironmentProviderManagerEvent::OpenRuntime {
                target: target_from_server(node_id.clone(), server.clone()),
            }
        }
        crate::ssh_manager::SshManagerPanelEvent::OpenProviderFileBrowserPane { node_id } => {
            EnvironmentProviderManagerEvent::OpenFileBrowser {
                connection_ref: node_id.clone(),
            }
        }
        crate::ssh_manager::SshManagerPanelEvent::PersistenceError(message) => {
            EnvironmentProviderManagerEvent::PersistenceError(message.clone())
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct SavedConnectionTarget {
    connection_ref: String,
    server: warp_ssh_manager::SshServerInfo,
}

impl SavedConnectionTarget {
    pub(super) fn new(connection_ref: String, server: warp_ssh_manager::SshServerInfo) -> Self {
        Self {
            connection_ref,
            server,
        }
    }

    pub(super) fn dormant_environment(
        &self,
        active_workspace_root: Option<String>,
    ) -> EnvironmentSnapshot {
        runtime_transport_snapshot(
            self.connection_ref.clone(),
            &self.server,
            active_workspace_root,
            EnvironmentLifecycleState::Dormant,
        )
    }

    pub(super) fn startup_command(&self) -> Option<String> {
        self.server
            .startup_command
            .as_ref()
            .filter(|command| !command.trim().is_empty())
            .cloned()
    }

    pub(super) fn connection_ref(&self) -> &str {
        &self.connection_ref
    }

    pub(super) fn transport_descriptor(&self) -> EnvironmentTransportDescriptor {
        EnvironmentTransportDescriptor::from_saved_connection_transport(
            SavedConnectionTransport::new(self.server.clone()),
        )
    }
}

#[derive(Debug, Clone)]
pub(super) struct SavedConnectionTransport {
    server: warp_ssh_manager::SshServerInfo,
}

impl SavedConnectionTransport {
    pub(super) fn new(server: warp_ssh_manager::SshServerInfo) -> Self {
        Self { server }
    }

    pub(super) fn connection_ref(&self) -> &str {
        &self.server.node_id
    }

    pub(super) fn host_label(&self) -> &str {
        &self.server.host
    }

    pub(super) fn target(&self) -> String {
        target_for_server(&self.server)
    }

    pub(super) fn args(&self) -> Vec<String> {
        warp_ssh_manager::build_ssh_args(&self.server)
    }

    pub(super) fn runtime_snapshot(
        &self,
        connection_ref: String,
        active_workspace_root: Option<String>,
        lifecycle_state: EnvironmentLifecycleState,
    ) -> EnvironmentSnapshot {
        runtime_transport_snapshot(
            connection_ref,
            &self.server,
            active_workspace_root,
            lifecycle_state,
        )
    }
}

#[cfg(test)]
pub(super) fn test_transport_descriptor() -> EnvironmentTransportDescriptor {
    let mut server = warp_ssh_manager::SshServerInfo::new_default("test-node".to_owned());
    server.host = "example.internal".to_owned();
    server.username = "root".to_owned();
    EnvironmentTransportDescriptor::from_saved_connection_transport(SavedConnectionTransport::new(
        server,
    ))
}

pub(crate) fn target_from_server(
    connection_ref: String,
    server: warp_ssh_manager::SshServerInfo,
) -> EnvironmentProviderTarget {
    EnvironmentProviderTarget::from_saved_connection_target(SavedConnectionTarget::new(
        connection_ref,
        server,
    ))
}

pub(super) fn load_saved_provider_search_targets(
    max_targets: usize,
) -> Result<Vec<EnvironmentProviderSearchTarget>, String> {
    let nodes =
        warp_ssh_manager::with_conn(|conn| Ok(warp_ssh_manager::SshRepository::list_nodes(conn)?))
            .map_err(|error| error.to_string())?;

    let mut targets = Vec::new();
    for node in nodes
        .into_iter()
        .filter(|node| matches!(node.kind, warp_ssh_manager::NodeKind::Server))
        .take(max_targets)
    {
        let Some(server) = warp_ssh_manager::with_conn(|conn| {
            Ok(warp_ssh_manager::SshRepository::get_server(conn, &node.id)?)
        })
        .map_err(|error| error.to_string())?
        else {
            continue;
        };
        let title = node.name.clone();
        let detail = if server.username.is_empty() {
            server.host.clone()
        } else {
            format!("{}@{}", server.username, server.host)
        };
        let search_text = format!("{title} {detail}");
        targets.push(EnvironmentProviderSearchTarget {
            target: target_from_server(node.id, server),
            title,
            detail,
            search_text,
        });
    }

    Ok(targets)
}

pub(super) fn runtime_transport_descriptor_for_connection_ref(
    connection_ref: &str,
    catalog: &SshTargetCatalog,
) -> Option<EnvironmentTransportDescriptor> {
    if let Some(alias) = config_alias_from_node_id(connection_ref) {
        return config_candidate_by_alias(alias, catalog)
            .as_ref()
            .map(|candidate| config_candidate_to_server(candidate, catalog))
            .map(SavedConnectionTransport::new)
            .map(EnvironmentTransportDescriptor::from_saved_connection_transport);
    }

    catalog
        .find_entry(&format!("saved:{connection_ref}"))
        .and_then(SshTargetCatalogEntry::saved)
        .map(|(_, server)| server.clone())
        .map(SavedConnectionTransport::new)
        .map(EnvironmentTransportDescriptor::from_saved_connection_transport)
}

pub(super) fn describe_runtime_transport_descriptor_lookup_failure(
    connection_ref: &str,
    catalog: &SshTargetCatalog,
) -> String {
    if let Some(alias) = config_alias_from_node_id(connection_ref) {
        return match catalog.outcome() {
            warp_ssh_manager::LoadOutcome::Loaded(_) => {
                if catalog.find_candidate(alias).is_some() {
                    format!(
                        "SSH config host {alias} was found but could not be converted into an environment transport"
                    )
                } else {
                    format!(
                        "SSH config host {alias} was not found. Open SSH config and reconnect this environment."
                    )
                }
            }
            warp_ssh_manager::LoadOutcome::NotFound => {
                format!(
                    "SSH config was not found while resolving host {alias}. Open SSH config and reconnect this environment."
                )
            }
            warp_ssh_manager::LoadOutcome::Error(message) => {
                format!("SSH config could not be read while resolving host {alias}: {message}")
            }
        };
    }

    match catalog
        .find_entry(&format!("saved:{connection_ref}"))
        .and_then(SshTargetCatalogEntry::saved)
    {
        Some(_) => format!(
            "Saved environment provider {connection_ref} was found but could not be converted into an environment transport"
        ),
        None => format!(
            "Saved environment provider {connection_ref} was not found. Re-add it from SSH config or Environment Providers."
        ),
    }
}

pub(super) fn target_for_provider_candidate(
    identity: &str,
    catalog: &SshTargetCatalog,
) -> Option<EnvironmentProviderTarget> {
    if let Some(entry) = catalog.find_entry(identity) {
        return match entry {
            SshTargetCatalogEntry::Saved { server, .. } => {
                Some(target_from_server(server.node_id.clone(), server.clone()))
            }
            SshTargetCatalogEntry::Config(candidate) => {
                let server = config_candidate_to_server(candidate, catalog);
                let connection_ref = server.node_id.clone();
                Some(target_from_server(connection_ref, server))
            }
        };
    }

    let candidate = config_candidate_by_alias(identity, catalog)?;
    let server = config_candidate_to_server(&candidate, catalog);
    Some(target_from_server(server.node_id.clone(), server))
}

pub(super) fn provider_candidates(
    catalog: &SshTargetCatalog,
) -> EnvironmentProviderCandidateLoadResult {
    let config_open_target = catalog.config_open_target().map(ToOwned::to_owned);
    let candidates = catalog
        .entries()
        .iter()
        .map(|entry| match entry {
            SshTargetCatalogEntry::Saved { name, server } => {
                provider_candidate_from_saved(name, server)
            }
            SshTargetCatalogEntry::Config(candidate) => {
                provider_candidate_from_config(candidate.clone())
            }
        })
        .collect::<Vec<_>>();
    let outcome = match (candidates.is_empty(), catalog.outcome()) {
        (true, warp_ssh_manager::LoadOutcome::NotFound) => {
            EnvironmentProviderCandidateLoadOutcome::NotFound
        }
        (true, warp_ssh_manager::LoadOutcome::Error(message)) => {
            EnvironmentProviderCandidateLoadOutcome::Error(message.clone())
        }
        (false, warp_ssh_manager::LoadOutcome::Loaded(_))
        | (false, warp_ssh_manager::LoadOutcome::NotFound)
        | (false, warp_ssh_manager::LoadOutcome::Error(_))
        | (true, warp_ssh_manager::LoadOutcome::Loaded(_)) => {
            EnvironmentProviderCandidateLoadOutcome::Loaded(candidates)
        }
    };
    EnvironmentProviderCandidateLoadResult {
        config_open_target,
        outcome,
        is_loading: catalog.is_loading(),
        refresh_error: catalog.error().map(ToOwned::to_owned),
    }
}

pub(crate) fn runtime_transport_snapshot(
    connection_ref: String,
    server: &warp_ssh_manager::SshServerInfo,
    active_workspace_root: Option<String>,
    lifecycle_state: EnvironmentLifecycleState,
) -> EnvironmentSnapshot {
    let label = environment_label_for_saved_connection(server);
    EnvironmentSnapshot::runtime_transport(
        EnvironmentKind::Ssh,
        label,
        saved_ssh_authority(&connection_ref),
        Some(connection_ref),
        active_workspace_root,
        lifecycle_state,
    )
}

fn environment_label_for_saved_connection(server: &warp_ssh_manager::SshServerInfo) -> String {
    let user_host = if server.username.is_empty() {
        server.host.clone()
    } else {
        format!("{}@{}", server.username, server.host)
    };

    if server.port == 22 {
        user_host
    } else {
        format!("{user_host}:{}", server.port)
    }
}

fn config_environment_node_id(alias: &str) -> String {
    format!("{CONFIG_ENVIRONMENT_NODE_PREFIX}{alias}")
}

fn config_alias_from_node_id(node_id: &str) -> Option<&str> {
    node_id.strip_prefix(CONFIG_ENVIRONMENT_NODE_PREFIX)
}

fn config_candidate_to_server(
    candidate: &warp_ssh_manager::SshConfigCandidate,
    catalog: &SshTargetCatalog,
) -> warp_ssh_manager::SshServerInfo {
    let node_id = config_environment_node_id(&candidate.alias);
    warp_ssh_manager::SshServerInfo {
        node_id,
        // Keep the OpenSSH Host alias as the executable target so ProxyJump,
        // Include, HostName and other directives continue to be resolved by ssh.
        host: candidate.alias.clone(),
        port: candidate.port.unwrap_or(22),
        username: candidate.user.clone().unwrap_or_default(),
        auth_type: if candidate.identity_file.is_some() {
            warp_ssh_manager::AuthType::Key
        } else {
            warp_ssh_manager::AuthType::Password
        },
        key_path: candidate
            .identity_file
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
        startup_command: None,
        notes: catalog
            .config_path_display()
            .map(|path| format!("Loaded from {path}")),
        last_connected_at: None,
    }
}

fn provider_candidate_from_config(
    candidate: warp_ssh_manager::SshConfigCandidate,
) -> EnvironmentProviderCandidate {
    let alias = candidate.alias.clone();
    let node_id = config_environment_node_id(&alias);
    let authority_key = saved_ssh_authority(&node_id);
    let mut details = Vec::new();
    if let Some(user) = candidate.user.as_deref() {
        details.push(user.to_string());
    }
    if let Some(hostname) = candidate.hostname.as_deref() {
        if let Some(last) = details.last_mut() {
            *last = format!("{last}@{hostname}");
        } else {
            details.push(hostname.to_string());
        }
    }
    if let Some(port) = candidate.port {
        if let Some(last) = details.last_mut() {
            *last = format!("{last}:{port}");
        } else {
            details.push(format!(":{port}"));
        }
    }
    let uses_key_auth = candidate.identity_file.is_some();
    if uses_key_auth {
        details.push(crate::t!("workspace-environment-provider-picker-key-auth"));
    }
    let detail = if details.is_empty() {
        crate::t!("workspace-environment-provider-picker-alias-only")
    } else {
        details.join(" · ")
    };
    EnvironmentProviderCandidate {
        alias: format!("config:{alias}"),
        authority_key,
        title: alias,
        detail,
    }
}

fn provider_candidate_from_saved(
    name: &str,
    server: &warp_ssh_manager::SshServerInfo,
) -> EnvironmentProviderCandidate {
    let detail = environment_label_for_saved_connection(server);
    EnvironmentProviderCandidate {
        alias: format!("saved:{}", server.node_id),
        authority_key: saved_ssh_authority(&server.node_id),
        title: name.to_owned(),
        detail,
    }
}

fn target_for_server(server: &warp_ssh_manager::SshServerInfo) -> String {
    if server.username.is_empty() {
        server.host.clone()
    } else {
        format!("{}@{}", server.username, server.host)
    }
}

fn config_candidate_by_alias(
    alias: &str,
    catalog: &SshTargetCatalog,
) -> Option<warp_ssh_manager::SshConfigCandidate> {
    catalog.find_candidate(alias).cloned()
}

#[cfg(test)]
#[path = "source_saved_ssh_tests.rs"]
mod tests;
