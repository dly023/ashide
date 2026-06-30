use crate::terminal::{
    bootstrap::init_shell_script_for_shell,
    event_listener::ChannelEventListener,
    model::{ansi::Processor, terminal_model::ExitReason},
    session_settings::SessionSettings,
    shell::ShellType,
    writeable_pty::Message as EventLoopMessage,
    SizeInfo, TerminalModel,
};
use crate::workspace::environment_runtime::{
    self, EnvironmentRuntimeClient, EnvironmentRuntimeClientError,
    EnvironmentRuntimePtyCreateResult, EnvironmentRuntimePtyEvent,
};
use async_channel::Receiver;
use futures_util::SinkExt;
use parking_lot::FairMutex;
use serde::Serialize;
use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use warp_core::SessionId;
use warpui::{Entity, ModelContext, SingletonEntity};
use websocket::{Message, Sink, Stream, WebSocket, WebsocketMessage as _};

const CREATE_SESSION_ENDPOINT: &str = "ws://127.0.0.1:3030/create";

#[derive(Clone)]
pub struct EnvironmentRuntimePtyConfig {
    pub client: Arc<EnvironmentRuntimeClient>,
    pub session_id: SessionId,
    pub working_directory: String,
    pub shell: String,
    pub startup_command: Option<String>,
    pub environment_variables: HashMap<String, String>,
    pub honor_ps1_enabled: bool,
}

struct EnvironmentRuntimePtyState {
    config: EnvironmentRuntimePtyConfig,
    pty_id: Option<u64>,
    closed: bool,
    pending_messages: Vec<EventLoopMessage>,
}

/// Contains info needed to resize the SSH terminal session. Is serialized and
/// sent over the websocket as text.
///
/// The field names need to be kept the same as the `WindowSizeChange` struct in
/// https://github.com/warpdotdev/ssh-proxy-server/blob/main/src/ssh/session.rs.
#[derive(Serialize, Debug)]
struct WindowSizeChange {
    width: u32,
    height: u32,
    width_px: u32,
    height_px: u32,
}

pub(super) struct EventLoop {
    terminal_model: Arc<FairMutex<TerminalModel>>,
    parser: Processor,
    event_loop_rx: Receiver<EventLoopMessage>,
    channel_event_listener: ChannelEventListener,
    environment_runtime_pty: Option<EnvironmentRuntimePtyState>,
}

impl EventLoop {
    /// Starts the [`EventLoop`] by starting a websocket connection with the server and
    /// bootstrapping the PTY.
    pub(super) fn start(
        model: Arc<FairMutex<TerminalModel>>,
        websocket_receiver: Receiver<EventLoopMessage>,
        channel_event_listener: ChannelEventListener,
        size_info: SizeInfo,
        ctx: &mut ModelContext<Self>,
    ) -> Self {
        let event_loop = Self::new(model, websocket_receiver, channel_event_listener, None);

        let url = Self::get_new_session_url(size_info);
        let response = WebSocket::connect(url, None /* protocols */);

        ctx.spawn(response, Self::on_ws_connection);

        event_loop
    }

    pub(super) fn start_environment_runtime(
        model: Arc<FairMutex<TerminalModel>>,
        message_receiver: Receiver<EventLoopMessage>,
        channel_event_listener: ChannelEventListener,
        size_info: SizeInfo,
        config: EnvironmentRuntimePtyConfig,
        ctx: &mut ModelContext<Self>,
    ) -> Self {
        let event_loop = Self::new(
            model,
            message_receiver.clone(),
            channel_event_listener,
            Some(EnvironmentRuntimePtyState {
                config: config.clone(),
                pty_id: None,
                closed: false,
                pending_messages: Vec::new(),
            }),
        );

        environment_runtime::subscribe_to_pty_events(ctx, |me, event, _ctx| {
            me.on_environment_runtime_pty_event(event);
        });

        let create_client = config.client.clone();
        let create_config = config.clone();
        ctx.spawn(
            async move {
                environment_runtime::create_pty(
                    &create_client,
                    create_config.working_directory,
                    create_config.shell,
                    size_info.rows as u32,
                    size_info.columns as u32,
                    create_config.environment_variables,
                )
                .await
            },
            |me, result, _ctx| {
                me.on_environment_runtime_pty_created(result);
            },
        );

        ctx.spawn_stream_local(
            message_receiver,
            |me, message, _ctx| me.handle_environment_runtime_event_loop_message(message),
            |_, _| {},
        );

        event_loop
    }

