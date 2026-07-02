//! The scheduler - persisted jobs that fire even across sleep.
//!
//! Jobs are stored as plain JSON (`jobs.json`); each holds its [`Recurrence`] and a
//! precomputed `next_fire_ms`. The core does not stay resident to wait: an external
//! timer wakes the socket-activated core, which runs what is due (`--run-due`) and
//! sleeps again. If the machine was asleep past several fires, rescheduling advances
//! to the next *future* occurrence - one catch-up run, never a stampede.
//!
//! WAKE ARMING IS NOT YET WIRED. [`Scheduler::next_wake`] computes the soonest fire and
//! [`crate::systemd::wake_timer`] can generate the `--run-due` timer unit, but nothing in the
//! daemon or deploy command currently calls them, so the only wake in production is a static
//! `OnCalendar=*-*-* 09:00:00` documented in `deploy/README.md`. Until the daemon recomputes
//! `next_wake()` on job add/remove/fire (or a fine-grained recurring timer is installed),
//! sub-daily and non-9am schedules only get a chance to fire at 09:00. See NEEDS-INTEGRATION.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use engram_core::Ledger;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::recur::Recurrence;

#[derive(Debug, thiserror::Error)]
pub enum SchedError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("ledger: {0}")]
    Ledger(#[from] engram_core::LedgerError),
    #[error("schedule never fires: {0}")]
    NeverFires(String),
    #[error("not found: {0}")]
    NotFound(String),
}

type Result<T> = std::result::Result<T, SchedError>;

/// A scheduled unit of work. `payload` is opaque to the scheduler - it is whatever
/// the core needs to run the job (a skill id, a prompt, a command).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub name: String,
    pub payload: serde_json::Value,
    pub recurrence: Recurrence,
    pub next_fire_ms: i64,
    pub created_ms: i64,
    pub last_fire_ms: Option<i64>,
    /// The id of the most recent task this job spawned, so the UI can open its receipt.
    #[serde(default)]
    pub last_task_id: Option<String>,
}

pub struct Scheduler {
    path: PathBuf,
    ledger: Arc<Ledger>,
    jobs: Mutex<Vec<Job>>,
}

