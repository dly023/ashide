use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::terminal::CLIAgent;

use super::adapter_registry::{session_bridge_adapter_for_target, SessionBridgeForkTarget};
use super::ir::{SessionIr, SessionSourceProvenance};
use super::transform::{edit_session, fork_session, SessionEditSpec};
use super::SessionBridgeError;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeSessionIdentity {
    pub agent: CLIAgent,
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SessionBridgeTargetIdentity {
    Ashide { session_id: String },
    Native(NativeSessionIdentity),
}

impl SessionBridgeTargetIdentity {
    pub fn session_id(&self) -> &str {
        match self {
            Self::Ashide { session_id }
            | Self::Native(NativeSessionIdentity { session_id, .. }) => session_id,
        }
    }

    pub fn target(&self) -> SessionBridgeForkTarget {
        match self {
            Self::Ashide { .. } => SessionBridgeForkTarget::Ashide,
            Self::Native(identity) => SessionBridgeForkTarget::Agent(identity.agent),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionBridgeIntent {
    Read,
    Attach {
        existing_target: SessionBridgeTargetIdentity,
    },
    Fork {
        target: SessionBridgeForkTarget,
    },
    EditPreview {
        target: SessionBridgeForkTarget,
        edit: SessionEditSpec,
    },
    WriteBack {
        target: NativeSessionIdentity,
    },
}

impl SessionBridgeIntent {
    pub fn target(&self) -> Option<SessionBridgeForkTarget> {
        match self {
            Self::Read => None,
            Self::Attach { existing_target } => Some(existing_target.target()),
            Self::Fork { target } | Self::EditPreview { target, .. } => Some(*target),
            Self::WriteBack { target } => Some(SessionBridgeForkTarget::Agent(target.agent)),
        }
    }

    pub fn writes_target(&self) -> bool {
        matches!(
            self,
            Self::Fork { .. } | Self::EditPreview { .. } | Self::WriteBack { .. }
        )
    }

    pub fn launches_target(&self) -> bool {
        matches!(self, Self::Fork { .. } | Self::EditPreview { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionBridgeLifecyclePhase {
    Prepared,
    Previewed,
    Confirmed,
    Staged,
    Launched,
    Published,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionBridgeLifecyclePreview {
    pub operation_id: Uuid,
    pub intent: SessionBridgeIntent,
    pub source: SessionSourceProvenance,
    pub target: Option<SessionBridgeTargetIdentity>,
    pub session: SessionIr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionBridgeStageRequest {
    pub operation_id: Uuid,
    pub intent: SessionBridgeIntentKind,
    pub target: Option<SessionBridgeTargetIdentity>,
    pub source: SessionSourceProvenance,
    pub expected_source_revision: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionBridgeIntentKind {
    Read,
    Attach,
    Fork,
    EditPreview,
    WriteBack,
}

impl From<&SessionBridgeIntent> for SessionBridgeIntentKind {
    fn from(intent: &SessionBridgeIntent) -> Self {
        match intent {
            SessionBridgeIntent::Read => Self::Read,
            SessionBridgeIntent::Attach { .. } => Self::Attach,
            SessionBridgeIntent::Fork { .. } => Self::Fork,
            SessionBridgeIntent::EditPreview { .. } => Self::EditPreview,
            SessionBridgeIntent::WriteBack { .. } => Self::WriteBack,
        }
    }
}

pub trait SessionBridgeLifecycleTransport {
    fn stage_write(
        &mut self,
        request: &SessionBridgeStageRequest,
        session: &SessionIr,
    ) -> Result<(), SessionBridgeError>;

    fn launch(&mut self, request: &SessionBridgeStageRequest) -> Result<(), SessionBridgeError>;

    fn publish_atomically(
        &mut self,
        request: &SessionBridgeStageRequest,
    ) -> Result<(), SessionBridgeError>;

    fn cleanup_staging(&mut self, operation_id: Uuid) -> Result<(), SessionBridgeError>;
}

#[derive(Debug, Clone)]
pub struct SessionBridgeLifecycle {
    operation_id: Uuid,
    intent: SessionBridgeIntent,
    source: SessionIr,
    source_provenance: SessionSourceProvenance,
    expected_source_revision: String,
    target: Option<SessionBridgeTargetIdentity>,
    prepared_session: SessionIr,
    phase: SessionBridgeLifecyclePhase,
}

impl SessionBridgeLifecycle {
    pub fn prepare_fork_derivation(
        source: &SessionIr,
        target: SessionBridgeForkTarget,
        prepared_session: SessionIr,
    ) -> Result<Self, SessionBridgeError> {
        let intent = SessionBridgeIntent::Fork { target };
        validate_intent(source, &intent, None)?;
        let target = match target {
            SessionBridgeForkTarget::Ashide => SessionBridgeTargetIdentity::Ashide {
                session_id: prepared_session.session_id.clone(),
            },
            SessionBridgeForkTarget::Agent(agent) => {
                SessionBridgeTargetIdentity::Native(NativeSessionIdentity {
                    agent,
                    session_id: prepared_session.session_id.clone(),
                })
            }
        };
        Uuid::parse_str(target.session_id()).map_err(|_| {
            SessionBridgeError::InvalidLifecycleTransition {
                message: "prepared fork target identity is not a canonical UUID".to_owned(),
            }
        })?;
        Ok(Self {
            operation_id: Uuid::new_v4(),
            intent,
            source: source.clone(),
            source_provenance: source.source_provenance(),
            expected_source_revision: session_revision(source)?,
            target: Some(target),
            prepared_session,
            phase: SessionBridgeLifecyclePhase::Prepared,
        })
    }

    pub fn prepare(
        source: &SessionIr,
        source_native_identity: Option<NativeSessionIdentity>,
        intent: SessionBridgeIntent,
    ) -> Result<Self, SessionBridgeError> {
        validate_intent(source, &intent, source_native_identity.as_ref())?;

        let source_provenance = source.source_provenance();
        let expected_source_revision = session_revision(source)?;
        let (target, prepared_session) = match &intent {
            SessionBridgeIntent::Read => (None, source.clone()),
            SessionBridgeIntent::Attach { existing_target } => {
                (Some(existing_target.clone()), source.clone())
            }
            SessionBridgeIntent::Fork { target } => {
                let target = generated_target_identity(*target);
                let derivation = fork_session(source, Some(target.session_id().to_owned()));
                (Some(target), derivation.session)
            }
            SessionBridgeIntent::EditPreview { target, edit } => {
                let target = generated_target_identity(*target);
                let derivation =
                    edit_session(source, edit.clone(), Some(target.session_id().to_owned()))?;
                (Some(target), derivation.session)
            }
            SessionBridgeIntent::WriteBack { target } => (
                Some(SessionBridgeTargetIdentity::Native(target.clone())),
                source.clone(),
            ),
        };

        Ok(Self {
            operation_id: Uuid::new_v4(),
            intent,
            source: source.clone(),
            source_provenance,
            expected_source_revision,
            target,
            prepared_session,
            phase: SessionBridgeLifecyclePhase::Prepared,
        })
    }

    pub fn operation_id(&self) -> Uuid {
        self.operation_id
    }

    pub fn phase(&self) -> SessionBridgeLifecyclePhase {
        self.phase
    }

    pub fn target_identity(&self) -> Option<&SessionBridgeTargetIdentity> {
        self.target.as_ref()
    }

    pub fn prepared_session(&self) -> &SessionIr {
        &self.prepared_session
    }

    pub fn preview(&mut self) -> Result<SessionBridgeLifecyclePreview, SessionBridgeError> {
        self.require_phase(SessionBridgeLifecyclePhase::Prepared)?;
        self.phase = SessionBridgeLifecyclePhase::Previewed;
        Ok(SessionBridgeLifecyclePreview {
            operation_id: self.operation_id,
            intent: self.intent.clone(),
            source: self.source_provenance.clone(),
            target: self.target.clone(),
            session: self.prepared_session.clone(),
        })
    }

    pub fn confirm(&mut self) -> Result<(), SessionBridgeError> {
        self.require_phase(SessionBridgeLifecyclePhase::Previewed)?;
        if matches!(self.intent, SessionBridgeIntent::Read) {
            return Err(SessionBridgeError::InvalidLifecycleTransition {
                message: "Read ends at preview and cannot be confirmed".to_owned(),
            });
        }
        self.phase = SessionBridgeLifecyclePhase::Confirmed;
        Ok(())
    }

    pub fn stage_write(
        &mut self,
        transport: &mut impl SessionBridgeLifecycleTransport,
    ) -> Result<(), SessionBridgeError> {
        self.require_phase(SessionBridgeLifecyclePhase::Confirmed)?;
        let request = self.stage_request();
        if self.intent.writes_target() {
            if let Err(error) = transport.stage_write(&request, &self.prepared_session) {
                return self.fail_and_cleanup(transport, error);
            }
        }
        self.phase = SessionBridgeLifecyclePhase::Staged;
        Ok(())
    }

    pub fn launch(
        &mut self,
        transport: &mut impl SessionBridgeLifecycleTransport,
    ) -> Result<(), SessionBridgeError> {
        self.require_phase(SessionBridgeLifecyclePhase::Staged)?;
        let request = self.stage_request();
        if self.intent.launches_target() {
            if let Err(error) = transport.launch(&request) {
                return self.fail_and_cleanup(transport, error);
            }
        }
        self.phase = SessionBridgeLifecyclePhase::Launched;
        Ok(())
    }

    pub fn publish(
        &mut self,
        transport: &mut impl SessionBridgeLifecycleTransport,
    ) -> Result<(), SessionBridgeError> {
        self.require_phase(SessionBridgeLifecyclePhase::Launched)?;
        let request = self.stage_request();
        if let Err(error) = transport.publish_atomically(&request) {
            return self.fail_and_cleanup(transport, error);
        }
        self.phase = SessionBridgeLifecyclePhase::Published;
        Ok(())
    }

    pub fn cancel(
        &mut self,
        transport: &mut impl SessionBridgeLifecycleTransport,
    ) -> Result<(), SessionBridgeError> {
        match self.phase {
            SessionBridgeLifecyclePhase::Published
            | SessionBridgeLifecyclePhase::Cancelled
            | SessionBridgeLifecyclePhase::Failed => {
                return Err(SessionBridgeError::InvalidLifecycleTransition {
                    message: format!(
                        "cannot cancel SessionBridge transaction in {:?}",
                        self.phase
                    ),
                });
            }
            SessionBridgeLifecyclePhase::Prepared
            | SessionBridgeLifecyclePhase::Previewed
            | SessionBridgeLifecyclePhase::Confirmed
            | SessionBridgeLifecyclePhase::Staged
            | SessionBridgeLifecyclePhase::Launched => {}
        }
        transport.cleanup_staging(self.operation_id)?;
        self.phase = SessionBridgeLifecyclePhase::Cancelled;
        Ok(())
    }

    pub fn source_is_unchanged(&self, source: &SessionIr) -> bool {
        &self.source == source
    }

    fn stage_request(&self) -> SessionBridgeStageRequest {
        SessionBridgeStageRequest {
            operation_id: self.operation_id,
            intent: (&self.intent).into(),
            target: self.target.clone(),
            source: self.source_provenance.clone(),
            expected_source_revision: self.expected_source_revision.clone(),
        }
    }

    fn require_phase(
        &self,
        expected: SessionBridgeLifecyclePhase,
    ) -> Result<(), SessionBridgeError> {
        if self.phase != expected {
            return Err(SessionBridgeError::InvalidLifecycleTransition {
                message: format!("expected {expected:?}, found {:?}", self.phase),
            });
        }
        Ok(())
    }

    fn fail_and_cleanup<T>(
        &mut self,
        transport: &mut impl SessionBridgeLifecycleTransport,
        error: SessionBridgeError,
    ) -> Result<T, SessionBridgeError> {
        let cleanup = transport.cleanup_staging(self.operation_id);
        self.phase = SessionBridgeLifecyclePhase::Failed;
        match cleanup {
            Ok(()) => Err(error),
            Err(cleanup_error) => Err(SessionBridgeError::InvalidLifecycleTransition {
                message: format!("{error}; staging cleanup also failed: {cleanup_error}"),
            }),
        }
    }
}

fn session_revision(session: &SessionIr) -> Result<String, SessionBridgeError> {
    let bytes = serde_json::to_vec(session)?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn generated_target_identity(target: SessionBridgeForkTarget) -> SessionBridgeTargetIdentity {
    let session_id = Uuid::new_v4().to_string();
    match target {
        SessionBridgeForkTarget::Ashide => SessionBridgeTargetIdentity::Ashide { session_id },
        SessionBridgeForkTarget::Agent(agent) => {
            SessionBridgeTargetIdentity::Native(NativeSessionIdentity { agent, session_id })
        }
    }
}

fn validate_intent(
    source: &SessionIr,
    intent: &SessionBridgeIntent,
    source_native_identity: Option<&NativeSessionIdentity>,
) -> Result<(), SessionBridgeError> {
    let Some(target) = intent.target() else {
        return Ok(());
    };
    let adapter = session_bridge_adapter_for_target(target).ok_or_else(|| {
        SessionBridgeError::InvalidLifecycleTransition {
            message: format!("{} has no SessionBridge adapter", target.display_label()),
        }
    })?;

    match intent {
        SessionBridgeIntent::Read => Ok(()),
        SessionBridgeIntent::Attach { existing_target } => {
            let supported = match existing_target {
                SessionBridgeTargetIdentity::Ashide { .. } => true,
                SessionBridgeTargetIdentity::Native(identity) => {
                    identity.agent.capabilities().can_attach
                }
            };
            if supported {
                Ok(())
            } else {
                Err(unsupported_intent(adapter.label, "Attach"))
            }
        }
        SessionBridgeIntent::Fork { target } => {
            let supported = match target {
                SessionBridgeForkTarget::Ashide => true,
                SessionBridgeForkTarget::Agent(agent) => {
                    agent.capabilities().can_fork
                        && agent.capabilities().can_write_native_history
                        && agent.capabilities().can_launch_derived_session
                }
            };
            if supported {
                Ok(())
            } else {
                Err(unsupported_intent(adapter.label, "Fork"))
            }
        }
        SessionBridgeIntent::EditPreview { target, .. } => {
            let supported = match target {
                SessionBridgeForkTarget::Ashide => true,
                SessionBridgeForkTarget::Agent(agent) => {
                    agent.capabilities().can_edit_preview
                        && agent.capabilities().can_write_native_history
                        && agent.capabilities().can_launch_derived_session
                }
            };
            if supported {
                Ok(())
            } else {
                Err(unsupported_intent(adapter.label, "EditPreview"))
            }
        }
        SessionBridgeIntent::WriteBack { target } => {
            if source_native_identity != Some(target)
                || source.session_id != target.session_id
                || !source_provider_matches_agent(&source.source, target.agent)
            {
                return Err(SessionBridgeError::InvalidLifecycleTransition {
                    message:
                        "WriteBack requires an exact matching source native provider and identity"
                            .to_owned(),
                });
            }
            if target.agent.capabilities().can_write_native_history {
                Ok(())
            } else {
                Err(unsupported_intent(adapter.label, "WriteBack"))
            }
        }
    }
}

fn source_provider_matches_agent(source: &str, agent: CLIAgent) -> bool {
    match agent {
        CLIAgent::Jcode => source.eq_ignore_ascii_case("jcode"),
        CLIAgent::Claude => source.eq_ignore_ascii_case("claude"),
        CLIAgent::Codex => source.eq_ignore_ascii_case("codex"),
        CLIAgent::Amp
        | CLIAgent::Droid
        | CLIAgent::OpenCode
        | CLIAgent::Copilot
        | CLIAgent::Pi
        | CLIAgent::Auggie
        | CLIAgent::CursorCli
        | CLIAgent::Goose
        | CLIAgent::DeepSeek
        | CLIAgent::Antigravity
        | CLIAgent::Omp
        | CLIAgent::Unknown => false,
    }
}

fn unsupported_intent(label: &str, intent: &str) -> SessionBridgeError {
    SessionBridgeError::InvalidLifecycleTransition {
        message: format!("{label} does not support SessionBridge {intent}"),
    }
}
