//! Visible-only thumbnail scheduling. No file I/O or decoding on the UI thread.
use crate::fs::Entry;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

const MAX_CACHE: usize = 512;
const CACHE_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_VISIBLE: usize = 512;
/// A viewport change younger than this delays the first request, so a fast scroll
/// does not start work for rows that are already leaving. Kept short: every
/// preview waits for it after the folder opens.
const SETTLE: Duration = Duration::from_millis(60);

#[derive(Clone, Debug, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct Key {
    pub path: PathBuf,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub pixels: u32,
    #[serde(default)]
    pub directory: bool,
    /// Shortcut targets may change without changing the .lnk metadata.
    #[serde(default)]
    pub shortcut_epoch: u64,
}
impl Key {
    pub fn from_entry(entry: &Entry, pixels: u32) -> Option<Self> {
        let extension = entry
            .path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        // Portable-device and recycled items live in the Shell namespace: no
        // filesystem path to check, and their listing already carries size and
        // mtime. They are cached like any local item so a phone preview does not
        // vanish when the window refreshes.
        let namespace = crate::shell_folder::is_namespace_item(&entry.path);
        if (!entry.directory && entry.size == 0 && !namespace)
            || (!entry.path.is_absolute() && !namespace)
            || (!namespace && entry.path.to_string_lossy().starts_with("\\\\"))
            || (!entry.directory
                && !namespace
                && !matches!(
                    extension.as_str(),
                    "jpg"
                        | "jpeg"
                        | "png"
                        | "gif"
                        | "bmp"
                        | "webp"
                        | "tif"
                        | "tiff"
                        | "heic"
                        | "heif"
                        | "avif"
                        | "mp4"
                        | "m4v"
                        | "webm"
                        | "mov"
                        | "mkv"
                        | "avi"
                        | "wmv"
                        | "pdf"
                        | "lnk"
                        // An Internet shortcut has no thumbnail: the Shell's icon
                        // fallback returns a generic page. Its real artwork comes
                        // from the icon pipeline (`IconFile=` through an STA
                        // apartment), so `.url` items are not thumbnail requests.
                        | "ttf"
                        | "otf"
                        | "ttc"
                        | "fon"
                ))
        {
            return None;
        }
        Some(Self {
            path: entry.path.clone(),
            size: if entry.directory { 0 } else { entry.size },
            modified: entry.modified,
            pixels: resolution(pixels),
            directory: entry.directory,
            // Only shortcuts need the time bucket: their target can change without
            // the .lnk metadata changing. Folder artwork is invalidated by the
            // folder's own mtime, so a folder keeps one cache entry instead of a new
            // one every 30 seconds (composition is the slowest preview to build).
            shortcut_epoch: if matches!(extension.as_str(), "lnk" | "url") {
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
                    / 30
            } else {
                0
            },
        })
    }
    fn cost(&self) -> usize {
        self.pixels as usize * self.pixels as usize * 4
    }
}
pub fn resolution(pixels: u32) -> u32 {
    [48, 64, 96, 128, 192, 256]
        .into_iter()
        .find(|n| *n >= pixels)
        .unwrap_or(256)
}
struct Cached {
    path: Option<PathBuf>,
    presentation: Presentation,
    used: u64,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Presentation {
    pub directory: bool,
    pub shortcut: bool,
    pub video: bool,
    pub width: u32,
    pub height: u32,
    pub source_width: u32,
    pub source_height: u32,
}
#[derive(Default)]
struct State {
    visible: Vec<Key>,
    visible_set: HashSet<Key>,
    pending: VecDeque<Key>,
    cache: HashMap<Key, Cached>,
    /// One slot per worker thread: the key it is generating and its cancel flag.
    active: Vec<Option<(Key, Arc<AtomicBool>)>>,
    /// A Shell namespace item (portable device, recycled file) is generated
    /// alone: one phone does not serve concurrent thumbnail sessions.
    shell_busy: bool,
    updated: Option<Instant>,
    clock: u64,
    stopped: bool,
}
/// Thumbnails are generated by a small pool of workers, each with its own codec
/// helper process. Measured on a 16k-entry folder (C:\Windows\WinSxS): two workers
/// already saturate Shell preview generation, four is a little better and eight
/// adds nothing, so the pool is capped instead of spawning one helper per core
/// (`cores - 2`, at least one).
const MAX_WORKERS: usize = 4;
fn worker_count() -> usize {
    let cores = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(4)
        .saturating_sub(2)
        .max(1);
    cores.min(MAX_WORKERS)
}
fn is_shell(key: &Key) -> bool {
    crate::shell_folder::is_namespace_item(&key.path)
}
impl State {
    fn set_visible(&mut self, keys: Vec<Key>) {
        let mut seen = HashSet::new();
        let mut budget = CACHE_BYTES;
        let keys: Vec<_> = keys
            .into_iter()
            .filter(|k| {
                if !seen.insert(k.clone()) || k.cost() > budget {
                    return false;
                }
                budget -= k.cost();
                true
            })
            .take(MAX_VISIBLE)
            .collect();
        if self.visible == keys {
            return;
        }
        self.visible_set = keys.iter().cloned().collect();
        self.visible = keys;
        self.updated = Some(Instant::now());
        // Drop work that scrolled away; another worker picks up what is visible.
        for (key, cancel) in self.active.iter_mut().flatten() {
            if !self.visible_set.contains(key) {
                cancel.store(true, Ordering::Relaxed);
            }
        }
        self.pending = self
            .visible
            .iter()
            .filter(|k| {
                !self.cache.contains_key(*k)
                    && !self
                        .active
                        .iter()
                        .flatten()
                        .any(|(active, cancel)| active == *k && !cancel.load(Ordering::Relaxed))
            })
            .cloned()
            .collect();
    }
    /// Next job in visibility order. A Shell namespace item is only handed out
    /// when the device is idle, so local thumbnails keep flowing in parallel.
    fn take_job(&mut self) -> Option<Key> {
        let shell_busy = self.shell_busy;
        let position = self
            .pending
            .iter()
            .position(|key| !is_shell(key) || !shell_busy)?;
        let key = self.pending.remove(position)?;
        if is_shell(&key) {
            self.shell_busy = true;
        }
        Some(key)
    }
    fn finish_job(&mut self, index: usize, key: &Key) {
        if let Some(slot) = self.active.get_mut(index) {
            *slot = None;
        }
        if is_shell(key) {
            self.shell_busy = false;
        }
    }
    fn prune(&mut self) {
        let mut bytes: usize = self
            .cache
            .iter()
            .filter(|(_, value)| value.path.is_some())
            .map(|(key, _)| key.cost())
            .sum();
        while self.cache.len() > MAX_CACHE || bytes > CACHE_BYTES {
            let Some(key) = self
                .cache
                .iter()
                .filter(|(key, _)| !self.visible_set.contains(key))
                .min_by_key(|(_, value)| value.used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if self
                .cache
                .remove(&key)
                .is_some_and(|value| value.path.is_some())
            {
                bytes = bytes.saturating_sub(key.cost());
            }
        }
    }
}
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    changed: AtomicBool,
}
fn run_worker(shared: Arc<Shared>, index: usize) {
    let mut session = crate::native_assets::ThumbnailSession::default();
    loop {
        let (key, cancel) = {
            let mut state = shared.state.lock().unwrap();
            loop {
                if state.stopped {
                    return;
                }
                if state.pending.is_empty() {
                    if state.visible.is_empty() {
                        drop(state);
                        session.stop();
                        state = shared.state.lock().unwrap();
                        if state.stopped {
                            return;
                        }
                        if !state.pending.is_empty() {
                            continue;
                        }
                    }
                    let (next, timeout) = shared
                        .wake
                        .wait_timeout(state, Duration::from_secs(30))
                        .unwrap();
                    state = next;
                    if timeout.timed_out() {
                        drop(state);
                        session.stop();
                        state = shared.state.lock().unwrap();
                    }
                    continue;
                }
                if let Some(delay) = state
                    .updated
                    .and_then(|time| SETTLE.checked_sub(time.elapsed()))
                {
                    // The viewport just changed: wait for it to settle so work is
                    // not started for rows that already scrolled away.
                    state = shared.wake.wait_timeout(state, delay).unwrap().0;
                    continue;
                }
                let Some(job) = state.take_job() else {
                    // Only a portable device item is left and another worker owns
                    // the device. Wake up when it finishes.
                    state = shared
                        .wake
                        .wait_timeout(state, Duration::from_millis(100))
                        .unwrap()
                        .0;
                    continue;
                };
                let cancel = Arc::new(AtomicBool::new(false));
                state.active[index] = Some((job.clone(), cancel.clone()));
                break (job, cancel);
            }
        };
        // A slow codec/provider is killed with the helper, never joined by the UI.
        let asset = session.get(&key, &cancel).ok();
        let presentation = asset
            .as_ref()
            .map(|asset| asset.presentation)
            .unwrap_or_else(|| {
                let extension = key
                    .path
                    .extension()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_ascii_lowercase();
                Presentation {
                    directory: key.directory,
                    width: 0,
                    height: 0,
                    source_width: 0,
                    source_height: 0,
                    shortcut: matches!(extension.as_str(), "lnk" | "url"),
                    video: matches!(
                        extension.as_str(),
                        "mp4" | "m4v" | "webm" | "mov" | "mkv" | "avi" | "wmv"
                    ),
                }
            });
        let image = asset.map(|asset| asset.image);
        let mut state = shared.state.lock().unwrap();
        state.finish_job(index, &key);
        if is_shell(&key) {
            // Hand the device to whichever worker has the next portable item.
            shared.wake.notify_all();
        }
        if !cancel.load(Ordering::Relaxed) {
            // A failing portable device is usually busy or reconnecting, so its
            // failure is not remembered: the next viewport update retries it.
            // Local failures are deterministic and stay cached as no-image.
            if image.is_none() && is_shell(&key) {
                continue;
            }
            state.clock += 1;
            let used = state.clock;
            let changed = image.is_some();
            state.cache.insert(
                key,
                Cached {
                    path: image,
                    presentation,
                    used,
                },
            );
            state.prune();
            if changed {
                shared.changed.store(true, Ordering::Release);
            }
        }
    }
}
pub struct Loader {
    shared: Arc<Shared>,
}
impl Loader {
    pub fn new() -> Self {
        let workers = worker_count();
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                active: (0..workers).map(|_| None).collect(),
                ..State::default()
            }),
            wake: Condvar::new(),
            changed: AtomicBool::new(false),
        });
        for index in 0..workers {
            let worker = shared.clone();
            std::thread::Builder::new()
                .name(format!("visible-thumbnail-{index}"))
                .spawn(move || run_worker(worker, index))
                .expect("thumbnail scheduler");
        }
        Self { shared }
    }
    pub fn set_visible(&self, keys: Vec<Key>) {
        self.shared.state.lock().unwrap().set_visible(keys);
        // Every worker must re-read the queue: waking one would leave the others
        // asleep while the new viewport waits for previews.
        self.shared.wake.notify_all();
    }
    pub fn image(&self, key: &Key) -> Option<PathBuf> {
        let mut state = self.shared.state.lock().unwrap();
        state.clock += 1;
        let used = state.clock;
        let lookup = if state.cache.contains_key(key) {
            key.clone()
        } else {
            state
                .cache
                .keys()
                .filter(|old| {
                    old.path == key.path
                        && old.size == key.size
                        && old.modified == key.modified
                        && old.pixels == key.pixels
                })
                .max_by_key(|old| old.shortcut_epoch)
                .cloned()?
        };
        let cached = state.cache.get_mut(&lookup)?;
        cached.used = used;
        cached.path.clone()
    }
    pub fn changed(&self) -> bool {
        self.shared.changed.swap(false, Ordering::AcqRel)
    }
    /// Cached decoration flags only: never resolve links or touch disk while rendering.
    pub fn presentation(&self, key: &Key) -> Option<Presentation> {
        let state = self.shared.state.lock().unwrap();
        state
            .cache
            .get(key)
            .or_else(|| {
                state
                    .cache
                    .iter()
                    .filter(|(old, _)| {
                        old.path == key.path
                            && old.size == key.size
                            && old.modified == key.modified
                            && old.pixels == key.pixels
                    })
                    .max_by_key(|(old, _)| old.shortcut_epoch)
                    .map(|(_, value)| value)
            })
            .map(|value| value.presentation)
    }
}
impl Drop for Loader {
    fn drop(&mut self) {
        let mut state = self.shared.state.lock().unwrap();
        state.stopped = true;
        for (_, cancel) in state.active.iter_mut().flatten() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.shared.wake.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(n: usize) -> Key {
        Key {
            path: PathBuf::from(format!("D:\\photo{n}.jpg")),
            size: 10,
            modified: None,
            pixels: 64,
            directory: false,
            shortcut_epoch: 0,
        }
    }
    fn device_key(n: usize) -> Key {
        Key {
            path: PathBuf::from(format!("mtp:0000{n}")),
            size: 10,
            modified: None,
            pixels: 64,
            directory: false,
            shortcut_epoch: 0,
        }
    }
    #[test]
    fn worker_pool_is_capped_below_the_core_count() {
        let cores = std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(1);
        assert_eq!(
            worker_count(),
            cores.saturating_sub(2).clamp(1, MAX_WORKERS)
        );
        assert!(worker_count() <= MAX_WORKERS);
    }
    #[test]
    fn portable_items_are_handed_out_one_at_a_time() {
        let mut state = State::default();
        state.pending = [device_key(1), key(2), device_key(3)].into();
        let first = state.take_job().unwrap();
        assert!(is_shell(&first));
        // The device is busy: the next portable item waits, local work flows.
        let second = state.take_job().unwrap();
        assert!(!is_shell(&second));
        assert_eq!(second.path, key(2).path);
        assert!(state.take_job().is_none());
        state.finish_job(0, &first);
        let third = state.take_job().unwrap();
        assert_eq!(third.path, device_key(3).path);
    }
    #[test]
    fn folders_keep_a_stable_cache_key_while_shortcuts_get_a_time_bucket() {
        let entry = |path: &str, directory: bool| Entry {
            recycle: None,
            path: PathBuf::from(path),
            name: path.into(),
            directory,
            link: false,
            hidden: false,
            system: false,
            size: if directory { 0 } else { 1024 },
            total: None,
            free: None,
            modified: None,
            created: None,
            kind: String::new(),
        };
        let folder = Key::from_entry(&entry("C:\\Windows\\System32", true), 96).unwrap();
        assert_eq!(folder.shortcut_epoch, 0);
        // Same folder, same key: the cache survives window switches and restarts.
        assert_eq!(
            Key::from_entry(&entry("C:\\Windows\\System32", true), 96).unwrap(),
            folder
        );
        let shortcut = Key::from_entry(&entry("C:\\Windows\\photo.lnk", false), 96).unwrap();
        assert!(shortcut.shortcut_epoch > 0);
        let photo = Key::from_entry(&entry("C:\\Windows\\photo.jpg", false), 96).unwrap();
        assert_eq!(photo.shortcut_epoch, 0);
        // Portable-device folders are cached the same way.
        let device = Key::from_entry(&entry("mtp:00112233", true), 96).unwrap();
        assert_eq!(device.shortcut_epoch, 0);
    }
    #[test]
    fn internet_shortcuts_are_icon_only_in_the_thumbnail_views() {
        let entry = |path: &str| Entry {
            recycle: None,
            path: PathBuf::from(path),
            name: path.into(),
            directory: false,
            link: true,
            hidden: false,
            system: false,
            size: 223,
            total: None,
            free: None,
            modified: None,
            created: None,
            kind: String::new(),
        };
        // A `.url` has no thumbnail; the Shell's fallback is a generic page, so
        // medium/large/content views must use the icon pipeline instead.
        assert!(Key::from_entry(&entry("C:\\Users\\rei\\Desktop\\Site.url"), 96).is_none());
        // A `.lnk` still previews its target (photo/document shortcuts).
        assert!(Key::from_entry(&entry("C:\\Users\\rei\\Desktop\\Photo.lnk"), 96).is_some());
        assert!(Key::from_entry(&entry("C:\\Users\\rei\\Desktop\\photo.jpg"), 96).is_some());
    }
    #[test]
    fn preview_deadlines_give_slow_previews_more_time() {
        let folder = Key {
            path: PathBuf::from("C:\\Windows\\System32"),
            directory: true,
            ..key(1)
        };
        assert!(crate::native_assets::preview_deadline(&folder) > Duration::from_millis(800));
        let device = device_key(1);
        assert!(
            crate::native_assets::preview_deadline(&device)
                > crate::native_assets::preview_deadline(&folder)
        );
        assert_eq!(
            crate::native_assets::preview_deadline(&key(1)),
            Duration::from_millis(800)
        );
    }
    #[test]
    fn viewport_replaces_stale_queue_and_cancels_offscreen_provider() {
        let mut state = State::default();
        state.active = vec![None, None];
        state.set_visible(vec![key(1), key(2), key(2)]);
        let cancel = Arc::new(AtomicBool::new(false));
        state.active[0] = Some((key(1), cancel.clone()));
        state.set_visible(vec![key(3)]);
        assert!(cancel.load(Ordering::Relaxed));
        assert_eq!(state.pending.into_iter().collect::<Vec<_>>(), vec![key(3)]);
    }
    #[test]
    fn cache_revision_and_resolution_do_not_alias() {
        let original = key(1);
        let mut edited = original.clone();
        edited.size += 1;
        let mut larger = original.clone();
        larger.pixels = 128;
        assert_ne!(original, edited);
        assert_ne!(original, larger);
        assert_eq!(resolution(50), 64);
        assert_eq!(resolution(400), 256);
        let mut shortcut = original.clone();
        shortcut.shortcut_epoch = 1;
        let mut refreshed = shortcut.clone();
        refreshed.shortcut_epoch = 2;
        assert_ne!(shortcut, refreshed);
    }
    #[test]
    fn pending_and_negative_cache_are_bounded() {
        let mut state = State::default();
        state.set_visible((0..2000).map(key).collect());
        assert_eq!(state.pending.len(), MAX_VISIBLE);
        for n in 0..1000 {
            state.cache.insert(
                key(n),
                Cached {
                    path: None,
                    presentation: Presentation::default(),
                    used: n as u64,
                },
            );
        }
        state.set_visible(vec![]);
        state.prune();
        assert_eq!(state.cache.len(), MAX_CACHE);
    }
    #[test]
    fn repeated_paints_do_not_postpone_decode_and_failures_are_not_requeued() {
        let mut state = State::default();
        state.set_visible(vec![key(1)]);
        let settled = state.updated;
        state.set_visible(vec![key(1)]);
        assert_eq!(state.updated, settled);
        state.cache.insert(
            key(1),
            Cached {
                path: None,
                presentation: Presentation::default(),
                used: 0,
            },
        );
        state.set_visible(vec![]);
        state.set_visible(vec![key(1)]);
        assert!(state.pending.is_empty());
    }
    #[test]
    fn visible_large_images_respect_pixel_budget() {
        let mut state = State::default();
        state.set_visible(
            (0..1000)
                .map(|n| {
                    let mut key = key(n);
                    key.pixels = 256;
                    key
                })
                .collect(),
        );
        assert!(state.visible.iter().map(Key::cost).sum::<usize>() <= CACHE_BYTES);
        assert_eq!(state.visible.len(), 128);
    }
}
