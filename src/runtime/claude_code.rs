use super::RuntimeConfig;

pub(super) static CONFIG: RuntimeConfig = RuntimeConfig {
    default_command: "claude",
    prompt_flag: "",
    auth_patterns: &[
        "invalid_api_key",
        "authentication_error",
        "token has expired",
        "unauthorized",
        "OAuth token",
        "session expired",
        "please log in",
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
        "Reading",
        "Writing",
        "Editing",
        "Running",
        "Searching",
    ],
    idle_patterns: &[
        "What would you like to do?",
        "How can I help",
        "> ",
        "Claude Code",
    ],
};
