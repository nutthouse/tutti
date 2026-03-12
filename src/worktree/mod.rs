use crate::error::{Result, TuttiError};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Ensure a git worktree exists for the given agent.
/// Creates the branch and worktree if they don't exist.
/// Returns the path to the worktree directory.
pub fn ensure_worktree(project_root: &Path, agent_name: &str, branch: &str) -> Result<PathBuf> {
    let worktree_dir = project_root
        .join(".tutti")
        .join("worktrees")
        .join(agent_name);

    if worktree_dir.exists() {
        return Ok(worktree_dir);
    }

    // Create parent directories
    if let Some(parent) = worktree_dir.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Check if branch exists
    let branch_exists = Command::new("git")
        .args(["rev-parse", "--verify", branch])
        .current_dir(project_root)
        .output()
        .is_ok_and(|out| out.status.success());

    if branch_exists {
        // Create worktree from existing branch
        let output = Command::new("git")
            .args(["worktree", "add", worktree_dir.to_str().unwrap(), branch])
            .current_dir(project_root)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(TuttiError::Worktree(format!(
                "failed to create worktree for '{agent_name}': {stderr}"
            )));
        }
    } else {
        // Create new branch and worktree
        let output = Command::new("git")
            .args([
                "worktree",
                "add",
                "-b",
                branch,
                worktree_dir.to_str().unwrap(),
            ])
            .current_dir(project_root)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(TuttiError::Worktree(format!(
                "failed to create worktree for '{agent_name}': {stderr}"
            )));
        }
    }

    Ok(worktree_dir)
}

/// Remove a git worktree for the given agent.
pub fn remove_worktree(project_root: &Path, agent_name: &str) -> Result<()> {
    let worktree_dir = project_root
        .join(".tutti")
        .join("worktrees")
        .join(agent_name);

    if !worktree_dir.exists() {
        return Ok(());
    }

    let output = Command::new("git")
        .args([
            "worktree",
            "remove",
            "--force",
            worktree_dir.to_str().unwrap(),
        ])
        .current_dir(project_root)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(TuttiError::Worktree(format!(
            "failed to remove worktree for '{agent_name}': {stderr}"
        )));
    }

    Ok(())
}
