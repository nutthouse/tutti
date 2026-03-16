use crate::error::{Result, TuttiError};
use serde_json::Value;
use std::path::Path;
use std::process::Command;

const ENFORCE_MERGE_GATE_ENV: &str = "TT_ENFORCE_MERGE_GATE";

pub fn run(agent_ref: &str, pr: bool, force: bool) -> Result<()> {
    run_with_options(agent_ref, pr, force, merge_gate_enabled())
}

pub fn run_with_options(
    agent_ref: &str,
    pr: bool,
    force: bool,
    enforce_merge_gate: bool,
) -> Result<()> {
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
    maybe_enforce_merge_gate(&resolved.project_root, &branch, enforce_merge_gate)?;
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

fn maybe_enforce_merge_gate(project_root: &Path, branch: &str, enabled: bool) -> Result<()> {
    if !enabled {
        return Ok(());
    }

    if which::which("gh").is_err() {
        return Err(TuttiError::ConfigValidation(
            "merge gate enabled, but `gh` is not installed. Install GitHub CLI or disable TT_ENFORCE_MERGE_GATE."
                .to_string(),
        ));
    }

    let pr_number = find_open_pr_number(project_root, branch)?;
    ensure_required_checks_green(project_root, pr_number)?;
    ensure_all_review_threads_resolved(project_root, pr_number)?;

    println!(
        "Merge gate passed for PR #{} on branch '{}': required checks green, review threads resolved.",
        pr_number, branch
    );
    Ok(())
}

fn merge_gate_enabled() -> bool {
    matches!(
        std::env::var(ENFORCE_MERGE_GATE_ENV)
            .ok()
            .as_deref()
            .map(|v| v.trim().to_ascii_lowercase()),
        Some(v) if v == "1" || v == "true" || v == "yes" || v == "on"
    )
}

fn find_open_pr_number(project_root: &Path, branch: &str) -> Result<u64> {
    let output = Command::new("gh")
        .args([
            "pr",
            "list",
            "--head",
            branch,
            "--state",
            "open",
            "--limit",
            "1",
            "--json",
            "number,url",
        ])
        .current_dir(project_root)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(TuttiError::Git(format!(
            "merge gate failed to query PR for branch '{}': {}",
            branch,
            stderr.trim()
        )));
    }

    let prs: Value = serde_json::from_slice(&output.stdout).map_err(|e| {
        TuttiError::State(format!("merge gate failed to parse `gh pr list` output: {e}"))
    })?;
    let number = parse_pr_number(&prs).ok_or_else(|| {
        TuttiError::Git(format!(
            "merge gate requires an open PR for branch '{}', but none was found.",
            branch
        ))
    })?;

    Ok(number)
}

fn ensure_required_checks_green(project_root: &Path, pr_number: u64) -> Result<()> {
    let output = Command::new("gh")
        .args([
            "pr",
            "checks",
            &pr_number.to_string(),
            "--required",
        ])
        .current_dir(project_root)
        .output()?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if !stderr.is_empty() { stderr } else { stdout };

    Err(TuttiError::Git(format!(
        "merge gate blocked: required checks are not green for PR #{}{}",
        pr_number,
        if detail.is_empty() {
            String::new()
        } else {
            format!(" ({detail})")
        }
    )))
}

fn ensure_all_review_threads_resolved(project_root: &Path, pr_number: u64) -> Result<()> {
    let repo = gh_repo_name_with_owner(project_root)?;
    let (owner, name) = split_name_with_owner(&repo)?;

    let query = r#"query($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      reviewThreads(first: 100) {
        nodes {
          isResolved
        }
      }
    }
  }
}"#;

    let output = Command::new("gh")
        .args(["api", "graphql", "-f", &format!("query={query}")])
        .args(["-F", &format!("owner={owner}")])
        .args(["-F", &format!("name={name}")])
        .args(["-F", &format!("number={pr_number}")])
        .current_dir(project_root)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(TuttiError::Git(format!(
            "merge gate failed to query review threads for PR #{}: {}",
            pr_number,
            stderr.trim()
        )));
    }

    let payload: Value = serde_json::from_slice(&output.stdout).map_err(|e| {
        TuttiError::State(format!(
            "merge gate failed to parse review thread payload for PR #{}: {e}",
            pr_number
        ))
    })?;

    let unresolved = unresolved_review_thread_count(&payload).ok_or_else(|| {
        TuttiError::State(format!(
            "merge gate could not determine unresolved review thread count for PR #{}",
            pr_number
        ))
    })?;

    if unresolved == 0 {
        Ok(())
    } else {
        Err(TuttiError::Git(format!(
            "merge gate blocked: PR #{} has {} unresolved review thread(s). Resolve all threads before land/merge.",
            pr_number, unresolved
        )))
    }
}

fn gh_repo_name_with_owner(project_root: &Path) -> Result<String> {
    let output = Command::new("gh")
        .args(["repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner"])
        .current_dir(project_root)
        .output()?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(TuttiError::Git(format!(
            "merge gate failed to detect repo owner/name: {}",
            stderr.trim()
        )))
    }
}

fn split_name_with_owner(name_with_owner: &str) -> Result<(String, String)> {
    let (owner, name) = name_with_owner.split_once('/').ok_or_else(|| {
        TuttiError::State(format!(
            "invalid GitHub repo slug '{}'; expected owner/name",
            name_with_owner
        ))
    })?;
    Ok((owner.to_string(), name.to_string()))
}

fn parse_pr_number(payload: &Value) -> Option<u64> {
    payload.as_array()?.first()?.get("number")?.as_u64()
}

fn unresolved_review_thread_count(payload: &Value) -> Option<usize> {
    let nodes = payload
        .get("data")?
        .get("repository")?
        .get("pullRequest")?
        .get("reviewThreads")?
        .get("nodes")?
        .as_array()?;

    Some(
        nodes
            .iter()
            .filter(|node| node.get("isResolved").and_then(Value::as_bool) == Some(false))
            .count(),
    )
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

    #[test]
    fn parse_pr_number_returns_first_open_pr() {
        let payload: Value = serde_json::from_str(
            r#"[
                {"number": 42, "url": "https://github.com/nutthouse/tutti/pull/42"}
            ]"#,
        )
        .unwrap();

        assert_eq!(parse_pr_number(&payload), Some(42));
    }

    #[test]
    fn unresolved_review_thread_count_counts_false_flags() {
        let payload: Value = serde_json::from_str(
            r#"{
                "data": {
                    "repository": {
                        "pullRequest": {
                            "reviewThreads": {
                                "nodes": [
                                    {"isResolved": true},
                                    {"isResolved": false},
                                    {"isResolved": false}
                                ]
                            }
                        }
                    }
                }
            }"#,
        )
        .unwrap();

        assert_eq!(unresolved_review_thread_count(&payload), Some(2));
    }

    #[test]
    fn split_name_with_owner_validates_repo_slug() {
        let (owner, repo) = split_name_with_owner("nutthouse/tutti").unwrap();
        assert_eq!(owner, "nutthouse");
        assert_eq!(repo, "tutti");
    }
}
