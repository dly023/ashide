use super::super::controller::{BlocklistAIController, BlocklistAIControllerEvent};
use crate::terminal::event::BlockType;
use crate::terminal::model_events::{ModelEvent, ModelEventDispatcher};
use crate::terminal::view::ambient_agent::AmbientAgentViewModel;
use warp_core::features::FeatureFlag;
use warpui::{Entity, ModelContext, ModelHandle};

pub struct PassiveSuggestionsModel {
    pending_request_active: bool,
    ambient_agent_view_model: ModelHandle<AmbientAgentViewModel>,
}

impl PassiveSuggestionsModel {
    pub fn new(
        ai_controller: ModelHandle<BlocklistAIController>,
        model_event_dispatcher: &ModelHandle<ModelEventDispatcher>,
        ambient_agent_view_model: ModelHandle<AmbientAgentViewModel>,
        ctx: &mut ModelContext<Self>,
    ) -> Self {
        ctx.subscribe_to_model(model_event_dispatcher, |me, event, ctx| {
            me.handle_model_event(event, ctx);
        });
        ctx.subscribe_to_model(&ai_controller, |me, event, ctx| {
            me.handle_controller_event(event, ctx);
        });

        Self {
            pending_request_active: false,
            ambient_agent_view_model,
        }
    }

    pub fn abort_pending_requests(&mut self, _ctx: &mut ModelContext<Self>) {
        self.pending_request_active = false;
    }

    fn handle_model_event(&mut self, event: &ModelEvent, ctx: &mut ModelContext<Self>) {
        if let ModelEvent::AfterBlockStarted { .. } = event {
            self.abort_pending_requests(ctx);
            return;
        }

        if let ModelEvent::AfterBlockCompleted(after_block_completed_event) = event {
            if !FeatureFlag::PromptSuggestionsViaMAA.is_enabled() {
                self.abort_pending_requests(ctx);
                return;
            }

            if let BlockType::User(block_completed) = &after_block_completed_event.block_type {
                if !block_completed.was_part_of_agent_interaction {
                    self.skip_disabled_request(ctx);
                }
            }
        }
    }

    fn handle_controller_event(
        &mut self,
        event: &BlocklistAIControllerEvent,
        ctx: &mut ModelContext<Self>,
    ) {
        match event {
            BlocklistAIControllerEvent::SentRequest { .. } => {
                self.abort_pending_requests(ctx);
            }
            BlocklistAIControllerEvent::FinishedReceivingOutput { .. } => {
                if FeatureFlag::PromptSuggestionsViaMAA.is_enabled() {
                    self.skip_disabled_request(ctx);
                } else {
                    self.abort_pending_requests(ctx);
                }
            }
            BlocklistAIControllerEvent::ExportConversationToFile { .. } => {}
        }
    }

    fn skip_disabled_request(&mut self, _ctx: &mut ModelContext<Self>) {
        if self
            .ambient_agent_view_model
            .as_ref(_ctx)
            .is_ambient_agent()
        {
            return;
        }

        self.pending_request_active = false;
        log::debug!(
            "[passive-suggestions] skipped MAA request because the multi-agent endpoint is disabled in Ashide"
        );
    }
}

impl Entity for PassiveSuggestionsModel {
    type Event = ();
}