impl Scheduler {
    pub fn open(dir: impl AsRef<Path>, ledger: Arc<Ledger>) -> Result<Self> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir)?;
        let path = dir.join("jobs.json");
        // Back up an unparseable file rather than hard-failing boot: on an unattended box a
        // power-loss mid-save can leave a truncated jobs.json, and propagating that parse error
        // (via `?` at the daemon's `Scheduler::open`) would take the whole product down until
        // someone hand-deletes the file. Mirror TaskStore: move the bad file aside and start fresh.
        let jobs = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(jobs) => jobs,
                Err(e) => {
                    let _ = std::fs::rename(&path, dir.join("jobs.corrupt.json"));
                    tracing::error!(error = %e, "jobs.json was unparseable - backed it up and started fresh");
                    Vec::new()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        Ok(Scheduler {
            path,
            ledger,
            jobs: Mutex::new(jobs),
        })
    }

    /// Schedule a new job. Errors if the recurrence has no future fire.
    pub fn add(
        &self,
        name: impl Into<String>,
        payload: serde_json::Value,
        recurrence: Recurrence,
        now: DateTime<Utc>,
    ) -> Result<Job> {
        let name = name.into();
        let next = recurrence
            .next_after(now)
            .ok_or_else(|| SchedError::NeverFires(name.clone()))?;
        let created_ms = now.timestamp_millis();
        let job = Job {
            id: format!("{}-{}", slug(&name), created_ms),
            name,
            payload,
            recurrence,
            next_fire_ms: next.timestamp_millis(),
            created_ms,
            last_fire_ms: None,
            last_task_id: None,
        };
        {
            let mut jobs = self.jobs.lock().expect("sched mutex poisoned");
            jobs.push(job.clone());
            save(&self.path, &jobs)?;
        }
        self.ledger.append(
            "schedule.create",
            "user",
            json!({ "id": job.id, "name": job.name, "next_fire_ms": job.next_fire_ms }),
        )?;
        Ok(job)
    }

    /// Jobs whose fire time has arrived.
    pub fn due(&self, now: DateTime<Utc>) -> Vec<Job> {
        let now_ms = now.timestamp_millis();
        let jobs = self.jobs.lock().expect("sched mutex poisoned");
        jobs.iter()
            .filter(|j| j.next_fire_ms <= now_ms)
            .cloned()
            .collect()
    }

    /// Record that a job fired and reschedule it (or remove a spent one-off). Returns
    /// the rescheduled job, or `None` if it was a one-off that is now complete.
    pub fn mark_fired(&self, id: &str, now: DateTime<Utc>) -> Result<Option<Job>> {
        let now_ms = now.timestamp_millis();
        let result = {
            let mut jobs = self.jobs.lock().expect("sched mutex poisoned");
            let idx = jobs
                .iter()
                .position(|j| j.id == id)
                .ok_or_else(|| SchedError::NotFound(id.to_string()))?;
            // Advance to the next strictly-future occurrence (skip any missed fires).
            let next = jobs[idx].recurrence.next_after(now);
            let result = match next {
                Some(t) => {
                    jobs[idx].next_fire_ms = t.timestamp_millis();
                    jobs[idx].last_fire_ms = Some(now_ms);
                    Some(jobs[idx].clone())
                }
                None => {
                    jobs.remove(idx);
                    None
                }
            };
            save(&self.path, &jobs)?;
            result
        };
        self.ledger.append(
            "schedule.fire",
            "core",
            json!({ "id": id, "rescheduled": result.as_ref().map(|j| j.next_fire_ms) }),
        )?;
        Ok(result)
    }

    /// Update a job in place: rename, retime (new recurrence), or replace its payload. The id
    /// stays stable so receipts and UI links keep working; the next fire is recomputed when the
    /// cadence changes. The update is itself signed to the ledger.
    pub fn update(
        &self,
        id: &str,
        name: Option<String>,
        payload: Option<serde_json::Value>,
        recurrence: Option<Recurrence>,
        now: DateTime<Utc>,
    ) -> Result<Job> {
        let job = {
            let mut jobs = self.jobs.lock().expect("sched mutex poisoned");
            let j = jobs
                .iter_mut()
                .find(|j| j.id == id)
                .ok_or_else(|| SchedError::NotFound(id.to_string()))?;
            if let Some(n) = name {
                j.name = n;
            }
            if let Some(p) = payload {
                j.payload = p;
            }
            if let Some(r) = recurrence {
                let next = r
                    .next_after(now)
                    .ok_or_else(|| SchedError::NeverFires(j.name.clone()))?;
                j.recurrence = r;
                j.next_fire_ms = next.timestamp_millis();
            }
            let out = j.clone();
            save(&self.path, &jobs)?;
            out
        };
        self.ledger.append(
            "schedule.update",
            "user",
            json!({ "id": job.id, "name": job.name, "next_fire_ms": job.next_fire_ms }),
        )?;
        Ok(job)
    }

    /// Record the task most recently spawned for a job, so the UI can link to its receipt.
    /// Persists, but does not touch the ledger (the fire itself is already audited).
    pub fn set_last_task(&self, id: &str, task_id: &str) -> Result<()> {
        let mut jobs = self.jobs.lock().expect("sched mutex poisoned");
        let job = jobs
            .iter_mut()
            .find(|j| j.id == id)
            .ok_or_else(|| SchedError::NotFound(id.to_string()))?;
        job.last_task_id = Some(task_id.to_string());
        save(&self.path, &jobs)?;
        Ok(())
    }

    pub fn remove(&self, id: &str) -> Result<bool> {
        let removed = {
            let mut jobs = self.jobs.lock().expect("sched mutex poisoned");
            let before = jobs.len();
            jobs.retain(|j| j.id != id);
            let changed = jobs.len() != before;
            if changed {
                save(&self.path, &jobs)?;
            }
            changed
        };
        if removed {
            self.ledger
                .append("schedule.remove", "user", json!({ "id": id }))?;
        }
        Ok(removed)
    }

    pub fn list(&self) -> Vec<Job> {
        self.jobs.lock().expect("sched mutex poisoned").clone()
    }

    /// The soonest upcoming fire time across all jobs (epoch millis), if any. Intended for the
    /// deploy/daemon layer to arm a wake timer for this instant — but that arming is NOT yet wired
    /// (see the module doc), so today this only feeds tests and callers that opt in.
    pub fn next_wake(&self) -> Option<i64> {
        self.jobs
            .lock()
            .expect("sched mutex poisoned")
            .iter()
            .map(|j| j.next_fire_ms)
            .min()
    }
}

