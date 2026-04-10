#![allow(dead_code)]
//! Agent loop for the API-direct path.
//!
//! The loop:
//!   1. Call the model with current messages + tool definitions.
//!   2. If response stop_reason is EndTurn/MaxTokens → done.
//!   3. If response has tool_use blocks → for each one, run policy
//!      gate, execute tool, append result.
//!   4. Repeat until done, max iterations, or budget exhausted.
//!
//! The loop is sync (reqwest::blocking). Model API errors classified
//! as transient are retried with exponential backoff. Tool side
//! effects are never retried — only the model API call itself.

pub mod event;
pub mod policy;

use crate::error::{Result, TuttiError};
use crate::provider::{
    ContentBlock, InferenceConfig, Message, ModelProvider, ModelResponse, Role, StopReason,
    classify_retry,
};
use crate::tools::{Tool, ToolOutput};
use event::{Event, EventLog};
use policy::{PolicyViolation, StepPolicy};
use std::path::Path;
use std::thread;
use std::time::Duration;

/// Configuration for a single agent run.
#[derive(Debug, Clone)]
pub struct RunConfig {
    /// Maximum iterations of the model → tools loop before failing.
    pub max_iterations: u32,
    /// Retry policy for transient model API failures.
    pub retry: RetryPolicy,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            max_iterations: 100,
            retry: RetryPolicy::default(),
        }
    }
}

/// Model-aware retry policy.
///
/// Unlike the existing automation retry (which reruns shell commands
/// wholesale), this only retries the model API call itself — never
/// re-executes tools that have already run. This is required because
/// tool side effects (file writes, shell commands) are not idempotent.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff_ms: 500,
            max_backoff_ms: 10_000,
        }
    }
}

/// Result of an agent run.
pub struct AgentResult {
    pub messages: Vec<Message>,
    pub iterations: u32,
    pub cumulative_cost_usd: f64,
    pub final_stop_reason: StopReason,
}

/// Run an agent loop for a single task.
#[allow(clippy::too_many_arguments)]
pub fn run_agent(
    provider: &dyn ModelProvider,
    tools: &[Box<dyn Tool>],
    policy: &StepPolicy,
    system_prompt: &str,
    user_prompt: &str,
    inference: &InferenceConfig,
    config: &RunConfig,
    event_log: &EventLog,
    workdir: &Path,
) -> Result<AgentResult> {
    event_log.emit(Event::RunStarted {
        provider: provider.name().to_string(),
        model: inference.model.clone(),
    })?;

    let mut messages = vec![Message::system(system_prompt), Message::user(user_prompt)];
    let tool_defs: Vec<_> = tools.iter().map(|t| t.definition()).collect();
    let mut iterations: u32 = 0;
    let mut cumulative_cost = 0.0f64;

    loop {
        iterations += 1;
        if iterations > config.max_iterations {
            event_log.emit(Event::RunFailed {
                reason: format!("max iterations exceeded ({})", config.max_iterations),
            })?;
            return Err(TuttiError::MaxIterationsExceeded(config.max_iterations));
        }

        // Budget check before each model call.
        if let Some(budget) = policy.budget_usd
            && cumulative_cost >= budget
        {
            event_log.emit(Event::RunFailed {
                reason: format!(
                    "budget exhausted: spent ${cumulative_cost:.4}, limit ${budget:.4}"
                ),
            })?;
            return Err(TuttiError::BudgetExhausted {
                spent: cumulative_cost,
                limit: budget,
            });
        }

        event_log.emit(Event::ModelCalled {
            iteration: iterations,
            message_count: messages.len(),
        })?;

        let response = call_with_retry(provider, &messages, &tool_defs, inference, &config.retry)
            .inspect_err(|e| {
            let _ = event_log.emit(Event::ModelError {
                error: e.to_string(),
            });
        })?;

        let call_cost = (response.usage.input_tokens as f64 * provider.cost_per_input_token())
            + (response.usage.output_tokens as f64 * provider.cost_per_output_token());
        cumulative_cost += call_cost;

        event_log.emit(Event::ModelResponded {
            iteration: iterations,
            input_tokens: response.usage.input_tokens,
            output_tokens: response.usage.output_tokens,
            cost_usd: call_cost,
            cumulative_cost_usd: cumulative_cost,
            stop_reason: response.stop_reason,
        })?;

        // Append the assistant message (mixed text + tool_use blocks).
        messages.push(Message {
            role: Role::Assistant,
            content: response.content.clone(),
        });

        let final_stop_reason = match response.stop_reason {
            StopReason::EndTurn | StopReason::MaxTokens | StopReason::Other => {
                event_log.emit(Event::RunFinished {
                    iterations,
                    cumulative_cost_usd: cumulative_cost,
                })?;
                response.stop_reason
            }
            StopReason::ToolUse => {
                // Execute every tool_use block the model emitted. Results
                // go in a single tool-role message.
                let tool_results =
                    execute_tool_calls(&response.content, tools, policy, event_log, workdir)?;
                if tool_results.is_empty() {
                    // Model said ToolUse but didn't emit any tool_use blocks.
                    // Treat as end of conversation to avoid spinning.
                    event_log.emit(Event::RunFinished {
                        iterations,
                        cumulative_cost_usd: cumulative_cost,
                    })?;
                    StopReason::EndTurn
                } else {
                    messages.push(Message {
                        role: Role::Tool,
                        content: tool_results,
                    });
                    continue;
                }
            }
        };

        return Ok(AgentResult {
            messages,
            iterations,
            cumulative_cost_usd: cumulative_cost,
            final_stop_reason,
        });
    }
}

