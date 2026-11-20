//! The only non-bootstrap adapter allowed to depend directly on Codex
//! app-server DTOs.
//!
//! Polar orchestration imports these names through this module so routine
//! upstream DTO churn stays inside one declared compatibility unit.
//! Codex optional project, trigger, tool-output, Daybreak and turn-tier settings
//! remain unset: upgrading the adapter must not opt Polar into new behavior.


pub(crate) use codex_app_server_client::TypedRequestError;

pub(crate) use codex_app_server_protocol::{
    AskForApproval, ClientRequest, CommandExecutionApprovalDecision,
    CommandExecutionRequestApprovalParams, CommandExecutionRequestApprovalResponse,
    DynamicToolCallOutputContentItem, DynamicToolCallResponse,
    DynamicToolFunctionSpec, DynamicToolSpec, ErrorNotification, FileChangeApprovalDecision,
    FileChangeRequestApprovalParams, FileChangeRequestApprovalResponse, ItemCompletedNotification,
    ItemStartedNotification, JSONRPCErrorError, ModelListParams, ModelListResponse, RequestId,
    SandboxMode, SandboxPolicy, ServerNotification, ServerRequest, ThreadItem, ThreadItemPayload, ThreadReadParams,
    ThreadReadResponse, ThreadResumeParams, ThreadStartParams, ThreadStartResponse,
    ToolRequestUserInputResponse, Turn, TurnError, TurnInterruptParams, TurnInterruptResponse,
    TurnPlanStep, TurnPlanStepStatus, TurnStartParams, TurnStartResponse, TurnStatus, UserInput,
};
pub(crate) use codex_protocol::models::{ContentItem, ResponseItem};
pub(crate) use codex_protocol::openai_models::ReasoningEffort;

use polar_inhouse_contract::InhouseDynamicToolSpec;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;

