use crate::ai::agent::api::ServerConversationToken;
use crate::ai::agent::conversation::AIConversationId;
use crate::ai::ambient_agents::{AgentSource, AmbientAgentTask, AmbientAgentTaskId};
use crate::ai::blocklist::{BlocklistAIHistoryEvent, BlocklistAIHistoryModel};
use crate::ai::conversation_navigation::ConversationNavigationData;
use crate::workspace::{RestoreConversationLayout, WorkspaceAction};
use clap::ValueEnum;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{HashMap, HashSet};
use warp_cli::agent::Harness;
use warp_core::features::FeatureFlag;
use warpui::{Entity, ModelContext, SingletonEntity};

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum StatusFilter {
    #[default]
    All,
    Working,
    Done,
    Failed,
}

#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum SourceFilter {
    #[default]
    All,
    Specific(AgentSource),
}

#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum CreatorFilter {
    #[default]
    All,
    Specific {
        name: String,
        uid: String,
    },
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum ArtifactFilter {
    #[default]
    All,
    PullRequest,
    Plan,
    Screenshot,
    File,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum CreatedOnFilter {
    #[default]
    All,
    Last24Hours,
    Past3Days,
    LastWeek,
}

#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum EnvironmentFilter {
    #[default]
    All,
    NoEnvironment,
    Specific(String),
}

#[derive(Default, Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OwnerFilter {
    All,
    #[default]
    PersonalOnly,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum HarnessFilter {
    #[default]
    All,
    Specific(Harness),
}

impl Serialize for HarnessFilter {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            HarnessFilter::All => serializer.serialize_str("all"),
            HarnessFilter::Specific(harness) => serializer.collect_str(harness),
        }
    }
}

impl<'de> Deserialize<'de> for HarnessFilter {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Ok(Harness::from_str(&raw, false)
            .ok()
            .map(HarnessFilter::Specific)
            .unwrap_or(HarnessFilter::All))
    }
}

#[derive(Default, PartialEq, Eq, Clone, Debug, Serialize, Deserialize)]
pub struct AgentManagementFilters {
    pub owners: OwnerFilter,
    pub status: StatusFilter,
    pub source: SourceFilter,
    pub created_on: CreatedOnFilter,
    pub creator: CreatorFilter,
    pub artifact: ArtifactFilter,
    #[serde(default)]
    pub environment: EnvironmentFilter,
    #[serde(default)]
    pub harness: HarnessFilter,
}

impl AgentManagementFilters {
    pub fn reset_all_but_owner(&mut self) {
        self.status = StatusFilter::default();
        self.source = SourceFilter::default();
        self.created_on = CreatedOnFilter::default();
        self.creator = CreatorFilter::default();
        self.artifact = ArtifactFilter::default();
        self.environment = EnvironmentFilter::default();
        self.harness = HarnessFilter::default();
    }

    pub fn is_filtering(&self) -> bool {
        self.status != StatusFilter::default()
            || self.source != SourceFilter::default()
            || self.created_on != CreatedOnFilter::default()
            || self.creator != CreatorFilter::default() && self.owners != OwnerFilter::PersonalOnly
            || self.artifact != ArtifactFilter::default()
            || self.environment != EnvironmentFilter::default()
            || self.harness != HarnessFilter::default()
    }
}

/// Stores conversation metadata needed for display in conversation/task views.
pub struct ConversationMetadata {
    pub nav_data: ConversationNavigationData,
}

