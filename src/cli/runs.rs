use crate::error::{Result, TuttiError};
use crate::state::{load_active_runs, load_sdlc_run_ledger};
use comfy_table::{Table, presets::UTF8_BORDERS_ONLY};

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

    println!("Run: {}", ledger.run_id);
    println!(
        "Issue: #{} {}",
        ledger.issue_number,
        ledger.issue_title.unwrap_or_default()
    );
    println!("Workflow: {}", ledger.workflow_name);
    println!("State: {:?}", ledger.state);
    println!("Updated: {}", ledger.updated_at.to_rfc3339());
    println!(
        "Branch: {}",
        ledger.branch.unwrap_or_else(|| "--".to_string())
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

    if !ledger.transitions.is_empty() {
        println!("\nTransitions:");
        for transition in ledger.transitions {
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
            state: SdlcRunState::InProgress,
            updated_at: Utc::now(),
            actor: "test".to_string(),
            branch: None,
            failure_message: None,
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
}
