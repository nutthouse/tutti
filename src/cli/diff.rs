use crate::error::{Result, TuttiError};
use std::path::Path;
use std::process::Command;

pub fn run(agent_ref: &str, staged: bool, name_only: bool, stat: bool) -> Result<()> {
    let resolved = super::agent_ref::resolve(agent_ref)?;
    let agent = resolved.agent_config()?;
    let branch = agent.resolved_branch();
    let worktree_path = resolved
        .project_root
        .join(".tutti")
        .join("worktrees")
        .join(&resolved.agent_name);

    if !worktree_path.exists() {
        return Err(TuttiError::Worktree(format!(
            "worktree not found for '{}' at {}. Launch with `tt up` first.",
            resolved.agent_name,
            worktree_path.display()
        )));
    }

    println!(
        "Diff for {} ({})",
        resolved.agent_name, resolved.workspace_name
    );
    println!("Worktree: {}", worktree_path.display());
    println!("Branch: {branch}");
    println!();

    let commit_ahead = git_output(
        &["log", "--oneline", &format!("HEAD..{branch}")],
        &resolved.project_root,
    )?;
    if commit_ahead.trim().is_empty() {
        println!("Committed changes ahead of current branch: none");
    } else {
        println!("Committed changes ahead of current branch:");
        println!("{commit_ahead}");
    }
    println!();

    let status = git_output(&["status", "--short"], &worktree_path)?;
    if status.trim().is_empty() {
        println!("Uncommitted changes: none");
    } else {
        println!("Uncommitted status:");
        println!("{status}");
    }
    println!();

    let mut args = vec!["diff"];
    if staged {
        args.push("--cached");
    }
    if name_only {
        args.push("--name-only");
    } else if stat {
        args.push("--stat");
    }
    let diff = git_output(&args, &worktree_path)?;
    if diff.trim().is_empty() {
        println!("No diff output for selected mode.");
    } else {
        println!("{diff}");
    }

    Ok(())
}

fn git_output(args: &[&str], cwd: &Path) -> Result<String> {
    let output = Command::new("git").args(args).current_dir(cwd).output()?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(TuttiError::Git(format!(
            "git {} failed: {}",
            args.join(" "),
            stderr.trim()
        )))
    }
}
