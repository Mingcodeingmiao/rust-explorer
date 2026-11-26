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
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
static REVISION: AtomicU64 = AtomicU64::new(0);

pub fn revision() -> u64 {
    REVISION.load(Ordering::Acquire)
}

pub const CACHE_TTL: Duration = Duration::from_secs(60 * 60);
#[derive(Default)]
struct Cache {
    serial: u64,
    next_expiry: Option<Instant>,
    entries: HashMap<PathBuf, Usage>,
    in_flight: HashMap<PathBuf, u64>,
}
impl Cache {
    fn prune(&mut self, now: Instant) {
        if self.next_expiry.is_some_and(|expiry| now >= expiry) {
            self.entries.retain(|_, usage| !usage.expired(now));
            self.next_expiry = self
                .entries
                .values()
                .filter_map(|usage| usage.expires_at)
                .min();
        }
    }
    fn get(&mut self, path: &Path, now: Instant) -> Option<Usage> {
        self.prune(now);
        if let Some(usage) = self.entries.get(path).filter(|usage| !usage.expired(now)) {
            return Some(usage.clone());
        }
        self.entries.remove(path);
        None
    }
    fn begin(&mut self, path: &Path) -> u64 {
        self.serial = self.serial.wrapping_add(1);
        self.in_flight.insert(path.to_path_buf(), self.serial);
        self.serial
    }
    fn invalidate(&mut self, path: &Path) {
        self.entries.remove(path);
        self.in_flight.remove(path);
    }
    fn finish(&mut self, path: &Path, usage: &Usage, now: Instant, serial: u64) -> bool {
        if self.in_flight.get(path) != Some(&serial) {
            return false;
        }
        self.in_flight.remove(path);
        if let Some(expiry) = usage.expires_at.filter(|_| usage.complete) {
            self.prune(now);
            self.next_expiry = Some(self.next_expiry.map_or(expiry, |next| next.min(expiry)));
            self.entries.insert(path.to_path_buf(), usage.clone());
        }
        true
    }
}
fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}
/// Read cached results without starting any filesystem traversal.
pub fn cached_sizes<'a>(paths: impl Iterator<Item = &'a Path>) -> HashMap<PathBuf, Usage> {
    let mut cache = cache().lock().unwrap();
    let now = Instant::now();
    paths
        .filter_map(|path| {
            cache
                .get(path, now)
                .map(|usage| (path.to_path_buf(), usage))
        })
        .collect()
}
/// Refresh the displayed directories while retaining unrelated cached totals.
pub fn invalidate_paths<'a>(paths: impl Iterator<Item = &'a Path>) {
    let mut cache = cache().lock().unwrap();
    for path in paths {
        cache.invalidate(path);
    }
    REVISION.fetch_add(1, Ordering::Release);
}
/// Shared across windows in this process. Cancelled traversals never enter the cache.
pub fn calculate_cached(path: &Path, cancel: &Arc<AtomicBool>) -> Option<(Usage, bool)> {
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let serial = {
        let mut cache = cache().lock().unwrap();
        if let Some(usage) = cache.get(path, Instant::now()) {
            return Some((usage, true));
        }
        cache.begin(path)
    };
    let mut usage = calculate(path, cancel);
    let now = Instant::now();
    if cancel.load(Ordering::Relaxed) {
        usage.complete = false;
    }
    if usage.complete {
        usage.expires_at = Some(now + CACHE_TTL);
    }
    let mut cache = cache().lock().unwrap();
    if !cache.finish(path, &usage, now, serial) || !usage.complete {
        return None;
    }
    REVISION.fetch_add(1, Ordering::Release);
    Some((usage, false))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub bytes: u64,
    pub files: u64,
    pub skipped: u64,
    pub complete: bool,
    pub expires_at: Option<Instant>,
}
impl Usage {
    pub fn expired(&self, now: Instant) -> bool {
        self.expires_at.is_some_and(|expiry| now >= expiry)
    }
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
        let serial = cache.begin(&path);
        let usage = Usage {
            bytes: 17,
            complete: true,
            expires_at: Some(now + CACHE_TTL),
            ..Default::default()
        };
        cache.finish(&path, &usage, now, serial);
        assert_eq!(
            cache
                .get(&path, now + CACHE_TTL - Duration::from_secs(1))
                .unwrap()
                .bytes,
            17
        );
        assert!(cache.get(&path, now + CACHE_TTL).is_none());
        let serial = cache.begin(&path);
        cache.finish(&path, &Usage::default(), now, serial);
        assert!(cache.get(&path, now).is_none());
        let serial = cache.begin(&path);
        cache.invalidate(&path);
        assert!(!cache.finish(&path, &usage, now, serial));
        assert!(cache.get(&path, now).is_none());

        let serial = cache.begin(&path);
        cache.invalidate(Path::new("unrelated"));
        cache.finish(&path, &usage, now, serial);
        assert_eq!(cache.get(&path, now).unwrap().bytes, 17);
        for index in 0..4096 {
            let extra = PathBuf::from(format!("entry-{index}"));
            let serial = cache.begin(&extra);
            cache.finish(&extra, &usage, now, serial);
        }
        assert_eq!(cache.get(&path, now).unwrap().bytes, 17);
    }
    #[test]
    fn cached_traversal_reuses_totals_until_invalidated() {
        let root =
            std::env::temp_dir().join(format!("rust-explorer-du-cache-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let file = root.join("fixture");
        let cancel = Arc::new(AtomicBool::new(false));
        std::fs::write(&file, [0; 17]).unwrap();
        let (first, hit) = calculate_cached(&root, &cancel).unwrap();
        assert_eq!((first.bytes, hit), (17, false));
        std::fs::write(&file, [0; 29]).unwrap();
        let (cached, hit) = calculate_cached(&root, &cancel).unwrap();
        assert_eq!((cached.bytes, hit), (17, true));
        assert_eq!(
            cached_sizes(std::iter::once(root.as_path()))[&root].bytes,
            17
        );
        let other = root.join("unrelated");
        {
            let mut cache = cache().lock().unwrap();
            let now = Instant::now();
            let serial = cache.begin(&other);
            cache.finish(
                &other,
                &Usage {
                    bytes: 5,
                    complete: true,
                    expires_at: Some(now + CACHE_TTL),
                    ..Default::default()
                },
                now,
                serial,
            );
        }
        invalidate_paths(std::iter::once(root.as_path()));
        assert!(cached_sizes(std::iter::once(root.as_path())).is_empty());
        assert_eq!(
            cached_sizes(std::iter::once(other.as_path()))[&other].bytes,
            5
        );
        let (updated, hit) = calculate_cached(&root, &cancel).unwrap();
        assert_eq!((updated.bytes, hit), (29, false));
        cancel.store(true, Ordering::Relaxed);
        assert!(calculate_cached(&root, &cancel).is_none());
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
