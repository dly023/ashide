use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, SecondsFormat, TimeZone, Utc};
use serde_json::{json, Value};
use uuid::Uuid;
use walkdir::WalkDir;

#[cfg(feature = "local_fs")]
use command::blocking::Command;

#[cfg(test)]
use crate::cli_agent_jsonl::require_cli_agent_home;
use crate::cli_agent_jsonl::CliAgentStoreRoots;
use crate::session_bridge::adapter_registry::{
    session_bridge_adapter_for_target, SessionBridgeForkTarget,
};
use crate::terminal::CLIAgent;

use super::ir::{SessionIr, SessionMessageIr, SessionTimestamp};
use super::lifecycle::{
    NativeSessionIdentity, SessionBridgeLifecycleTransport, SessionBridgeStageRequest,
    SessionBridgeTargetIdentity,
};
use super::SessionBridgeError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeSessionWriteReceipt {
    pub target: SessionBridgeForkTarget,
    pub session_id: String,
    pub title: String,
    pub project_path: String,
    pub session_file: PathBuf,
    pub backup_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeSessionRemoteWriteReceipt {
    pub target: SessionBridgeForkTarget,
    pub session_id: String,
    pub title: String,
    pub project_path: String,
    pub session_file: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeSessionWriteOperation {
    Write { path: String, contents: Vec<u8> },
    Append { path: String, contents: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeSessionWritePlan {
    pub receipt: NativeSessionRemoteWriteReceipt,
    pub operations: Vec<NativeSessionWriteOperation>,
    pub backup_paths: Vec<String>,
}

pub(crate) struct ProductionSessionBridgeLifecycleTransport<'a> {
    roots: &'a CliAgentStoreRoots,
    staged: Option<(NativeSessionWritePlan, NativeSessionWriteReceipt)>,
    published: Option<NativeSessionWriteReceipt>,
}

impl<'a> ProductionSessionBridgeLifecycleTransport<'a> {
    pub(crate) fn new(roots: &'a CliAgentStoreRoots) -> Self {
        Self {
            roots,
            staged: None,
            published: None,
        }
    }

    pub(crate) fn take_published_receipt(
        &mut self,
    ) -> Result<NativeSessionWriteReceipt, SessionBridgeError> {
        self.published
            .take()
            .ok_or_else(|| SessionBridgeError::InvalidLifecycleTransition {
                message: "native SessionBridge result is not published".to_owned(),
            })
    }
}

impl SessionBridgeLifecycleTransport for ProductionSessionBridgeLifecycleTransport<'_> {
    fn stage_write(
        &mut self,
        request: &SessionBridgeStageRequest,
        session: &SessionIr,
    ) -> Result<(), SessionBridgeError> {
        let SessionBridgeTargetIdentity::Native(target_identity) = request
            .target
            .as_ref()
            .ok_or_else(|| SessionBridgeError::InvalidLifecycleTransition {
                message: "native SessionBridge stage is missing target identity".to_owned(),
            })?
        else {
            return Err(SessionBridgeError::InvalidLifecycleTransition {
                message: "native SessionBridge transport received an Ashide target".to_owned(),
            });
        };
        let target = SessionBridgeForkTarget::Agent(target_identity.agent);
        let target_context = local_native_session_target_context(target, self.roots)?;
        let project_context = local_native_session_project_context(session, &self.roots.home_dir)?;
        let plan = plan_native_session_write_for_target_identity(
            session,
            target_identity,
            target_context,
            project_context,
        )?;
        let receipt = execute_local_native_session_write_plan(&self.roots.home_dir, &plan)?;
        self.staged = Some((plan, receipt));
        Ok(())
    }

    fn launch(&mut self, request: &SessionBridgeStageRequest) -> Result<(), SessionBridgeError> {
        let receipt = self
            .staged
            .as_ref()
            .map(|(_, receipt)| receipt)
            .ok_or_else(|| SessionBridgeError::InvalidLifecycleTransition {
                message: "native SessionBridge launch requires staged bytes".to_owned(),
            })?;
        if request
            .target
            .as_ref()
            .map(SessionBridgeTargetIdentity::session_id)
            != Some(receipt.session_id.as_str())
        {
            return Err(SessionBridgeError::InvalidLifecycleTransition {
                message: "native SessionBridge launch target changed after staging".to_owned(),
            });
        }
        Ok(())
    }

    fn publish_atomically(
        &mut self,
        request: &SessionBridgeStageRequest,
    ) -> Result<(), SessionBridgeError> {
        let (_, receipt) =
            self.staged
                .take()
                .ok_or_else(|| SessionBridgeError::InvalidLifecycleTransition {
                    message: "native SessionBridge publish requires staged bytes".to_owned(),
                })?;
        if request
            .target
            .as_ref()
            .map(SessionBridgeTargetIdentity::session_id)
            != Some(receipt.session_id.as_str())
        {
            return Err(SessionBridgeError::InvalidLifecycleTransition {
                message: "native SessionBridge publish target changed after staging".to_owned(),
            });
        }
        self.published = Some(receipt);
        Ok(())
    }

    fn cleanup_staging(&mut self, _operation_id: Uuid) -> Result<(), SessionBridgeError> {
        if let Some((plan, receipt)) = self.staged.take() {
            rollback_local_native_session_write_plan(
                &self.roots.home_dir,
                &receipt.backup_dir,
                &plan,
            )?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodexTargetConfiguration {
    model_provider: String,
    cli_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NativeSessionTargetContext {
    Codex {
        provider_root: String,
        target_configuration: CodexTargetConfiguration,
    },
    Claude {
        provider_root: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeSessionProjectContext {
    canonical_path: String,
}

impl NativeSessionProjectContext {
    pub(crate) fn from_target_canonical_path(
        canonical_path: String,
    ) -> Result<Self, SessionBridgeError> {
        let canonical_path = canonical_path.trim();
        if canonical_path.is_empty() {
            return Err(SessionBridgeError::InvalidImport {
                message: "target native session project path is empty".to_owned(),
            });
        }
        Ok(Self {
            canonical_path: canonical_path.to_owned(),
        })
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.canonical_path
    }
}

impl NativeSessionTargetContext {
    fn target(&self) -> SessionBridgeForkTarget {
        match self {
            Self::Codex { .. } => SessionBridgeForkTarget::Agent(CLIAgent::Codex),
            Self::Claude { .. } => SessionBridgeForkTarget::Agent(CLIAgent::Claude),
        }
    }

    fn provider_root(&self) -> &str {
        match self {
            Self::Codex { provider_root, .. } | Self::Claude { provider_root } => provider_root,
        }
    }
}

#[derive(Debug, Clone)]
struct NativeSessionWritePlanSeed {
    session_id: String,
    timestamp: DateTime<Utc>,
    claude_message_ids: Vec<String>,
}

impl NativeSessionWritePlanSeed {
    fn new(session: &SessionIr, target: SessionBridgeForkTarget) -> Self {
        let claude_message_ids = if target == SessionBridgeForkTarget::Agent(CLIAgent::Claude) {
            session
                .messages
                .iter()
                .map(|_| Uuid::new_v4().to_string())
                .collect()
        } else {
            Vec::new()
        };
        Self {
            session_id: Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            claude_message_ids,
        }
    }

    fn for_target_identity(
        session: &SessionIr,
        target_context: &NativeSessionTargetContext,
        target_identity: &NativeSessionIdentity,
    ) -> Result<Self, SessionBridgeError> {
        if target_context.target() != SessionBridgeForkTarget::Agent(target_identity.agent) {
            return Err(SessionBridgeError::InvalidLifecycleTransition {
                message: "native writer target context does not match lifecycle target identity"
                    .to_owned(),
            });
        }
        let session_id = Uuid::parse_str(&target_identity.session_id).map_err(|_| {
            SessionBridgeError::InvalidLifecycleTransition {
                message: format!(
                    "native lifecycle target identity is not a canonical UUID: {}",
                    target_identity.session_id
                ),
            }
        })?;
        if session.session_id != target_identity.session_id {
            return Err(SessionBridgeError::InvalidLifecycleTransition {
                message: format!(
                    "prepared session identity {} does not match lifecycle target identity {}",
                    session.session_id, target_identity.session_id
                ),
            });
        }
        let claude_message_ids =
            if target_context.target() == SessionBridgeForkTarget::Agent(CLIAgent::Claude) {
                session
                    .messages
                    .iter()
                    .map(|_| Uuid::new_v4().to_string())
                    .collect()
            } else {
                Vec::new()
            };
        Ok(Self {
            session_id: session_id.to_string(),
            timestamp: Utc::now(),
            claude_message_ids,
        })
    }
}

#[cfg(test)]
pub(crate) fn execute_native_session_lifecycle_with_home_for_test(
    session: &SessionIr,
    target: SessionBridgeForkTarget,
    home: Option<PathBuf>,
) -> Result<NativeSessionWriteReceipt, SessionBridgeError> {
    let home_dir =
        require_cli_agent_home(home).map_err(|error| SessionBridgeError::InvalidImport {
            message: error.to_string(),
        })?;
    let roots = CliAgentStoreRoots::for_current_process(home_dir);
    execute_native_session_lifecycle_with_roots_for_test(session, target, &roots)
}

#[cfg(test)]
pub(crate) fn execute_native_session_lifecycle_to_home_for_test(
    session: &SessionIr,
    target: SessionBridgeForkTarget,
    home_dir: &Path,
) -> Result<NativeSessionWriteReceipt, SessionBridgeError> {
    let roots = CliAgentStoreRoots::for_current_process(home_dir.to_path_buf());
    execute_native_session_lifecycle_with_roots_for_test(session, target, &roots)
}

#[cfg(test)]
pub(crate) fn execute_native_session_lifecycle_with_roots_for_test(
    session: &SessionIr,
    target: SessionBridgeForkTarget,
    roots: &CliAgentStoreRoots,
) -> Result<NativeSessionWriteReceipt, SessionBridgeError> {
    let adapter = session_bridge_adapter_for_target(target).ok_or_else(|| {
        SessionBridgeError::InvalidImport {
            message: format!(
                "{} has no registered SessionBridge adapter",
                target.display_label()
            ),
        }
    })?;
    if !adapter
        .agent
        .is_some_and(|agent| agent.capabilities().can_write_native_history)
    {
        return Err(SessionBridgeError::InvalidImport {
            message: format!(
                "{} does not support native SessionBridge write-back",
                adapter.label
            ),
        });
    }
    let target_context = local_native_session_target_context(target, roots)?;
    let project_context = local_native_session_project_context(session, &roots.home_dir)?;
    let seed = NativeSessionWritePlanSeed::new(session, target_context.target());
    let plan = build_native_session_write_plan(session, &target_context, &project_context, &seed)?;
    execute_local_native_session_write_plan(&roots.home_dir, &plan)
}

pub(crate) fn plan_native_session_write_for_home_root(
    session: &SessionIr,
    target_context: NativeSessionTargetContext,
    project_context: NativeSessionProjectContext,
) -> Result<NativeSessionWritePlan, SessionBridgeError> {
    let target = target_context.target();
    let adapter = session_bridge_adapter_for_target(target).ok_or_else(|| {
        SessionBridgeError::InvalidImport {
            message: format!(
                "{} has no registered SessionBridge adapter",
                target.display_label()
            ),
        }
    })?;
    if !adapter
        .agent
        .is_some_and(|agent| agent.capabilities().can_write_native_history)
    {
        return Err(SessionBridgeError::InvalidImport {
            message: format!(
                "{} does not support native SessionBridge write-back",
                adapter.label
            ),
        });
    }

    let seed = NativeSessionWritePlanSeed::new(session, target);
    build_native_session_write_plan(session, &target_context, &project_context, &seed)
}

/// Builds the native staging plan for one typed SessionBridge lifecycle target.
///
/// Unlike the legacy writer wrappers, this entry never allocates or infers identity. The
/// lifecycle transaction owns target identity before target configuration is read or bytes are
/// staged, and local/runtime transports consume the same resulting plan.
pub(crate) fn plan_native_session_write_for_target_identity(
    session: &SessionIr,
    target_identity: &NativeSessionIdentity,
    target_context: NativeSessionTargetContext,
    project_context: NativeSessionProjectContext,
) -> Result<NativeSessionWritePlan, SessionBridgeError> {
    let adapter = session_bridge_adapter_for_target(target_context.target()).ok_or_else(|| {
        SessionBridgeError::InvalidImport {
            message: format!(
                "{} has no registered SessionBridge adapter",
                target_context.target().display_label()
            ),
        }
    })?;
    if !target_identity
        .agent
        .capabilities()
        .can_write_native_history
    {
        return Err(SessionBridgeError::InvalidImport {
            message: format!(
                "{} does not support native SessionBridge write-back",
                adapter.label
            ),
        });
    }
    let seed =
        NativeSessionWritePlanSeed::for_target_identity(session, &target_context, target_identity)?;
    build_native_session_write_plan(session, &target_context, &project_context, &seed)
}

#[cfg(test)]
pub(crate) fn deterministic_local_and_remote_native_write_plans_for_test(
    session: &SessionIr,
    target: SessionBridgeForkTarget,
    home_dir: &Path,
) -> Result<(NativeSessionWritePlan, NativeSessionWritePlan), SessionBridgeError> {
    let timestamp = Utc
        .with_ymd_and_hms(2026, 7, 15, 12, 0, 0)
        .single()
        .expect("fixed native write timestamp must be valid");
    let seed = NativeSessionWritePlanSeed {
        session_id: "11111111-1111-4111-8111-111111111111".to_owned(),
        timestamp,
        claude_message_ids: session
            .messages
            .iter()
            .enumerate()
            .map(|(index, _)| format!("22222222-2222-4222-8222-{index:012}"))
            .collect(),
    };
    let roots = CliAgentStoreRoots::for_current_process(home_dir.to_path_buf());
    let target_context = local_native_session_target_context(target, &roots)?;
    let project_context = local_native_session_project_context(session, home_dir)?;
    let local = build_native_session_write_plan(session, &target_context, &project_context, &seed)?;
    let remote =
        build_native_session_write_plan(session, &target_context, &project_context, &seed)?;
    Ok((local, remote))
}

fn build_native_session_write_plan(
    session: &SessionIr,
    target_context: &NativeSessionTargetContext,
    project_context: &NativeSessionProjectContext,
    seed: &NativeSessionWritePlanSeed,
) -> Result<NativeSessionWritePlan, SessionBridgeError> {
    match target_context {
        NativeSessionTargetContext::Codex {
            target_configuration,
            ..
        } => build_codex_native_session_write_plan(
            session,
            target_context.provider_root(),
            target_configuration,
            project_context,
            seed,
        ),
        NativeSessionTargetContext::Claude { .. } => build_claude_native_session_write_plan(
            session,
            target_context.provider_root(),
            project_context,
            seed,
        ),
    }
}

fn build_codex_native_session_write_plan(
    session: &SessionIr,
    root: &str,
    target_configuration: &CodexTargetConfiguration,
    project_context: &NativeSessionProjectContext,
    seed: &NativeSessionWritePlanSeed,
) -> Result<NativeSessionWritePlan, SessionBridgeError> {
    // Native field provenance:
    // - protocol constants: type/originator/source/thread_source/instructions
    // - source-derived: title, messages and first user text
    // - target-config-derived: cwd, cli_version and model_provider
    // - generated identity: target session id, timestamps and rollout path
    // Target-owned provider/model/profile/permission fields must never be copied from SessionIr.
    let sid = seed.session_id.clone();
    let now = seed.timestamp;
    let timestamp = now.to_rfc3339_opts(SecondsFormat::Secs, true);
    let project_path = project_context.as_str().to_owned();
    let title = native_title(session);
    let first_user_message = first_user_message(session).unwrap_or_else(|| title.clone());
    let rollout_path = codex_rollout_path_for_root(root, &sid, now);

    let mut rows = vec![json!({
        "timestamp": timestamp,
        "type": "session_meta",
        "payload": {
            "id": sid,
            "timestamp": timestamp,
            "cwd": project_path,
            "originator": "codex-tui",
            "source": "cli",
            "thread_source": "user",
            "cli_version": target_configuration.cli_version,
            "instructions": null,
            "model_provider": target_configuration.model_provider,
        },
    })];
    rows.extend(codex_transcript_rows(&session.messages, &timestamp));
    rows.push(json!({
        "timestamp": timestamp,
        "type": "turn_context",
        "payload": {
            "cwd": project_path,
            "summary": title,
        },
    }));

    let mut operations = vec![NativeSessionWriteOperation::Write {
        path: rollout_path.clone(),
        contents: jsonl_values_to_bytes(&rows)?,
    }];
    operations.push(NativeSessionWriteOperation::Append {
        path: native_join(root, &["session_index.jsonl"]),
        contents: jsonl_value_to_bytes(&json!({
            "id": sid,
            "thread_name": title,
            "updated_at": timestamp,
        }))?,
    });
    operations.push(NativeSessionWriteOperation::Append {
        path: native_join(root, &["history.jsonl"]),
        contents: jsonl_value_to_bytes(&json!({
            "session_id": sid,
            "ts": now.timestamp(),
            "text": first_user_message,
        }))?,
    });

    Ok(NativeSessionWritePlan {
        receipt: NativeSessionRemoteWriteReceipt {
            target: SessionBridgeForkTarget::Agent(CLIAgent::Codex),
            session_id: sid,
            title,
            project_path,
            session_file: rollout_path,
        },
        operations,
        backup_paths: ["session_index.jsonl", "history.jsonl"]
            .map(|path| native_join(root, &[path]))
            .to_vec(),
    })
}
fn build_claude_native_session_write_plan(
    session: &SessionIr,
    root: &str,
    project_context: &NativeSessionProjectContext,
    seed: &NativeSessionWritePlanSeed,
) -> Result<NativeSessionWritePlan, SessionBridgeError> {
    // Native field provenance:
    // - protocol constants: isSidechain/userType/entrypoint/type
    // - source-derived: title, message role/text and source timestamps
    // - target-config-derived: canonical target cwd and provider root
    // - generated identity: target session id, row UUIDs and missing timestamps
    // Claude runtime model/provider/profile/permission fields are target-owned and omitted.
    if seed.claude_message_ids.len() != session.messages.len() {
        return Err(SessionBridgeError::InvalidImport {
            message: "Claude native write plan seed does not match message count".to_owned(),
        });
    }
    let project_path = project_context.as_str().to_owned();
    let title = native_title(session);
    let project_slug = project_to_claude_slug(&project_path);
    let project_dir = native_join(root, &["projects", &project_slug]);
    let sid = seed.session_id.clone();
    let session_filename = format!("{sid}.jsonl");
    let session_file = native_join(&project_dir, &[&session_filename]);
    let timestamp = seed.timestamp.to_rfc3339_opts(SecondsFormat::Secs, true);

    let mut rows = Vec::new();
    let mut parent_uuid = None::<String>;
    let mut leaf_uuid = None::<String>;
    for (message, row_uuid) in session.messages.iter().zip(&seed.claude_message_ids) {
        let row_timestamp = source_timestamp_iso(message.timestamp.as_ref(), &timestamp);
        rows.push(json!({
            "isSidechain": false,
            "userType": "external",
            "entrypoint": "cli",
            "cwd": project_path,
            "sessionId": sid,
            "parentUuid": parent_uuid,
            "type": message.role,
            "uuid": row_uuid,
            "timestamp": row_timestamp,
            "message": claude_message_payload(&message.role, &message.text, row_uuid),
        }));
        parent_uuid = Some(row_uuid.clone());
        leaf_uuid = Some(row_uuid.clone());
    }
    rows.push(json!({"type": "last-prompt", "lastPrompt": title, "leafUuid": leaf_uuid, "sessionId": sid}));

    let operations = vec![
        NativeSessionWriteOperation::Write {
            path: session_file.clone(),
            contents: jsonl_values_to_bytes(&rows)?,
        },
        NativeSessionWriteOperation::Append {
            path: native_join(root, &["history.jsonl"]),
            contents: jsonl_value_to_bytes(&json!({
                "display": title,
                "pastedContents": {},
                "timestamp": seed.timestamp.timestamp_millis(),
                "project": project_path,
                "sessionId": sid,
            }))?,
        },
    ];

    Ok(NativeSessionWritePlan {
        receipt: NativeSessionRemoteWriteReceipt {
            target: SessionBridgeForkTarget::Agent(CLIAgent::Claude),
            session_id: sid,
            title,
            project_path,
            session_file,
        },
        operations,
        backup_paths: vec![native_join(root, &["history.jsonl"]), project_dir],
    })
}

fn local_native_session_target_context(
    target: SessionBridgeForkTarget,
    roots: &CliAgentStoreRoots,
) -> Result<NativeSessionTargetContext, SessionBridgeError> {
    match target {
        SessionBridgeForkTarget::Agent(CLIAgent::Codex) => {
            let provider_root = roots.codex_home.clone();
            let config_path = provider_root.join("config.toml");
            let contents =
                fs::read(&config_path).map_err(|error| SessionBridgeError::InvalidImport {
                    message: format!(
                        "failed to read target Codex configuration {}: {error}",
                        config_path.display()
                    ),
                })?;
            let version_output =
                Command::new("codex")
                    .arg("--version")
                    .output()
                    .map_err(|error| SessionBridgeError::InvalidImport {
                        message: format!("failed to execute target Codex --version: {error}"),
                    })?;
            if !version_output.status.success() {
                return Err(SessionBridgeError::InvalidImport {
                    message: format!(
                        "target Codex --version failed with status {}",
                        version_output.status
                    ),
                });
            }
            codex_native_target_context(
                provider_root.to_string_lossy().into_owned(),
                &contents,
                &config_path.display().to_string(),
                &version_output.stdout,
                "target Codex --version",
            )
        }
        SessionBridgeForkTarget::Agent(CLIAgent::Claude) => {
            Ok(NativeSessionTargetContext::Claude {
                provider_root: roots.claude_config_dir.to_string_lossy().into_owned(),
            })
        }
        SessionBridgeForkTarget::Ashide => Err(SessionBridgeError::InvalidImport {
            message: "Ashide does not use native CLI-agent history write-back".to_owned(),
        }),
        SessionBridgeForkTarget::Agent(agent) => Err(SessionBridgeError::InvalidImport {
            message: format!(
                "{} does not support native SessionBridge write-back",
                agent.display_name()
            ),
        }),
    }
}

fn local_native_session_project_context(
    session: &SessionIr,
    home_dir: &Path,
) -> Result<NativeSessionProjectContext, SessionBridgeError> {
    let requested_path = session
        .project_path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(Path::new)
        .unwrap_or(home_dir);
    let canonical_path =
        requested_path
            .canonicalize()
            .map_err(|error| SessionBridgeError::InvalidImport {
                message: format!(
                    "failed to resolve target native session project path {}: {error}",
                    requested_path.display()
                ),
            })?;
    NativeSessionProjectContext::from_target_canonical_path(
        canonical_path.to_string_lossy().into_owned(),
    )
}

pub(crate) fn codex_native_target_context(
    provider_root: String,
    config_contents: &[u8],
    config_source: &str,
    version_output: &[u8],
    version_source: &str,
) -> Result<NativeSessionTargetContext, SessionBridgeError> {
    let config_contents = std::str::from_utf8(config_contents).map_err(|error| {
        SessionBridgeError::InvalidImport {
            message: format!("target Codex configuration {config_source} is not UTF-8: {error}"),
        }
    })?;
    let config = toml::from_str::<toml::Value>(config_contents).map_err(|error| {
        SessionBridgeError::InvalidImport {
            message: format!("failed to parse target Codex configuration {config_source}: {error}"),
        }
    })?;
    let model_provider = config
        .get("model_provider")
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| SessionBridgeError::InvalidImport {
            message: format!(
                "target Codex configuration {config_source} must define a non-empty model_provider"
            ),
        })?
        .to_owned();
    let version_output =
        std::str::from_utf8(version_output).map_err(|error| SessionBridgeError::InvalidImport {
            message: format!("{version_source} output is not UTF-8: {error}"),
        })?;
    let cli_version = version_output
        .split_whitespace()
        .last()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| SessionBridgeError::InvalidImport {
            message: format!("{version_source} returned no version"),
        })?
        .to_owned();
    Ok(NativeSessionTargetContext::Codex {
        provider_root,
        target_configuration: CodexTargetConfiguration {
            model_provider,
            cli_version,
        },
    })
}

pub(crate) fn codex_native_config_path(provider_root: &str) -> String {
    native_join(provider_root, &["config.toml"])
}

fn execute_local_native_session_write_plan(
    home_dir: &Path,
    plan: &NativeSessionWritePlan,
) -> Result<NativeSessionWriteReceipt, SessionBridgeError> {
    let paths_to_backup = plan
        .backup_paths
        .iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    let backup_dir = backup_paths(home_dir, &paths_to_backup)?;

    let result = (|| {
        for operation in &plan.operations {
            match operation {
                NativeSessionWriteOperation::Write { path, contents } => {
                    let path = Path::new(path);
                    if let Some(parent) = path.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::write(path, contents)?;
                }
                NativeSessionWriteOperation::Append { path, contents } => {
                    let path = Path::new(path);
                    if let Some(parent) = path.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    use std::io::Write;
                    fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)?
                        .write_all(contents)?;
                }
            }
        }

        let session_file = PathBuf::from(&plan.receipt.session_file);
        if fs::metadata(&session_file)?.len() == 0 {
            return Err(SessionBridgeError::InvalidImport {
                message: "native write verification failed: session file is empty".to_owned(),
            });
        }
        Ok(session_file)
    })();

    match result {
        Ok(session_file) => Ok(NativeSessionWriteReceipt {
            target: plan.receipt.target,
            session_id: plan.receipt.session_id.clone(),
            title: plan.receipt.title.clone(),
            project_path: plan.receipt.project_path.clone(),
            session_file,
            backup_dir,
        }),
        Err(error) => {
            if let Err(rollback_error) =
                rollback_local_native_session_write_plan(home_dir, &backup_dir, plan)
            {
                return Err(SessionBridgeError::InvalidImport {
                    message: format!(
                        "native write failed ({error}); rollback also failed: {rollback_error}"
                    ),
                });
            }
            Err(error)
        }
    }
}

fn rollback_local_native_session_write_plan(
    home_dir: &Path,
    backup_dir: &Path,
    plan: &NativeSessionWritePlan,
) -> Result<(), SessionBridgeError> {
    for operation in &plan.operations {
        let path = match operation {
            NativeSessionWriteOperation::Write { path, .. }
            | NativeSessionWriteOperation::Append { path, .. } => Path::new(path),
        };
        if !plan
            .backup_paths
            .iter()
            .any(|backup_path| path.starts_with(Path::new(backup_path)))
            && path.exists()
        {
            fs::remove_file(path)?;
        }
    }

    for path in &plan.backup_paths {
        let path = PathBuf::from(path);
        let relative_path = path
            .strip_prefix(home_dir)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| {
                path.file_name()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("unknown"))
            });
        let backup = backup_dir.join(relative_path);
        if path.exists() {
            if path.is_dir() {
                fs::remove_dir_all(&path)?;
            } else {
                fs::remove_file(&path)?;
            }
        }
        if backup.is_dir() {
            copy_dir_recursive(&backup, &path)?;
        } else if backup.is_file() {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(backup, path)?;
        }
    }
    Ok(())
}

fn native_title(session: &SessionIr) -> String {
    let title = session.title.trim();
    if title.is_empty() {
        "Untitled fork".to_owned()
    } else {
        title.to_owned()
    }
}

fn first_user_message(session: &SessionIr) -> Option<String> {
    session
        .messages
        .iter()
        .find(|message| message.role == "user")
        .map(|message| message.text.clone())
        .filter(|text| !text.trim().is_empty())
}

fn codex_rollout_path_for_root(root: &str, sid: &str, timestamp: DateTime<Utc>) -> String {
    let year = timestamp.format("%Y").to_string();
    let month = timestamp.format("%m").to_string();
    let day = timestamp.format("%d").to_string();
    let filename = format!(
        "rollout-{}-{sid}.jsonl",
        timestamp.format("%Y-%m-%dT%H-%M-%S")
    );
    native_join(root, &["sessions", &year, &month, &day, &filename])
}

fn codex_transcript_rows(messages: &[SessionMessageIr], fallback_timestamp: &str) -> Vec<Value> {
    let mut rows = Vec::new();
    let mut turn_id = None::<String>;
    let mut last_agent_message = None::<String>;
    let mut user_turn_count = 0usize;

    for message in messages {
        let timestamp = source_timestamp_iso(message.timestamp.as_ref(), fallback_timestamp);
        if message.role == "user" {
            if let Some(turn_id) = turn_id.take() {
                rows.push(codex_turn_complete_row(
                    fallback_timestamp,
                    &turn_id,
                    last_agent_message.as_deref(),
                    None,
                ));
            }
            user_turn_count += 1;
            let new_turn_id = format!("ashide-session-bridge-turn-{user_turn_count}");
            rows.push(codex_turn_started_row(
                &timestamp,
                &new_turn_id,
                message.timestamp.as_ref(),
            ));
            rows.extend(codex_message_rows(message, &timestamp));
            turn_id = Some(new_turn_id);
            last_agent_message = None;
            continue;
        }

        if turn_id.is_none() {
            user_turn_count += 1;
            let new_turn_id = format!("ashide-session-bridge-turn-{user_turn_count}");
            rows.push(codex_turn_started_row(
                &timestamp,
                &new_turn_id,
                message.timestamp.as_ref(),
            ));
            turn_id = Some(new_turn_id);
        }
        rows.extend(codex_message_rows(message, &timestamp));
        if message.role == "assistant" {
            last_agent_message = Some(message.text.clone());
        }
    }

    if let Some(turn_id) = turn_id {
        rows.push(json!({
            "timestamp": fallback_timestamp,
            "type": "event_msg",
            "payload": {
                "type": "agent_message",
                "message": "<ASHIDE SESSION FORK>",
                "phase": null,
                "memory_citation": null,
            },
        }));
        rows.push(codex_token_count_row(fallback_timestamp, messages));
        rows.push(codex_turn_complete_row(
            fallback_timestamp,
            &turn_id,
            last_agent_message.as_deref(),
            messages
                .last()
                .and_then(|message| message.timestamp.as_ref()),
        ));
    }
    rows
}

fn codex_message_rows(message: &SessionMessageIr, timestamp: &str) -> Vec<Value> {
    match message.role.as_str() {
        "user" => vec![
            json!({
                "timestamp": timestamp,
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": message.text}],
                },
            }),
            json!({
                "timestamp": timestamp,
                "type": "event_msg",
                "payload": {
                    "type": "user_message",
                    "message": message.text,
                    "images": [],
                    "local_images": [],
                    "text_elements": [],
                },
            }),
        ],
        "assistant" => vec![
            json!({
                "timestamp": timestamp,
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": message.text}],
                },
            }),
            json!({
                "timestamp": timestamp,
                "type": "event_msg",
                "payload": {
                    "type": "agent_message",
                    "message": message.text,
                    "phase": "final_answer",
                    "memory_citation": null,
                },
            }),
        ],
        _ => Vec::new(),
    }
}

fn codex_turn_started_row(
    timestamp: &str,
    turn_id: &str,
    source_timestamp: Option<&SessionTimestamp>,
) -> Value {
    let mut payload = json!({
        "type": "task_started",
        "turn_id": turn_id,
        "trace_id": null,
        "model_context_window": null,
    });
    if let Some(started_at) = source_timestamp_seconds(source_timestamp) {
        payload["started_at"] = json!(started_at);
    }
    json!({"timestamp": timestamp, "type": "event_msg", "payload": payload})
}

fn codex_turn_complete_row(
    timestamp: &str,
    turn_id: &str,
    last_agent_message: Option<&str>,
    source_timestamp: Option<&SessionTimestamp>,
) -> Value {
    let mut payload = json!({
        "type": "task_complete",
        "turn_id": turn_id,
        "last_agent_message": last_agent_message,
        "duration_ms": null,
        "time_to_first_token_ms": null,
    });
    if let Some(completed_at) = source_timestamp_seconds(source_timestamp) {
        payload["completed_at"] = json!(completed_at);
    }
    json!({"timestamp": timestamp, "type": "event_msg", "payload": payload})
}

fn codex_token_count_row(timestamp: &str, messages: &[SessionMessageIr]) -> Value {
    let total_tokens = messages
        .iter()
        .map(|message| (message.text.chars().count() / 4).max(1))
        .sum::<usize>();
    let usage = json!({"total_tokens": total_tokens});
    json!({
        "timestamp": timestamp,
        "type": "event_msg",
        "payload": {
            "type": "token_count",
            "info": {
                "total_token_usage": usage,
                "last_token_usage": usage,
                "model_context_window": null,
            },
            "rate_limits": null,
        },
    })
}

fn claude_message_payload(role: &str, text: &str, row_uuid: &str) -> Value {
    if role == "user" {
        return json!({"role": "user", "content": text});
    }
    json!({
        "id": row_uuid,
        "type": "message",
        "role": "assistant",
        "model": "<synthetic>",
        "content": [{"type": "text", "text": text}],
        "stop_reason": "end_turn",
        "stop_sequence": null,
        "usage": {},
    })
}

fn project_to_claude_slug(project_path: &str) -> String {
    let normalized = project_path.trim();
    let normalized = if normalized.is_empty() {
        "workspace"
    } else {
        normalized
    };
    let slug = normalized
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>();
    if slug.is_empty() {
        "-".to_owned()
    } else {
        slug
    }
}

fn source_timestamp_iso(timestamp: Option<&SessionTimestamp>, fallback: &str) -> String {
    source_datetime(timestamp)
        .map(|timestamp| timestamp.to_rfc3339_opts(SecondsFormat::Secs, true))
        .unwrap_or_else(|| fallback.to_owned())
}

fn source_timestamp_seconds(timestamp: Option<&SessionTimestamp>) -> Option<i64> {
    source_datetime(timestamp).map(|timestamp| timestamp.timestamp())
}

fn source_datetime(timestamp: Option<&SessionTimestamp>) -> Option<DateTime<Utc>> {
    match timestamp? {
        SessionTimestamp::String(value) => DateTime::parse_from_rfc3339(value)
            .ok()
            .map(|timestamp| timestamp.with_timezone(&Utc)),
        SessionTimestamp::Integer(value) => {
            if *value > 100_000_000_000 {
                Utc.timestamp_millis_opt(*value).single()
            } else {
                Utc.timestamp_opt(*value, 0).single()
            }
        }
        SessionTimestamp::Float(value) => {
            let millis = if *value > 100_000_000_000. {
                *value as i64
            } else {
                (*value * 1000.) as i64
            };
            Utc.timestamp_millis_opt(millis).single()
        }
    }
}

fn jsonl_value_to_string(row: &Value) -> Result<String, SessionBridgeError> {
    let mut line = serde_json::to_string(row)?;
    line.push('\n');
    Ok(line)
}

fn jsonl_value_to_bytes(row: &Value) -> Result<Vec<u8>, SessionBridgeError> {
    Ok(jsonl_value_to_string(row)?.into_bytes())
}

fn jsonl_values_to_string(rows: &[Value]) -> Result<String, SessionBridgeError> {
    let mut contents = String::new();
    for row in rows {
        contents.push_str(&jsonl_value_to_string(row)?);
    }
    Ok(contents)
}

fn jsonl_values_to_bytes(rows: &[Value]) -> Result<Vec<u8>, SessionBridgeError> {
    Ok(jsonl_values_to_string(rows)?.into_bytes())
}

fn native_join(root: &str, segments: &[&str]) -> String {
    let separator = native_path_separator(root);
    let mut path = root.trim_end_matches(['/', '\\']).to_owned();
    if path.is_empty() {
        path.push(separator);
    }
    for segment in segments {
        let segment = segment.trim_matches(['/', '\\']);
        if segment.is_empty() {
            continue;
        }
        if path != separator.to_string() && !path.ends_with(['/', '\\']) {
            path.push(separator);
        }
        path.push_str(segment);
    }
    path
}

fn native_path_separator(path: &str) -> char {
    if path.contains('\\') || path.as_bytes().get(1).is_some_and(|byte| *byte == b':') {
        '\\'
    } else {
        '/'
    }
}

fn backup_paths(home_dir: &Path, paths: &[PathBuf]) -> Result<PathBuf, SessionBridgeError> {
    let backup_dir = home_dir
        .join(".agents")
        .join("session-bridge")
        .join("backups")
        .join(format!(
            "{}-{}",
            Utc::now().format("%Y%m%dT%H%M%SZ"),
            Uuid::new_v4()
        ));
    fs::create_dir_all(&backup_dir)?;

    for path in paths.iter().filter(|path| path.exists()) {
        let relative_path = path
            .strip_prefix(home_dir)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| {
                path.file_name()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("unknown"))
            });
        let destination = backup_dir.join(relative_path);
        if path.is_dir() {
            copy_dir_recursive(path, &destination)?;
        } else {
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(path, destination)?;
        }
    }
    Ok(backup_dir)
}

fn copy_dir_recursive(source: &Path, destination: &Path) -> Result<(), SessionBridgeError> {
    for entry in WalkDir::new(source).follow_links(false) {
        let entry = entry.map_err(|error| SessionBridgeError::Io(error.into()))?;
        let relative_path = entry
            .path()
            .strip_prefix(source)
            .map_err(|error| SessionBridgeError::Io(std::io::Error::other(error)))?;
        let target = destination.join(relative_path);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}
