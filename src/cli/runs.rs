use crate::error::{Result, TuttiError};
use crate::state::{load_active_runs, load_run_steps, load_sdlc_run_ledger};
use comfy_table::{presets::UTF8_BORDERS_ONLY, Table};

pub fn list() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let (_config, config_path) = crate::config::TuttiConfig::load(&cwd)?;
    let project_root = config_path.parent().ok_or_else(|| {
        TuttiError::ConfigValidation("could not determine workspace root".to_string())
    })?;

    let runs = load_active_runs(project_root)?;
    if runs.is_empty() {
        println!("No tracked runs.");
        return Ok(());
    }

    let mut table = Table::new();
    table.load_preset(UTF8_BORDERS_ONLY);
    table.set_header(vec![
        "Run", "Issue", "State", "Updated", "Branch", "Failure",
    ]);

    for run in runs {
        table.add_row(vec![
            truncate_run_id(&run.run_id),
            format_issue(&run),
            format!("{:?}", run.state).to_lowercase(),
            run.updated_at.to_rfc3339(),
            run.branch.unwrap_or_else(|| "--".to_string()),
            run.failure_class.unwrap_or_else(|| "--".to_string()),
        ]);
    }

    println!("{table}");
    Ok(())
}

pub fn show(run_id: &str) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let (_config, config_path) = crate::config::TuttiConfig::load(&cwd)?;
    let project_root = config_path.parent().ok_or_else(|| {
        TuttiError::ConfigValidation("could not determine workspace root".to_string())
    })?;
    let ledger = load_sdlc_run_ledger(project_root, run_id)?
        .ok_or_else(|| TuttiError::ConfigValidation(format!("run '{}' not found", run_id)))?;

    // Header block
    println!("Run: {}", ledger.run_id);
    println!(
        "Issue: #{} {}",
        ledger.issue_number,
        ledger.issue_title.as_deref().unwrap_or("")
    );
    println!("Workflow: {}", ledger.workflow_name);
    println!("State: {:?}", ledger.state);
    println!("Updated: {}", ledger.updated_at.to_rfc3339());
    println!(
        "Branch: {}",
        ledger.branch.as_deref().unwrap_or("--")
    );
    println!(
        "Current step: {}",
        ledger.current_step_id.as_deref().unwrap_or("--")
    );
    println!(
        "Last successful step: {}",
        ledger.last_successful_step_id.as_deref().unwrap_or("--")
    );
    println!(
        "Failure: {}",
        ledger.failure_class.as_deref().unwrap_or("--")
    );
    if let Some(message) = ledger.failure_message.as_deref() {
        println!("Failure message: {message}");
    }
    println!(
        "Resume eligible: {}",
        if ledger.resume_eligible { "yes" } else { "no" }
    );
    println!(
        "Active agents: {}",
        if ledger.active_agents.is_empty() {
            "--".to_string()
        } else {
            ledger.active_agents.join(", ")
        }
    );

    // Step table
    let steps = load_run_steps(project_root, run_id)?;
    if !steps.is_empty() {
        println!("\nSteps:");
        let mut step_table = Table::new();
        step_table.load_preset(UTF8_BORDERS_ONLY);
        step_table.set_header(vec![
            "#", "Step", "Type", "Status", "Duration", "Agent", "Failure",
        ]);

        for step in &steps {
            let (status, duration, failure_msg) = match &step.outcome {
                Some(outcome) => {
                    let status = if outcome.timed_out {
                        "timed_out".to_string()
                    } else if outcome.success {
                        "success".to_string()
                    } else {
                        "failed".to_string()
                    };
                    let dur = outcome
                        .completed_at
                        .signed_duration_since(step.planned_at)
                        .num_milliseconds();
                    let duration = format_duration_ms(dur);
                    let failure = if outcome.success {
                        "--".to_string()
                    } else {
                        outcome.message.as_deref().unwrap_or("--").to_string()
                    };
                    (status, duration, failure)
                }
                None => ("pending".to_string(), "--".to_string(), "--".to_string()),
            };

            let agent = step
                .intent
                .get("agent_scope")
                .and_then(|v| v.as_str())
                .unwrap_or("--");

            step_table.add_row(vec![
                step.step_index.to_string(),
                step.step_id.clone(),
                step.step_type.clone(),
                status,
                duration,
                agent.to_string(),
                failure_msg,
            ]);
        }

        println!("{step_table}");
    }

    // Next action guidance
    let next_action = derive_next_action(&ledger, &steps);
    println!("\nNext action: {next_action}");

    // Transitions
    if !ledger.transitions.is_empty() {
        println!("\nTransitions:");
        for transition in &ledger.transitions {
            println!(
                "- {:?} -> {:?} @ {} by {}{}",
                transition.from,
                transition.to,
                transition.timestamp.to_rfc3339(),
                transition.actor,
                transition
                    .reason
                    .as_ref()
                    .map(|reason| format!(" ({reason})"))
                    .unwrap_or_default()
            );
        }
    }

    Ok(())
}

