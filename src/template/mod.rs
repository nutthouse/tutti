use crate::error::{Result, TuttiError};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

const SEPARATOR: &str = "# ── config below ──";

/// Metadata from the [template] section of a template file.
#[derive(Debug, Clone, Deserialize)]
pub struct TemplateMetadata {
    pub name: String,
    pub version: String,
    pub description: String,
    #[allow(dead_code)]
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub detect: Vec<String>,
    #[serde(default)]
    pub detect_all: Vec<String>,
    #[serde(default)]
    pub roles: HashMap<String, TemplateRoleDef>,
}

/// Role definition within a template.
#[derive(Debug, Clone, Deserialize)]
pub struct TemplateRoleDef {
    pub default_runtime: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// Intermediate struct for deserializing the full template TOML (Phase 1).
#[derive(Debug, Deserialize)]
struct TemplateFile {
    template: TemplateMetadata,
}

/// A parsed template ready for generation.
#[derive(Debug, Clone)]
pub struct ParsedTemplate {
    pub metadata: TemplateMetadata,
    /// Raw config body (everything below the separator), ready for variable substitution.
    pub config_body: String,
}

/// Parse a template from its raw string content.
pub fn parse_template(content: &str) -> Result<ParsedTemplate> {
    // Phase 1: Parse metadata via TOML deserialization
    let template_file: TemplateFile =
        toml::from_str(content).map_err(|e| TuttiError::TemplateParse(e.to_string()))?;

    // Phase 2: Extract config body below separator
    let sep_pos = content.find(SEPARATOR).ok_or_else(|| {
        TuttiError::TemplateParse(
            "template file missing '# ── config below ──' separator".to_string(),
        )
    })?;

    let after_sep = &content[sep_pos + SEPARATOR.len()..];
    // Skip the rest of the separator line (newline)
    let config_body = after_sep
        .strip_prefix('\n')
        .or_else(|| after_sep.strip_prefix("\r\n"))
        .unwrap_or(after_sep);

    Ok(ParsedTemplate {
        metadata: template_file.template,
        config_body: config_body.to_string(),
    })
}

/// Generate a tutti.toml from a parsed template.
pub fn generate_config(template: &ParsedTemplate, project_name: &str) -> String {
    let header = format!(
        "# template: {} {}\n",
        template.metadata.name, template.metadata.version
    );

    let body = template
        .config_body
        .replace("{{project_name}}", project_name);

    format!("{header}{body}")
}

/// Built-in templates embedded at compile time.
pub struct BuiltinTemplates;

impl BuiltinTemplates {
    /// Get a built-in template by name.
    pub fn get(name: &str) -> Option<&'static str> {
        match name {
            "gstack-startup" => Some(include_str!("../../templates/gstack-startup.toml")),
            "rust-cli" => Some(include_str!("../../templates/rust-cli.toml")),
            "minimal" => Some(include_str!("../../templates/minimal.toml")),
            _ => None,
        }
    }

    /// List all built-in template names.
    pub fn list() -> &'static [&'static str] {
        &["gstack-startup", "rust-cli", "minimal"]
    }
}

/// Detect which templates match a given repo root by checking file existence.
pub fn detect_templates(repo_root: &Path) -> Vec<(String, ParsedTemplate, usize)> {
    let mut matches = Vec::new();

    for &name in BuiltinTemplates::list() {
        let Some(content) = BuiltinTemplates::get(name) else {
            continue;
        };
        let Ok(template) = parse_template(content) else {
            continue;
        };

        let mut score = 0;
        let mut any_match_ok = template.metadata.detect.is_empty();
        let mut all_match_ok = true;

        // Check any-match detection
        for file in &template.metadata.detect {
            if repo_root.join(file).exists() {
                score += 1;
                any_match_ok = true;
            }
        }

        // Check all-match detection
        if !template.metadata.detect_all.is_empty() {
            for file in &template.metadata.detect_all {
                if repo_root.join(file).exists() {
                    score += 1;
                } else {
                    all_match_ok = false;
                }
            }
        }

        if any_match_ok && all_match_ok && score > 0 {
            matches.push((name.to_string(), template, score));
        }
    }

    // Sort by score descending
    matches.sort_by(|a, b| b.2.cmp(&a.2));
    matches
}

