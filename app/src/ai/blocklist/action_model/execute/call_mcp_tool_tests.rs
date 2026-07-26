//! Unit tests for MCP tool execution helpers.

use super::*;
use serde_json::json;
use std::borrow::Cow;
use std::time::Duration;

fn obj(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    match value {
        serde_json::Value::Object(m) => m,
        _ => panic!("expected a JSON object"),
    }
}

#[test]
fn whole_float_is_coerced_when_schema_declares_integer() {
    let mut args = obj(json!({ "line": 5.0 }));
    let schema = obj(json!({
        "properties": { "line": { "type": "integer" } }
    }));

    coerce_integer_args(&mut args, &schema);

    // Serialized as "5", not "5.0", and round-trips as i64.
    assert_eq!(serde_json::to_string(&args["line"]).unwrap(), "5");
    assert_eq!(args["line"].as_i64(), Some(5));
}

#[test]
fn no_coercion_when_not_typed_as_integer() {
    // Three scenarios that should all preserve the original float value:
    //   * schema declares `"type": "number"` (explicit float)
    //   * schema has no `properties` at all
    //   * schema property lacks a `"type"` key
    let cases = [
        json!({ "properties": { "x": { "type": "number" } } }),
        json!({}),
        json!({ "properties": { "x": { "description": "no type" } } }),
    ];

    for schema_value in cases {
        let mut args = obj(json!({ "x": 1.0 }));
        let schema = obj(schema_value);

        coerce_integer_args(&mut args, &schema);

        assert_eq!(args["x"].as_f64(), Some(1.0));
        assert_eq!(serde_json::to_string(&args["x"]).unwrap(), "1.0");
    }
}

fn failure_kind(
    res: Result<rmcp::model::CallToolResult, rmcp::ServiceError>,
) -> MCPToolCallFailureKind {
    match classify_call_tool_result(res) {
        MCPToolCallOutcome::Failure(failure) => failure.kind,
        MCPToolCallOutcome::Success { .. } => panic!("expected MCP tool call failure"),
    }
}

fn transport_send_error() -> rmcp::ServiceError {
    rmcp::ServiceError::TransportSend(rmcp::transport::DynamicTransportError {
        transport_name: Cow::Borrowed("test"),
        transport_type_id: std::any::TypeId::of::<()>(),
        error: Box::new(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "send failed",
        )),
    })
}

#[test]
fn classify_mcp_tool_returned_error_separately_from_transport_failures() {
    let result =
        rmcp::model::CallToolResult::error(vec![rmcp::model::Content::text("tool rejected args")]);
    match classify_call_tool_result(Ok(result)) {
        MCPToolCallOutcome::Failure(failure) => {
            assert_eq!(failure.kind, MCPToolCallFailureKind::ToolReturnedError);
            assert_eq!(failure.model_visible_message, "tool rejected args");
        }
        MCPToolCallOutcome::Success { .. } => panic!("expected tool-returned error"),
    }
}

#[test]
fn classify_mcp_service_errors_by_recovery_policy() {
    let server_error = rmcp::ServiceError::McpError(rmcp::model::ErrorData::new(
        rmcp::model::ErrorCode::INTERNAL_ERROR,
        "server exploded",
        None,
    ));

    assert_eq!(
        failure_kind(Err(server_error)),
        MCPToolCallFailureKind::ServerError
    );
    assert_eq!(
        failure_kind(Err(rmcp::ServiceError::TransportClosed)),
        MCPToolCallFailureKind::TransportClosed
    );
    assert_eq!(
        failure_kind(Err(transport_send_error())),
        MCPToolCallFailureKind::TransportSendFailed
    );
    assert_eq!(
        failure_kind(Err(rmcp::ServiceError::UnexpectedResponse)),
        MCPToolCallFailureKind::UnexpectedResponse
    );
    assert_eq!(
        failure_kind(Err(rmcp::ServiceError::Cancelled {
            reason: Some("user cancelled".to_owned())
        })),
        MCPToolCallFailureKind::Cancelled
    );
    assert_eq!(
        failure_kind(Err(rmcp::ServiceError::Timeout {
            timeout: Duration::from_secs(3)
        })),
        MCPToolCallFailureKind::Timeout
    );
}

#[test]
fn handle_mcp_transport_failure_remains_model_visible_error_result() {
    let result = handle_call_tool_result(Err(rmcp::ServiceError::TransportClosed));

    let AIAgentActionResultType::CallMCPTool(CallMCPToolResult::Error(message)) = result else {
        panic!("expected model-visible MCP tool error");
    };
    assert!(message.contains("Transport closed"));
}