fn derive_next_action(
    ledger: &crate::state::SdlcRunLedgerRecord,
    steps: &[crate::state::WorkflowStepIntentRecord],
) -> String {
    let failing_step = steps
        .iter()
        .find(|s| s.outcome.as_ref().is_some_and(|o| !o.success));

    if let Some(step) = failing_step {
        let agent = step.intent.get("agent_scope").and_then(|v| v.as_str());
        if let Some(agent_name) = agent {
            return format!(
                "Inspect agent '{}' transcript for step '{}'",
                agent_name, step.step_id
            );
        }
        return format!("Inspect step '{}' output", step.step_id);
    }

    if ledger.resume_eligible {
        return format!("Run: tt run --resume {}", ledger.run_id);
    }

    let has_pending = steps.iter().any(|s| s.outcome.is_none());
    if has_pending {
        return "Run is still in progress — wait for pending steps to complete".to_string();
    }

    "All steps completed successfully".to_string()
}

fn format_duration_ms(ms: i64) -> String {
    if ms < 0 {
        return "--".to_string();
    }
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{:.1}m", ms as f64 / 60_000.0)
    }
}

fn truncate_run_id(run_id: &str) -> String {
    if run_id.chars().count() <= 12 {
        return run_id.to_string();
    }
    let mut out = run_id.chars().take(9).collect::<String>();
    out.push_str("...");
    out
}

fn format_issue(run: &crate::state::SdlcRunLedgerRecord) -> String {
    match run.issue_title.as_deref() {
        Some(title) if !title.trim().is_empty() => format!("#{} {}", run.issue_number, title),
        _ => format!("#{}", run.issue_number),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{SdlcRunLedgerRecord, SdlcRunState};
    use chrono::Utc;

    fn stub_record(issue_title: Option<&str>) -> SdlcRunLedgerRecord {
        SdlcRunLedgerRecord {
            run_id: "1234567890123-99999".to_string(),
            issue_number: 42,
            issue_title: issue_title.map(|s| s.to_string()),
            repository: "test/repo".to_string(),
            workflow_name: "sdlc-auto".to_string(),
            state: SdlcRunState::Selected,
            updated_at: Utc::now(),
            actor: "test".to_string(),
            branch: None,
            failure_message: None,
            failure_class: None,
            current_step_id: None,
            last_successful_step_id: None,
            resume_eligible: false,
            active_agents: Vec::new(),
            transitions: Vec::new(),
        }
    }

    #[test]
    fn truncate_run_id_short() {
        assert_eq!(truncate_run_id("abc"), "abc");
        assert_eq!(truncate_run_id("exactly12chr"), "exactly12chr");
    }

    #[test]
    fn truncate_run_id_long() {
        assert_eq!(truncate_run_id("1234567890123-99999"), "123456789...");
    }

    #[test]
    fn format_issue_with_title() {
        let run = stub_record(Some("fix the bug"));
        assert_eq!(format_issue(&run), "#42 fix the bug");
    }

    #[test]
    fn format_issue_without_title() {
        let run = stub_record(None);
        assert_eq!(format_issue(&run), "#42");
    }

    #[test]
    fn format_issue_with_blank_title() {
        let run = stub_record(Some("   "));
        assert_eq!(format_issue(&run), "#42");
    }

    #[test]
    fn format_duration_ms_values() {
        assert_eq!(format_duration_ms(500), "500ms");
        assert_eq!(format_duration_ms(1500), "1.5s");
        assert_eq!(format_duration_ms(90000), "1.5m");
        assert_eq!(format_duration_ms(-1), "--");
    }

    #[test]
    fn derive_next_action_all_success() {
        let ledger = stub_record(None);
        assert_eq!(
            derive_next_action(&ledger, &[]),
            "All steps completed successfully"
        );
    }

    #[test]
    fn derive_next_action_resume_eligible() {
        let mut ledger = stub_record(None);
        ledger.resume_eligible = true;
        assert!(derive_next_action(&ledger, &[]).contains("tt run --resume"));
    }
}
