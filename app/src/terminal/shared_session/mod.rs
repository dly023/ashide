use byte_unit::Byte;
use warpui::{id, keymap::ContextPredicate, AppContext};

use crate::editor::{InteractionState, ReplicaId};

pub mod protocol;
pub use protocol::ParticipantId;

#[cfg(test)]
use protocol::Scrollback;
use protocol::{Role, SessionSourceType};

use super::{model::terminal_model::BlockIndex, GridType, TerminalModel};

pub mod ai_agent;
pub mod participant_avatar_view;
pub mod presence_manager;
pub mod render_util;
pub(crate) mod selections;
pub mod settings;
pub mod viewer;

#[cfg(test)]
pub use tests::MAX_BYTES_SHAREABLE;

/// The toast copy when copying a shared session link.
pub const COPY_LINK_TEXT: &str = "Sharing link copied";

/// Whether or not a current-app session is also being shared.
/// Since a shared session creator is also the creator of a current-app session,
/// we make use of the local_tty::TerminalManager for shared session creators.
/// Otherwise, there would be a lot of overlap between a shared session creator
/// and a regular, current-app-only session.
#[derive(Debug, Clone, Default)]
pub enum IsSharedSessionCreator {
    /// This session should be shared automatically once bootstrapped, using the
    /// provided source type.
    Yes { source_type: SessionSourceType },
    #[default]
    No,
}

/// The type of shared session a particular session is, if applicable.
#[derive(Debug, Clone, Default)]
pub enum SharedSessionStatus {
    /// This session is not a shared session.
    /// When a sharer ends a session, the status
    /// changes back to [`SharedSessionStatus::NotShared`].
    #[default]
    NotShared,

    /// We're in the process of joining the session but have not
    /// established the connection with the server yet, or have not received all the events that occurred before the viewer joined yet.
    ViewPending,

    /// This session is a shared session that we are actively viewing.
    /// We have received all the scrollback and events for the shared session that occurred before the viewer joined, and are caught up and receiving events live.
    ActiveViewer { role: Role },

    /// We were viewing a shared session but it ended.
    FinishedViewer,

    /// This session is actively being shared.
    ActiveSharer,
}

impl SharedSessionStatus {
    pub fn reader() -> Self {
        Self::ActiveViewer { role: Role::Reader }
    }

    pub fn executor() -> Self {
        Self::ActiveViewer {
            role: Role::Executor,
        }
    }

    pub fn is_view_pending(&self) -> bool {
        matches!(self, SharedSessionStatus::ViewPending)
    }

    pub fn is_active_viewer(&self) -> bool {
        matches!(self, SharedSessionStatus::ActiveViewer { .. })
    }

    pub fn is_finished_viewer(&self) -> bool {
        matches!(self, SharedSessionStatus::FinishedViewer)
    }

    pub fn is_viewer(&self) -> bool {
        self.is_view_pending() || self.is_active_viewer() || self.is_finished_viewer()
    }

    pub fn is_executor(&self) -> bool {
        matches!(self, SharedSessionStatus::ActiveViewer { role } if role.can_execute())
    }

    pub fn is_reader(&self) -> bool {
        matches!(
            self,
            SharedSessionStatus::ActiveViewer { role: Role::Reader }
        )
    }

    pub fn is_active_sharer(&self) -> bool {
        matches!(self, SharedSessionStatus::ActiveSharer)
    }

    pub fn is_sharer(&self) -> bool {
        self.is_active_sharer()
    }

    pub fn is_sharer_or_viewer(&self) -> bool {
        !matches!(self, Self::NotShared)
    }

    pub fn as_keymap_context(&self) -> &'static str {
        match self {
            Self::NotShared => "SharedSessionStatus_NotShared",
            Self::ViewPending => "SharedSessionStatus_ViewPending",
            Self::ActiveViewer { role: Role::Reader } => "SharedSessionStatus_Reader",
            Self::ActiveViewer {
                role: Role::Executor | Role::Full,
            } => "SharedSessionStatus_Executor",
            Self::FinishedViewer => "SharedSessionStatus_FinishedViewer",
            Self::ActiveSharer => "SharedSessionStatus_ActiveSharer",
        }
    }

    pub fn active_viewer_keymap_context() -> ContextPredicate {
        id!(Self::reader().as_keymap_context()) | id!(Self::executor().as_keymap_context())
    }
}

/// The scrollback options when starting a shared session.
/// Note: currently, these options only encode the point at which
/// scrollback _starts_. We do not yet support more
/// selective scrollback (e.g. a closed range).
/// The active block is always included in scrollback for the prompt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SharedSessionScrollbackType {
    /// Do not include any scrollback in this shared session.
    /// Note the active block is still sent as part of scrollback for the prompt.
    /// TODO(suraj): consider renaming this to "from active block" or encapsulating
    /// this with the `FromBlock` variant with the block_index equal to the
    /// active block index.
    None,

    /// Include scrollback starting at `block_index`.
    FromBlock { block_index: BlockIndex },

    /// The entire blocklist should be part of the scrollback.
    All,
}

