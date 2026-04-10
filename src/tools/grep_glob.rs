//! grep_glob tool — search files by glob pattern and/or content regex.
//!
//! Respects .gitignore and similar via the `ignore` crate. Results are
//! capped to prevent runaway output.

use super::{Tool, ToolDefinition, ToolOutput, resolve_workdir_path};
use crate::error::{Result, TuttiError};
use ignore::WalkBuilder;
use serde::Deserialize;
use serde_json::json;
use std::path::Path;

pub struct GrepGlobTool;

#[derive(Debug, Deserialize)]
struct GrepGlobArgs {
    #[serde(default)]
    glob: Option<String>,
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    case_sensitive: Option<bool>,
}

const MAX_FILES_SCANNED: usize = 2000;
const MAX_MATCHES: usize = 500;
const MAX_CONTENT_BYTES: u64 = 512 * 1024;

impl Tool for GrepGlobTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "grep_glob".into(),
            description: "Search files by glob pattern and/or content regex. Provide at least one of `glob` (e.g. \"**/*.rs\") or `pattern` (regex searched in file contents). Respects .gitignore.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "glob": {
                        "type": "string",
                        "description": "Glob pattern to match file paths against (e.g. \"**/*.rs\", \"src/**/test_*.py\"). If omitted, all non-ignored files are scanned."
                    },
                    "pattern": {
                        "type": "string",
                        "description": "Regex to search for in file contents. If omitted, just lists matching file paths."
                    },
                    "path": {
                        "type": "string",
                        "description": "Subdirectory to search under. Defaults to the working directory."
                    },
                    "case_sensitive": {
                        "type": "boolean",
                        "description": "Case-sensitive content matching. Defaults to false."
                    }
                }
            }),
        }
    }

    fn execute(&self, args: &serde_json::Value, workdir: &Path) -> Result<ToolOutput> {
        let args: GrepGlobArgs =
            serde_json::from_value(args.clone()).map_err(|e| TuttiError::ToolExecution {
                name: "grep_glob".into(),
                reason: format!("invalid arguments: {e}"),
            })?;

        if args.glob.is_none() && args.pattern.is_none() {
            return Ok(ToolOutput::error(
                "must provide at least one of `glob` or `pattern`".to_string(),
            ));
        }

        let search_root = match args.path.as_deref() {
            Some(p) => resolve_workdir_path(workdir, p).map_err(|e| match e {
                TuttiError::ToolExecution { reason, .. } => TuttiError::ToolExecution {
                    name: "grep_glob".into(),
                    reason,
                },
                other => other,
            })?,
            None => workdir.to_path_buf(),
        };
        if !search_root.exists() {
            return Ok(ToolOutput::error(format!(
                "search path does not exist: {}",
                search_root.display()
            )));
        }

        let glob_matcher = args
            .glob
            .as_deref()
            .map(|g| {
                glob::Pattern::new(g).map_err(|e| TuttiError::ToolExecution {
                    name: "grep_glob".into(),
                    reason: format!("invalid glob: {e}"),
                })
            })
            .transpose()?;

        let content_regex = args
            .pattern
            .as_deref()
            .map(|p| {
                let case_sensitive = args.case_sensitive.unwrap_or(false);
                let prefix = if case_sensitive { "" } else { "(?i)" };
                regex_lite_compile(&format!("{prefix}{p}"))
            })
            .transpose()?;

        let mut files_scanned = 0usize;
        let mut results: Vec<String> = Vec::new();
        let mut matches_found = 0usize;

        let walker = WalkBuilder::new(&search_root)
            .hidden(false)
            .git_ignore(true)
            .build();

        for entry in walker {
            if files_scanned >= MAX_FILES_SCANNED || matches_found >= MAX_MATCHES {
                break;
            }
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                continue;
            }
            files_scanned += 1;

            let path = entry.path();
            let rel = path.strip_prefix(workdir).unwrap_or(path);
            let rel_str = rel.display().to_string();

            if let Some(g) = &glob_matcher
                && !g.matches(&rel_str)
            {
                continue;
            }

            if let Some(regex) = &content_regex {
                let metadata = match std::fs::metadata(path) {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                if metadata.len() > MAX_CONTENT_BYTES {
                    continue;
                }
                let contents = match std::fs::read_to_string(path) {
                    Ok(c) => c,
                    Err(_) => continue, // likely binary
                };
                for (lineno, line) in contents.lines().enumerate() {
                    if regex.is_match(line) {
                        results.push(format!("{rel_str}:{}: {}", lineno + 1, line.trim_end()));
                        matches_found += 1;
                        if matches_found >= MAX_MATCHES {
                            break;
                        }
                    }
                }
            } else {
                results.push(rel_str);
                matches_found += 1;
            }
        }

        if results.is_empty() {
            return Ok(ToolOutput::text(format!(
                "no matches (scanned {files_scanned} files)"
            )));
        }

        let mut body = results.join("\n");
        body.push_str(&format!(
            "\n\n{} results (scanned {files_scanned} files{})",
            results.len(),
            if matches_found >= MAX_MATCHES {
                ", capped at 500 matches"
            } else if files_scanned >= MAX_FILES_SCANNED {
                ", capped at 2000 files"
            } else {
                ""
            }
        ));
        Ok(ToolOutput::text(body))
    }
}