/// Load a template from a name or path.
pub fn load_template(name_or_path: &str) -> Result<(String, ParsedTemplate)> {
    // If it looks like a path, read from filesystem
    if name_or_path.contains('/') || name_or_path.ends_with(".toml") {
        let path = Path::new(name_or_path);
        let content = std::fs::read_to_string(path)
            .map_err(|e| TuttiError::TemplateNotFound(format!("{}: {}", name_or_path, e)))?;
        let template = parse_template(&content)?;
        let name = template.metadata.name.clone();
        return Ok((name, template));
    }

    // Otherwise look up built-in
    let content = BuiltinTemplates::get(name_or_path)
        .ok_or_else(|| TuttiError::TemplateNotFound(name_or_path.to_string()))?;
    let template = parse_template(content)?;
    Ok((name_or_path.to_string(), template))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_TEMPLATE: &str = r#"
[template]
name = "test-template"
version = "0.1.0"
description = "A test template"
detect = ["Cargo.toml"]

[template.roles.backend]
default_runtime = "claude-code"
description = "Backend developer"

[template.roles.reviewer]
default_runtime = "codex"
description = "Code reviewer"

# ── config below ──
[workspace]
name = "{{project_name}}"
description = "Generated from test-template"

[defaults]
worktree = true

[roles]
backend = "claude-code"
reviewer = "codex"

[[agent]]
name = "backend"
role = "backend"
prompt = "You own the backend."

[[agent]]
name = "reviewer"
role = "reviewer"
prompt = "You review code."
"#;

    #[test]
    fn parse_template_extracts_metadata() {
        let parsed = parse_template(TEST_TEMPLATE).unwrap();
        assert_eq!(parsed.metadata.name, "test-template");
        assert_eq!(parsed.metadata.version, "0.1.0");
        assert_eq!(parsed.metadata.description, "A test template");
        assert_eq!(parsed.metadata.detect, vec!["Cargo.toml"]);
        assert_eq!(parsed.metadata.roles.len(), 2);
        assert_eq!(
            parsed.metadata.roles["backend"].default_runtime,
            "claude-code"
        );
        assert_eq!(parsed.metadata.roles["reviewer"].default_runtime, "codex");
    }

    #[test]
    fn parse_template_extracts_config_body() {
        let parsed = parse_template(TEST_TEMPLATE).unwrap();
        assert!(parsed.config_body.contains("[workspace]"));
        assert!(parsed.config_body.contains("{{project_name}}"));
        assert!(!parsed.config_body.contains("[template]"));
    }

    #[test]
    fn generate_config_substitutes_variables() {
        let parsed = parse_template(TEST_TEMPLATE).unwrap();
        let config = generate_config(&parsed, "my-app");
        assert!(config.starts_with("# template: test-template 0.1.0\n"));
        assert!(config.contains("name = \"my-app\""));
        assert!(!config.contains("{{project_name}}"));
    }

    #[test]
    fn generated_config_parses_as_tutti_config() {
        let parsed = parse_template(TEST_TEMPLATE).unwrap();
        let config_str = generate_config(&parsed, "my-app");
        let config: crate::config::TuttiConfig = toml::from_str(&config_str).unwrap();
        assert_eq!(config.workspace.name, "my-app");
        assert_eq!(config.agents.len(), 2);
        assert_eq!(config.agents[0].role, Some("backend".to_string()));
        assert!(config.roles.is_some());
    }

    #[test]
    fn parse_template_missing_separator_errors() {
        let bad = r#"
[template]
name = "bad"
version = "0.1.0"
description = "Missing separator"

[workspace]
name = "test"
"#;
        let err = parse_template(bad).unwrap_err();
        assert!(err.to_string().contains("separator"));
    }

    #[test]
    fn parse_template_missing_metadata_errors() {
        let bad = r#"
# ── config below ──
[workspace]
name = "test"
"#;
        let err = parse_template(bad).unwrap_err();
        assert!(err.to_string().contains("template"));
    }

    #[test]
    fn detect_templates_on_empty_dir() {
        let dir = std::env::temp_dir().join(format!("tutti-detect-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let matches = detect_templates(&dir);
        // No files = no matches (minimal has no detect, only matches as fallback)
        assert!(matches.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn builtin_templates_are_valid() {
        for &name in BuiltinTemplates::list() {
            let content = BuiltinTemplates::get(name).unwrap();
            let parsed = parse_template(content)
                .unwrap_or_else(|e| panic!("template '{}' failed to parse: {}", name, e));
            assert_eq!(parsed.metadata.name, name);

            // Verify generated config is valid TOML
            let config_str = generate_config(&parsed, "test-project");
            let _config: crate::config::TuttiConfig = toml::from_str(&config_str)
                .unwrap_or_else(|e| panic!("template '{}' generates invalid config: {}", name, e));
        }
    }
}