impl SharedSessionScrollbackType {
    /// Returns the set of scrollback that adheres to the scrollback type.
    /// Note that some blocks might not actually be included in the scrollback
    /// even if they were specified as part of the scrollback type.
    /// For example, if the [`Self::All]` variant is used, restored blocks
    /// _won't_ be included in scrollback.
    #[cfg(test)]
    fn to_scrollback(self, model: &TerminalModel) -> Scrollback {
        use super::model::block::SerializedBlock;
        use protocol::ScrollbackBlock;
        let first_block_index = self.first_block_index(model);
        let blocks = model
            .block_list()
            .blocks()
            .iter()
            .skip(first_block_index.into())
            .filter(|block| {
                block.is_scrollback_block_for_shared_session(model.block_list().agent_view_state())
            })
            .filter_map(|block| {
                let serialized_block: SerializedBlock = block.into();
                let bytes = serde_json::to_vec(&serialized_block);
                bytes.ok().map(|raw| ScrollbackBlock { raw })
            })
            .collect();

        let is_alt_screen_active = model.is_alt_screen_active();

        Scrollback {
            blocks,
            is_alt_screen_active,
        }
    }

    /// Returns the first block index that will be used for scrollback.
    pub fn first_block_index(self, model: &TerminalModel) -> BlockIndex {
        match self {
            Self::None => model.block_list().active_block_index(),
            Self::FromBlock { block_index } => model
                .block_list()
                .blocks()
                .iter()
                .skip(block_index.into())
                .find(|block| {
                    block.is_scrollback_block_for_shared_session(
                        model.block_list().agent_view_state(),
                    )
                })
                .map_or(model.block_list().active_block_index(), |block| {
                    block.index()
                }),
            Self::All => Self::FromBlock {
                block_index: BlockIndex::zero(),
            }
            .first_block_index(model),
        }
    }
}

#[cfg(not(test))]
pub fn max_session_size(ctx: &AppContext) -> Byte {
    use crate::workspaces::user_workspaces::UserWorkspaces;
    use warpui::SingletonEntity;

    UserWorkspaces::as_ref(ctx)
        .current_team()
        .and_then(|team| team.workspace_policy.policy.session_sharing_policy)
        .map(|policy| Byte::from_u64(policy.max_session_size))
        .unwrap_or(Byte::from_u64_with_unit(100, byte_unit::Unit::MB).unwrap())
}

#[cfg(test)]
pub fn max_session_size(_ctx: &AppContext) -> Byte {
    Byte::from_u64(MAX_BYTES_SHAREABLE as u64)
}

impl From<GridType> for protocol::GridType {
    fn from(val: GridType) -> Self {
        match val {
            GridType::Prompt => protocol::GridType::Prompt,
            GridType::Rprompt => protocol::GridType::Rprompt,
            GridType::Output => protocol::GridType::Output,
            GridType::PromptAndCommand => protocol::GridType::PromptAndCommand,
        }
    }
}

impl From<protocol::GridType> for GridType {
    fn from(value: protocol::GridType) -> Self {
        match value {
            protocol::GridType::Prompt => Self::Prompt,
            protocol::GridType::Rprompt => Self::Rprompt,
            protocol::GridType::Output => Self::Output,
            protocol::GridType::PromptAndCommand => Self::PromptAndCommand,
        }
    }
}

impl From<ReplicaId> for protocol::InputReplicaId {
    fn from(value: ReplicaId) -> Self {
        value.to_string().into()
    }
}

impl From<protocol::InputReplicaId> for ReplicaId {
    fn from(value: protocol::InputReplicaId) -> Self {
        ReplicaId::new(value)
    }
}

impl From<&Role> for InteractionState {
    fn from(value: &Role) -> InteractionState {
        match value {
            Role::Reader => InteractionState::Selectable,
            Role::Executor => InteractionState::Editable,
            Role::Full => InteractionState::Editable,
        }
    }
}

/// Decode scrollback blocks from their JSON wire format into [`SerializedBlock`]s.
///
/// Blocks that fail to deserialize are silently dropped.
#[cfg(test)]
pub(crate) fn decode_scrollback(
    scrollback: &Scrollback,
) -> Vec<super::model::block::SerializedBlock> {
    scrollback
        .blocks
        .iter()
        .filter_map(|block| serde_json::from_slice(&block.raw).ok())
        .collect()
}

#[cfg(test)]
#[path = "mod_test.rs"]
mod tests;
