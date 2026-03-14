use super::RuntimeConfig;

pub(super) static CONFIG: RuntimeConfig = RuntimeConfig {
    default_command: "aider",
    prompt_flag: "--message",
    auth_patterns: &["No API key", "API key not found", "AuthenticationError"],
    rate_limit_patterns: &[
        "RateLimitError",
        "rate_limit_exceeded",
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
        "Applying",
        "Editing",
        "Committing",
    ],
    idle_patterns: &["aider>", "> ", "/help"],
    completion_patterns: &["aider>"],
};
