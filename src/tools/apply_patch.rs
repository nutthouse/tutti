//! apply_patch tool — old_string → new_string replacement.
//!
//! Fails if:
//! - the file doesn't exist
//! - `old_string` is not found
//! - `old_string` appears more than once (ambiguous replacement)
//!
//! This matches the semantics of Claude Agent SDK's `Edit` tool: exact
//! string replacement with unambiguous matching. Line endings are
//! normalized to the file's dominant style before comparison so
//! models can send either CRLF or LF content.

use super::{Tool, ToolDefinition, ToolOutput, resolve_workdir_path};
use crate::error::{Result, TuttiError};
use serde::Deserialize;
use serde_json::json;
use std::path::Path;

pub struct ApplyPatchTool;

#[derive(Debug, Deserialize)]
struct ApplyPatchArgs {
    path: String,
    old_string: String,
    new_string: String,
}

impl Tool for ApplyPatchTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "apply_patch".into(),
            description: "Apply an edit to a file by replacing `old_string` with `new_string`. The `old_string` must appear EXACTLY ONCE in the file — if it appears zero or multiple times, the edit fails and you should read_file to get more context and make `old_string` more specific (include surrounding lines).".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to edit, relative to the working directory"
                    },
                    "old_string": {
                        "type": "string",
                        "description": "Exact text to find in the file. Must match verbatim and appear exactly once."
                    },
                    "new_string": {
                        "type": "string",
                        "description": "Replacement text"
                    }
                },
                "required": ["path", "old_string", "new_string"]
            }),
        }
    }

    fn execute(&self, args: &serde_json::Value, workdir: &Path) -> Result<ToolOutput> {
        let args: ApplyPatchArgs =
            serde_json::from_value(args.clone()).map_err(|e| TuttiError::ToolExecution {
                name: "apply_patch".into(),
                reason: format!("invalid arguments: {e}"),
            })?;

        if args.old_string == args.new_string {
            return Ok(ToolOutput::error(
                "old_string and new_string are identical — no change to apply".to_string(),
            ));
        }

        let path = resolve_workdir_path(workdir, &args.path).map_err(|e| match e {
            TuttiError::ToolExecution { reason, .. } => TuttiError::ToolExecution {
                name: "apply_patch".into(),
                reason,
            },
            other => other,
        })?;

        if !path.exists() {
            return Ok(ToolOutput::error(format!("file not found: {}", args.path)));
        }

        let contents = std::fs::read_to_string(&path).map_err(|e| TuttiError::ToolExecution {
            name: "apply_patch".into(),
            reason: format!("failed to read {}: {e}", args.path),
        })?;

        // Normalize the model's old_string against the file's line ending
        // style before comparing — models often emit LF even when the file
        // is CRLF.
        let (file_contents, normalized_old, normalized_new) =
            normalize_line_endings(&contents, &args.old_string, &args.new_string);

        let occurrences = count_occurrences(&file_contents, &normalized_old);
        match occurrences {
            0 => Ok(ToolOutput::error(format!(
                "old_string not found in {}. Read the file and make old_string more specific.",
                args.path
            ))),
            1 => {
                let replaced = file_contents.replacen(&normalized_old, &normalized_new, 1);
                std::fs::write(&path, replaced.as_bytes()).map_err(|e| {
                    TuttiError::ToolExecution {
                        name: "apply_patch".into(),
                        reason: format!("failed to write {}: {e}", args.path),
                    }
                })?;
                Ok(ToolOutput::text(format!(
                    "Applied patch to {} (replaced {} chars with {} chars)",
                    args.path,
                    normalized_old.len(),
                    normalized_new.len()
                ))
                .with_files(vec![path]))
            }
            n => Ok(ToolOutput::error(format!(
                "old_string appears {n} times in {} — ambiguous. Add surrounding context to make it unique.",
                args.path
            ))),
        }
    }
}

/// Normalize `old_string` and `new_string` to match the file's line ending
/// style. Returns (file_contents, normalized_old, normalized_new).
fn normalize_line_endings(file_contents: &str, old: &str, new: &str) -> (String, String, String) {
    let file_uses_crlf = file_contents.contains("\r\n");
    let old_uses_crlf = old.contains("\r\n");
    let new_uses_crlf = new.contains("\r\n");

    let normalized_old = if file_uses_crlf && !old_uses_crlf {
        old.replace('\n', "\r\n")
    } else if !file_uses_crlf && old_uses_crlf {
        old.replace("\r\n", "\n")
    } else {
        old.to_string()
    };

    let normalized_new = if file_uses_crlf && !new_uses_crlf {
        new.replace('\n', "\r\n")
    } else if !file_uses_crlf && new_uses_crlf {
        new.replace("\r\n", "\n")
    } else {
        new.to_string()
    };

    (file_contents.to_string(), normalized_old, normalized_new)
}

fn count_occurrences(haystack: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    let mut count = 0;
    let mut rest = haystack;
    while let Some(idx) = rest.find(needle) {
        count += 1;
        rest = &rest[idx + needle.len()..];
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(args: serde_json::Value, workdir: &Path) -> Result<ToolOutput> {
        ApplyPatchTool.execute(&args, workdir)
    }

    #[test]
    fn replaces_unique_string() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f.txt");
        std::fs::write(&path, "hello world\ngoodbye\n").unwrap();
        let out = run(
            json!({
                "path": "f.txt",
                "old_string": "hello world",
                "new_string": "hi there"
            }),
            tmp.path(),
        )
        .unwrap();
        assert!(!out.is_error, "unexpected error: {}", out.content);
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(after, "hi there\ngoodbye\n");
    }

    #[test]
    fn fails_when_string_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("f.txt"), "one\n").unwrap();
        let out = run(
            json!({
                "path": "f.txt",
                "old_string": "two",
                "new_string": "three"
            }),
            tmp.path(),
        )
        .unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("not found"));
    }

    #[test]
    fn fails_on_ambiguous_match() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("f.txt"), "foo\nfoo\nfoo\n").unwrap();
        let out = run(
            json!({
                "path": "f.txt",
                "old_string": "foo",
                "new_string": "bar"
            }),
            tmp.path(),
        )
        .unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("3 times"));
    }

    #[test]
    fn identical_strings_is_noop_error() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("f.txt"), "foo\n").unwrap();
        let out = run(
            json!({
                "path": "f.txt",
                "old_string": "foo",
                "new_string": "foo"
            }),
            tmp.path(),
        )
        .unwrap();
        assert!(out.is_error);
    }

    #[test]
    fn handles_crlf_file_with_lf_old_string() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f.txt");
        std::fs::write(&path, "line one\r\nline two\r\n").unwrap();
        let out = run(
            json!({
                "path": "f.txt",
                "old_string": "line one\nline two",
                "new_string": "replaced"
            }),
            tmp.path(),
        )
        .unwrap();
        assert!(!out.is_error, "unexpected: {}", out.content);
        let after = std::fs::read(&path).unwrap();
        let after_s = String::from_utf8(after).unwrap();
        assert!(after_s.contains("replaced"));
    }

    #[test]
    fn fails_on_missing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let out = run(
            json!({
                "path": "nope.txt",
                "old_string": "x",
                "new_string": "y"
            }),
            tmp.path(),
        )
        .unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("file not found"));
    }
}
