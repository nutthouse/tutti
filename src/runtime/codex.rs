use super::RuntimeConfig;

pub(super) static CONFIG: RuntimeConfig = RuntimeConfig {
    default_command: "codex",
    prompt_flag: "--prompt",
    auth_patterns: &[
        "invalid_api_key",
        "Incorrect API key",
        "rate_limit_exceeded",
        "insufficient_quota",
        "APIError: 401",
        "APIError: 429",
    ],
    working_patterns: &[
        "⠋",
        "⠙",
        "⠹",
        "⠸",
        "⠼",
        "⠴",
        "⠦",
        "⠧",
        "⠇",
        "⠏", // spinner
        "Thinking",
        "Generating",
        "Running",
    ],
    idle_patterns: &[">", "What would you like", "How can I"],
};
