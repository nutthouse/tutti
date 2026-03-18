use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn run_tt(cwd: &Path, args: &[&str]) -> Output {
    let bin = env!("CARGO_BIN_EXE_tt");
    Command::new(bin)
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("run tt command")
}

#[test]
fn sdlc_auto_dry_run_exposes_structured_handoff_artifacts() {
    let root = unique_temp_dir("tutti-sdlc-auto-dry-run");
    let workspace = root.join("workspace");
    fs::create_dir_all(&workspace).expect("create workspace");

    let example = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("docs/examples/tutti-codex-sdlc.toml");
    fs::copy(example, workspace.join("tutti.toml")).expect("copy example config");

    fs::create_dir_all(workspace.join(".tutti/worktrees/planner")).expect("create planner worktree");
    fs::create_dir_all(workspace.join(".tutti/worktrees/implementer"))
        .expect("create implementer worktree");

    let output = run_tt(&workspace, &["run", "sdlc-auto", "--dry-run", "--json"]);
    if !output.status.success() {
        panic!(
            "tt run --dry-run failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let plan: Value = serde_json::from_slice(&output.stdout).expect("parse dry-run json");
    let steps = plan["steps"].as_array().expect("steps array");

    let plan_issue = steps
        .iter()
        .find(|step| {
            step["type"] == "prompt"
                && step["summary"]
                    .as_str()
                    .is_some_and(|summary| summary.contains("plan_issue.json"))
        })
        .expect("plan_issue prompt step");
    assert!(
        plan_issue["output_json"]
            .as_str()
            .is_some_and(|path| path.ends_with(".tutti/state/auto/plan_issue.json"))
    );

    let implement_code = steps
        .iter()
        .find(|step| {
            step["type"] == "prompt"
                && step["summary"]
                    .as_str()
                    .is_some_and(|summary| summary.contains("{{output.plan_issue.path}}"))
        })
        .expect("implement_code prompt step");
    assert!(
        implement_code["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("implement_result.json"))
    );
    assert!(
        implement_code["output_json"]
            .as_str()
            .is_some_and(|path| path.ends_with(".tutti/state/auto/implement_result.json"))
    );

    let _ = fs::remove_dir_all(&root);
}
