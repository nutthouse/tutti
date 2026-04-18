//! OpenAI-compatible chat completions adapter.
//!
//! Covers:
//! - OpenAI (api.openai.com/v1)
//! - Azure OpenAI (resource.openai.azure.com with api-version + api-key header)
//! - Ollama (localhost:11434/v1, no auth)
//! - vLLM / Together / Groq / any OpenAI-compatible endpoint
//!
//! This is a sync adapter using `reqwest::blocking`. The agent loop
//! is sync in the MVP (decided in the eng review). Streaming and
//! async are deferred to post-spine.

use super::{
    ContentBlock, InferenceConfig, Message, ModelProvider, ModelResponse, Role, StopReason, Usage,
};
use crate::error::{Result, TuttiError};
use crate::tools::ToolDefinition;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Which wire-format variant this provider expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    /// Standard OpenAI-compatible (Bearer auth, POST /chat/completions).
    /// Covers openai.com, Ollama, vLLM, Together, Groq.
    OpenAI,
    /// Azure OpenAI: `api-key` header instead of Bearer, `api-version`
    /// query parameter on the URL.
    Azure,
}

#[derive(Debug, Clone)]
pub struct OpenAIConfig {
    pub kind: ProviderKind,
    pub name: String,
    /// Base URL. For OpenAI: "https://api.openai.com/v1".
    /// For Azure: "https://{resource}.openai.azure.com/openai/deployments/{model}".
    /// For Ollama: "http://localhost:11434/v1".
    pub base_url: String,
    /// API key. Empty string is allowed for Ollama and similar.
    pub api_key: String,
    /// Azure-only: the API version (e.g. "2024-10-21"). Ignored for OpenAI kind.
    pub api_version: Option<String>,
    /// Estimated context window in tokens.
    pub max_context_tokens: usize,
    pub cost_per_input_token: f64,
    pub cost_per_output_token: f64,
    /// Network timeout for a single request.
    pub request_timeout_secs: u64,
}

impl OpenAIConfig {
    pub fn openai(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        let _ = model; // model is set on the InferenceConfig, not here
        Self {
            kind: ProviderKind::OpenAI,
            name: "openai".into(),
            base_url: "https://api.openai.com/v1".into(),
            api_key: api_key.into(),
            api_version: None,
            max_context_tokens: 128_000,
            cost_per_input_token: 2.5 / 1_000_000.0,
            cost_per_output_token: 10.0 / 1_000_000.0,
            request_timeout_secs: 120,
        }
    }

    pub fn ollama(base_url: impl Into<String>) -> Self {
        Self {
            kind: ProviderKind::OpenAI,
            name: "ollama".into(),
            base_url: base_url.into(),
            api_key: String::new(),
            api_version: None,
            max_context_tokens: 32_000,
            cost_per_input_token: 0.0,
            cost_per_output_token: 0.0,
            request_timeout_secs: 300,
        }
    }
}

pub struct OpenAIProvider {
    config: OpenAIConfig,
    client: reqwest::blocking::Client,
}

impl OpenAIProvider {
    pub fn new(config: OpenAIConfig) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(config.request_timeout_secs))
            .build()
            .map_err(|e| TuttiError::ApiCallFailed(format!("failed to build HTTP client: {e}")))?;
        Ok(Self { config, client })
    }

    fn endpoint_url(&self, model: &str) -> String {
        match self.config.kind {
            ProviderKind::OpenAI => format!(
                "{}/chat/completions",
                self.config.base_url.trim_end_matches('/')
            ),
            ProviderKind::Azure => {
                let base = self.config.base_url.trim_end_matches('/');
                let api_version = self.config.api_version.as_deref().unwrap_or("2024-10-21");
                // Azure URL format: /openai/deployments/{deployment}/chat/completions?api-version=...
                // If the base_url already contains /deployments/, assume the caller
                // embedded the deployment name. Otherwise append it from `model`.
                if base.contains("/deployments/") {
                    format!("{base}/chat/completions?api-version={api_version}")
                } else {
                    format!(
                        "{base}/openai/deployments/{model}/chat/completions?api-version={api_version}"
                    )
                }
            }
        }
    }
}

