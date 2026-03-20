use crate::config::WebhookConfig;
use crate::error::Result;
use serde_json::Value;
use std::collections::HashSet;
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
        let after_open = &rest[start + 2..];
        if let Some(end) = after_open.find("}}") {
            let key = &after_open[..end];
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

/// Derive a delivery ID from well-known webhook headers or fall back to a
/// SHA-256 hash of the payload.  Accepts pre-extracted header values so this
/// module stays independent of the HTTP library.
pub fn delivery_id(
    delivery_header: Option<&str>,
    idempotency_header: Option<&str>,
    payload: &Value,
) -> Option<String> {
    // Prefer explicit delivery / idempotency headers
    if let Some(h) = delivery_header.filter(|v| !v.trim().is_empty()) {
        return Some(h.to_string());
    }
    if let Some(h) = idempotency_header.filter(|v| !v.trim().is_empty()) {
        return Some(h.to_string());
    }
    // Fall back to SHA-256 of the canonical payload JSON
    let bytes = serde_json::to_vec(payload).ok()?;
    use std::fmt::Write;
    let digest = sha256(&bytes);
    let mut hex = String::with_capacity(64);
    for b in &digest {
        let _ = write!(hex, "{b:02x}");
    }
    Some(format!("sha256:{hex}"))
}

/// Minimal SHA-256 — uses the `ring` crate if available, otherwise falls back
/// to a pure-Rust implementation bundled here so we avoid adding a new dep.
fn sha256(data: &[u8]) -> [u8; 32] {
    // Simple inline SHA-256 (no external dep needed — this is a small utility)
    sha256_impl(data)
}

/// Check whether a delivery ID has already been processed.
pub fn is_replay(project_root: &Path, id: &str) -> Result<bool> {
    let file = replay_file(project_root);
    if !file.exists() {
        return Ok(false);
    }
    let body = std::fs::read_to_string(&file)?;
    let set: HashSet<String> = serde_json::from_str(&body).unwrap_or_default();
    Ok(set.contains(id))
}

/// Record a delivery ID so future duplicates are detected.
pub fn record_delivery(project_root: &Path, id: &str) -> Result<()> {
    let file = replay_file(project_root);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut set: HashSet<String> = if file.exists() {
        let body = std::fs::read_to_string(&file)?;
        serde_json::from_str(&body).unwrap_or_default()
    } else {
        HashSet::new()
    };
    set.insert(id.to_string());

    // Cap the replay set to prevent unbounded growth (keep last 10 000 entries)
    if set.len() > 10_000 {
        let excess = set.len() - 10_000;
        let to_remove: Vec<String> = set.iter().take(excess).cloned().collect();
        for key in to_remove {
            set.remove(&key);
        }
    }

    let json = serde_json::to_string(&set)?;
    std::fs::write(&file, json)?;
    Ok(())
}

fn replay_file(project_root: &Path) -> std::path::PathBuf {
    project_root
        .join(".tutti")
        .join("state")
        .join("webhook-replay.json")
}

/// Append a webhook event record to the JSONL log file.
pub fn log_event(
    project_root: &Path,
    source: &str,
    event_type: &str,
    matched_rule: Option<&str>,
    outcome: &str,
) -> Result<()> {
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
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    writeln!(f, "{}", record)?;
    Ok(())
}

// ── Inline SHA-256 ──────────────────────────────────────────────────────────

fn sha256_impl(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    // Pre-processing: pad message
    let bit_len = (data.len() as u64) * 8;
    let mut msg = data.to_vec();
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    // Process each 512-bit block
    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[4 * i],
                chunk[4 * i + 1],
                chunk[4 * i + 2],
                chunk[4 * i + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut out = [0u8; 32];
    for (i, val) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&val.to_be_bytes());
    }
    out
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
        log_event(&dir, "generic", "test.event", Some("rule-1"), "dispatched").unwrap();
        let log_path = dir
            .join(".tutti")
            .join("state")
            .join("webhook-events.jsonl");
        let content = std::fs::read_to_string(&log_path).unwrap();
        assert!(content.contains("\"source\":\"generic\""));
        assert!(content.contains("\"outcome\":\"dispatched\""));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delivery_id_prefers_header() {
        let payload = json!({"data": "test"});
        let id = delivery_id(Some("abc-123"), None, &payload);
        assert_eq!(id.as_deref(), Some("abc-123"));
    }

    #[test]
    fn delivery_id_falls_back_to_hash() {
        let payload = json!({"data": "test"});
        let id = delivery_id(None, None, &payload);
        assert!(id.as_ref().unwrap().starts_with("sha256:"));
    }

    #[test]
    fn replay_detection_round_trip() {
        let dir = std::env::temp_dir().join(format!("tutti-replay-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".tutti").join("state")).unwrap();

        assert!(!is_replay(&dir, "delivery-1").unwrap());
        record_delivery(&dir, "delivery-1").unwrap();
        assert!(is_replay(&dir, "delivery-1").unwrap());
        assert!(!is_replay(&dir, "delivery-2").unwrap());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sha256_known_vector() {
        // SHA-256 of empty string
        let hash = sha256_impl(b"");
        let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
