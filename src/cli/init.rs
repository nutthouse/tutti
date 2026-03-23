#[cfg(test)]
use crate::config::defaults::DEFAULT_CONFIG;
use crate::config::defaults::DEFAULT_GLOBAL_CONFIG;
use crate::config::{GlobalConfig, global_config_path};
use crate::error::{Result, TuttiError};
use crate::template::{self, BuiltinTemplates, ParsedTemplate};
use comfy_table::{Table, presets::UTF8_FULL};
use std::io::{self, BufRead, Write};
use std::path::Path;

/// Trait for user input — enables testing interactive flows.
pub trait InputSource {
    fn prompt_choice(&mut self, prompt: &str, options: &[&str]) -> String;
    fn prompt_text(&mut self, prompt: &str, default: &str) -> String;
}

/// Standard stdin input for production.
pub struct StdinInput;

impl InputSource for StdinInput {
    fn prompt_choice(&mut self, prompt: &str, options: &[&str]) -> String {
        let stdin = io::stdin();
        loop {
            print!("{prompt} ");
            io::stdout().flush().ok();
            let mut line = String::new();
            match stdin.lock().read_line(&mut line) {
                Ok(0) => {
                    // EOF — return first option as default
                    return options.first().unwrap_or(&"Q").to_uppercase();
                }
                Err(_) => {
                    return options.first().unwrap_or(&"Q").to_uppercase();
                }
                Ok(_) => {}
            }
            if line.trim().is_empty() {
                continue;
            }
            let choice = line.trim().to_uppercase();
            if options.iter().any(|o| o.to_uppercase() == choice) {
                return choice;
            }
            println!("Invalid choice. Options: {}", options.join(", "));
        }
    }

    fn prompt_text(&mut self, prompt: &str, default: &str) -> String {
        print!("{prompt} [{default}]: ");
        io::stdout().flush().ok();
        let mut line = String::new();
        if io::stdin().lock().read_line(&mut line).unwrap_or(0) == 0 {
            return default.to_string();
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            default.to_string()
        } else {
            trimmed.to_string()
        }
    }
}

pub fn run(template_name: Option<&str>) -> Result<()> {
    run_with_input(template_name, &mut StdinInput)
}

pub fn run_with_input(template_name: Option<&str>, input: &mut dyn InputSource) -> Result<()> {
    run_with_input_in(template_name, input, &std::env::current_dir()?)
}

pub fn run_with_input_in(
    template_name: Option<&str>,
    input: &mut dyn InputSource,
    cwd: &Path,
) -> Result<()> {
    let cwd = cwd.to_path_buf();
    let config_path = cwd.join("tutti.toml");

    // Re-init guard: offer backup instead of hard error
    if config_path.exists() {
        let choice = input.prompt_choice(
            "tutti.toml already exists. [B] Backup and regenerate  [Q] Quit",
            &["B", "Q"],
        );
        if choice == "Q" {
            println!("Aborted.");
            return Ok(());
        }
        // Backup existing file
        let backup_path = cwd.join("tutti.toml.bak");
        std::fs::copy(&config_path, &backup_path)?;
        println!("Backed up existing config to tutti.toml.bak");
    }

    let project_name = cwd
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unnamed");

    // Resolve template: explicit flag, auto-detect, or user pick
    let mut parsed = resolve_template(template_name, &cwd, input)?;
    let mut current_project_name = project_name.to_string();

    // Show team preview
    println!();
    render_team_preview(&parsed, &current_project_name);

    // Interactive L/C/Q loop
    loop {
        let choice = input.prompt_choice(
            "\n  [L] Launch now  [C] Customize  [Q] Quit",
            &["L", "C", "Q"],
        );
        match choice.as_str() {
            "L" => {
                let config_content = template::generate_config(&parsed, &current_project_name)?;
                write_config_and_register(
                    &config_path,
                    &config_content,
                    &current_project_name,
                    &cwd,
                )?;
                println!("Created tutti.toml");

                // Auto-launch
                match launch_agents(&cwd) {
                    Ok(()) => {
                        println!("\nYour agent team is running!");
                        println!("Run `tt serve --port 4040` for the web dashboard");
                    }
                    Err(e) => {
                        eprintln!("\nFailed to launch agents: {e}");
                        println!(
                            "Config saved to tutti.toml. Fix the issue above, then run: tt up"
                        );
                    }
                }
                return Ok(());
            }
            "C" => {
                customize_template(&mut parsed, &mut current_project_name, input);
                println!();
                render_team_preview(&parsed, &current_project_name);
            }
            "Q" => {
                let config_content = template::generate_config(&parsed, &current_project_name)?;
                write_config_and_register(
                    &config_path,
                    &config_content,
                    &current_project_name,
                    &cwd,
                )?;
                println!("Created tutti.toml");
                println!("\nEdit tutti.toml to configure your agent team, then run: tt up");
                return Ok(());
            }
            _ => {}
        }
    }
}