/// ConversationOrTask is a wrapper around either conversation
/// or task data stored in the `AgentConversationsModel`.
///
/// It provides a unified interface for reading data related to tasks and conversations.
pub enum ConversationOrTask<'a> {
    Task(&'a AmbientAgentTask),
    Conversation(&'a ConversationMetadata),
}

impl ConversationOrTask<'_> {
    /// Returns the appropriate `WorkspaceAction` to dispatch when opening this item.
    /// This encapsulates the decision logic for opening ambient agent runs vs
    /// navigating to persisted user conversations.
    pub fn get_open_action(
        &self,
        restore_layout: Option<RestoreConversationLayout>,
    ) -> Option<WorkspaceAction> {
        match self {
            ConversationOrTask::Task(task) => task
                .conversation_id
                .as_ref()
                .filter(|conversation_id| !conversation_id.is_empty())
                .map(
                    |conversation_id| WorkspaceAction::OpenConversationTranscriptViewer {
                        conversation_id: ServerConversationToken::new(conversation_id.clone()),
                        ambient_agent_task_id: Some(task.task_id),
                    },
                )
                .or(Some(WorkspaceAction::OpenAmbientAgentSession {
                    task_id: task.task_id,
                })),
            ConversationOrTask::Conversation(metadata) => {
                let nav_data = &metadata.nav_data;
                Some(WorkspaceAction::RestoreOrNavigateToConversation {
                    conversation_id: nav_data.id,
                    window_id: nav_data.window_id,
                    pane_view_locator: nav_data.pane_view_locator,
                    terminal_view_id: nav_data.terminal_view_id,
                    restore_layout,
                })
            }
        }
    }
}

/// This model serves as a unified interface for reading both persisted user and ambient agent conversations
/// (i.e. conversations & tasks). The model is responsible for polling for new tasks and updating
/// its app state accordingly.
///
/// This model backs both the agent management view and Session Navigator.
pub struct AgentConversationsModel {
    /// A map of task IDs to agent tasks.
    tasks: HashMap<AmbientAgentTaskId, AmbientAgentTask>,
    /// A map of conversation IDs to persisted user conversations.
    conversations: HashMap<AIConversationId, ConversationMetadata>,
    /// Task IDs that have been manually opened from the management page.
    /// These will appear in Session Navigator even if their source is not user-initiated
    /// (and even after they have been closed).
    manually_opened_task_ids: HashSet<AmbientAgentTaskId>,
}

pub enum AgentConversationsModelEvent {
    /// Initial load of tasks completed.
    ConversationsLoaded,
    /// Existing task data may have been updated (e.g., state changes).
    TasksUpdated,
    /// Conversation status data was updated
    ConversationUpdated,
    /// Conversation artifacts were updated (plans, PRs, etc.)
    ConversationArtifactsUpdated,
    /// A task was manually opened from the management page.
    TaskManuallyOpened,
}

impl Entity for AgentConversationsModel {
    type Event = AgentConversationsModelEvent;
}

impl SingletonEntity for AgentConversationsModel {}

impl AgentConversationsModel {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        // AgentConversationsModel 不再负责轮询/探听
        // 远端 ambient agent tasks 与 conversation metadata。本地化场景下:
        //   - 无轮询子系统
        // BYOP agent 本地运行不依赖该模型。
        //
        // Issue #93 修复:必须订阅 BlocklistAIHistoryModel 的事件,否则用户在历史对话
        // 列表中删除对话后,本模型缓存的 conversations 不会刷新,UI 将持续展示已删除的项。
        let history_model = BlocklistAIHistoryModel::handle(ctx);
        ctx.subscribe_to_model(&history_model, |me, event, ctx| {
            me.handle_history_event(event, ctx);
        });

