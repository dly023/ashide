use std::path::{Path, PathBuf};

use super::bundle::{
    build_bundle, bundle_output_path, default_bundle_name, read_bundle, safe_filename_component,
    write_bundle, BUNDLE_FORMAT, BUNDLE_VERSION, SESSION_BRIDGE_VERSION,
};
use super::ir::{SessionArtifactIr, SessionIr, SessionMessageIr, SessionTimestamp};
use super::lifecycle::{
    NativeSessionIdentity, SessionBridgeIntent, SessionBridgeIntentKind, SessionBridgeLifecycle,
    SessionBridgeLifecyclePhase, SessionBridgeLifecycleTransport, SessionBridgeStageRequest,
    SessionBridgeTargetIdentity,
};
use super::preview::SessionBridgePreview;
use super::sanitize::{clean_text, redact, sanitize_embedded_images};
use super::transform::{edit_session, fork_session, SessionEditSpec};

fn sample_session() -> SessionIr {
    SessionIr {
        source: "ashide".to_owned(),
        session_id: "session/id with spaces".to_owned(),
        title: "Test Session".to_owned(),
        project_path: Some("/tmp/project".to_owned()),
        created_at: Some(SessionTimestamp::String("2026-06-18T00:00:00Z".to_owned())),
        updated_at: Some(SessionTimestamp::String("2026-06-18T00:01:00Z".to_owned())),
        messages: vec![
            SessionMessageIr {
                role: "user".to_owned(),
                text: "hello\n  indented".to_owned(),
                timestamp: Some(SessionTimestamp::String("2026-06-18T00:00:00Z".to_owned())),
            },
            SessionMessageIr {
                role: "assistant".to_owned(),
                text: "token=secret sk-abcdefghijklmnop".to_owned(),
                timestamp: Some(SessionTimestamp::String("2026-06-18T00:00:01Z".to_owned())),
            },
        ],
        artifacts: vec![SessionArtifactIr {
            kind: "artifact".to_owned(),
            text: "image data:image/png;base64,AAAA after".to_owned(),
            path: None,
            metadata: serde_json::Value::Null,
        }],
        metadata: serde_json::json!({ "runId": "run-123" }),
    }
}

struct RecordingLifecycleTransport {
    staged: Vec<(SessionBridgeStageRequest, String)>,
    launched: Vec<SessionBridgeStageRequest>,
    published: Vec<SessionBridgeStageRequest>,
    cleaned: Vec<uuid::Uuid>,
    staging: Vec<uuid::Uuid>,
    revision_conflict: bool,
    carrier_present: bool,
    fail_stage: bool,
    fail_launch: bool,
    fail_publish: bool,
}

impl Default for RecordingLifecycleTransport {
    fn default() -> Self {
        Self {
            staged: Vec::new(),
            launched: Vec::new(),
            published: Vec::new(),
            cleaned: Vec::new(),
            staging: Vec::new(),
            revision_conflict: false,
            carrier_present: true,
            fail_stage: false,
            fail_launch: false,
            fail_publish: false,
        }
    }
}

impl SessionBridgeLifecycleTransport for RecordingLifecycleTransport {
    fn stage_write(
        &mut self,
        request: &SessionBridgeStageRequest,
        session: &SessionIr,
    ) -> Result<(), super::SessionBridgeError> {
        if self.revision_conflict {
            return Err(super::SessionBridgeError::InvalidLifecycleTransition {
                message: "source revision conflict".to_owned(),
            });
        }
        if self.fail_stage {
            return Err(super::SessionBridgeError::InvalidImport {
                message: "injected stage failure".to_owned(),
            });
        }
        self.staged
            .push((request.clone(), session.session_id.clone()));
        self.staging.push(request.operation_id);
        Ok(())
    }

    fn launch(
        &mut self,
        request: &SessionBridgeStageRequest,
    ) -> Result<(), super::SessionBridgeError> {
        if !self.carrier_present {
            return Err(super::SessionBridgeError::InvalidLifecycleTransition {
                message: "target carrier missing".to_owned(),
            });
        }
        if self.fail_launch {
            return Err(super::SessionBridgeError::InvalidImport {
                message: "injected launch failure".to_owned(),
            });
        }
        self.launched.push(request.clone());
        Ok(())
    }

    fn publish_atomically(
        &mut self,
        request: &SessionBridgeStageRequest,
    ) -> Result<(), super::SessionBridgeError> {
        if self.fail_publish {
            return Err(super::SessionBridgeError::InvalidImport {
                message: "injected publish failure".to_owned(),
            });
        }
        self.published.push(request.clone());
        self.staging.retain(|id| *id != request.operation_id);
        Ok(())
    }

    fn cleanup_staging(
        &mut self,
        operation_id: uuid::Uuid,
    ) -> Result<(), super::SessionBridgeError> {
        self.cleaned.push(operation_id);
        self.staging.retain(|id| *id != operation_id);
        Ok(())
    }
}

#[test]
fn fork_creates_new_target_identity_and_preserves_source_provenance() {
    use crate::terminal::CLIAgent;

    use super::adapter_registry::SessionBridgeForkTarget;

    let source = sample_session();
    let mut lifecycle = SessionBridgeLifecycle::prepare(
        &source,
        None,
        SessionBridgeIntent::Fork {
            target: SessionBridgeForkTarget::Agent(CLIAgent::Claude),
        },
    )
    .unwrap();
    let preview = lifecycle.preview().unwrap();
    let target = preview
        .target
        .expect("Fork must allocate a target identity");

    assert_ne!(target.session_id(), source.session_id);
    assert_eq!(preview.session.session_id, target.session_id());
    assert_eq!(preview.source, source.source_provenance());
    assert_eq!(
        preview.session.metadata["sessionBridge"]["sourceProvenance"],
        serde_json::to_value(source.source_provenance()).unwrap()
    );
    assert!(lifecycle.source_is_unchanged(&source));
}

#[test]
fn attach_never_creates_or_rewrites_native_session() {
    use crate::terminal::CLIAgent;

    let source = sample_session();
    let existing_target = SessionBridgeTargetIdentity::Native(NativeSessionIdentity {
        agent: CLIAgent::Claude,
        session_id: "existing-native-session".to_owned(),
    });
    let mut lifecycle = SessionBridgeLifecycle::prepare(
        &source,
        None,
        SessionBridgeIntent::Attach {
            existing_target: existing_target.clone(),
        },
    )
    .unwrap();
    let preview = lifecycle.preview().unwrap();
    lifecycle.confirm().unwrap();
    let mut transport = RecordingLifecycleTransport::default();
    lifecycle.stage_write(&mut transport).unwrap();
    lifecycle.launch(&mut transport).unwrap();
    lifecycle.publish(&mut transport).unwrap();

    assert_eq!(preview.target, Some(existing_target));
    assert!(transport.staged.is_empty());
    assert!(transport.launched.is_empty());
    assert_eq!(transport.published.len(), 1);
    assert!(lifecycle.source_is_unchanged(&source));
}

#[test]
fn writeback_requires_matching_native_identity_and_explicit_capability() {
    use crate::terminal::CLIAgent;

    let mut source = sample_session();
    source.source = "claude".to_owned();
    let identity = NativeSessionIdentity {
        agent: CLIAgent::Claude,
        session_id: source.session_id.clone(),
    };
    let lifecycle = SessionBridgeLifecycle::prepare(
        &source,
        Some(identity.clone()),
        SessionBridgeIntent::WriteBack {
            target: identity.clone(),
        },
    )
    .unwrap();
    assert_eq!(
        lifecycle.target_identity(),
        Some(&SessionBridgeTargetIdentity::Native(identity.clone()))
    );

    let mismatch = NativeSessionIdentity {
        session_id: "different-native-session".to_owned(),
        ..identity
    };
    assert!(SessionBridgeLifecycle::prepare(
        &source,
        Some(mismatch),
        SessionBridgeIntent::WriteBack {
            target: NativeSessionIdentity {
                agent: CLIAgent::Claude,
                session_id: source.session_id.clone(),
            },
        },
    )
    .is_err());
    let unsupported = NativeSessionIdentity {
        agent: CLIAgent::Pi,
        session_id: source.session_id.clone(),
    };
    assert!(SessionBridgeLifecycle::prepare(
        &source,
        Some(unsupported.clone()),
        SessionBridgeIntent::WriteBack {
            target: unsupported,
        },
    )
    .is_err());
}

#[test]
fn failure_or_cancel_leaves_source_and_target_publication_unchanged() {
    use crate::terminal::CLIAgent;

    use super::adapter_registry::SessionBridgeForkTarget;

    let source = sample_session();
    let mut failed = SessionBridgeLifecycle::prepare(
        &source,
        None,
        SessionBridgeIntent::Fork {
            target: SessionBridgeForkTarget::Agent(CLIAgent::Claude),
        },
    )
    .unwrap();
    failed.preview().unwrap();
    failed.confirm().unwrap();
    let mut failed_transport = RecordingLifecycleTransport {
        fail_launch: true,
        ..Default::default()
    };
    failed.stage_write(&mut failed_transport).unwrap();
    assert!(failed.launch(&mut failed_transport).is_err());
    assert_eq!(failed.phase(), SessionBridgeLifecyclePhase::Failed);
    assert!(failed_transport.published.is_empty());
    assert_eq!(failed_transport.cleaned, vec![failed.operation_id()]);
    assert!(failed.source_is_unchanged(&source));

    for mut transport in [
        RecordingLifecycleTransport {
            fail_stage: true,
            ..Default::default()
        },
        RecordingLifecycleTransport {
            fail_publish: true,
            ..Default::default()
        },
    ] {
        let mut transaction = SessionBridgeLifecycle::prepare(
            &source,
            None,
            SessionBridgeIntent::Fork {
                target: SessionBridgeForkTarget::Agent(CLIAgent::Claude),
            },
        )
        .unwrap();
        transaction.preview().unwrap();
        transaction.confirm().unwrap();
        let result = transaction.stage_write(&mut transport).and_then(|()| {
            transaction.launch(&mut transport)?;
            transaction.publish(&mut transport)
        });
        assert!(result.is_err());
        assert_eq!(transaction.phase(), SessionBridgeLifecyclePhase::Failed);
        assert!(transport.published.is_empty());
        assert_eq!(transport.cleaned, vec![transaction.operation_id()]);
        assert!(transaction.source_is_unchanged(&source));
    }

    let mut cancelled = SessionBridgeLifecycle::prepare(
        &source,
        None,
        SessionBridgeIntent::EditPreview {
            target: SessionBridgeForkTarget::Agent(CLIAgent::Claude),
            edit: SessionEditSpec {
                redactions: vec!["secret".to_owned()],
                trim_after: None,
            },
        },
    )
    .unwrap();
    cancelled.preview().unwrap();
    let mut cancelled_transport = RecordingLifecycleTransport::default();
    cancelled.cancel(&mut cancelled_transport).unwrap();
    assert_eq!(cancelled.phase(), SessionBridgeLifecyclePhase::Cancelled);
    assert!(cancelled_transport.staged.is_empty());
    assert!(cancelled_transport.published.is_empty());
    assert_eq!(cancelled_transport.cleaned, vec![cancelled.operation_id()]);
    assert!(cancelled.source_is_unchanged(&source));
}

