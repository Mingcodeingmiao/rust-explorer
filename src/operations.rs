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
    time::{Duration, Instant},
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
    kind: OperationKind,
}

/// History must not race a mutation in any folder window. Read-only work may
/// overlap; the owning view separately waits until its receipts are applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationKind {
    ReadOnly,
    Mutation,
    History,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BeginError {
    Limit,
    HistoryBusy,
}

/// Owned by the worker, never by the window receiving its result. Even a panic
/// or an abandoned task must release the process-wide running-operation slot.
pub struct Completion {
    id: Option<u64>,
}
impl Completion {
    pub fn new(id: u64) -> Self {
        Self { id: Some(id) }
    }
    pub fn finish(mut self, status: String) {
        if let Some(id) = self.id.take() {
            finish(id, status);
        }
    }
}
impl Drop for Completion {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            finish(id, crate::i18n::text("失败").into());
        }
    }
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
pub fn try_begin(
    id: u64,
    label: String,
    details: String,
    control: OperationControl,
    kind: OperationKind,
) -> Result<(), BeginError> {
    let mut records = records().lock().map_err(|_| BeginError::Limit)?;
    let running: Vec<_> = records
        .iter()
        .filter(|record| record.finished.is_none())
        .collect();
    if running.len() >= 8 {
        return Err(BeginError::Limit);
    }
    if (kind == OperationKind::History
        && running
            .iter()
            .any(|record| record.kind != OperationKind::ReadOnly))
        || (kind == OperationKind::Mutation
            && running
                .iter()
                .any(|record| record.kind == OperationKind::History))
    {
        return Err(BeginError::HistoryBusy);
    }
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
        kind,
    });
    drop(records);
    changed();
    Ok(())
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

/// A visible operation clock ticks at most once a second; an idle window needs
/// no time-driven repaint at all.
pub fn elapsed_refresh_due(since_last_update: Duration, running: bool) -> bool {
    running && since_last_update >= Duration::from_secs(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn begin(id: u64, label: String, details: String, control: OperationControl) {
        try_begin(id, label, details, control, OperationKind::Mutation).unwrap();
    }

    fn start(id: u64, kind: OperationKind) -> Result<(), BeginError> {
        try_begin(
            id,
            String::new(),
            String::new(),
            OperationControl::default(),
            kind,
        )
    }

    #[test]
    fn completed_worker_releases_slot_even_when_its_window_is_gone() {
        reset();
        start(101, OperationKind::ReadOnly).unwrap();
        let completion = Completion::new(101);
        let (result, window) = std::sync::mpsc::channel();
        drop(window);
        std::thread::spawn(move || {
            completion.finish("已完成".into());
            assert!(result.send(()).is_err());
        })
        .join()
        .unwrap();
        assert_eq!(running(), 0);
        assert!(snapshot()[0].finished.is_some());

        start(102, OperationKind::Mutation).unwrap();
        // Dropping an abandoned worker also releases its slot.
        drop(Completion::new(102));
        assert_eq!(running(), 0);
        reset();
    }

    #[test]
    fn history_and_mutations_cannot_overlap_across_windows() {
        reset();
        start(1, OperationKind::Mutation).unwrap();
        start(2, OperationKind::Mutation).unwrap();
        assert_eq!(
            start(3, OperationKind::History),
            Err(BeginError::HistoryBusy)
        );
        finish(1, String::new());
        assert_eq!(
            start(3, OperationKind::History),
            Err(BeginError::HistoryBusy)
        );
        finish(2, String::new());
        start(3, OperationKind::History).unwrap();
        assert_eq!(
            start(4, OperationKind::Mutation),
            Err(BeginError::HistoryBusy)
        );
        assert_eq!(
            start(5, OperationKind::History),
            Err(BeginError::HistoryBusy)
        );
        // Properties do not produce history and need not block undo/redo.
        start(6, OperationKind::ReadOnly).unwrap();
        assert_eq!(running(), 2);
        finish(3, String::new());
        start(4, OperationKind::Mutation).unwrap();
        reset();
    }

    #[test]
    fn admission_limit_is_atomic_and_released_on_completion() {
        reset();
        for id in 1..=8 {
            start(id, OperationKind::Mutation).unwrap();
        }
        assert_eq!(start(9, OperationKind::Mutation), Err(BeginError::Limit));
        finish(1, String::new());
        start(9, OperationKind::Mutation).unwrap();
        assert_eq!(running(), 8);
        reset();
    }

    #[test]
    fn elapsed_clock_ticks_only_while_running() {
        assert!(!elapsed_refresh_due(Duration::from_millis(999), true));
        assert!(elapsed_refresh_due(Duration::from_secs(1), true));
        assert!(!elapsed_refresh_due(Duration::from_secs(60), false));
    }

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
