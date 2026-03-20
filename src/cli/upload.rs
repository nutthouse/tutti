use crate::cli::agent_ref;
use crate::error::{Result, TuttiError};
use crate::state;
use colored::Colorize;
use std::path::Path;

/// Blocked executable extensions (case-insensitive).
const BLOCKED_EXTENSIONS: &[&str] = &["exe", "sh", "bat", "cmd"];

pub fn run(
    agent_ref_str: &str,
    file: &Path,
    dest: Option<&str>,
    workspace: Option<&str>,
) -> Result<()> {
    // Resolve the agent
    let resolved = if let Some(ws) = workspace {
        let composite = format!("{ws}/{agent_ref_str}");
        agent_ref::resolve(&composite)?
    } else {
        agent_ref::resolve(agent_ref_str)?
    };

    // Validate source file exists and is a file
    if !file.exists() {
        return Err(TuttiError::State(format!(
            "source file does not exist: {}",
            file.display()
        )));
    }
    if !file.is_file() {
        return Err(TuttiError::State(format!(
            "source path is not a file: {}",
            file.display()
        )));
    }

    // Validate file type (reject executables)
    validate_file_type(file)?;

    // Determine agent worktree path
    let worktree_dir = resolved
        .project_root
        .join(".tutti")
        .join("worktrees")
        .join(&resolved.agent_name);

    if !worktree_dir.exists() {
        return Err(TuttiError::Worktree(format!(
            "agent '{}' worktree does not exist at {}; run `tt up {}` first",
            resolved.agent_name,
            worktree_dir.display(),
            resolved.agent_name,
        )));
    }

    // Determine destination path
    let dest_path = if let Some(dest_rel) = dest {
        let dest_path = worktree_dir.join(dest_rel);
        validate_dest_path(&worktree_dir, &dest_path)?;
        // Ensure parent directory exists
        if let Some(parent) = dest_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        dest_path
    } else {
        // Default: .tutti/uploads/{timestamp}-{filename}
        let uploads_dir = state::ensure_uploads_dir(&worktree_dir)?;
        let filename = file
            .file_name()
            .ok_or_else(|| TuttiError::State("cannot determine filename".to_string()))?
            .to_string_lossy();
        let timestamp = chrono::Utc::now().format("%Y%m%d%H%M%S");
        uploads_dir.join(format!("{timestamp}-{filename}"))
    };

    // Copy file
    let bytes_copied = std::fs::copy(file, &dest_path)?;

    println!(
        "{} uploaded {} to {} ({} bytes)",
        "ok".green(),
        file.display(),
        dest_path
            .strip_prefix(&worktree_dir)
            .unwrap_or(&dest_path)
            .display(),
        bytes_copied,
    );

    Ok(())
}

/// Reject files with executable extensions.
fn validate_file_type(file: &Path) -> Result<()> {
    if let Some(ext) = file.extension() {
        let ext_lower = ext.to_string_lossy().to_lowercase();
        if BLOCKED_EXTENSIONS.contains(&ext_lower.as_str()) {
            return Err(TuttiError::ConfigValidation(format!(
                "file type '.{ext_lower}' is not allowed; executable files are blocked by default"
            )));
        }
    }
    Ok(())
}

/// Validate that the destination path stays within the worktree root (no traversal).
fn validate_dest_path(worktree_root: &Path, dest: &Path) -> Result<()> {
    // Canonicalize the worktree root (it exists)
    let canonical_root = worktree_root.canonicalize().map_err(|e| {
        TuttiError::State(format!(
            "cannot resolve worktree root {}: {e}",
            worktree_root.display()
        ))
    })?;

    // For the dest, resolve the longest existing prefix then check
    // We need to handle the case where the file doesn't exist yet
    let mut check_path = dest.to_path_buf();
    while !check_path.exists() {
        if !check_path.pop() {
            break;
        }
    }
    let canonical_prefix = check_path.canonicalize().map_err(|e| {
        TuttiError::State(format!(
            "cannot resolve destination path {}: {e}",
            dest.display()
        ))
    })?;

    if !canonical_prefix.starts_with(&canonical_root) {
        return Err(TuttiError::ConfigValidation(format!(
            "destination path escapes agent worktree: {}",
            dest.display()
        )));
    }

    // Also reject if any component is a symlink pointing outside
    if dest.exists() && dest.is_symlink() {
        let target = std::fs::read_link(dest).map_err(|e| {
            TuttiError::State(format!("cannot read symlink {}: {e}", dest.display()))
        })?;
        let resolved = if target.is_absolute() {
            target
        } else {
            dest.parent().unwrap_or(dest).join(&target)
        };
        if let Ok(canonical_target) = resolved.canonicalize() {
            if !canonical_target.starts_with(&canonical_root) {
                return Err(TuttiError::ConfigValidation(format!(
                    "destination is a symlink escaping the worktree: {}",
                    dest.display()
                )));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn validate_file_type_blocks_executables() {
        let tmp = TempDir::new().unwrap();
        for ext in &["exe", "sh", "bat", "cmd"] {
            let p = tmp.path().join(format!("test.{ext}"));
            fs::write(&p, "").unwrap();
            assert!(
                validate_file_type(&p).is_err(),
                ".{ext} should be blocked"
            );
        }
    }

    #[test]
    fn validate_file_type_allows_normal_files() {
        let tmp = TempDir::new().unwrap();
        for ext in &["txt", "png", "jpg", "rs", "toml", "json", "md"] {
            let p = tmp.path().join(format!("test.{ext}"));
            fs::write(&p, "").unwrap();
            assert!(
                validate_file_type(&p).is_ok(),
                ".{ext} should be allowed"
            );
        }
    }

    #[test]
    fn validate_dest_path_rejects_traversal() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("worktree");
        fs::create_dir_all(&root).unwrap();
        let bad_dest = root.join("..").join("escape.txt");
        assert!(validate_dest_path(&root, &bad_dest).is_err());
    }

    #[test]
    fn validate_dest_path_accepts_nested() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("worktree");
        fs::create_dir_all(&root).unwrap();
        let good_dest = root.join("subdir").join("file.txt");
        // subdir doesn't exist yet, but that's fine — we check the existing prefix
        assert!(validate_dest_path(&root, &good_dest).is_ok());
    }

    #[test]
    fn validate_file_type_allows_no_extension() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("Makefile");
        fs::write(&p, "").unwrap();
        assert!(validate_file_type(&p).is_ok());
    }
}
