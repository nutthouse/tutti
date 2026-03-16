use crate::error::{Result, TuttiError};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::cmp::Ordering;
use std::fs;
use std::path::Path;
use std::process::Command;

const CLAIM_LABEL: &str = "automation-claimed";
const COMMENT_MARKER_PREFIX: &str = "<!-- tutti-issue-claim ";
const COMMENT_MARKER_SUFFIX: &str = "-->";
const MAX_EVENT_HISTORY: usize = 20;

pub fn acquire(
    output: &Path,
    label: &str,
    repo: Option<&str>,
    run_id: Option<&str>,
    lease_ttl_secs: u64,
) -> Result<()> {
    ensure_gh()?;
    let repo = resolve_repo(repo)?;
    let run_id = run_id
        .map(ToOwned::to_owned)
        .or_else(|| std::env::var("GITHUB_RUN_ID").ok())
        .unwrap_or_else(default_run_id);
    let lease_ttl_secs = normalize_lease_ttl_secs(lease_ttl_secs);
    let now = Utc::now();

    let mut issues = list_open_issues(&repo, label)?;
    issues.sort_by(|left, right| left.created_at.cmp(&right.created_at));

    for issue in issues {
        let mut details = get_issue_details(&repo, issue.number)?;
        release_stale_claims(&repo, &mut details, now)?;

        if winner_active_claim(&details.comments, now).is_some() {
            continue;
        }

        ensure_claim_label(&repo, issue.number)?;
        let claim = ClaimRecord::new_active(run_id.clone(), now, lease_ttl_secs, "acquired");
        let comment = create_claim_comment(&repo, issue.number, &claim)?;

        details = get_issue_details(&repo, issue.number)?;
        release_stale_claims(&repo, &mut details, now)?;

        match winner_active_claim(&details.comments, now) {
            Some(winner) if winner.id == comment.id => {
                let payload = SelectedIssueOutput {
                    repo: repo.clone(),
                    issue_number: details.number,
                    title: details.title.clone(),
                    url: details.html_url.clone(),
                    labels: details.label_names(),
                    author: details.author_login(),
                    created_at: details.created_at,
                    body: details.body.clone().unwrap_or_default().trim().to_string(),
                    run_id: claim.run_id.clone(),
                    claimed_at: claim.claimed_at,
                    lease_ttl_secs: claim.lease_ttl_secs,
                    last_heartbeat_at: claim.last_heartbeat_at,
                    claim_comment_id: comment.id,
                    claim_status: claim.status,
                };
                write_selected_issue(output, &payload)?;
                println!(
                    "Acquired issue #{} with lease {}s for run {}.",
                    details.number, claim.lease_ttl_secs, claim.run_id
                );
                return Ok(());
            }
            Some(winner) => {
                release_claim_by_id(
                    &repo,
                    issue.number,
                    comment.id,
                    &format!("superseded by claim comment {}", winner.id),
                    now,
                )?;
            }
            None => {
                release_claim_by_id(
                    &repo,
                    issue.number,
                    comment.id,
                    "claim verification lost",
                    now,
                )?;
            }
        }
    }

    Err(TuttiError::ConfigValidation(format!(
        "No unclaimed open issues found for label '{label}'"
    )))
}

pub fn heartbeat(state: &Path, repo: Option<&str>, allow_missing_state: bool) -> Result<()> {
    ensure_gh()?;
    let Some(mut selected) = load_selected_issue(state, allow_missing_state)? else {
        return Ok(());
    };
    let repo = repo
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| selected.repo.clone());
    let now = Utc::now();
    let mut comment = get_claim_comment(&repo, selected.claim_comment_id)?;
    let mut claim = parse_claim_comment(&comment.body).ok_or_else(|| {
        TuttiError::State(format!(
            "claim comment {} is missing claim metadata",
            selected.claim_comment_id
        ))
    })?;

    if claim.run_id != selected.run_id {
        return Err(TuttiError::State(format!(
            "claim state run_id '{}' does not match comment run_id '{}'",
            selected.run_id, claim.run_id
        )));
    }

    if claim.status != ClaimStatus::Active {
        return Err(TuttiError::State(format!(
            "cannot renew released claim comment {}",
            selected.claim_comment_id
        )));
    }

    claim.last_heartbeat_at = now;
    claim.push_event(now, ClaimEventKind::Renewed, None);
    update_claim_comment(&repo, comment.id, &claim)?;

    comment.updated_at = Some(now);
    selected.last_heartbeat_at = claim.last_heartbeat_at;
    write_selected_issue(state, &selected)?;
    println!(
        "Renewed issue #{} claim for run {} at {}.",
        selected.issue_number,
        selected.run_id,
        claim.last_heartbeat_at.to_rfc3339()
    );
    Ok(())
}