/// Write jobs.json atomically (temp + rename) and owner-only. `save` runs on every
/// add/mark_fired/set_last_task/remove, i.e. constantly on an unattended box; a bare
/// `std::fs::write` can be interrupted mid-write and leave a truncated file, so we write a
/// sibling temp file and rename it into place (rename is atomic on the same filesystem).
fn save(path: &Path, jobs: &[Job]) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(jobs)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn slug(name: &str) -> String {
    let s: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "job".into()
    } else {
        s.chars().take(32).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 24, 12, 0, 0).unwrap()
    }

    fn scheduler() -> (Scheduler, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Arc::new(Ledger::open(dir.path()).unwrap());
        (Scheduler::open(dir.path(), ledger).unwrap(), dir)
    }

    #[test]
    fn adds_and_finds_due() {
        let (s, _d) = scheduler();
        // Fires in 1 second; not due now, due a minute later.
        s.add("ping", json!({}), Recurrence::Interval { secs: 1 }, now())
            .unwrap();
        assert!(s.due(now()).is_empty());
        let later = now() + chrono::Duration::seconds(2);
        assert_eq!(s.due(later).len(), 1);
    }

    #[test]
    fn one_off_completes_and_is_removed() {
        let (s, _d) = scheduler();
        let at = (now() + chrono::Duration::minutes(5)).timestamp_millis();
        let job = s
            .add("reminder", json!({}), Recurrence::Once { at_ms: at }, now())
            .unwrap();
        let after = now() + chrono::Duration::minutes(6);
        let res = s.mark_fired(&job.id, after).unwrap();
        assert!(res.is_none(), "spent one-off should be removed");
        assert!(s.list().is_empty());
    }

    #[test]
    fn recurring_reschedules_forward_skipping_missed() {
        let (s, _d) = scheduler();
        let job = s
            .add(
                "hourly",
                json!({}),
                Recurrence::Interval { secs: 3600 },
                now(),
            )
            .unwrap();
        // Machine was asleep for ~5 hours; one catch-up, next fire is in the future.
        let woke = now() + chrono::Duration::hours(5);
        let res = s.mark_fired(&job.id, woke).unwrap().unwrap();
        assert!(res.next_fire_ms > woke.timestamp_millis());
        assert_eq!(s.due(woke).len(), 0, "no stampede of missed fires");
    }

    #[test]
    fn persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Arc::new(Ledger::open(dir.path()).unwrap());
        {
            let s = Scheduler::open(dir.path(), ledger.clone()).unwrap();
            s.add(
                "daily",
                json!({}),
                Recurrence::Daily { hour: 9, min: 0 },
                now(),
            )
            .unwrap();
        }
        let s = Scheduler::open(dir.path(), ledger).unwrap();
        assert_eq!(s.list().len(), 1);
        assert!(s.next_wake().is_some());
    }
}
