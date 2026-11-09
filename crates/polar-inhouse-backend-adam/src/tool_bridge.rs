use polar_inhouse_contract::{
    InhouseDynamicToolHandler, InhouseToolCallEnvelope, InhouseToolExecutionError,
    InhouseToolExecutionErrorKind, InhouseToolPlan, InhouseToolResultEnvelope,
};
use serde_json::{json, Value};

pub fn adam_provider_tool_schemas(plan: &InhouseToolPlan) -> Vec<Value> {
    plan.tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.input_schema,
                }
            })
        })
        .collect()
}

/// Project Adam's existing Chat-shaped function declarations onto Responses.
/// Non-strict mode preserves optional parameters in the domain tool schema.
pub(crate) fn adam_responses_tool_schemas(tools: &[Value]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            if tool.get("type").and_then(Value::as_str) != Some("function") {
                return tool.clone();
            }
            let mut projected = tool.clone();
            if let Some(function) = tool.get("function").and_then(Value::as_object) {
                projected = Value::Object(function.clone());
                projected["type"] = json!("function");
            }
            if projected.get("strict").is_none() {
                projected["strict"] = json!(false);
            }
            projected
        })
        .collect()
}

pub fn normalize_adam_provider_tool_call(
    plan: &InhouseToolPlan,
    wire_call_id: impl Into<String>,
    provider_call_id: Option<String>,
    tool_name: impl Into<String>,
    arguments: Value,
) -> anyhow::Result<InhouseToolCallEnvelope> {
    let tool_name = tool_name.into();
    if !plan.tools.iter().any(|tool| tool.name == tool_name) {
        anyhow::bail!("Adam provider requested undeclared tool `{tool_name}`");
    }
    let wire_call_id = wire_call_id.into();
    if wire_call_id.trim().is_empty() {
        anyhow::bail!("Adam wire tool call id must be nonblank");
    }
    if provider_call_id
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
    {
        anyhow::bail!("Adam provider tool call id must be nonblank when present");
    }
    Ok(InhouseToolCallEnvelope {
        call_id: wire_call_id,
        provider_call_id,
        tool_name,
        arguments,
    })
}

pub async fn execute_adam_tool_call(
    handler: &dyn InhouseDynamicToolHandler,
    call: &InhouseToolCallEnvelope,
) -> anyhow::Result<InhouseToolResultEnvelope> {
    match handler
        .handle_call(&call.tool_name, call.arguments.clone())
        .await
    {
        Ok(result_text) => Ok(InhouseToolResultEnvelope {
            call_id: call.call_id.clone(),
            tool_name: call.tool_name.clone(),
            arguments: call.arguments.clone(),
            result_text,
            ok: true,
            argument_correction_allowed: false,
            error_kind: None,
            retry_guidance: None,
        }),
        Err(error) => {
            let (kind, message, retry_guidance) = InhouseToolExecutionError::downcast_ref(&error)
                .map(|typed| {
                    (
                        typed.kind(),
                        typed.message().to_string(),
                        typed.retry_guidance().to_string(),
                    )
                })
                .unwrap_or_else(|| {
                    (
                        InhouseToolExecutionErrorKind::ExecutionFailed,
                        "The Polar tool did not complete.".to_string(),
                        "Do not repeat this operation automatically; its outcome may be uncertain. Inspect the correlated tool event.".to_string(),
                    )
                });
            Ok(InhouseToolResultEnvelope {
                call_id: call.call_id.clone(),
                tool_name: call.tool_name.clone(),
                arguments: call.arguments.clone(),
                result_text: message,
                ok: false,
                argument_correction_allowed: InhouseToolExecutionError::downcast_ref(&error)
                    .is_some_and(InhouseToolExecutionError::argument_correction_allowed),
                error_kind: Some(kind.as_str().to_string()),
                retry_guidance: Some(retry_guidance),
            })
        }
    }
}

/// Error data is a function result, never a developer instruction or assistant answer.
pub(crate) fn model_tool_result(result: &InhouseToolResultEnvelope) -> String {
    if result.ok {
        return result.result_text.clone();
    }
    json!({"ok":false,"error_kind":result.error_kind,"message":result.result_text,
        "retry_guidance":result.retry_guidance,"recovery":"correct_arguments",
        "repeat_identical_arguments":false})
    .to_string()
}
