use super::RuntimeConfig;

pub(super) static CONFIG: RuntimeConfig = RuntimeConfig {
    default_command: "openclaw",
    prompt_flag: "--prompt",
    auth_patterns: &[
        "invalid_api_key",
        "authentication_error",
        "token has expired",
        "unauthorized",
        "not authenticated",
        "APIError: 401",
        "APIError: 403",
    ],
    rate_limit_patterns: &[
        "rate_limit_exceeded",
        "rate limit",
        "too many requests",
        "apierror: 429",
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
        "Planning",
    ],
    idle_patterns: &[
        "What would you like to do?",
        "How can I help",
        "openclaw>",
        "> ",
    ],
    completion_patterns: &["What would you like to do?", "How can I help", "openclaw>"],
};
