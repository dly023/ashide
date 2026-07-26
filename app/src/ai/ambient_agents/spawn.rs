//! Stream-based API for spawning ambient agents.

use futures::Stream;

use crate::ai::ambient_agents::SpawnAgentRequest;

/// Ambient agent spawning is disabled in Ashide.
#[derive(Debug)]
pub enum AmbientAgentEvent {}

/// Spawns an ambient agent task.
///
/// Ashide has removed the cloud multi-agent backend, so this stream reports a
/// deterministic error instead of retaining the old polling/join-session flow.
pub fn spawn_task(
    _request: SpawnAgentRequest,
    _timeout: Option<std::time::Duration>,
) -> impl Stream<Item = Result<AmbientAgentEvent, anyhow::Error>> {
    async_stream::stream! {
        yield Err(anyhow::anyhow!("Agent spawning is disabled in Ashide"));
    }
}
