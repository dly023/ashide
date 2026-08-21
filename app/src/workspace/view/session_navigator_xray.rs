//! Session Navigator 关系 X-Ray。
//!
//! 这里不拥有任何状态，只把 Environment、container、durable session、binding、
//! runtime 与 detached lease 的 canonical owner 做一次瞬态只读 join。

use serde::Serialize;
use warpui::{AppContext, SingletonEntity, ViewContext};

use crate::app_state::{
    PaneSessionBinding, WorkspaceSessionAliasSubject, WorkspaceSessionSnapshot,
};
use crate::environment_authority::{
    session_authority_or_terminal_bootstrap, ParsedEnvironmentAuthority,
};
use crate::terminal::TerminalRuntimeDiagnostics;
use crate::workspace::action::WorkspaceSessionActionTarget;
use crate::workspace::registry::RetiringWorkspaceSessionOwnerDiagnostics;
use crate::workspace::WorkspaceRegistry;

use super::Workspace;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum SessionPresence {
    Cold,
    StartingHere,
    LiveHere,
    LiveElsewhere,
    UndoRetained,
    Stopping,
    Unknown,
}

impl SessionPresence {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Cold => "Cold",
            Self::StartingHere => "Starting here",
            Self::LiveHere => "Live here",
            Self::LiveElsewhere => "Live elsewhere",
            Self::UndoRetained => "Undo retained",
            Self::Stopping => "Stopping",
            Self::Unknown => "Unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct SessionPresenceEvidence {
    snapshot_expects_live_carrier: bool,
    current_carrier: bool,
    current_runtime: bool,
    pending_materialization: bool,
    other_owner_count: usize,
    retiring_owner_count: usize,
    retiring_shutdown_requested: bool,
    retiring_model_has_exited: bool,
}

