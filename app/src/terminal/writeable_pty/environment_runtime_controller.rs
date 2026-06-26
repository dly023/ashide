use settings::Setting;
use smol_str::SmolStr;
use std::path::PathBuf;
use std::sync::Arc;
use warp_core::SessionId;
use warpui::{Entity, ModelContext, ModelHandle, SingletonEntity, WeakModelHandle};

use crate::terminal::warpify::settings::SshExtensionInstallMode;
use crate::workspace::view::ASHIDE_REMOTE_SERVER_AUTO_INSTALL_ENV;

use crate::terminal::model::session::{IsLegacySSHSession, SessionInfo};
use crate::terminal::model_events::{ModelEvent, ModelEventDispatcher};
use crate::terminal::warpify::settings::WarpifySettings;
use crate::workspace::environment_runtime::{
    self, EnvironmentRuntimeAuthContext, EnvironmentRuntimeSetupEvent, EnvironmentRuntimeTransport,
};
use crate::workspace::environment_runtime::{
    EnvironmentRuntimePreinstallCheckResult, EnvironmentRuntimePreinstallStatus,
};

use super::pty_controller::{EventLoopSender, PtyController};

/// Per-SSH-init state machine. Encoding the state as an enum makes invalid
/// transitions unrepresentable and ensures the `SessionInfo` stash cannot be
/// accessed after it has been consumed.
enum SshInitState {
    Idle,
    /// Stash held, `check_binary` in flight.
    AwaitingCheck {
        session_info: SessionInfo,
        transport: EnvironmentRuntimeTransport,
    },
    /// Stash held, choice block showing.
    AwaitingUserChoice {
        session_info: SessionInfo,
        transport: EnvironmentRuntimeTransport,
    },
    /// Stash held, `install_binary` in flight.
    /// `for_update` is `true` when reinstalling over an existing install
    /// (auto-update path) and `false` for a fresh install.
    AwaitingInstall {
        session_id: SessionId,
        session_info: SessionInfo,
        transport: EnvironmentRuntimeTransport,
        #[allow(dead_code)]
        for_update: bool,
    },
    /// Stash held, `connect_session` in flight. Bootstrap is flushed only
    /// once `SessionConnected` arrives (or on connection failure).
    AwaitingConnect {
        session_id: SessionId,
        session_info: SessionInfo,
    },
}

/// Per-pane orchestrator that defers the bootstrap script write for SSH sessions,
/// checks for the environment runtime helper, and presents a two-option choice block when the binary is missing.
///
/// Uses a [`WeakModelHandle`] back to [`PtyController`] to avoid preventing
/// `PtyController` from being deallocated.
pub struct EnvironmentRuntimeController<T: EventLoopSender> {
    pty_controller: WeakModelHandle<PtyController<T>>,
    model_event_dispatcher: ModelHandle<ModelEventDispatcher>,
    auth_context: Arc<EnvironmentRuntimeAuthContext>,
    state: SshInitState,
}

impl<T: EventLoopSender> Entity for EnvironmentRuntimeController<T> {
    type Event = ();
}

impl<T: EventLoopSender> EnvironmentRuntimeController<T> {
    pub fn new(
        pty_controller: WeakModelHandle<PtyController<T>>,
        model_event_dispatcher: ModelHandle<ModelEventDispatcher>,
        ctx: &mut ModelContext<Self>,
    ) -> Self {
        let auth_context = environment_runtime::auth_context(ctx);
        ctx.subscribe_to_model(&model_event_dispatcher, |me, event, ctx| {
            if let ModelEvent::SshInitShell {
                pending_session_info,
            } = event
            {
                me.on_ssh_init_shell_requested(pending_session_info.as_ref().clone(), ctx);
            }
        });

        environment_runtime::subscribe_to_setup_events(ctx, |me, event, ctx| match event {
            EnvironmentRuntimeSetupEvent::BinaryCheckComplete {
                session_id,
                result,
                remote_platform: _,
                preinstall_check,
                has_old_binary,
            } => {
                me.on_binary_check_complete(
                    session_id,
                    result,
                    preinstall_check,
                    has_old_binary,
                    ctx,
                );
            }
            EnvironmentRuntimeSetupEvent::BinaryInstallComplete { session_id, result } => {
                me.on_binary_install_complete(session_id, result, ctx);
            }
            EnvironmentRuntimeSetupEvent::Connected { session_id } => {
                me.on_session_connected(session_id, ctx);
            }
            EnvironmentRuntimeSetupEvent::ConnectionFailed { session_id } => {
                me.on_session_connection_failed(session_id, ctx);
            }
        });

        Self {
            pty_controller,
            model_event_dispatcher,
            auth_context,
            state: SshInitState::Idle,
        }
    }

