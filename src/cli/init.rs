#[cfg(test)]
use crate::config::defaults::DEFAULT_CONFIG;
use crate::config::defaults::DEFAULT_GLOBAL_CONFIG;
use crate::config::{GlobalConfig, global_config_path};
use crate::error::{Result, TuttiError};
use crate::template::{self, BuiltinTemplates};

pub fn run(template_name: Option<&str>) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let config_path = cwd.join("tutti.toml");

    if config_path.exists() {
        return Err(TuttiError::ConfigAlreadyExists(cwd.clone()));
    }

    let project_name = cwd
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unnamed");

    let config_content = if let Some(name) = template_name {
        // Explicit template specified
        let (_tpl_name, parsed) = template::load_template(name)?;
        print_template_info(&parsed);
        template::generate_config(&parsed, project_name)?
    } else {
        // Auto-detect: scan repo for matching templates
        let matches = template::detect_templates(&cwd);

        if matches.len() == 1 {
            // High confidence: one template matches
            let (name, parsed, _score) = &matches[0];
            println!("Detected repo type — using '{}' template.", name);
            print_template_info(parsed);
            template::generate_config(parsed, project_name)?
        } else if matches.len() > 1 {
            // Low confidence: multiple matches — use first but list alternatives
            println!("Multiple templates match this repo:");
            for (name, parsed, score) in &matches {
                println!(
                    "  {} (score: {}) — {}",
                    name, score, parsed.metadata.description
                );
            }
            println!();

            // Fall back to minimal
            let content = BuiltinTemplates::get("minimal").ok_or_else(|| {
                TuttiError::TemplateParse(
                    "built-in 'minimal' template missing — reinstall or run `tt doctor`".into(),
                )
            })?;
            let parsed = template::parse_template(content)?;
            println!(
                "Using 'minimal' template. Run `tt init --template <name>` to choose a different template."
            );
            template::generate_config(&parsed, project_name)?
        } else {
            // No matches — fall back to minimal
            let content = BuiltinTemplates::get("minimal").ok_or_else(|| {
                TuttiError::TemplateParse(
                    "built-in 'minimal' template missing — reinstall or run `tt doctor`".into(),
                )
            })?;
            let parsed = template::parse_template(content)?;
            println!("No template matched this repo — using 'minimal' template.");
            println!("Available templates:");
            for &name in BuiltinTemplates::list() {
                if let Some(c) = BuiltinTemplates::get(name)
                    && let Ok(p) = template::parse_template(c)
                {
                    println!("  {:<20} {}", name, p.metadata.description);
                }
            }
            println!();
            println!("Run `tt init --template <name>` to choose a specific template.");
            template::generate_config(&parsed, project_name)?
        }
    };

    std::fs::write(&config_path, &config_content)?;
    println!("Created tutti.toml in {}", cwd.display());

    // Ensure global config exists
    let global_path = global_config_path();
    if !global_path.exists() {
        if let Some(parent) = global_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&global_path, DEFAULT_GLOBAL_CONFIG)?;
        println!("Created global config at {}", global_path.display());
    }

    // Register using the workspace name from the generated config, falling back to dir basename
    let workspace_name = toml::from_str::<crate::config::TuttiConfig>(&config_content)
        .map(|c| c.workspace.name)
        .unwrap_or_else(|_| project_name.to_string());

    let mut global = GlobalConfig::load()?;
    global.register_workspace(&workspace_name, &cwd);
    global.save()?;
    println!("Registered workspace '{workspace_name}'");

    println!("\nEdit tutti.toml to configure your agent team, then run: tt up");
    Ok(())
}

fn print_template_info(parsed: &template::ParsedTemplate) {
    println!(
        "Template: {} v{}",
        parsed.metadata.name, parsed.metadata.version
    );
    println!("  \"{}\"", parsed.metadata.description);
    println!();
    println!("  Roles:");
    for (role, def) in &parsed.metadata.roles {
        println!(
            "    {:<16} → {}  ({})",
            role,
            def.default_runtime,
            def.description.as_deref().unwrap_or("")
        );
    }
    println!();
}

/// Init into a specific directory (used for testing).
#[cfg(test)]
pub fn run_in(dir: &std::path::Path) -> Result<()> {
    let config_path = dir.join("tutti.toml");

    if config_path.exists() {
        return Err(TuttiError::ConfigAlreadyExists(dir.to_path_buf()));
    }

    std::fs::write(&config_path, DEFAULT_CONFIG)?;
    Ok(())
}