    fn new(
        terminal_model: Arc<FairMutex<TerminalModel>>,
        websocket_receiver: Receiver<EventLoopMessage>,
        channel_event_listener: ChannelEventListener,
        environment_runtime_pty: Option<EnvironmentRuntimePtyState>,
    ) -> Self {
        Self {
            terminal_model,
            parser: Processor::default(),
            event_loop_rx: websocket_receiver,
            channel_event_listener,
            environment_runtime_pty,
        }
    }

    fn get_new_session_url(size_info: SizeInfo) -> String {
        let num_rows = size_info.rows;
        let num_cols = size_info.columns;

        format!("{CREATE_SESSION_ENDPOINT}?num_rows={num_rows}&num_cols={num_cols}")
    }

    /// Starts tasks to listen to and write to the websocket.
    fn start_websocket_listener_and_writer_tasks(
        &mut self,
        mut sink: impl Sink,
        stream: impl Stream,
        ctx: &mut ModelContext<Self>,
    ) {
        // TODO(alokedesai): Add a spawn_stream equivalent that runs on the background executor.
        ctx.spawn_stream_local(
            stream,
            |event_loop, message, _| {
                let message = match message {
                    Ok(message) => message,
                    Err(err) => {
                        log::error!("Unable to receive item: {err:?}");
                        return;
                    }
                };

                let Some(bytes) = message.binary() else {
                    log::error!("Received non binary message");
                    return;
                };

                event_loop.process_pty_bytes(bytes);
            },
            |_, _| {},
        );

        let is_honor_ps1_enabled = *SessionSettings::as_ref(ctx).honor_ps1;

        let receiver = self.event_loop_rx.clone();
        ctx.background_executor()
            .spawn(async move {
                if let Err(e) = Self::write_env_vars(&mut sink, is_honor_ps1_enabled).await {
                    log::error!("Failed to write env vars to pty {e:?}");
                }
                if let Err(e) = Self::write_zsh_init_shell_script(&mut sink).await {
                    log::error!("Failed to write zsh bootstrap bytes to pty {e:?}");
                }

                while let Ok(message) = receiver.recv().await {
                    match message {
                        EventLoopMessage::Input(bytes) => {
                            if let Err(e) = sink.send(Message::new_binary(bytes.to_vec())).await {
                                log::error!("Failed to send message to network-backed PTY {e:?}");
                            };
                        }
                        EventLoopMessage::Resize(size_info) => {
                            let size_change = WindowSizeChange {
                                width: size_info.columns as u32,
                                height: size_info.rows as u32,
                                width_px: size_info.pane_width_px().as_f32() as u32,
                                height_px: size_info.pane_height_px().as_f32() as u32,
                            };

                            let Ok(serialized) = serde_json::to_string(&size_change) else {
                                log::error!("Error serializing window size change info");
                                continue;
                            };

                            // Sending as a `Text` message implies that this is a
                            // control channel message. The SSH proxy server should
                            // make this distinction.
                            if let Err(e) = sink.send(Message::new_text(serialized)).await {
                                log::error!("Failed to send message to network-backed PTY {e:?}");
                            };
                        }
                        // TODO(alokedesai): Implement shutdown on the network backed PTY.
                        EventLoopMessage::Shutdown | EventLoopMessage::ChildExited => {}
                    }
                }
            })
            .detach();
    }

    /// Writes the ZSH init shell script to the "PTY", mimicking how we send the init shell script
    /// when there is a local pty:
    /// <https://github.com/warpdotdev/warp-internal/blob/747da2df83f2caa97e781ce284ceb226fb97a66c/app/src/terminal/local_tty/unix.rs#L338-L347>.
    async fn write_zsh_init_shell_script(sink: &mut impl Sink) -> anyhow::Result<()> {
        let zsh_init_shell_script = init_shell_script_for_shell(ShellType::Zsh, &crate::ASSETS);
        sink.send(Message::new_binary(
            zsh_init_shell_script.as_bytes().to_vec(),
        ))
        .await?;

        sink.send(Message::new_binary(
            ShellType::Zsh.execute_command_bytes().to_vec(),
        ))
        .await?;

        Ok(())
    }

