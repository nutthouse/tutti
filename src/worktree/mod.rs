use crate::error::{Result, TuttiError};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorktreeSnapshot {
    pub exists: bool,
    pub dirty: bool,
    pub at_project_head: bool,
    pub current_branch: Option<String>,
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
    let current_branch = git_current_branch(&worktree_dir)?;

    Ok(WorktreeSnapshot {
        exists: true,
        dirty,
        at_project_head: root_head == worktree_head,
        current_branch,
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

fn git_current_branch(path: &Path) -> Result<Option<String>> {
    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(path)
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(TuttiError::Worktree(format!(
            "failed to resolve branch at '{}': {stderr}",
            path.display()
        )));
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    // "HEAD" means detached state — no branch
    if branch == "HEAD" {
        Ok(None)
    } else {
        Ok(Some(branch))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        WorktreeSnapshot, ensure_fresh_worktree, ensure_worktree, inspect_worktree,
        remove_worktree, worktree_path,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestRepo {
        root: PathBuf,
    }

    impl TestRepo {
        fn new() -> Self {
            let unique = format!(
                "tutti-worktree-tests-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("current time should be after epoch")
                    .as_nanos()
            );
            let root = std::env::temp_dir().join(unique);
            fs::create_dir_all(&root).expect("temp repo dir should be created");

            run_git(&root, ["init"]);
            run_git(&root, ["config", "user.name", "Tutti Tests"]);
            run_git(&root, ["config", "user.email", "tutti-tests@example.com"]);

            fs::write(root.join("README.md"), "initial\n").expect("seed file should be written");
            run_git(&root, ["add", "README.md"]);
            run_git(&root, ["commit", "-m", "initial"]);

            Self { root }
        }

        fn path(&self) -> &Path {
            &self.root
        }
    }

    impl Drop for TestRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn run_git<I, S>(path: &Path, args: I) -> String
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let args_vec: Vec<String> = args
            .into_iter()
            .map(|arg| arg.as_ref().to_string())
            .collect();
        let output = Command::new("git")
            .args(args_vec.iter().map(String::as_str))
            .current_dir(path)
            .output()
            .expect("git command should run");

        assert!(
            output.status.success(),
            "git {:?} failed in {}: {}",
            args_vec,
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        );

        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    #[test]
    fn ensure_worktree_creates_new_branch_and_reports_clean_snapshot() {
        let repo = TestRepo::new();

        let path = ensure_worktree(repo.path(), "tester", "tutti/tester")
            .expect("worktree should be created");

        assert_eq!(path, worktree_path(repo.path(), "tester"));
        assert!(path.exists());
        assert_eq!(
            run_git(&path, ["rev-parse", "--abbrev-ref", "HEAD"]),
            "tutti/tester"
        );
        assert_eq!(
            inspect_worktree(repo.path(), "tester").expect("snapshot should succeed"),
            WorktreeSnapshot {
                exists: true,
                dirty: false,
                at_project_head: true,
                current_branch: Some("tutti/tester".to_string()),
            }
        );
    }

    #[test]
    fn inspect_worktree_detects_dirty_state_and_head_divergence() {
        let repo = TestRepo::new();
        let path = ensure_worktree(repo.path(), "tester", "tutti/tester")
            .expect("worktree should be created");

        fs::write(path.join("dirty.txt"), "pending\n").expect("dirty file should be written");

        assert_eq!(
            inspect_worktree(repo.path(), "tester").expect("snapshot should succeed"),
            WorktreeSnapshot {
                exists: true,
                dirty: true,
                at_project_head: true,
                current_branch: Some("tutti/tester".to_string()),
            }
        );

        run_git(&path, ["add", "dirty.txt"]);
        run_git(&path, ["commit", "-m", "diverge"]);

        assert_eq!(
            inspect_worktree(repo.path(), "tester").expect("snapshot should succeed"),
            WorktreeSnapshot {
                exists: true,
                dirty: false,
                at_project_head: false,
                current_branch: Some("tutti/tester".to_string()),
            }
        );
    }

    #[test]
    fn ensure_fresh_worktree_resets_diverged_branch_to_project_head() {
        let repo = TestRepo::new();
        let path = ensure_worktree(repo.path(), "tester", "tutti/tester")
            .expect("worktree should be created");

        fs::write(path.join("feature.txt"), "change\n").expect("feature file should be written");
        run_git(&path, ["add", "feature.txt"]);
        run_git(&path, ["commit", "-m", "feature"]);

        let refreshed = ensure_fresh_worktree(repo.path(), "tester", "tutti/tester")
            .expect("fresh worktree should be recreated");

        assert_eq!(refreshed, worktree_path(repo.path(), "tester"));
        assert_eq!(
            inspect_worktree(repo.path(), "tester").expect("snapshot should succeed"),
            WorktreeSnapshot {
                exists: true,
                dirty: false,
                at_project_head: true,
                current_branch: Some("tutti/tester".to_string()),
            }
        );
        assert!(!refreshed.join("feature.txt").exists());
    }

    #[test]
    fn remove_worktree_is_idempotent_and_removes_existing_checkout() {
        let repo = TestRepo::new();

        remove_worktree(repo.path(), "tester").expect("missing worktree removal should succeed");

        let path = ensure_worktree(repo.path(), "tester", "tutti/tester")
            .expect("worktree should be created");
        assert!(path.exists());

        remove_worktree(repo.path(), "tester").expect("existing worktree removal should succeed");

        assert!(!path.exists());
        assert_eq!(
            inspect_worktree(repo.path(), "tester").expect("missing snapshot should succeed"),
            WorktreeSnapshot::default()
        );
    }
}
