use crate::config::defaults::{DEFAULT_CONFIG, DEFAULT_GLOBAL_CONFIG};
use crate::config::{GlobalConfig, global_config_path};
use crate::error::{Result, TuttiError};
use std::path::Path;

pub fn run() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let config_path = cwd.join("tutti.toml");

    if config_path.exists() {
        return Err(TuttiError::ConfigAlreadyExists(cwd.clone()));
    }

    std::fs::write(&config_path, DEFAULT_CONFIG)?;
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

    // Register this workspace in the global config
    let mut global = GlobalConfig::load()?;
    // Derive workspace name from directory name
    let ws_name = cwd
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unnamed");
    global.register_workspace(ws_name, &cwd);
    global.save()?;
    println!("Registered workspace '{ws_name}'");

    println!("\nEdit tutti.toml to configure your agent team, then run: tt up");
    Ok(())
}

/// Init into a specific directory (used for testing).
pub fn run_in(dir: &Path) -> Result<()> {
    let config_path = dir.join("tutti.toml");

    if config_path.exists() {
        return Err(TuttiError::ConfigAlreadyExists(dir.to_path_buf()));
    }

    std::fs::write(&config_path, DEFAULT_CONFIG)?;
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
}
