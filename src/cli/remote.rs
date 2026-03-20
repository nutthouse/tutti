use crate::config::{GlobalConfig, RemoteEntry};
use crate::error::{Result, TuttiError};
use colored::Colorize;
use comfy_table::{Cell, Table};
use std::process::{Command, Stdio};

/// Build the SSH tunnel argument list for `ssh -N -L`.
fn ssh_tunnel_args(host: &str, port: u16) -> Vec<String> {
    vec![
        "-N".to_string(),
        "-L".to_string(),
        format!("{port}:127.0.0.1:{port}"),
        host.to_string(),
    ]
}

/// `tt remote attach <host>` — open an SSH port-forward tunnel and print connection instructions.
pub fn attach(host: &str, port: u16, name: Option<&str>) -> Result<()> {
    // Verify ssh is available
    which::which("ssh").map_err(|_| {
        TuttiError::Ssh("ssh not found on PATH — install OpenSSH to use remote tunnels".into())
    })?;

    let args = ssh_tunnel_args(host, port);
    println!(
        "{} opening SSH tunnel to {} (port {})",
        "remote:".bold(),
        host.cyan(),
        port
    );

    let mut child = Command::new("ssh")
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| TuttiError::Ssh(format!("failed to spawn ssh: {e}")))?;

    let pid = child.id();
    println!("{} tunnel PID {pid}", "remote:".bold());
    println!();
    println!("  Connect a local tutti client to the remote host:");
    println!();
    println!(
        "    {}",
        format!("export TUTTI_REMOTE=http://127.0.0.1:{port}").green()
    );
    println!();
    println!("  The tunnel runs in the foreground. Press Ctrl-C to close it.");

    // Persist as a [[remote]] entry in global config if it doesn't already exist
    let entry_name = name.unwrap_or(host);
    let global_result = GlobalConfig::load();
    if let Err(e) = &global_result {
        eprintln!("warning: could not load global config to persist remote entry: {e}");
    }
    if let Ok(mut global) = global_result {
        let already = global
            .remotes
            .iter()
            .any(|r| r.host == host && r.port == port);
        if !already {
            global.remotes.push(RemoteEntry {
                name: entry_name.to_string(),
                host: host.to_string(),
                port,
                token: None,
            });
            if let Err(e) = global.save() {
                eprintln!("warning: could not persist remote entry: {e}");
            }
        }
    }

    // Wait for the SSH process (blocks until Ctrl-C / disconnect)
    let status = child
        .wait()
        .map_err(|e| TuttiError::Ssh(format!("ssh tunnel failed: {e}")))?;

    if !status.success() {
        return Err(TuttiError::Ssh(format!(
            "ssh exited with status {}",
            status.code().unwrap_or(-1)
        )));
    }

    Ok(())
}

/// `tt remote status` — list registered remotes and probe reachability.
pub fn status() -> Result<()> {
    let global = GlobalConfig::load()
        .map_err(|e| TuttiError::RemoteConnection(format!("could not load global config: {e}")))?;

    if global.remotes.is_empty() {
        println!("No remote hosts registered. Use `tt remote attach <host>` to add one.");
        return Ok(());
    }

    let mut table = Table::new();
    table.set_header(vec!["Name", "Host", "Port", "Status"]);

    for remote in &global.remotes {
        let reachable = probe_remote(remote);
        let status_cell = if reachable {
            Cell::new("Online").fg(comfy_table::Color::Green)
        } else {
            Cell::new("Down").fg(comfy_table::Color::Red)
        };

        table.add_row(vec![
            Cell::new(&remote.name),
            Cell::new(&remote.host),
            Cell::new(remote.port),
            status_cell,
        ]);
    }

    println!("{table}");
    Ok(())
}

/// Probe a remote host by attempting `ssh <host> curl -sf http://127.0.0.1:<port>/v1/health`.
fn probe_remote(remote: &RemoteEntry) -> bool {
    let Ok(output) = Command::new("ssh")
        .args([
            "-o",
            "ConnectTimeout=5",
            "-o",
            "BatchMode=yes",
            &remote.host,
            &format!("curl -sf http://127.0.0.1:{}/v1/health", remote.port),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return false;
    };
    output.status.success()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_args_default_port() {
        let args = ssh_tunnel_args("myhost.example.com", 4040);
        assert_eq!(
            args,
            vec!["-N", "-L", "4040:127.0.0.1:4040", "myhost.example.com"]
        );
    }

    #[test]
    fn ssh_args_custom_port() {
        let args = ssh_tunnel_args("10.0.0.5", 8080);
        assert_eq!(args, vec!["-N", "-L", "8080:127.0.0.1:8080", "10.0.0.5"]);
    }
}
