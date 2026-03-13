use crate::automation::{
    ExecuteOptions, ExecutionOrigin, WorkflowExecutor, WorkflowResolver, save_verify_summary,
};
use crate::config::TuttiConfig;
use crate::error::{Result, TuttiError};

pub fn run(workflow: Option<&str>, agent: Option<&str>, strict: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let (config, config_path) = TuttiConfig::load(&cwd)?;
    config.validate()?;

    let project_root = config_path.parent().ok_or_else(|| {
        TuttiError::ConfigValidation("could not determine workspace root".to_string())
    })?;

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