impl ModelProvider for OpenAIProvider {
    fn name(&self) -> &str {
        &self.config.name
    }

    fn max_context_tokens(&self) -> usize {
        self.config.max_context_tokens
    }

    fn cost_per_input_token(&self) -> f64 {
        self.config.cost_per_input_token
    }

    fn cost_per_output_token(&self) -> f64 {
        self.config.cost_per_output_token
    }

    fn send_message(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        config: &InferenceConfig,
    ) -> Result<ModelResponse> {
        let url = self.endpoint_url(&config.model);
        let wire_messages = messages
            .iter()
            .flat_map(messages_to_wire)
            .collect::<Vec<_>>();
        let wire_tools = if tools.is_empty() {
            None
        } else {
            Some(
                tools
                    .iter()
                    .map(|t| WireTool {
                        kind: "function",
                        function: WireFunctionDef {
                            name: t.name.clone(),
                            description: t.description.clone(),
                            parameters: t.parameters.clone(),
                        },
                    })
                    .collect::<Vec<_>>(),
            )
        };

        let body = WireRequest {
            model: config.model.clone(),
            messages: wire_messages,
            tools: wire_tools,
            max_tokens: config.max_tokens,
            temperature: config.temperature,
        };

        let mut request = self.client.post(&url).json(&body);
        request = match self.config.kind {
            ProviderKind::OpenAI => {
                if self.config.api_key.is_empty() {
                    request
                } else {
                    request.header("Authorization", format!("Bearer {}", self.config.api_key))
                }
            }
            ProviderKind::Azure => request.header("api-key", &self.config.api_key),
        };

        let response = request
            .send()
            .map_err(|e| TuttiError::ApiCallFailed(format!("request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().unwrap_or_else(|_| "(no body)".to_string());
            return Err(TuttiError::ApiCallFailed(format!(
                "HTTP {} from {}: {}",
                status.as_u16(),
                self.config.name,
                truncate(&body_text, 800)
            )));
        }

        // Cap response body at 16 MB. Check Content-Length first for
        // an early reject, then stream-read with take() so we never
        // allocate more than the cap even without Content-Length.
        const MAX_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;
        if let Some(len) = response.content_length()
            && len > MAX_RESPONSE_BYTES
        {
            return Err(TuttiError::ApiCallFailed(format!(
                "response Content-Length too large: {len} bytes (max {MAX_RESPONSE_BYTES})"
            )));
        }
        let mut body_bytes = Vec::new();
        use std::io::Read;
        response
            .take(MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut body_bytes)
            .map_err(|e| TuttiError::ApiCallFailed(format!("failed to read response body: {e}")))?;
        if body_bytes.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(TuttiError::ApiCallFailed(format!(
                "response body too large: {} bytes (max {MAX_RESPONSE_BYTES})",
                body_bytes.len()
            )));
        }
        let wire_response: WireResponse = serde_json::from_slice(&body_bytes)
            .map_err(|e| TuttiError::ApiCallFailed(format!("invalid JSON response: {e}")))?;

        parse_wire_response(wire_response)
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max).collect();
        format!("{truncated}... [truncated]")
    }
}