/// Compile a regex via the `regex` crate. We use a thin wrapper to make
/// error conversion consistent with the rest of the tool.
fn regex_lite_compile(pattern: &str) -> Result<regex::Regex> {
    regex::Regex::new(pattern).map_err(|e| TuttiError::ToolExecution {
        name: "grep_glob".into(),
        reason: format!("invalid regex: {e}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(args: serde_json::Value, workdir: &Path) -> Result<ToolOutput> {
        GrepGlobTool.execute(&args, workdir)
    }

    fn make_fixture() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("src")).unwrap();
        std::fs::write(
            tmp.path().join("src/main.rs"),
            "fn main() { println!(\"hi\"); }\n",
        )
        .unwrap();
        std::fs::write(tmp.path().join("src/lib.rs"), "pub fn greet() {}\n").unwrap();
        std::fs::write(tmp.path().join("README.md"), "# Project\n").unwrap();
        std::fs::write(tmp.path().join("notes.txt"), "TODO: fix this\n").unwrap();
        tmp
    }

    #[test]
    fn glob_lists_matching_files() {
        let tmp = make_fixture();
        let out = run(json!({"glob": "src/*.rs"}), tmp.path()).unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("src/main.rs"));
        assert!(out.content.contains("src/lib.rs"));
        assert!(!out.content.contains("README.md"));
    }

    #[test]
    fn pattern_greps_contents() {
        let tmp = make_fixture();
        let out = run(json!({"pattern": "TODO"}), tmp.path()).unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("notes.txt"));
        assert!(out.content.contains("TODO: fix this"));
    }

    #[test]
    fn glob_and_pattern_combine() {
        let tmp = make_fixture();
        let out = run(json!({"glob": "**/*.rs", "pattern": "println"}), tmp.path()).unwrap();
        assert!(out.content.contains("main.rs"));
        assert!(!out.content.contains("lib.rs"));
    }

    #[test]
    fn no_matches_returns_friendly_message() {
        let tmp = make_fixture();
        let out = run(json!({"pattern": "NEVER_APPEARS_xyz"}), tmp.path()).unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("no matches"));
    }

    #[test]
    fn missing_both_args_errors() {
        let tmp = make_fixture();
        let out = run(json!({}), tmp.path()).unwrap();
        assert!(out.is_error);
    }

    #[test]
    fn case_insensitive_by_default() {
        let tmp = make_fixture();
        let out = run(json!({"pattern": "todo"}), tmp.path()).unwrap();
        assert!(out.content.contains("TODO"));
    }

    #[test]
    fn respects_gitignore() {
        let tmp = tempfile::tempdir().unwrap();
        // Initialize a git repo so ignore picks up .gitignore
        std::process::Command::new("git")
            .arg("init")
            .current_dir(tmp.path())
            .output()
            .ok();
        std::fs::write(tmp.path().join(".gitignore"), "secret.txt\n").unwrap();
        std::fs::write(tmp.path().join("visible.txt"), "hello\n").unwrap();
        std::fs::write(tmp.path().join("secret.txt"), "password123\n").unwrap();
        let out = run(json!({"pattern": ".*"}), tmp.path()).unwrap();
        assert!(out.content.contains("visible.txt"));
        assert!(
            !out.content.contains("secret.txt") || !out.content.contains("password123"),
            "secret.txt should be filtered by .gitignore"
        );
    }
}
