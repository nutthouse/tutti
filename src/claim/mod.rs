use crate::error::{Result, TuttiError};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Default lease duration in seconds (30 minutes).
const DEFAULT_LEASE_TTL_SECS: u64 = 1800;
/// Label applied to claimed issues.
const CLAIM_LABEL: &str = "automation-claimed";

/// Metadata stored alongside a claim to enable lease expiry and audit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimLease {
    pub issue_number: u64,
    pub repo: String,
    pub run_id: String,
    pub claimed_at: DateTime<Utc>,
    pub renewed_at: DateTime<Utc>,
    pub lease_ttl_secs: u64,
}

impl ClaimLease {
    /// Returns true if the lease has expired based on `renewed_at + lease_ttl_secs`.
    pub fn is_expired(&self) -> bool {
        let deadline = self.renewed_at + chrono::Duration::seconds(self.lease_ttl_secs as i64);
        Utc::now() > deadline
    }

    /// Seconds remaining on the lease, or 0 if expired.
    pub fn remaining_secs(&self) -> i64 {
        let deadline = self.renewed_at + chrono::Duration::seconds(self.lease_ttl_secs as i64);
        (deadline - Utc::now()).num_seconds().max(0)
    }
}

// ---------------------------------------------------------------------------
// State persistence
// ---------------------------------------------------------------------------

fn claims_dir(project_root: &Path) -> PathBuf {
    project_root.join(".tutti").join("state").join("claims")
}

fn claim_path(project_root: &Path, issue_number: u64) -> PathBuf {
    claims_dir(project_root).join(format!("{}.json", issue_number))
}

/// Persist a claim lease to disk.
pub fn save_claim(project_root: &Path, lease: &ClaimLease) -> Result<()> {
    let dir = claims_dir(project_root);
    std::fs::create_dir_all(&dir)?;
    let path = claim_path(project_root, lease.issue_number);
    let json = serde_json::to_string_pretty(lease)?;
    std::fs::write(path, json)?;
    Ok(())
}

/// Load a claim lease from disk, if one exists.
pub fn load_claim(project_root: &Path, issue_number: u64) -> Result<Option<ClaimLease>> {
    let path = claim_path(project_root, issue_number);
    if !path.exists() {
        return Ok(None);
    }
    let data = std::fs::read_to_string(&path)?;
    let lease: ClaimLease = serde_json::from_str(&data)?;
    Ok(Some(lease))
}

/// Remove a claim lease file from disk.
fn remove_claim_file(project_root: &Path, issue_number: u64) -> Result<()> {
    let path = claim_path(project_root, issue_number);
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// List all persisted claim leases.
pub fn list_claims(project_root: &Path) -> Result<Vec<ClaimLease>> {
    let dir = claims_dir(project_root);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut claims = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            let data = std::fs::read_to_string(&path)?;
            if let Ok(lease) = serde_json::from_str::<ClaimLease>(&data) {
                claims.push(lease);
            }
        }
    }
    Ok(claims)
}

// ---------------------------------------------------------------------------
// GitHub interactions via `gh` CLI
// ---------------------------------------------------------------------------

fn gh_command() -> Command {
    Command::new("gh")
}

