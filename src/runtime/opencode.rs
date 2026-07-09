use super::RuntimeConfig;

pub(super) static CONFIG: RuntimeConfig = RuntimeConfig {
    default_command: "opencode",
    // OpenCode's TUI accepts an initial prompt via `--prompt` (the positional
    // argument is a project path, not a prompt).
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
        "Working",
        "Running",
        "esc to interrupt",
    ],
    idle_patterns: &[
        "What would you like to do?",
        "How can I help",
        "opencode",
        "> ",
    ],
    completion_patterns: &["What would you like to do?", "How can I help"],
};
