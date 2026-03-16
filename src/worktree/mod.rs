use crate::error::{Result, TuttiError};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WorktreeSnapshot {
    pub exists: bool,
    pub dirty: bool,
    pub at_project_head: bool,
}

/// Ensure a git worktree exists for the given agent.
/// Creates the branch and worktree if they don't exist.
/// Returns the path to the worktree directory.
pub fn ensure_worktree(project_root: &Path, agent_name: &str, branch: &str) -> Result<PathBuf> {
    let worktree_dir = worktree_path(project_root, agent_name);

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

/// Recreate a worktree from the current project HEAD.
/// If the branch already exists, it is reset to HEAD.
pub fn ensure_fresh_worktree(
    project_root: &Path,
    agent_name: &str,
    branch: &str,
) -> Result<PathBuf> {
    let worktree_dir = worktree_path(project_root, agent_name);

    // Remove existing linked worktree first (if present).
    remove_worktree(project_root, agent_name)?;

    // Clean up stray directory if git worktree remove left anything behind.
    if worktree_dir.exists() {
        std::fs::remove_dir_all(&worktree_dir)?;
    }

    if let Some(parent) = worktree_dir.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let output = Command::new("git")
        .args([
            "worktree",
            "add",
            "--force",
            "-B",
            branch,
            worktree_dir.to_str().unwrap(),
            "HEAD",
        ])
        .current_dir(project_root)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(TuttiError::Worktree(format!(
            "failed to recreate fresh worktree for '{agent_name}': {stderr}"
        )));
    }

    Ok(worktree_dir)
}

/// Remove a git worktree for the given agent.
pub fn remove_worktree(project_root: &Path, agent_name: &str) -> Result<()> {
    let worktree_dir = worktree_path(project_root, agent_name);

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

/// Inspect whether an existing worktree is dirty and/or diverged from workspace HEAD.
pub fn inspect_worktree(project_root: &Path, agent_name: &str) -> Result<WorktreeSnapshot> {
    let worktree_dir = worktree_path(project_root, agent_name);
    if !worktree_dir.exists() {
        return Ok(WorktreeSnapshot::default());
    }

    let status_output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&worktree_dir)
        .output()?;
    if !status_output.status.success() {
        let stderr = String::from_utf8_lossy(&status_output.stderr);
        return Err(TuttiError::Worktree(format!(
            "failed to inspect worktree status for '{agent_name}': {stderr}"
        )));
    }
    let dirty = !String::from_utf8_lossy(&status_output.stdout)
        .trim()
        .is_empty();

    let root_head = git_rev_parse(project_root)?;
    let worktree_head = git_rev_parse(&worktree_dir)?;

    Ok(WorktreeSnapshot {
        exists: true,
        dirty,
        at_project_head: root_head == worktree_head,
    })
}

fn worktree_path(project_root: &Path, agent_name: &str) -> PathBuf {
    project_root
        .join(".tutti")
        .join("worktrees")
        .join(agent_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn worktree_path_builds_expected_path() {
        let root = Path::new("/home/user/project");
        let result = worktree_path(root, "backend");
        assert_eq!(
            result,
            PathBuf::from("/home/user/project/.tutti/worktrees/backend")
        );
    }

    #[test]
    fn worktree_path_handles_special_chars_in_agent_name() {
        let root = Path::new("/repo");
        let result = worktree_path(root, "my-agent_v2");
        assert_eq!(result, PathBuf::from("/repo/.tutti/worktrees/my-agent_v2"));
    }

    #[test]
    fn snapshot_default_is_non_existent() {
        let snap = WorktreeSnapshot::default();
        assert!(!snap.exists);
        assert!(!snap.dirty);
        assert!(!snap.at_project_head);
    }

    #[test]
    fn snapshot_equality() {
        let a = WorktreeSnapshot {
            exists: true,
            dirty: false,
            at_project_head: true,
        };
        let b = WorktreeSnapshot {
            exists: true,
            dirty: false,
            at_project_head: true,
        };
        assert_eq!(a, b);

        let c = WorktreeSnapshot {
            exists: true,
            dirty: true,
            at_project_head: true,
        };
        assert_ne!(a, c);
    }
}

fn git_rev_parse(path: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(path)
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(TuttiError::Worktree(format!(
            "failed to resolve HEAD at '{}': {stderr}",
            path.display()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn worktree_path_constructs_expected_path() {
        let root = Path::new("/projects/myapp");
        let path = worktree_path(root, "coder");
        assert_eq!(
            path,
            PathBuf::from("/projects/myapp/.tutti/worktrees/coder")
        );
    }

    #[test]
    fn worktree_path_handles_special_agent_names() {
        let root = Path::new("/repo");
        let path = worktree_path(root, "my-agent_v2");
        assert_eq!(path, PathBuf::from("/repo/.tutti/worktrees/my-agent_v2"));
    }

    #[test]
    fn worktree_snapshot_default_is_not_exists() {
        let snap = WorktreeSnapshot::default();
        assert!(!snap.exists);
        assert!(!snap.dirty);
        assert!(!snap.at_project_head);
    }
}