/// Resolve which template to use: explicit name, auto-detect, or user pick.
fn resolve_template(
    template_name: Option<&str>,
    cwd: &Path,
    input: &mut dyn InputSource,
) -> Result<ParsedTemplate> {
    if let Some(name) = template_name {
        // Explicit --template flag
        let (_name, parsed) = template::load_template(name)?;
        println!(
            "Using template: {} v{}",
            parsed.metadata.name, parsed.metadata.version
        );
        return Ok(parsed);
    }

    // Auto-detect from repo files
    let mut matches = template::detect_templates(cwd);
    // Also check custom templates
    let custom = template::discover_custom_templates();
    for (name, tpl) in &custom {
        let score = template::score_template_detection(&tpl.metadata, cwd);
        if score > 0 {
            matches.push((name.clone(), tpl.clone(), score));
        }
    }
    matches.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));

    if matches.len() == 1 {
        let (name, parsed, _score) = matches.into_iter().next().unwrap();
        // Show what we detected
        let detected_files: Vec<&str> = parsed
            .metadata
            .detect
            .iter()
            .filter(|f| cwd.join(f).exists())
            .map(|s| s.as_str())
            .collect();
        println!(
            "  Detected: {} \u{2192} {} project",
            detected_files.join(", "),
            name
        );
        println!("  Template: {} ({})", name, parsed.metadata.description);
        return Ok(parsed);
    }

    if matches.len() > 1 {
        println!("Multiple templates match this repo:\n");
        for (i, (name, parsed, score)) in matches.iter().enumerate() {
            println!(
                "  {}) {} (score: {}) \u{2014} {}",
                i + 1,
                name,
                score,
                parsed.metadata.description
            );
        }
        println!();

        // Build options: "1", "2", "3", ...
        let opts: Vec<String> = (1..=matches.len()).map(|i| i.to_string()).collect();
        let opt_refs: Vec<&str> = opts.iter().map(|s| s.as_str()).collect();
        let choice = input.prompt_choice("Pick a template:", &opt_refs);
        let idx: usize = choice.parse::<usize>().unwrap_or(1) - 1;
        let (name, parsed, _) = matches.into_iter().nth(idx).unwrap();
        println!("Using template: {}", name);
        return Ok(parsed);
    }

    // No matches — show all templates and let user pick
    println!("No template matched this repo. Available templates:\n");
    let all = list_all_templates();
    for (i, (name, parsed)) in all.iter().enumerate() {
        println!("  {}) {:<20} {}", i + 1, name, parsed.metadata.description);
    }
    println!();

    let opts: Vec<String> = (1..=all.len()).map(|i| i.to_string()).collect();
    let opt_refs: Vec<&str> = opts.iter().map(|s| s.as_str()).collect();
    let choice = input.prompt_choice("Pick a template:", &opt_refs);
    let idx: usize = choice.parse::<usize>().unwrap_or(all.len()) - 1;
    let (_name, parsed) = all.into_iter().nth(idx).unwrap();
    Ok(parsed)
}

/// Render the team preview table using comfy-table.
fn render_team_preview(parsed: &ParsedTemplate, project_name: &str) {
    println!("  Project: {project_name}");
    println!(
        "  Template: {} v{}\n",
        parsed.metadata.name, parsed.metadata.version
    );

    // Agent table
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(vec!["Agent", "Runtime", "Role"]);

    // Parse the config body to extract agent info
    let config_str = parsed.config_body.replace("{{project_name}}", project_name);
    if let Ok(config) = toml::from_str::<crate::config::TuttiConfig>(&config_str) {
        for agent in &config.agents {
            let runtime = agent
                .resolved_runtime(&config.defaults, &config.roles)
                .unwrap_or_else(|| "unknown".to_string());
            let role_desc = agent
                .role
                .as_ref()
                .and_then(|r| parsed.metadata.roles.get(r))
                .and_then(|rd| rd.description.as_deref())
                .unwrap_or("");
            table.add_row(vec![&agent.name, &runtime, role_desc]);
        }

        println!("{table}");

        // Workflow summary
        if !config.workflows.is_empty() {
            println!();
            for wf in &config.workflows {
                println!("  Workflow: {} ({} steps)", wf.name, wf.steps.len());
            }
        }
    } else {
        // Fallback: show roles from template metadata
        let mut table = Table::new();
        table.load_preset(UTF8_FULL);
        table.set_header(vec!["Role", "Runtime", "Description"]);
        for (role, def) in &parsed.metadata.roles {
            table.add_row(vec![
                role.as_str(),
                &def.default_runtime,
                def.description.as_deref().unwrap_or(""),
            ]);
        }
        println!("{table}");
    }
}

