use crate::error::{Result, TuttiError};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

const CLAIM_MARKER_START: &str = "<!-- tutti-issue-claim ";
const CLAIM_MARKER_END: &str = " -->";
const DEFAULT_LEASE_TTL_SECS: u64 = 1800; // 30 minutes
const MAX_EVENTS: usize = 20;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClaimStatus {
    Active,
    Released,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimEvent {
    pub timestamp: DateTime<Utc>,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimRecord {
    pub schema_version: u32,
    pub run_id: String,
    pub claimed_at: DateTime<Utc>,
    pub lease_ttl_secs: u64,
    pub last_heartbeat_at: DateTime<Utc>,
    pub status: ClaimStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub released_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_reason: Option<String>,
    pub events: Vec<ClaimEvent>,
}

impl ClaimRecord {
    fn new(run_id: &str, lease_ttl_secs: u64) -> Self {
        let now = Utc::now();
        Self {
            schema_version: 1,
            run_id: run_id.to_string(),
            claimed_at: now,
            lease_ttl_secs,
            last_heartbeat_at: now,
            status: ClaimStatus::Active,
            released_at: None,
            release_reason: None,
            events: vec![ClaimEvent {
                timestamp: now,
                kind: "Acquired".to_string(),
                detail: None,
            }],
        }
    }

    fn expires_at(&self) -> DateTime<Utc> {
        {
            let secs = i64::try_from(self.lease_ttl_secs).unwrap_or(i64::MAX);
            self.last_heartbeat_at + chrono::Duration::seconds(secs)
        }
    }

    fn is_expired(&self) -> bool {
        Utc::now() > self.expires_at()
    }

    fn push_event(&mut self, kind: &str, detail: Option<String>) {
        self.events.push(ClaimEvent {
            timestamp: Utc::now(),
            kind: kind.to_string(),
            detail,
        });
        if self.events.len() > MAX_EVENTS {
            self.events.remove(0);
        }
    }
}

/// Output written to selected_issue.json (extends the old format).
#[derive(Debug, Serialize, Deserialize)]
pub struct SelectedIssueOutput {
    pub issue_number: u64,
    pub title: String,
    pub url: String,
    pub labels: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    pub created_at: String,
    pub body: String,
    // claim metadata
    pub run_id: String,
    pub claimed_at: DateTime<Utc>,
    pub lease_ttl_secs: u64,
    pub last_heartbeat_at: DateTime<Utc>,
    pub claim_comment_id: u64,
    pub claim_status: ClaimStatus,
}

// ---------------------------------------------------------------------------
// GitHub helpers
// ---------------------------------------------------------------------------

fn gh_repo() -> Result<String> {
    if let Ok(repo) = std::env::var("GITHUB_REPOSITORY") {
        return Ok(repo);
    }
    let out = Command::new("gh")
        .args([
            "repo",
            "view",
            "--json",
            "nameWithOwner",
            "-q",
            ".nameWithOwner",
        ])
        .output()?;
    if !out.status.success() {
        return Err(TuttiError::IssueClaim(
            "failed to detect repository via gh cli".into(),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn gh_run_id() -> String {
    std::env::var("GITHUB_RUN_ID").unwrap_or_else(|_| format!("local-{}", std::process::id()))
}

/// List open issues for a label, returning raw JSON array.
fn gh_list_issues(repo: &str, label: &str) -> Result<Vec<serde_json::Value>> {
    let out = Command::new("gh")
        .args([
            "issue",
            "list",
            "--repo",
            repo,
            "--state",
            "open",
            "--label",
            label,
            "--limit",
            "100",
            "--json",
            "number,title,url,labels,author,createdAt",
        ])
        .output()?;
    if !out.status.success() {
        return Err(TuttiError::IssueClaim(format!(
            "gh issue list failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    let items: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout)?;
    Ok(items)
}

/// Fetch full issue body.
fn gh_issue_body(repo: &str, number: u64) -> Result<String> {
    let out = Command::new("gh")
        .args([
            "issue",
            "view",
            &number.to_string(),
            "--repo",
            repo,
            "--json",
            "body",
            "-q",
            ".body",
        ])
        .output()?;
    if !out.status.success() {
        return Err(TuttiError::IssueClaim(format!(
            "gh issue view failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Add a label to an issue.
fn gh_add_label(repo: &str, number: u64, label: &str) -> Result<()> {
    let out = Command::new("gh")
        .args([
            "issue",
            "edit",
            &number.to_string(),
            "--repo",
            repo,
            "--add-label",
            label,
        ])
        .output()?;
    if !out.status.success() {
        return Err(TuttiError::IssueClaim(format!(
            "gh add label failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(())
}

/// Remove a label from an issue.
fn gh_remove_label(repo: &str, number: u64, label: &str) -> Result<()> {
    let out = Command::new("gh")
        .args([
            "issue",
            "edit",
            &number.to_string(),
            "--repo",
            repo,
            "--remove-label",
            label,
        ])
        .output()?;
    if !out.status.success() {
        // label may not exist — treat as non-fatal
        eprintln!(
            "warning: could not remove label '{}': {}",
            label,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

/// Create a comment on an issue, return the comment ID.
fn gh_create_comment(repo: &str, number: u64, body: &str) -> Result<u64> {
    let out = Command::new("gh")
        .args([
            "issue",
            "comment",
            &number.to_string(),
            "--repo",
            repo,
            "--body",
            body,
        ])
        .output()?;
    if !out.status.success() {
        return Err(TuttiError::IssueClaim(format!(
            "gh issue comment failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    // gh issue comment outputs the URL of the created comment
    // e.g. https://github.com/owner/repo/issues/29#issuecomment-123456
    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    // Extract comment ID from URL
    if let Some(id_str) = url.rsplit("issuecomment-").next()
        && let Ok(id) = id_str.trim().parse::<u64>()
    {
        return Ok(id);
    }
    // Fallback: fetch comments and find ours
    let comments = gh_list_comments(repo, number)?;
    for c in comments.iter().rev() {
        if let Some(body_text) = c["body"].as_str()
            && body_text.contains(CLAIM_MARKER_START)
            && let Some(id) = c["id"].as_u64()
        {
            return Ok(id);
        }
    }
    Err(TuttiError::IssueClaim(
        "could not determine created comment ID".into(),
    ))
}

/// Update an existing comment.
fn gh_update_comment(repo: &str, comment_id: u64, body: &str) -> Result<()> {
    let out = Command::new("gh")
        .args([
            "api",
            &format!("repos/{repo}/issues/comments/{comment_id}"),
            "-X",
            "PATCH",
            "-f",
            &format!("body={body}"),
        ])
        .output()?;
    if !out.status.success() {
        return Err(TuttiError::IssueClaim(format!(
            "gh api comment update failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(())
}

/// List comments on an issue.
fn gh_list_comments(repo: &str, number: u64) -> Result<Vec<serde_json::Value>> {
    let out = Command::new("gh")
        .args([
            "api",
            &format!("repos/{repo}/issues/{number}/comments"),
            "--paginate",
        ])
        .output()?;
    if !out.status.success() {
        return Err(TuttiError::IssueClaim(format!(
            "gh api comments failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    let comments: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout)?;
    Ok(comments)
}

/// List issues with a specific label (for sweep).
fn gh_issues_with_label(repo: &str, label: &str) -> Result<Vec<u64>> {
    let out = Command::new("gh")
        .args([
            "issue", "list", "--repo", repo, "--state", "open", "--label", label, "--limit", "100",
            "--json", "number",
        ])
        .output()?;
    if !out.status.success() {
        return Err(TuttiError::IssueClaim(format!(
            "gh issue list for sweep failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    let items: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout)?;
    Ok(items.iter().filter_map(|v| v["number"].as_u64()).collect())
}

// ---------------------------------------------------------------------------
// Claim comment encoding
// ---------------------------------------------------------------------------

fn encode_claim_comment(record: &ClaimRecord) -> Result<String> {
    let json = serde_json::to_string(record)?;
    let status_emoji = match record.status {
        ClaimStatus::Active => "🔒",
        ClaimStatus::Released => "🔓",
    };
    Ok(format!(
        "{CLAIM_MARKER_START}{json}{CLAIM_MARKER_END}\n\n\
         {status_emoji} **Claim** — run `{}` | status: `{:?}` | \
         expires: {} | heartbeat: {}",
        record.run_id,
        record.status,
        record.expires_at().format("%Y-%m-%dT%H:%M:%SZ"),
        record.last_heartbeat_at.format("%Y-%m-%dT%H:%M:%SZ"),
    ))
}

fn decode_claim_comment(body: &str) -> Option<ClaimRecord> {
    let start = body.find(CLAIM_MARKER_START)?;
    let json_start = start + CLAIM_MARKER_START.len();
    let end = body[json_start..].find(CLAIM_MARKER_END)?;
    let json_str = &body[json_start..json_start + end];
    serde_json::from_str(json_str).ok()
}

// ---------------------------------------------------------------------------
// Claim operations
// ---------------------------------------------------------------------------

/// Find all claim comments on an issue, returning (comment_id, record) pairs.
fn find_claim_comments(repo: &str, issue_number: u64) -> Result<Vec<(u64, ClaimRecord)>> {
    let comments = gh_list_comments(repo, issue_number)?;
    let mut claims = Vec::new();
    for c in &comments {
        if let Some(body) = c["body"].as_str()
            && let Some(record) = decode_claim_comment(body)
            && let Some(id) = c["id"].as_u64()
        {
            claims.push((id, record));
        }
    }
    Ok(claims)
}

/// Release stale (expired) claims on an issue.
fn release_stale_claims(repo: &str, issue_number: u64) -> Result<u32> {
    let claims = find_claim_comments(repo, issue_number)?;
    let mut released = 0u32;
    for (comment_id, mut record) in claims {
        if record.status == ClaimStatus::Active && record.is_expired() {
            record.status = ClaimStatus::Released;
            record.released_at = Some(Utc::now());
            record.release_reason = Some("lease expired (stale)".into());
            record.push_event("Released", Some("lease expired".into()));
            let body = encode_claim_comment(&record)?;
            gh_update_comment(repo, comment_id, &body)?;
            released += 1;
            eprintln!(
                "released stale claim on #{} (run {}, expired {})",
                issue_number,
                record.run_id,
                record.expires_at().format("%Y-%m-%dT%H:%M:%SZ")
            );
        }
    }
    Ok(released)
}

/// Find the winning active claim (earliest claimed_at, then lowest comment_id).
fn winner_active_claim(claims: &[(u64, ClaimRecord)]) -> Option<(u64, &ClaimRecord)> {
    claims
        .iter()
        .filter(|(_, r)| r.status == ClaimStatus::Active && !r.is_expired())
        .min_by_key(|(cid, r)| (r.claimed_at, *cid))
        .map(|(cid, r)| (*cid, r))
}

// ---------------------------------------------------------------------------
// Public subcommands
// ---------------------------------------------------------------------------

/// `tt issue-claim acquire`
pub fn acquire(output_path: &Path, label: &str, lease_ttl_secs: Option<u64>) -> Result<()> {
    let repo = gh_repo()?;
    let run_id = gh_run_id();
    let ttl = lease_ttl_secs.unwrap_or(DEFAULT_LEASE_TTL_SECS);

    let issues = gh_list_issues(&repo, label)?;

    // Filter out already-claimed issues and sort by creation date
    let mut candidates: Vec<&serde_json::Value> = issues
        .iter()
        .filter(|i| {
            let labels = i["labels"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|l| l["name"].as_str())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            !labels.contains(&"automation-claimed")
        })
        .collect();

    candidates.sort_by_key(|i| i["createdAt"].as_str().unwrap_or("").to_string());

    if candidates.is_empty() {
        // Also try issues with automation-claimed but whose claims are all stale
        let claimed_issues: Vec<&serde_json::Value> = issues
            .iter()
            .filter(|i| {
                let labels = i["labels"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|l| l["name"].as_str())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                labels.contains(&"automation-claimed")
            })
            .collect();

        for issue in &claimed_issues {
            let number = issue["number"].as_u64().unwrap_or(0);
            if number == 0 {
                continue;
            }
            release_stale_claims(&repo, number)?;
            let claims = find_claim_comments(&repo, number)?;
            if winner_active_claim(&claims).is_none() {
                // All claims released — remove label and treat as candidate
                gh_remove_label(&repo, number, "automation-claimed")?;
                candidates.push(issue);
            }
        }
        candidates.sort_by_key(|i| i["createdAt"].as_str().unwrap_or("").to_string());
    }

    if candidates.is_empty() {
        return Err(TuttiError::IssueClaim(format!(
            "no unclaimed open issues found for label '{label}'"
        )));
    }

    // Try to claim the first candidate
    let issue = candidates[0];
    let number = issue["number"]
        .as_u64()
        .ok_or_else(|| TuttiError::IssueClaim("issue missing number".into()))?;

    // Create claim record and post as comment
    let record = ClaimRecord::new(&run_id, ttl);
    let comment_body = encode_claim_comment(&record)?;

    // Add label first to prevent races
    gh_add_label(&repo, number, "automation-claimed")?;

    let comment_id = match gh_create_comment(&repo, number, &comment_body) {
        Ok(id) => id,
        Err(err) => {
            // Best-effort cleanup to avoid stranded labels when comment creation fails.
            let _ = gh_remove_label(&repo, number, "automation-claimed");
            return Err(err);
        }
    };

    // Verify we won the race (check all claims, find winner)
    let claims = find_claim_comments(&repo, number)?;
    if let Some((winner_id, _)) = winner_active_claim(&claims)
        && winner_id != comment_id
    {
        // We lost the race — release our claim
        let mut our_record = record.clone();
        our_record.status = ClaimStatus::Released;
        our_record.released_at = Some(Utc::now());
        our_record.release_reason = Some("lost claim race".into());
        our_record.push_event("Released", Some("lost claim race".into()));
        let body = encode_claim_comment(&our_record)?;
        gh_update_comment(&repo, comment_id, &body)?;

        return Err(TuttiError::IssueClaim(format!(
            "lost claim race for issue #{number}"
        )));
    }

    // Fetch issue body
    let body = gh_issue_body(&repo, number)?;

    // Build output
    let labels: Vec<String> = issue["labels"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|l| l["name"].as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let output = SelectedIssueOutput {
        issue_number: number,
        title: issue["title"].as_str().unwrap_or("").to_string(),
        url: issue["url"].as_str().unwrap_or("").to_string(),
        labels,
        author: issue["author"]["login"].as_str().map(|s| s.to_string()),
        created_at: issue["createdAt"].as_str().unwrap_or("").to_string(),
        body,
        run_id: run_id.clone(),
        claimed_at: record.claimed_at,
        lease_ttl_secs: ttl,
        last_heartbeat_at: record.last_heartbeat_at,
        claim_comment_id: comment_id,
        claim_status: ClaimStatus::Active,
    };

    // Write output
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp_path = output_path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(&output)?;
    std::fs::write(&tmp_path, &json)?;
    std::fs::rename(&tmp_path, output_path)?;

    eprintln!(
        "claimed issue #{} (comment {}, ttl {}s, run {})",
        number, comment_id, ttl, run_id
    );
    println!("{}", output_path.display());
    Ok(())
}

/// `tt issue-claim heartbeat`
pub fn heartbeat(state_path: &Path) -> Result<()> {
    let repo = gh_repo()?;
    let data = std::fs::read_to_string(state_path)
        .map_err(|e| TuttiError::IssueClaim(format!("cannot read state file: {e}")))?;
    let output: SelectedIssueOutput = serde_json::from_str(&data)?;

    let comment_id = output.claim_comment_id;
    let issue_number = output.issue_number;

    // Fetch current claim from comment
    let comments = gh_list_comments(&repo, issue_number)?;
    let mut found = false;
    for c in &comments {
        if c["id"].as_u64() == Some(comment_id) {
            if let Some(body) = c["body"].as_str()
                && let Some(mut record) = decode_claim_comment(body)
            {
                if record.status != ClaimStatus::Active {
                    return Err(TuttiError::IssueClaim(
                        "claim is no longer active — cannot renew".into(),
                    ));
                }
                record.last_heartbeat_at = Utc::now();
                record.push_event("Renewed", None);
                let new_body = encode_claim_comment(&record)?;
                gh_update_comment(&repo, comment_id, &new_body)?;
                found = true;

                // Also update local state
                let mut updated_output: serde_json::Value = serde_json::from_str(&data)?;
                updated_output["last_heartbeat_at"] =
                    serde_json::Value::String(record.last_heartbeat_at.to_rfc3339());
                let json = serde_json::to_string_pretty(&updated_output)?;
                let tmp_path = state_path.with_extension("json.tmp");
                std::fs::write(&tmp_path, &json)?;
                std::fs::rename(&tmp_path, state_path)?;

                eprintln!(
                    "heartbeat renewed for #{} (expires {})",
                    issue_number,
                    record.expires_at().format("%Y-%m-%dT%H:%M:%SZ")
                );
            }
            break;
        }
    }
    if !found {
        return Err(TuttiError::IssueClaim(format!(
            "claim comment {comment_id} not found on issue #{issue_number}"
        )));
    }
    Ok(())
}

/// `tt issue-claim release`
pub fn release(state_path: &Path, reason: Option<&str>) -> Result<()> {
    let repo = gh_repo()?;
    let data = std::fs::read_to_string(state_path)
        .map_err(|e| TuttiError::IssueClaim(format!("cannot read state file: {e}")))?;
    let output: SelectedIssueOutput = serde_json::from_str(&data)?;

    let comment_id = output.claim_comment_id;
    let issue_number = output.issue_number;
    let release_reason = reason.unwrap_or("workflow completed");

    // Fetch and update claim comment
    let comments = gh_list_comments(&repo, issue_number)?;
    let mut released = false;
    for c in &comments {
        if c["id"].as_u64() == Some(comment_id) {
            if let Some(body) = c["body"].as_str()
                && let Some(mut record) = decode_claim_comment(body)
            {
                if record.status == ClaimStatus::Released {
                    eprintln!("claim already released for #{}", issue_number);
                    return Ok(());
                }
                record.status = ClaimStatus::Released;
                record.released_at = Some(Utc::now());
                record.release_reason = Some(release_reason.to_string());
                record.push_event("Released", Some(release_reason.to_string()));
                let new_body = encode_claim_comment(&record)?;
                gh_update_comment(&repo, comment_id, &new_body)?;
                released = true;
            }
            break;
        }
    }

    if !released {
        eprintln!("warning: claim comment {} not found", comment_id);
    }

    // Check if there are remaining active claims; if not, remove the label
    let remaining_claims = find_claim_comments(&repo, issue_number)?;
    if winner_active_claim(&remaining_claims).is_none() {
        gh_remove_label(&repo, issue_number, "automation-claimed")?;
        eprintln!("removed automation-claimed label from #{}", issue_number);
    }

    // Update local state
    let mut updated: serde_json::Value = serde_json::from_str(&data)?;
    updated["claim_status"] = serde_json::Value::String("released".into());
    let json = serde_json::to_string_pretty(&updated)?;
    std::fs::write(state_path, json)?;

    eprintln!(
        "released claim on #{} (reason: {})",
        issue_number, release_reason
    );
    Ok(())
}

/// `tt issue-claim sweep`
pub fn sweep() -> Result<()> {
    let repo = gh_repo()?;
    let issues = gh_issues_with_label(&repo, "automation-claimed")?;

    if issues.is_empty() {
        eprintln!("no issues with automation-claimed label");
        return Ok(());
    }

    let mut total_released = 0u32;
    let mut labels_removed = 0u32;

    for number in issues {
        let released = release_stale_claims(&repo, number)?;
        total_released += released;

        // If no active claims remain, remove the label
        let claims = find_claim_comments(&repo, number)?;
        if winner_active_claim(&claims).is_none() {
            gh_remove_label(&repo, number, "automation-claimed")?;
            labels_removed += 1;
            eprintln!("removed automation-claimed label from #{}", number);
        }
    }

    eprintln!(
        "sweep complete: {} stale claims released, {} labels removed",
        total_released, labels_removed
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_claim_record_new() {
        let record = ClaimRecord::new("run-123", 1800);
        assert_eq!(record.run_id, "run-123");
        assert_eq!(record.lease_ttl_secs, 1800);
        assert_eq!(record.status, ClaimStatus::Active);
        assert!(record.released_at.is_none());
        assert_eq!(record.events.len(), 1);
        assert_eq!(record.events[0].kind, "Acquired");
    }

    #[test]
    fn test_claim_record_expiry() {
        let mut record = ClaimRecord::new("run-456", 0); // 0s TTL = immediately expired
        // Need to set last_heartbeat_at to the past
        record.last_heartbeat_at = Utc::now() - chrono::Duration::seconds(1);
        assert!(record.is_expired());
    }

    #[test]
    fn test_claim_record_not_expired() {
        let record = ClaimRecord::new("run-789", 3600);
        assert!(!record.is_expired());
    }

    #[test]
    fn test_encode_decode_roundtrip() {
        let record = ClaimRecord::new("run-abc", 1800);
        let encoded = encode_claim_comment(&record).expect("encode should succeed");
        let decoded = decode_claim_comment(&encoded).expect("should decode");
        assert_eq!(decoded.run_id, "run-abc");
        assert_eq!(decoded.lease_ttl_secs, 1800);
        assert_eq!(decoded.status, ClaimStatus::Active);
    }

    #[test]
    fn test_decode_no_marker() {
        assert!(decode_claim_comment("just a regular comment").is_none());
    }

    #[test]
    fn test_push_event_caps_at_max() {
        let mut record = ClaimRecord::new("run-xyz", 1800);
        for i in 0..25 {
            record.push_event("Renewed", Some(format!("tick {i}")));
        }
        assert_eq!(record.events.len(), MAX_EVENTS);
    }

    #[test]
    fn test_winner_active_claim_empty() {
        let claims: Vec<(u64, ClaimRecord)> = vec![];
        assert!(winner_active_claim(&claims).is_none());
    }

    #[test]
    fn test_winner_active_claim_picks_earliest() {
        let mut r1 = ClaimRecord::new("run-a", 3600);
        r1.claimed_at = Utc::now() - chrono::Duration::seconds(100);
        r1.last_heartbeat_at = Utc::now();
        let mut r2 = ClaimRecord::new("run-b", 3600);
        r2.claimed_at = Utc::now() - chrono::Duration::seconds(50);
        r2.last_heartbeat_at = Utc::now();

        let claims = vec![(10, r1), (20, r2)];
        let (winner_id, winner) = winner_active_claim(&claims).unwrap();
        assert_eq!(winner_id, 10);
        assert_eq!(winner.run_id, "run-a");
    }

    #[test]
    fn test_winner_skips_released() {
        let mut r1 = ClaimRecord::new("run-released", 3600);
        r1.status = ClaimStatus::Released;
        let r2 = ClaimRecord::new("run-active", 3600);

        let claims = vec![(10, r1), (20, r2)];
        let (winner_id, winner) = winner_active_claim(&claims).unwrap();
        assert_eq!(winner_id, 20);
        assert_eq!(winner.run_id, "run-active");
    }

    #[test]
    fn test_winner_skips_expired() {
        let mut r1 = ClaimRecord::new("run-expired", 0);
        r1.last_heartbeat_at = Utc::now() - chrono::Duration::seconds(10);
        let r2 = ClaimRecord::new("run-fresh", 3600);

        let claims = vec![(10, r1), (20, r2)];
        let (winner_id, winner) = winner_active_claim(&claims).unwrap();
        assert_eq!(winner_id, 20);
        assert_eq!(winner.run_id, "run-fresh");
    }

    #[test]
    fn test_selected_issue_output_serializes() {
        let output = SelectedIssueOutput {
            issue_number: 29,
            title: "test issue".into(),
            url: "https://example.com".into(),
            labels: vec!["bug".into()],
            author: Some("user".into()),
            created_at: "2026-03-15T00:00:00Z".into(),
            body: "issue body".into(),
            run_id: "run-1".into(),
            claimed_at: Utc::now(),
            lease_ttl_secs: 1800,
            last_heartbeat_at: Utc::now(),
            claim_comment_id: 12345,
            claim_status: ClaimStatus::Active,
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("claim_comment_id"));
        assert!(json.contains("run_id"));
    }
}
