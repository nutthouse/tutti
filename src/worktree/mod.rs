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
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempRepo {
        path: PathBuf,
    }

    impl TempRepo {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!("tutti-worktree-test-{unique}"));
            fs::create_dir_all(&path).expect("create temp repo directory");

            let repo = Self { path };
            repo.git(&["init"]);
            repo.git(&["config", "user.name", "Tutti Tests"]);
            repo.git(&["config", "user.email", "tests@example.com"]);
            fs::write(repo.path.join("README.md"), "seed\n").expect("write initial file");
            repo.git(&["add", "README.md"]);
            repo.git(&["commit", "-m", "initial commit"]);
            repo
        }

        fn path(&self) -> &Path {
            &self.path
        }

        fn git(&self, args: &[&str]) -> String {
            git_success(&self.path, args)
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn git_success(path: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(path)
            .output()
            .expect("run git command");
        if !output.status.success() {
            panic!(
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        }

        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    #[test]
    fn ensure_worktree_creates_branch_at_project_head() {
        let repo = TempRepo::new();

        let worktree_dir = ensure_worktree(repo.path(), "backend", "tutti/backend").unwrap();

        assert!(worktree_dir.exists());
        assert_eq!(
            git_success(repo.path(), &["rev-parse", "HEAD"]),
            git_success(&worktree_dir, &["rev-parse", "HEAD"])
        );
        assert_eq!(
            git_success(&worktree_dir, &["branch", "--show-current"]),
            "tutti/backend"
        );

        let snapshot = inspect_worktree(repo.path(), "backend").unwrap();
        assert_eq!(
            snapshot,
            WorktreeSnapshot {
                exists: true,
                dirty: false,
                at_project_head: true,
            }
        );
    }

    #[test]
    fn inspect_worktree_reports_dirty_and_diverged_states() {
        let repo = TempRepo::new();
        let worktree_dir = ensure_worktree(repo.path(), "backend", "tutti/backend").unwrap();

        fs::write(worktree_dir.join("work-in-progress.txt"), "draft\n").unwrap();
        let dirty_snapshot = inspect_worktree(repo.path(), "backend").unwrap();
        assert!(dirty_snapshot.exists);
        assert!(dirty_snapshot.dirty);
        assert!(dirty_snapshot.at_project_head);

        git_success(&worktree_dir, &["add", "work-in-progress.txt"]);
        git_success(&worktree_dir, &["commit", "-m", "agent change"]);

        let diverged_snapshot = inspect_worktree(repo.path(), "backend").unwrap();
        assert!(diverged_snapshot.exists);
        assert!(!diverged_snapshot.dirty);
        assert!(!diverged_snapshot.at_project_head);
    }

    #[test]
    fn ensure_fresh_worktree_resets_existing_branch_to_project_head() {
        let repo = TempRepo::new();
        let worktree_dir = ensure_worktree(repo.path(), "backend", "tutti/backend").unwrap();

        fs::write(worktree_dir.join("agent-change.txt"), "agent change\n").unwrap();
        git_success(&worktree_dir, &["add", "agent-change.txt"]);
        git_success(&worktree_dir, &["commit", "-m", "agent branch commit"]);

        let refreshed_dir =
            ensure_fresh_worktree(repo.path(), "backend", "tutti/backend").unwrap();

        assert_eq!(refreshed_dir, worktree_dir);
        assert_eq!(
            git_success(repo.path(), &["rev-parse", "HEAD"]),
            git_success(&refreshed_dir, &["rev-parse", "HEAD"])
        );
        assert!(!refreshed_dir.join("agent-change.txt").exists());

        let snapshot = inspect_worktree(repo.path(), "backend").unwrap();
        assert!(snapshot.exists);
        assert!(!snapshot.dirty);
        assert!(snapshot.at_project_head);
    }

    #[test]
    fn remove_worktree_is_noop_when_directory_is_missing() {
        let repo = TempRepo::new();

        remove_worktree(repo.path(), "backend").unwrap();

        assert!(!worktree_path(repo.path(), "backend").exists());
    }
}