#[test]
fn revision_conflict_fails_closed_and_cleans_staging() {
    use crate::terminal::CLIAgent;

    use super::adapter_registry::SessionBridgeForkTarget;

    let source = sample_session();
    let mut lifecycle = SessionBridgeLifecycle::prepare(
        &source,
        None,
        SessionBridgeIntent::Fork {
            target: SessionBridgeForkTarget::Agent(CLIAgent::Claude),
        },
    )
    .unwrap();
    lifecycle.preview().unwrap();
    lifecycle.confirm().unwrap();
    let mut transport = RecordingLifecycleTransport {
        revision_conflict: true,
        ..Default::default()
    };

    let error = lifecycle.stage_write(&mut transport).unwrap_err();

    assert!(error.to_string().contains("source revision conflict"));
    assert_eq!(lifecycle.phase(), SessionBridgeLifecyclePhase::Failed);
    assert!(transport.staging.is_empty());
    assert!(transport.published.is_empty());
    assert_eq!(transport.cleaned, vec![lifecycle.operation_id()]);
}

#[test]
fn carrier_missing_after_native_write_fails_closed_and_cleans_staging() {
    use crate::terminal::CLIAgent;

    use super::adapter_registry::SessionBridgeForkTarget;

    let source = sample_session();
    let mut lifecycle = SessionBridgeLifecycle::prepare(
        &source,
        None,
        SessionBridgeIntent::Fork {
            target: SessionBridgeForkTarget::Agent(CLIAgent::Claude),
        },
    )
    .unwrap();
    lifecycle.preview().unwrap();
    lifecycle.confirm().unwrap();
    let mut transport = RecordingLifecycleTransport {
        carrier_present: false,
        ..Default::default()
    };
    lifecycle.stage_write(&mut transport).unwrap();
    assert_eq!(transport.staging, vec![lifecycle.operation_id()]);
    assert!(transport.published.is_empty());

    let error = lifecycle.launch(&mut transport).unwrap_err();

    assert!(error.to_string().contains("target carrier missing"));
    assert_eq!(lifecycle.phase(), SessionBridgeLifecyclePhase::Failed);
    assert!(transport.staging.is_empty());
    assert!(transport.published.is_empty());
    assert_eq!(transport.cleaned, vec![lifecycle.operation_id()]);
}

#[test]
fn publication_is_visible_only_after_write_and_launch_succeed() {
    use crate::terminal::CLIAgent;

    use super::adapter_registry::SessionBridgeForkTarget;

    let source = sample_session();
    let mut lifecycle = SessionBridgeLifecycle::prepare(
        &source,
        None,
        SessionBridgeIntent::Fork {
            target: SessionBridgeForkTarget::Agent(CLIAgent::Claude),
        },
    )
    .unwrap();
    let mut transport = RecordingLifecycleTransport::default();
    lifecycle.preview().unwrap();
    lifecycle.confirm().unwrap();
    assert!(transport.published.is_empty());
    lifecycle.stage_write(&mut transport).unwrap();
    assert!(transport.published.is_empty());
    lifecycle.launch(&mut transport).unwrap();
    assert!(transport.published.is_empty());
    lifecycle.publish(&mut transport).unwrap();
    assert_eq!(transport.published.len(), 1);
    assert!(transport.staging.is_empty());
}

#[test]
fn parallel_conversions_do_not_cross_bind_target_identity() {
    use crate::terminal::CLIAgent;

    use super::adapter_registry::SessionBridgeForkTarget;

    let source = sample_session();
    let mut first = SessionBridgeLifecycle::prepare(
        &source,
        None,
        SessionBridgeIntent::Fork {
            target: SessionBridgeForkTarget::Agent(CLIAgent::Claude),
        },
    )
    .unwrap();
    let mut second = SessionBridgeLifecycle::prepare(
        &source,
        None,
        SessionBridgeIntent::Fork {
            target: SessionBridgeForkTarget::Agent(CLIAgent::Claude),
        },
    )
    .unwrap();
    let first_target = first.target_identity().unwrap().clone();
    let second_target = second.target_identity().unwrap().clone();
    assert_ne!(first.operation_id(), second.operation_id());
    assert_ne!(first_target, second_target);

    let mut transport = RecordingLifecycleTransport::default();
    for lifecycle in [&mut second, &mut first] {
        lifecycle.preview().unwrap();
        lifecycle.confirm().unwrap();
        lifecycle.stage_write(&mut transport).unwrap();
        lifecycle.launch(&mut transport).unwrap();
        lifecycle.publish(&mut transport).unwrap();
    }

    assert_eq!(transport.published.len(), 2);
    assert_eq!(transport.published[0].target, Some(second_target));
    assert_eq!(transport.published[1].target, Some(first_target));
    assert_ne!(
        transport.published[0].operation_id,
        transport.published[1].operation_id
    );
}

#[test]
fn local_and_runtime_share_lifecycle_and_split_only_at_transport() {
    use crate::terminal::CLIAgent;

    let source = sample_session();
    let target_id = "11111111-1111-4111-8111-111111111111".to_owned();
    let intent = SessionBridgeIntent::Attach {
        existing_target: SessionBridgeTargetIdentity::Native(NativeSessionIdentity {
            agent: CLIAgent::Claude,
            session_id: target_id,
        }),
    };
    let mut local = SessionBridgeLifecycle::prepare(&source, None, intent.clone()).unwrap();
    let mut runtime = SessionBridgeLifecycle::prepare(&source, None, intent).unwrap();
    let mut local_transport = RecordingLifecycleTransport::default();
    let mut runtime_transport = RecordingLifecycleTransport::default();

    for (lifecycle, transport) in [
        (&mut local, &mut local_transport),
        (&mut runtime, &mut runtime_transport),
    ] {
        lifecycle.preview().unwrap();
        lifecycle.confirm().unwrap();
        lifecycle.stage_write(transport).unwrap();
        lifecycle.launch(transport).unwrap();
        lifecycle.publish(transport).unwrap();
    }

    assert_eq!(local.phase(), runtime.phase());
    assert_eq!(local.target_identity(), runtime.target_identity());
    assert!(local_transport.staged.is_empty());
    assert!(runtime_transport.staged.is_empty());
    assert!(local_transport.launched.is_empty());
    assert!(runtime_transport.launched.is_empty());
    assert_eq!(local_transport.published.len(), 1);
    assert_eq!(runtime_transport.published.len(), 1);
    assert_eq!(
        local_transport.published[0].intent,
        SessionBridgeIntentKind::Attach
    );
    assert_eq!(
        runtime_transport.published[0].intent,
        SessionBridgeIntentKind::Attach
    );
}

#[cfg(feature = "local_fs")]
#[test]
fn target_identity_plan_never_reuses_source_identity() {
    use crate::terminal::CLIAgent;

    use super::adapter_registry::SessionBridgeForkTarget;

    let tempdir = tempfile::tempdir().unwrap();
    let mut source = native_sample_session(tempdir.path());
    source.session_id = uuid::Uuid::new_v4().to_string();
    let target_context = super::native_writer::NativeSessionTargetContext::Claude {
        provider_root: tempdir.path().join(".claude").display().to_string(),
    };
    let plan = super::native_writer::plan_native_session_write_for_home_root(
        &source,
        target_context,
        target_project_context(source.project_path.clone().unwrap()),
    )
    .unwrap();

    assert_eq!(
        plan.receipt.target,
        SessionBridgeForkTarget::Agent(CLIAgent::Claude)
    );
    assert_ne!(plan.receipt.session_id, source.session_id);
}

#[cfg(feature = "local_fs")]
#[test]
fn typed_native_writer_uses_only_matching_lifecycle_target_identity() {
    use crate::terminal::CLIAgent;

    let tempdir = tempfile::tempdir().unwrap();
    let mut session = native_sample_session(tempdir.path());
    let identity = NativeSessionIdentity {
        agent: CLIAgent::Claude,
        session_id: uuid::Uuid::new_v4().to_string(),
    };
    session.session_id = identity.session_id.clone();
    let target_context = super::native_writer::NativeSessionTargetContext::Claude {
        provider_root: tempdir.path().join(".claude").display().to_string(),
    };
    let plan = super::native_writer::plan_native_session_write_for_target_identity(
        &session,
        &identity,
        target_context.clone(),
        target_project_context(session.project_path.clone().unwrap()),
    )
    .unwrap();
    assert_eq!(plan.receipt.session_id, identity.session_id);

    let mismatched = NativeSessionIdentity {
        session_id: uuid::Uuid::new_v4().to_string(),
        ..identity
    };
    assert!(
        super::native_writer::plan_native_session_write_for_target_identity(
            &session,
            &mismatched,
            target_context,
            target_project_context(session.project_path.clone().unwrap()),
        )
        .is_err()
    );
}

