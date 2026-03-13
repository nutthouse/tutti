use crate::automation::{
    ExecuteOptions, ExecutionOrigin, WorkflowResolver, execute_workflow_with_hooks,
};
use crate::config::TuttiConfig;
use crate::error::Result;
use crate::state;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use cron::Schedule;
use std::collections::HashSet;
use std::str::FromStr;

pub fn parse_schedule(expr: &str) -> Result<Schedule> {
    let normalized = normalize_schedule(expr)?;
    let schedule = Schedule::from_str(&normalized).map_err(|e| {
        crate::error::TuttiError::ConfigValidation(format!("cron parse failed: {e}"))
    })?;
    Ok(schedule)
}

pub fn run_due_workflows_for_workspace(
    config: &TuttiConfig,
    project_root: &std::path::Path,
    in_flight: &mut HashSet<String>,
) -> Result<Vec<String>> {
    let mut events = Vec::new();
    let mut last_runs = state::load_scheduler_last_runs(project_root)?;
    let resolver = WorkflowResolver::new(config, project_root);
    let now = Utc::now();

    for workflow in config.workflows.iter().filter(|w| w.schedule.is_some()) {
        let schedule_expr = workflow
            .schedule
            .as_deref()
            .unwrap_or_default()
            .trim()
            .to_string();
        if schedule_expr.is_empty() {
            continue;
        }

        let schedule = parse_schedule(&schedule_expr)?;
        let key = format!("{}/{}", config.workspace.name, workflow.name);
        if in_flight.contains(&key) {
            continue;
        }

        let last = last_runs
            .get(&key)
            .copied()
            .unwrap_or_else(|| now - ChronoDuration::minutes(1));
        let due = next_due_at(&schedule, last).is_some_and(|next| next <= now);
        if !due {
            continue;
        }

        in_flight.insert(key.clone());
        let options = ExecuteOptions {
            strict: false,
            force_open_commands: false,
            origin: ExecutionOrigin::ObserveCycle,
            hook_event: None,
            hook_agent: None,
        };

        let resolved = resolver.resolve(&workflow.name, None, &options)?;
        match execute_workflow_with_hooks(config, project_root, &resolved, &options, None, None) {
            Ok(result) => {
                events.push(format!(
                    "scheduled workflow '{}' ({})",
                    result.workflow_name,
                    if result.success { "ok" } else { "failed" }
                ));
                last_runs.insert(key.clone(), now);
            }
            Err(e) => {
                events.push(format!(
                    "scheduled workflow '{}' failed: {e}",
                    workflow.name
                ));
                last_runs.insert(key.clone(), now);
            }
        }
        in_flight.remove(&key);
    }

    state::save_scheduler_last_runs(project_root, &last_runs)?;
    Ok(events)
}

fn normalize_schedule(expr: &str) -> Result<String> {
    let trimmed = expr.trim();
    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    if parts.len() != 5 {
        return Err(crate::error::TuttiError::ConfigValidation(
            "expected 5-field cron expression".to_string(),
        ));
    }
    Ok(format!("0 {trimmed}"))
}

fn next_due_at(schedule: &Schedule, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
    schedule.after(&after).next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_schedule_accepts_five_field_cron() {
        let sched = parse_schedule("*/15 * * * *").unwrap();
        let now = Utc::now();
        assert!(next_due_at(&sched, now - ChronoDuration::minutes(1)).is_some());
    }

    #[test]
    fn parse_schedule_rejects_bad_field_count() {
        let err = parse_schedule("* * * *").unwrap_err();
        assert!(err.to_string().contains("5-field cron"));
    }
}
