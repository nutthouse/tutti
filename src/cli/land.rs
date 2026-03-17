use crate::error::{Result, TuttiError};
use std::path::Path;
use std::process::Command;

pub fn run(agent_ref: &str, pr: bool, force: bool) -> Result<()> {
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

    if !force {
        ensure_git_clean(&resolved.project_root)?;
    }
    ensure_branch_exists(&resolved.project_root, &branch)?;
    let wip_committed = commit_wip_if_needed(&worktree_path, &resolved.agent_name)?;

    if pr {
        push_and_open_pr(&resolved.project_root, &branch)?;
        if wip_committed {
            println!(
                "Committed pending worktree changes for {} before pushing.",
                resolved.agent_name
            );
        }
        return Ok(());
    }

    let merge_base = git_output(
        &["merge-base", "HEAD", branch.as_str()],
        &resolved.project_root,
    )?;
    let rev_range = format!("{merge_base}..{branch}");
    let commits = git_output(
        &["rev-list", "--reverse", rev_range.as_str()],
        &resolved.project_root,
    )?;
    let commit_list: Vec<&str> = commits
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();

    if commit_list.is_empty() {
        println!(
            "No new commits to land from {} (branch: {}).",
            resolved.agent_name, branch
        );
        return Ok(());
    }

    let stashed_for_force = if force {
        stash_for_force_land(&resolved.project_root)?
    } else {
        false
    };

    let land_result = (|| -> Result<()> {
        for sha in &commit_list {
            run_git(&["cherry-pick", sha], &resolved.project_root).map_err(|e| {
                TuttiError::Git(format!(
                    "{e}. Resolve conflicts and continue with `git cherry-pick --continue` or abort with `git cherry-pick --abort`."
                ))
            })?;
        }
        Ok(())
    })();

    if stashed_for_force {
        match &land_result {
            Ok(_) => restore_force_land_stash(&resolved.project_root)?,
            Err(_) => {
                eprintln!(
                    "warn: force-land stash was kept because landing failed; recover with `git stash list` / `git stash pop`."
                );
            }
        }
    }

    land_result?;

    println!(
        "Landed {} commit(s) from {} ({}) onto current branch.",
        commit_list.len(),
        resolved.agent_name,
        branch
    );
    if wip_committed {
        println!(
            "Included an auto-commit for pending worktree changes from {}.",
            resolved.agent_name
        );
    }

    Ok(())
}