/// Customize mode: swap runtimes, remove agents, change project name.
fn customize_template(
    parsed: &mut ParsedTemplate,
    project_name: &mut String,
    input: &mut dyn InputSource,
) {
    loop {
        println!("\n  Customize:");
        println!("    1) Change runtime for a role");
        println!("    2) Remove an agent");
        println!("    3) Change project name");
        println!("    D) Done");

        let choice = input.prompt_choice("  Pick:", &["1", "2", "3", "D"]);
        match choice.as_str() {
            "1" => {
                let roles: Vec<String> = parsed.metadata.roles.keys().cloned().collect();
                if roles.is_empty() {
                    println!("  No roles defined.");
                    continue;
                }
                println!("  Roles:");
                for (i, role) in roles.iter().enumerate() {
                    let rt = &parsed.metadata.roles[role].default_runtime;
                    println!("    {}) {} \u{2192} {}", i + 1, role, rt);
                }
                let opts: Vec<String> = (1..=roles.len()).map(|i| i.to_string()).collect();
                let opt_refs: Vec<&str> = opts.iter().map(|s| s.as_str()).collect();
                let pick = input.prompt_choice("  Which role?", &opt_refs);
                let idx: usize = pick.parse::<usize>().unwrap_or(1) - 1;
                if idx < roles.len() {
                    let new_rt = input
                        .prompt_text("  New runtime (claude-code, codex, aider)", "claude-code");
                    // Update both template metadata and config body
                    let role_name = &roles[idx];
                    let old_rt = parsed.metadata.roles[role_name].default_runtime.clone();
                    if let Some(role_def) = parsed.metadata.roles.get_mut(role_name) {
                        role_def.default_runtime = new_rt.clone();
                    }
                    // Update the config body: replace the role's runtime in [roles] table
                    parsed.config_body = parsed.config_body.replace(
                        &format!("{role_name} = \"{old_rt}\""),
                        &format!("{role_name} = \"{new_rt}\""),
                    );
                    println!(
                        "  Changed {} runtime: {} \u{2192} {}",
                        role_name, old_rt, new_rt
                    );
                }
            }
            "2" => {
                // Parse config to get agent list
                let config_str = parsed.config_body.replace("{{project_name}}", project_name);
                if let Ok(config) = toml::from_str::<crate::config::TuttiConfig>(&config_str) {
                    let agents: Vec<String> =
                        config.agents.iter().map(|a| a.name.clone()).collect();
                    if agents.len() <= 1 {
                        println!("  Can't remove \u{2014} only one agent left.");
                        continue;
                    }
                    println!("  Agents:");
                    for (i, name) in agents.iter().enumerate() {
                        println!("    {}) {}", i + 1, name);
                    }
                    let opts: Vec<String> = (1..=agents.len()).map(|i| i.to_string()).collect();
                    let opt_refs: Vec<&str> = opts.iter().map(|s| s.as_str()).collect();
                    let pick = input.prompt_choice("  Remove which agent?", &opt_refs);
                    let idx: usize = pick.parse::<usize>().unwrap_or(0);
                    if idx >= 1 && idx <= agents.len() {
                        let agent_name = &agents[idx - 1];
                        // Remove the [[agent]] block from config body
                        remove_agent_from_config_body(&mut parsed.config_body, agent_name);
                        // Remove the role from metadata if no other agent uses it
                        if let Some(role) = config.agents[idx - 1].role.as_ref() {
                            let other_uses = config
                                .agents
                                .iter()
                                .filter(|a| a.name != *agent_name)
                                .any(|a| a.role.as_deref() == Some(role));
                            if !other_uses {
                                parsed.metadata.roles.remove(role);
                            }
                        }
                        println!("  Removed agent: {}", agent_name);
                    }
                }
            }
            "3" => {
                let new_name = input.prompt_text("  Project name", project_name);
                *project_name = new_name.clone();
                println!("  Project name set to: {}", new_name);
            }
            "D" => break,
            _ => {}
        }
    }
}

