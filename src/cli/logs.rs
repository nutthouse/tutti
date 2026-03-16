use crate::config::TuttiConfig;
use crate::error::{Result, TuttiError};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

pub fn run(agent_ref: &str, lines: u32, follow: bool) -> Result<()> {
    let (_workspace_name, agent_name, project_root) = resolve_agent_ref(agent_ref)?;
    let log_path = project_root
        .join(".tutti")
        .join("logs")
        .join(format!("{agent_name}.log"));

    if !log_path.exists() {
        return Err(TuttiError::State(format!(
            "log file not found for '{agent_name}' at {}. Start `tt watch` to capture logs.",
            log_path.display()
        )));
    }

    print_tail(&log_path, lines as usize)?;

    if follow {
        follow_log(&log_path)?;
    }

    Ok(())
}

fn resolve_agent_ref(agent_ref: &str) -> Result<(String, String, PathBuf)> {
    if let Some((ws_name, agent_name)) = agent_ref.split_once('/') {
        let (config, config_path) = super::up::load_workspace_by_name(ws_name)?;
        if !config.agents.iter().any(|a| a.name == agent_name) {
            return Err(TuttiError::AgentNotFound(agent_ref.to_string()));
        }
        let project_root = config_path
            .parent()
            .ok_or_else(|| TuttiError::State("invalid workspace config path".to_string()))?
            .to_path_buf();
        Ok((config.workspace.name, agent_name.to_string(), project_root))
    } else {
        let cwd = std::env::current_dir()?;
        let (config, config_path) = TuttiConfig::load(&cwd)?;
        if !config.agents.iter().any(|a| a.name == agent_ref) {
            return Err(TuttiError::AgentNotFound(agent_ref.to_string()));
        }
        let project_root = config_path
            .parent()
            .ok_or_else(|| TuttiError::State("invalid workspace config path".to_string()))?
            .to_path_buf();
        Ok((config.workspace.name, agent_ref.to_string(), project_root))
    }
}

fn print_tail(path: &Path, lines: usize) -> Result<()> {
    let contents = std::fs::read_to_string(path)?;
    let all_lines: Vec<&str> = contents.lines().collect();
    let start = all_lines.len().saturating_sub(lines);
    for line in &all_lines[start..] {
        println!("{line}");
    }
    Ok(())
}

fn follow_log(path: &Path) -> Result<()> {
    let mut position = std::fs::metadata(path)?.len();

    loop {
        thread::sleep(Duration::from_millis(500));

        let metadata = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let new_len = metadata.len();

        if new_len < position {
            position = 0;
        }
        if new_len == position {
            continue;
        }

        let mut file = std::fs::File::open(path)?;
        file.seek(SeekFrom::Start(position))?;
        let mut chunk = String::new();
        file.read_to_string(&mut chunk)?;
        print!("{chunk}");
        std::io::stdout().flush()?;

        position = new_len;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as IoWrite;

    #[test]
    fn print_tail_reads_last_n_lines() {
        let dir = std::env::temp_dir().join("tutti-test-logs-tail");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("test.log");
        {
            let mut f = std::fs::File::create(&log).unwrap();
            for i in 1..=10 {
                writeln!(f, "line {i}").unwrap();
            }
        }
        assert!(print_tail(&log, 3).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn print_tail_handles_fewer_lines_than_requested() {
        let dir = std::env::temp_dir().join("tutti-test-logs-few");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("short.log");
        std::fs::write(&log, "only one line").unwrap();
        assert!(print_tail(&log, 100).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn print_tail_handles_empty_file() {
        let dir = std::env::temp_dir().join("tutti-test-logs-empty");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("empty.log");
        std::fs::write(&log, "").unwrap();
        assert!(print_tail(&log, 10).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn print_tail_errors_on_missing_file() {
        let missing = std::env::temp_dir().join("tutti-test-logs-missing/no.log");
        assert!(print_tail(&missing, 5).is_err());
    }
}
