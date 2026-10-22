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
const SETTLE: Duration = Duration::from_millis(100);

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
        if (!entry.directory && entry.size == 0)
            || !entry.path.is_absolute()
            || entry.path.to_string_lossy().starts_with("\\\\")
            || (!entry.directory
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
                        | "url"
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
            shortcut_epoch: if entry.directory || matches!(extension.as_str(), "lnk" | "url") {
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
    active: Option<(Key, Arc<AtomicBool>)>,
    updated: Option<Instant>,
    clock: u64,
    stopped: bool,
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
        if let Some((key, cancel)) = &self.active {
            if !self.visible_set.contains(key) {
                cancel.store(true, Ordering::Relaxed);
            }
        }
        self.pending = self
            .visible
            .iter()
            .filter(|k| {
                !self.cache.contains_key(*k)
                    && !self.active.as_ref().is_some_and(|(active, cancel)| {
                        active == *k && !cancel.load(Ordering::Relaxed)
                    })
            })
            .cloned()
            .collect();
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
pub struct Loader {
    shared: Arc<Shared>,
}
impl Loader {
    pub fn new() -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
            changed: AtomicBool::new(false),
        });
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("visible-thumbnails".into())
            .spawn(move || {
                let mut session = crate::native_assets::ThumbnailSession::default();
                loop {
                    let (key, cancel) = {
                        let mut state = worker.state.lock().unwrap();
                        loop {
                            if state.stopped {
                                return;
                            }
                            if state.pending.is_empty() {
                                if state.visible.is_empty() {
                                    drop(state);
                                    session.stop();
                                    state = worker.state.lock().unwrap();
                                    if state.stopped {
                                        return;
                                    }
                                    if !state.pending.is_empty() {
                                        continue;
                                    }
                                }
                                let (next, timeout) = worker
                                    .wake
                                    .wait_timeout(state, Duration::from_secs(30))
                                    .unwrap();
                                state = next;
                                if timeout.timed_out() {
                                    drop(state);
                                    session.stop();
                                    state = worker.state.lock().unwrap();
                                }
                            } else if let Some(delay) = state
                                .updated
                                .and_then(|time| SETTLE.checked_sub(time.elapsed()))
                            {
                                state = worker.wake.wait_timeout(state, delay).unwrap().0;
                            } else {
                                let key = state.pending.pop_front().unwrap();
                                let cancel = Arc::new(AtomicBool::new(false));
                                state.active = Some((key.clone(), cancel.clone()));
                                break (key, cancel);
                            }
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
                    let mut state = worker.state.lock().unwrap();
                    state.active = None;
                    if !cancel.load(Ordering::Relaxed) {
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
                            worker.changed.store(true, Ordering::Release);
                        }
                    }
                }
            })
            .expect("thumbnail scheduler");
        Self { shared }
    }
    pub fn set_visible(&self, keys: Vec<Key>) {
        self.shared.state.lock().unwrap().set_visible(keys);
        self.shared.wake.notify_one();
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
        if let Some((_, cancel)) = &state.active {
            cancel.store(true, Ordering::Relaxed);
        }
        self.shared.wake.notify_one();
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
    #[test]
    fn viewport_replaces_stale_queue_and_cancels_offscreen_provider() {
        let mut state = State::default();
        state.set_visible(vec![key(1), key(2), key(2)]);
        let cancel = Arc::new(AtomicBool::new(false));
        state.active = Some((key(1), cancel.clone()));
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