/// Add the `automation-claimed` label to an issue.
fn add_claim_label(repo: &str, issue_number: u64) -> Result<()> {
    let output = gh_command()
        .args([
            "issue",
            "edit",
            &issue_number.to_string(),
            "--repo",
            repo,
            "--add-label",
            CLAIM_LABEL,
        ])
        .output()?;
    if !output.status.success() {
        return Err(TuttiError::State(format!(
            "failed to add claim label to issue #{}: {}",
            issue_number,
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
}

/// Remove the `automation-claimed` label from an issue.
fn remove_claim_label(repo: &str, issue_number: u64) -> Result<()> {
    let output = gh_command()
        .args([
            "issue",
            "edit",
            &issue_number.to_string(),
            "--repo",
            repo,
            "--remove-label",
            CLAIM_LABEL,
        ])
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Tolerate the label already being absent.
        if !stderr.contains("not found") && !stderr.contains("does not have") {
            return Err(TuttiError::State(format!(
                "failed to remove claim label from issue #{}: {}",
                issue_number, stderr
            )));
        }
    }
    Ok(())
}

/// Post an audit comment on the issue.
fn post_claim_comment(repo: &str, issue_number: u64, body: &str) -> Result<()> {
    let output = gh_command()
        .args([
            "issue",
            "comment",
            &issue_number.to_string(),
            "--repo",
            repo,
            "--body",
            body,
        ])
        .output()?;
    if !output.status.success() {
        // Non-fatal: log but don't fail the claim operation.
        eprintln!(
            "warning: failed to comment on issue #{}: {}",
            issue_number,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Public claim lifecycle
// ---------------------------------------------------------------------------

/// Acquire a claim on an issue: add the label, persist metadata, post a comment.
pub fn acquire_claim(
    project_root: &Path,
    repo: &str,
    issue_number: u64,
    run_id: &str,
    lease_ttl_secs: Option<u64>,
) -> Result<ClaimLease> {
    let now = Utc::now();
    let ttl = lease_ttl_secs.unwrap_or(DEFAULT_LEASE_TTL_SECS);

    let lease = ClaimLease {
        issue_number,
        repo: repo.to_string(),
        run_id: run_id.to_string(),
        claimed_at: now,
        renewed_at: now,
        lease_ttl_secs: ttl,
    };

    add_claim_label(repo, issue_number)?;
    save_claim(project_root, &lease)?;

    let comment = format!(
        "🤖 **Claim acquired** by run `{}` — lease {}s (expires ~{})",
        run_id,
        ttl,
        now + chrono::Duration::seconds(ttl as i64),
    );
    let _ = post_claim_comment(repo, issue_number, &comment);

    eprintln!(
        "claim: acquired issue #{} (run={}, ttl={}s)",
        issue_number, run_id, ttl
    );
    Ok(lease)
}

/// Renew an existing claim lease, extending the expiry window.
pub fn renew_claim(project_root: &Path, issue_number: u64) -> Result<ClaimLease> {
    let mut lease = load_claim(project_root, issue_number)?
        .ok_or_else(|| TuttiError::State(format!("no claim found for issue #{}", issue_number)))?;

    lease.renewed_at = Utc::now();
    save_claim(project_root, &lease)?;

    eprintln!(
        "claim: renewed issue #{} (remaining={}s)",
        issue_number,
        lease.remaining_secs()
    );
    Ok(lease)
}

/// Release a claim: remove the label, delete state, post a comment.
pub fn release_claim(project_root: &Path, issue_number: u64, reason: &str) -> Result<()> {
    let lease = load_claim(project_root, issue_number)?;
    let repo = match &lease {
        Some(l) => l.repo.clone(),
        None => {
            eprintln!(
                "claim: no local lease for issue #{}, attempting label removal from env",
                issue_number
            );
            resolve_repo()?
        }
    };

    remove_claim_label(&repo, issue_number)?;
    remove_claim_file(project_root, issue_number)?;

    let comment = format!(
        "🤖 **Claim released** — reason: {}{}",
        reason,
        lease
            .as_ref()
            .map(|l| format!(" (run `{}`)", l.run_id))
            .unwrap_or_default(),
    );
    let _ = post_claim_comment(&repo, issue_number, &comment);

    eprintln!("claim: released issue #{} ({})", issue_number, reason);
    Ok(())
}

/// Sweep all persisted claims, releasing any whose lease has expired.
/// Returns the list of issue numbers that were released.
pub fn sweep_stale_claims(project_root: &Path) -> Result<Vec<u64>> {
    let claims = list_claims(project_root)?;
    let mut released = Vec::new();

    for lease in &claims {
        if lease.is_expired() {
            eprintln!(
                "claim: sweeping stale claim on issue #{} (run={}, expired {}s ago)",
                lease.issue_number,
                lease.run_id,
                -lease.remaining_secs(),
            );
            if let Err(e) =
                release_claim(project_root, lease.issue_number, "lease expired (sweeper)")
            {
                eprintln!(
                    "claim: failed to release stale claim on issue #{}: {}",
                    lease.issue_number, e
                );
            } else {
                released.push(lease.issue_number);
            }
        }
    }

    if released.is_empty() {
        eprintln!("claim: sweep complete — no stale claims found");
    } else {
        eprintln!(
            "claim: sweep complete — released {} stale claim(s)",
            released.len()
        );
    }
    Ok(released)
}

/// Try to resolve the current GitHub repo from `GITHUB_REPOSITORY` env or `gh`.
fn resolve_repo() -> Result<String> {
    if let Ok(repo) = std::env::var("GITHUB_REPOSITORY") {
        return Ok(repo);
    }
    let output = gh_command()
        .args([
            "repo",
            "view",
            "--json",
            "nameWithOwner",
            "-q",
            ".nameWithOwner",
        ])
        .output()?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(TuttiError::State(
            "cannot resolve GitHub repo: set GITHUB_REPOSITORY or run from a gh-authenticated checkout"
                .to_string(),
        ))
    }
}

/// Load the selected issue from the standard state file and return its number.
pub fn load_selected_issue_number(project_root: &Path) -> Result<Option<u64>> {
    let path = project_root
        .join(".tutti")
        .join("state")
        .join("auto")
        .join("selected_issue.json");
    if !path.exists() {
        return Ok(None);
    }
    let data = std::fs::read_to_string(&path)?;
    let val: serde_json::Value = serde_json::from_str(&data)?;
    Ok(val.get("issue_number").and_then(|v| v.as_u64()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn lease_expiry_logic() {
        let now = Utc::now();
        let lease = ClaimLease {
            issue_number: 42,
            repo: "owner/repo".to_string(),
            run_id: "run-1".to_string(),
            claimed_at: now - Duration::seconds(3600),
            renewed_at: now - Duration::seconds(3600),
            lease_ttl_secs: 1800,
        };
        assert!(lease.is_expired());
        assert_eq!(lease.remaining_secs(), 0);
    }

    #[test]
    fn lease_not_expired() {
        let now = Utc::now();
        let lease = ClaimLease {
            issue_number: 42,
            repo: "owner/repo".to_string(),
            run_id: "run-1".to_string(),
            claimed_at: now,
            renewed_at: now,
            lease_ttl_secs: 1800,
        };
        assert!(!lease.is_expired());
        assert!(lease.remaining_secs() > 1790);
    }

    #[test]
    fn save_and_load_claim() {
        let tmp = std::env::temp_dir().join(format!("tutti-claim-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        let lease = ClaimLease {
            issue_number: 7,
            repo: "owner/repo".to_string(),
            run_id: "run-abc".to_string(),
            claimed_at: Utc::now(),
            renewed_at: Utc::now(),
            lease_ttl_secs: 600,
        };

        save_claim(&tmp, &lease).unwrap();
        let loaded = load_claim(&tmp, 7).unwrap().expect("should exist");
        assert_eq!(loaded.issue_number, 7);
        assert_eq!(loaded.run_id, "run-abc");
        assert_eq!(loaded.lease_ttl_secs, 600);

        remove_claim_file(&tmp, 7).unwrap();
        assert!(load_claim(&tmp, 7).unwrap().is_none());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn list_claims_empty_dir() {
        let tmp = std::env::temp_dir().join(format!("tutti-claim-list-{}", std::process::id()));
        // Dir doesn't exist yet
        let claims = list_claims(&tmp).unwrap();
        assert!(claims.is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
