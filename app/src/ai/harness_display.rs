//! Shared display metadata for [`Harness`] variants.
//!
//! Any UI surface that shows a harness to the user — the harness selector
//! dropdown, the conversation details sidebar, etc. — should source its label,
//! icon, and brand color from here so the two surfaces cannot drift.

use warp_cli::agent::Harness;

use crate::ai::agent::conversation::AIAgentHarness;

/// Map [`AIAgentHarness`] (from `ServerAIConversationMetadata`) to the
/// canonical [`Harness`].
impl From<AIAgentHarness> for Harness {
    fn from(harness: AIAgentHarness) -> Self {
        match harness {
            AIAgentHarness::Oz => Harness::Oz,
            AIAgentHarness::ClaudeCode => Harness::Claude,
            AIAgentHarness::Unknown => Harness::Unknown,
        }
    }
}

impl PartialEq<Harness> for AIAgentHarness {
    fn eq(&self, other: &Harness) -> bool {
        Harness::from(*self) == *other
    }
}
