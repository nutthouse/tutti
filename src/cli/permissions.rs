use crate::cli::PermissionsSubcommand;
use crate::config::{GlobalConfig, PermissionsConfig, global_config_path};
use crate::error::{Result, TuttiError};
use serde_json::json;
use std::path::Path;

pub fn run(command: PermissionsSubcommand) -> Result<()> {
    match command {
        PermissionsSubcommand::Check { command } => run_check(&command),
        PermissionsSubcommand::Export { runtime, output } => {
            run_export(&runtime, output.as_deref())
        }
    }
}

fn run_check(parts: &[String]) -> Result<()> {
    let command_line = normalize(parts.join(" "));
    if command_line.is_empty() {
        return Err(TuttiError::ConfigValidation(
            "command cannot be empty".to_string(),
        ));
    }

    let global = GlobalConfig::load()?;
    let Some(policy) = global.permissions.as_ref() else {
        println!(
            "Permissions policy is not configured in {}; allowing command.",
            global_config_path().display()
        );
        println!("allowed: {command_line}");
        return Ok(());
    };

    if let Some(matched) = matching_allow_rule(policy, &command_line) {
        println!("allowed: {command_line}");
        println!("matched rule: {matched}");
        return Ok(());
    }

    Err(TuttiError::ConfigValidation(format!(
        "command blocked by permissions policy: '{command_line}'"
    )))
}

fn run_export(runtime: &str, output: Option<&Path>) -> Result<()> {
    let global = GlobalConfig::load()?;
    let policy = global.permissions.unwrap_or_default();
    let rendered = render_export(runtime, &policy)?;

    if let Some(path) = output {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let payload = format!("{rendered}\n");
        std::fs::write(path, payload)?;
        println!("wrote {}", path.display());
    } else {
        println!("{rendered}");
    }

    Ok(())
}

fn render_export(runtime: &str, policy: &PermissionsConfig) -> Result<String> {
    match normalize(runtime).to_ascii_lowercase().as_str() {
        "claude" | "claude-code" => render_claude_export(policy),
        other => Err(TuttiError::ConfigValidation(format!(
            "unsupported runtime '{}'; supported: claude",
            other
        ))),
    }
}

fn render_claude_export(policy: &PermissionsConfig) -> Result<String> {
    let allow: Vec<String> = policy
        .allow
        .iter()
        .map(normalize)
        .filter(|entry| !entry.is_empty())
        .map(|entry| format!("Bash({entry})"))
        .collect();

    let payload = json!({
        "permissions": {
            "allow": allow
        }
    });

    serde_json::to_string_pretty(&payload).map_err(Into::into)
}

fn matching_allow_rule<'a>(policy: &'a PermissionsConfig, command_line: &str) -> Option<&'a str> {
    for raw_rule in &policy.allow {
        let trimmed = raw_rule.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(prefix) = trimmed.strip_suffix('*') {
            let prefix = normalize(prefix);
            if !prefix.is_empty() && command_line.starts_with(&prefix) {
                return Some(trimmed);
            }
            continue;
        }

        let normalized_rule = normalize(trimmed);
        if normalized_rule.is_empty() {
            continue;
        }
        if command_line == normalized_rule {
            return Some(trimmed);
        }
        let mut bounded = normalized_rule;
        bounded.push(' ');
        if command_line.starts_with(&bounded) {
            return Some(trimmed);
        }
    }

    None
}

fn normalize<S: AsRef<str>>(input: S) -> String {
    input
        .as_ref()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_rule_supports_exact_and_prefix() {
        let policy = PermissionsConfig {
            allow: vec!["git status".to_string(), "cargo test".to_string()],
        };

        assert_eq!(
            matching_allow_rule(&policy, "git status"),
            Some("git status")
        );
        assert_eq!(
            matching_allow_rule(&policy, "cargo test --quiet"),
            Some("cargo test")
        );
        assert_eq!(matching_allow_rule(&policy, "git stash"), None);
    }

    #[test]
    fn matching_rule_supports_wildcard_prefix() {
        let policy = PermissionsConfig {
            allow: vec!["npm run *".to_string()],
        };

        assert_eq!(
            matching_allow_rule(&policy, "npm run build"),
            Some("npm run *")
        );
        assert_eq!(matching_allow_rule(&policy, "npm test"), None);
    }

    #[test]
    fn normalize_compacts_whitespace() {
        assert_eq!(normalize("  cargo   test   --all "), "cargo test --all");
    }

    #[test]
    fn render_claude_export_wraps_allow_entries_as_bash_permissions() {
        let policy = PermissionsConfig {
            allow: vec!["git status".to_string(), "cargo test --quiet".to_string()],
        };
        let rendered = render_claude_export(&policy).expect("render should succeed");
        assert!(rendered.contains("Bash(git status)"));
        assert!(rendered.contains("Bash(cargo test --quiet)"));
    }

    #[test]
    fn render_export_rejects_unknown_runtime() {
        let policy = PermissionsConfig::default();
        let err = render_export("codex", &policy).expect_err("should fail");
        assert!(err.to_string().contains("unsupported runtime"));
    }
}
