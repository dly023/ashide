#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CodingPanelEnablementState {
    Enabled,
    /// An environment entry command has been detected at preexec time but the
    /// runtime session has not finished bootstrapping yet. The file tree should
    /// show a loading state immediately to avoid flickering the stale
    /// terminal tree.
    PendingRuntimeSession,
    /// The active session is backed by an environment runtime.
    ///
    /// `has_environment_runtime` is `true` when the runtime client is registered
    /// and can provide repo metadata. When `false` (for example a plain
    /// terminal-level SSH shell), no data will arrive and the file tree should
    /// show a disabled message.
    RuntimeSession {
        has_environment_runtime: bool,
    },
    UnsupportedSession,
    Disabled,
}

impl CodingPanelEnablementState {
    pub(crate) fn from_session_runtime(
        is_enabled: bool,
        uses_environment_runtime: bool,
        is_unsupported_session: bool,
        has_environment_runtime: bool,
    ) -> Self {
        if uses_environment_runtime {
            Self::RuntimeSession {
                has_environment_runtime,
            }
        } else if is_unsupported_session {
            Self::UnsupportedSession
        } else if is_enabled {
            Self::Enabled
        } else {
            Self::Disabled
        }
    }
}
