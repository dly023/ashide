use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Weak},
};

use warpui::{Entity, EntityId, ModelContext, SingletonEntity, WindowId};

use crate::ai::agent::conversation::AIConversationId;
use crate::ai::ambient_agents::AmbientAgentTaskId;
use crate::app_state::EnvironmentLifecycleState;
use crate::terminal::model::session::Session;

/// The active terminal session in each window. The active session of a window is the current
/// session of the most-recently-focused terminal pane of the active tab of the window's workspace.
///
/// #### When to use `ActiveSession`
/// Generally, if a more specific session is available, it should be preferred. For example, when
/// opening a Markdown file from a file link in a block's output, that block's session should be
/// the basis. However, sometimes there is no contextual session (such as when opening a file
/// in Ashide from Finder, or when starting from an object-store object). In that case, the `ActiveSession`
/// might be used, but it's often still better to be context-independent.
#[derive(Default)]
pub struct ActiveSession {
    window_sessions: HashMap<WindowId, WindowActiveSession>,
}

/// Active session information for an individual window.
#[derive(Default)]
struct WindowActiveSession {
    /// The [`Session`] model for the active session. This is a weak reference so that it doesn't
    /// prevent cleaning up the session when it closes, in case no other session is activated.
    session: Option<Weak<Session>>,
    /// The active session's working directory in the current app filesystem, if available.
    current_app_path: Option<PathBuf>,
    /// The active session's raw working directory in that session's environment, if available.
    current_working_directory: Option<String>,
    /// The [`EntityId`]` for the [`TerminalView`] for the active session, if there is one.
    terminal_view_id: Option<EntityId>,
    /// The active persisted conversation shown in the active terminal view, if available.
    active_conversation_id: Option<AIConversationId>,
    /// The active ambient/App Agent task shown in the active terminal view, if available.
    ambient_agent_task_id: Option<AmbientAgentTaskId>,
    /// The active Environment authority for runtime-backed sessions, if available.
    environment_authority_key: Option<String>,
    /// The provider connection reference for the active Environment, if available.
    environment_connection_ref: Option<String>,
    /// The active Environment lifecycle state, if available.
    environment_lifecycle_state: Option<EnvironmentLifecycleState>,
}

impl ActiveSession {
    /// The workspace's active session, if there is one.
    pub fn session(&self, window_id: WindowId) -> Option<Arc<Session>> {
        self.window_sessions
            .get(&window_id)?
            .session
            .as_ref()?
            .upgrade()
    }

    pub fn terminal_view_id(&self, window_id: WindowId) -> Option<EntityId> {
        self.window_sessions.get(&window_id)?.terminal_view_id
    }

    pub fn active_conversation_id(&self, window_id: WindowId) -> Option<AIConversationId> {
        self.window_sessions.get(&window_id)?.active_conversation_id
    }

    pub fn ambient_agent_task_id(&self, window_id: WindowId) -> Option<AmbientAgentTaskId> {
        self.window_sessions.get(&window_id)?.ambient_agent_task_id
    }

    pub fn environment_authority_key(&self, window_id: WindowId) -> Option<&str> {
        self.window_sessions
            .get(&window_id)?
            .environment_authority_key
            .as_deref()
    }

    pub fn environment_connection_ref(&self, window_id: WindowId) -> Option<&str> {
        self.window_sessions
            .get(&window_id)?
            .environment_connection_ref
            .as_deref()
    }

    pub fn environment_lifecycle_state(
        &self,
        window_id: WindowId,
    ) -> Option<&EnvironmentLifecycleState> {
        self.window_sessions
            .get(&window_id)?
            .environment_lifecycle_state
            .as_ref()
    }

    pub fn environment_id_for_terminal_view(&self, terminal_view_id: EntityId) -> Option<&str> {
        let window_state = self
            .window_sessions
            .values()
            .find(|state| state.terminal_view_id == Some(terminal_view_id))?;
        window_state
            .environment_connection_ref
            .as_deref()
            .or(window_state.environment_authority_key.as_deref())
    }

