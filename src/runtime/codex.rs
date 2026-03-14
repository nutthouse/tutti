use super::RuntimeConfig;

pub(super) static CONFIG: RuntimeConfig = RuntimeConfig {
    default_command: "codex",
    prompt_flag: "--prompt",
    auth_patterns: &[
        "invalid_api_key",
        "Incorrect API key",
        "APIError: 401",
        "unauthorized",
        "authentication error",
    ],
    rate_limit_patterns: &[
        "rate_limit_exceeded",
        "insufficient_quota",
        "apierror: 429",
        "too many requests",
        "quota exceeded",
    ],
    provider_down_patterns: &[
        "service unavailable",
        "temporarily unavailable",
        "provider unavailable",
        "upstream timeout",
        "gateway timeout",
        "bad gateway",
        "connection reset",
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
    completion_patterns: &["What would you like", "How can I"],
};