    /// Extracts the `SessionInfo` from the stash and writes the bootstrap
    /// script to the PTY via `PtyController::initialize_shell`.
    fn flush_stashed_bootstrap(&mut self, session_info: SessionInfo, ctx: &mut ModelContext<Self>) {
        if let Some(pty) = self.pty_controller.upgrade(ctx) {
            pty.update(ctx, |pty, ctx| {
                pty.initialize_shell(&session_info, ctx);
            });
        } else {
            log::warn!("PtyController dropped before bootstrap could be flushed");
        }
    }

    /// Idle -> AwaitingCheck
    fn on_ssh_init_shell_requested(&mut self, info: SessionInfo, ctx: &mut ModelContext<Self>) {
        let IsLegacySSHSession::Yes { socket_path } = &info.is_legacy_ssh_session else {
            return;
        };
        let session_id = info.session_id;
        let socket_path = socket_path.clone();
        debug_assert!(matches!(self.state, SshInitState::Idle));
        match std::mem::replace(&mut self.state, SshInitState::Idle) {
            SshInitState::Idle => {}
            SshInitState::AwaitingCheck {
                session_info: old_info,
                ..
            }
            | SshInitState::AwaitingUserChoice {
                session_info: old_info,
                ..
            }
            | SshInitState::AwaitingInstall {
                session_info: old_info,
                ..
            }
            | SshInitState::AwaitingConnect {
                session_info: old_info,
                ..
            } => {
                self.flush_stashed_bootstrap(old_info, ctx);
            }
        }
        let transport =
            environment_runtime::transport_for_control_path(socket_path, self.auth_context.clone());
        self.state = SshInitState::AwaitingCheck {
            session_info: info,
            transport: transport.clone(),
        };
        environment_runtime::check_session_binary(session_id, transport, ctx);
    }

    fn on_binary_check_complete(
        &mut self,
        session_id: SessionId,
        result: Result<bool, String>,
        preinstall_check: Option<EnvironmentRuntimePreinstallCheckResult>,
        has_old_binary: bool,
        ctx: &mut ModelContext<Self>,
    ) {
        let SshInitState::AwaitingCheck {
            ref session_info, ..
        } = self.state
        else {
            return;
        };
        if session_info.session_id != session_id {
            return;
        }

        let SshInitState::AwaitingCheck {
            session_info,
            transport,
        } = std::mem::replace(&mut self.state, SshInitState::Idle)
        else {
            unreachable!("just matched AwaitingCheck above");
        };

        // Preinstall gate. Runs **before** any user-visible install
        // affordance: if the script positively classified the host as
        // unsupported, stop setup and surface the unsupported state.
        let unsupported = preinstall_check
            .as_ref()
            .and_then(|check| match &check.status {
                EnvironmentRuntimePreinstallStatus::Unsupported { reason } => {
                    Some((check, reason.clone()))
                }
                EnvironmentRuntimePreinstallStatus::Supported
                | EnvironmentRuntimePreinstallStatus::Unknown => None,
            });
        if let Some((check, reason)) = unsupported {
            log::info!(
                "Preinstall check classified {session_id:?} as unsupported ({:?})",
                check.status
            );
            environment_runtime::mark_session_setup_unsupported(session_id, reason, ctx);
            return;
        }

        match result {
            Ok(true) => {
                let socket_path = transport.socket_path().clone();
                self.state = SshInitState::AwaitingConnect {
                    session_id,
                    session_info,
                };
                self.connect_session_for_current_identity(session_id, socket_path, ctx);
            }
            Ok(false) if has_old_binary => {
                // Auto-update: a prior install exists, so skip the modal
                // and reinstall.
                self.state = SshInitState::AwaitingInstall {
                    session_id,
                    session_info,
                    transport: transport.clone(),
                    for_update: true,
                };
                environment_runtime::install_session_binary(session_id, transport, true, ctx);
            }
            Ok(false) => {
                let install_mode = *WarpifySettings::as_ref(ctx)
                    .ssh_extension_install_mode
                    .value();
                let explicit_connect_auto_install = session_info
                    .environment_variable_names
                    .contains(&SmolStr::new(ASHIDE_REMOTE_SERVER_AUTO_INSTALL_ENV));
                match install_mode {
                    SshExtensionInstallMode::AlwaysAsk if !explicit_connect_auto_install => {
                        self.state = SshInitState::AwaitingUserChoice {
                            session_info,
                            transport,
                        };
                        self.model_event_dispatcher.update(ctx, |d, ctx| {
                            d.request_environment_runtime_setup_choice(session_id, ctx);
                        });
                    }
                    SshExtensionInstallMode::AlwaysAsk | SshExtensionInstallMode::AlwaysInstall => {
                        self.state = SshInitState::AwaitingInstall {
                            session_id,
                            session_info,
                            transport: transport.clone(),
                            for_update: false,
                        };
                        environment_runtime::install_session_binary(
                            session_id, transport, false, ctx,
                        );
                    }
                    SshExtensionInstallMode::NeverInstall => {
                        self.flush_stashed_bootstrap(session_info, ctx);
                    }
                }
            }
            Err(err) => {
                log::error!("Binary check failed for {session_id:?}: {err}");
                self.flush_stashed_bootstrap(session_info, ctx);
            }
        }
    }