        Self {
            tasks: HashMap::new(),
            conversations: HashMap::new(),
            manually_opened_task_ids: HashSet::new(),
        }
    }

    /// Sync all conversations to the AgentConversationsModel.
    ///
    /// This function will loop through all active panes, recently closed panes, and historical
    /// conversations to construct a complete snapshot of conversations.
    pub fn sync_conversations(&mut self, ctx: &mut ModelContext<Self>) {
        if !FeatureFlag::InteractiveConversationManagementView.is_enabled() {
            return;
        }

        let nav_data_list = ConversationNavigationData::all_conversations(ctx);

        self.conversations.clear();
        for nav_data in nav_data_list {
            let conversation_id = nav_data.id;
            let metadata = ConversationMetadata { nav_data };
            self.conversations.insert(conversation_id, metadata);
        }

        ctx.emit(AgentConversationsModelEvent::ConversationsLoaded);
    }

    fn handle_history_event(
        &mut self,
        event: &BlocklistAIHistoryEvent,
        ctx: &mut ModelContext<Self>,
    ) {
        if !FeatureFlag::InteractiveConversationManagementView.is_enabled() {
            return;
        }

        match event {
            BlocklistAIHistoryEvent::StartedNewConversation { .. }
            | BlocklistAIHistoryEvent::SetActiveConversation { .. }
            | BlocklistAIHistoryEvent::AppendedExchange { .. }
            | BlocklistAIHistoryEvent::SplitConversation { .. }
            | BlocklistAIHistoryEvent::RestoredConversations { .. }
            | BlocklistAIHistoryEvent::RemoveConversation { .. }
            | BlocklistAIHistoryEvent::DeletedConversation { .. }
            | BlocklistAIHistoryEvent::UpdatedConversationMetadata { .. }
            | BlocklistAIHistoryEvent::ClearedConversationsInTerminalView { .. }
            | BlocklistAIHistoryEvent::ClearedActiveConversation { .. } => {
                self.sync_conversations(ctx);
            }
            BlocklistAIHistoryEvent::UpdatedConversationStatus { .. } => {
                ctx.emit(AgentConversationsModelEvent::ConversationUpdated);
            }
            BlocklistAIHistoryEvent::UpdatedConversationArtifacts {
                conversation_id, ..
            } => {
                let conversation =
                    BlocklistAIHistoryModel::as_ref(ctx).conversation(conversation_id);
                let Some(conversation) = conversation else {
                    return;
                };

                if let Some(task_id) = conversation.task_id() {
                    if let Some(task) = self.tasks.get_mut(&task_id) {
                        task.artifacts = conversation.artifacts().to_vec();
                        ctx.emit(AgentConversationsModelEvent::TasksUpdated);
                    }
                }

                ctx.emit(AgentConversationsModelEvent::ConversationArtifactsUpdated);
            }
            BlocklistAIHistoryEvent::CreatedSubtask { .. }
            | BlocklistAIHistoryEvent::UpgradedTask { .. }
            | BlocklistAIHistoryEvent::ReassignedExchange { .. }
            | BlocklistAIHistoryEvent::UpdatedTodoList { .. }
            | BlocklistAIHistoryEvent::UpdatedAutoexecuteOverride { .. }
            | BlocklistAIHistoryEvent::UpdatedStreamingExchange { .. }
            | BlocklistAIHistoryEvent::ConversationAgentIdAssigned { .. } => {}
        }
    }

    /// Returns an iterator over all ambient agent tasks.
    pub fn tasks_iter(&self) -> impl Iterator<Item = &AmbientAgentTask> {
        self.tasks.values()
    }

    /// Get raw task data by task ID
    pub fn get_task_data(&self, task_id: &AmbientAgentTaskId) -> Option<AmbientAgentTask> {
        self.tasks.get(task_id).cloned()
    }

    /// 按 task ID 读取本地已缓存的 task 数据。
    ///
    /// 当前应用不再向外部服务补取 ambient agent task。调用方如果恢复了布局但本地模型没有
    /// 对应 task,这里返回 `None`,由现有面板降级路径处理。
    pub fn get_or_async_fetch_task_data(
        &self,
        task_id: &AmbientAgentTaskId,
    ) -> Option<AmbientAgentTask> {
        self.tasks.get(task_id).cloned()
    }

    /// Get a conversation by its AIConversationId
    pub fn get_conversation(
        &self,
        conversation_id: &AIConversationId,
    ) -> Option<ConversationOrTask<'_>> {
        self.conversations
            .get(conversation_id)
            .map(ConversationOrTask::Conversation)
    }

    pub fn mark_task_as_manually_opened(
        &mut self,
        task_id: AmbientAgentTaskId,
        ctx: &mut ModelContext<Self>,
    ) {
        if self.manually_opened_task_ids.insert(task_id) {
            ctx.emit(AgentConversationsModelEvent::TaskManuallyOpened);
        }
    }
}

#[cfg(test)]
#[path = "agent_conversations_model_tests.rs"]
mod tests;