/// Init with a template into a specific directory (used for testing).
#[cfg(test)]
pub fn run_template_in(dir: &std::path::Path, template_name: &str) -> Result<()> {
    let config_path = dir.join("tutti.toml");

    if config_path.exists() {
        return Err(TuttiError::ConfigAlreadyExists(dir.to_path_buf()));
    }

    let project_name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unnamed");

    let (_name, parsed) = template::load_template(template_name)?;
    let config_content = template::generate_config(&parsed, project_name)?;
    std::fs::write(&config_path, config_content)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_creates_parseable_config() {
        let dir = std::env::temp_dir().join(format!("tutti-test-init-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        run_in(&dir).unwrap();

        let contents = std::fs::read_to_string(dir.join("tutti.toml")).unwrap();
        let config: crate::config::TuttiConfig = toml::from_str(&contents).unwrap();
        assert_eq!(config.workspace.name, "my-project");
        assert!(!config.agents.is_empty());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn init_refuses_overwrite() {
        let dir = std::env::temp_dir().join(format!("tutti-test-init2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        run_in(&dir).unwrap();
        let err = run_in(&dir).unwrap_err();
        assert!(err.to_string().contains("already exists"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn init_template_generates_valid_config() {
        let dir = std::env::temp_dir().join(format!("tutti-test-tpl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        run_template_in(&dir, "gstack-startup").unwrap();

        let contents = std::fs::read_to_string(dir.join("tutti.toml")).unwrap();

        // Verify template comment on line 1
        assert!(contents.starts_with("# template: gstack-startup 0.1.0\n"));

        // Verify it parses
        let config: crate::config::TuttiConfig = toml::from_str(&contents).unwrap();
        assert_eq!(config.agents.len(), 5);
        assert!(config.roles.is_some());

        // Verify role mapping works
        let roles = config.roles.as_ref().unwrap();
        assert_eq!(roles.get("reviewer").unwrap(), "codex");

        // Verify validation passes
        config.validate().unwrap();

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn init_template_role_remap_works() {
        let dir = std::env::temp_dir().join(format!("tutti-test-remap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        run_template_in(&dir, "gstack-startup").unwrap();

        let contents = std::fs::read_to_string(dir.join("tutti.toml")).unwrap();
        // Simulate role remap: change reviewer from codex to claude-code
        let remapped = contents.replace("reviewer = \"codex\"", "reviewer = \"claude-code\"");
        let config: crate::config::TuttiConfig = toml::from_str(&remapped).unwrap();
        config.validate().unwrap();

        // Verify the reviewer agent now resolves to claude-code
        let reviewer = config.agents.iter().find(|a| a.name == "reviewer").unwrap();
        assert_eq!(
            reviewer.resolved_runtime(&config.defaults, &config.roles),
            Some("claude-code".to_string())
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn init_template_refuses_nonexistent() {
        let err = template::load_template("nonexistent-template").unwrap_err();
        assert!(err.to_string().contains("nonexistent-template"));
    }

    #[test]
    fn init_template_refuses_overwrite() {
        let dir = std::env::temp_dir().join(format!("tutti-test-tpldup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        run_template_in(&dir, "minimal").unwrap();
        let err = run_template_in(&dir, "minimal").unwrap_err();
        assert!(err.to_string().contains("already exists"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn validate_rejects_role_without_roles_table() {
        let toml_str = r#"
[workspace]
name = "test"

[[agent]]
name = "backend"
role = "implementer"
"#;
        let config: crate::config::TuttiConfig = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("[roles] table is not defined"));
    }

    #[test]
    fn validate_rejects_unknown_role() {
        let toml_str = r#"
[workspace]
name = "test"

[roles]
implementer = "claude-code"

[[agent]]
name = "backend"
role = "planner"
"#;
        let config: crate::config::TuttiConfig = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("[roles] does not define it"));
    }

    #[test]
    fn validate_accepts_explicit_runtime_with_role() {
        let toml_str = r#"
[workspace]
name = "test"

[roles]
implementer = "claude-code"

[[agent]]
name = "backend"
role = "implementer"
runtime = "aider"
"#;
        let config: crate::config::TuttiConfig = toml::from_str(toml_str).unwrap();
        config.validate().unwrap();
        // Explicit runtime should win
        let agent = &config.agents[0];
        assert_eq!(
            agent.resolved_runtime(&config.defaults, &config.roles),
            Some("aider".to_string())
        );
    }
}
