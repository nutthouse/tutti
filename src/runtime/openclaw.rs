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
};