    pub fn handle_environment_runtime_install(
        &mut self,
        session_id: SessionId,
        ctx: &mut ModelContext<Self>,
    ) {
        let SshInitState::AwaitingUserChoice { .. } = self.state else {
            log::warn!("Install clicked but state is not AwaitingUserChoice for {session_id:?}");
            return;
        };

        let SshInitState::AwaitingUserChoice {
            session_info,
            transport,
        } = std::mem::replace(&mut self.state, SshInitState::Idle)
        else {
            unreachable!("just matched AwaitingUserChoice above");
        };

        // Reaching this path implies the user explicitly confirmed a
        // fresh install from the modal. Auto-update flows (with an old
        // binary detected) skip the modal entirely and go through
        // `on_binary_check_complete` with `is_update: true`.
        self.state = SshInitState::AwaitingInstall {
            session_id,
            session_info,
            transport: transport.clone(),
            for_update: false,
        };
        environment_runtime::install_session_binary(session_id, transport, false, ctx);
    }

    /// Called when the remote server session is connected. Flushes the
    /// stashed bootstrap so the session initializes with a live client.
    fn on_session_connected(&mut self, session_id: SessionId, ctx: &mut ModelContext<Self>) {
        let SshInitState::AwaitingConnect {
            session_id: expected,
            ..
        } = &self.state
        else {
            return;
        };
        if *expected != session_id {
            return;
        }

        let SshInitState::AwaitingConnect { session_info, .. } =
            std::mem::replace(&mut self.state, SshInitState::Idle)
        else {
            unreachable!("just matched AwaitingConnect above");
        };

        // Flush the stashed bootstrap now that the server is connected.
        // `client_for_session` will return `Some` when the session
        // subsequently initializes, so it picks `EnvironmentRuntimeCommandExecutor`.
        self.flush_stashed_bootstrap(session_info, ctx);
    }

    /// Called when the remote server connection failed. Flushes the stashed
    /// bootstrap so the SSH session is not permanently blocked.
    fn on_session_connection_failed(
        &mut self,
        session_id: SessionId,
        ctx: &mut ModelContext<Self>,
    ) {
        let SshInitState::AwaitingConnect {
            session_id: expected,
            ..
        } = &self.state
        else {
            return;
        };
        if *expected != session_id {
            return;
        }

        let SshInitState::AwaitingConnect { session_info, .. } =
            std::mem::replace(&mut self.state, SshInitState::Idle)
        else {
            unreachable!("just matched AwaitingConnect above");
        };
        log::warn!(
            "Remote server connection failed for session {session_id:?}; \
             flushing bootstrap to unblock SSH session"
        );
        self.flush_stashed_bootstrap(session_info, ctx);
    }

    pub fn handle_environment_runtime_skip(
        &mut self,
        session_id: SessionId,
        ctx: &mut ModelContext<Self>,
    ) {
        let SshInitState::AwaitingUserChoice { session_info, .. } =
            std::mem::replace(&mut self.state, SshInitState::Idle)
        else {
            log::warn!("Skip clicked but state is not AwaitingUserChoice for {session_id:?}");
            return;
        };
        self.flush_stashed_bootstrap(session_info, ctx);
    }

    fn on_binary_install_complete(
        &mut self,
        session_id: SessionId,
        result: Result<(), String>,
        ctx: &mut ModelContext<Self>,
    ) {
        let expected = match &self.state {
            SshInitState::AwaitingInstall { session_id, .. } => *session_id,
            _ => return,
        };
        if expected != session_id {
            return;
        }

        let (session_info, transport) = match std::mem::replace(&mut self.state, SshInitState::Idle)
        {
            SshInitState::AwaitingInstall {
                session_info,
                transport,
                ..
            } => (session_info, transport),
            _ => unreachable!("just matched AwaitingInstall above"),
        };
        match result {
            Ok(()) => {
                let socket_path = transport.socket_path().clone();
                self.state = SshInitState::AwaitingConnect {
                    session_id,
                    session_info,
                };
                self.connect_session_for_current_identity(session_id, socket_path, ctx);
            }
            Err(err) => {
                log::error!("Binary install failed for {session_id:?}: {err}");
                self.flush_stashed_bootstrap(session_info, ctx);
            }
        }
    }

    fn connect_session_for_current_identity(
        &mut self,
        session_id: SessionId,
        socket_path: PathBuf,
        ctx: &mut ModelContext<Self>,
    ) {
        let transport =
            environment_runtime::transport_for_control_path(socket_path, self.auth_context.clone());
        let auth_context = self.auth_context.clone();
        environment_runtime::connect_session_transport(session_id, transport, auth_context, ctx);
    }
}
