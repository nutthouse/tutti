use crate::config::WebhookConfig;
use serde_json::Value;
use std::path::Path;

/// Find all webhook configs that match a given source and event type.
pub fn match_triggers<'a>(
    webhooks: &'a [WebhookConfig],
    source: &str,
    event_type: &str,
) -> Vec<&'a WebhookConfig> {
    webhooks
        .iter()
        .filter(|wh| {
            if wh.source != source && wh.source != "*" {
                return false;
            }
            if wh.events.is_empty() {
                return true;
            }
            wh.events.iter().any(|e| e == "*" || e == event_type)
        })
        .collect()
}

/// Expand `{{event.field}}` and `{{event.nested.field}}` placeholders in a
/// template string using values from the JSON payload.
pub fn expand_template(template: &str, payload: &Value) -> String {
    let mut result = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{event.") {
        result.push_str(&rest[..start]);
        let after_open = &rest[start + 2..]; // skip "{{"
        if let Some(end) = after_open.find("}}") {
            let key = &after_open[..end]; // e.g. "event.issue.number"
            let path = key.strip_prefix("event.").unwrap_or(key);
            let value = resolve_json_path(payload, path);
            result.push_str(&value);
            rest = &after_open[end + 2..];
        } else {
            result.push_str(&rest[start..]);
            rest = "";
        }
    }
    result.push_str(rest);
    result
}

/// Walk a dotted path into a JSON value, returning the leaf as a string.
fn resolve_json_path(value: &Value, path: &str) -> String {
    let mut current = value;
    for segment in path.split('.') {
        match current.get(segment) {
            Some(v) => current = v,
            None => return String::new(),
        }
    }
    match current {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Append a webhook event record to the JSONL log file.
pub fn log_event(
    project_root: &Path,
    source: &str,
    event_type: &str,
    matched_rule: Option<&str>,
    outcome: &str,
) {
    use std::io::Write;
    let log_path = project_root
        .join(".tutti")
        .join("state")
        .join("webhook-events.jsonl");
    let record = serde_json::json!({
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "source": source,
        "event": event_type,
        "matched_rule": matched_rule,
        "outcome": outcome,
    });
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        let _ = writeln!(f, "{}", record);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WebhookConfig;
    use serde_json::json;

    fn wh(source: &str, events: &[&str], workflow: Option<&str>) -> WebhookConfig {
        WebhookConfig {
            source: source.to_string(),
            events: events.iter().map(|s| s.to_string()).collect(),
            workflow: workflow.map(|s| s.to_string()),
            agent: None,
            prompt: None,
        }
    }

    #[test]
    fn match_by_source_and_event() {
        let hooks = vec![
            wh("github", &["issues.labeled", "push"], Some("sdlc")),
            wh("slack", &["message"], Some("deploy")),
            wh("generic", &["*"], Some("catch-all")),
        ];
        let matched = match_triggers(&hooks, "github", "push");
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].workflow.as_deref(), Some("sdlc"));
    }

    #[test]
    fn wildcard_source_matches_all() {
        let hooks = vec![wh("*", &["push"], Some("any-push"))];
        let matched = match_triggers(&hooks, "github", "push");
        assert_eq!(matched.len(), 1);
    }

    #[test]
    fn wildcard_event_matches_all() {
        let hooks = vec![wh("generic", &["*"], Some("catch-all"))];
        let matched = match_triggers(&hooks, "generic", "anything");
        assert_eq!(matched.len(), 1);
    }

    #[test]
    fn empty_events_matches_all() {
        let hooks = vec![wh("generic", &[], Some("no-filter"))];
        let matched = match_triggers(&hooks, "generic", "whatever");
        assert_eq!(matched.len(), 1);
    }

    #[test]
    fn no_match_returns_empty() {
        let hooks = vec![wh("github", &["push"], Some("ci"))];
        let matched = match_triggers(&hooks, "slack", "message");
        assert!(matched.is_empty());
    }

    #[test]
    fn expand_template_simple() {
        let payload = json!({"action": "labeled", "number": 42});
        assert_eq!(
            expand_template("Issue #{{event.number}} was {{event.action}}", &payload),
            "Issue #42 was labeled"
        );
    }

    #[test]
    fn expand_template_nested() {
        let payload = json!({"issue": {"number": 7, "title": "Fix bug"}});
        assert_eq!(
            expand_template(
                "Fix {{event.issue.title}} (#{{event.issue.number}})",
                &payload
            ),
            "Fix Fix bug (#7)"
        );
    }

    #[test]
    fn expand_template_missing_key() {
        let payload = json!({"action": "opened"});
        assert_eq!(
            expand_template("{{event.missing}} happened", &payload),
            " happened"
        );
    }

    #[test]
    fn expand_template_no_placeholders() {
        let payload = json!({});
        assert_eq!(expand_template("plain text", &payload), "plain text");
    }

    #[test]
    fn log_event_creates_file() {
        let dir = std::env::temp_dir().join(format!("tutti-webhook-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".tutti").join("state")).unwrap();
        log_event(&dir, "generic", "test.event", Some("rule-1"), "dispatched");
        let log_path = dir
            .join(".tutti")
            .join("state")
            .join("webhook-events.jsonl");
        let content = std::fs::read_to_string(&log_path).unwrap();
        assert!(content.contains("\"source\":\"generic\""));
        assert!(content.contains("\"outcome\":\"dispatched\""));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