#[cfg(feature = "local_fs")]
#[test]
fn production_transport_stages_cleans_and_publishes_one_target_identity() {
    use crate::cli_agent_jsonl::CliAgentStoreRoots;
    use crate::terminal::CLIAgent;

    use super::adapter_registry::SessionBridgeForkTarget;

    let tempdir = tempfile::tempdir().unwrap();
    with_isolated_native_history_env(tempdir.path(), || {
        let source = native_sample_session(tempdir.path());
        let roots = CliAgentStoreRoots::for_current_process(tempdir.path().to_path_buf());
        let derivation = fork_session(&source, Some(uuid::Uuid::new_v4().to_string()));
        let target_id = derivation.session.session_id.clone();
        let mut lifecycle = SessionBridgeLifecycle::prepare_fork_derivation(
            &source,
            SessionBridgeForkTarget::Agent(CLIAgent::Claude),
            derivation.session,
        )
        .unwrap();
        let mut transport =
            super::native_writer::ProductionSessionBridgeLifecycleTransport::new(&roots);

        let preview = lifecycle.preview().unwrap();
        lifecycle.confirm().unwrap();
        lifecycle.stage_write(&mut transport).unwrap();
        assert!(transport.take_published_receipt().is_err());
        lifecycle.cancel(&mut transport).unwrap();
        assert!(!tempdir
            .path()
            .join(".claude")
            .join("history.jsonl")
            .exists());

        let derivation = fork_session(&source, Some(target_id.clone()));
        let mut lifecycle = SessionBridgeLifecycle::prepare_fork_derivation(
            &source,
            SessionBridgeForkTarget::Agent(CLIAgent::Claude),
            derivation.session,
        )
        .unwrap();
        let mut transport =
            super::native_writer::ProductionSessionBridgeLifecycleTransport::new(&roots);
        lifecycle.preview().unwrap();
        lifecycle.confirm().unwrap();
        lifecycle.stage_write(&mut transport).unwrap();
        lifecycle.launch(&mut transport).unwrap();
        assert!(transport.take_published_receipt().is_err());
        lifecycle.publish(&mut transport).unwrap();
        let receipt = transport.take_published_receipt().unwrap();

        assert_eq!(receipt.session_id, target_id);
        assert_eq!(preview.target.unwrap().session_id(), target_id);
        assert!(receipt.session_file.exists());
    });
}

#[cfg(feature = "local_fs")]
fn native_sample_session(home_dir: &Path) -> SessionIr {
    let project_path = home_dir.join("project");
    std::fs::create_dir_all(&project_path).unwrap();
    let mut session = sample_session();
    session.project_path = Some(project_path.canonicalize().unwrap().display().to_string());
    session
}

#[cfg(feature = "local_fs")]
fn target_project_context(
    path: impl Into<String>,
) -> super::native_writer::NativeSessionProjectContext {
    super::native_writer::NativeSessionProjectContext::from_target_canonical_path(path.into())
        .unwrap()
}

#[cfg(feature = "local_fs")]
fn jsonl_values(contents: &[u8]) -> Vec<serde_json::Value> {
    std::str::from_utf8(contents)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[cfg(feature = "local_fs")]
static NATIVE_HISTORY_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(feature = "local_fs")]
fn with_isolated_native_history_env<T>(home_dir: &Path, f: impl FnOnce() -> T) -> T {
    let _guard = NATIVE_HISTORY_ENV_LOCK
        .lock()
        .expect("native history env lock should not be poisoned");
    let old_codex_home = std::env::var_os("CODEX_HOME");
    let old_claude_config_dir = std::env::var_os("CLAUDE_CONFIG_DIR");
    std::env::set_var("CODEX_HOME", home_dir.join(".codex"));
    std::env::set_var("CLAUDE_CONFIG_DIR", home_dir.join(".claude"));
    write_test_codex_system_configuration(
        home_dir,
        "test-system-provider",
        "test-system-model",
        "high",
    );

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));

    match old_codex_home {
        Some(value) => std::env::set_var("CODEX_HOME", value),
        None => std::env::remove_var("CODEX_HOME"),
    }
    match old_claude_config_dir {
        Some(value) => std::env::set_var("CLAUDE_CONFIG_DIR", value),
        None => std::env::remove_var("CLAUDE_CONFIG_DIR"),
    }

    match result {
        Ok(value) => value,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

#[cfg(feature = "local_fs")]
fn write_test_codex_system_configuration(
    home_dir: &Path,
    provider: &str,
    model: &str,
    reasoning_effort: &str,
) {
    let codex_home = home_dir.join(".codex");
    std::fs::create_dir_all(&codex_home).unwrap();
    std::fs::write(
        codex_home.join("config.toml"),
        format!(
            "model_provider = {provider:?}\nmodel = {model:?}\nmodel_reasoning_effort = {reasoning_effort:?}\n\n[model_providers.{provider}]\nname = {provider:?}\nbase_url = \"https://provider.invalid/v1\"\n"
        ),
    )
    .unwrap();
}

#[cfg(feature = "local_fs")]
fn codex_session_meta_from_receipt(
    receipt: &super::native_writer::NativeSessionWriteReceipt,
) -> serde_json::Value {
    let contents = std::fs::read_to_string(&receipt.session_file).unwrap();
    contents
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|row| row["type"] == "session_meta")
        .expect("Codex native write must include session_meta")
}

#[cfg(feature = "local_fs")]
fn codex_session_meta_from_plan(
    plan: &super::native_writer::NativeSessionWritePlan,
) -> serde_json::Value {
    let contents = plan
        .operations
        .iter()
        .find_map(|operation| match operation {
            super::native_writer::NativeSessionWriteOperation::Write { contents, .. } => {
                Some(contents)
            }
            super::native_writer::NativeSessionWriteOperation::Append { .. } => None,
        })
        .expect("Codex native plan must write a rollout");
    std::str::from_utf8(contents)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|row| row["type"] == "session_meta")
        .expect("Codex native plan must include session_meta")
}

#[cfg(feature = "local_fs")]
fn read_native_receipt_back(
    agent: crate::terminal::CLIAgent,
    receipt: &super::native_writer::NativeSessionWriteReceipt,
) -> super::cli_agent_reader::CliAgentSessionReadResult {
    let bytes = std::fs::read(&receipt.session_file).unwrap();
    super::cli_agent_reader::parse_cli_agent_session_source_bytes(
        agent,
        receipt.session_id.clone(),
        super::cli_agent_reader::CliAgentSessionSourceBytes {
            reference: receipt.session_file.display().to_string(),
            bytes,
        },
        None,
        None,
    )
    .unwrap()
}

#[test]
fn safe_filename_component_preserves_allowed_chars_and_collapses_unsafe_runs() {
    assert_eq!(
        safe_filename_component("abc.DEF_123-xyz"),
        "abc.DEF_123-xyz"
    );
    assert_eq!(
        safe_filename_component(" session/id with spaces "),
        "session-id-with-spaces"
    );
    assert_eq!(safe_filename_component("***"), "session");
}

#[test]
fn default_bundle_name_uses_safe_full_session_id() {
    let session = sample_session();
    assert_eq!(
        default_bundle_name(&session),
        "ashide-sessionbridge-export-ashide-session-id-with-spaces.json"
    );
}

#[test]
fn build_bundle_sets_format_version_and_sanitizes_text() {
    let bundle = build_bundle(&sample_session());
    assert_eq!(bundle.format, BUNDLE_FORMAT);
    assert_eq!(bundle.version, BUNDLE_VERSION);
    assert_eq!(bundle.session.session_id, "session/id with spaces");
    assert_eq!(bundle.session.messages[0].text, "hello\n  indented");
    assert!(bundle.session.messages[1].text.contains("token=[REDACTED]"));
    assert!(!bundle.session.messages[1].text.contains("secret"));
    assert!(!bundle.session.messages[1]
        .text
        .contains("sk-abcdefghijklmnop"));
    assert!(bundle.session.artifacts[0]
        .text
        .contains("[Image attachment not imported: embedded PNG data URL"));
    assert!(bundle.session.artifacts[0].text.ends_with(" after"));
}

#[test]
fn redact_leaves_words_merely_ending_in_pass_alone() {
    let text = redact("set compass=north and multipass: ticket for the trip");
    assert!(text.contains("compass=north"));
    assert!(text.contains("multipass: ticket"));

    let redacted = redact("pass=secret password: hunter2 api_key: abc123");
    assert!(redacted.contains("pass=[REDACTED]"));
    assert!(redacted.contains("password: [REDACTED]"));
    assert!(redacted.contains("api_key: [REDACTED]"));
    assert!(!redacted.contains("hunter2"));
    assert!(!redacted.contains("abc123"));
}

#[test]
fn sanitize_embedded_images_replaces_data_url_and_preserves_surrounding_text() {
    let text = "before input_image data:image/png;base64,AAAA after";
    let result = sanitize_embedded_images(text);
    assert!(result.starts_with("before "));
    assert!(result.contains("embedded PNG data URL, approx 3 B"));
    assert!(result.ends_with(" after"));
}

#[test]
fn clean_text_redacts_then_sanitizes() {
    let result = clean_text("api_key: abc123 data:image/jpeg;base64,AAAA");
    assert!(result.contains("api_key: [REDACTED]"));
    assert!(result.contains("embedded JPEG data URL"));
}

#[test]
fn dry_run_preview_reports_counts_without_writing() {
    let session = sample_session();
    let preview = SessionBridgePreview::from_session(
        &session,
        Some(PathBuf::from("/tmp/session.json")),
        vec!["artifact warning".to_owned()],
    );
    assert_eq!(preview.message_count, 2);
    assert_eq!(preview.artifact_count, 1);
    let text = preview.dry_run_text();
    assert!(text.contains("DRY RUN: would export ashide session session/id with spaces"));
    assert!(text.contains("Messages: 2"));
    assert!(text.contains("Artifacts: 1"));
    assert!(text.contains("- artifact warning"));
}

