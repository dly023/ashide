use crate::terminal::cli_agent::AgentCapabilities;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentActionIntent {
    New,
    Resume,
    Attach,
    Fork,
    Edit,
    WriteBack,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentReadiness {
    pub installed: bool,
    pub runtime_ready: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentActionContext {
    NewSession,
    SessionContext {
        resumable_carrier: bool,
        live_carrier: bool,
        bridge_carrier: bool,
        edit_carrier: bool,
        native_target_carrier: bool,
    },
}

pub fn project_agent_actions(
    capabilities: AgentCapabilities,
    readiness: AgentReadiness,
    context: AgentActionContext,
) -> Vec<AgentActionIntent> {
    const ORDER: [AgentActionIntent; 6] = [
        AgentActionIntent::New,
        AgentActionIntent::Resume,
        AgentActionIntent::Attach,
        AgentActionIntent::Fork,
        AgentActionIntent::Edit,
        AgentActionIntent::WriteBack,
    ];

    ORDER
        .into_iter()
        .filter(|intent| match (intent, context) {
            (AgentActionIntent::New, AgentActionContext::NewSession) => {
                readiness.installed && capabilities.can_detect
            }
            (
                AgentActionIntent::Resume,
                AgentActionContext::SessionContext {
                    resumable_carrier, ..
                },
            ) => readiness.runtime_ready && capabilities.can_resume && resumable_carrier,
            (
                AgentActionIntent::Attach,
                AgentActionContext::SessionContext { live_carrier, .. },
            ) => readiness.runtime_ready && capabilities.can_attach && live_carrier,
            (
                AgentActionIntent::Fork,
                AgentActionContext::SessionContext { bridge_carrier, .. },
            ) => readiness.runtime_ready && capabilities.can_fork && bridge_carrier,
            (AgentActionIntent::Edit, AgentActionContext::SessionContext { edit_carrier, .. }) => {
                readiness.runtime_ready && capabilities.can_edit_preview && edit_carrier
            }
            (
                AgentActionIntent::WriteBack,
                AgentActionContext::SessionContext {
                    native_target_carrier,
                    ..
                },
            ) => {
                readiness.runtime_ready
                    && capabilities.can_write_native_history
                    && native_target_carrier
            }
            (AgentActionIntent::New, AgentActionContext::SessionContext { .. })
            | (
                AgentActionIntent::Resume
                | AgentActionIntent::Attach
                | AgentActionIntent::Fork
                | AgentActionIntent::Edit
                | AgentActionIntent::WriteBack,
                AgentActionContext::NewSession,
            ) => false,
        })
        .collect()
}

#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentSourceSession {
    pub id: u64,
    pub provider: String,
}

#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentActionPublication {
    Idle,
    Publishing(AgentActionIntent),
    Published(AgentActionIntent),
}

#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentActionPublicationState {
    pub source_sessions: Vec<AgentSourceSession>,
    pub selection: Option<u64>,
    pub order: Vec<u64>,
    pub publication: AgentActionPublication,
}

#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentActionPublicationAttempt {
    original: AgentActionPublicationState,
    intent: AgentActionIntent,
}

#[cfg(test)]
impl AgentActionPublicationState {
    pub fn begin(&mut self, intent: AgentActionIntent) -> AgentActionPublicationAttempt {
        let attempt = AgentActionPublicationAttempt {
            original: self.clone(),
            intent,
        };
        self.publication = AgentActionPublication::Publishing(intent);
        attempt
    }

    pub fn complete(
        &mut self,
        attempt: AgentActionPublicationAttempt,
        result: Result<(), AgentActionPublicationError>,
    ) {
        match result {
            Ok(()) => self.publication = AgentActionPublication::Published(attempt.intent),
            Err(AgentActionPublicationError) => *self = attempt.original,
        }
    }

    pub fn cancel(&mut self, attempt: AgentActionPublicationAttempt) {
        *self = attempt.original;
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentActionPublicationError;

#[cfg(test)]
mod tests {
    use super::*;

    fn all_capabilities() -> AgentCapabilities {
        AgentCapabilities {
            can_detect: true,
            can_bind_live_session: true,
            can_index_sessions: true,
            can_list_sessions: true,
            can_cold_restore: true,
            can_resume: true,
            can_attach: true,
            can_target_environment_runtime: true,
            can_read_session_ir: true,
            can_fork: true,
            can_edit_preview: true,
            can_write_native_history: true,
            can_launch_derived_session: true,
        }
    }

    #[test]
    fn agent_sidecar_shows_only_contextually_supported_actions() {
        let ready = AgentReadiness {
            installed: true,
            runtime_ready: true,
        };
        assert_eq!(
            project_agent_actions(all_capabilities(), ready, AgentActionContext::NewSession),
            vec![AgentActionIntent::New]
        );
        assert_eq!(
            project_agent_actions(
                all_capabilities(),
                ready,
                AgentActionContext::SessionContext {
                    resumable_carrier: true,
                    live_carrier: false,
                    bridge_carrier: true,
                    edit_carrier: false,
                    native_target_carrier: true,
                },
            ),
            vec![
                AgentActionIntent::Resume,
                AgentActionIntent::Fork,
                AgentActionIntent::WriteBack,
            ]
        );
        assert!(project_agent_actions(
            all_capabilities(),
            AgentReadiness {
                installed: true,
                runtime_ready: false,
            },
            AgentActionContext::SessionContext {
                resumable_carrier: true,
                live_carrier: true,
                bridge_carrier: true,
                edit_carrier: true,
                native_target_carrier: true,
            },
        )
        .is_empty());
    }

    #[test]
    fn adding_agent_does_not_add_provider_times_action_top_level_items() {
        for provider_count in [0, 1, 7] {
            let providers = (0..provider_count).collect::<Vec<_>>();
            let domain = project_agent_actions(
                all_capabilities(),
                AgentReadiness {
                    installed: true,
                    runtime_ready: true,
                },
                AgentActionContext::NewSession,
            );
            assert_eq!(domain.len(), 1, "provider count was {}", providers.len());
        }
    }

    fn publication_state() -> AgentActionPublicationState {
        AgentActionPublicationState {
            source_sessions: vec![
                AgentSourceSession {
                    id: 1,
                    provider: "alpha".into(),
                },
                AgentSourceSession {
                    id: 2,
                    provider: "beta".into(),
                },
                AgentSourceSession {
                    id: 3,
                    provider: "gamma".into(),
                },
            ],
            selection: Some(2),
            order: vec![3, 2, 1],
            publication: AgentActionPublication::Idle,
        }
    }

    #[test]
    fn cancel_or_failed_conversion_preserves_source_session_and_selection() {
        let original = publication_state();
        let mut cancelled = original.clone();
        let attempt = cancelled.begin(AgentActionIntent::Attach);
        cancelled.cancel(attempt);
        assert_eq!(cancelled, original);

        let mut failed = original.clone();
        let attempt = failed.begin(AgentActionIntent::WriteBack);
        failed.complete(attempt, Err(AgentActionPublicationError));
        assert_eq!(failed, original);
    }
}
