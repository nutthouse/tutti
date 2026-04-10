//! SQLite-backed event log for API-direct agent runs.
//!
//! Every model call, tool invocation, policy decision, and error is
//! recorded to an append-only SQLite database (default location:
//! `.tutti/events.db`). `tt replay` reads the log to reconstruct the
//! decision trail.
//!
//! The log supports two modes:
//!
//! - **Strict**: emit failures (disk full, permissions) propagate up
//!   and abort the run. This is the default.
//! - **BestEffort**: emit failures are logged to stderr and the run
//!   continues. This is the recommended mode for production runs
//!   where losing audit data is preferable to aborting mid-task.
//!
//! The mode is a per-EventLog setting.

use crate::error::{Result, TuttiError};
use crate::provider::StopReason;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Resilience mode for the event log.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LogMode {
    /// Emit failures abort the run (default).
    #[default]
    Strict,
    /// Emit failures are logged to stderr; the run continues.
    BestEffort,
}

/// Canonical event types recorded to the log.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event_type", rename_all = "snake_case")]
pub enum Event {
    RunStarted {
        provider: String,
        model: String,
    },
    ModelCalled {
        iteration: u32,
        message_count: usize,
    },
    ModelResponded {
        iteration: u32,
        input_tokens: u64,
        output_tokens: u64,
        cost_usd: f64,
        cumulative_cost_usd: f64,
        stop_reason: StopReason,
    },
    ModelError {
        error: String,
    },
    ToolApproved {
        tool: String,
    },
    ToolDenied {
        tool: String,
        reason: String,
    },
    ToolExecuted {
        tool: String,
        output_len: usize,
        files_modified: Vec<String>,
        is_error: bool,
    },
    ToolFailed {
        tool: String,
        error: String,
    },
    RunFinished {
        iterations: u32,
        cumulative_cost_usd: f64,
    },
    RunFailed {
        reason: String,
    },
}

impl Event {
    pub fn event_type(&self) -> &'static str {
        match self {
            Event::RunStarted { .. } => "run_started",
            Event::ModelCalled { .. } => "model_called",
            Event::ModelResponded { .. } => "model_responded",
            Event::ModelError { .. } => "model_error",
            Event::ToolApproved { .. } => "tool_approved",
            Event::ToolDenied { .. } => "tool_denied",
            Event::ToolExecuted { .. } => "tool_executed",
            Event::ToolFailed { .. } => "tool_failed",
            Event::RunFinished { .. } => "run_finished",
            Event::RunFailed { .. } => "run_failed",
        }
    }
}

/// A single row read back from the events table.
#[derive(Debug, Clone)]
pub struct EventRow {
    pub id: i64,
    pub run_id: String,
    pub event_type: String,
    pub timestamp: DateTime<Utc>,
    pub payload: serde_json::Value,
}

/// A single row read back from the runs table.
#[derive(Debug, Clone)]
pub struct RunRow {
    pub id: String,
    pub arrangement: String,
    pub provider: String,
    pub model: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub status: String,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cost_usd: f64,
}

/// Handle to the SQLite event log.
pub struct EventLog {
    conn: Mutex<Connection>,
    mode: LogMode,
    current_run: Mutex<Option<String>>,
    path: PathBuf,
}