#[test]
fn bundle_output_path_matches_directory_write_target() {
    let tempdir = tempfile::tempdir().unwrap();
    let session = sample_session();

    let output_path = bundle_output_path(&session, Some(tempdir.path())).unwrap();
    let written_path = write_bundle(&session, Some(tempdir.path())).unwrap();

    assert_eq!(written_path, output_path);
    assert!(written_path.exists());
    assert!(written_path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("ashide-sessionbridge-export-ashide-")));
}

#[test]
fn read_bundle_rejects_wrong_format() {
    let tempdir = tempfile::tempdir().unwrap();
    let path = tempdir.path().join("bundle.json");
    let mut bundle = build_bundle(&sample_session());
    bundle.format = "old-sessionbridge-bundle".to_owned();
    std::fs::write(&path, serde_json::to_string(&bundle).unwrap()).unwrap();

    let error = read_bundle(&path).unwrap_err();

    assert!(error
        .to_string()
        .contains("invalid SessionBridge bundle format"));
}

#[test]
fn read_bundle_rejects_wrong_versions() {
    let tempdir = tempfile::tempdir().unwrap();
    let path = tempdir.path().join("bundle.json");
    let mut bundle = build_bundle(&sample_session());
    bundle.version = BUNDLE_VERSION + 1;
    std::fs::write(&path, serde_json::to_string(&bundle).unwrap()).unwrap();

    let error = read_bundle(&path).unwrap_err();

    assert!(error
        .to_string()
        .contains("unsupported SessionBridge bundle version"));

    bundle.version = BUNDLE_VERSION;
    bundle.session_bridge_version = SESSION_BRIDGE_VERSION + 1;
    std::fs::write(&path, serde_json::to_string(&bundle).unwrap()).unwrap();

    let error = read_bundle(&path).unwrap_err();

    assert!(error
        .to_string()
        .contains("unsupported SessionBridge runtime version"));
}

#[test]
fn live_ashide_conversation_to_session_ir_exports_in_memory_exchange() {
    use std::collections::HashMap;

    use crate::ai::agent::conversation::{AIConversation, AIConversationId};
    use warp_multi_agent_api as api;

    let task = api::Task {
        id: "root-task".to_string(),
        description: "Live title".to_string(),
        dependencies: None,
        messages: vec![
            api::Message {
                id: "user-1".to_string(),
                task_id: "root-task".to_string(),
                request_id: "request-1".to_string(),
                timestamp: None,
                server_message_data: String::new(),
                citations: vec![],
                message: Some(api::message::Message::UserQuery(api::message::UserQuery {
                    query: "live user prompt".to_string(),
                    context: None,
                    referenced_attachments: HashMap::new(),
                    mode: None,
                    intended_agent: Default::default(),
                })),
            },
            api::Message {
                id: "assistant-1".to_string(),
                task_id: "root-task".to_string(),
                request_id: "request-1".to_string(),
                timestamp: None,
                server_message_data: String::new(),
                citations: vec![],
                message: Some(api::message::Message::AgentOutput(
                    api::message::AgentOutput {
                        text: "live assistant response".to_string(),
                    },
                )),
            },
        ],
        summary: String::new(),
        server_data: String::new(),
    };
    let conversation = AIConversation::new_restored(AIConversationId::new(), vec![task], None)
        .expect("test conversation should restore from in-memory task data");

    let read_result = super::ashide_store::live_ashide_conversation_to_session_ir(&conversation);

    assert_eq!(read_result.session.title, "Live title");
    assert_eq!(read_result.session.messages.len(), 2);
    assert_eq!(read_result.session.messages[0].role, "user");
    assert_eq!(read_result.session.messages[0].text, "live user prompt");
    assert_eq!(read_result.session.messages[1].role, "assistant");
    assert_eq!(
        read_result.session.messages[1].text,
        "live assistant response"
    );
    assert!(read_result.warnings.is_empty());
}

#[cfg(feature = "local_fs")]
#[test]
fn adapter_registry_keeps_pi_blocked_until_native_history_contract_exists() {
    use crate::terminal::CLIAgent;

    use super::adapter_registry::{
        session_bridge_adapter_for_agent, session_bridge_fork_targets, SessionBridgeForkTarget,
    };

    assert!(
        session_bridge_adapter_for_agent(CLIAgent::Codex).is_some_and(|adapter| adapter
            .agent
            .is_some_and(|agent| agent.capabilities().can_read_session_ir))
    );
    assert!(
        session_bridge_adapter_for_agent(CLIAgent::Claude).is_some_and(|adapter| adapter
            .agent
            .is_some_and(|agent| agent.capabilities().can_read_session_ir))
    );

    assert!(
        session_bridge_adapter_for_agent(CLIAgent::Pi).is_none(),
        "Pi must not be exposed as SessionBridge-capable until a stable native history/read/write contract exists"
    );
    assert!(
        !session_bridge_fork_targets()
            .any(|target| target == SessionBridgeForkTarget::Agent(CLIAgent::Pi)),
        "SessionBridge fork targets must come from registered adapters, not CLIAgent identity metadata"
    );
}

#[cfg(feature = "local_fs")]
#[test]
fn claude_native_writer_reader_round_trip_preserves_forked_session() {
    use crate::terminal::CLIAgent;

    use super::adapter_registry::SessionBridgeForkTarget;

    let tempdir = tempfile::tempdir().unwrap();
    with_isolated_native_history_env(tempdir.path(), || {
        let source = native_sample_session(tempdir.path());
        let fork = fork_session(&source, Some(uuid::Uuid::new_v4().to_string()));

        let receipt = super::native_writer::execute_native_session_lifecycle_to_home_for_test(
            &fork.session,
            SessionBridgeForkTarget::Agent(CLIAgent::Claude),
            tempdir.path(),
        )
        .unwrap();

        assert!(receipt
            .session_file
            .starts_with(tempdir.path().join(".claude")));
        let read_result = read_native_receipt_back(CLIAgent::Claude, &receipt);

        assert_eq!(read_result.session.source, "claude");
        assert_eq!(read_result.session.session_id, receipt.session_id);
        assert_eq!(read_result.session.title, fork.session.title);
        assert_eq!(read_result.session.project_path, fork.session.project_path);
        assert_eq!(read_result.session.messages, fork.session.messages);
        assert_eq!(read_result.source.source_session_id, receipt.session_id);
        assert_eq!(
            read_result.source.reference,
            receipt.session_file.display().to_string()
        );
    });
}

#[cfg(feature = "local_fs")]
#[test]
fn codex_native_writer_reader_round_trip_preserves_forked_session() {
    use crate::terminal::CLIAgent;

    use super::adapter_registry::SessionBridgeForkTarget;

    let tempdir = tempfile::tempdir().unwrap();
    with_isolated_native_history_env(tempdir.path(), || {
        let source = native_sample_session(tempdir.path());
        let fork = fork_session(&source, Some(uuid::Uuid::new_v4().to_string()));

        let receipt = super::native_writer::execute_native_session_lifecycle_to_home_for_test(
            &fork.session,
            SessionBridgeForkTarget::Agent(CLIAgent::Codex),
            tempdir.path(),
        )
        .unwrap();

        assert!(receipt
            .session_file
            .starts_with(tempdir.path().join(".codex")));
        assert!(receipt
            .session_file
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(&format!("{}.jsonl", receipt.session_id))));
        let read_result = read_native_receipt_back(CLIAgent::Codex, &receipt);

        assert_eq!(read_result.session.source, "codex");
        assert_eq!(read_result.session.session_id, receipt.session_id);
        assert_eq!(read_result.session.title, fork.session.title);
        assert_eq!(read_result.session.project_path, fork.session.project_path);
        assert_eq!(read_result.session.messages, fork.session.messages);
        assert_eq!(read_result.source.source_session_id, receipt.session_id);
        assert_eq!(
            read_result.source.reference,
            receipt.session_file.display().to_string()
        );
    });
}

#[cfg(feature = "local_fs")]
#[test]
fn codex_native_writer_reads_bootstrap_provider_from_target_system_configuration() {
    use crate::terminal::CLIAgent;

    use super::adapter_registry::SessionBridgeForkTarget;

    let tempdir = tempfile::tempdir().unwrap();
    with_isolated_native_history_env(tempdir.path(), || {
        let source = native_sample_session(tempdir.path());
        let receipt = super::native_writer::execute_native_session_lifecycle_to_home_for_test(
            &fork_session(&source, Some(uuid::Uuid::new_v4().to_string())).session,
            SessionBridgeForkTarget::Agent(CLIAgent::Codex),
            tempdir.path(),
        )
        .unwrap();
        let session_meta = codex_session_meta_from_receipt(&receipt);
        assert_eq!(
            session_meta["payload"]["model_provider"],
            "test-system-provider"
        );
        for target_owned_field in [
            "model",
            "profile",
            "reasoning_effort",
            "model_reasoning_effort",
            "sandbox",
            "sandbox_policy",
            "approval",
            "approval_policy",
            "endpoint",
        ] {
            assert!(
                session_meta["payload"].get(target_owned_field).is_none(),
                "SessionBridge must not copy target-owned Codex field {target_owned_field} into the generated rollout"
            );
        }
    });
}

#[cfg(feature = "local_fs")]
#[test]
fn codex_native_writer_without_model_provider_fails_before_writing() {
    use crate::terminal::CLIAgent;

    use super::adapter_registry::SessionBridgeForkTarget;

    let tempdir = tempfile::tempdir().unwrap();
    with_isolated_native_history_env(tempdir.path(), || {
        std::fs::write(tempdir.path().join(".codex/config.toml"), "model = \"x\"\n").unwrap();
        let error = super::native_writer::execute_native_session_lifecycle_to_home_for_test(
            &fork_session(&sample_session(), Some(uuid::Uuid::new_v4().to_string())).session,
            SessionBridgeForkTarget::Agent(CLIAgent::Codex),
            tempdir.path(),
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("must define a non-empty model_provider"));
        assert!(!tempdir.path().join(".codex/sessions").exists());
    });
}

