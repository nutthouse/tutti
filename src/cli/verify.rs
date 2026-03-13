use crate::automation::{
    ExecuteOptions, ExecutionOrigin, WorkflowExecutor, WorkflowResolver, save_verify_summary,
};
use crate::config::TuttiConfig;
use crate::error::{Result, TuttiError};
use crate::state::load_verify_last_summary;
use colored::Colorize;
use comfy_table::{Table, presets::UTF8_BORDERS_ONLY};

pub fn run(last: bool, workflow: Option<&str>, agent: Option<&str>, strict: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let (config, config_path) = TuttiConfig::load(&cwd)?;
    config.validate()?;

    let project_root = config_path.parent().ok_or_else(|| {
        TuttiError::ConfigValidation("could not determine workspace root".to_string())
    })?;

    if last {
        return print_last_summary(project_root);
    }

    let workflow_name = workflow.unwrap_or("verify");
    let options = ExecuteOptions {
        strict,
        // Verify defaults to lenient unless strict is explicitly requested.
        force_open_commands: !strict,
        origin: ExecutionOrigin::Verify,
        hook_event: None,
        hook_agent: None,
    };

    let resolver = WorkflowResolver::new(&config, project_root);
    let resolved = resolver.resolve(workflow_name, agent, &options)?;

    let executor = WorkflowExecutor::new(project_root);
    let result = executor.execute(&resolved, &options, agent)?;

    super::run::print_execution_result(&result);
    save_verify_summary(project_root, workflow_name, strict, agent, &result)?;

    if strict && !result.success {
        return Err(TuttiError::ConfigValidation(format!(
            "verify failed in strict mode (failed steps: {:?})",
            result.failed_steps
        )));
    }

    Ok(())
}

fn print_last_summary(project_root: &std::path::Path) -> Result<()> {
    let Some(summary) = load_verify_last_summary(project_root)? else {
        println!("No verify summary found yet (.tutti/state/verify-last.json).");
        return Ok(());
    };

    let mut table = Table::new();
    table.load_preset(UTF8_BORDERS_ONLY);
    table.set_header(vec!["Field", "Value"]);
    table.add_row(vec!["Workflow".to_string(), summary.workflow_name.clone()]);
    table.add_row(vec![
        "Timestamp".to_string(),
        summary.timestamp.to_rfc3339(),
    ]);
    table.add_row(vec![
        "Result".to_string(),
        if summary.success {
            "PASS".green().to_string()
        } else {
            "FAIL".red().to_string()
        },
    ]);
    table.add_row(vec![
        "Strict".to_string(),
        if summary.strict {
            "true".to_string()
        } else {
            "false".to_string()
        },
    ]);
    table.add_row(vec![
        "Agent Scope".to_string(),
        summary.agent_scope.unwrap_or_else(|| "--".to_string()),
    ]);
    table.add_row(vec![
        "Failed Steps".to_string(),
        format_failed_steps(&summary.failed_steps),
    ]);

    println!("{table}");
    Ok(())
}

fn format_failed_steps(steps: &[usize]) -> String {
    if steps.is_empty() {
        return "--".to_string();
    }
    steps
        .iter()
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_failed_steps_handles_empty_and_values() {
        assert_eq!(format_failed_steps(&[]), "--");
        assert_eq!(format_failed_steps(&[2, 4, 7]), "2, 4, 7");
    }
}