/// Remove an [[agent]] block from the config body by agent name.
fn remove_agent_from_config_body(config_body: &mut String, agent_name: &str) {
    // Find the [[agent]] block that contains name = "agent_name"
    let search = format!("name = \"{}\"", agent_name);
    if let Some(name_pos) = config_body.find(&search) {
        // Walk backwards to find the [[agent]] header
        let before = &config_body[..name_pos];
        if let Some(block_start) = before.rfind("[[agent]]") {
            // Walk forward to find the next [[agent]] or [[workflow]] or end
            let after_block = &config_body[block_start + "[[agent]]".len()..];
            let block_end = after_block
                .find("\n[[")
                .map(|p| block_start + "[[agent]]".len() + p)
                .unwrap_or(config_body.len());
            // Include any leading newline before the block
            let trim_start = if block_start > 0 && config_body.as_bytes()[block_start - 1] == b'\n'
            {
                block_start - 1
            } else {
                block_start
            };
            config_body.replace_range(trim_start..block_end, "");
        }
    }
}

/// Write config, ensure global config, register workspace.
fn write_config_and_register(
    config_path: &Path,
    config_content: &str,
    project_name: &str,
    cwd: &Path,
) -> Result<()> {
    std::fs::write(config_path, config_content)?;

    // Ensure global config exists
    let global_path = global_config_path();
    if !global_path.exists() {
        if let Some(parent) = global_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&global_path, DEFAULT_GLOBAL_CONFIG)?;
    }

    // Register workspace
    let workspace_name = toml::from_str::<crate::config::TuttiConfig>(config_content)
        .map(|c| c.workspace.name)
        .unwrap_or_else(|_| project_name.to_string());

    let mut global = GlobalConfig::load()?;
    global.register_workspace(&workspace_name, cwd);
    global.save()?;
    println!("Registered workspace '{workspace_name}'");
    Ok(())
}

/// Launch agents by invoking `tt up` as a subprocess.
fn launch_agents(project_root: &Path) -> Result<()> {
    let tt_bin = std::env::current_exe().unwrap_or_else(|_| "tt".into());
    let output = std::process::Command::new(&tt_bin)
        .arg("up")
        .current_dir(project_root)
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status();

    match output {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(TuttiError::ConfigValidation(format!(
            "tt up exited with status {}",
            status
        ))),
        Err(e) => Err(TuttiError::ConfigValidation(format!(
            "failed to run tt up: {e}"
        ))),
    }
}

/// List all available templates (built-in + custom).
fn list_all_templates() -> Vec<(String, ParsedTemplate)> {
    let mut templates = Vec::new();

    for &name in BuiltinTemplates::list() {
        if let Some(content) = BuiltinTemplates::get(name)
            && let Ok(parsed) = template::parse_template(content)
        {
            templates.push((name.to_string(), parsed));
        }
    }

    // Custom templates
    for (name, parsed) in template::discover_custom_templates() {
        templates.push((name, parsed));
    }

    templates
}