#[cfg(feature = "local_fs")]
#[test]
fn local_and_remote_codex_native_plans_use_target_owned_configuration() {
    let local_context = super::native_writer::codex_native_target_context(
        "/local/.codex".to_owned(),
        b"model_provider = \"local-provider\"\n",
        "/local/.codex/config.toml",
        b"codex-cli 1.2.3\n",
        "test Codex --version",
    )
    .unwrap();
    let remote_context = super::native_writer::codex_native_target_context(
        "/remote/.codex".to_owned(),
        b"model_provider = \"remote-provider\"\n",
        "/remote/.codex/config.toml",
        b"codex-cli 4.5.6\n",
        "test Codex --version",
    )
    .unwrap();
    let local = super::native_writer::plan_native_session_write_for_home_root(
        &sample_session(),
        local_context,
        target_project_context("/local/project"),
    )
    .unwrap();
    let remote = super::native_writer::plan_native_session_write_for_home_root(
        &sample_session(),
        remote_context,
        target_project_context("/remote/project"),
    )
    .unwrap();
    assert_eq!(
        codex_session_meta_from_plan(&local)["payload"]["model_provider"],
        "local-provider"
    );
    assert_eq!(
        codex_session_meta_from_plan(&local)["payload"]["cli_version"],
        "1.2.3"
    );
    assert_eq!(
        codex_session_meta_from_plan(&remote)["payload"]["model_provider"],
        "remote-provider"
    );
    assert_eq!(
        codex_session_meta_from_plan(&remote)["payload"]["cli_version"],
        "4.5.6"
    );
}

#[cfg(feature = "local_fs")]
#[test]
fn codex_native_context_without_target_cli_version_fails() {
    let error = super::native_writer::codex_native_target_context(
        "/target/.codex".to_owned(),
        b"model_provider = \"target-provider\"\n",
        "/target/.codex/config.toml",
        b"  \n",
        "target Codex --version",
    )
    .unwrap_err();

    assert!(error.to_string().contains("returned no version"));
}

#[cfg(feature = "local_fs")]
#[test]
fn claude_native_writer_omits_target_owned_runtime_configuration() {
    use crate::terminal::CLIAgent;

    use super::adapter_registry::SessionBridgeForkTarget;

    let tempdir = tempfile::tempdir().unwrap();
    with_isolated_native_history_env(tempdir.path(), || {
        let source = native_sample_session(tempdir.path());
        let receipt = super::native_writer::execute_native_session_lifecycle_to_home_for_test(
            &fork_session(&source, Some(uuid::Uuid::new_v4().to_string())).session,
            SessionBridgeForkTarget::Agent(CLIAgent::Claude),
            tempdir.path(),
        )
        .unwrap();
        let rows = std::fs::read_to_string(&receipt.session_file)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();

        assert!(!rows.iter().any(|row| matches!(
            row.get("type").and_then(serde_json::Value::as_str),
            Some("mode" | "permission-mode")
        )));
        for row in &rows {
            for target_owned_field in ["version", "gitBranch", "permissionMode"] {
                assert!(
                    row.get(target_owned_field).is_none(),
                    "SessionBridge must not write target-owned Claude field {target_owned_field}"
                );
            }
        }
        for assistant in rows.iter().filter(|row| row["type"] == "assistant") {
            assert_eq!(assistant["message"]["model"], "<synthetic>");
        }
    });
}

#[cfg(feature = "local_fs")]
#[test]
fn claude_native_writer_uses_claude_config_dir() {
    use crate::terminal::CLIAgent;

    use super::adapter_registry::SessionBridgeForkTarget;

    let tempdir = tempfile::tempdir().unwrap();
    with_isolated_native_history_env(tempdir.path(), || {
        let target_config_dir = tempdir.path().join("target-claude-config");
        std::env::set_var("CLAUDE_CONFIG_DIR", &target_config_dir);
        let session = native_sample_session(tempdir.path());
        let receipt = super::native_writer::execute_native_session_lifecycle_to_home_for_test(
            &session,
            SessionBridgeForkTarget::Agent(CLIAgent::Claude),
            tempdir.path(),
        )
        .unwrap();

        assert!(receipt.session_file.starts_with(&target_config_dir));
        assert!(!tempdir.path().join(".claude/projects").exists());
    });
}

#[cfg(all(feature = "local_fs", not(target_family = "wasm")))]
#[test]
#[ignore = "requires an installed Codex CLI"]
fn codex_native_fork_real_cli_resume_bootstrap() {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    use crate::terminal::CLIAgent;

    use super::adapter_registry::SessionBridgeForkTarget;

    fn read_request(stream: &mut TcpStream) -> String {
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let read = stream.read(&mut buffer).unwrap();
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..read]);
            let Some(headers_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n")
            else {
                continue;
            };
            let headers_end = headers_end + 4;
            let headers = String::from_utf8_lossy(&bytes[..headers_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            if bytes.len() >= headers_end + content_length {
                break;
            }
        }
        String::from_utf8(bytes).unwrap()
    }

    let tempdir = tempfile::tempdir().unwrap();
    let canonical_root = tempdir.path().canonicalize().unwrap();
    with_isolated_native_history_env(&canonical_root, || {
        let project_path = canonical_root.join("project");
        std::fs::create_dir_all(&project_path).unwrap();
        let mut source = sample_session();
        source.project_path = Some(project_path.display().to_string());
        source.messages[1].text = "ashide-codex-native-resume-context-marker".to_owned();
        let fork = fork_session(&source, Some(uuid::Uuid::new_v4().to_string()));

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
        std::fs::write(
            canonical_root.join(".codex/config.toml"),
            format!(
                "model_provider = \"ashide-test\"\nmodel = \"ashide-test-model\"\nmodel_reasoning_effort = \"high\"\napproval_policy = \"never\"\nsandbox_mode = \"read-only\"\n\n[model_providers.ashide-test]\nname = \"ashide-test\"\nbase_url = {endpoint:?}\nenv_key = \"ASHIDE_CODEX_TEST_API_KEY\"\nwire_api = \"responses\"\nrequest_max_retries = 0\nstream_max_retries = 0\nrequires_openai_auth = false\n"
            ),
        )
        .unwrap();

        let receipt = super::native_writer::execute_native_session_lifecycle_to_home_for_test(
            &fork.session,
            SessionBridgeForkTarget::Agent(CLIAgent::Codex),
            &canonical_root,
        )
        .unwrap();

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            let body = request
                .split_once("\r\n\r\n")
                .map(|(_, body)| body)
                .unwrap_or_default()
                .to_owned();
            stream
                .write_all(
                    b"HTTP/1.1 418 Intentional Test Stop\r\ncontent-type: application/json\r\ncontent-length: 42\r\nconnection: close\r\n\r\n{\"error\":{\"message\":\"intentional stop\"}}",
                )
                .unwrap();
            stream.flush().unwrap();
            (request, body)
        });

        let output = command::blocking::Command::new("codex")
            .args([
                "exec",
                "resume",
                &receipt.session_id,
                "ashide-codex-native-resume-prompt-marker",
                "--json",
                "--skip-git-repo-check",
            ])
            .current_dir(&project_path)
            .env("CODEX_HOME", canonical_root.join(".codex"))
            .env("ASHIDE_CODEX_TEST_API_KEY", "ashide-native-resume-test")
            .env("TERM", "dumb")
            .output()
            .expect("installed Codex CLI must start");
        let (request, body) = server.join().unwrap();

        assert!(request.starts_with("POST /v1/responses "), "{request}");
        assert!(
            body.contains("ashide-codex-native-resume-context-marker"),
            "real Codex request did not contain restored SessionBridge history: {body}"
        );
        assert!(
            body.contains("ashide-codex-native-resume-prompt-marker"),
            "real Codex request did not contain the new resume prompt: {body}"
        );
        assert!(
            !output.status.success(),
            "intentional mock-provider failure must stop the real Codex command"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stderr.contains("No saved session found"), "{stderr}");
        assert!(
            !stderr.contains("Model provider `codex` not found"),
            "{stderr}"
        );
    });
}