    /// Writes environment variables that should be defined in the session
    /// before bootstrapping. This is a subset of the environment variables
    /// defined in `app/src/terminal/local_tty/unix.rs` that are necessary in
    /// order to dogfood Ashide on Web over the remote tty.
    async fn write_env_vars(
        sink: &mut impl Sink,
        is_honor_ps1_enabled: bool,
    ) -> anyhow::Result<()> {
        let honor_ps1_env_var = format!(r#"WARP_HONOR_PS1="{}";"#, is_honor_ps1_enabled as u8);
        sink.send(Message::new_binary(honor_ps1_env_var.as_bytes().to_vec()))
            .await?;

        Ok(())
    }

    fn mark_environment_runtime_pty_failed(&self, reason: ExitReason) {
        self.terminal_model.lock().exit(reason);
        self.channel_event_listener.send_wakeup_event();
    }

    fn on_environment_runtime_pty_created(
        &mut self,
        result: Result<EnvironmentRuntimePtyCreateResult, EnvironmentRuntimeClientError>,
    ) {
        let Some(state) = self.environment_runtime_pty.as_mut() else {
            return;
        };
        let response = match result {
            Ok(response) => response,
            Err(error) => {
                log::error!("Failed to create environment runtime PTY: {error}");
                state.closed = true;
                state.pending_messages.clear();
                self.mark_environment_runtime_pty_failed(ExitReason::PtySpawnFailed);
                return;
            }
        };
        match response {
            EnvironmentRuntimePtyCreateResult::Created { pty_id, shell_type } => {
                state.pty_id = Some(pty_id);
                let shell_type =
                    Self::environment_runtime_shell_type(&shell_type, &state.config.shell);
                if let Some(shell_type) = shell_type {
                    if let Err(error) = Self::bootstrap_environment_runtime_pty(state, shell_type) {
                        log::error!("Failed to bootstrap environment runtime PTY: {error}");
                        if let Err(close_error) = state.config.client.close_pty(pty_id) {
                            log::debug!(
                                "Failed to close environment runtime PTY after bootstrap error: {close_error}"
                            );
                        }
                        state.pty_id = None;
                        state.closed = true;
                        state.pending_messages.clear();
                        self.mark_environment_runtime_pty_failed(ExitReason::PtySpawnFailed);
                        return;
                    }
                } else {
                    log::warn!(
                        "Skipping environment runtime PTY bootstrap for unsupported shell {:?}",
                        shell_type
                    );
                }
                if let Some(command) = &state.config.startup_command {
                    let bytes = format!("{command}\n").into_bytes();
                    if let Err(error) = state.config.client.write_pty(pty_id, bytes) {
                        log::error!(
                            "Failed to write environment runtime PTY startup command: {error}"
                        );
                        state.pty_id = None;
                        state.closed = true;
                        state.pending_messages.clear();
                        self.mark_environment_runtime_pty_failed(ExitReason::PtyDisconnected);
                        return;
                    }
                }
                for message in std::mem::take(&mut state.pending_messages) {
                    Self::send_environment_runtime_event_loop_message(state, message);
                }
            }
            EnvironmentRuntimePtyCreateResult::Failed(error) => {
                log::error!("Environment runtime PTY creation failed: {error}");
                state.closed = true;
                state.pending_messages.clear();
                self.mark_environment_runtime_pty_failed(ExitReason::PtySpawnFailed);
            }
            EnvironmentRuntimePtyCreateResult::Empty => {
                log::error!("Environment runtime PTY creation returned an empty result");
                state.closed = true;
                state.pending_messages.clear();
                self.mark_environment_runtime_pty_failed(ExitReason::PtySpawnFailed);
            }
        }
    }

    fn environment_runtime_shell_type(
        shell_type: &str,
        configured_shell: &str,
    ) -> Option<ShellType> {
        ShellType::from_name(shell_type).or_else(|| ShellType::from_name(configured_shell))
    }

    fn bootstrap_environment_runtime_pty(
        state: &EnvironmentRuntimePtyState,
        shell_type: ShellType,
    ) -> Result<(), EnvironmentRuntimeClientError> {
        let Some(pty_id) = state.pty_id else {
            return Ok(());
        };
        let honor_ps1_env_var = format!(
            r#"WARP_HONOR_PS1="{}";"#,
            state.config.honor_ps1_enabled as u8
        );
        state
            .config
            .client
            .write_pty(pty_id, honor_ps1_env_var.into_bytes())?;

        let init_shell_script = init_shell_script_for_shell(shell_type, &crate::ASSETS);
        state
            .config
            .client
            .write_pty(pty_id, init_shell_script.into_bytes())?;
        state
            .config
            .client
            .write_pty(pty_id, shell_type.execute_command_bytes().to_vec())
    }

    fn on_environment_runtime_pty_event(&mut self, event: EnvironmentRuntimePtyEvent) {
        let Some(state) = self.environment_runtime_pty.as_mut() else {
            return;
        };
        match event {
            EnvironmentRuntimePtyEvent::Output {
                session_id,
                pty_id,
                bytes,
            } if session_id == state.config.session_id && Some(pty_id) == state.pty_id => {
                self.process_pty_bytes(&bytes);
            }
            EnvironmentRuntimePtyEvent::Exited { session_id, pty_id }
                if session_id == state.config.session_id && Some(pty_id) == state.pty_id =>
            {
                log::info!("Environment runtime PTY {pty_id} exited for session {session_id:?}");
                if let Err(error) = state.config.client.close_pty(pty_id) {
                    log::debug!("Failed to acknowledge environment runtime PTY exit: {error}");
                }
                state.pty_id = None;
                state.closed = true;
                state.pending_messages.clear();
                self.mark_environment_runtime_pty_failed(ExitReason::ShellProcessExited);
            }
            _ => {}
        }
    }

    fn handle_environment_runtime_event_loop_message(&mut self, message: EventLoopMessage) {
        let Some(state) = self.environment_runtime_pty.as_mut() else {
            return;
        };
        if state.closed {
            return;
        }
        if state.pty_id.is_none() {
            if matches!(
                message,
                EventLoopMessage::Shutdown | EventLoopMessage::ChildExited
            ) {
                state.closed = true;
                state.pending_messages.clear();
                return;
            }
            state.pending_messages.push(message);
            return;
        };
        let should_close = matches!(
            message,
            EventLoopMessage::Shutdown | EventLoopMessage::ChildExited
        );
        Self::send_environment_runtime_event_loop_message(state, message);
        if should_close {
            state.pty_id = None;
            state.closed = true;
            state.pending_messages.clear();
        }
    }

    fn send_environment_runtime_event_loop_message(
        state: &EnvironmentRuntimePtyState,
        message: EventLoopMessage,
    ) {
        let Some(pty_id) = state.pty_id else {
            return;
        };
        match message {
            EventLoopMessage::Input(bytes) => {
                if let Err(error) = state.config.client.write_pty(pty_id, bytes.to_vec()) {
                    log::error!("Failed to send input to environment runtime PTY: {error}");
                }
            }
            EventLoopMessage::Resize(size_info) => {
                if let Err(error) = state.config.client.resize_pty(
                    pty_id,
                    size_info.rows as u32,
                    size_info.columns as u32,
                    size_info.pane_width_px().as_f32() as u32,
                    size_info.pane_height_px().as_f32() as u32,
                ) {
                    log::error!("Failed to resize environment runtime PTY: {error}");
                }
            }
            EventLoopMessage::Shutdown | EventLoopMessage::ChildExited => {
                if let Err(error) = state.config.client.close_pty(pty_id) {
                    log::error!("Failed to close environment runtime PTY: {error}");
                }
            }
        }
    }

    fn on_ws_connection(
        &mut self,
        connection: anyhow::Result<WebSocket>,
        ctx: &mut ModelContext<Self>,
    ) {
        let connection = match connection {
            Ok(connection) => connection,
            Err(e) => {
                log::error!("Failed to construct websocket connection: {e:?}");
                return;
            }
        };

        ctx.spawn(connection.split(), |me, (sink, stream), ctx| {
            me.start_websocket_listener_and_writer_tasks(sink, stream, ctx);
        });
    }

    /// Processes a byte slice through the `Processor`.
    fn process_pty_bytes(&mut self, bytes: &[u8]) {
        let mut terminal_model = self.terminal_model.lock();
        self.parser
            .parse_bytes(&mut *terminal_model, bytes, &mut io::sink());
        self.channel_event_listener.send_wakeup_event();
    }
}

impl Entity for EventLoop {
    type Event = ();
}