/// `tt template list` command handler.
pub fn template_list() -> Result<()> {
    println!();

    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(vec!["Name", "Version", "Description", "Detects"]);

    for &name in BuiltinTemplates::list() {
        if let Some(content) = BuiltinTemplates::get(name)
            && let Ok(parsed) = template::parse_template(content)
        {
            let detects = if !parsed.metadata.detect.is_empty() {
                parsed.metadata.detect.join(", ")
            } else {
                "\u{2014}".to_string()
            };
            table.add_row(vec![
                name,
                &parsed.metadata.version,
                &parsed.metadata.description,
                &detects,
            ]);
        }
    }

    println!("  Built-in templates:");
    println!("{table}");

    // Custom templates
    let custom = template::discover_custom_templates();
    if custom.is_empty() {
        println!("\n  Custom templates: none found");
        println!("  Place .toml files in ~/.config/tutti/templates/");
    } else {
        println!("\n  Custom templates:");
        let mut ctable = Table::new();
        ctable.load_preset(UTF8_FULL);
        ctable.set_header(vec!["Name", "Version", "Description", "Detects"]);
        for (name, parsed) in &custom {
            let detects = if !parsed.metadata.detect.is_empty() {
                parsed.metadata.detect.join(", ")
            } else {
                "\u{2014}".to_string()
            };
            ctable.add_row(vec![
                name.as_str(),
                &parsed.metadata.version,
                &parsed.metadata.description,
                &detects,
            ]);
        }
        println!("{ctable}");
    }

    println!();
    Ok(())
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

    /// Mock input source for testing interactive flows.
    struct MockInput {
        responses: Vec<String>,
        idx: usize,
    }

    impl MockInput {
        fn new(responses: Vec<&str>) -> Self {
            Self {
                responses: responses.into_iter().map(|s| s.to_string()).collect(),
                idx: 0,
            }
        }
    }

    impl InputSource for MockInput {
        fn prompt_choice(&mut self, _prompt: &str, _options: &[&str]) -> String {
            let resp = self.responses.get(self.idx).cloned().unwrap_or_default();
            self.idx += 1;
            resp.to_uppercase()
        }

        fn prompt_text(&mut self, _prompt: &str, default: &str) -> String {
            let resp = self.responses.get(self.idx).cloned().unwrap_or_default();
            self.idx += 1;
            if resp.is_empty() {
                default.to_string()
            } else {
                resp
            }
        }
    }

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
        assert!(contents.starts_with("# template: gstack-startup 0.1.0\n"));

        let config: crate::config::TuttiConfig = toml::from_str(&contents).unwrap();
        assert_eq!(config.agents.len(), 5);
        assert!(config.roles.is_some());

        let roles = config.roles.as_ref().unwrap();
        assert_eq!(roles.get("reviewer").unwrap(), "codex");

        config.validate().unwrap();

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn init_template_role_remap_works() {
        let dir = std::env::temp_dir().join(format!("tutti-test-remap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        run_template_in(&dir, "gstack-startup").unwrap();

        let contents = std::fs::read_to_string(dir.join("tutti.toml")).unwrap();
        let remapped = contents.replace("reviewer = \"codex\"", "reviewer = \"claude-code\"");
        let config: crate::config::TuttiConfig = toml::from_str(&remapped).unwrap();
        config.validate().unwrap();

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
        let agent = &config.agents[0];
        assert_eq!(
            agent.resolved_runtime(&config.defaults, &config.roles),
            Some("aider".to_string())
        );
    }

    #[test]
    fn interactive_init_quit_writes_config() {
        let dir = std::env::temp_dir().join(format!("tutti-test-iq-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"test\"").unwrap();

        let mut input = MockInput::new(vec!["Q"]);
        let result = run_with_input_in(None, &mut input, &dir);

        assert!(result.is_ok());
        assert!(dir.join("tutti.toml").exists());

        let contents = std::fs::read_to_string(dir.join("tutti.toml")).unwrap();
        let config: crate::config::TuttiConfig = toml::from_str(&contents).unwrap();
        assert!(!config.agents.is_empty());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn interactive_init_customize_project_name() {
        let dir = std::env::temp_dir().join(format!("tutti-test-icn-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"test\"").unwrap();

        let mut input = MockInput::new(vec!["C", "3", "my-custom-name", "D", "Q"]);
        let result = run_with_input_in(None, &mut input, &dir);

        assert!(result.is_ok());
        let contents = std::fs::read_to_string(dir.join("tutti.toml")).unwrap();
        let config: crate::config::TuttiConfig = toml::from_str(&contents).unwrap();
        assert_eq!(config.workspace.name, "my-custom-name");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn interactive_init_backup_existing() {
        let dir = std::env::temp_dir().join(format!("tutti-test-bak-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        std::fs::write(dir.join("tutti.toml"), "# old config").unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"test\"").unwrap();

        let mut input = MockInput::new(vec!["B", "Q"]);
        let result = run_with_input_in(None, &mut input, &dir);

        assert!(result.is_ok());
        assert!(dir.join("tutti.toml.bak").exists());
        let backup = std::fs::read_to_string(dir.join("tutti.toml.bak")).unwrap();
        assert_eq!(backup, "# old config");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn team_preview_renders_without_panic() {
        let content = BuiltinTemplates::get("gstack-startup").unwrap();
        let parsed = template::parse_template(content).unwrap();
        // Should not panic
        render_team_preview(&parsed, "test-project");
    }

    #[test]
    fn template_list_runs_without_error() {
        template_list().unwrap();
    }

    #[test]
    fn remove_agent_from_config_body_works() {
        let mut body = r#"
[[agent]]
name = "planner"
role = "planner"

[[agent]]
name = "tester"
role = "tester"

[[workflow]]
name = "test"
"#
        .to_string();
        remove_agent_from_config_body(&mut body, "planner");
        assert!(!body.contains("planner"));
        assert!(body.contains("tester"));
        assert!(body.contains("[[workflow]]"));
    }
}