#[cfg(all(feature = "local_fs", not(target_family = "wasm")))]
#[test]
#[ignore = "requires an installed Claude Code CLI"]
fn claude_native_fork_real_cli_resume_bootstrap() {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::os::unix::fs::symlink;
    use std::thread;

    use crate::terminal::CLIAgent;

    use super::adapter_registry::SessionBridgeForkTarget;

    fn read_request(stream: &mut TcpStream) -> String {
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let read = stream.read(&mut buffer).unwrap();
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..read]);
            let Some(headers_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n")
            else {
                continue;
            };
            let headers_end = headers_end + 4;
            let headers = String::from_utf8_lossy(&bytes[..headers_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            if bytes.len() >= headers_end + content_length {
                break;
            }
        }
        String::from_utf8(bytes).unwrap()
    }

    fn respond_ok(stream: &mut TcpStream) {
        let events = [
            serde_json::json!({
                "type": "message_start",
                "message": {
                    "id": "msg_ashide_native_resume",
                    "type": "message",
                    "role": "assistant",
                    "model": "claude-sonnet-4-5-20250929",
                    "content": [],
                    "stop_reason": null,
                    "stop_sequence": null,
                    "usage": {
                        "input_tokens": 1,
                        "cache_creation_input_tokens": 0,
                        "cache_read_input_tokens": 0,
                        "output_tokens": 0
                    }
                }
            }),
            serde_json::json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {"type": "text", "text": ""}
            }),
            serde_json::json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "text_delta", "text": "OK"}
            }),
            serde_json::json!({"type": "content_block_stop", "index": 0}),
            serde_json::json!({
                "type": "message_delta",
                "delta": {"stop_reason": "end_turn", "stop_sequence": null},
                "usage": {"output_tokens": 1}
            }),
            serde_json::json!({"type": "message_stop"}),
        ];
        let event_names = [
            "message_start",
            "content_block_start",
            "content_block_delta",
            "content_block_stop",
            "message_delta",
            "message_stop",
        ];
        let body = event_names
            .iter()
            .zip(events)
            .map(|(name, event)| format!("event: {name}\ndata: {event}\n\n"))
            .collect::<String>();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        stream.flush().unwrap();
    }

    let tempdir = tempfile::tempdir().unwrap();
    let canonical_root = tempdir.path().canonicalize().unwrap();
    with_isolated_native_history_env(&canonical_root, || {
        let project_path = canonical_root.join("project");
        let project_alias = canonical_root.join("project-alias");
        std::fs::create_dir_all(&project_path).unwrap();
        symlink(&project_path, &project_alias).unwrap();
        let mut session = sample_session();
        session.project_path = Some(project_alias.display().to_string());
        session.messages[1].text = "ashide-native-resume-context-marker".to_owned();
        let receipt = super::native_writer::execute_native_session_lifecycle_to_home_for_test(
            &session,
            SessionBridgeForkTarget::Agent(CLIAgent::Claude),
            &canonical_root,
        )
        .unwrap();
        assert_eq!(receipt.project_path, project_path.display().to_string());
        assert!(!receipt
            .session_file
            .to_string_lossy()
            .contains("project-alias"));

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || loop {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            if request.starts_with("HEAD ") {
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
                    .unwrap();
                continue;
            }
            assert!(request.starts_with("POST /v1/messages"));
            assert!(request.contains("ashide-native-resume-context-marker"));
            assert!(request.contains("ashide-native-resume-prompt-marker"));
            respond_ok(&mut stream);
            break;
        });

        let output = command::blocking::Command::new("claude")
            .args([
                "--resume",
                &receipt.session_id,
                "--print",
                "ashide-native-resume-prompt-marker",
                "--output-format",
                "json",
                "--tools",
                "",
                "--safe-mode",
            ])
            .current_dir(&project_alias)
            .env("CLAUDE_CONFIG_DIR", canonical_root.join(".claude"))
            .env("ANTHROPIC_API_KEY", "ashide-native-resume-test")
            .env("ANTHROPIC_BASE_URL", endpoint)
            .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
            .output()
            .expect("installed Claude Code CLI must start");
        server.join().unwrap();

        assert!(
            output.status.success(),
            "Claude failed to bootstrap generated native session: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["subtype"], "success");
        assert_eq!(result["session_id"], receipt.session_id);
        assert_eq!(result["result"], "OK");
    });
}

#[cfg(feature = "local_fs")]
#[test]
fn native_writer_registers_forked_session_for_listing_in_history_and_index() {
    use crate::terminal::CLIAgent;

    use super::adapter_registry::SessionBridgeForkTarget;

    // The round-trip tests above cover "resume" (our reader reads the transcript
    // back). This covers the other half — "list": the forked session must be
    // appended to the tool's discovery file (Claude `history.jsonl` / Codex
    // `session_index.jsonl`) so the real CLI surfaces it. It also pins the
    // discovery-row fields the reader never consumes, so a future field cleanup
    // can't silently drop what makes the session discoverable (ZAP-H2 caution).
    fn jsonl_has_line(path: &Path, pred: impl Fn(&serde_json::Value) -> bool) -> bool {
        let Ok(text) = std::fs::read_to_string(path) else {
            return false;
        };
        text.lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line.trim()).ok())
            .any(|value| pred(&value))
    }

    let tempdir = tempfile::tempdir().unwrap();
    with_isolated_native_history_env(tempdir.path(), || {
        let source = native_sample_session(tempdir.path());
        let claude_fork = fork_session(&source, Some(uuid::Uuid::new_v4().to_string()));
        let claude_receipt =
            super::native_writer::execute_native_session_lifecycle_to_home_for_test(
                &claude_fork.session,
                SessionBridgeForkTarget::Agent(CLIAgent::Claude),
                tempdir.path(),
            )
            .unwrap();
        let claude_history = tempdir.path().join(".claude").join("history.jsonl");
        assert!(
            jsonl_has_line(&claude_history, |row| {
                row["sessionId"] == serde_json::json!(claude_receipt.session_id)
                    && row.get("display").is_some()
            }),
            "claude history.jsonl must list the forked session by sessionId (with a display title)"
        );

        let codex_fork = fork_session(&source, Some(uuid::Uuid::new_v4().to_string()));
        let codex_receipt =
            super::native_writer::execute_native_session_lifecycle_to_home_for_test(
                &codex_fork.session,
                SessionBridgeForkTarget::Agent(CLIAgent::Codex),
                tempdir.path(),
            )
            .unwrap();
        let codex_index = tempdir.path().join(".codex").join("session_index.jsonl");
        assert!(
            jsonl_has_line(&codex_index, |row| {
                row["id"] == serde_json::json!(codex_receipt.session_id)
                    && row.get("thread_name").is_some()
            }),
            "codex session_index.jsonl must list the forked session by id (with a thread_name)"
        );
    });
}

#[cfg(feature = "local_fs")]
#[test]
fn local_cli_agent_operation_reuses_one_store_root_snapshot_across_scan_read_mutate_and_fork() {
    use crate::cli_agent_jsonl::{
        AgentSessionDiscoveryPlan, AgentSessionDiscoveryProvider, CliAgentStoreRoots,
    };
    use crate::terminal::cli_agent_session_index::{
        current_app_cli_agent_session_source_target_from_id_with_roots,
        delete_current_app_cli_agent_session_with_roots,
        scan_current_app_cli_agent_sessions_with_plan,
    };
    use crate::terminal::CLIAgent;

    use super::adapter_registry::SessionBridgeForkTarget;

    let tempdir = tempfile::tempdir().unwrap();
    let home_dir = tempdir.path().join("home");
    let claude_config_dir = tempdir.path().join("operation-claude");
    let codex_home = tempdir.path().join("operation-codex");
    std::fs::create_dir_all(&home_dir).unwrap();
    let roots = CliAgentStoreRoots::from_explicit_target_paths(
        home_dir.clone(),
        claude_config_dir.clone(),
        codex_home,
    )
    .unwrap();

    let source = native_sample_session(&home_dir);
    let source_receipt =
        super::native_writer::execute_native_session_lifecycle_with_roots_for_test(
            &source,
            SessionBridgeForkTarget::Agent(CLIAgent::Claude),
            &roots,
        )
        .unwrap();

    let indexed = scan_current_app_cli_agent_sessions_with_plan(
        AgentSessionDiscoveryPlan::for_test(vec![AgentSessionDiscoveryProvider::Claude], 32),
        &roots,
    )
    .unwrap();
    let source_snapshot = indexed
        .into_iter()
        .find(|session| session.cli_agent_session_id.as_deref() == Some(&source_receipt.session_id))
        .expect("custom Claude root session must be scanned");
    let source_target = current_app_cli_agent_session_source_target_from_id_with_roots(
        &source_snapshot.id,
        source_snapshot.cli_agent.as_deref(),
        source_snapshot.cli_agent_session_id.clone(),
        &roots,
    )
    .unwrap()
    .expect("scanned source must resolve through the same roots snapshot");
    let read_result = super::cli_agent_reader::read_current_app_cli_agent_session_with_roots(
        source_target,
        &roots,
        None,
        None,
    )
    .unwrap();
    let fork = fork_session(&read_result.session, Some(uuid::Uuid::new_v4().to_string()));
    let fork_receipt = super::native_writer::execute_native_session_lifecycle_with_roots_for_test(
        &fork.session,
        SessionBridgeForkTarget::Agent(CLIAgent::Claude),
        &roots,
    )
    .unwrap();

    assert!(source_receipt.session_file.starts_with(&claude_config_dir));
    assert!(fork_receipt.session_file.starts_with(&claude_config_dir));
    assert!(!home_dir.join(".claude").exists());

    delete_current_app_cli_agent_session_with_roots(&source_snapshot.id, &roots).unwrap();
    assert!(!source_receipt.session_file.exists());
    assert!(fork_receipt.session_file.exists());
}

#[cfg(feature = "local_fs")]
#[test]
fn edit_fork_round_trips_through_native_history_without_mutating_source() {
    use crate::terminal::CLIAgent;

    use super::adapter_registry::SessionBridgeForkTarget;

    let tempdir = tempfile::tempdir().unwrap();
    with_isolated_native_history_env(tempdir.path(), || {
        let source = native_sample_session(tempdir.path());
        let edited = edit_session(
            &source,
            SessionEditSpec {
                redactions: vec!["hello".to_owned(), "secret".to_owned()],
                trim_after: Some(1),
            },
            Some(uuid::Uuid::new_v4().to_string()),
        )
        .unwrap();

        let receipt = super::native_writer::execute_native_session_lifecycle_to_home_for_test(
            &edited.session,
            SessionBridgeForkTarget::Agent(CLIAgent::Codex),
            tempdir.path(),
        )
        .unwrap();
        let read_result = read_native_receipt_back(CLIAgent::Codex, &receipt);

        assert_eq!(source.messages.len(), 2);
        assert_eq!(source.messages[0].text, "hello\n  indented");
        assert!(source.messages[1].text.contains("secret"));
        assert_eq!(read_result.session.title, "Test Session (edited)");
        assert_eq!(read_result.session.messages.len(), 1);
        assert_eq!(
            read_result.session.messages[0].text,
            "[REDACTED_BY_SESSION_BRIDGE]\n  indented"
        );
        assert!(!read_result.session.messages[0].text.contains("hello"));
        assert!(!read_result.session.messages[0].text.contains("secret"));
        assert_eq!(
            read_result.session.metadata["sessionBridge"]["providerSessionId"],
            receipt.session_id
        );
    });
}

#[cfg(feature = "local_fs")]
#[test]
fn session_bridge_native_write_requires_resolved_home() {
    use crate::terminal::CLIAgent;

    use super::adapter_registry::SessionBridgeForkTarget;

    let error = super::native_writer::execute_native_session_lifecycle_with_home_for_test(
        &sample_session(),
        SessionBridgeForkTarget::Agent(CLIAgent::Codex),
        None,
    )
    .expect_err("native write must fail before creating a provider store");

    assert!(error.to_string().contains("home directory"));
}

