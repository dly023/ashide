//! `manager.rs` 的纯函数级单元测试。
//!
//! 这里只覆盖纯函数 helper —— 不触碰 `RemoteServerManager` 本体,
//! 因为后者依赖 `warpui::Entity` / `ModelContext`,要起一整套 App
//! 上下文,放到 integration testing 框架更合适。

use super::*;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use warpui::r#async::executor;

// ---------------------------------------------------------------------------
// version_is_compatible
// ---------------------------------------------------------------------------

#[test]
fn version_compat_both_tagged_and_equal() {
    assert!(version_is_compatible(
        Some("v0.2026.05.10.stable"),
        "v0.2026.05.10.stable",
    ));
}

#[test]
fn version_compat_both_tagged_and_different() {
    assert!(!version_is_compatible(
        Some("v0.2026.05.10.stable"),
        "v0.2026.05.10.preview",
    ));
}

#[test]
fn version_compat_both_untagged() {
    // 客户端没有 GIT_RELEASE_TAG(cargo run),服务器也报空串
    // (`script/deploy_remote_server` dev 部署):视为兼容,保留
    // 本地开发循环不受影响。
    assert!(version_is_compatible(None, ""));
}

#[test]
fn version_compat_client_tagged_server_untagged() {
    // 客户端是 release,服务器是 dev 部署 → 视为不兼容,正常
    // 触发 reinstall 流程。
    assert!(!version_is_compatible(Some("v0.2026.05.10.stable"), ""));
}

#[test]
fn version_compat_client_untagged_server_tagged() {
    // **关键场景**:Ashide 客户端无 tag(cargo build),
    // helper 来自 GitHub release(带 tag)。若仍做严格版本比对,
    // 会触发 `remove_remote_server_binary` → 死循环。
    // 这个 test 仅记录 `version_is_compatible` 自身的行为不变,
    // 真正"跳过校验"由 [`should_enforce_remote_version_check`] 负责。
    assert!(!version_is_compatible(None, "v0.2026.05.10.stable"));
}

#[test]
fn helper_compatibility_failure_selects_update_remediation() {
    let expected = crate::REMOTE_SERVER_PROTOCOL_REVISION;
    let protocol = ConnectAndHandshakeError::Initialize(
        crate::client::ClientError::ProtocolRevisionMismatch {
            expected,
            received: expected.saturating_sub(1),
        },
    );
    assert_eq!(
        protocol.remediation(),
        RemoteServerFailureRemediation::UpdateHelper
    );

    let release = ConnectAndHandshakeError::HelperVersionMismatch {
        client_version: Some("client".to_owned()),
        server_version: "helper".to_owned(),
    };
    assert_eq!(
        release.remediation(),
        RemoteServerFailureRemediation::UpdateHelper
    );

    let disconnected =
        ConnectAndHandshakeError::Initialize(crate::client::ClientError::Disconnected);
    assert_eq!(
        disconnected.remediation(),
        RemoteServerFailureRemediation::Reconnect
    );
}

// ---------------------------------------------------------------------------
// should_enforce_remote_version_check
// ---------------------------------------------------------------------------

#[test]
fn enforce_version_check_skipped_on_oss() {
    // `Channel::Oss`(Ashide) 下 client 没有 `GIT_RELEASE_TAG`,与 helper
    // release version 字符串可能不一致,故跳过严格校验。
    assert!(!should_enforce_remote_version_check(Channel::Oss));
}

#[test]
fn enforce_version_check_kept_on_official_channels() {
    // release channel 上 client 与 helper 要么都来自同一次 release CI,
    // 要么都来自 `script/deploy_remote_server` 的本地部署,严格
    // 校验仍然必要 —— 保留原有 stale binary 自愈路径。
    for channel in [
        Channel::Stable,
        Channel::Preview,
        Channel::Dev,
        Channel::Local,
        Channel::Integration,
    ] {
        assert!(
            should_enforce_remote_version_check(channel),
            "channel {channel:?} should still enforce version check"
        );
    }
}

#[test]
fn terminal_session_alias_matches_environment_runtime_navigation_binding() {
    let runtime_session_id = SessionId::from(u64::MAX);
    let terminal_session_id = SessionId::from(42);
    let peer_session_id = SessionId::from(u64::MAX - 1);
    let mut registry = SessionExecutionRegistry::default();
    registry.register(
        terminal_session_id,
        runtime_session_id,
        test_bootstrap_info("/home/terminal", "/workspace/terminal"),
    );

    assert!(registry.sessions_share_transport_binding(terminal_session_id, runtime_session_id,));
    assert!(!registry.sessions_share_transport_binding(terminal_session_id, peer_session_id,));
}