pub(crate) fn lifecycle_request(method: &str, params: Value) -> anyhow::Result<ClientRequest> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let request_id = RequestId::String(format!(
        "polar-lifecycle-{}",
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    Ok(match method {
        "thread/read" => ClientRequest::ThreadRead {
            request_id,
            params: serde_json::from_value(params)?,
        },
        "thread/resume" => ClientRequest::ThreadResume {
            request_id,
            params: serde_json::from_value(params)?,
        },
        "thread/unsubscribe" => ClientRequest::ThreadUnsubscribe {
            request_id,
            params: serde_json::from_value(params)?,
        },
        "thread/inject_items" => ClientRequest::ThreadInjectItems {
            request_id,
            params: serde_json::from_value(params)?,
        },
        _ => anyhow::bail!("Unsupported Codex lifecycle request: {method}"),
    })
}

/// Native account protocol only; Polar never handles access/refresh tokens.
pub(crate) async fn account_operation(
    client: &crate::shared_client::ThreadClient,
    action: polar_inhouse_contract::InhouseBackendAuthAction,
) -> anyhow::Result<polar_inhouse_contract::InhouseBackendAuthView> {
    use codex_app_server_protocol::{
        Account, CancelLoginAccountParams, CancelLoginAccountResponse, CancelLoginAccountStatus,
        GetAccountParams, GetAccountResponse, LoginAccountParams, LoginAccountResponse,
        LogoutAccountResponse,
    };
    use polar_inhouse_contract::{
        InhouseBackendAuthAction as Action, InhouseBackendAuthView as View,
    };
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let request_id = RequestId::String(format!(
        "polar-account-{}",
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    Ok(match action {
        polar_inhouse_contract::InhouseBackendAuthAction::Import { .. } => {
            anyhow::bail!("offline auth import must use the profile registry")
        }
        Action::BeginBrowser | Action::BeginDevice => {
            let params = if action == Action::BeginDevice {
                LoginAccountParams::ChatgptDeviceCode
            } else {
                LoginAccountParams::Chatgpt {
                    codex_streamlined_login: false,
                    use_hosted_login_success_page: false,
                    app_brand: None,
                }
            };
            let result: LoginAccountResponse = client
                .request_typed(ClientRequest::LoginAccount { request_id, params })
                .await?;
            match result {
                LoginAccountResponse::Chatgpt { login_id, auth_url } => View::LoginRequired {
                    login_id,
                    url: auth_url,
                    user_code: None,
                },
                LoginAccountResponse::ChatgptDeviceCode {
                    login_id,
                    verification_url,
                    user_code,
                } => View::LoginRequired {
                    login_id,
                    url: verification_url,
                    user_code: Some(user_code),
                },
                _ => anyhow::bail!("Unexpected native Codex OAuth response"),
            }
        }
        Action::Read => {
            let result: GetAccountResponse = client
                .request_typed(ClientRequest::GetAccount {
                    request_id,
                    params: GetAccountParams {
                        refresh_token: false,
                    },
                })
                .await?;
            let (kind, email) = match result.account {
                None => (None, None),
                Some(Account::Chatgpt { email, .. }) => (Some("chatgpt".to_string()), email),
                Some(Account::ApiKey {}) => (Some("api_key".to_string()), None),
                Some(Account::AmazonBedrock { .. }) => (Some("amazon_bedrock".to_string()), None),
                Some(Account::Gcp {}) => (Some("gcp".to_string()), None),
            };
            View::Account {
                kind,
                email,
                requires_auth: result.requires_openai_auth,
            }
        }
        Action::Cancel { login_id } => {
            let result: CancelLoginAccountResponse = client
                .request_typed(ClientRequest::CancelLoginAccount {
                    request_id,
                    params: CancelLoginAccountParams { login_id },
                })
                .await?;
            View::Cancelled {
                found: result.status == CancelLoginAccountStatus::Canceled,
            }
        }
        Action::Logout => {
            let _: LogoutAccountResponse = client
                .request_typed(ClientRequest::LogoutAccount {
                    request_id,
                    params: None,
                })
                .await?;
            View::LoggedOut
        }
    })
}

fn dynamic_tool_spec(spec: InhouseDynamicToolSpec) -> DynamicToolSpec {
    DynamicToolSpec::Function(DynamicToolFunctionSpec {
        name: spec.name,
        description: spec.description,
        input_schema: spec.input_schema,
        defer_loading: spec.defer_loading,
    })
}

pub(crate) struct ThreadStartInput {
    pub(crate) model: Option<String>,
    pub(crate) model_provider: Option<String>,
    pub(crate) cwd: Option<String>,
    pub(crate) approval_policy: Option<AskForApproval>,
    pub(crate) sandbox: Option<SandboxMode>,
    pub(crate) config: Option<HashMap<String, Value>>,
    pub(crate) base_instructions: Option<String>,
    pub(crate) developer_instructions: Option<String>,
    pub(crate) dynamic_tools: Option<Vec<InhouseDynamicToolSpec>>,
}

pub(crate) fn thread_start_params(input: ThreadStartInput) -> ThreadStartParams {
    ThreadStartParams {
        model: input.model,
        model_provider: input.model_provider,
        allow_provider_model_fallback: false,
        service_tier: None,
        cwd: input.cwd,
        runtime_workspace_roots: None,
        approval_policy: input.approval_policy,
        approvals_reviewer: None,
        sandbox: input.sandbox,
        permissions: None,
        config: input.config,
        service_name: None,
        base_instructions: input.base_instructions,
        developer_instructions: input.developer_instructions,
        personality: None,
        multi_agent_mode: None,
        ephemeral: Some(false),
        history_mode: None,
        session_start_source: None,
        thread_source: None,
        project_id: None,
        daybreak_enabled: None,
        environments: Some(Vec::new()),
        dynamic_tools: input
            .dynamic_tools
            .map(|specs| specs.into_iter().map(dynamic_tool_spec).collect()),
        selected_capability_roots: None,
        mock_experimental_field: None,
        experimental_raw_events: false,
    }
}

pub(crate) struct ThreadResumeInput {
    pub(crate) thread_id: String,
    pub(crate) model: Option<String>,
    pub(crate) model_provider: Option<String>,
    pub(crate) cwd: Option<String>,
    pub(crate) approval_policy: Option<AskForApproval>,
    pub(crate) sandbox: Option<SandboxMode>,
    pub(crate) config: Option<HashMap<String, Value>>,
    pub(crate) base_instructions: Option<String>,
    pub(crate) developer_instructions: Option<String>,
}

pub(crate) fn thread_resume_params(input: ThreadResumeInput) -> ThreadResumeParams {
    ThreadResumeParams {
        thread_id: input.thread_id,
        history: None,
        path: None,
        model: input.model,
        model_provider: input.model_provider,
        service_tier: None,
        cwd: input.cwd,
        runtime_workspace_roots: None,
        approval_policy: input.approval_policy,
        approvals_reviewer: None,
        sandbox: input.sandbox,
        permissions: None,
        config: input.config,
        base_instructions: input.base_instructions,
        developer_instructions: input.developer_instructions,
        personality: None,
        exclude_turns: false,
        initial_turns_page: None,
    }
}

pub(crate) struct TurnStartInput {
    pub(crate) thread_id: String,
    pub(crate) input: Vec<UserInput>,
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) approval_policy: Option<AskForApproval>,
    pub(crate) sandbox_policy: Option<SandboxPolicy>,
    pub(crate) model: Option<String>,
    pub(crate) effort: Option<ReasoningEffort>,
}

pub(crate) fn turn_start_params(input: TurnStartInput) -> TurnStartParams {
    TurnStartParams {
        thread_id: input.thread_id,
        client_user_message_id: None,
        disabled_plugin_ids: None,
        turn_trigger: None,
        parent_turn_id: None,
        root_turn_id: None,
        tool_output: None,
        input: input.input,
        responsesapi_client_metadata: None,
        responsesapi_client_metadata_patch: None,
        additional_context: None,
        environments: Some(Vec::new()),
        cwd: input.cwd,
        runtime_workspace_roots: None,
        approval_policy: input.approval_policy,
        approvals_reviewer: None,
        sandbox_policy: input.sandbox_policy,
        permissions: None,
        model: input.model,
        service_tier: None,
        effort: input.effort,
        service_tier_for_turn: None,
        cyber_access_program: None,
        summary: None,
        personality: None,
        output_schema: None,
        collaboration_mode: None,
        multi_agent_mode: None,
    }
}

pub(crate) fn command_execution_cwd(
    params: &CommandExecutionRequestApprovalParams,
) -> Option<PathBuf> {
    params
        .cwd
        .as_ref()
        .map(|cwd| PathBuf::from(cwd.render_for_ui()))
}

/// Preserve the inline image wire shape while isolating upstream DTO changes.
pub(crate) fn inline_image_input(url: String) -> UserInput {
    UserInput::Image {
        image: codex_app_server_protocol::ImageReference::Inline { url },
        detail: None,
    }
}