#[cfg(feature = "local_fs")]
#[test]
fn native_writer_rejects_unregistered_session_bridge_target() {
    use crate::terminal::CLIAgent;

    use super::adapter_registry::SessionBridgeForkTarget;

    let tempdir = tempfile::tempdir().unwrap();
    let error = super::native_writer::execute_native_session_lifecycle_to_home_for_test(
        &sample_session(),
        SessionBridgeForkTarget::Agent(CLIAgent::Pi),
        tempdir.path(),
    )
    .unwrap_err();

    assert!(error
        .to_string()
        .contains("has no registered SessionBridge adapter"));
    assert!(
        !tempdir
            .path()
            .join(".agents/session-bridge/backups")
            .exists(),
        "unsupported targets must fail before creating backup or fake native history state"
    );
}

#[cfg(feature = "local_fs")]
#[test]
fn remote_native_write_plan_targets_supplied_home_root_without_python() {
    use crate::terminal::CLIAgent;

    use super::adapter_registry::SessionBridgeForkTarget;
    use super::native_writer::NativeSessionWriteOperation;

    let session = sample_session();
    let claude_plan = super::native_writer::plan_native_session_write_for_home_root(
        &session,
        super::native_writer::NativeSessionTargetContext::Claude {
            provider_root: "/home/remote-user/.claude".to_owned(),
        },
        target_project_context("/home/remote-user/project"),
    )
    .unwrap();
    assert_eq!(
        claude_plan.receipt.target,
        SessionBridgeForkTarget::Agent(CLIAgent::Claude)
    );
    assert!(claude_plan
        .receipt
        .session_file
        .starts_with("/home/remote-user/.claude/projects/"));
    assert!(claude_plan.operations.iter().any(|operation| matches!(
        operation,
        NativeSessionWriteOperation::Append { path, .. }
            if path == "/home/remote-user/.claude/history.jsonl"
    )));

    let codex_context = super::native_writer::codex_native_target_context(
        r"C:\Users\remote-user\.codex".to_owned(),
        b"model_provider = \"remote-provider\"\n",
        r"C:\Users\remote-user\.codex\config.toml",
        b"codex-cli 9.8.7\n",
        "test Codex --version",
    )
    .unwrap();
    let codex_plan = super::native_writer::plan_native_session_write_for_home_root(
        &session,
        codex_context,
        target_project_context(r"C:\Users\remote-user\project"),
    )
    .unwrap();
    assert_eq!(
        codex_plan.receipt.target,
        SessionBridgeForkTarget::Agent(CLIAgent::Codex)
    );
    assert!(
        codex_plan
            .receipt
            .session_file
            .starts_with(r"C:\Users\remote-user\.codex\sessions\"),
        "remote plan must preserve Windows-style remote home roots"
    );
    assert!(codex_plan.operations.iter().any(|operation| matches!(
        operation,
        NativeSessionWriteOperation::Append { path, .. }
            if path == r"C:\Users\remote-user\.codex\session_index.jsonl"
    )));
}

#[cfg(all(feature = "local_fs", unix))]
#[test]
fn local_native_project_context_canonicalizes_symlink_path() {
    use std::os::unix::fs::symlink;

    use crate::terminal::CLIAgent;

    use super::adapter_registry::SessionBridgeForkTarget;

    let tempdir = tempfile::tempdir().unwrap();
    with_isolated_native_history_env(tempdir.path(), || {
        let canonical_project = tempdir.path().join("canonical-project");
        let lexical_project = tempdir.path().join("project-alias");
        std::fs::create_dir_all(&canonical_project).unwrap();
        symlink(&canonical_project, &lexical_project).unwrap();

        let canonical_project = canonical_project.canonicalize().unwrap();
        let mut session = sample_session();
        session.project_path = Some(lexical_project.display().to_string());
        let receipt = super::native_writer::execute_native_session_lifecycle_to_home_for_test(
            &session,
            SessionBridgeForkTarget::Agent(CLIAgent::Claude),
            tempdir.path(),
        )
        .unwrap();

        assert_eq!(
            receipt.project_path,
            canonical_project.display().to_string()
        );
        let rows = jsonl_values(&std::fs::read(&receipt.session_file).unwrap());
        for row in rows
            .iter()
            .filter(|row| matches!(row["type"].as_str(), Some("user" | "assistant")))
        {
            assert_eq!(row["cwd"], receipt.project_path);
        }
        let history =
            jsonl_values(&std::fs::read(tempdir.path().join(".claude/history.jsonl")).unwrap());
        assert_eq!(history.last().unwrap()["project"], receipt.project_path);
        assert!(!receipt
            .session_file
            .to_string_lossy()
            .contains("project-alias"));
    });
}

#[cfg(feature = "local_fs")]
#[test]
fn claude_native_plan_uses_target_canonical_project_identity() {
    use super::native_writer::NativeSessionWriteOperation;

    let mut session = sample_session();
    session.project_path = Some("/source/lexical/project-alias".to_owned());
    let target_project_path = "/canonical/target/project";
    let plan = super::native_writer::plan_native_session_write_for_home_root(
        &session,
        super::native_writer::NativeSessionTargetContext::Claude {
            provider_root: "/target/.claude".to_owned(),
        },
        target_project_context(target_project_path),
    )
    .unwrap();

    assert_eq!(plan.receipt.project_path, target_project_path);
    assert!(plan
        .receipt
        .session_file
        .starts_with("/target/.claude/projects/-canonical-target-project/"));

    let transcript = plan
        .operations
        .iter()
        .find_map(|operation| match operation {
            NativeSessionWriteOperation::Write { contents, .. } => Some(jsonl_values(contents)),
            NativeSessionWriteOperation::Append { .. } => None,
        })
        .unwrap();
    for row in transcript
        .iter()
        .filter(|row| matches!(row["type"].as_str(), Some("user" | "assistant")))
    {
        assert_eq!(row["cwd"], target_project_path);
    }

    let history = plan
        .operations
        .iter()
        .find_map(|operation| match operation {
            NativeSessionWriteOperation::Append { path, contents }
                if path == "/target/.claude/history.jsonl" =>
            {
                jsonl_values(contents).into_iter().next()
            }
            NativeSessionWriteOperation::Write { .. }
            | NativeSessionWriteOperation::Append { .. } => None,
        })
        .unwrap();
    assert_eq!(history["project"], target_project_path);

    let serialized_plan = format!("{plan:?}");
    assert!(!serialized_plan.contains("/source/lexical/project-alias"));
}

#[cfg(feature = "local_fs")]
#[test]
fn local_and_remote_native_write_plans_are_equivalent() {
    use crate::terminal::CLIAgent;

    use super::adapter_registry::SessionBridgeForkTarget;

    let tempdir = tempfile::tempdir().unwrap();
    with_isolated_native_history_env(tempdir.path(), || {
        let session = native_sample_session(tempdir.path());
        for target in [
            SessionBridgeForkTarget::Agent(CLIAgent::Codex),
            SessionBridgeForkTarget::Agent(CLIAgent::Claude),
        ] {
            let (local, remote) =
                super::native_writer::deterministic_local_and_remote_native_write_plans_for_test(
                    &session,
                    target,
                    tempdir.path(),
                )
                .unwrap();

            assert_eq!(local, remote, "{} plan drifted", target.display_label());
        }
    });
}

#[cfg(feature = "local_fs")]
#[test]
fn codex_native_writer_does_not_mutate_codex_private_sqlite_registry() {
    use crate::terminal::CLIAgent;

    use super::adapter_registry::SessionBridgeForkTarget;

    let tempdir = tempfile::tempdir().unwrap();
    with_isolated_native_history_env(tempdir.path(), || {
        let db_path = tempdir.path().join(".codex/state_5.sqlite");
        std::fs::write(&db_path, b"codex-owned-registry-sentinel").unwrap();
        let before = std::fs::read(&db_path).unwrap();

        let session = native_sample_session(tempdir.path());
        super::native_writer::execute_native_session_lifecycle_to_home_for_test(
            &session,
            SessionBridgeForkTarget::Agent(CLIAgent::Codex),
            tempdir.path(),
        )
        .unwrap();

        assert_eq!(std::fs::read(&db_path).unwrap(), before);
    });
}

#[cfg(feature = "local_fs")]
#[test]
fn import_bundle_writes_native_session_and_dry_run_stays_read_only() {
    use diesel::connection::SimpleConnection;
    use diesel::Connection;
    use diesel_migrations::MigrationHarness;
    use uuid::Uuid;

    let tempdir = tempfile::tempdir().unwrap();
    let database_path = tempdir.path().join("ashide.sqlite");
    let mut conn = diesel::SqliteConnection::establish(database_path.to_str().unwrap()).unwrap();
    conn.batch_execute("PRAGMA foreign_keys = ON;").unwrap();
    conn.run_pending_migrations(::persistence::MIGRATIONS)
        .unwrap();

    let mut session = sample_session();
    session.session_id = Uuid::new_v4().to_string();
    session.title = "Imported Session".to_owned();
    let bundle_path = write_bundle(&session, Some(tempdir.path())).unwrap();
    let bundle = read_bundle(&bundle_path).unwrap();

    let dry_run_plan =
        super::ashide_store::preview_ashide_session_import(&mut conn, &bundle, &bundle_path, None)
            .unwrap();
    assert_eq!(dry_run_plan.target_session_id, session.session_id);
    assert_eq!(dry_run_plan.project_path, Some("/tmp/project".to_owned()));
    assert!(super::ashide_store::list_ashide_sessions(&mut conn)
        .unwrap()
        .is_empty());

    let plan =
        super::ashide_store::import_ashide_session_bundle(&mut conn, &bundle, &bundle_path, None)
            .unwrap();

    assert_eq!(plan.source_session_id, session.session_id);
    assert_eq!(plan.target_session_id, session.session_id);
    assert_eq!(plan.message_count, 2);
    assert_eq!(plan.artifact_count, 1);
    assert_eq!(
        plan.source_reference,
        bundle_path.canonicalize().unwrap().display().to_string()
    );

    let read_result =
        super::ashide_store::read_ashide_session_by_id(&mut conn, &plan.target_session_id).unwrap();
    assert_eq!(read_result.session.title, "Imported Session");
    assert_eq!(
        read_result.session.project_path,
        Some("/tmp/project".to_owned())
    );
    assert_eq!(read_result.session.messages.len(), 2);
    assert_eq!(read_result.session.artifacts.len(), 1);
    assert_eq!(
        read_result.session.metadata["sessionBridgeImport"]["sourceSessionId"],
        session.session_id
    );
    assert_eq!(
        read_result.session.metadata["sessionBridgeImport"]["sourceReference"],
        bundle_path.canonicalize().unwrap().display().to_string()
    );

    let error =
        super::ashide_store::import_ashide_session_bundle(&mut conn, &bundle, &bundle_path, None)
            .unwrap_err();
    assert!(error.to_string().contains("conversation already exists"));
}

#[cfg(feature = "local_fs")]
#[test]
fn derivation_write_back_writes_native_session_and_preserves_original_source_provenance() {
    use diesel::connection::SimpleConnection;
    use diesel::Connection;
    use diesel_migrations::MigrationHarness;
    use uuid::Uuid;

    let tempdir = tempfile::tempdir().unwrap();
    let database_path = tempdir.path().join("ashide.sqlite");
    let mut conn = diesel::SqliteConnection::establish(database_path.to_str().unwrap()).unwrap();
    conn.batch_execute("PRAGMA foreign_keys = ON;").unwrap();
    conn.run_pending_migrations(::persistence::MIGRATIONS)
        .unwrap();

    let mut source_session = sample_session();
    source_session.session_id = Uuid::new_v4().to_string();
    source_session.title = "Source Session".to_owned();
    let target_session_id = Uuid::new_v4().to_string();
    let derivation = edit_session(
        &source_session,
        SessionEditSpec {
            redactions: vec!["secret".to_owned()],
            trim_after: Some(2),
        },
        Some(target_session_id.clone()),
    )
    .unwrap();
    let import_source = super::ashide_store::SessionBridgeImportSource::from_derived_session(
        &derivation.receipt.operation,
        &derivation.receipt.source_session_id,
        &derivation.receipt.derived_session_id,
        &derivation.session,
    )
    .unwrap();

    let dry_run_plan = super::ashide_store::preview_ashide_session_write_back(
        &mut conn,
        &derivation.session,
        import_source.clone(),
    )
    .unwrap();
    assert_eq!(dry_run_plan.source_session_id, source_session.session_id);
    assert_eq!(dry_run_plan.target_session_id, target_session_id);
    assert!(dry_run_plan
        .source_reference
        .starts_with("session-bridge://derived/edit/"));
    assert_eq!(dry_run_plan.source_sha256.len(), 64);
    assert!(super::ashide_store::list_ashide_sessions(&mut conn)
        .unwrap()
        .is_empty());

    let plan = super::ashide_store::import_ashide_session_write_back(
        &mut conn,
        &derivation.session,
        import_source,
    )
    .unwrap();
    assert_eq!(plan.source_session_id, source_session.session_id);
    assert_eq!(plan.target_session_id, target_session_id);

    let read_result =
        super::ashide_store::read_ashide_session_by_id(&mut conn, &target_session_id).unwrap();
    assert_eq!(read_result.session.title, "Source Session (edited)");
    assert!(read_result.session.messages[1]
        .text
        .contains("[REDACTED_BY_SESSION_BRIDGE]"));
    assert!(!read_result.session.messages[1].text.contains("secret"));
    assert_eq!(
        read_result.session.metadata["sessionBridge"]["operation"],
        "edit"
    );
    assert_eq!(
        read_result.session.metadata["sessionBridgeImport"]["sourceSessionId"],
        source_session.session_id
    );
    assert_eq!(
        read_result.session.metadata["sessionBridgeImport"]["sourceSha256"]
            .as_str()
            .unwrap()
            .len(),
        64
    );

    let duplicate_source = super::ashide_store::SessionBridgeImportSource::from_derived_session(
        &derivation.receipt.operation,
        &derivation.receipt.source_session_id,
        &derivation.receipt.derived_session_id,
        &derivation.session,
    )
    .unwrap();
    let error = super::ashide_store::import_ashide_session_write_back(
        &mut conn,
        &derivation.session,
        duplicate_source,
    )
    .unwrap_err();
    assert!(error.to_string().contains("conversation already exists"));
}

#[cfg(feature = "local_fs")]
#[test]
fn import_bundle_rejects_non_uuid_native_session_id() {
    use diesel::connection::SimpleConnection;
    use diesel::Connection;
    use diesel_migrations::MigrationHarness;

    let tempdir = tempfile::tempdir().unwrap();
    let database_path = tempdir.path().join("ashide.sqlite");
    let mut conn = diesel::SqliteConnection::establish(database_path.to_str().unwrap()).unwrap();
    conn.batch_execute("PRAGMA foreign_keys = ON;").unwrap();
    conn.run_pending_migrations(::persistence::MIGRATIONS)
        .unwrap();

    let mut session = sample_session();
    session.session_id = "not-a-native-uuid".to_owned();
    let bundle_path = write_bundle(&session, Some(tempdir.path())).unwrap();
    let bundle = read_bundle(&bundle_path).unwrap();

    let error =
        super::ashide_store::preview_ashide_session_import(&mut conn, &bundle, &bundle_path, None)
            .unwrap_err();
    assert!(error
        .to_string()
        .contains("native imported sessions require a UUID id"));

    let error = super::ashide_store::preview_ashide_session_import(
        &mut conn,
        &bundle,
        &bundle_path,
        Some("also-not-a-native-uuid".to_owned()),
    )
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("native imported sessions require a UUID id"));
}

