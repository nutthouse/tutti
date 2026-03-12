use crate::config::TuttiConfig;
use crate::error::{Result, TuttiError};
use crate::session::TmuxSession;
use crate::state;
use colored::Colorize;
use comfy_table::{Table, presets::UTF8_BORDERS_ONLY};
use std::io::{self, Read, Write};
use std::thread;
use std::time::Duration;

use super::status::gather_agent_statuses;

pub fn run(interval: u64) -> Result<()> {
    crate::session::tmux::check_tmux()?;

    let cwd = std::env::current_dir()?;
    let (config, config_path) = TuttiConfig::load(&cwd)?;
    let project_root = config_path.parent().unwrap();

    if config.agents.is_empty() {
        println!("No agents defined in tutti.toml");
        return Ok(());
    }

    // Enable raw mode for key input
    enable_raw_mode();

    let agent_names: Vec<String> = config.agents.iter().map(|a| a.name.clone()).collect();
    let mut selected: usize = 0;
    let peek_lines: u32 = 20;

    loop {
        // Clear terminal
        print!("\x1B[2J\x1B[H");

        let rows = gather_agent_statuses(&config, project_root);

        // Header
        println!(
            "{}\n",
            format!(
                "tutti: {} (refreshing every {}s)",
                config.workspace.name, interval
            )
            .bold()
        );

        // Status table with selection indicator
        let mut table = Table::new();
        table.load_preset(UTF8_BORDERS_ONLY);
        table.set_header(vec!["", "Agent", "Runtime", "Status"]);

        for (i, row) in rows.iter().enumerate() {
            let marker = if i == selected {
                ">".bold().to_string()
            } else {
                " ".to_string()
            };
            table.add_row(vec![&marker, &row.name, &row.runtime, &row.status]);
        }

        println!("{table}");

        // Peek at selected agent's output
        let selected_name = &agent_names[selected];
        let session = TmuxSession::session_name(&config.workspace.name, selected_name);

        println!("\n{}", format!("─── {} ───", selected_name).bold());

        if TmuxSession::session_exists(&session) {
            match TmuxSession::capture_pane(&session, peek_lines) {
                Ok(output) => {
                    // Show last N non-empty lines
                    let lines: Vec<&str> = output
                        .lines()
                        .rev()
                        .take(peek_lines as usize)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect();
                    for line in lines {
                        println!("  {}", line.dimmed());
                    }
                }
                Err(_) => println!("  {}", "(could not read output)".dimmed()),
            }
        } else {
            println!("  {}", "(not running)".dimmed());
        }

        println!(
            "\n{}",
            "j/k: select agent  a: attach  p: peek (full)  q: quit".dimmed()
        );

        // Update state files
        for row in &rows {
            let _ = state::update_status_if_exists(project_root, &row.name, &row.raw_status);
        }

        // Poll for key input during the sleep interval
        let deadline = std::time::Instant::now() + Duration::from_secs(interval);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break;
            }

            thread::sleep(Duration::from_millis(50));

            if let Some(key) = read_key_nonblocking() {
                match key {
                    b'q' => {
                        disable_raw_mode();
                        print!("\x1B[2J\x1B[H");
                        return Ok(());
                    }
                    b'j' | b'J' => {
                        selected = (selected + 1) % agent_names.len();
                        break; // Refresh immediately
                    }
                    b'k' | b'K' => {
                        selected = selected.checked_sub(1).unwrap_or(agent_names.len() - 1);
                        break;
                    }
                    b'a' | b'A' => {
                        disable_raw_mode();
                        let agent = &agent_names[selected];
                        let session = TmuxSession::session_name(&config.workspace.name, agent);
                        if TmuxSession::session_exists(&session) {
                            // Set status bar before attaching
                            let others: Vec<&str> = agent_names
                                .iter()
                                .filter(|n| n.as_str() != agent.as_str())
                                .map(|n| n.as_str())
                                .collect();
                            let switch_hint = if others.is_empty() {
                                String::new()
                            } else {
                                format!(" ── tt attach {}", others[0])
                            };
                            let bar = format!(
                                " tutti: {} ({}) ── Ctrl+b d to detach{}",
                                agent, config.workspace.name, switch_hint
                            );
                            let _ = TmuxSession::set_status_bar(&session, &bar);
                            let _ = TmuxSession::attach_session(&session);
                        }
                        // After detaching, re-enable raw mode and continue
                        enable_raw_mode();
                        break;
                    }
                    b'p' | b'P' => {
                        disable_raw_mode();
                        // Show full peek output
                        print!("\x1B[2J\x1B[H");
                        let agent = &agent_names[selected];
                        let session = TmuxSession::session_name(&config.workspace.name, agent);
                        println!("{}\n", format!("─── {} (full peek) ───", agent).bold());
                        if TmuxSession::session_exists(&session) {
                            match TmuxSession::capture_pane(&session, 100) {
                                Ok(output) => println!("{output}"),
                                Err(_) => println!("{}", "(could not read output)".dimmed()),
                            }
                        } else {
                            println!("{}", "(not running)".dimmed());
                        }
                        println!("\n{}", "Press any key to return to watch...".dimmed());
                        // Wait for any key
                        let _ = io::stdin().read(&mut [0u8]);
                        enable_raw_mode();
                        break;
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Read a single byte from stdin without blocking, using non-blocking I/O.
/// Returns None if no input is available.
fn read_key_nonblocking() -> Option<u8> {
    use std::os::unix::io::AsRawFd;

    let fd = io::stdin().as_raw_fd();

    // Get current flags
    let flags = unsafe { nix_fcntl_getfl(fd) };
    if flags < 0 {
        return None;
    }

    // Set non-blocking
    unsafe {
        nix_fcntl_setfl(fd, flags | 0x0004 /* O_NONBLOCK */)
    };

    let mut buf = [0u8; 1];
    let result = io::stdin().read(&mut buf);

    // Restore blocking mode
    unsafe { nix_fcntl_setfl(fd, flags) };

    match result {
        Ok(1) => Some(buf[0]),
        _ => None,
    }
}

unsafe fn nix_fcntl_getfl(fd: i32) -> i32 {
    // F_GETFL = 3
    unsafe extern "C" {
        safe fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    }
    fcntl(fd, 3)
}

unsafe fn nix_fcntl_setfl(fd: i32, flags: i32) -> i32 {
    // F_SETFL = 4
    unsafe extern "C" {
        safe fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    }
    fcntl(fd, 4, flags)
}

fn enable_raw_mode() {
    let _ = std::process::Command::new("stty")
        .args(["raw", "-echo"])
        .stdin(std::process::Stdio::inherit())
        .status();
}

fn disable_raw_mode() {
    let _ = std::process::Command::new("stty")
        .args(["sane"])
        .stdin(std::process::Stdio::inherit())
        .status();
}
