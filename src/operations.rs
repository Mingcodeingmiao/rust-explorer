//! Process-wide record of the system (file) operations that are running.
//!
//! The operations window and the window that started an operation are different
//! views, so the records cannot live in one window's entity. They are kept here,
//! behind a mutex, with a version counter the views poll. Each record carries the
//! [`OperationControl`] so any view can request a stop.

use crate::shell::OperationControl;
use std::{
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

/// One operation as shown in the operations window and the status bar.
#[derive(Clone)]
pub struct OperationRecord {
    pub id: u64,
    pub label: String,
    /// What the operation acts on (paths and destination), one line.
    pub details: String,
    pub started: Instant,
    /// Wall-clock start, for the window's "started at" line.
    pub started_at: chrono::DateTime<chrono::Local>,
    pub finished: Option<Instant>,
    pub status: String,
    pub control: OperationControl,
}

/// Finished records kept for the history in the window.
const KEEP: usize = 32;

static RECORDS: OnceLock<Mutex<Vec<OperationRecord>>> = OnceLock::new();
static VERSION: AtomicU64 = AtomicU64::new(0);
/// Process-wide operation ids. Window-local counters reused id 1 in every
/// window, so finishing one window's copy could leave another window's record
/// "running" forever.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Allocate an id that is unique across every Explorer window.
pub fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

fn records() -> &'static Mutex<Vec<OperationRecord>> {
    RECORDS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Version counter: views re-render when it changes.
pub fn version() -> u64 {
    VERSION.load(Ordering::Relaxed)
}

fn changed() {
    VERSION.fetch_add(1, Ordering::Relaxed);
}

/// Record a newly started operation. Oldest finished records are dropped when the
/// history is full.
pub fn begin(id: u64, label: String, details: String, control: OperationControl) {
    let Ok(mut records) = records().lock() else {
        return;
    };
    while records.len() >= KEEP {
        match records.iter().position(|record| record.finished.is_some()) {
            Some(index) => {
                records.remove(index);
            }
            None => break,
        }
    }
    records.push(OperationRecord {
        id,
        label,
        details,
        started: Instant::now(),
        started_at: chrono::Local::now(),
        finished: None,
        status: crate::i18n::text("正在进行").to_string(),
        control,
    });
    drop(records);
    changed();
}

/// Mark an operation as finished with its final status text.
pub fn finish(id: u64, status: String) {
    if let Ok(mut records) = records().lock()
        && let Some(record) = records.iter_mut().find(|record| record.id == id)
    {
        record.finished = Some(Instant::now());
        record.status = status;
    }
    changed();
}

/// Replace the status text of a running operation (e.g. "stopping").
pub fn set_status(id: u64, status: String) {
    if let Ok(mut records) = records().lock()
        && let Some(record) = records.iter_mut().find(|record| record.id == id)
    {
        record.status = status;
    }
    changed();
}

/// Remove finished history while keeping running operations visible so their
/// stop controls remain available.
pub fn clear_finished() {
    if let Ok(mut records) = records().lock() {
        records.retain(|record| record.finished.is_none());
    }
    changed();
}

/// Ask one operation to stop. `force` kills the helper instead of waiting for the
/// operating system to wind down.
pub fn stop(id: u64, force: bool) {
    let found = if let Ok(records) = records().lock()
        && let Some(record) = records.iter().find(|record| record.id == id)
    {
        record.control.stop(force);
        true
    } else {
        false
    };
    if found {
        record_status(id, crate::i18n::text("正在请求停止"));
    }
}

fn record_status(id: u64, status: &str) {
    if let Ok(mut records) = records().lock()
        && let Some(record) = records.iter_mut().find(|record| record.id == id)
    {
        record.status = status.to_string();
    }
    changed();
}

/// Ask every running operation to stop; returns how many were asked.
pub fn stop_all(force: bool) -> usize {
    let mut asked = 0;
    if let Ok(records) = records().lock() {
        for record in records.iter().filter(|record| record.finished.is_none()) {
            record.control.stop(force);
            asked += 1;
        }
    }
    if asked > 0 {
        record_status_all(crate::i18n::text("正在请求停止"));
    }
    asked
}

fn record_status_all(status: &str) {
    if let Ok(mut records) = records().lock() {
        for record in records
            .iter_mut()
            .filter(|record| record.finished.is_none())
        {
            record.status = status.to_string();
        }
    }
    changed();
}

/// Whether a stop was requested for this operation.
pub fn stopping(id: u64) -> bool {
    records()
        .lock()
        .ok()
        .and_then(|records| {
            records
                .iter()
                .find(|record| record.id == id)
                .map(|record| record.control.stopping())
        })
        .unwrap_or(false)
}

/// Number of operations that have not finished yet.
pub fn running() -> usize {
    records()
        .lock()
        .map(|records| {
            records
                .iter()
                .filter(|record| record.finished.is_none())
                .count()
        })
        .unwrap_or(0)
}

/// Copy of the records, oldest first.
pub fn snapshot() -> Vec<OperationRecord> {
    records()
        .lock()
        .map(|records| records.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset() {
        if let Ok(mut records) = records().lock() {
            records.clear();
        }
        changed();
    }

    #[test]
    fn records_track_progress_and_cap_the_history() {
        reset();
        let control = OperationControl::default();
        begin(1, "复制".into(), "a → b".into(), control.clone());
        begin(2, "删除".into(), "c".into(), control);
        assert_eq!(running(), 2);
        finish(1, "已完成".into());
        assert_eq!(running(), 1);
        let records = snapshot();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].status, "已完成");
        assert!(records[0].finished.is_some());

        for id in 3..(KEEP as u64 + 6) {
            begin(
                id,
                "复制".into(),
                String::new(),
                OperationControl::default(),
            );
            finish(id, "已完成".into());
        }
        assert!(snapshot().len() <= KEEP);
        reset();
    }

    #[test]
    fn operation_ids_are_unique_across_windows() {
        let first = next_id();
        let second = next_id();
        assert_ne!(first, second);
    }

    #[test]
    fn clear_finished_keeps_running_operations() {
        reset();
        begin(1, "复制".into(), String::new(), OperationControl::default());
        finish(1, "已完成".into());
        begin(2, "删除".into(), String::new(), OperationControl::default());
        clear_finished();
        let records = snapshot();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, 2);
        assert!(records[0].finished.is_none());
        finish(2, "已完成".into());
        clear_finished();
        assert!(snapshot().is_empty());
        reset();
    }

    #[test]
    fn stopping_marks_the_status_and_flags_the_control() {
        reset();
        begin(7, "复制".into(), String::new(), OperationControl::default());
        stop(7, false);
        let records = snapshot();
        assert!(records[0].control.stopping());
        assert_eq!(records[0].status, crate::i18n::text("正在请求停止"));
        assert_eq!(stop_all(false), 1);
        reset();
    }
}
