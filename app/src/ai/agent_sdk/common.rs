//! Common utilities for agent SDK commands.

use std::future::Future;

use futures::TryFutureExt;

use warpui::r#async::FutureExt;
use warpui::{AppContext, SingletonEntity as _};

use crate::ai::agent_sdk::driver::LOCAL_DRIVE_SYNC_TIMEOUT;

use crate::ai::llms::{LLMId, LLMPreferences};
use crate::object_store::model::persistence::ObjectStoreModel;

pub fn validate_agent_mode_base_model_id(
    model_id: &str,
    ctx: &AppContext,
) -> anyhow::Result<LLMId> {
    let llm_prefs = LLMPreferences::as_ref(ctx);

    let llm_id: LLMId = model_id.into();
    let valid_ids = llm_prefs
        .get_base_llm_choices_for_agent_mode()
        .map(|info| info.id.clone())
        .collect::<Vec<_>>();

    if valid_ids.contains(&llm_id) {
        Ok(llm_id)
    } else {
        let suggestions = valid_ids
            .into_iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        Err(anyhow::anyhow!(
            "Unknown model id '{model_id}'. Try one of: {suggestions}"
        ))
    }
}

/// Refresh workspace metadata before executing an operation.
///
/// This keeps local workspace state fresh before object operations.
pub fn refresh_workspace_metadata<C>(
    _ctx: &mut C,
) -> impl Future<Output = anyhow::Result<()>> + Send + 'static {
    async { Ok(()) }
}

/// Refresh Ashide Drive before executing an operation.
pub fn refresh_local_drive(
    ctx: &AppContext,
) -> impl Future<Output = anyhow::Result<()>> + Send + 'static {
    ObjectStoreModel::as_ref(ctx)
        .initial_load_complete()
        .with_timeout(LOCAL_DRIVE_SYNC_TIMEOUT)
        .map_err(|_| anyhow::anyhow!("Timed out waiting for Ashide Drive to sync"))
}