impl EventLog {
    /// Open (or create) an event log at the given path.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self> {
        Self::with_mode(path, LogMode::Strict)
    }

    /// Open an event log with a custom resilience mode.
    pub fn with_mode(path: impl Into<PathBuf>, mode: LogMode) -> Result<Self> {
        let path: PathBuf = path.into();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
            && !parent.exists()
        {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&path)?;
        // WAL improves concurrent-read behavior and crash resilience.
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        init_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            mode,
            current_run: Mutex::new(None),
            path,
        })
    }

    /// Path of the underlying SQLite database.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Begin a new run. Records it in the `runs` table and sets the
    /// current run id — subsequent `emit` calls attach to this run.
    pub fn start_run(&self, arrangement: &str, provider: &str, model: &str) -> Result<String> {
        let run_id = generate_run_id();
        let now = Utc::now();
        let conn = self
            .conn
            .lock()
            .map_err(|_| TuttiError::EventLog("mutex poisoned".into()))?;
        conn.execute(
            "INSERT INTO runs (id, arrangement, provider, model, started_at, status, total_input_tokens, total_output_tokens, total_cost_usd, metadata) VALUES (?1, ?2, ?3, ?4, ?5, 'running', 0, 0, 0.0, NULL)",
            params![run_id, arrangement, provider, model, now.to_rfc3339()],
        )?;
        drop(conn);
        *self
            .current_run
            .lock()
            .map_err(|_| TuttiError::EventLog("mutex poisoned".into()))? = Some(run_id.clone());
        Ok(run_id)
    }

    /// Mark the current run as completed.
    pub fn finish_run(
        &self,
        status: &str,
        total_input_tokens: u64,
        total_output_tokens: u64,
        total_cost_usd: f64,
    ) -> Result<()> {
        let run_id = self
            .current_run
            .lock()
            .map_err(|_| TuttiError::EventLog("mutex poisoned".into()))?
            .clone();
        let Some(run_id) = run_id else {
            return Ok(());
        };
        let now = Utc::now();
        let conn = self
            .conn
            .lock()
            .map_err(|_| TuttiError::EventLog("mutex poisoned".into()))?;
        conn.execute(
            "UPDATE runs SET completed_at = ?1, status = ?2, total_input_tokens = ?3, total_output_tokens = ?4, total_cost_usd = ?5 WHERE id = ?6",
            params![
                now.to_rfc3339(),
                status,
                total_input_tokens as i64,
                total_output_tokens as i64,
                total_cost_usd,
                run_id
            ],
        )?;
        Ok(())
    }

    /// Emit an event attached to the current run. Honors LogMode:
    /// Strict propagates errors, BestEffort writes to stderr and
    /// returns Ok.
    pub fn emit(&self, event: Event) -> Result<()> {
        match self.try_emit(event) {
            Ok(()) => Ok(()),
            Err(e) => match self.mode {
                LogMode::Strict => Err(e),
                LogMode::BestEffort => {
                    eprintln!("tutti: event log write failed (best-effort mode): {e}");
                    Ok(())
                }
            },
        }
    }

    fn try_emit(&self, event: Event) -> Result<()> {
        let run_id = self
            .current_run
            .lock()
            .map_err(|_| TuttiError::EventLog("mutex poisoned".into()))?
            .clone()
            .ok_or_else(|| {
                TuttiError::EventLog(
                    "emit called without an active run — call start_run first".into(),
                )
            })?;
        let event_type = event.event_type().to_string();
        let payload = serde_json::to_string(&event)?;
        let now = Utc::now();
        let conn = self
            .conn
            .lock()
            .map_err(|_| TuttiError::EventLog("mutex poisoned".into()))?;
        conn.execute(
            "INSERT INTO events (run_id, event_type, timestamp, payload) VALUES (?1, ?2, ?3, ?4)",
            params![run_id, event_type, now.to_rfc3339(), payload],
        )?;
        Ok(())
    }

    /// Count the total number of events logged for the current run.
    pub fn event_count(&self) -> Result<u64> {
        let run_id = match self
            .current_run
            .lock()
            .map_err(|_| TuttiError::EventLog("mutex poisoned".into()))?
            .clone()
        {
            Some(id) => id,
            None => return Ok(0),
        };
        let conn = self
            .conn
            .lock()
            .map_err(|_| TuttiError::EventLog("mutex poisoned".into()))?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM events WHERE run_id = ?1",
            params![run_id],
            |row| row.get(0),
        )?;
        Ok(count as u64)
    }

    /// Load all events for a specific run, in insertion order.
    pub fn events_for_run(&self, run_id: &str) -> Result<Vec<EventRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| TuttiError::EventLog("mutex poisoned".into()))?;
        let mut stmt = conn.prepare(
            "SELECT id, run_id, event_type, timestamp, payload FROM events WHERE run_id = ?1 ORDER BY id ASC",
        )?;
        let rows = stmt
            .query_map(params![run_id], |row| {
                let timestamp: String = row.get(3)?;
                let payload: String = row.get(4)?;
                Ok(EventRow {
                    id: row.get(0)?,
                    run_id: row.get(1)?,
                    event_type: row.get(2)?,
                    timestamp: DateTime::parse_from_rfc3339(&timestamp)
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                    payload: serde_json::from_str(&payload).unwrap_or(serde_json::Value::Null),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// List runs in reverse chronological order, newest first.
    pub fn list_runs(&self, limit: usize) -> Result<Vec<RunRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| TuttiError::EventLog("mutex poisoned".into()))?;
        let mut stmt = conn.prepare(
            "SELECT id, arrangement, provider, model, started_at, completed_at, status, total_input_tokens, total_output_tokens, total_cost_usd FROM runs ORDER BY started_at DESC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![limit as i64], |row| {
                let started: String = row.get(4)?;
                let completed: Option<String> = row.get(5)?;
                Ok(RunRow {
                    id: row.get(0)?,
                    arrangement: row.get(1)?,
                    provider: row.get(2)?,
                    model: row.get(3)?,
                    started_at: DateTime::parse_from_rfc3339(&started)
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                    completed_at: completed.and_then(|s| {
                        DateTime::parse_from_rfc3339(&s)
                            .ok()
                            .map(|dt| dt.with_timezone(&Utc))
                    }),
                    status: row.get(6)?,
                    total_input_tokens: row.get(7)?,
                    total_output_tokens: row.get(8)?,
                    total_cost_usd: row.get(9)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS runs (
            id TEXT PRIMARY KEY,
            arrangement TEXT NOT NULL,
            provider TEXT NOT NULL,
            model TEXT NOT NULL,
            started_at TEXT NOT NULL,
            completed_at TEXT,
            status TEXT NOT NULL,
            total_input_tokens INTEGER NOT NULL DEFAULT 0,
            total_output_tokens INTEGER NOT NULL DEFAULT 0,
            total_cost_usd REAL NOT NULL DEFAULT 0.0,
            metadata TEXT
        );

        CREATE TABLE IF NOT EXISTS events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            run_id TEXT NOT NULL REFERENCES runs(id),
            event_type TEXT NOT NULL,
            timestamp TEXT NOT NULL,
            payload TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_events_run_id ON events(run_id);
        CREATE INDEX IF NOT EXISTS idx_runs_started_at ON runs(started_at);
        ",
    )?;
    Ok(())
}

fn generate_run_id() -> String {
    let now = Utc::now();
    let ts = now.format("%Y%m%d-%H%M%S").to_string();
    // Add a random suffix so concurrent starts don't collide.
    let mut suffix_bytes = [0u8; 4];
    let _ = getrandom::fill(&mut suffix_bytes);
    let suffix: String = suffix_bytes.iter().map(|b| format!("{:02x}", b)).collect();
    format!("run_{ts}_{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::StopReason;

    fn make_log() -> (tempfile::TempDir, EventLog) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("events.db");
        let log = EventLog::new(path).unwrap();
        (tmp, log)
    }

    #[test]
    fn start_run_and_emit_events() {
        let (_dir, log) = make_log();
        let run_id = log.start_run("test", "mock", "test-model").unwrap();
        assert!(run_id.starts_with("run_"));
        log.emit(Event::ModelCalled {
            iteration: 1,
            message_count: 2,
        })
        .unwrap();
        log.emit(Event::ModelResponded {
            iteration: 1,
            input_tokens: 100,
            output_tokens: 50,
            cost_usd: 0.01,
            cumulative_cost_usd: 0.01,
            stop_reason: StopReason::EndTurn,
        })
        .unwrap();
        assert_eq!(log.event_count().unwrap(), 2);
    }

    #[test]
    fn emit_without_run_fails_in_strict_mode() {
        let (_dir, log) = make_log();
        let result = log.emit(Event::ModelCalled {
            iteration: 1,
            message_count: 1,
        });
        assert!(result.is_err());
    }

    #[test]
    fn best_effort_mode_swallows_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("events.db");
        let log = EventLog::with_mode(path, LogMode::BestEffort).unwrap();
        // No start_run — emit would normally fail.
        let result = log.emit(Event::ModelCalled {
            iteration: 1,
            message_count: 1,
        });
        assert!(result.is_ok(), "best-effort should swallow errors");
    }

    #[test]
    fn events_for_run_returns_in_order() {
        let (_dir, log) = make_log();
        let run_id = log.start_run("t", "mock", "m").unwrap();
        log.emit(Event::ModelCalled {
            iteration: 1,
            message_count: 1,
        })
        .unwrap();
        log.emit(Event::ToolApproved {
            tool: "read_file".into(),
        })
        .unwrap();
        log.emit(Event::RunFinished {
            iterations: 1,
            cumulative_cost_usd: 0.0,
        })
        .unwrap();
        let events = log.events_for_run(&run_id).unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].event_type, "model_called");
        assert_eq!(events[1].event_type, "tool_approved");
        assert_eq!(events[2].event_type, "run_finished");
    }

    #[test]
    fn finish_run_updates_totals() {
        let (_dir, log) = make_log();
        let run_id = log.start_run("t", "mock", "m").unwrap();
        log.finish_run("completed", 500, 250, 0.03).unwrap();
        let runs = log.list_runs(10).unwrap();
        let row = runs.iter().find(|r| r.id == run_id).unwrap();
        assert_eq!(row.status, "completed");
        assert_eq!(row.total_input_tokens, 500);
        assert_eq!(row.total_output_tokens, 250);
        assert!((row.total_cost_usd - 0.03).abs() < 1e-9);
        assert!(row.completed_at.is_some());
    }

    #[test]
    fn list_runs_is_reverse_chronological() {
        let (_dir, log) = make_log();
        let _r1 = log.start_run("first", "m", "mod").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let _r2 = log.start_run("second", "m", "mod").unwrap();
        let runs = log.list_runs(10).unwrap();
        assert!(runs.len() >= 2);
        assert_eq!(runs[0].arrangement, "second");
        assert_eq!(runs[1].arrangement, "first");
    }

    #[test]
    fn run_id_is_unique() {
        let ids: Vec<_> = (0..5).map(|_| generate_run_id()).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "run ids should be unique");
    }

    #[test]
    fn schema_creates_tables() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("events.db");
        let _log = EventLog::new(&path).unwrap();
        let conn = Connection::open(&path).unwrap();
        let runs: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='runs'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(runs, 1);
        let events: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='events'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(events, 1);
    }
}
