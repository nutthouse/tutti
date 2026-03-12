use super::RuntimeConfig;

pub(super) static CONFIG: RuntimeConfig = RuntimeConfig {
    default_command: "aider",
    prompt_flag: "--message",
    auth_patterns: &[
        "No API key",
        "API key not found",
        "AuthenticationError",
        "RateLimitError",
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
};