#[tokio::test]
async fn reconnect_replays_session_execution_context() {
    let session_id = SessionId::from(91);
    let environment_variables = HashMap::from([
        (
            "ASHIDE_SESSION_EXECUTION_CONTEXT".to_owned(),
            "1".to_owned(),
        ),
        ("HOME".to_owned(), "/home/reconnected".to_owned()),
        ("PATH".to_owned(), "/reconnected/bin".to_owned()),
        ("CODEX_HOME".to_owned(), "/reconnected/codex".to_owned()),
        (
            "CLAUDE_CONFIG_DIR".to_owned(),
            "/reconnected/claude".to_owned(),
        ),
    ]);
    let info = SessionBootstrapInfo::from_context(&SessionExecutionContextInput {
        shell_type: "fish",
        shell_path: Some("/usr/bin/fish"),
        working_directory: Some("/workspace/reconnected"),
        environment_variables: &environment_variables,
    })
    .unwrap();
    let mut registry = SessionExecutionRegistry::default();
    registry.register(session_id, session_id, info);

    let (client_stream, server_stream) = tokio::io::duplex(4096);
    let (server_read, _server_write) = tokio::io::split(server_stream);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let executor = executor::Background::default();
    let (client, _event_rx) =
        RemoteServerClient::new(client_read.compat(), client_write.compat_write(), &executor);

    registry.replay_session(session_id, Some(&client));

    let mut server_read = server_read.compat();
    let message = crate::protocol::read_client_message(&mut server_read)
        .await
        .expect("reconnect must replay the recorded SessionBootstrapped snapshot");
    let Some(crate::proto::client_message::Message::SessionBootstrapped(notification)) =
        message.message
    else {
        panic!("expected replayed SessionBootstrapped notification");
    };
    assert_eq!(notification.session_id, session_id.as_u64());
    assert_eq!(notification.shell_type, "fish");
    assert_eq!(notification.shell_path.as_deref(), Some("/usr/bin/fish"));
    assert_eq!(
        notification.working_directory.as_deref(),
        Some("/workspace/reconnected")
    );
    assert_eq!(notification.environment_variables, environment_variables);
}

fn test_bootstrap_info(home: &str, cwd: &str) -> SessionBootstrapInfo {
    SessionBootstrapInfo::from_context(&SessionExecutionContextInput {
        shell_type: "bash",
        shell_path: Some("/bin/bash"),
        working_directory: Some(cwd),
        environment_variables: &HashMap::from([
            (
                "ASHIDE_SESSION_EXECUTION_CONTEXT".to_owned(),
                "1".to_owned(),
            ),
            ("HOME".to_owned(), home.to_owned()),
            ("PATH".to_owned(), format!("{home}/bin")),
        ]),
    })
    .expect("test execution context must be complete")
}

#[test]
fn environment_runtime_control_session_rejects_missing_execution_context() {
    assert!(
        SessionBootstrapInfo::from_context(&SessionExecutionContextInput {
            shell_type: "bash",
            shell_path: None,
            working_directory: None,
            environment_variables: &HashMap::new(),
        })
        .is_none(),
        "a connected synthetic owner without a terminal bootstrap must not fabricate an executor"
    );
}

#[test]
fn two_terminal_sessions_do_not_overwrite_runtime_owner_execution_context() {
    let owner = SessionId::from(900);
    let terminal_a = SessionId::from(901);
    let terminal_b = SessionId::from(902);
    let first = test_bootstrap_info("/home/first", "/workspace/first");
    let second = test_bootstrap_info("/home/second", "/workspace/second");
    let mut registry = SessionExecutionRegistry::default();

    let first_registration = registry.register(terminal_a, owner, first.clone());
    assert_eq!(first_registration.owner_context_established, Some(owner));
    let second_registration = registry.register(terminal_b, owner, second.clone());
    assert_eq!(second_registration.owner_context_established, None);
    assert_eq!(registry.bootstrap_info.get(&owner), Some(&first));
    assert_eq!(registry.bootstrap_info.get(&terminal_a), Some(&first));
    assert_eq!(registry.bootstrap_info.get(&terminal_b), Some(&second));
    assert_eq!(registry.canonical_session_id(terminal_a), owner);
    assert_eq!(registry.canonical_session_id(terminal_b), owner);
}

#[test]
fn reconnect_replays_all_alias_session_execution_contexts() {
    let owner = SessionId::from(910);
    let terminal_a = SessionId::from(911);
    let terminal_b = SessionId::from(912);
    let unrelated = SessionId::from(913);
    let mut registry = SessionExecutionRegistry::default();
    registry.register(
        terminal_a,
        owner,
        test_bootstrap_info("/home/a", "/workspace/a"),
    );
    registry.register(
        terminal_b,
        owner,
        test_bootstrap_info("/home/b", "/workspace/b"),
    );
    registry.register(
        unrelated,
        unrelated,
        test_bootstrap_info("/home/unrelated", "/workspace/unrelated"),
    );

    assert_eq!(
        registry.session_ids_for_transport(owner),
        vec![owner, terminal_a, terminal_b]
    );
}

