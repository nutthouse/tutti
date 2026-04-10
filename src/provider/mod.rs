#![allow(dead_code)]
//! Model provider adapters — the API-direct path that calls model
//! APIs directly instead of shelling out to CLI tools.
//!
//! The `ModelProvider` trait abstracts over any chat-completion-style
//! endpoint. For the MVP, we ship one adapter that speaks the OpenAI
//! wire format, covering OpenAI itself, Azure OpenAI (with a branch
//! for the auth/URL quirks), Ollama, vLLM, Together, Groq, and
//! anything else that exposes an OpenAI-compatible `/chat/completions`
//! endpoint.
//!
//! The internal message format uses `Vec<ContentBlock>` so a single
//! message can carry mixed text and tool_use blocks. This lets us
//! translate bidirectionally between OpenAI's `tool_calls` array
//! format and Anthropic's block-based format without a future refactor.

use crate::error::Result;
use crate::tools::ToolDefinition;
use serde::{Deserialize, Serialize};

pub mod openai;

/// A single conversation message. `content` is a vector so a message
/// can contain multiple blocks (text + tool_use, for example).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// A block inside a message. OpenAI returns a message with `content`
/// (text) and `tool_calls` (array); we normalize both into blocks.
/// Anthropic's format is already block-based. Tool results go in a
/// `user` role message on Anthropic and a `tool` role on OpenAI — the
/// adapter handles that translation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },
}

impl Message {
    pub fn system(text: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }
}

/// Model response after a single `send_message` call.
#[derive(Debug, Clone)]
pub struct ModelResponse {
    /// Blocks the model emitted — any combination of text and tool_use.
    pub content: Vec<ContentBlock>,
    pub stop_reason: StopReason,
    pub usage: Usage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The model finished normally without requesting more tools.
    EndTurn,
    /// The model emitted tool_use blocks and expects tool results.
    ToolUse,
    /// Hit the max_tokens limit before finishing.
    MaxTokens,
    /// The model stopped for some other reason (content filter, etc.).
    Other,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Inference configuration passed with every request.
#[derive(Debug, Clone)]
pub struct InferenceConfig {
    pub model: String,
    pub max_tokens: u32,
    pub temperature: f32,
}

impl Default for InferenceConfig {
    fn default() -> Self {
        Self {
            model: String::new(),
            max_tokens: 4096,
            temperature: 0.0,
        }
    }
}

/// Classify whether a provider error is worth retrying. Model-aware
/// retry only retries the API call itself, never re-executing tools
/// that have already run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryHint {
    /// Don't retry — this is a permanent failure (auth, invalid input,
    /// model not found).
    DoNotRetry,
    /// Transient — safe to retry with backoff (5xx, 429, network
    /// timeout).
    Transient,
}

/// Trait implemented by every model provider adapter.
pub trait ModelProvider: Send + Sync {
    /// Human-readable identifier for this provider instance.
    fn name(&self) -> &str;

    /// Maximum context window in tokens. Used to fail fast when a
    /// conversation would exceed it.
    fn max_context_tokens(&self) -> usize;

    /// Estimated USD cost per input token (for budget tracking).
    fn cost_per_input_token(&self) -> f64;

    /// Estimated USD cost per output token (for budget tracking).
    fn cost_per_output_token(&self) -> f64;

    /// Send messages to the model and get a response. This is sync
    /// by design — the MVP agent loop is sync, and streaming is a
    /// post-spine concern.
    fn send_message(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        config: &InferenceConfig,
    ) -> Result<ModelResponse>;
}

/// Classify whether an error returned from a provider is transient
/// and should be retried by the agent loop.
pub fn classify_retry(error: &crate::error::TuttiError) -> RetryHint {
    use crate::error::TuttiError;
    match error {
        TuttiError::ApiCallFailed(msg) => {
            let lower = msg.to_lowercase();
            // Transient signals: 5xx, 429, timeouts, network resets.
            // Status codes are anchored with "http " or "apierror: "
            // prefixes so we don't misclassify errors that happen to
            // contain "500" as a substring of something else.
            if lower.contains("timeout")
                || lower.contains("rate limit")
                || lower.contains("http 429")
                || lower.contains("apierror: 429")
                || lower.contains("http 500")
                || lower.contains("apierror: 500")
                || lower.contains("http 502")
                || lower.contains("apierror: 502")
                || lower.contains("http 503")
                || lower.contains("apierror: 503")
                || lower.contains("http 504")
                || lower.contains("apierror: 504")
                || lower.contains("connection reset")
                || lower.contains("temporarily unavailable")
                || lower.contains("service unavailable")
            {
                RetryHint::Transient
            } else {
                RetryHint::DoNotRetry
            }
        }
        _ => RetryHint::DoNotRetry,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::TuttiError;

    #[test]
    fn message_system_builds_text_block() {
        let msg = Message::system("hello");
        assert_eq!(msg.role, Role::System);
        assert_eq!(msg.content.len(), 1);
        match &msg.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "hello"),
            _ => panic!("expected text block"),
        }
    }

    #[test]
    fn classify_retry_treats_429_as_transient() {
        let err = TuttiError::ApiCallFailed("HTTP 429 rate limit exceeded".into());
        assert_eq!(classify_retry(&err), RetryHint::Transient);
    }

    #[test]
    fn classify_retry_treats_500_as_transient() {
        let err = TuttiError::ApiCallFailed("HTTP 503 service unavailable".into());
        assert_eq!(classify_retry(&err), RetryHint::Transient);
    }

    #[test]
    fn classify_retry_treats_auth_as_permanent() {
        let err = TuttiError::ApiCallFailed("HTTP 401 invalid api key".into());
        assert_eq!(classify_retry(&err), RetryHint::DoNotRetry);
    }

    #[test]
    fn classify_retry_treats_non_api_errors_as_permanent() {
        let err = TuttiError::PolicyViolation("not allowed".into());
        assert_eq!(classify_retry(&err), RetryHint::DoNotRetry);
    }

    #[test]
    fn content_block_serializes_with_type_tag() {
        let block = ContentBlock::ToolUse {
            id: "tool_1".into(),
            name: "read_file".into(),
            input: serde_json::json!({"path": "x"}),
        };
        let serialized = serde_json::to_string(&block).unwrap();
        assert!(serialized.contains("\"type\":\"tool_use\""));
        assert!(serialized.contains("\"id\":\"tool_1\""));
    }
}
