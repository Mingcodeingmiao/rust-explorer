//! On-demand, cancellable, single-worker traversal. Never follows reparse points.
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
pub const CACHE_TTL: Duration = Duration::from_secs(60 * 60);
const CACHE_LIMIT: usize = 4096;
#[derive(Default)]
struct Cache {
    epoch: u64,
    entries: HashMap<PathBuf, (Instant, Usage)>,
}
impl Cache {
    fn get(&mut self, path: &Path, now: Instant) -> Option<Usage> {
        if self
            .entries
            .get(path)
            .is_some_and(|(when, _)| now.saturating_duration_since(*when) < CACHE_TTL)
        {
            return self.entries.get(path).map(|(_, usage)| usage.clone());
        }
        self.entries.remove(path);
        None
    }
    fn put(&mut self, path: PathBuf, usage: Usage, now: Instant, epoch: u64) {
        if !usage.complete || epoch != self.epoch {
            return;
        }
        self.entries
            .retain(|_, (when, _)| now.saturating_duration_since(*when) < CACHE_TTL);
        if self.entries.len() >= CACHE_LIMIT {
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (when, _))| *when)
                .map(|(path, _)| path.clone())
            {
                self.entries.remove(&oldest);
            }
        }
        self.entries.insert(path, (now, usage));
    }
}
fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}
/// Shared across windows in this process. Cancelled traversals never enter the cache.
pub fn calculate_cached(path: &Path, cancel: &Arc<AtomicBool>) -> (Usage, bool) {
    if cancel.load(Ordering::Relaxed) {
        return (Usage::default(), false);
    }
    let epoch = {
        let mut cache = cache().lock().unwrap();
        if let Some(usage) = cache.get(path, Instant::now()) {
            return (usage, true);
        }
        cache.epoch
    };
    let usage = calculate(path, cancel);
    if !cancel.load(Ordering::Relaxed) {
        cache()
            .lock()
            .unwrap()
            .put(path.to_path_buf(), usage.clone(), Instant::now(), epoch);
    }
    (usage, false)
}
/// Explicit file mutations invalidate old totals, including in-flight traversals.
pub fn invalidate_cache() {
    let mut cache = cache().lock().unwrap();
    cache.epoch = cache.epoch.wrapping_add(1);
    cache.entries.clear();
}
#[derive(Clone, Debug, Default)]
pub struct Usage {
    pub bytes: u64,
    pub files: u64,
    pub skipped: u64,
    pub complete: bool,
}
fn reparse(m: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        m.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        m.file_type().is_symlink()
    }
}
pub fn calculate(path: &Path, cancel: &Arc<AtomicBool>) -> Usage {
    let mut result = Usage::default();
    let mut directories = Vec::<std::fs::ReadDir>::new();
    let mut next = Some(path.to_path_buf());
    loop {
        if cancel.load(Ordering::Relaxed) {
            return result;
        }
        let path = if let Some(path) = next.take() {
            path
        } else {
            let Some(directory) = directories.last_mut() else {
                break;
            };
            match directory.next() {
                Some(Ok(entry)) => entry.path(),
                Some(Err(_)) => {
                    result.skipped += 1;
                    continue;
                }
                None => {
                    directories.pop();
                    continue;
                }
            }
        };
        let m = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(_) => {
                result.skipped += 1;
                continue;
            }
        };
        if reparse(&m) {
            result.skipped += 1;
            continue;
        }
        if m.is_dir() {
            match std::fs::read_dir(path) {
                Ok(items) => directories.push(items),
                Err(_) => result.skipped += 1,
            }
        } else {
            result.bytes = result.bytes.saturating_add(m.len());
            result.files += 1;
        }
    }
    result.complete = true;
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_expires_at_one_hour_and_rejects_partial_or_invalidated_work() {
        let mut cache = Cache::default();
        let now = Instant::now();
        let path = PathBuf::from("test-cache-entry");
        cache.put(
            path.clone(),
            Usage {
                bytes: 17,
                complete: true,
                ..Default::default()
            },
            now,
            0,
        );
        assert_eq!(
            cache
                .get(&path, now + CACHE_TTL - Duration::from_secs(1))
                .unwrap()
                .bytes,
            17
        );
        assert!(cache.get(&path, now + CACHE_TTL).is_none());
        cache.put(path.clone(), Usage::default(), now, 0);
        assert!(cache.get(&path, now).is_none());
        cache.epoch = 1;
        cache.put(
            path.clone(),
            Usage {
                complete: true,
                ..Default::default()
            },
            now,
            0,
        );
        assert!(cache.get(&path, now).is_none());
    }
    #[test]
    fn cached_traversal_reuses_totals_until_invalidated() {
        let root =
            std::env::temp_dir().join(format!("rust-explorer-du-cache-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let file = root.join("fixture");
        let cancel = Arc::new(AtomicBool::new(false));
        invalidate_cache();
        std::fs::write(&file, [0; 17]).unwrap();
        let (first, hit) = calculate_cached(&root, &cancel);
        assert_eq!((first.bytes, hit), (17, false));
        std::fs::write(&file, [0; 29]).unwrap();
        let (cached, hit) = calculate_cached(&root, &cancel);
        assert_eq!((cached.bytes, hit), (17, true));
        invalidate_cache();
        let (updated, hit) = calculate_cached(&root, &cancel);
        assert_eq!((updated.bytes, hit), (29, false));
        cancel.store(true, Ordering::Relaxed);
        assert!(!calculate_cached(&root, &cancel).0.complete);
        std::fs::remove_file(file).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
    #[test]
    fn totals_and_cancellation() {
        let root = std::env::temp_dir().join(format!("rust-explorer-du-{}", std::process::id()));
        std::fs::create_dir_all(root.join("nested")).unwrap();
        std::fs::write(root.join("a"), [0; 17]).unwrap();
        std::fs::write(root.join("nested/b"), [0; 29]).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let usage = calculate(&root, &cancel);
        assert_eq!((usage.bytes, usage.files, usage.complete), (46, 2, true));
        cancel.store(true, Ordering::Relaxed);
        assert!(!calculate(&root, &cancel).complete);
        std::fs::remove_dir_all(root).unwrap();
    }
}
