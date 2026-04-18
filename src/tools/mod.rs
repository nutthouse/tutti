//! Tool executor for the API-direct agent framework.
//!
//! Tools are invoked by the agent loop when the model emits a tool_use
//! content block. Each tool is a sync function that takes JSON arguments
//! and returns a textual output plus metadata about files modified.
//!
//! Tools are sync by design — the agent loop wraps them in `tokio::task::
//! spawn_blocking` only if async is needed. For the MVP sync loop, tools
//! run directly.

// This module is wired into the CLI in Week 2 (see the Direct step
// variant in automation/mod.rs). Until then, clippy flags public items
// as unused — silence those reports here.
#![allow(dead_code)]

use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub mod apply_patch;
pub mod grep_glob;
pub mod read_file;
pub mod shell;
pub mod write_file;

/// JSON Schema definition for a tool, sent to model APIs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    /// JSON Schema for the tool's input arguments.
    pub parameters: serde_json::Value,
}

/// Result of executing a tool.
#[derive(Debug, Clone)]
pub struct ToolOutput {
    /// Textual output shown to the model as the tool result.
    pub content: String,
    /// Paths of files that were created or modified.
    pub files_modified: Vec<PathBuf>,
    /// Whether this execution represents an error condition the model
    /// should be aware of. Non-error tool calls with failed exit codes
    /// (e.g. a test command returning 1) can still set `is_error = true`.
    pub is_error: bool,
}

impl ToolOutput {
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            files_modified: Vec::new(),
            is_error: false,
        }
    }

    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            files_modified: Vec::new(),
            is_error: true,
        }
    }

    pub fn with_files(mut self, files: Vec<PathBuf>) -> Self {
        self.files_modified = files;
        self
    }
}

/// Trait implemented by every tool the agent loop can invoke.
pub trait Tool: Send + Sync {
    /// The tool's JSON Schema definition. Sent to the model as part
    /// of the `tools` array on every request.
    fn definition(&self) -> ToolDefinition;

    /// Execute the tool with JSON arguments supplied by the model.
    /// `workdir` is the directory the agent is operating in — tools
    /// resolve relative paths against it.
    fn execute(&self, args: &serde_json::Value, workdir: &Path) -> Result<ToolOutput>;
}

/// Resolve a path argument against the agent's working directory.
/// Rejects paths that escape the workdir via `..` or absolute paths
/// pointing outside it — defense in depth, not a security boundary.
pub(crate) fn resolve_workdir_path(workdir: &Path, raw: &str) -> Result<PathBuf> {
    let candidate = Path::new(raw);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        workdir.join(candidate)
    };

    // Walk up `joined` until we find an existing ancestor, canonicalize
    // that ancestor, then reattach the remaining path. This works for
    // both existing files (canonicalizes the full path) and
    // not-yet-created files (canonicalizes the nearest real parent).
    let canonical = canonicalize_with_new_tail(&joined)?;

    // Workdir containment check. If the workdir can't be canonicalized
    // (e.g., it was deleted after the agent started), we cannot verify
    // containment — reject absolute paths in that case as a safety
    // measure. For relative paths, the logical join already guarantees
    // the result is rooted at the workdir.
    match workdir.canonicalize() {
        Ok(workdir_canonical) => {
            if !canonical.starts_with(&workdir_canonical) {
                return Err(crate::error::TuttiError::ToolExecution {
                    name: "<unknown>".into(),
                    reason: format!("path escapes working directory: {raw}"),
                });
            }
        }
        Err(_) => {
            if candidate.is_absolute() {
                return Err(crate::error::TuttiError::ToolExecution {
                    name: "<unknown>".into(),
                    reason: format!(
                        "cannot verify containment (workdir unreadable) — refusing absolute path: {raw}"
                    ),
                });
            }
        }
    }

    Ok(canonical)
}

/// Canonicalize a path that may point to a file that doesn't exist yet.
/// Walks up the path until finding an existing ancestor, canonicalizes
/// it, then reattaches the remaining path segments.
fn canonicalize_with_new_tail(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return path.canonicalize().map_err(crate::error::TuttiError::Io);
    }

    let mut tail = std::collections::VecDeque::new();
    let mut cursor: PathBuf = path.to_path_buf();
    loop {
        match cursor.parent() {
            Some(parent) if parent.as_os_str().is_empty() => {
                // Root or drive letter reached without finding an
                // existing ancestor. Return the raw path.
                return Ok(path.to_path_buf());
            }
            Some(parent) => {
                if parent.exists() {
                    let parent_canonical = parent
                        .canonicalize()
                        .map_err(crate::error::TuttiError::Io)?;
                    let mut result = parent_canonical;
                    if let Some(name) = cursor.file_name() {
                        result.push(name);
                    }
                    while let Some(segment) = tail.pop_front() {
                        result.push(segment);
                    }
                    return Ok(result);
                }
                if let Some(name) = cursor.file_name() {
                    tail.push_front(name.to_os_string());
                }
                cursor = parent.to_path_buf();
            }
            None => return Ok(path.to_path_buf()),
        }
    }
}

/// Convenience factory returning the five built-in tools.
pub fn builtin_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(read_file::ReadFileTool),
        Box::new(write_file::WriteFileTool),
        Box::new(apply_patch::ApplyPatchTool),
        Box::new(shell::ShellTool::default()),
        Box::new(grep_glob::GrepGlobTool),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_output_text_is_not_error() {
        let out = ToolOutput::text("hello");
        assert_eq!(out.content, "hello");
        assert!(!out.is_error);
        assert!(out.files_modified.is_empty());
    }

    #[test]
    fn tool_output_error_flags_error() {
        let out = ToolOutput::error("boom");
        assert!(out.is_error);
    }

    #[test]
    fn resolve_path_rejects_escape() {
        let tmp = std::env::temp_dir();
        let result = resolve_workdir_path(&tmp, "../../../etc/passwd");
        assert!(result.is_err(), "should reject escape attempt");
    }

    #[test]
    fn resolve_path_allows_nested() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("sub")).unwrap();
        let result = resolve_workdir_path(tmp.path(), "sub/file.txt");
        assert!(result.is_ok());
    }

    #[test]
    fn builtin_tools_has_five() {
        let tools = builtin_tools();
        assert_eq!(tools.len(), 5);
        let names: Vec<String> = tools.iter().map(|t| t.definition().name).collect();
        assert!(names.contains(&"read_file".to_string()));
        assert!(names.contains(&"write_file".to_string()));
        assert!(names.contains(&"apply_patch".to_string()));
        assert!(names.contains(&"shell".to_string()));
        assert!(names.contains(&"grep_glob".to_string()));
    }
}
