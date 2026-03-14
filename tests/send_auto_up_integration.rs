use std::fs;
use std::os::unix::fs::PermissionsExt;
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

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).expect("write executable");
    let mut perms = fs::metadata(path).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).expect("chmod +x");
}

fn run_tt(cwd: &Path, config_home: &Path, args: &[&str]) -> Output {
    let bin = env!("CARGO_BIN_EXE_tt");
    Command::new(bin)
        .current_dir(cwd)
        .env("HOME", config_home)
        .env("XDG_CONFIG_HOME", config_home)
        .args(args)
        .output()
        .expect("run tt command")
}

fn normalize_no_ws(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

#[test]
fn send_auto_up_wait_output_preserves_long_prompt() {
    if Command::new("tmux").arg("-V").output().is_err() {
        // Skip if tmux is unavailable in this environment.
        return;
    }

    let root = unique_temp_dir("tutti-send-auto-up");
    let workspace = root.join("workspace");
    let config_home = root.join("config-home");
    fs::create_dir_all(&workspace).expect("create workspace");
    fs::create_dir_all(config_home.join(".config/tutti")).expect("create config dir");

    let runtime_script = root.join("mock-claude-runtime");
    write_executable(
        &runtime_script,
        "#!/bin/sh\n\
         echo \"What would you like to do?\"\n\
         while IFS= read -r line; do\n\
           echo \"$line\"\n\
           echo \"What would you like to do?\"\n\
         done\n",
    );

    let global_config = format!(
        "[[profile]]\n\
         name = \"test\"\n\
         provider = \"anthropic\"\n\
         command = \"{}\"\n",
        runtime_script.display()
    );
    fs::write(config_home.join(".config/tutti/config.toml"), global_config)
        .expect("write global config");

    let workspace_name = format!("itest-send-{}", std::process::id());
    let workspace_config = format!(
        "[workspace]\n\
         name = \"{}\"\n\n\
         [workspace.auth]\n\
         default_profile = \"test\"\n\n\
         [defaults]\n\
         worktree = false\n\
         runtime = \"claude-code\"\n\n\
         [launch]\n\
         mode = \"safe\"\n\n\
         [[agent]]\n\
         name = \"etl\"\n\
         runtime = \"claude-code\"\n\
         persistent = true\n",
        workspace_name
    );
    fs::write(workspace.join("tutti.toml"), workspace_config).expect("write workspace config");

    let _ = run_tt(&workspace, &config_home, &["down", "etl"]);

    let prompt = format!(
        "TUTTI_PROMPT_BEGIN_{}{}_TUTTI_PROMPT_END",
        "X".repeat(300),
        "Y".repeat(300)
    );
    let send = run_tt(
        &workspace,
        &config_home,
        &[
            "send",
            "--auto-up",
            "--wait",
            "--timeout-secs",
            "30",
            "--idle-stable-secs",
            "1",
            "--output",
            "--output-lines",
            "400",
            "etl",
            &prompt,
        ],
    );

    if !send.status.success() {
        panic!(
            "tt send failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&send.stdout),
            String::from_utf8_lossy(&send.stderr)
        );
    }

    let peek = run_tt(&workspace, &config_home, &["peek", "etl", "--lines", "5000"]);
    if !peek.status.success() {
        panic!(
            "tt peek failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&peek.stdout),
            String::from_utf8_lossy(&peek.stderr)
        );
    }

    let peek_out = String::from_utf8_lossy(&peek.stdout);
    let normalized_stdout = normalize_no_ws(&peek_out);
    let normalized_prompt = normalize_no_ws(&prompt);
    assert!(
        normalized_stdout.contains(&normalized_prompt),
        "pane output missing full long prompt\npeek:\n{}",
        peek_out
    );

    let _ = run_tt(&workspace, &config_home, &["down", "etl"]);
    let _ = fs::remove_dir_all(&root);
}