#[tokio::test]
async fn reconnect_wire_replays_owner_and_alias_execution_contexts() {
    let owner = SessionId::from(930);
    let terminal_a = SessionId::from(931);
    let terminal_b = SessionId::from(932);
    let unrelated = SessionId::from(933);
    let mut registry = SessionExecutionRegistry::default();
    registry.register(
        terminal_a,
        owner,
        test_bootstrap_info("/home/a", "/workspace/a"),
    );
    registry.register(
        terminal_b,
        owner,
        test_bootstrap_info("/home/b", "/workspace/b"),
    );
    registry.register(
        unrelated,
        unrelated,
        test_bootstrap_info("/home/unrelated", "/workspace/unrelated"),
    );

    let (client_stream, server_stream) = tokio::io::duplex(8192);
    let (server_read, _server_write) = tokio::io::split(server_stream);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let executor = executor::Background::default();
    let (client, _event_rx) =
        RemoteServerClient::new(client_read.compat(), client_write.compat_write(), &executor);
    registry.replay_transport(owner, Some(&client));

    let mut server_read = server_read.compat();
    let mut notifications = HashMap::new();
    for _ in 0..3 {
        let message = crate::protocol::read_client_message(&mut server_read)
            .await
            .expect("transport replay must send every live execution context");
        let Some(crate::proto::client_message::Message::SessionBootstrapped(notification)) =
            message.message
        else {
            panic!("expected SessionBootstrapped replay");
        };
        notifications.insert(SessionId::from(notification.session_id), notification);
    }

    assert_eq!(
        notifications.keys().copied().collect::<HashSet<_>>(),
        HashSet::from([owner, terminal_a, terminal_b])
    );
    assert_eq!(
        notifications[&owner].working_directory.as_deref(),
        Some("/workspace/a")
    );
    assert_eq!(
        notifications[&terminal_a].working_directory.as_deref(),
        Some("/workspace/a")
    );
    assert_eq!(
        notifications[&terminal_b].working_directory.as_deref(),
        Some("/workspace/b")
    );
}

#[test]
fn deregistered_alias_context_is_not_replayed() {
    let owner = SessionId::from(920);
    let terminal = SessionId::from(921);
    let mut registry = SessionExecutionRegistry::default();
    registry.register(
        terminal,
        owner,
        test_bootstrap_info("/home/terminal", "/workspace/terminal"),
    );

    assert!(matches!(
        registry.remove(terminal),
        RemovedSessionExecution::Alias {
            transport_session_id
        } if transport_session_id == owner
    ));
    assert_eq!(registry.session_ids_for_transport(owner), vec![owner]);
    assert!(!registry.has_execution_context(terminal));
}

#[test]
fn owner_teardown_removes_alias_context_records() {
    let owner = SessionId::from(940);
    let terminal_a = SessionId::from(941);
    let terminal_b = SessionId::from(942);
    let mut registry = SessionExecutionRegistry::default();
    registry.register(
        terminal_a,
        owner,
        test_bootstrap_info("/home/a", "/workspace/a"),
    );
    registry.register(
        terminal_b,
        owner,
        test_bootstrap_info("/home/b", "/workspace/b"),
    );

    let RemovedSessionExecution::Owner { alias_session_ids } = registry.remove(owner) else {
        panic!("owner teardown must remove the transport registry");
    };
    assert_eq!(
        alias_session_ids.into_iter().collect::<HashSet<_>>(),
        HashSet::from([terminal_a, terminal_b])
    );
    assert!(registry.session_ids_for_transport(owner).is_empty());
    assert!(!registry.session_id_is_in_use(owner));
    assert!(!registry.session_id_is_in_use(terminal_a));
    assert!(!registry.session_id_is_in_use(terminal_b));
}

#[test]
fn reconnect_preserves_owner_alias_execution_contexts() {
    const MANAGER_RS: &str = include_str!("manager.rs");
    let reconnect = MANAGER_RS
        .split_once("pub fn restart_session_transport(")
        .expect("RemoteServerManager must expose one reconnect transport boundary")
        .1
        .split_once("pub fn ")
        .expect("restart_session_transport must end before the next public function")
        .0;

    assert!(
        !reconnect.contains("execution_registry.remove")
            && !reconnect.contains("deregister_session"),
        "transport restart must preserve the canonical owner and every live alias execution context"
    );
}

#[test]
fn restart_transport_disconnect_event_has_typed_cause() {
    const MANAGER_RS: &str = include_str!("manager.rs");
    let restart = MANAGER_RS
        .split_once("pub fn restart_session_transport(")
        .expect("RemoteServerManager must expose one transport restart boundary")
        .1
        .split_once("pub fn deregister_session")
        .expect("transport restart must end before explicit deregistration")
        .0;

    assert!(
        restart.contains("RemoteSessionDisconnectCause::ExplicitTransportRestart"),
        "explicit transport restart must identify its disconnect event so Workspace cannot recursively create another reconnect intent"
    );
}
