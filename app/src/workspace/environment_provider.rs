use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::{Duration, Instant};

use warpui::{ViewContext, ViewHandle};

use crate::app_state::{EnvironmentLifecycleState, EnvironmentSnapshot};
use crate::pane_group::PaneGroup;

pub(crate) mod source_saved_ssh;

const ENVIRONMENT_PROVIDER_PANE_SPLIT_RATIO: f32 = 0.56;
#[cfg(not(target_family = "wasm"))]
const CONTROL_MASTER_READY_TIMEOUT: Duration = Duration::from_secs(12);
#[cfg(not(target_family = "wasm"))]
const CONTROL_MASTER_READY_POLL_INTERVAL: Duration = Duration::from_millis(150);

pub(crate) type EnvironmentProviderManagerView = source_saved_ssh::ProviderManagerView;

pub(crate) fn new_provider_manager_view<V: warpui::View>(
    ctx: &mut ViewContext<V>,
) -> ViewHandle<EnvironmentProviderManagerView> {
    source_saved_ssh::new_provider_manager_view(ctx)
}

pub(crate) enum EnvironmentProviderManagerEvent {
    OpenEditor { connection_ref: String },
    OpenRuntimeTerminal { target: EnvironmentProviderTarget },
    OpenRuntime { target: EnvironmentProviderTarget },
    OpenFileBrowser { connection_ref: String },
    PersistenceError(String),
}

pub(crate) fn provider_manager_event(
    event: &source_saved_ssh::ProviderManagerEvent,
) -> EnvironmentProviderManagerEvent {
    source_saved_ssh::provider_manager_event(event)
}

#[derive(Debug, Clone)]
pub struct EnvironmentProviderTarget {
    payload: EnvironmentProviderTargetPayload,
}

#[derive(Debug, Clone)]
enum EnvironmentProviderTargetPayload {
    SavedConnection(source_saved_ssh::SavedConnectionTarget),
}

impl EnvironmentProviderTarget {
    fn from_saved_connection_target(target: source_saved_ssh::SavedConnectionTarget) -> Self {
        Self {
            payload: EnvironmentProviderTargetPayload::SavedConnection(target),
        }
    }

    pub fn dormant_environment(
        &self,
        active_workspace_root: Option<String>,
    ) -> EnvironmentSnapshot {
        match &self.payload {
            EnvironmentProviderTargetPayload::SavedConnection(target) => {
                target.dormant_environment(active_workspace_root)
            }
        }
    }

    pub fn startup_command(&self) -> Option<String> {
        match &self.payload {
            EnvironmentProviderTargetPayload::SavedConnection(target) => target.startup_command(),
        }
    }

    pub fn connection_ref(&self) -> &str {
        match &self.payload {
            EnvironmentProviderTargetPayload::SavedConnection(target) => target.connection_ref(),
        }
    }