fn classify_session_presence(evidence: SessionPresenceEvidence) -> SessionPresence {
    let has_current_evidence =
        evidence.current_carrier || evidence.current_runtime || evidence.pending_materialization;
    let has_other_evidence = evidence.other_owner_count != 0;
    let has_retiring_evidence = evidence.retiring_owner_count != 0;

    if evidence.other_owner_count > 1
        || evidence.retiring_owner_count > 1
        || (has_current_evidence && (has_other_evidence || has_retiring_evidence))
        || (has_other_evidence && has_retiring_evidence)
        || evidence.retiring_model_has_exited
        || (evidence.current_runtime && !evidence.current_carrier)
        || (evidence.pending_materialization && !evidence.current_carrier)
    {
        return SessionPresence::Unknown;
    }

    if evidence.pending_materialization {
        return SessionPresence::StartingHere;
    }
    if evidence.current_carrier {
        return SessionPresence::LiveHere;
    }
    if evidence.other_owner_count == 1 {
        return SessionPresence::LiveElsewhere;
    }
    if evidence.retiring_owner_count == 1 {
        return if evidence.retiring_shutdown_requested {
            SessionPresence::Stopping
        } else {
            SessionPresence::UndoRetained
        };
    }
    if evidence.snapshot_expects_live_carrier {
        SessionPresence::Unknown
    } else {
        SessionPresence::Cold
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct SessionIdentityXRay {
    logical_key: String,
    durable_identity_key: Option<String>,
    observed_identity_keys: Vec<String>,
    alias_subject: WorkspaceSessionAliasSubject,
    container_uuid_hex: Option<String>,
    volatile_layout_locator: String,
}

fn session_identity_projection(session: &WorkspaceSessionSnapshot) -> SessionIdentityXRay {
    SessionIdentityXRay {
        logical_key: session.logical_key(),
        durable_identity_key: session.durable_identity_key(),
        observed_identity_keys: session.observed_identity_keys(),
        alias_subject: session.alias_subject(),
        container_uuid_hex: session.container_uuid.as_deref().map(hex::encode),
        volatile_layout_locator: session.id.clone(),
    }
}

#[derive(Clone, Debug, Serialize)]
struct EnvironmentXRay {
    authority: String,
    navigation_key: String,
    backend: &'static str,
    connection_ref: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct CurrentCarrierXRay {
    pane_group_id: String,
    pane_id: String,
    binding: Option<PaneSessionBinding>,
    container_override: Option<String>,
    pending_materialization_stage: Option<String>,
    runtime: Option<TerminalRuntimeDiagnostics>,
}

#[derive(Clone, Debug, Serialize)]
struct OtherOwnerXRay {
    window_id: String,
    pane_group_id: String,
    pane_id: String,
}

#[derive(Clone, Debug, Serialize)]
struct SessionTitleXRay {
    displayed_label: String,
    user_alias: Option<String>,
    resolution: &'static str,
    merged_automatic_candidates: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct SessionNavigatorXRay {
    presence: SessionPresence,
    environment: EnvironmentXRay,
    row_id: String,
    identity: SessionIdentityXRay,
    title: SessionTitleXRay,
    current_carrier: Option<CurrentCarrierXRay>,
    other_owners: Vec<OtherOwnerXRay>,
    retiring_owners: Vec<RetiringWorkspaceSessionOwnerDiagnostics>,
    legal_actions: Vec<&'static str>,
    missing_actions: Vec<&'static str>,
    invariant_violations: Vec<String>,
}

impl SessionNavigatorXRay {
    pub(super) fn presence(&self) -> SessionPresence {
        self.presence
    }

    pub(super) fn compact_facts(&self) -> Vec<String> {
        let alias_subject = match &self.identity.alias_subject {
            WorkspaceSessionAliasSubject::DurableSession(key) => format!("session · {key}"),
            WorkspaceSessionAliasSubject::Container(key) => format!("container · {key}"),
            WorkspaceSessionAliasSubject::VirtualSource(key) => format!("source · {key}"),
        };
        let runtime = self
            .current_carrier
            .as_ref()
            .and_then(|carrier| carrier.runtime.as_ref())
            .map(|runtime| match runtime.kind {
                crate::terminal::TerminalRuntimeKind::LocalPty => runtime
                    .process_id
                    .map(|pid| format!("local PTY · pid {pid}"))
                    .unwrap_or_else(|| "local PTY · pid pending".to_owned()),
                crate::terminal::TerminalRuntimeKind::RemoteEnvironmentPty => format!(
                    "remote PTY · session {}",
                    runtime.runtime_ref.as_deref().unwrap_or("unknown")
                ),
                crate::terminal::TerminalRuntimeKind::RemoteProxy => {
                    "remote proxy · no Environment session ref".to_owned()
                }
                crate::terminal::TerminalRuntimeKind::Mock => "mock runtime".to_owned(),
            })
            .unwrap_or_else(|| "runtime · none".to_owned());
        vec![
            format!("Environment · {}", self.environment.navigation_key),
            alias_subject,
            runtime,
        ]
    }
}

fn actions_for_presence(presence: SessionPresence) -> (Vec<&'static str>, Vec<&'static str>) {
    match presence {
        SessionPresence::Cold => (vec!["Resume"], Vec::new()),
        SessionPresence::StartingHere => (vec!["Wait"], Vec::new()),
        SessionPresence::LiveHere => (vec!["Focus", "CloseCarrier"], Vec::new()),
        SessionPresence::LiveElsewhere => (vec!["FocusOwner"], Vec::new()),
        SessionPresence::UndoRetained => (vec!["UndoCloseLifo"], vec!["TargetedRestoreMissing"]),
        SessionPresence::Stopping => (vec!["WaitForExit"], vec!["ForceStopMissing"]),
        SessionPresence::Unknown => (vec!["Diagnose"], Vec::new()),
    }
}

impl Workspace {
    pub(super) fn session_navigator_xray(
        &self,
        session: &WorkspaceSessionSnapshot,
        ctx: &AppContext,
    ) -> SessionNavigatorXRay {
        let authority =
            session_authority_or_terminal_bootstrap(session.environment_authority_key.as_deref());
        let parsed_authority = ParsedEnvironmentAuthority::parse(authority);
        let environment = EnvironmentXRay {
            authority: parsed_authority.authority().to_owned(),
            navigation_key: parsed_authority.navigation_key().to_owned(),
            backend: match parsed_authority {
                ParsedEnvironmentAuthority::TerminalBootstrap { .. } => "terminal_bootstrap",
                ParsedEnvironmentAuthority::SavedSsh { .. } => "saved_ssh_runtime",
                ParsedEnvironmentAuthority::Runtime { .. } => "runtime",
            },
            connection_ref: parsed_authority.runtime_connection_ref().map(str::to_owned),
        };
        let identity = session_identity_projection(session);
        let navigator_state = self.snapshot_session_navigator_state();
        let row_id = Self::workspace_session_row_id(session, &navigator_state);

        let locator = self.locator_for_workspace_session_snapshot(session, ctx);
        let current_carrier = locator.and_then(|locator| {
            let pane_group = self.get_pane_group_view_with_id(locator.pane_group_id)?;
            let pane_group = pane_group.as_ref(ctx);
            let pane = pane_group.pane_by_id(locator.pane_id)?;
            let configuration = pane.pane_configuration().as_ref(ctx);
            let binding = configuration.session_binding();
            let container_override = configuration
                .custom_vertical_tabs_title()
                .map(str::to_owned);
            let runtime = pane_group
                .terminal_manager_for_pane_id(locator.pane_id, ctx)
                .map(|manager| manager.as_ref(ctx).runtime_diagnostics());
            let pending_materialization_stage = self
                .environments
                .pending_materialization_for_pane(authority, locator.pane_id)
                .map(|pending| format!("{:?}", pending.stage));
            Some(CurrentCarrierXRay {
                pane_group_id: format!("{:?}", locator.pane_group_id),
                pane_id: format!("{:?}", locator.pane_id),
                binding,
                container_override,
                pending_materialization_stage,
                runtime,
            })
        });

        let registry = WorkspaceRegistry::as_ref(ctx);
        let (other_owners, retiring_owners) = identity
            .durable_identity_key
            .as_deref()
            .map(|durable_identity_key| {
                let other_owners = registry
                    .other_workspace_session_owners(self.window_id, durable_identity_key, ctx)
                    .into_iter()
                    .map(|owner| OtherOwnerXRay {
                        window_id: format!("{:?}", owner.window_id),
                        pane_group_id: format!("{:?}", owner.locator.pane_group_id),
                        pane_id: format!("{:?}", owner.locator.pane_id),
                    })
                    .collect::<Vec<_>>();
                let retiring_owners =
                    registry.retiring_session_owner_diagnostics(durable_identity_key, ctx);
                (other_owners, retiring_owners)
            })
            .unwrap_or_default();

        let evidence = SessionPresenceEvidence {
            snapshot_expects_live_carrier: session.is_live_container(),
            current_carrier: current_carrier.is_some(),
            current_runtime: current_carrier
                .as_ref()
                .is_some_and(|carrier| carrier.runtime.is_some()),
            pending_materialization: current_carrier
                .as_ref()
                .is_some_and(|carrier| carrier.pending_materialization_stage.is_some()),
            other_owner_count: other_owners.len(),
            retiring_owner_count: retiring_owners.len(),
            retiring_shutdown_requested: retiring_owners
                .first()
                .is_some_and(|owner| owner.runtime.shutdown_requested),
            retiring_model_has_exited: retiring_owners
                .iter()
                .any(|owner| owner.model_has_exited == Some(true)),
        };
        let presence = classify_session_presence(evidence);

        let user_alias = self.workspace_session_alias(session);
        let container_override = current_carrier
            .as_ref()
            .and_then(|carrier| carrier.container_override.clone());
        let resolution = if user_alias.is_some() {
            "user_session_alias"
        } else if container_override.is_some() {
            "explicit_container_override"
        } else if session.label.is_some() {
            // Provider title 与首条真实用户消息已在 discovery owner 中按固定优先级
            // 合并；这里不凭最终字符串反推 provenance。
            "merged_provider_or_first_user_title"
        } else if session.cli_agent.is_some() || session.cli_command.is_some() {
            "agent_fallback"
        } else {
            "kind_fallback"
        };
        let mut merged_automatic_candidates = self
            .backing_sessions_for_workspace_session(session)
            .into_iter()
            .filter_map(|backing| backing.label)
            .collect::<Vec<_>>();
        merged_automatic_candidates.sort();
        merged_automatic_candidates.dedup();

        let mut invariant_violations = Vec::new();
        if presence == SessionPresence::Unknown {
            invariant_violations.push(
                "canonical lifecycle evidence is missing or contradictory; no state was guessed"
                    .to_owned(),
            );
        }
        if session.is_live_container()
            && session
                .container_uuid
                .as_deref()
                .is_none_or(|id| id.is_empty())
        {
            invariant_violations.push("live carrier is missing container UUID".to_owned());
        }
        if identity.durable_identity_key.is_some()
            && matches!(
                identity.alias_subject,
                WorkspaceSessionAliasSubject::Container(_)
            )
        {
            invariant_violations.push(
                "durable session incorrectly resolved alias ownership to its carrier".to_owned(),
            );
        }
        let (legal_actions, missing_actions) = actions_for_presence(presence);

        SessionNavigatorXRay {
            presence,
            environment,
            row_id,
            identity,
            title: SessionTitleXRay {
                displayed_label: Self::workspace_session_label(session),
                user_alias,
                resolution,
                merged_automatic_candidates,
            },
            current_carrier,
            other_owners,
            retiring_owners,
            legal_actions,
            missing_actions,
            invariant_violations,
        }
    }

    pub(super) fn copy_workspace_session_xray(
        &mut self,
        target: &WorkspaceSessionActionTarget,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(session) = self.workspace_session_for_action_target(target, ctx) else {
            log::warn!(
                "copy_workspace_session_xray: missing session {} in {:?}",
                target.session_id,
                target.environment_authority_key
            );
            return;
        };
        let xray = self.session_navigator_xray(&session, ctx);
        let json = serde_json::to_string_pretty(&xray)
            .expect("SessionNavigatorXRay must remain serializable");
        ctx.clipboard()
            .write(warpui::clipboard::ClipboardContent::plain_text(json));
        self.show_workspace_session_success_toast("已复制关系 X-Ray JSON".to_owned(), ctx);
        ctx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_state::{CliAgentSessionOrigin, WorkspaceSessionKind};

    fn fixture(is_live_container: bool) -> WorkspaceSessionSnapshot {
        WorkspaceSessionSnapshot {
            id: if is_live_container {
                "tab:0:leaf:0".to_owned()
            } else {
                "provider-source".to_owned()
            },
            container_uuid: is_live_container.then(|| vec![0x01, 0x02]),
            kind: WorkspaceSessionKind::AgentTerminal,
            label: Some("Fixture".to_owned()),
            environment_authority_key: Some("local".to_owned()),
            cwd: None,
            startup_directory: None,
            cli_agent: Some("Codex".to_owned()),
            cli_command: Some("codex resume session-1".to_owned()),
            cli_agent_origin: Some(CliAgentSessionOrigin::PluginObserved),
            conversation_ids: Vec::new(),
            active_conversation_id: None,
            cli_agent_session_id: Some("session-1".to_owned()),
            is_active: false,
            is_pinned: false,
            updated_at_unix_ms: None,
            is_live_container,
        }
    }

    #[test]
    fn session_relationship_presence_classifier_is_exhaustive() {
        let cases = [
            (SessionPresenceEvidence::default(), SessionPresence::Cold),
            (
                SessionPresenceEvidence {
                    current_carrier: true,
                    pending_materialization: true,
                    ..Default::default()
                },
                SessionPresence::StartingHere,
            ),
            (
                SessionPresenceEvidence {
                    current_carrier: true,
                    current_runtime: true,
                    ..Default::default()
                },
                SessionPresence::LiveHere,
            ),
            (
                SessionPresenceEvidence {
                    other_owner_count: 1,
                    ..Default::default()
                },
                SessionPresence::LiveElsewhere,
            ),
            (
                SessionPresenceEvidence {
                    retiring_owner_count: 1,
                    ..Default::default()
                },
                SessionPresence::UndoRetained,
            ),
            (
                SessionPresenceEvidence {
                    retiring_owner_count: 1,
                    retiring_shutdown_requested: true,
                    ..Default::default()
                },
                SessionPresence::Stopping,
            ),
            (
                SessionPresenceEvidence {
                    current_runtime: true,
                    ..Default::default()
                },
                SessionPresence::Unknown,
            ),
        ];
        for (evidence, expected) in cases {
            assert_eq!(classify_session_presence(evidence), expected);
        }
    }

    #[test]
    fn session_relationship_identity_projection_uses_canonical_keys() {
        for session in [fixture(false), fixture(true)] {
            let projection = session_identity_projection(&session);
            assert_eq!(projection.logical_key, session.logical_key());
            assert_eq!(
                projection.durable_identity_key,
                session.durable_identity_key()
            );
            assert_eq!(
                projection.observed_identity_keys,
                session.observed_identity_keys()
            );
            assert_eq!(projection.alias_subject, session.alias_subject());
            assert!(matches!(
                projection.alias_subject,
                WorkspaceSessionAliasSubject::DurableSession(_)
            ));
        }
    }
}