pub fn release(
    state: &Path,
    reason: &str,
    repo: Option<&str>,
    allow_missing_state: bool,
) -> Result<()> {
    ensure_gh()?;
    let Some(selected) = load_selected_issue(state, allow_missing_state)? else {
        return Ok(());
    };
    let repo = repo
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| selected.repo.clone());
    let now = Utc::now();
    release_claim_by_id(
        &repo,
        selected.issue_number,
        selected.claim_comment_id,
        reason,
        now,
    )?;
    println!(
        "Released issue #{} claim for run {} ({reason}).",
        selected.issue_number, selected.run_id
    );
    Ok(())
}

pub fn sweep(repo: Option<&str>, label: Option<&str>) -> Result<()> {
    ensure_gh()?;
    let repo = resolve_repo(repo)?;
    let label = label.unwrap_or(CLAIM_LABEL);
    let now = Utc::now();
    let mut released = 0usize;

    let mut issues = list_open_issues(&repo, label)?;
    issues.sort_by(|left, right| left.created_at.cmp(&right.created_at));
    for issue in issues {
        let mut details = get_issue_details(&repo, issue.number)?;
        released += release_stale_claims(&repo, &mut details, now)?;
    }

    println!("Swept {released} stale claim(s) in {repo}.");
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SelectedIssueOutput {
    repo: String,
    issue_number: u64,
    title: String,
    url: String,
    labels: Vec<String>,
    author: Option<String>,
    created_at: DateTime<Utc>,
    body: String,
    run_id: String,
    claimed_at: DateTime<Utc>,
    lease_ttl_secs: u64,
    last_heartbeat_at: DateTime<Utc>,
    claim_comment_id: u64,
    claim_status: ClaimStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ClaimStatus {
    Active,
    Released,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ClaimEventKind {
    Acquired,
    Renewed,
    Released,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClaimEvent {
    at: DateTime<Utc>,
    kind: ClaimEventKind,
    note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClaimRecord {
    schema_version: u32,
    run_id: String,
    claimed_at: DateTime<Utc>,
    lease_ttl_secs: u64,
    last_heartbeat_at: DateTime<Utc>,
    status: ClaimStatus,
    released_at: Option<DateTime<Utc>>,
    release_reason: Option<String>,
    events: Vec<ClaimEvent>,
}

impl ClaimRecord {
    fn new_active(run_id: String, now: DateTime<Utc>, lease_ttl_secs: u64, note: &str) -> Self {
        let mut claim = Self {
            schema_version: 1,
            run_id,
            claimed_at: now,
            lease_ttl_secs,
            last_heartbeat_at: now,
            status: ClaimStatus::Active,
            released_at: None,
            release_reason: None,
            events: Vec::new(),
        };
        claim.push_event(now, ClaimEventKind::Acquired, Some(note.to_string()));
        claim
    }

    fn push_event(&mut self, at: DateTime<Utc>, kind: ClaimEventKind, note: Option<String>) {
        self.events.push(ClaimEvent { at, kind, note });
        if self.events.len() > MAX_EVENT_HISTORY {
            let drain = self.events.len() - MAX_EVENT_HISTORY;
            self.events.drain(0..drain);
        }
    }

    fn expires_at(&self) -> DateTime<Utc> {
        self.last_heartbeat_at + Duration::seconds(self.lease_ttl_secs as i64)
    }

    fn is_active(&self, now: DateTime<Utc>) -> bool {
        self.status == ClaimStatus::Active && self.expires_at() > now
    }

    fn release(&mut self, now: DateTime<Utc>, reason: &str) {
        self.status = ClaimStatus::Released;
        self.released_at = Some(now);
        self.release_reason = Some(reason.to_string());
        self.push_event(now, ClaimEventKind::Released, Some(reason.to_string()));
    }
}

#[derive(Debug, Clone, Deserialize)]
struct IssueSummary {
    number: u64,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
struct IssueDetails {
    number: u64,
    title: String,
    html_url: String,
    body: Option<String>,
    created_at: DateTime<Utc>,
    user: Option<UserRef>,
    labels: Vec<LabelRef>,
    #[serde(default)]
    comments: Vec<IssueComment>,
}

impl IssueDetails {
    fn author_login(&self) -> Option<String> {
        self.user.as_ref().map(|user| user.login.clone())
    }

    fn label_names(&self) -> Vec<String> {
        self.labels.iter().map(|label| label.name.clone()).collect()
    }
}

#[derive(Debug, Clone, Deserialize)]
struct UserRef {
    login: String,
}

#[derive(Debug, Clone, Deserialize)]
struct LabelRef {
    name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct IssueComment {
    id: u64,
    body: String,
    updated_at: Option<DateTime<Utc>>,
}

fn ensure_gh() -> Result<()> {
    if which::which("gh").is_ok() {
        Ok(())
    } else {
        Err(TuttiError::ConfigValidation(
            "`gh` is required for issue-claim automation".to_string(),
        ))
    }
}

fn resolve_repo(repo: Option<&str>) -> Result<String> {
    if let Some(repo) = repo {
        return Ok(repo.to_string());
    }
    if let Ok(repo) = std::env::var("GITHUB_REPOSITORY")
        && !repo.trim().is_empty()
    {
        return Ok(repo);
    }

    let output = Command::new("gh")
        .args([
            "repo",
            "view",
            "--json",
            "nameWithOwner",
            "-q",
            ".nameWithOwner",
        ])
        .output()?;
    if !output.status.success() {
        return Err(TuttiError::ConfigValidation(format!(
            "failed to determine GitHub repo: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn list_open_issues(repo: &str, label: &str) -> Result<Vec<IssueSummary>> {
    let path = format!("repos/{repo}/issues");
    let json = gh_api_json(
        &[
            "api",
            "--method",
            "GET",
            path.as_str(),
            "-f",
            "state=open",
            "-f",
            &format!("labels={label}"),
            "-f",
            "per_page=100",
        ],
        None,
    )?;
    let items = json.as_array().ok_or_else(|| {
        TuttiError::State("GitHub issue list response was not an array".to_string())
    })?;
    let mut out = Vec::new();
    for item in items {
        if item.get("pull_request").is_some() {
            continue;
        }
        out.push(serde_json::from_value::<IssueSummary>(item.clone())?);
    }
    Ok(out)
}

fn get_issue_details(repo: &str, issue_number: u64) -> Result<IssueDetails> {
    let issue_path = format!("repos/{repo}/issues/{issue_number}");
    let comments_path = format!("repos/{repo}/issues/{issue_number}/comments");
    let mut issue = serde_json::from_value::<IssueDetails>(gh_api_json(
        &["api", "--method", "GET", issue_path.as_str()],
        None,
    )?)?;
    issue.comments = serde_json::from_value::<Vec<IssueComment>>(gh_api_json(
        &[
            "api",
            "--method",
            "GET",
            comments_path.as_str(),
            "-f",
            "per_page=100",
        ],
        None,
    )?)?;
    Ok(issue)
}

fn get_claim_comment(repo: &str, comment_id: u64) -> Result<IssueComment> {
    let path = format!("repos/{repo}/issues/comments/{comment_id}");
    Ok(serde_json::from_value::<IssueComment>(gh_api_json(
        &["api", "--method", "GET", path.as_str()],
        None,
    )?)?)
}

fn ensure_claim_label(repo: &str, issue_number: u64) -> Result<()> {
    let path = format!("repos/{repo}/issues/{issue_number}/labels");
    gh_api_no_output(
        &[
            "api",
            "--method",
            "POST",
            path.as_str(),
            "-f",
            &format!("labels[]={CLAIM_LABEL}"),
        ],
        None,
    )
}

fn create_claim_comment(
    repo: &str,
    issue_number: u64,
    claim: &ClaimRecord,
) -> Result<IssueComment> {
    let path = format!("repos/{repo}/issues/{issue_number}/comments");
    Ok(serde_json::from_value::<IssueComment>(gh_api_json(
        &[
            "api",
            "--method",
            "POST",
            path.as_str(),
            "-f",
            &format!("body={}", render_claim_comment_body(claim)?),
        ],
        None,
    )?)?)
}

fn update_claim_comment(repo: &str, comment_id: u64, claim: &ClaimRecord) -> Result<()> {
    let path = format!("repos/{repo}/issues/comments/{comment_id}");
    gh_api_no_output(
        &[
            "api",
            "--method",
            "PATCH",
            path.as_str(),
            "-f",
            &format!("body={}", render_claim_comment_body(claim)?),
        ],
        None,
    )
}

fn remove_claim_label(repo: &str, issue_number: u64) -> Result<()> {
    let path = format!("repos/{repo}/issues/{issue_number}/labels/{CLAIM_LABEL}");
    let output = Command::new("gh")
        .args(["api", "--method", "DELETE", path.as_str()])
        .output()?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("404") {
        return Ok(());
    }
    Err(TuttiError::State(format!(
        "gh api DELETE {path} failed: {}",
        stderr.trim()
    )))
}

fn release_stale_claims(repo: &str, issue: &mut IssueDetails, now: DateTime<Utc>) -> Result<usize> {
    let mut released = 0usize;
    for comment in issue.comments.clone() {
        let Some(mut claim) = parse_claim_comment(&comment.body) else {
            continue;
        };
        if claim.status != ClaimStatus::Active || claim.is_active(now) {
            continue;
        }
        let reason = format!("lease expired at {}", claim.expires_at().to_rfc3339());
        claim.release(now, &reason);
        update_claim_comment(repo, comment.id, &claim)?;
        released += 1;
        println!(
            "Released stale issue #{} claim for run {}.",
            issue.number, claim.run_id
        );
    }

    let refreshed = get_issue_details(repo, issue.number)?;
    *issue = refreshed;
    if winner_active_claim(&issue.comments, now).is_none() {
        remove_claim_label(repo, issue.number)?;
    }
    Ok(released)
}

fn release_claim_by_id(
    repo: &str,
    issue_number: u64,
    comment_id: u64,
    reason: &str,
    now: DateTime<Utc>,
) -> Result<()> {
    let mut comment = get_claim_comment(repo, comment_id)?;
    let mut claim = parse_claim_comment(&comment.body).ok_or_else(|| {
        TuttiError::State(format!(
            "claim comment {comment_id} is missing claim metadata"
        ))
    })?;
    if claim.status == ClaimStatus::Released {
        return Ok(());
    }
    claim.last_heartbeat_at = now;
    claim.release(now, reason);
    update_claim_comment(repo, comment_id, &claim)?;
    comment.updated_at = Some(now);

    let issue = get_issue_details(repo, issue_number)?;
    if winner_active_claim(&issue.comments, now).is_none() {
        remove_claim_label(repo, issue_number)?;
    }
    Ok(())
}

fn write_selected_issue(path: &Path, payload: &SelectedIssueOutput) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(payload)?)?;
    fs::rename(tmp, path)?;
    Ok(())
}

fn load_selected_issue(
    path: &Path,
    allow_missing_state: bool,
) -> Result<Option<SelectedIssueOutput>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(err) if allow_missing_state && err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn render_claim_comment_body(claim: &ClaimRecord) -> Result<String> {
    let mut body = String::new();
    body.push_str("Tutti automation issue claim.\n\n");
    body.push_str(&format!("- Run ID: `{}`\n", claim.run_id));
    body.push_str(&format!(
        "- Status: `{}`\n",
        claim_status_label(&claim.status)
    ));
    body.push_str(&format!(
        "- Claimed At: `{}`\n",
        claim.claimed_at.to_rfc3339()
    ));
    body.push_str(&format!(
        "- Last Heartbeat: `{}`\n",
        claim.last_heartbeat_at.to_rfc3339()
    ));
    body.push_str(&format!("- Lease TTL: `{}s`\n", claim.lease_ttl_secs));
    if let Some(released_at) = claim.released_at {
        body.push_str(&format!("- Released At: `{}`\n", released_at.to_rfc3339()));
    }
    if let Some(reason) = &claim.release_reason {
        body.push_str(&format!("- Release Reason: `{reason}`\n"));
    }
    body.push_str("\nRecent events:\n");
    for event in &claim.events {
        let action = match event.kind {
            ClaimEventKind::Acquired => "acquired",
            ClaimEventKind::Renewed => "renewed",
            ClaimEventKind::Released => "released",
        };
        body.push_str(&format!("- {} {action}", event.at.to_rfc3339()));
        if let Some(note) = &event.note {
            body.push_str(&format!(" ({note})"));
        }
        body.push('\n');
    }
    body.push('\n');
    body.push_str(COMMENT_MARKER_PREFIX);
    body.push_str(&serde_json::to_string(claim)?);
    body.push(' ');
    body.push_str(COMMENT_MARKER_SUFFIX);
    Ok(body)
}

fn parse_claim_comment(body: &str) -> Option<ClaimRecord> {
    let start = body.find(COMMENT_MARKER_PREFIX)?;
    let rest = &body[start + COMMENT_MARKER_PREFIX.len()..];
    let end = rest.find(COMMENT_MARKER_SUFFIX)?;
    serde_json::from_str(rest[..end].trim()).ok()
}

fn winner_active_claim(comments: &[IssueComment], now: DateTime<Utc>) -> Option<WinningClaim> {
    comments
        .iter()
        .filter_map(|comment| {
            let claim = parse_claim_comment(&comment.body)?;
            if !claim.is_active(now) {
                return None;
            }
            Some(WinningClaim {
                id: comment.id,
                claim,
            })
        })
        .min_by(compare_winning_claims)
}

fn compare_winning_claims(left: &WinningClaim, right: &WinningClaim) -> Ordering {
    left.claim
        .claimed_at
        .cmp(&right.claim.claimed_at)
        .then_with(|| left.id.cmp(&right.id))
        .then_with(|| left.claim.run_id.cmp(&right.claim.run_id))
}

fn claim_status_label(status: &ClaimStatus) -> &'static str {
    match status {
        ClaimStatus::Active => "active",
        ClaimStatus::Released => "released",
    }
}

fn default_run_id() -> String {
    format!(
        "local-{}-{}",
        Utc::now().format("%Y%m%d%H%M%S"),
        std::process::id()
    )
}

fn normalize_lease_ttl_secs(lease_ttl_secs: u64) -> u64 {
    lease_ttl_secs.max(60)
}

fn gh_api_json(args: &[&str], cwd: Option<&Path>) -> Result<Value> {
    let mut cmd = Command::new("gh");
    cmd.args(args);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    let output = cmd.output()?;
    if output.status.success() {
        Ok(serde_json::from_slice(&output.stdout)?)
    } else {
        Err(TuttiError::State(format!(
            "gh {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

fn gh_api_no_output(args: &[&str], cwd: Option<&Path>) -> Result<()> {
    let mut cmd = Command::new("gh");
    cmd.args(args);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    let output = cmd.output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(TuttiError::State(format!(
            "gh {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

#[derive(Debug)]
struct WinningClaim {
    id: u64,
    claim: ClaimRecord,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_comment_round_trips() {
        let now = DateTime::parse_from_rfc3339("2026-03-16T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut claim = ClaimRecord::new_active("run-123".to_string(), now, 600, "acquired");
        claim.push_event(now + Duration::seconds(120), ClaimEventKind::Renewed, None);
        let body = render_claim_comment_body(&claim).unwrap();
        let parsed = parse_claim_comment(&body).unwrap();
        assert_eq!(parsed.run_id, "run-123");
        assert_eq!(parsed.lease_ttl_secs, 600);
        assert_eq!(parsed.status, ClaimStatus::Active);
        assert_eq!(parsed.events.len(), 2);
    }

    #[test]
    fn active_claim_expires_after_ttl() {
        let now = DateTime::parse_from_rfc3339("2026-03-16T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let claim = ClaimRecord::new_active("run-123".to_string(), now, 300, "acquired");
        assert!(claim.is_active(now + Duration::seconds(299)));
        assert!(!claim.is_active(now + Duration::seconds(300)));
    }

    #[test]
    fn winner_prefers_oldest_claim_then_lowest_comment_id() {
        let now = DateTime::parse_from_rfc3339("2026-03-16T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let older = ClaimRecord::new_active("run-1".to_string(), now, 600, "acquired");
        let newer = ClaimRecord::new_active(
            "run-2".to_string(),
            now + Duration::seconds(5),
            600,
            "acquired",
        );
        let tie_left = ClaimRecord::new_active("run-3".to_string(), now, 600, "acquired");
        let tie_right = ClaimRecord::new_active("run-4".to_string(), now, 600, "acquired");
        let comments = vec![
            IssueComment {
                id: 40,
                body: render_claim_comment_body(&newer).unwrap(),
                updated_at: None,
            },
            IssueComment {
                id: 20,
                body: render_claim_comment_body(&older).unwrap(),
                updated_at: None,
            },
        ];
        let winner = winner_active_claim(&comments, now + Duration::seconds(10)).unwrap();
        assert_eq!(winner.id, 20);

        let tied = vec![
            IssueComment {
                id: 9,
                body: render_claim_comment_body(&tie_right).unwrap(),
                updated_at: None,
            },
            IssueComment {
                id: 4,
                body: render_claim_comment_body(&tie_left).unwrap(),
                updated_at: None,
            },
        ];
        let winner = winner_active_claim(&tied, now + Duration::seconds(1)).unwrap();
        assert_eq!(winner.id, 4);
    }

    #[test]
    fn release_marks_claim_inactive() {
        let now = DateTime::parse_from_rfc3339("2026-03-16T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut claim = ClaimRecord::new_active("run-123".to_string(), now, 600, "acquired");
        claim.release(now + Duration::seconds(60), "failed");
        assert_eq!(claim.status, ClaimStatus::Released);
        assert!(!claim.is_active(now + Duration::seconds(61)));
        assert_eq!(claim.release_reason.as_deref(), Some("failed"));
    }
}