    /// The current working directory of the active session in the current app filesystem, if available.
    pub fn current_app_path(&self, window_id: WindowId) -> Option<&Path> {
        self.window_sessions
            .get(&window_id)?
            .current_app_path
            .as_deref()
    }

    /// The current working directory of the active session in its own environment, if available.
    pub fn current_working_directory(&self, window_id: WindowId) -> Option<&str> {
        self.window_sessions
            .get(&window_id)?
            .current_working_directory
            .as_deref()
    }

    /// Set the current session, for use in tests.
    #[cfg(test)]
    pub fn set_session_for_test(
        &mut self,
        window_id: WindowId,
        session: Arc<Session>,
        current_app_path: Option<impl Into<PathBuf>>,
        current_working_directory: Option<impl Into<String>>,
        terminal_view_id: Option<EntityId>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.set_session_state(
            window_id,
            Some(session),
            current_app_path.map(Into::into),
            current_working_directory.map(Into::into),
            terminal_view_id,
            None,
            None,
            None,
            None,
            None,
            ctx,
        );
    }

    pub(super) fn set_session_state(
        &mut self,
        window_id: WindowId,
        session: Option<Arc<Session>>,
        current_app_path: Option<PathBuf>,
        current_working_directory: Option<String>,
        terminal_view_id: Option<EntityId>,
        active_conversation_id: Option<AIConversationId>,
        ambient_agent_task_id: Option<AmbientAgentTaskId>,
        environment_authority_key: Option<String>,
        environment_connection_ref: Option<String>,
        environment_lifecycle_state: Option<EnvironmentLifecycleState>,
        ctx: &mut ModelContext<Self>,
    ) {
        let window_state = self.window_sessions.entry(window_id).or_default();

        let session = session.map(|session| Arc::downgrade(&session));
        if window_state.session.is_some() != session.is_some() {
            window_state.session = session;
            ctx.notify();
        } else if let Some((prev_session, next_session)) =
            window_state.session.as_ref().zip(session)
        {
            // Session IDs can't necessarily be compared across terminal panes, so check if the backing
            // allocation is the same. We can do this because each `Session` is a singleton.
            if !Weak::ptr_eq(prev_session, &next_session) {
                window_state.session = Some(next_session);
                ctx.notify();
            }
        }

        if window_state.current_app_path != current_app_path {
            window_state.current_app_path = current_app_path;
            ctx.notify();
        }

        if window_state.current_working_directory != current_working_directory {
            window_state.current_working_directory = current_working_directory;
            ctx.notify();
        }

        if window_state.terminal_view_id != terminal_view_id {
            window_state.terminal_view_id = terminal_view_id;
            ctx.notify();
        }

        if window_state.active_conversation_id != active_conversation_id {
            window_state.active_conversation_id = active_conversation_id;
            ctx.notify();
        }

        if window_state.ambient_agent_task_id != ambient_agent_task_id {
            window_state.ambient_agent_task_id = ambient_agent_task_id;
            ctx.notify();
        }

        if window_state.environment_authority_key != environment_authority_key {
            window_state.environment_authority_key = environment_authority_key;
            ctx.notify();
        }

        if window_state.environment_connection_ref != environment_connection_ref {
            window_state.environment_connection_ref = environment_connection_ref;
            ctx.notify();
        }

        if window_state.environment_lifecycle_state != environment_lifecycle_state {
            window_state.environment_lifecycle_state = environment_lifecycle_state;
            ctx.notify();
        }
    }

    pub(super) fn close_workspace(&mut self, window_id: WindowId) {
        self.window_sessions.remove(&window_id);
    }
}

impl Entity for ActiveSession {
    type Event = ();
}

impl SingletonEntity for ActiveSession {}
