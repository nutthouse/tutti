use crate::error::{Result, TuttiError};
use chrono::Utc;
use std::path::Path;
use std::process::Command;

pub fn run(agent_ref: &str, reviewer: &str) -> Result<()> {
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

    let merge_base = git_output(
        &["merge-base", "HEAD", branch.as_str()],
        &resolved.project_root,
    )
    .unwrap_or_else(|_| "HEAD".to_string());
    let committed_stat = git_output(
        &["diff", "--stat", &format!("{merge_base}..{branch}")],
        &resolved.project_root,
    )
    .unwrap_or_default();
    let committed_diff = git_output(
        &["diff", &format!("{merge_base}..{branch}")],
        &resolved.project_root,
    )
    .unwrap_or_default();
    let wip_stat = git_output(&["diff", "--stat"], &worktree_path).unwrap_or_default();
    let wip_diff = git_output(&["diff"], &worktree_path).unwrap_or_default();

    if committed_diff.trim().is_empty() && wip_diff.trim().is_empty() {
        println!(
            "No changes found for {}. Nothing to send for review.",
            resolved.agent_name
        );
        return Ok(());
    }

    let packet = ReviewPacketData {
        agent_name: resolved.agent_name.clone(),
        branch,
        merge_base,
        committed_stat,
        committed_diff,
        wip_stat,
        wip_diff,
    };
    let packet_path = write_review_packet(&resolved.project_root, &packet)?;

    let prompt = format!(
        "Review changes from agent '{}'. Read {} and return prioritized findings (bugs, regressions, missing tests) with file/line references.",
        resolved.agent_name,
        packet_path.display()
    );

    super::send::run(
        reviewer,
        &[prompt],
        super::send::SendOptions {
            auto_up: false,
            wait: false,
            timeout_secs: 900,
            idle_stable_secs: 5,
            output: false,
            output_lines: 200,
        },
    )?;
    println!(
        "Queued review request with {reviewer} for {}.",
        resolved.agent_name
    );
    println!("Review packet: {}", packet_path.display());

    Ok(())
}

struct ReviewPacketData {
    agent_name: String,
    branch: String,
    merge_base: String,
    committed_stat: String,
    committed_diff: String,
    wip_stat: String,
    wip_diff: String,
}

fn write_review_packet(
    project_root: &Path,
    packet: &ReviewPacketData,
) -> Result<std::path::PathBuf> {
    let reviews_dir = project_root.join(".tutti").join("state").join("reviews");
    std::fs::create_dir_all(&reviews_dir)?;
    let timestamp = Utc::now().format("%Y%m%d-%H%M%S");
    let packet_path = reviews_dir.join(format!("{}-review-{timestamp}.md", packet.agent_name));

    let mut content = String::new();
    content.push_str(&format!("# Review Packet: {}\n\n", packet.agent_name));
    content.push_str(&format!("Generated: {}\n\n", Utc::now().to_rfc3339()));
    content.push_str(&format!("Branch: {}\n", packet.branch));
    content.push_str(&format!("Merge base: {}\n\n", packet.merge_base));
    content.push_str("## Committed Diff Stat\n\n");
    content.push_str("```text\n");
    content.push_str(if packet.committed_stat.trim().is_empty() {
        "(none)"
    } else {
        &packet.committed_stat
    });
    content.push_str("\n```\n\n");
    content.push_str("## Worktree WIP Diff Stat\n\n");
    content.push_str("```text\n");
    content.push_str(if packet.wip_stat.trim().is_empty() {
        "(none)"
    } else {
        &packet.wip_stat
    });
    content.push_str("\n```\n\n");
    content.push_str("## Committed Diff\n\n");
    content.push_str("```diff\n");
    content.push_str(&truncate(&packet.committed_diff, 120_000));
    content.push_str("\n```\n\n");
    content.push_str("## Worktree WIP Diff\n\n");
    content.push_str("```diff\n");
    content.push_str(&truncate(&packet.wip_diff, 120_000));
    content.push_str("\n```\n");

    std::fs::write(&packet_path, content)?;
    Ok(packet_path)
}

