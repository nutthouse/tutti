//! read_file tool — read a file with optional line range.

use super::{Tool, ToolDefinition, ToolOutput, resolve_workdir_path};
use crate::error::{Result, TuttiError};
use serde::Deserialize;
use serde_json::json;
use std::path::Path;

pub struct ReadFileTool;

#[derive(Debug, Deserialize)]
struct ReadFileArgs {
    path: String,
    #[serde(default)]
    start_line: Option<usize>,
    #[serde(default)]
    end_line: Option<usize>,
}

const MAX_BYTES: u64 = 2 * 1024 * 1024; // 2 MB

impl Tool for ReadFileTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "read_file".into(),
            description: "Read the contents of a file. Optionally read a line range via start_line/end_line (1-indexed, inclusive). Files are limited to 2MB.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file, relative to the working directory"
                    },
                    "start_line": {
                        "type": "integer",
                        "description": "First line to read (1-indexed). Optional."
                    },
                    "end_line": {
                        "type": "integer",
                        "description": "Last line to read (inclusive). Optional."
                    }
                },
                "required": ["path"]
            }),
        }
    }

    fn execute(&self, args: &serde_json::Value, workdir: &Path) -> Result<ToolOutput> {
        let args: ReadFileArgs =
            serde_json::from_value(args.clone()).map_err(|e| TuttiError::ToolExecution {
                name: "read_file".into(),
                reason: format!("invalid arguments: {e}"),
            })?;

        let path = resolve_workdir_path(workdir, &args.path).map_err(|e| match e {
            TuttiError::ToolExecution { reason, .. } => TuttiError::ToolExecution {
                name: "read_file".into(),
                reason,
            },
            other => other,
        })?;

        if !path.exists() {
            return Ok(ToolOutput::error(format!("file not found: {}", args.path)));
        }

        let metadata = std::fs::metadata(&path)?;
        if metadata.len() > MAX_BYTES {
            return Ok(ToolOutput::error(format!(
                "file exceeds 2MB limit: {} is {} bytes",
                args.path,
                metadata.len()
            )));
        }

        let contents = std::fs::read_to_string(&path).map_err(|e| TuttiError::ToolExecution {
            name: "read_file".into(),
            reason: format!("failed to read {}: {e}", args.path),
        })?;

        let output = match (args.start_line, args.end_line) {
            (None, None) => format_with_line_numbers(&contents, 1, None),
            (Some(start), end) => {
                let lines: Vec<&str> = contents.lines().collect();
                let start_idx = start.saturating_sub(1);
                if start_idx >= lines.len() {
                    return Ok(ToolOutput::error(format!(
                        "start_line {start} is beyond end of file ({} lines)",
                        lines.len()
                    )));
                }
                let end_idx = end.map(|e| e.min(lines.len())).unwrap_or(lines.len());
                let slice = &lines[start_idx..end_idx];
                format_with_line_numbers(&slice.join("\n"), start, None)
            }
            (None, Some(end)) => {
                let lines: Vec<&str> = contents.lines().collect();
                let end_idx = end.min(lines.len());
                let slice = &lines[..end_idx];
                format_with_line_numbers(&slice.join("\n"), 1, None)
            }
        };

        Ok(ToolOutput::text(output))
    }
}

fn format_with_line_numbers(content: &str, start_line: usize, _cap: Option<usize>) -> String {
    content
        .lines()
        .enumerate()
        .map(|(i, line)| format!("{:>6}  {}", start_line + i, line))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    fn run(args: serde_json::Value, workdir: &Path) -> Result<ToolOutput> {
        ReadFileTool.execute(&args, workdir)
    }

    #[test]
    fn reads_existing_file() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("hello.txt"), "line 1\nline 2\nline 3\n").unwrap();
        let out = run(json!({"path": "hello.txt"}), tmp.path()).unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("line 1"));
        assert!(out.content.contains("line 2"));
        assert!(out.content.contains("line 3"));
    }

    #[test]
    fn missing_file_returns_error_output() {
        let tmp = tempfile::tempdir().unwrap();
        let out = run(json!({"path": "nope.txt"}), tmp.path()).unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("file not found"));
    }

    #[test]
    fn line_range_slices_content() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("f.txt"), "a\nb\nc\nd\ne\n").unwrap();
        let out = run(
            json!({"path": "f.txt", "start_line": 2, "end_line": 4}),
            tmp.path(),
        )
        .unwrap();
        assert!(out.content.contains("b"));
        assert!(out.content.contains("c"));
        assert!(out.content.contains("d"));
        assert!(!out.content.contains("     1  a"));
        assert!(!out.content.contains("     5  e"));
    }

    #[test]
    fn start_beyond_end_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("f.txt"), "a\nb\n").unwrap();
        let out = run(json!({"path": "f.txt", "start_line": 100}), tmp.path()).unwrap();
        assert!(out.is_error);
    }

    #[test]
    fn rejects_oversized_file() {
        let tmp = tempfile::tempdir().unwrap();
        let big = vec![b'x'; (MAX_BYTES + 1) as usize];
        fs::write(tmp.path().join("big.bin"), &big).unwrap();
        let out = run(json!({"path": "big.bin"}), tmp.path()).unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("2MB"));
    }

    #[test]
    fn invalid_arguments_fail_loudly() {
        let tmp = tempfile::tempdir().unwrap();
        let result = run(json!({"wrong_field": 42}), tmp.path());
        assert!(result.is_err());
    }

    #[test]
    fn has_definition_with_required_path() {
        let def = ReadFileTool.definition();
        assert_eq!(def.name, "read_file");
        let required = def.parameters["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "path"));
    }
}