    pub fn transport_descriptor(&self) -> EnvironmentTransportDescriptor {
        match &self.payload {
            EnvironmentProviderTargetPayload::SavedConnection(target) => {
                target.transport_descriptor()
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct EnvironmentTransportDescriptor {
    payload: EnvironmentTransportPayload,
}

#[derive(Debug, Clone)]
enum EnvironmentTransportPayload {
    SavedConnection(source_saved_ssh::SavedConnectionTransport),
}

impl EnvironmentTransportDescriptor {
    fn from_saved_connection_transport(
        transport: source_saved_ssh::SavedConnectionTransport,
    ) -> Self {
        Self {
            payload: EnvironmentTransportPayload::SavedConnection(transport),
        }
    }

    pub fn connection_ref(&self) -> &str {
        match &self.payload {
            EnvironmentTransportPayload::SavedConnection(transport) => transport.connection_ref(),
        }
    }

    pub fn host_label(&self) -> &str {
        match &self.payload {
            EnvironmentTransportPayload::SavedConnection(transport) => transport.host_label(),
        }
    }

    pub fn target(&self) -> String {
        match &self.payload {
            EnvironmentTransportPayload::SavedConnection(transport) => transport.target(),
        }
    }

    pub fn args(&self) -> Vec<String> {
        match &self.payload {
            EnvironmentTransportPayload::SavedConnection(transport) => transport.args(),
        }
    }

    pub fn runtime_snapshot(
        &self,
        connection_ref: String,
        active_workspace_root: Option<String>,
        lifecycle_state: EnvironmentLifecycleState,
    ) -> EnvironmentSnapshot {
        match &self.payload {
            EnvironmentTransportPayload::SavedConnection(transport) => {
                transport.runtime_snapshot(connection_ref, active_workspace_root, lifecycle_state)
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct EnvironmentProviderCandidate {
    pub(crate) alias: String,
    pub(crate) authority_key: String,
    pub(crate) title: String,
    pub(crate) detail: String,
    pub(crate) uses_key_auth: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct EnvironmentProviderSearchTarget {
    pub(crate) target: EnvironmentProviderTarget,
    pub(crate) title: String,
    pub(crate) detail: String,
    pub(crate) search_text: String,
}

pub(crate) fn load_saved_provider_search_targets(
    max_targets: usize,
) -> Result<Vec<EnvironmentProviderSearchTarget>, String> {
    source_saved_ssh::load_saved_provider_search_targets(max_targets)
}

#[derive(Debug, Clone)]
pub(crate) enum EnvironmentProviderCandidateLoadOutcome {
    Loaded(Vec<EnvironmentProviderCandidate>),
    NotFound,
    Error(String),
}

#[derive(Debug, Clone)]
pub(crate) struct EnvironmentProviderCandidateLoadResult {
    pub(crate) path: Option<String>,
    pub(crate) outcome: EnvironmentProviderCandidateLoadOutcome,
}

pub(crate) fn add_provider_editor_pane(
    pane_group: &mut PaneGroup,
    connection_ref: String,
    ctx: &mut ViewContext<PaneGroup>,
) {
    use crate::pane_group::pane::provider_connection_pane::ProviderConnectionPane;
    let pane = ProviderConnectionPane::new(connection_ref, ctx);
    let smart_split_direction =
        pane_group.smart_split_direction(ctx, ENVIRONMENT_PROVIDER_PANE_SPLIT_RATIO);
    pane_group.add_pane_with_direction(
        smart_split_direction,
        pane,
        true, /* focus_new_pane */
        ctx,
    );
}

pub(crate) fn add_provider_file_browser_pane(
    pane_group: &mut PaneGroup,
    connection_ref: String,
    ctx: &mut ViewContext<PaneGroup>,
) {
    use crate::pane_group::pane::provider_file_browser_pane::ProviderFileBrowserPane;
    let pane = ProviderFileBrowserPane::new(connection_ref, ctx);
    let smart_split_direction =
        pane_group.smart_split_direction(ctx, ENVIRONMENT_PROVIDER_PANE_SPLIT_RATIO);
    pane_group.add_pane_with_direction(
        smart_split_direction,
        pane,
        true, /* focus_new_pane */
        ctx,
    );
}

#[cfg(not(target_family = "wasm"))]
fn build_control_master_args(
    transport: &EnvironmentTransportDescriptor,
    socket_path: &Path,
) -> Result<Vec<String>, String> {
    let (mut args, target) = split_transport_args(transport)?;
    args.extend([
        "-f".to_owned(),
        "-N".to_owned(),
        "-M".to_owned(),
        "-S".to_owned(),
        socket_path.display().to_string(),
        "-o".to_owned(),
        "ControlMaster=yes".to_owned(),
        "-o".to_owned(),
        "ControlPersist=yes".to_owned(),
        "-o".to_owned(),
        "ExitOnForwardFailure=yes".to_owned(),
        "-o".to_owned(),
        "BatchMode=yes".to_owned(),
        "-o".to_owned(),
        "ConnectTimeout=8".to_owned(),
        "-o".to_owned(),
        "ConnectionAttempts=1".to_owned(),
        "-o".to_owned(),
        "StrictHostKeyChecking=accept-new".to_owned(),
        "-o".to_owned(),
        "LogLevel=ERROR".to_owned(),
        "-o".to_owned(),
        "ServerAliveInterval=30".to_owned(),
        "-o".to_owned(),
        "ServerAliveCountMax=6".to_owned(),
        "-o".to_owned(),
        "TCPKeepAlive=yes".to_owned(),
    ]);
    args.push(target);
    Ok(args)
}

#[cfg(not(target_family = "wasm"))]
fn build_control_master_operation_args(
    transport: &EnvironmentTransportDescriptor,
    socket_path: &Path,
    operation: &str,
) -> Result<Vec<String>, String> {
    let (mut args, target) = split_transport_args(transport)?;
    args.extend([
        "-S".to_owned(),
        socket_path.display().to_string(),
        "-O".to_owned(),
        operation.to_owned(),
        "-o".to_owned(),
        "BatchMode=yes".to_owned(),
        "-o".to_owned(),
        "ConnectTimeout=5".to_owned(),
        "-o".to_owned(),
        "ConnectionAttempts=1".to_owned(),
        "-o".to_owned(),
        "LogLevel=ERROR".to_owned(),
    ]);
    args.push(target);
    Ok(args)
}

#[cfg(not(target_family = "wasm"))]
fn split_transport_args(
    transport: &EnvironmentTransportDescriptor,
) -> Result<(Vec<String>, String), String> {
    let mut args = transport.args().into_iter().skip(1).collect::<Vec<_>>();
    let Some(target) = args.pop() else {
        return Err(format!(
            "environment transport {} has no ssh target",
            transport.connection_ref()
        ));
    };
    Ok((args, target))
}

#[cfg(not(target_family = "wasm"))]
async fn run_control_master_operation(
    transport: &EnvironmentTransportDescriptor,
    socket_path: &Path,
    operation: &str,
) -> Result<Output, String> {
    let args = build_control_master_operation_args(transport, socket_path, operation)?;
    command::r#async::Command::new("ssh")
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|error| format!("failed to run ssh control master {operation}: {error}"))
}

#[cfg(not(target_family = "wasm"))]
async fn wait_for_transport_control_master(
    transport: &EnvironmentTransportDescriptor,
    socket_path: &Path,
) -> Result<(), String> {
    let started = Instant::now();
    let mut last_error = None;

    while started.elapsed() < CONTROL_MASTER_READY_TIMEOUT {
        match run_control_master_operation(transport, socket_path, "check").await {
            Ok(output) if output.status.success() => return Ok(()),
            Ok(output) => last_error = Some(format_control_master_failure("check", &output)),
            Err(error) => last_error = Some(error),
        }
        warpui::r#async::Timer::after(CONTROL_MASTER_READY_POLL_INTERVAL).await;
    }

    let detail = last_error.unwrap_or_else(|| "ssh control master check did not run".to_owned());
    Err(format!(
        "environment transport control master was not ready within {}s: {detail}",
        CONTROL_MASTER_READY_TIMEOUT.as_secs()
    ))
}

#[cfg(not(target_family = "wasm"))]
fn format_control_master_failure(operation: &str, output: &Output) -> String {
    let stderr = trimmed_output(&output.stderr);
    let stdout = trimmed_output(&output.stdout);
    let detail = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        "no ssh output".to_owned()
    };
    format!(
        "ssh control master {operation} failed with status {}: {detail}",
        output.status
    )
}

#[cfg(not(target_family = "wasm"))]
fn trimmed_output(bytes: &[u8]) -> String {
    const MAX_OUTPUT_LEN: usize = 600;
    let text = String::from_utf8_lossy(bytes).trim().to_owned();
    if text.len() <= MAX_OUTPUT_LEN {
        return text;
    }
    let mut truncated = text.chars().take(MAX_OUTPUT_LEN).collect::<String>();
    truncated.push('…');
    truncated
}

#[cfg(not(target_family = "wasm"))]
fn prepare_control_master_socket_dir(socket_path: &Path) -> Result<(), String> {
    let Some(socket_dir) = socket_path.parent() else {
        return Ok(());
    };
    std::fs::create_dir_all(socket_dir).map_err(|error| {
        format!(
            "failed to create environment transport control master socket directory {}: {error}",
            socket_dir.display()
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        std::fs::set_permissions(socket_dir, std::fs::Permissions::from_mode(0o700)).map_err(
            |error| {
                format!(
                    "failed to lock down environment transport control master socket directory {}: {error}",
                    socket_dir.display()
                )
            },
        )?;
    }
    Ok(())
}

#[cfg(not(target_family = "wasm"))]
pub(crate) async fn spawn_transport_control_master(
    transport: EnvironmentTransportDescriptor,
    socket_path: PathBuf,
) -> Result<PathBuf, String> {
    prepare_control_master_socket_dir(&socket_path)?;
    if socket_path.exists() {
        let _ = run_control_master_operation(&transport, &socket_path, "exit").await;
    }
    let _ = std::fs::remove_file(&socket_path);
    let args = build_control_master_args(&transport, &socket_path)?;
    let output = command::r#async::Command::new("ssh")
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|error| {
            format!("failed to start environment transport control master: {error}")
        })?;
    if !output.status.success() {
        let _ = std::fs::remove_file(&socket_path);
        return Err(format_control_master_failure("start", &output));
    }
    if let Err(error) = wait_for_transport_control_master(&transport, &socket_path).await {
        let _ = run_control_master_operation(&transport, &socket_path, "exit").await;
        let _ = std::fs::remove_file(&socket_path);
        return Err(error);
    }
    Ok(socket_path)
}

#[cfg(all(test, not(target_family = "wasm")))]
mod tests {
    use super::*;

    fn test_transport() -> EnvironmentTransportDescriptor {
        source_saved_ssh::test_transport_descriptor()
    }

    fn has_adjacent(args: &[String], first: &str, second: &str) -> bool {
        args.windows(2)
            .any(|window| window[0] == first && window[1] == second)
    }

    #[test]
    fn test_control_master_start_is_noninteractive_and_bounded() {
        let transport = test_transport();
        let args = build_control_master_args(&transport, Path::new("/tmp/ashide-test.sock"))
            .expect("test transport should produce ssh control master args");

        assert_eq!(
            args.last().map(String::as_str),
            Some("root@example.internal")
        );
        assert!(has_adjacent(&args, "-S", "/tmp/ashide-test.sock"));
        assert!(has_adjacent(&args, "-o", "ControlMaster=yes"));
        assert!(has_adjacent(&args, "-o", "BatchMode=yes"));
        assert!(has_adjacent(&args, "-o", "ConnectTimeout=8"));
        assert!(has_adjacent(&args, "-o", "ConnectionAttempts=1"));
        assert!(has_adjacent(
            &args,
            "-o",
            "StrictHostKeyChecking=accept-new"
        ));
        assert!(has_adjacent(&args, "-o", "LogLevel=ERROR"));
    }

    #[test]
    fn test_control_master_check_uses_existing_socket_only() {
        let transport = test_transport();
        let args = build_control_master_operation_args(
            &transport,
            Path::new("/tmp/ashide-test.sock"),
            "check",
        )
        .expect("test transport should produce ssh control master check args");

        assert_eq!(
            args.last().map(String::as_str),
            Some("root@example.internal")
        );
        assert!(has_adjacent(&args, "-S", "/tmp/ashide-test.sock"));
        assert!(has_adjacent(&args, "-O", "check"));
        assert!(has_adjacent(&args, "-o", "BatchMode=yes"));
        assert!(has_adjacent(&args, "-o", "ConnectTimeout=5"));
        assert!(has_adjacent(&args, "-o", "ConnectionAttempts=1"));
        assert!(has_adjacent(&args, "-o", "LogLevel=ERROR"));
    }
}

#[cfg(target_family = "wasm")]
pub(crate) fn unsupported_transport_message() -> String {
    "Environment transports are not supported on wasm".to_owned()
}

pub(crate) fn runtime_connection_ref_from_authority(authority: &str) -> Option<String> {
    source_saved_ssh::runtime_connection_ref_from_authority(authority)
}

pub(crate) fn runtime_transport_descriptor_for_connection_ref(
    connection_ref: &str,
) -> Option<EnvironmentTransportDescriptor> {
    source_saved_ssh::runtime_transport_descriptor_for_connection_ref(connection_ref)
}

pub(crate) fn describe_runtime_transport_descriptor_lookup_failure(connection_ref: &str) -> String {
    source_saved_ssh::describe_runtime_transport_descriptor_lookup_failure(connection_ref)
}

pub(crate) fn target_for_provider_candidate(alias: &str) -> Option<EnvironmentProviderTarget> {
    source_saved_ssh::target_for_provider_candidate(alias)
}

pub(crate) fn load_provider_candidates() -> EnvironmentProviderCandidateLoadResult {
    source_saved_ssh::load_provider_candidates()
}