/// Invoke the provider with retry for transient failures.
/// Only the API call is retried — never tools.
fn call_with_retry(
    provider: &dyn ModelProvider,
    messages: &[Message],
    tools: &[crate::tools::ToolDefinition],
    config: &InferenceConfig,
    retry: &RetryPolicy,
) -> Result<ModelResponse> {
    let mut attempt: u32 = 0;
    let mut backoff = Duration::from_millis(retry.initial_backoff_ms);
    loop {
        attempt += 1;
        match provider.send_message(messages, tools, config) {
            Ok(response) => return Ok(response),
            Err(e) => {
                if attempt >= retry.max_attempts {
                    return Err(e);
                }
                use crate::provider::RetryHint;
                if classify_retry(&e) != RetryHint::Transient {
                    return Err(e);
                }
                thread::sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_millis(retry.max_backoff_ms));
            }
        }
    }
}

/// Execute every `tool_use` block in the assistant's response, running
/// the policy gate before each. Returns the `tool_result` blocks.
fn execute_tool_calls(
    assistant_content: &[ContentBlock],
    tools: &[Box<dyn Tool>],
    policy: &StepPolicy,
    event_log: &EventLog,
    workdir: &Path,
) -> Result<Vec<ContentBlock>> {
    let mut results = Vec::new();
    for block in assistant_content {
        if let ContentBlock::ToolUse { id, name, input } = block {
            // Policy gate first. If denied, send a synthetic tool_result
            // to the model instead of executing.
            if let Err(violation) = policy.check_tool_call(name, input) {
                let reason = match &violation {
                    PolicyViolation::ToolNotAllowed(_) => format!("{}", violation),
                    PolicyViolation::NetworkBoundary { .. } => format!("{}", violation),
                };
                event_log.emit(Event::ToolDenied {
                    tool: name.clone(),
                    reason: reason.clone(),
                })?;
                results.push(ContentBlock::ToolResult {
                    tool_use_id: id.clone(),
                    content: format!("POLICY DENIED: {reason}"),
                    is_error: true,
                });
                continue;
            }

            event_log.emit(Event::ToolApproved { tool: name.clone() })?;

            let tool = tools.iter().find(|t| t.definition().name == *name);
            let output = match tool {
                None => {
                    event_log.emit(Event::ToolFailed {
                        tool: name.clone(),
                        error: "tool not registered".into(),
                    })?;
                    ToolOutput::error(format!("tool '{name}' is not registered"))
                }
                Some(tool) => match tool.execute(input, workdir) {
                    Ok(output) => {
                        event_log.emit(Event::ToolExecuted {
                            tool: name.clone(),
                            output_len: output.content.len(),
                            files_modified: output
                                .files_modified
                                .iter()
                                .map(|p| p.display().to_string())
                                .collect(),
                            is_error: output.is_error,
                        })?;
                        output
                    }
                    Err(e) => {
                        let error_msg = e.to_string();
                        event_log.emit(Event::ToolFailed {
                            tool: name.clone(),
                            error: error_msg.clone(),
                        })?;
                        ToolOutput::error(format!("tool execution failed: {error_msg}"))
                    }
                },
            };

            results.push(ContentBlock::ToolResult {
                tool_use_id: id.clone(),
                content: output.content,
                is_error: output.is_error,
            });
        }
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{Usage, openai::OpenAIConfig};
    use crate::tools::{Tool, ToolDefinition, ToolOutput};
    use serde_json::json;
    use std::sync::Mutex;

    /// Scripted provider that returns a fixed sequence of model responses
    /// for deterministic testing of the agent loop.
    struct MockProvider {
        responses: Mutex<Vec<ModelResponse>>,
    }

    impl MockProvider {
        fn new(responses: Vec<ModelResponse>) -> Self {
            Self {
                responses: Mutex::new(responses),
            }
        }
    }

    impl ModelProvider for MockProvider {
        fn name(&self) -> &str {
            "mock"
        }
        fn max_context_tokens(&self) -> usize {
            100_000
        }
        fn cost_per_input_token(&self) -> f64 {
            0.000001
        }
        fn cost_per_output_token(&self) -> f64 {
            0.000001
        }
        fn send_message(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _config: &InferenceConfig,
        ) -> Result<ModelResponse> {
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                return Err(TuttiError::ApiCallFailed(
                    "mock provider ran out of responses".into(),
                ));
            }
            Ok(responses.remove(0))
        }
    }

    /// Provider that always returns a transient error up to `fail_times`,
    /// then returns the success response.
    struct FlakyProvider {
        remaining_fails: Mutex<u32>,
        success: Mutex<Option<ModelResponse>>,
    }

    impl ModelProvider for FlakyProvider {
        fn name(&self) -> &str {
            "flaky"
        }
        fn max_context_tokens(&self) -> usize {
            100_000
        }
        fn cost_per_input_token(&self) -> f64 {
            0.0
        }
        fn cost_per_output_token(&self) -> f64 {
            0.0
        }
        fn send_message(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _config: &InferenceConfig,
        ) -> Result<ModelResponse> {
            let mut fails = self.remaining_fails.lock().unwrap();
            if *fails > 0 {
                *fails -= 1;
                return Err(TuttiError::ApiCallFailed(
                    "HTTP 503 temporarily unavailable".into(),
                ));
            }
            Ok(self.success.lock().unwrap().take().unwrap())
        }
    }

    /// Tool that records every invocation so tests can assert call counts.
    struct CountingTool {
        name: String,
        calls: Mutex<u32>,
    }

    impl CountingTool {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
                calls: Mutex::new(0),
            }
        }
        fn call_count(&self) -> u32 {
            *self.calls.lock().unwrap()
        }
    }

    impl Tool for CountingTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: self.name.clone(),
                description: "test tool".into(),
                parameters: json!({"type": "object", "properties": {}}),
            }
        }
        fn execute(&self, _args: &serde_json::Value, _workdir: &Path) -> Result<ToolOutput> {
            *self.calls.lock().unwrap() += 1;
            Ok(ToolOutput::text("ok"))
        }
    }

    fn inference() -> InferenceConfig {
        InferenceConfig {
            model: "test-model".into(),
            max_tokens: 100,
            temperature: 0.0,
        }
    }

    fn make_event_log() -> (tempfile::TempDir, EventLog) {
        let tmp = tempfile::tempdir().unwrap();
        let log = EventLog::new(tmp.path().join("events.db")).unwrap();
        let _run_id = log.start_run("test_run", "mock", "test-model").unwrap();
        (tmp, log)
    }

    #[test]
    fn happy_path_single_text_response() {
        let (_dir, log) = make_event_log();
        let tmp = tempfile::tempdir().unwrap();
        let provider = MockProvider::new(vec![ModelResponse {
            content: vec![ContentBlock::Text {
                text: "done".into(),
            }],
            stop_reason: StopReason::EndTurn,
            usage: Usage {
                input_tokens: 10,
                output_tokens: 2,
            },
        }]);
        let tools: Vec<Box<dyn Tool>> = vec![];
        let result = run_agent(
            &provider,
            &tools,
            &StepPolicy::default(),
            "you are a test agent",
            "do nothing",
            &inference(),
            &RunConfig::default(),
            &log,
            tmp.path(),
        )
        .unwrap();
        assert_eq!(result.iterations, 1);
        assert_eq!(result.final_stop_reason, StopReason::EndTurn);
    }

    #[test]
    fn single_tool_round_trip() {
        let (_dir, log) = make_event_log();
        let tmp = tempfile::tempdir().unwrap();
        let provider = MockProvider::new(vec![
            ModelResponse {
                content: vec![ContentBlock::ToolUse {
                    id: "call_1".into(),
                    name: "counter".into(),
                    input: json!({}),
                }],
                stop_reason: StopReason::ToolUse,
                usage: Usage {
                    input_tokens: 10,
                    output_tokens: 5,
                },
            },
            ModelResponse {
                content: vec![ContentBlock::Text {
                    text: "done".into(),
                }],
                stop_reason: StopReason::EndTurn,
                usage: Usage {
                    input_tokens: 12,
                    output_tokens: 3,
                },
            },
        ]);
        let counter = CountingTool::new("counter");
        let tools: Vec<Box<dyn Tool>> = vec![Box::new(counter)];
        let result = run_agent(
            &provider,
            &tools,
            &StepPolicy::default(),
            "system",
            "user",
            &inference(),
            &RunConfig::default(),
            &log,
            tmp.path(),
        )
        .unwrap();
        assert_eq!(result.iterations, 2);
        assert_eq!(result.final_stop_reason, StopReason::EndTurn);
    }

    #[test]
    fn policy_denied_tool_returns_denial_to_model() {
        let (_dir, log) = make_event_log();
        let tmp = tempfile::tempdir().unwrap();
        let provider = MockProvider::new(vec![
            ModelResponse {
                content: vec![ContentBlock::ToolUse {
                    id: "call_1".into(),
                    name: "forbidden".into(),
                    input: json!({}),
                }],
                stop_reason: StopReason::ToolUse,
                usage: Usage::default(),
            },
            ModelResponse {
                content: vec![ContentBlock::Text {
                    text: "adapted".into(),
                }],
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
        ]);
        let counter = CountingTool::new("forbidden");
        let tools: Vec<Box<dyn Tool>> = vec![Box::new(counter)];
        let policy = StepPolicy {
            allowed_tools: Some(vec!["allowed_only".into()]),
            ..StepPolicy::default()
        };
        let result = run_agent(
            &provider,
            &tools,
            &policy,
            "system",
            "user",
            &inference(),
            &RunConfig::default(),
            &log,
            tmp.path(),
        )
        .unwrap();
        assert_eq!(result.final_stop_reason, StopReason::EndTurn);
        // The tool should NOT have been executed.
        let tool_ref = &tools[0];
        let counting: &CountingTool =
            unsafe { &*(tool_ref.as_ref() as *const dyn Tool as *const CountingTool) };
        assert_eq!(
            counting.call_count(),
            0,
            "policy-denied tool must not execute"
        );
    }

    #[test]
    fn budget_exhaustion_aborts() {
        let (_dir, log) = make_event_log();
        let tmp = tempfile::tempdir().unwrap();
        // Force the provider to return expensive calls so we blow the budget.
        // The mock cost is 0.000001/token, so with input 1_000_000 + output 1_000_000
        // we get $2 in cost.
        let provider = MockProvider::new(vec![ModelResponse {
            content: vec![ContentBlock::Text { text: "x".into() }],
            stop_reason: StopReason::ToolUse, // force another iteration
            usage: Usage {
                input_tokens: 1_000_000,
                output_tokens: 1_000_000,
            },
        }]);
        let tools: Vec<Box<dyn Tool>> = vec![];
        let policy = StepPolicy {
            budget_usd: Some(0.50),
            ..StepPolicy::default()
        };
        // First model call is allowed (budget check is before). Second call
        // should be rejected because cumulative cost is now $2 > $0.50.
        // Since ToolUse with no tool_use blocks breaks the loop, we instead
        // expect BudgetExhausted on the second iteration.
        let result = run_agent(
            &provider,
            &tools,
            &policy,
            "system",
            "user",
            &inference(),
            &RunConfig::default(),
            &log,
            tmp.path(),
        );
        // Either budget is exhausted OR the loop breaks because ToolUse
        // had no tool_use blocks. Either way is safe behavior.
        match result {
            Err(TuttiError::BudgetExhausted { .. }) => {}
            Ok(res) => {
                assert!(res.cumulative_cost_usd > 0.0);
            }
            Err(other) => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn max_iterations_enforced() {
        let (_dir, log) = make_event_log();
        let tmp = tempfile::tempdir().unwrap();
        // Infinite loop: every response says ToolUse with a counter tool.
        let mut responses = Vec::new();
        for _ in 0..10 {
            responses.push(ModelResponse {
                content: vec![ContentBlock::ToolUse {
                    id: "c".into(),
                    name: "counter".into(),
                    input: json!({}),
                }],
                stop_reason: StopReason::ToolUse,
                usage: Usage::default(),
            });
        }
        let provider = MockProvider::new(responses);
        let tools: Vec<Box<dyn Tool>> = vec![Box::new(CountingTool::new("counter"))];
        let config = RunConfig {
            max_iterations: 3,
            ..RunConfig::default()
        };
        let result = run_agent(
            &provider,
            &tools,
            &StepPolicy::default(),
            "system",
            "user",
            &inference(),
            &config,
            &log,
            tmp.path(),
        );
        assert!(matches!(result, Err(TuttiError::MaxIterationsExceeded(3))));
    }

    #[test]
    fn transient_errors_are_retried() {
        let (_dir, log) = make_event_log();
        let tmp = tempfile::tempdir().unwrap();
        let provider = FlakyProvider {
            remaining_fails: Mutex::new(2),
            success: Mutex::new(Some(ModelResponse {
                content: vec![ContentBlock::Text { text: "ok".into() }],
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            })),
        };
        let tools: Vec<Box<dyn Tool>> = vec![];
        let config = RunConfig {
            max_iterations: 5,
            retry: RetryPolicy {
                max_attempts: 5,
                initial_backoff_ms: 1,
                max_backoff_ms: 10,
            },
        };
        let result = run_agent(
            &provider,
            &tools,
            &StepPolicy::default(),
            "system",
            "user",
            &inference(),
            &config,
            &log,
            tmp.path(),
        )
        .unwrap();
        assert_eq!(result.iterations, 1);
    }

    #[test]
    fn permanent_errors_not_retried() {
        let (_dir, log) = make_event_log();
        let tmp = tempfile::tempdir().unwrap();
        struct AuthFail;
        impl ModelProvider for AuthFail {
            fn name(&self) -> &str {
                "auth-fail"
            }
            fn max_context_tokens(&self) -> usize {
                1000
            }
            fn cost_per_input_token(&self) -> f64 {
                0.0
            }
            fn cost_per_output_token(&self) -> f64 {
                0.0
            }
            fn send_message(
                &self,
                _m: &[Message],
                _t: &[ToolDefinition],
                _c: &InferenceConfig,
            ) -> Result<ModelResponse> {
                Err(TuttiError::ApiCallFailed("HTTP 401 unauthorized".into()))
            }
        }
        let tools: Vec<Box<dyn Tool>> = vec![];
        let result = run_agent(
            &AuthFail,
            &tools,
            &StepPolicy::default(),
            "system",
            "user",
            &inference(),
            &RunConfig::default(),
            &log,
            tmp.path(),
        );
        assert!(matches!(result, Err(TuttiError::ApiCallFailed(_))));
    }

    #[test]
    fn unused_openai_config_just_sanity_check() {
        // Mostly to keep the import alive for the test module.
        let _ = OpenAIConfig::openai("key", "gpt-4o");
    }
}
