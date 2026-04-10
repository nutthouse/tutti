//! write_file tool — create or overwrite a file.

use super::{Tool, ToolDefinition, ToolOutput, resolve_workdir_path};
use crate::error::{Result, TuttiError};
use serde::Deserialize;
use serde_json::json;
use std::path::Path;

pub struct WriteFileTool;

#[derive(Debug, Deserialize)]
struct WriteFileArgs {
    path: String,
    content: String,
}

impl Tool for WriteFileTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "write_file".into(),
            description: "Create a new file or overwrite an existing file with the given content. Parent directories are created as needed.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to write, relative to the working directory"
                    },
                    "content": {
                        "type": "string",
                        "description": "Full file content to write"
                    }
                },
                "required": ["path", "content"]
            }),
        }
    }

    fn execute(&self, args: &serde_json::Value, workdir: &Path) -> Result<ToolOutput> {
        let args: WriteFileArgs =
            serde_json::from_value(args.clone()).map_err(|e| TuttiError::ToolExecution {
                name: "write_file".into(),
                reason: format!("invalid arguments: {e}"),
            })?;

        let path = resolve_workdir_path(workdir, &args.path).map_err(|e| match e {
            TuttiError::ToolExecution { reason, .. } => TuttiError::ToolExecution {
                name: "write_file".into(),
                reason,
            },
            other => other,
        })?;

        if let Some(parent) = path.parent()
            && !parent.exists()
        {
            std::fs::create_dir_all(parent).map_err(|e| TuttiError::ToolExecution {
                name: "write_file".into(),
                reason: format!("failed to create parent directory: {e}"),
            })?;
        }

        let existed = path.exists();
        std::fs::write(&path, args.content.as_bytes()).map_err(|e| TuttiError::ToolExecution {
            name: "write_file".into(),
            reason: format!("failed to write {}: {e}", args.path),
        })?;

        let message = if existed {
            format!("Overwrote {} ({} bytes)", args.path, args.content.len())
        } else {
            format!("Created {} ({} bytes)", args.path, args.content.len())
        };

        Ok(ToolOutput::text(message).with_files(vec![path]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(args: serde_json::Value, workdir: &Path) -> Result<ToolOutput> {
        WriteFileTool.execute(&args, workdir)
    }

    #[test]
    fn creates_new_file() {
        let tmp = tempfile::tempdir().unwrap();
        let out = run(json!({"path": "new.txt", "content": "hello"}), tmp.path()).unwrap();
        assert!(!out.is_error);
        assert_eq!(out.files_modified.len(), 1);
        let written = std::fs::read_to_string(tmp.path().join("new.txt")).unwrap();
        assert_eq!(written, "hello");
        assert!(out.content.contains("Created"));
    }

    #[test]
    fn overwrites_existing_file() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("existing.txt"), "old").unwrap();
        let out = run(
            json!({"path": "existing.txt", "content": "new"}),
            tmp.path(),
        )
        .unwrap();
        assert!(!out.is_error);
        let written = std::fs::read_to_string(tmp.path().join("existing.txt")).unwrap();
        assert_eq!(written, "new");
        assert!(out.content.contains("Overwrote"));
    }

    #[test]
    fn creates_parent_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let out = run(
            json!({"path": "deeply/nested/new.txt", "content": "x"}),
            tmp.path(),
        )
        .unwrap();
        assert!(!out.is_error);
        assert!(tmp.path().join("deeply/nested/new.txt").exists());
    }

    #[test]
    fn rejects_missing_content_arg() {
        let tmp = tempfile::tempdir().unwrap();
        let result = run(json!({"path": "x.txt"}), tmp.path());
        assert!(result.is_err());
    }
}