fn truncate(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    let mut out: String = input.chars().take(max_chars).collect();
    out.push_str("\n... [truncated by tt review]");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_short_input_unchanged() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_exact_limit_unchanged() {
        let s = "abcde";
        assert_eq!(truncate(s, 5), "abcde");
    }

    #[test]
    fn truncate_over_limit_appends_marker() {
        let result = truncate("abcdef", 3);
        assert!(result.starts_with("abc"));
        assert!(result.contains("[truncated by tt review]"));
    }

    #[test]
    fn truncate_empty_input() {
        assert_eq!(truncate("", 10), "");
    }

    #[test]
    fn truncate_multibyte_chars() {
        // Ensure char-based truncation, not byte-based
        let s = "héllo wörld";
        let result = truncate(s, 5);
        assert!(result.starts_with("héllo"));
        assert!(result.contains("[truncated by tt review]"));
    }

    #[test]
    fn write_review_packet_creates_file_with_expected_sections() {
        let tmp = std::env::temp_dir().join("tutti-test-review-packet");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let packet = ReviewPacketData {
            agent_name: "backend".to_string(),
            branch: "tutti/backend".to_string(),
            merge_base: "abc123".to_string(),
            committed_stat: " src/main.rs | 5 +++++".to_string(),
            committed_diff: "+new line".to_string(),
            wip_stat: String::new(),
            wip_diff: String::new(),
        };

        let path = write_review_packet(&tmp, &packet).unwrap();
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("# Review Packet: backend"));
        assert!(content.contains("Branch: tutti/backend"));
        assert!(content.contains("Merge base: abc123"));
        assert!(content.contains("## Committed Diff Stat"));
        assert!(content.contains("src/main.rs | 5 +++++"));
        assert!(content.contains("## Worktree WIP Diff Stat"));
        assert!(content.contains("(none)")); // wip_stat is empty
        assert!(content.contains("+new line"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn write_review_packet_truncates_large_diffs() {
        let tmp = std::env::temp_dir().join("tutti-test-review-trunc");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let big_diff = "x".repeat(200_000);
        let packet = ReviewPacketData {
            agent_name: "big".to_string(),
            branch: "tutti/big".to_string(),
            merge_base: "def456".to_string(),
            committed_stat: String::new(),
            committed_diff: big_diff,
            wip_stat: String::new(),
            wip_diff: String::new(),
        };

        let path = write_review_packet(&tmp, &packet).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("[truncated by tt review]"));
        // The file should be smaller than the raw diff
        assert!(content.len() < 200_000);

        let _ = std::fs::remove_dir_all(&tmp);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_short_input_unchanged() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_exact_limit_unchanged() {
        assert_eq!(truncate("abcde", 5), "abcde");
    }

    #[test]
    fn truncate_over_limit_adds_suffix() {
        let result = truncate("abcdef", 3);
        assert!(result.starts_with("abc"));
        assert!(result.contains("[truncated by tt review]"));
    }

    #[test]
    fn truncate_empty_input() {
        assert_eq!(truncate("", 10), "");
    }

    #[test]
    fn truncate_handles_multibyte_chars() {
        let input = "àéî";
        let result = truncate(input, 2);
        assert!(result.starts_with("àé"));
        assert!(result.contains("[truncated by tt review]"));
    }

    #[test]
    fn write_review_packet_renders_markdown_sections() {
        let dir = std::env::temp_dir().join("tutti-test-review-packet");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".tutti").join("state")).unwrap();

        let packet = ReviewPacketData {
            agent_name: "coder".to_string(),
            branch: "tutti/coder".to_string(),
            merge_base: "abc123".to_string(),
            committed_stat: " src/main.rs | 2 +-".to_string(),
            committed_diff: "+new line".to_string(),
            wip_stat: String::new(),
            wip_diff: String::new(),
        };

        let path = write_review_packet(&dir, &packet).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();

        assert!(content.contains("# Review Packet: coder"));
        assert!(content.contains("Branch: tutti/coder"));
        assert!(content.contains("Merge base: abc123"));
        assert!(content.contains("## Committed Diff Stat"));
        assert!(content.contains("src/main.rs | 2 +-"));
        assert!(content.contains("## Worktree WIP Diff Stat"));
        assert!(content.contains("(none)"));
        assert!(content.contains("+new line"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