#[test]
fn read_native_session_rejects_non_uuid_conversation_id() {
    use crate::persistence::model::{AgentConversation, AgentConversationRecord};

    let persisted = AgentConversation {
        conversation: AgentConversationRecord {
            conversation_id: "not-a-native-uuid".to_owned(),
            conversation_data: "{}".to_owned(),
            ..Default::default()
        },
        tasks: Vec::new(),
    };

    let error = super::ashide_store::agent_conversation_to_session_ir(persisted).unwrap_err();
    assert!(error
        .to_string()
        .contains("native sessions require a UUID id"));
}

#[test]
fn fork_session_creates_derived_id_and_preserves_parent_provenance() {
    let session = sample_session();
    let fork = fork_session(&session, Some("fork-session".to_owned()));

    assert_eq!(session.session_id, "session/id with spaces");
    assert_eq!(fork.session.session_id, "fork-session");
    assert_eq!(fork.session.messages, session.messages);
    assert_eq!(fork.receipt.operation, "fork");
    assert_eq!(fork.receipt.source_session_id, "session/id with spaces");
    assert_eq!(fork.receipt.derived_session_id, "fork-session");

    let metadata = fork
        .session
        .metadata
        .get("sessionBridge")
        .expect("derived metadata should include a sessionBridge receipt");
    assert_eq!(metadata["operation"], "fork");
    assert_eq!(metadata["sourceSessionId"], "session/id with spaces");
    assert_eq!(
        metadata["operationMetadata"]["forkedFromSessionId"],
        "session/id with spaces"
    );
}

#[test]
fn edit_session_redacts_and_trims_without_mutating_source() {
    let session = sample_session();
    let edited = edit_session(
        &session,
        SessionEditSpec {
            redactions: vec!["hello".to_owned(), "secret".to_owned()],
            trim_after: Some(1),
        },
        Some("edited-session".to_owned()),
    )
    .unwrap();

    assert_eq!(session.messages[0].text, "hello\n  indented");
    assert_eq!(edited.session.session_id, "edited-session");
    assert_eq!(edited.session.messages.len(), 1);
    assert_eq!(
        edited.session.messages[0].text,
        "[REDACTED_BY_SESSION_BRIDGE]\n  indented"
    );
    assert_eq!(edited.receipt.operation, "edit");
    assert_eq!(edited.receipt.trimmed_message_count, 1);
    assert_eq!(edited.receipt.redaction_replacement_count, 1);
    assert_eq!(
        edited.session.metadata["sessionBridge"]["operationMetadata"]["trimmedMessageCount"],
        1
    );
}

#[test]
fn edit_session_messages_replaces_message_draft_without_mutating_source() {
    let session = sample_session();
    let mut edited_messages = session.messages.clone();
    edited_messages.remove(1);
    edited_messages[0].text = "edited user prompt".to_owned();

    let edited = super::transform::edit_session_messages(
        &session,
        edited_messages,
        Some("edited-session".to_owned()),
    )
    .unwrap();

    assert_eq!(session.messages.len(), 2);
    assert_eq!(session.messages[0].text, "hello\n  indented");
    assert!(session.messages[1].text.contains("secret"));
    assert_eq!(edited.session.session_id, "edited-session");
    assert_eq!(edited.session.title, "Test Session (edited)");
    assert_eq!(edited.session.messages.len(), 1);
    assert_eq!(edited.session.messages[0].text, "edited user prompt");
    assert_eq!(
        edited.session.messages[0].timestamp,
        session.messages[0].timestamp
    );
    assert_eq!(edited.receipt.operation, "edit");
    assert_eq!(edited.receipt.original_message_count, 2);
    assert_eq!(edited.receipt.message_count, 1);
    assert_eq!(edited.receipt.trimmed_message_count, 1);
    assert_eq!(edited.receipt.redaction_replacement_count, 0);
    assert_eq!(
        edited.session.metadata["sessionBridge"]["operationMetadata"]["messageEditor"],
        true
    );
}

#[test]
fn edit_session_requires_a_real_operation() {
    let error = edit_session(
        &sample_session(),
        SessionEditSpec {
            redactions: vec!["".to_owned()],
            trim_after: None,
        },
        Some("edited-session".to_owned()),
    )
    .unwrap_err();

    assert!(error.to_string().contains("provide at least one --redact"));
}