/// Translate one internal `Message` into one or more wire messages.
/// Tool results need to become separate `tool` role messages (OpenAI
/// requires one tool message per tool call).
fn messages_to_wire(msg: &Message) -> Vec<WireMessage> {
    // Split tool results into separate wire messages since OpenAI
    // expects one `role: tool` message per tool_call_id.
    if msg.role == Role::Tool || msg.role == Role::User {
        let mut out = Vec::new();
        let mut text_buf = String::new();
        for block in &msg.content {
            match block {
                ContentBlock::Text { text } => {
                    if !text_buf.is_empty() {
                        text_buf.push('\n');
                    }
                    text_buf.push_str(text);
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } => {
                    out.push(WireMessage {
                        role: "tool",
                        content: Some(content.clone()),
                        tool_calls: None,
                        tool_call_id: Some(tool_use_id.clone()),
                    });
                }
                ContentBlock::ToolUse { .. } => {
                    // Tool use blocks don't appear in user/tool messages
                    // in well-formed conversations. Skip silently.
                }
            }
        }
        if !text_buf.is_empty() {
            out.insert(
                0,
                WireMessage {
                    role: role_str(msg.role),
                    content: Some(text_buf),
                    tool_calls: None,
                    tool_call_id: None,
                },
            );
        }
        if out.is_empty() {
            out.push(WireMessage {
                role: role_str(msg.role),
                content: Some(String::new()),
                tool_calls: None,
                tool_call_id: None,
            });
        }
        return out;
    }

    // System or Assistant message. Assistant can mix text + tool_calls.
    let mut text_parts = Vec::new();
    let mut tool_calls: Vec<WireToolCall> = Vec::new();
    for block in &msg.content {
        match block {
            ContentBlock::Text { text } => text_parts.push(text.clone()),
            ContentBlock::ToolUse { id, name, input } => {
                tool_calls.push(WireToolCall {
                    id: id.clone(),
                    kind: "function".to_string(),
                    function: WireToolCallFunction {
                        name: name.clone(),
                        arguments: serde_json::to_string(input).unwrap_or_default(),
                    },
                });
            }
            ContentBlock::ToolResult { .. } => {
                // Tool results don't belong in system/assistant messages.
            }
        }
    }

    let content = if text_parts.is_empty() {
        None
    } else {
        Some(text_parts.join("\n"))
    };

    vec![WireMessage {
        role: role_str(msg.role),
        content,
        tool_calls: if tool_calls.is_empty() {
            None
        } else {
            Some(tool_calls)
        },
        tool_call_id: None,
    }]
}

fn role_str(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

fn parse_wire_response(response: WireResponse) -> Result<ModelResponse> {
    let first_choice = response
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| TuttiError::ApiCallFailed("response had no choices".to_string()))?;

    let mut blocks: Vec<ContentBlock> = Vec::new();
    if let Some(text) = first_choice.message.content
        && !text.is_empty()
    {
        blocks.push(ContentBlock::Text { text });
    }
    if let Some(tool_calls) = first_choice.message.tool_calls {
        for call in tool_calls {
            let input = serde_json::from_str::<serde_json::Value>(&call.function.arguments)
                .map_err(|e| {
                    TuttiError::ApiCallFailed(format!(
                        "tool call '{}' has malformed arguments: {e}",
                        call.function.name
                    ))
                })?;
            blocks.push(ContentBlock::ToolUse {
                id: call.id,
                name: call.function.name,
                input,
            });
        }
    }

    let stop_reason = match first_choice.finish_reason.as_deref() {
        Some("stop") => StopReason::EndTurn,
        Some("tool_calls") => StopReason::ToolUse,
        Some("length") => StopReason::MaxTokens,
        _ => StopReason::Other,
    };

    let usage = response
        .usage
        .map(|u| Usage {
            input_tokens: u.prompt_tokens,
            output_tokens: u.completion_tokens,
        })
        .unwrap_or_default();

    Ok(ModelResponse {
        content: blocks,
        stop_reason,
        usage,
    })
}

// ---------- wire format structs ----------

#[derive(Debug, Serialize)]
struct WireRequest {
    model: String,
    messages: Vec<WireMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<WireTool>>,
    max_tokens: u32,
    temperature: f32,
}

