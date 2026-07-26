use crate::ai::byop_readiness::ToolCallRef;
use warp_multi_agent_api::{message, Message};

const MISSING_RESULT_CANCELLATION_PAYLOAD: &str = r#"{
    "status": "cancelled",
    "reason": "interrupted_by_user",
    "synthetic": true,
    "repair_source": "byop_missing_tool_result"
}"#;

pub(super) struct ByopToolResultRepair;

impl ByopToolResultRepair {
    pub(super) fn missing_result_cancellation_message(
        request_id: &str,
        tool_call: &ToolCallRef,
    ) -> Message {
        Message {
            id: uuid::Uuid::new_v4().to_string(),
            task_id: tool_call.key.task_id.clone(),
            server_message_data: MISSING_RESULT_CANCELLATION_PAYLOAD.to_string(),
            citations: vec![],
            message: Some(message::Message::ToolCallResult(message::ToolCallResult {
                tool_call_id: tool_call.key.tool_call_id.clone(),
                context: None,
                result: None,
            })),
            request_id: request_id.to_owned(),
            timestamp: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::byop_readiness::{RedactedToolKind, ToolCallKey};

    fn tool_call_ref() -> ToolCallRef {
        ToolCallRef::new(
            ToolCallKey::new("task-1", "assistant-1", "call-1"),
            RedactedToolKind::new("shell"),
        )
    }

    #[test]
    fn missing_result_cancellation_message_preserves_tool_call_pairing() {
        let message = ByopToolResultRepair::missing_result_cancellation_message(
            "request-1",
            &tool_call_ref(),
        );

        assert_eq!(message.task_id, "task-1");
        assert_eq!(message.request_id, "request-1");
        assert_eq!(message.citations.len(), 0);
        assert!(message.timestamp.is_none());
        if let message::Message::ToolCallResult(result) =
            message.message.expect("expected tool call result")
        {
            assert_eq!(result.tool_call_id, "call-1");
            assert!(result.context.is_none());
            assert!(result.result.is_none());
        } else {
            panic!("expected ToolCallResult");
        }
    }

    #[test]
    fn missing_result_cancellation_payload_is_diagnostic_and_model_visible() {
        let message = ByopToolResultRepair::missing_result_cancellation_message(
            "request-1",
            &tool_call_ref(),
        );
        let payload: serde_json::Value =
            serde_json::from_str(&message.server_message_data).expect("valid JSON payload");

        assert_eq!(payload["status"], "cancelled");
        assert_eq!(payload["reason"], "interrupted_by_user");
        assert_eq!(payload["synthetic"], true);
        assert_eq!(payload["repair_source"], "byop_missing_tool_result");
    }
}
