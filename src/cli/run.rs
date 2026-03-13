use crate::automation::{
    ExecuteOptions, ExecutionOrigin, ExecutionResult, StepStatus, WorkflowExecutor,
    WorkflowResolver,
};
use crate::config::TuttiConfig;
use crate::error::{Result, TuttiError};
use comfy_table::{Table, presets::UTF8_BORDERS_ONLY};

pub fn run(workflow: &str, agent: Option<&str>, strict: bool, dry_run: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let (config, config_path) = TuttiConfig::load(&cwd)?;
    config.validate()?;

    let project_root = config_path.parent().ok_or_else(|| {
        TuttiError::ConfigValidation("could not determine workspace root".to_string())
    })?;

    let options = ExecuteOptions {
        strict,
        force_open_commands: false,
        origin: ExecutionOrigin::Run,
        hook_event: None,
        hook_agent: None,
    };

    let resolver = WorkflowResolver::new(&config, project_root);
    let resolved = resolver.resolve(workflow, agent, &options)?;

    if dry_run {
        print_dry_run(&resolved, strict);
        return Ok(());
    }

    let executor = WorkflowExecutor::new(project_root);
    let result = executor.execute(&resolved, &options, agent)?;
    print_execution_result(&result);

    if !result.success {
        return Err(TuttiError::ConfigValidation(format!(
            "workflow '{}' failed (failed steps: {:?})",
            result.workflow_name, result.failed_steps
        )));
    }

    Ok(())
}

fn print_dry_run(workflow: &crate::automation::ResolvedWorkflow, strict: bool) {
    println!("Workflow: {}", workflow.name);
    if let Some(desc) = &workflow.description {
        println!("  {}", desc);
    }
    println!("  Mode: {}", if strict { "strict" } else { "normal" });
    println!();

    let mut table = Table::new();
    table.load_preset(UTF8_BORDERS_ONLY);
    table.set_header(vec!["#", "Type", "Agent", "CWD", "Fail", "Summary"]);

    for (idx, step) in workflow.steps.iter().enumerate() {
        match step {
            crate::automation::ResolvedStep::Prompt { agent, text, .. } => table.add_row(vec![
                (idx + 1).to_string(),
                "prompt".to_string(),
                agent.clone(),
                "session".to_string(),
                "closed".to_string(),
                truncate(text, 80),
            ]),
            crate::automation::ResolvedStep::Command {
                run,
                cwd,
                agent,
                fail_mode,
                ..
            } => table.add_row(vec![
                (idx + 1).to_string(),
                "command".to_string(),
                agent.clone().unwrap_or_else(|| "--".to_string()),
                cwd.display().to_string(),
                format!("{:?}", fail_mode).to_lowercase(),
                truncate(run, 80),
            ]),
        };
    }

    println!("{table}");
}

pub fn print_execution_result(result: &ExecutionResult) {
    println!("Workflow: {}", result.workflow_name);

    let mut table = Table::new();
    table.load_preset(UTF8_BORDERS_ONLY);
    table.set_header(vec!["#", "Type", "Status", "Duration", "Message"]);

    for step in &result.step_results {
        let status = match step.status {
            StepStatus::Success => "success",
            StepStatus::Warning => "warning",
            StepStatus::Failed => "failed",
        };
        table.add_row(vec![
            step.index.to_string(),
            step.step_type.clone(),
            status.to_string(),
            format!("{}ms", step.duration_ms),
            step.message.clone().unwrap_or_default(),
        ]);
    }

    println!("{table}");
    if result.success {
        if result.warning_count() > 0 {
            println!("Result: success with {} warning(s)", result.warning_count());
        } else {
            println!("Result: success");
        }
    } else {
        println!("Result: failed (steps: {:?})", result.failed_steps);
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out = s.chars().take(max.saturating_sub(3)).collect::<String>();
    out.push_str("...");
    out
}
