use crate::automation::{
    ExecuteOptions, ExecutionOrigin, ExecutionResult, ResolvedStep, ResolvedWorkflow, StepStatus,
    WorkflowExecutor, WorkflowResolver,
};
use crate::config::TuttiConfig;
use crate::error::{Result, TuttiError};
use comfy_table::{Table, presets::UTF8_BORDERS_ONLY};
use serde::Serialize;

pub fn run(
    workflow: Option<&str>,
    list: bool,
    agent: Option<&str>,
    json: bool,
    strict: bool,
    dry_run: bool,
) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let (config, config_path) = TuttiConfig::load(&cwd)?;
    config.validate()?;

    if list {
        print_workflow_list(&config, json)?;
        return Ok(());
    }

    let workflow = workflow.ok_or_else(|| {
        TuttiError::ConfigValidation("workflow name is required unless --list is set".to_string())
    })?;

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
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serialize_dry_run(&resolved, strict))?
            );
        } else {
            print_dry_run(&resolved, strict);
        }
        return Ok(());
    }

    let executor = WorkflowExecutor::new(project_root);
    let result = executor.execute(&resolved, &options, agent)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        print_execution_result(&result);
    }

    if !result.success {
        return Err(TuttiError::ConfigValidation(format!(
            "workflow '{}' failed (failed steps: {:?})",
            result.workflow_name, result.failed_steps
        )));
    }

    Ok(())
}

#[derive(Debug, Serialize)]
struct WorkflowListItem {
    name: String,
    description: Option<String>,
    steps: usize,
}

fn print_workflow_list(config: &TuttiConfig, as_json: bool) -> Result<()> {
    if config.workflows.is_empty() {
        println!("No workflows configured.");
        println!("Add [[workflow]] entries to tutti.toml.");
        return Ok(());
    }

    if as_json {
        let items: Vec<WorkflowListItem> = config
            .workflows
            .iter()
            .map(|workflow| WorkflowListItem {
                name: workflow.name.clone(),
                description: workflow.description.clone(),
                steps: workflow.steps.len(),
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
        return Ok(());
    }

    let mut table = Table::new();
    table.load_preset(UTF8_BORDERS_ONLY);
    table.set_header(vec!["Workflow", "Steps", "Description"]);

    for workflow in &config.workflows {
        table.add_row(vec![
            workflow.name.clone(),
            workflow.steps.len().to_string(),
            workflow
                .description
                .clone()
                .unwrap_or_else(|| "--".to_string()),
        ]);
    }

    println!("{table}");
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

#[derive(Debug, Serialize)]
struct DryRunPlan {
    workflow: String,
    description: Option<String>,
    strict: bool,
    steps: Vec<DryRunStep>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum DryRunStep {
    Prompt {
        index: usize,
        agent: String,
        summary: String,
    },
    Command {
        index: usize,
        agent: Option<String>,
        cwd: String,
        fail_mode: String,
        summary: String,
    },
}

fn serialize_dry_run(workflow: &ResolvedWorkflow, strict: bool) -> DryRunPlan {
    let mut steps = Vec::with_capacity(workflow.steps.len());
    for (idx, step) in workflow.steps.iter().enumerate() {
        match step {
            ResolvedStep::Prompt { agent, text, .. } => steps.push(DryRunStep::Prompt {
                index: idx + 1,
                agent: agent.clone(),
                summary: text.clone(),
            }),
            ResolvedStep::Command {
                run,
                cwd,
                agent,
                fail_mode,
                ..
            } => steps.push(DryRunStep::Command {
                index: idx + 1,
                agent: agent.clone(),
                cwd: cwd.display().to_string(),
                fail_mode: format!("{:?}", fail_mode).to_lowercase(),
                summary: run.clone(),
            }),
        }
    }
    DryRunPlan {
        workflow: workflow.name.clone(),
        description: workflow.description.clone(),
        strict,
        steps,
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::ResolvedStep;
    use crate::config::WorkflowFailMode;
    use std::path::PathBuf;

    #[test]
    fn serialize_dry_run_contains_prompt_and_command_steps() {
        let workflow = ResolvedWorkflow {
            name: "verify".to_string(),
            description: Some("desc".to_string()),
            steps: vec![
                ResolvedStep::Prompt {
                    agent: "backend".to_string(),
                    text: "check changes".to_string(),
                    session_name: "sess".to_string(),
                },
                ResolvedStep::Command {
                    run: "cargo test".to_string(),
                    cwd: PathBuf::from("/tmp/ws"),
                    agent: Some("backend".to_string()),
                    timeout_secs: 30,
                    fail_mode: WorkflowFailMode::Closed,
                },
            ],
        };

        let plan = serialize_dry_run(&workflow, true);
        assert_eq!(plan.workflow, "verify");
        assert!(plan.strict);
        assert_eq!(plan.steps.len(), 2);
        match &plan.steps[0] {
            DryRunStep::Prompt {
                agent,
                summary,
                index,
                ..
            } => {
                assert_eq!(*index, 1);
                assert_eq!(agent, "backend");
                assert_eq!(summary, "check changes");
            }
            _ => panic!("expected prompt"),
        }
        match &plan.steps[1] {
            DryRunStep::Command {
                index,
                summary,
                fail_mode,
                ..
            } => {
                assert_eq!(*index, 2);
                assert_eq!(summary, "cargo test");
                assert_eq!(fail_mode, "closed");
            }
            _ => panic!("expected command"),
        }
    }
}