fn push_and_open_pr(project_root: &Path, branch: &str) -> Result<()> {
    run_git(&["push", "-u", "origin", branch], project_root)?;

    if which::which("gh").is_err() {
        return Err(TuttiError::ConfigValidation(
            "`gh` is required for `tt land --pr`. Install GitHub CLI or run `tt land <agent>` without `--pr`."
                .to_string(),
        ));
    }

    let base_branch = git_output(&["rev-parse", "--abbrev-ref", "HEAD"], project_root)?;
    let output = Command::new("gh")
        .args([
            "pr",
            "create",
            "--head",
            branch,
            "--base",
            &base_branch,
            "--fill",
        ])
        .current_dir(project_root)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(TuttiError::Git(format!(
            "failed to create PR with gh: {}",
            stderr.trim()
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        println!("Pushed {branch} and opened PR against {base_branch}.");
    } else {
        println!("{stdout}");
    }
    Ok(())
}

fn ensure_git_clean(cwd: &Path) -> Result<()> {
    let has_unstaged = !git_success(&["diff", "--quiet", "--ignore-submodules=all"], cwd, true)?;
    let has_staged = !git_success(
        &["diff", "--cached", "--quiet", "--ignore-submodules=all"],
        cwd,
        true,
    )?;

    if !has_unstaged && !has_staged {
        return Ok(());
    }
    Err(TuttiError::Git(
        "working tree has tracked changes. Commit/stash changes, or use `tt land <agent> --force` to override.".to_string(),
    ))
}

fn stash_for_force_land(cwd: &Path) -> Result<bool> {
    let has_changes = !git_success(&["diff", "--quiet", "--ignore-submodules=all"], cwd, true)?
        || !git_success(
            &["diff", "--cached", "--quiet", "--ignore-submodules=all"],
            cwd,
            true,
        )?;
    if !has_changes {
        return Ok(false);
    }

    let output = Command::new("git")
        .args([
            "stash",
            "push",
            "--include-untracked",
            "-m",
            "tutti: force-land preflight stash",
        ])
        .current_dir(cwd)
        .output()?;
    if output.status.success() {
        Ok(true)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(TuttiError::Git(format!(
            "git stash push failed before force-land: {}",
            stderr.trim()
        )))
    }
}

fn restore_force_land_stash(cwd: &Path) -> Result<()> {
    run_git(&["stash", "pop", "--index"], cwd).map_err(|e| {
        TuttiError::Git(format!(
            "{e}. force-land stash remains; inspect with `git stash list`."
        ))
    })
}

fn ensure_branch_exists(project_root: &Path, branch: &str) -> Result<()> {
    run_git(&["rev-parse", "--verify", branch], project_root).map_err(|_| {
        TuttiError::Git(format!(
            "agent branch '{}' does not exist. Ensure the agent was launched with worktree enabled.",
            branch
        ))
    })
}

fn commit_wip_if_needed(worktree_path: &Path, agent_name: &str) -> Result<bool> {
    let status = git_output(&["status", "--porcelain"], worktree_path)?;
    if status.trim().is_empty() {
        return Ok(false);
    }

    run_git(&["add", "-A"], worktree_path)?;
    let msg = format!("tutti: checkpoint {} before land", agent_name);
    run_git(&["commit", "-m", msg.as_str()], worktree_path).map_err(|e| {
        TuttiError::Git(format!(
            "failed to auto-commit pending changes in worktree: {e}"
        ))
    })?;
    Ok(true)
}

fn run_git(args: &[&str], cwd: &Path) -> Result<()> {
    let output = Command::new("git").args(args).current_dir(cwd).output()?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(TuttiError::Git(format!(
            "git {} failed: {}",
            args.join(" "),
            stderr.trim()
        )))
    }
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

fn git_success(args: &[&str], cwd: &Path, exit_one_is_false: bool) -> Result<bool> {
    let output = Command::new("git").args(args).current_dir(cwd).output()?;
    if output.status.success() {
        return Ok(true);
    }

    if exit_one_is_false && output.status.code() == Some(1) {
        return Ok(false);
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(TuttiError::Git(format!(
        "git {} failed: {}",
        args.join(" "),
        stderr.trim()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn git_ok(repo: &Path, args: &[&str]) {
        let output = Command::new("git").args(args).current_dir(repo).output().unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_stdout(repo: &Path, args: &[&str]) -> String {
        let output = Command::new("git").args(args).current_dir(repo).output().unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim_end().to_string()
    }

    fn init_repo(prefix: &str) -> PathBuf {
        let repo = unique_temp_dir(prefix);
        git_ok(&repo, &["init"]);
        git_ok(&repo, &["config", "user.name", "Tutti Tests"]);
        git_ok(&repo, &["config", "user.email", "tests@example.com"]);
        git_ok(&repo, &["config", "commit.gpgsign", "false"]);

        fs::write(repo.join("tracked.txt"), "base\n").unwrap();
        git_ok(&repo, &["add", "tracked.txt"]);
        git_ok(&repo, &["commit", "-m", "initial"]);
        repo
    }

    #[test]
    fn ensure_git_clean_accepts_clean_repo() {
        let repo = init_repo("tutti-land-clean");

        let result = ensure_git_clean(&repo);

        fs::remove_dir_all(&repo).unwrap();
        assert!(result.is_ok());
    }

    #[test]
    fn ensure_git_clean_rejects_tracked_changes() {
        let repo = init_repo("tutti-land-dirty");
        fs::write(repo.join("tracked.txt"), "changed\n").unwrap();

        let err = ensure_git_clean(&repo).unwrap_err();

        fs::remove_dir_all(&repo).unwrap();
        assert!(matches!(err, TuttiError::Git(message) if message.contains("working tree has tracked changes")));
    }

    #[test]
    fn commit_wip_if_needed_returns_false_when_repo_is_clean() {
        let repo = init_repo("tutti-land-no-wip");

        let committed = commit_wip_if_needed(&repo, "tester").unwrap();

        fs::remove_dir_all(&repo).unwrap();
        assert!(!committed);
    }

    #[test]
    fn commit_wip_if_needed_creates_checkpoint_commit() {
        let repo = init_repo("tutti-land-wip");
        fs::write(repo.join("tracked.txt"), "changed\n").unwrap();
        fs::write(repo.join("new.txt"), "new file\n").unwrap();

        let committed = commit_wip_if_needed(&repo, "tester").unwrap();
        let status = git_stdout(&repo, &["status", "--porcelain"]);
        let message = git_stdout(&repo, &["log", "-1", "--pretty=%s"]);

        fs::remove_dir_all(&repo).unwrap();
        assert!(committed);
        assert!(status.is_empty());
        assert_eq!(message, "tutti: checkpoint tester before land");
    }

    #[test]
    fn ensure_branch_exists_accepts_existing_branch_and_reports_missing_branch() {
        let repo = init_repo("tutti-land-branch");
        let current_branch = git_stdout(&repo, &["rev-parse", "--abbrev-ref", "HEAD"]);

        ensure_branch_exists(&repo, &current_branch).unwrap();

        let err = ensure_branch_exists(&repo, "missing-branch").unwrap_err();

        fs::remove_dir_all(&repo).unwrap();
        assert!(matches!(err, TuttiError::Git(message) if message.contains("agent branch 'missing-branch' does not exist")));
    }
}