#[derive(Debug, Serialize, Deserialize)]
struct WireMessage {
    role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    tool_calls: Option<Vec<WireToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    tool_call_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct WireToolCall {
    id: String,
    #[serde(rename = "type", default = "default_tool_kind")]
    kind: String,
    function: WireToolCallFunction,
}

fn default_tool_kind() -> String {
    "function".to_string()
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct WireToolCallFunction {
    name: String,
    /// OpenAI sends `arguments` as a JSON-encoded string, not an object.
    arguments: String,
}

#[derive(Debug, Serialize)]
struct WireTool {
    #[serde(rename = "type")]
    kind: &'static str,
    function: WireFunctionDef,
}

#[derive(Debug, Serialize)]
struct WireFunctionDef {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct WireResponse {
    choices: Vec<WireChoice>,
    #[serde(default)]
    usage: Option<WireUsage>,
}

#[derive(Debug, Deserialize)]
struct WireChoice {
    message: WireResponseMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireResponseMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<WireToolCall>>,
}

#[derive(Debug, Deserialize)]
struct WireUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{Message, Role};

    #[test]
    fn openai_endpoint_uses_base_plus_chat_completions() {
        let provider = OpenAIProvider::new(OpenAIConfig::openai("sk-test", "gpt-4o")).unwrap();
        let url = provider.endpoint_url("gpt-4o");
        assert_eq!(url, "https://api.openai.com/v1/chat/completions");
    }

    #[test]
    fn azure_endpoint_includes_api_version_and_deployment() {
        let config = OpenAIConfig {
            kind: ProviderKind::Azure,
            name: "azure".into(),
            base_url: "https://example.openai.azure.com".into(),
            api_key: "key".into(),
            api_version: Some("2024-10-21".into()),
            max_context_tokens: 128_000,
            cost_per_input_token: 0.0,
            cost_per_output_token: 0.0,
            request_timeout_secs: 60,
        };
        let provider = OpenAIProvider::new(config).unwrap();
        let url = provider.endpoint_url("gpt-4o");
        assert!(url.contains("/openai/deployments/gpt-4o/chat/completions"));
        assert!(url.contains("api-version=2024-10-21"));
    }

    #[test]
    fn azure_endpoint_respects_preembedded_deployment() {
        let config = OpenAIConfig {
            kind: ProviderKind::Azure,
            name: "azure".into(),
            base_url: "https://example.openai.azure.com/openai/deployments/my-deploy".into(),
            api_key: "key".into(),
            api_version: Some("2024-10-21".into()),
            max_context_tokens: 128_000,
            cost_per_input_token: 0.0,
            cost_per_output_token: 0.0,
            request_timeout_secs: 60,
        };
        let provider = OpenAIProvider::new(config).unwrap();
        let url = provider.endpoint_url("gpt-4o");
        assert!(url.contains("/deployments/my-deploy/chat/completions"));
        // Should not double-append gpt-4o since deployment is pre-embedded.
        assert!(!url.contains("deployments/my-deploy/gpt-4o"));
    }

    #[test]
    fn messages_to_wire_preserves_role() {
        let msg = Message::system("you are a helpful agent");
        let wire = messages_to_wire(&msg);
        assert_eq!(wire.len(), 1);
        assert_eq!(wire[0].role, "system");
        assert_eq!(wire[0].content, Some("you are a helpful agent".to_string()));
    }

    #[test]
    fn messages_to_wire_splits_tool_results() {
        let msg = Message {
            role: Role::Tool,
            content: vec![
                ContentBlock::ToolResult {
                    tool_use_id: "call_a".into(),
                    content: "result A".into(),
                    is_error: false,
                },
                ContentBlock::ToolResult {
                    tool_use_id: "call_b".into(),
                    content: "result B".into(),
                    is_error: false,
                },
            ],
        };
        let wire = messages_to_wire(&msg);
        assert_eq!(wire.len(), 2);
        assert_eq!(wire[0].role, "tool");
        assert_eq!(wire[0].tool_call_id, Some("call_a".to_string()));
        assert_eq!(wire[1].tool_call_id, Some("call_b".to_string()));
    }

    #[test]
    fn assistant_with_tool_use_emits_tool_calls() {
        let msg = Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Text {
                    text: "let me check".into(),
                },
                ContentBlock::ToolUse {
                    id: "call_1".into(),
                    name: "read_file".into(),
                    input: serde_json::json!({"path": "x"}),
                },
            ],
        };
        let wire = messages_to_wire(&msg);
        assert_eq!(wire.len(), 1);
        assert_eq!(wire[0].role, "assistant");
        assert_eq!(wire[0].content, Some("let me check".to_string()));
        let calls = wire[0].tool_calls.as_ref().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].function.name, "read_file");
        // Arguments must be JSON-encoded string per OpenAI spec.
        assert!(calls[0].function.arguments.contains("\"path\""));
    }

    #[test]
    fn parse_wire_response_text_only() {
        let wire = WireResponse {
            choices: vec![WireChoice {
                message: WireResponseMessage {
                    content: Some("hello".into()),
                    tool_calls: None,
                },
                finish_reason: Some("stop".into()),
            }],
            usage: Some(WireUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
            }),
        };
        let parsed = parse_wire_response(wire).unwrap();
        assert_eq!(parsed.stop_reason, StopReason::EndTurn);
        assert_eq!(parsed.usage.input_tokens, 10);
        assert_eq!(parsed.usage.output_tokens, 5);
        assert_eq!(parsed.content.len(), 1);
        match &parsed.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "hello"),
            _ => panic!("expected text block"),
        }
    }

    #[test]
    fn parse_wire_response_tool_call() {
        let wire = WireResponse {
            choices: vec![WireChoice {
                message: WireResponseMessage {
                    content: None,
                    tool_calls: Some(vec![WireToolCall {
                        id: "call_abc".into(),
                        kind: "function".into(),
                        function: WireToolCallFunction {
                            name: "read_file".into(),
                            arguments: "{\"path\":\"src/main.rs\"}".into(),
                        },
                    }]),
                },
                finish_reason: Some("tool_calls".into()),
            }],
            usage: None,
        };
        let parsed = parse_wire_response(wire).unwrap();
        assert_eq!(parsed.stop_reason, StopReason::ToolUse);
        assert_eq!(parsed.content.len(), 1);
        match &parsed.content[0] {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "call_abc");
                assert_eq!(name, "read_file");
                assert_eq!(input["path"], "src/main.rs");
            }
            _ => panic!("expected tool_use block"),
        }
    }

    #[test]
    fn parse_wire_response_mixed_text_and_tool_call() {
        let wire = WireResponse {
            choices: vec![WireChoice {
                message: WireResponseMessage {
                    content: Some("I'll read it".into()),
                    tool_calls: Some(vec![WireToolCall {
                        id: "c1".into(),
                        kind: "function".into(),
                        function: WireToolCallFunction {
                            name: "read_file".into(),
                            arguments: "{}".into(),
                        },
                    }]),
                },
                finish_reason: Some("tool_calls".into()),
            }],
            usage: None,
        };
        let parsed = parse_wire_response(wire).unwrap();
        assert_eq!(parsed.content.len(), 2);
    }

    #[test]
    fn parse_wire_response_length_means_max_tokens() {
        let wire = WireResponse {
            choices: vec![WireChoice {
                message: WireResponseMessage {
                    content: Some("truncated".into()),
                    tool_calls: None,
                },
                finish_reason: Some("length".into()),
            }],
            usage: None,
        };
        assert_eq!(
            parse_wire_response(wire).unwrap().stop_reason,
            StopReason::MaxTokens
        );
    }

    #[test]
    fn parse_wire_response_empty_choices_errors() {
        let wire = WireResponse {
            choices: vec![],
            usage: None,
        };
        assert!(parse_wire_response(wire).is_err());
    }
}
