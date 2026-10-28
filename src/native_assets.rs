//! Native providers run in a bounded helper process; the UI receives PNG bytes
//! and SQLite is the only image cache.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

/// Bumped when this process restores a network connection. A disconnected mapping
/// keeps its drive letter and the Shell draws a red X over its icon, so the cached
/// icon has to be asked for again once the connection is back.
static NETWORK_ICON_EPOCH: AtomicU64 = AtomicU64::new(0);
/// Bumped by "refresh" in This PC: capacity bars and connection overlays on the
/// drive tiles come from the Shell, not from the path.
static DRIVE_ICON_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Process start in seconds. A drive icon carries status badges (BitLocker,
/// warning, capacity) that the path does not describe, so it is re-read once per
/// launch instead of being reused forever under one stable key.
static DRIVE_ICON_SESSION: std::sync::OnceLock<u64> = std::sync::OnceLock::new();

fn session_epoch() -> u64 {
    *DRIVE_ICON_SESSION.get_or_init(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    })
}

/// Make every network root ask the Shell for its icon again.
pub fn refresh_network_icons() {
    NETWORK_ICON_EPOCH.fetch_add(1, Ordering::Relaxed);
}

/// Make every drive root ask the Shell for its icon again.
pub fn refresh_drive_icons() {
    DRIVE_ICON_EPOCH.fetch_add(1, Ordering::Relaxed);
}

#[derive(Clone, Debug, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct IconRequest {
    pub name: String,
    pub path: PathBuf,
    pub directory: bool,
    pub pixels: u32,
    #[serde(default)]
    pub epoch: u64,
}
/// Upper bound for the background icon queue of one folder. Every entry still gets
/// its icon when it becomes visible; this only pre-resolves the first N real icons.
pub const ICON_PREFETCH_LIMIT: usize = 3000;
impl IconRequest {
    /// Whether this item resolves an icon of its own (an executable, a shortcut, a
    /// drive root) instead of a shared type icon. Used to queue a folder's real icons
    /// ahead of scrolling.
    pub fn needs_own_icon(path: &Path, directory: bool) -> bool {
        if path.to_string_lossy().starts_with("mtp-root:") {
            return true;
        }
        if directory {
            return path.has_root() && path.parent().is_none();
        }
        path.is_absolute()
            && matches!(
                path.extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase()
                    .as_str(),
                "exe" | "lnk" | "url" | "ico" | "cur"
            )
    }
    pub fn ui(name: &str, pixels: u32) -> Self {
        Self {
            name: name.into(),
            path: PathBuf::new(),
            directory: false,
            pixels,
            epoch: crate::fluent_icons::resource_epoch().wrapping_add(0x20261022),
        }
    }
    pub fn file(path: &Path, directory: bool, pixels: u32) -> Self {
        // Shell class icons are shared by extension; avoid per-file extension
        // handlers. Portable-device child folders/files use generic stock icons
        // so a phone folder does not issue one slow Shell icon request each.
        let shell_root = path.to_string_lossy().starts_with("mtp-root:");
        let drive_root = directory && path.has_root() && path.parent().is_none();
        // Only folders that declare their own artwork (`C:\Windows\Offline Web
        // Pages` and other shell folders) need a per-path Shell lookup. Every other
        // folder shares one cached stock icon, so dragging through a folder such as
        // WinSxS never waits for the icon helper. The check runs once per folder and
        // is queued to bounded background workers and remembered for this process.
        let custom_folder =
            directory && !drive_root && path.is_absolute() && folder_has_own_artwork(path);
        let per_item = shell_root
            || custom_folder
            || drive_root
            || (path.is_absolute()
                && matches!(
                    path.extension()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_ascii_lowercase()
                        .as_str(),
                    "exe" | "lnk" | "url" | "ico" | "cur"
                ));
        let key = if per_item {
            path.to_path_buf()
        } else if directory {
            PathBuf::from("folder")
        } else {
            PathBuf::from(format!(
                "file.{}",
                path.extension()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_lowercase()
            ))
        };
        Self {
            name: "file".into(),
            path: key,
            directory,
            pixels,
            // A drive icon is re-read once per process launch: its status badges
            // (BitLocker, warning, connection) are Shell state that the path does
            // not describe, and a stale badge used to survive every restart.
            // Network roots additionally re-read on a slow bucket because they can
            // reconnect while the app runs, and "refresh" in This PC bumps them all
            // immediately.
            epoch: if shell_root || custom_folder {
                icon_epoch(30)
            } else if drive_root {
                let base = if crate::network::is_network_root(path) {
                    icon_epoch(60).wrapping_add(session_epoch())
                } else {
                    session_epoch()
                };
                base.wrapping_add(NETWORK_ICON_EPOCH.load(Ordering::Relaxed))
                    .wrapping_add(DRIVE_ICON_EPOCH.load(Ordering::Relaxed))
            } else {
                0
            },
        }
    }
}

/// Time bucket for icons whose artwork can change while the application runs.
fn icon_epoch(period: u64) -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / period
}
/// The already generated icon for this request, if any. Read from disk without a
/// helper round trip so a restarted application paints cached icons immediately.
pub fn cached_icon(key: &IconRequest) -> Option<Vec<u8>> {
    crate::thumbnail_store::load_icon(&icon_key(key))
        .ok()
        .flatten()
}
/// Rendering only consults memory and queues a bounded background probe. A slow
/// disk/network provider must never turn a desktop.ini lookup into a GUI stall.
fn folder_has_own_artwork(path: &Path) -> bool {
    FOLDER_ARTWORK.with(|cache| cache.borrow_mut().get(path))
}
thread_local! {
    static FOLDER_ARTWORK: std::cell::RefCell<FolderArtwork> =
        std::cell::RefCell::new(FolderArtwork::new(crate::system_icons::folder_has_custom_icon));
}
struct FolderArtwork {
    cache: std::collections::HashMap<PathBuf, bool>,
    pending: std::collections::HashSet<PathBuf>,
    requests: std::sync::mpsc::SyncSender<(u64, PathBuf)>,
    replies: std::sync::mpsc::Receiver<(u64, PathBuf, bool)>,
    generation: u64,
}
impl FolderArtwork {
    fn new(probe: fn(&Path) -> bool) -> Self {
        use std::sync::{Mutex, mpsc};
        let (requests, rx) = mpsc::sync_channel::<(u64, PathBuf)>(256);
        let (tx, replies) = mpsc::channel();
        let rx = Arc::new(Mutex::new(rx));
        for index in 0..2 {
            let rx = rx.clone();
            let tx = tx.clone();
            std::thread::Builder::new()
                .name(format!("folder-artwork-{index}"))
                .spawn(move || {
                    loop {
                        let request = rx.lock().unwrap().recv();
                        let Ok((generation, path)) = request else {
                            break;
                        };
                        let found = probe(&path);
                        if tx.send((generation, path, found)).is_err() {
                            break;
                        }
                    }
                })
                .expect("folder artwork worker");
        }
        Self {
            cache: Default::default(),
            pending: Default::default(),
            requests,
            replies,
            generation: 0,
        }
    }
    fn get(&mut self, path: &Path) -> bool {
        if let Some(found) = self.cache.get(path) {
            return *found;
        }
        if self.pending.len() < 256
            && !self.pending.contains(path)
            && self
                .requests
                .try_send((self.generation, path.to_path_buf()))
                .is_ok()
        {
            self.pending.insert(path.to_path_buf());
        }
        false
    }
    fn drain(&mut self) -> bool {
        let mut changed = false;
        for (generation, path, found) in self.replies.try_iter() {
            if generation != self.generation {
                continue;
            }
            self.pending.remove(&path);
            if self.cache.len() >= 32768 {
                self.cache.clear();
            }
            self.cache.insert(path, found);
            changed |= found;
        }
        changed
    }
    fn clear(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.cache.clear();
        self.pending.clear();
    }
}
pub fn poll_folder_artwork() -> bool {
    FOLDER_ARTWORK.with(|cache| cache.borrow_mut().drain())
}
pub fn clear_folder_artwork() {
    FOLDER_ARTWORK.with(|cache| cache.borrow_mut().clear());
}
#[derive(Clone, Copy, Serialize, Deserialize, Hash)]
pub enum PreviewKind {
    Video,
    Pdf,
    Font,
}
#[derive(Serialize, Deserialize)]
enum Request {
    ResolveShortcut {
        path: PathBuf,
    },
    Thumbnail {
        item: crate::thumbnails::Key,
    },
    Preview {
        path: PathBuf,
        kind: PreviewKind,
        page: u32,
        pixels: u32,
    },
    Icons {
        items: Vec<IconRequest>,
    },
}
#[derive(Serialize, Deserialize)]
pub struct PreviewAsset {
    pub image: Vec<u8>,
    pub pages: u32,
    pub elapsed_ms: u128,
}
/// One cached thumbnail: the content-addressed PNG plus the decorations that
/// belong to this *item*. A link badge or a photo resolution is not part of the
/// image, so entries sharing one PNG still keep their own presentation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThumbnailAsset {
    pub image: Vec<u8>,
    pub presentation: crate::thumbnails::Presentation,
}
#[derive(Serialize, Deserialize)]
enum Response {
    ShortcutTarget(PathBuf),
    Thumbnail(ThumbnailAsset),
    ThumbnailError(String),
    Preview(PreviewAsset),
    Icons(Vec<Option<Vec<u8>>>),
}

/// Legacy PNG/JSON directory names. They are only used when clearing an old
/// installation; new images are stored exclusively in SQLite.
const THUMBNAIL_DIR: &str = "thumbnails-v7";
const LEGACY_THUMBNAIL_DIR: &str = "thumbnails-v6";
const ICON_DIR: &str = "icons-v2";

fn cache_root() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("RustExplorer/cache/native-v6-r5")
}
/// Remove only this application's generated native PNGs; never follow links.
pub fn clear_disk_cache() -> Result<(usize, usize)> {
    crate::thumbnail_store::clear()?;
    clear_cache_at(&cache_root())
}
/// One-time upgrade cleanup for the old PNG/JSON cache. The new build stores
/// every image in SQLite, so these files are never written again.
pub fn cleanup_legacy_disk_cache() {
    static ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        let _ = clear_cache_at(&cache_root());
    });
}
fn clear_cache_at(root: &Path) -> Result<(usize, usize)> {
    use std::os::windows::fs::MetadataExt;
    let metadata = match std::fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((0, 0)),
        Err(error) => return Err(error.into()),
    };
    anyhow::ensure!(
        metadata.is_dir() && metadata.file_attributes() & 0x400 == 0,
        "缓存目录是链接，未进行清理"
    );
    let mut removed = 0;
    let mut failed = 0;
    for folder in [
        root.to_path_buf(),
        root.join(THUMBNAIL_DIR),
        root.join(LEGACY_THUMBNAIL_DIR),
        root.join(ICON_DIR),
    ] {
        let _ = std::fs::remove_file(folder.join("last-maintenance"));
        let Ok(metadata) = std::fs::symlink_metadata(&folder) else {
            continue;
        };
        if metadata.file_attributes() & 0x400 != 0 || !metadata.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(&folder)? {
            let Ok(entry) = entry else {
                failed += 1;
                continue;
            };
            let Ok(metadata) = entry.file_type() else {
                failed += 1;
                continue;
            };
            if !metadata.is_file() || metadata.is_symlink() {
                continue;
            }
            if !matches!(
                entry.path().extension().and_then(|s| s.to_str()),
                Some("png" | "json" | "tmp")
            ) {
                continue;
            }
            match std::fs::remove_file(entry.path()) {
                Ok(()) => removed += 1,
                Err(_) => failed += 1,
            }
        }
    }
    Ok((removed, failed))
}
/// Stable key for an icon's PNG in the shared SQLite cache.
fn icon_key(item: &IconRequest) -> String {
    sha256_hex(&serde_json::to_vec(item).unwrap_or_default())
}

/// Stable per-item key for a thumbnail.
fn thumbnail_index_path(key: &crate::thumbnails::Key) -> Result<PathBuf> {
    // Include the cache schema/provider revision in a stable SHA-256 key.
    let bytes = serde_json::to_vec(&(9u32, key))?;
    Ok(crate::thumbnail_store::directory().join(format!("{}.json", sha256_hex(&bytes))))
}

fn preview_key(
    path: &Path,
    kind: PreviewKind,
    page: u32,
    pixels: u32,
    size: u64,
    modified: Option<std::time::SystemTime>,
) -> Result<String> {
    Ok(sha256_hex(&serde_json::to_vec(&(
        2u32, path, kind, page, pixels, size, modified,
    ))?))
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut text = String::with_capacity(64);
    for byte in digest {
        text.push(HEX[(byte >> 4) as usize] as char);
        text.push(HEX[(byte & 0x0f) as usize] as char);
    }
    text
}

fn load_thumbnail(index: &Path) -> Option<ThumbnailAsset> {
    let (image, presentation) = crate::thumbnail_store::load_thumbnail(index).ok()??;
    Some(ThumbnailAsset {
        image,
        presentation,
    })
}
fn missing_thumbnail(index: &Path) -> bool {
    crate::thumbnail_store::missing(index)
}
pub fn remember_missing_thumbnail(key: &crate::thumbnails::Key) {
    if let Ok(index) = thumbnail_index_path(key) {
        crate::thumbnail_store::remember_missing(&index);
    }
}
fn save_thumbnail_index(index: &Path, asset: &ThumbnailAsset) -> Result<()> {
    // Cache failures never prevent a freshly generated image from being displayed.
    let _ = crate::thumbnail_store::save_thumbnail(index, &asset.image, &asset.presentation);
    Ok(())
}

/// Folders with their own `desktop.ini` artwork need a per-path icon (7-Zip shows
/// those too, e.g. `C:\Windows\Offline Web Pages`); every other folder shares the
/// one attribute-based folder icon instead of caching a PNG per folder.
///
/// Drive roots (and UNC share roots) are never shared: their icon carries the
/// drive type, BitLocker/optical/network overlays and the drive letter label.
fn share_plain_folder_icon(mut item: IconRequest) -> IconRequest {
    let root = item.path.has_root() && item.path.parent().is_none();
    if item.name == "file"
        && item.directory
        && item.path.is_absolute()
        && !root
        && !crate::shell_folder::is_shell_path(&item.path)
        && !crate::system_icons::folder_has_custom_icon(&item.path)
    {
        item.path = PathBuf::from("folder");
        item.epoch = 0;
    }
    item
}

/// Encode an image into PNG bytes. The bytes are persisted directly to SQLite;
/// no temporary or content-addressed image file is created.
fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    anyhow::ensure!(
        width > 0 && height > 0 && width <= 4096 && height <= 4096,
        "原生图像尺寸无效"
    );
    use image::{ImageEncoder, codecs::png::PngEncoder};
    let mut bytes = Vec::new();
    PngEncoder::new(&mut bytes).write_image(
        rgba,
        width,
        height,
        image::ExtendedColorType::Rgba8,
    )?;
    Ok(bytes)
}
pub fn serve(json: &str) -> Result<()> {
    let request: Request = serde_json::from_str(json)?;
    let response = match request {
        Request::ResolveShortcut { path } => {
            use std::os::windows::{ffi::OsStrExt, fs::MetadataExt};
            use windows::{Win32::Storage::FileSystem::GetDriveTypeW, core::PCWSTR};
            anyhow::ensure!(
                path.is_absolute() && !path.to_string_lossy().starts_with("\\\\"),
                "Only local shortcuts can be previewed"
            );
            let validate = |candidate: &Path| -> Result<()> {
                let root: Vec<u16> = candidate
                    .ancestors()
                    .last()
                    .unwrap()
                    .as_os_str()
                    .encode_wide()
                    .chain(Some(0))
                    .collect();
                anyhow::ensure!(
                    unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) } != 4,
                    "Mapped network shortcuts are not read automatically"
                );
                let metadata = std::fs::metadata(candidate)?;
                anyhow::ensure!(
                    metadata.file_attributes() & (0x1000 | 0x40000 | 0x400000) == 0,
                    "Offline shortcut targets are not downloaded automatically"
                );
                Ok(())
            };
            validate(&path)?;
            let target = crate::native_preview::shortcut_target(&path)?;
            validate(&target)?;
            anyhow::ensure!(
                !std::fs::metadata(&target)?.is_dir(),
                "Folder shortcut does not have a document preview"
            );
            Response::ShortcutTarget(target)
        }
        Request::Thumbnail { item } => {
            use std::os::windows::{ffi::OsStrExt, fs::MetadataExt};
            use windows::{Win32::Storage::FileSystem::GetDriveTypeW, core::PCWSTR};
            // Portable-device and recycled items have no filesystem path to stat;
            // their listing already carries the size and mtime used as the cache
            // identity, so they are cached exactly like local files.
            let namespace = crate::shell_folder::is_namespace_item(&item.path);
            let mut shortcut = false;
            let mut resolved = item.clone();
            if !namespace {
                anyhow::ensure!(
                    item.path.is_absolute() && !item.path.to_string_lossy().starts_with("\\\\"),
                    "不自动读取网络缩略图"
                );
                let root: Vec<u16> = item
                    .path
                    .ancestors()
                    .last()
                    .unwrap()
                    .as_os_str()
                    .encode_wide()
                    .chain(Some(0))
                    .collect();
                anyhow::ensure!(
                    unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) } != 4,
                    "不自动读取网络驱动器缩略图"
                );
                // `item` is built from `symlink_metadata`; keep the identity check on the
                // same source. Following a reparse point here reports the junction
                // target's mtime, which never matches the link's own metadata.
                let metadata = std::fs::symlink_metadata(&item.path)?;
                anyhow::ensure!(
                    metadata.file_attributes() & (0x1000 | 0x40000 | 0x400000) == 0,
                    "不自动下载脱机或云占位文件"
                );
                let directory = metadata.file_attributes() & 0x10 != 0;
                anyhow::ensure!(
                    directory == item.directory
                        && (if directory { 0 } else { metadata.len() }) == item.size
                        && metadata.modified().ok() == item.modified,
                    "文件已变化"
                );
                let extension = item
                    .path
                    .extension()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_ascii_lowercase();
                shortcut = !directory && matches!(extension.as_str(), "lnk" | "url");
                // A `.lnk` points at a file whose own artwork is what Explorer shows. A
                // `.url` usually points at a web page: resolving it would leave us with a
                // URL and no icon, so the Shell is asked for the shortcut file's own icon
                // (the site favicon) instead.
                let target = if extension == "lnk" {
                    crate::native_preview::shortcut_target(&item.path)?
                } else {
                    item.path.clone()
                };
                let target_root: Vec<u16> = target
                    .ancestors()
                    .last()
                    .unwrap()
                    .as_os_str()
                    .encode_wide()
                    .chain(Some(0))
                    .collect();
                anyhow::ensure!(
                    unsafe { GetDriveTypeW(PCWSTR(target_root.as_ptr())) } != 4,
                    "Do not thumbnail mapped network shortcut targets"
                );
                let target_metadata = std::fs::metadata(&target)?;
                anyhow::ensure!(
                    target_metadata.file_attributes() & (0x1000 | 0x40000 | 0x400000) == 0,
                    "Do not hydrate offline shortcut targets"
                );
                resolved = crate::thumbnails::Key {
                    path: target,
                    size: if target_metadata.is_dir() {
                        0
                    } else {
                        target_metadata.len()
                    },
                    directory: target_metadata.is_dir(),
                    modified: target_metadata.modified().ok(),
                    pixels: item.pixels,
                    shortcut_epoch: item.shortcut_epoch,
                };
            }
            let index = thumbnail_index_path(&item)?;
            if let Some(asset) = load_thumbnail(&index) {
                Response::Thumbnail(asset)
            } else {
                let pixels = item.pixels.clamp(32, 256);
                let extension = resolved
                    .path
                    .extension()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_ascii_lowercase();
                let bitmap = if namespace {
                    // A portable device or recycled item: ask the Shell for the
                    // item's own artwork through its ID list.
                    crate::native_preview::shell_item_thumbnail(
                        &item.path,
                        pixels,
                        resolved.directory,
                    )?
                } else if resolved.directory {
                    // Let Windows compose the folder's registered thumbnail. Do not
                    // enumerate children or extract a representative file ourselves.
                    crate::native_preview::folder_thumbnail(&resolved.path, pixels)?
                } else if extension == "pdf" {
                    anyhow::ensure!(
                        resolved.size <= 128 * 1024 * 1024,
                        "PDF 超过自动缩略图读取限制"
                    );
                    crate::native_preview::pdf_page(&resolved.path, 0, pixels)?.0
                } else if matches!(extension.as_str(), "ttf" | "otf" | "ttc" | "fon") {
                    crate::native_preview::font_sample(&resolved.path, pixels)?
                } else if matches!(
                    extension.as_str(),
                    "jpg"
                        | "jpeg"
                        | "png"
                        | "gif"
                        | "bmp"
                        | "tif"
                        | "tiff"
                        | "webp"
                        | "heic"
                        | "heif"
                        | "avif"
                ) {
                    anyhow::ensure!(
                        resolved.size <= 128 * 1024 * 1024,
                        "照片超过自动缩略图读取限制"
                    );
                    crate::native_preview::photo_thumbnail(&resolved.path, pixels)?
                } else {
                    // A shortcut to an app or document has no thumbnail of its own,
                    // but the Shell can still hand back its icon — which is what
                    // Explorer shows. Everything else stays on the video whitelist.
                    anyhow::ensure!(
                        shortcut
                            || matches!(
                                extension.as_str(),
                                "mp4" | "m4v" | "webm" | "mov" | "mkv" | "avi" | "wmv"
                            ),
                        "文件不支持缩略图"
                    );
                    crate::native_preview::file_thumbnail(&resolved.path, pixels)?
                };
                // No badge is painted into the bitmap: the view draws one link
                // arrow per tile, so baking a second one here produced two arrows.
                let image = encode_png(bitmap.width, bitmap.height, &bitmap.rgba)?;
                let (source_width, source_height) = if !namespace
                    && !resolved.directory
                    && matches!(
                        extension.as_str(),
                        "jpg"
                            | "jpeg"
                            | "png"
                            | "gif"
                            | "bmp"
                            | "tif"
                            | "tiff"
                            | "webp"
                            | "heic"
                            | "heif"
                            | "avif"
                    ) {
                    crate::native_preview::photo_dimensions(&resolved.path).unwrap_or_default()
                } else {
                    (0, 0)
                };
                let asset = ThumbnailAsset {
                    image,
                    presentation: crate::thumbnails::Presentation {
                        directory: resolved.directory,
                        shortcut,
                        source_width,
                        source_height,
                        width: bitmap.width,
                        height: bitmap.height,
                        video: !resolved.directory
                            && matches!(
                                extension.as_str(),
                                "mp4" | "m4v" | "webm" | "mov" | "mkv" | "avi" | "wmv"
                            ),
                    },
                };
                save_thumbnail_index(&index, &asset)?;
                Response::Thumbnail(asset)
            }
        }
        Request::Preview {
            path,
            kind,
            page,
            pixels,
        } => {
            let start = Instant::now();
            let metadata = std::fs::metadata(&path)?;
            let key = preview_key(
                &path,
                kind,
                page,
                pixels,
                metadata.len(),
                metadata.modified().ok(),
            )?;
            if let Some((image, pages)) = crate::thumbnail_store::load_preview(&key).ok().flatten()
            {
                Response::Preview(PreviewAsset {
                    image,
                    pages,
                    elapsed_ms: 0,
                })
            } else {
                let (width, height, rgba, pages) = match kind {
                    PreviewKind::Video => {
                        let b = crate::native_preview::video_thumbnail(&path, pixels)?;
                        (b.width, b.height, b.rgba, 1)
                    }
                    PreviewKind::Pdf => {
                        let (b, pages) = crate::native_preview::pdf_page(&path, page, pixels)?;
                        (b.width, b.height, b.rgba, pages)
                    }
                    PreviewKind::Font => {
                        let b = crate::native_preview::font_sample(&path, pixels)?;
                        (b.width, b.height, b.rgba, 1)
                    }
                };
                let image = encode_png(width, height, &rgba)?;
                let _ = crate::thumbnail_store::save_preview(&key, &image, pages);
                Response::Preview(PreviewAsset {
                    image,
                    pages,
                    elapsed_ms: start.elapsed().as_millis(),
                })
            }
        }
        Request::Icons { items } => {
            let mut paths = Vec::with_capacity(items.len());
            let mut writes = Vec::new();
            for item in items {
                let item = share_plain_folder_icon(item);
                let key = icon_key(&item);
                if let Some(image) = crate::thumbnail_store::load_icon(&key).ok().flatten() {
                    paths.push(Some(image));
                    continue;
                }
                let result = (|| -> Result<Vec<u8>> {
                    let bitmap = if item.name == "associated-app" {
                        crate::system_icons::associated_app_icon(&item.path, item.pixels)?
                    } else if item.name == "file" {
                        crate::system_icons::file_icon(&item.path, item.directory, item.pixels)?
                    } else {
                        crate::system_icons::ui_icon(&item.name, item.pixels)?
                    };
                    encode_png(bitmap.width, bitmap.height, &bitmap.rgba)
                })();
                match result {
                    Ok(image) => {
                        writes.push((key, image.clone()));
                        paths.push(Some(image));
                    }
                    Err(_) => paths.push(None),
                }
            }
            // One transaction per helper batch instead of one commit per icon.
            let _ = crate::thumbnail_store::save_icons(&writes);
            Response::Icons(paths)
        }
    };
    println!("{}", serde_json::to_string(&response)?);
    Ok(())
}
fn call(request: Request, timeout: Duration, cancel: &Arc<AtomicBool>) -> Result<Response> {
    if cancel.load(Ordering::Relaxed) {
        bail!("请求已取消");
    }
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.arg("--native-asset")
        .arg(serde_json::to_string(&request)?)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x08000000);
    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take().context("原生图像输出不可用")?;
    let stderr = child.stderr.take().context("原生图像错误输出不可用")?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(96 * 1024 * 1024)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let errors = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr
            .take(16 * 1024)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let start = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) || start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            let _ = errors.join();
            bail!("原生缩略图不可用或获取超时");
        }
        if let Some(status) = child.try_wait()? {
            let output = reader
                .join()
                .map_err(|_| anyhow::anyhow!("读取图像响应失败"))??;
            let error = errors
                .join()
                .map_err(|_| anyhow::anyhow!("读取图像错误失败"))??;
            if !status.success() {
                bail!("{}", String::from_utf8_lossy(&error));
            }
            return serde_json::from_slice(&output).context("解析原生图像响应失败");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
pub fn preview(
    path: &Path,
    kind: PreviewKind,
    page: u32,
    pixels: u32,
    cancel: &Arc<AtomicBool>,
) -> Result<PreviewAsset> {
    if let Ok(metadata) = std::fs::metadata(path) {
        let key = preview_key(
            path,
            kind,
            page,
            pixels,
            metadata.len(),
            metadata.modified().ok(),
        )?;
        if let Some((image, pages)) = crate::thumbnail_store::load_preview(&key).ok().flatten() {
            return Ok(PreviewAsset {
                image,
                pages,
                elapsed_ms: 0,
            });
        }
    }
    let timeout = match kind {
        PreviewKind::Video => Duration::from_millis(1500),
        PreviewKind::Pdf => Duration::from_secs(4),
        PreviewKind::Font => Duration::from_millis(1800),
    };
    match call(
        Request::Preview {
            path: path.into(),
            kind,
            page,
            pixels,
        },
        timeout,
        cancel,
    )? {
        Response::Preview(asset) => Ok(asset),
        _ => bail!("无效预览响应"),
    }
}
/// Background-only request; a slow or broken Shell link is killed with the
/// isolated helper. No shortcut target is opened or executed.
pub fn resolve_shortcut(path: &Path, cancel: &Arc<AtomicBool>) -> Result<PathBuf> {
    match call(
        Request::ResolveShortcut {
            path: path.to_path_buf(),
        },
        Duration::from_millis(800),
        cancel,
    )? {
        Response::ShortcutTarget(path) => Ok(path),
        _ => bail!("Invalid shortcut resolution response"),
    }
}
pub fn icons(items: Vec<IconRequest>) -> Vec<Option<Vec<u8>>> {
    // Called by icon workers, never by rendering: cached icons avoid a helper
    // round trip while SQLite lookups remain off the GUI thread.
    let mut result = vec![None; items.len()];
    let mut pending = Vec::new();
    let mut positions = Vec::new();
    for (index, item) in items.into_iter().enumerate() {
        if let Some(image) = cached_icon(&item) {
            result[index] = Some(image);
        } else {
            positions.push(index);
            pending.push(item);
        }
    }
    if !pending.is_empty() {
        for (index, image) in positions.into_iter().zip(load_icons(pending)) {
            result[index] = image;
        }
    }
    result
}
fn load_icons(items: Vec<IconRequest>) -> Vec<Option<Vec<u8>>> {
    if items.len() > 1
        && items
            .iter()
            .any(|item| item.directory && item.path.has_root() && item.path.parent().is_none())
    {
        // A stalled network/optical icon must not discard already available local
        // drive icons when the batch deadline expires.
        return std::thread::scope(|scope| {
            let tasks: Vec<_> = items
                .into_iter()
                .map(|item| scope.spawn(move || icons(vec![item]).into_iter().next().flatten()))
                .collect();
            tasks
                .into_iter()
                .map(|task| task.join().unwrap_or_default())
                .collect()
        });
    }
    let count = items.len();
    let cancel = Arc::new(AtomicBool::new(false));
    // Bigger batches mean fewer helper starts while scrolling a large folder; the
    // deadline grows with the batch so a slow Shell provider is not cut off.
    let deadline = Duration::from_millis(1200 + 60 * count.min(64) as u64);
    match call(Request::Icons { items }, deadline, &cancel) {
        Ok(Response::Icons(paths)) => paths,
        _ => vec![None; count],
    }
}

pub fn thumbnail(key: &crate::thumbnails::Key, cancel: &Arc<AtomicBool>) -> Result<ThumbnailAsset> {
    let index = thumbnail_index_path(key)?;
    if cancel.load(Ordering::Relaxed) {
        bail!("缩略图已取消");
    }
    // The index name already contains the whole key (including the shortcut
    // epoch), so reading it cannot return artwork for a different key.
    if let Some(asset) = load_thumbnail(&index) {
        crate::shell::menu_trace("thumbnail: db hit");
        return Ok(asset);
    }
    match call(
        Request::Thumbnail { item: key.clone() },
        preview_deadline(key),
        cancel,
    )? {
        Response::Thumbnail(asset) => Ok(asset),
        _ => bail!("无效缩略图响应"),
    }
}

/// How long one preview may take. Folder composition and portable devices are much
/// slower than a file thumbnail, and killing the helper mid-work costs another
/// process start plus a repeat of that work.
pub(crate) fn preview_deadline(key: &crate::thumbnails::Key) -> Duration {
    if crate::shell_folder::is_namespace_item(&key.path) {
        Duration::from_millis(4000)
    } else if key.directory {
        Duration::from_millis(3000)
    } else {
        Duration::from_millis(800)
    }
}

/// A private line protocol keeps native codec startup out of each image request.
pub fn thumbnail_server() -> Result<()> {
    use std::io::{BufRead, Write};
    let _runtime = crate::native_preview::thumbnail_runtime()?;
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        anyhow::ensure!(line.len() <= 16 * 1024 * 1024, "缩略图请求过大");
        if !matches!(
            serde_json::from_str::<Request>(&line),
            Ok(Request::Thumbnail { .. })
        ) {
            bail!("无效缩略图服务请求");
        }
        if let Err(error) = serve(&line) {
            println!(
                "{}",
                serde_json::json!({"ThumbnailError": error.to_string()})
            );
        }
        std::io::stdout().flush()?;
    }
    Ok(())
}

struct ThumbnailProcess {
    child: std::process::Child,
    requests: std::sync::mpsc::SyncSender<Vec<u8>>,
    replies: std::sync::mpsc::Receiver<String>,
}
#[derive(Default)]
pub struct ThumbnailSession {
    process: Option<ThumbnailProcess>,
}
impl ThumbnailSession {
    pub fn stop(&mut self) {
        if let Some(mut running) = self.process.take() {
            let _ = running.child.kill();
            let _ = running.child.wait();
        }
    }
    /// Start the codec helper before the first preview is needed. Process start
    /// costs ~80 ms, which otherwise lands on the first visible thumbnail.
    pub fn warm(&mut self) {
        let _ = self.ensure_process();
    }
    fn ensure_process(&mut self) -> Result<()> {
        use std::{io::BufRead, os::windows::process::CommandExt};
        if self.process.is_some() {
            return Ok(());
        }
        let mut child = Command::new(std::env::current_exe()?)
            .arg("--thumbnail-server")
            .creation_flags(0x08000000)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let mut input = child.stdin.take().context("缩略图服务输入不可用")?;
        let stdout = child.stdout.take().context("缩略图服务输出不可用")?;
        let (tx, replies) = std::sync::mpsc::sync_channel(1);
        let (requests, incoming) = std::sync::mpsc::sync_channel::<Vec<u8>>(1);
        std::thread::spawn(move || {
            use std::io::Write;
            while let Ok(bytes) = incoming.recv() {
                if input.write_all(&bytes).and_then(|_| input.flush()).is_err() {
                    break;
                }
            }
        });
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else {
                    break;
                };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        self.process = Some(ThumbnailProcess {
            child,
            requests,
            replies,
        });
        Ok(())
    }
    pub fn get(
        &mut self,
        key: &crate::thumbnails::Key,
        cancel: &Arc<AtomicBool>,
    ) -> Result<ThumbnailAsset> {
        let index = thumbnail_index_path(key)?;
        if cancel.load(Ordering::Relaxed) {
            bail!("缩略图已取消");
        }
        // The index name contains the whole key (epoch included), so a hit always
        // belongs to this key: no helper round trip for cached folders.
        if let Some(asset) = load_thumbnail(&index) {
            crate::shell::menu_trace("thumbnail: db hit");
            return Ok(asset);
        }
        if missing_thumbnail(&index) {
            crate::shell::menu_trace("thumbnail: cached miss");
            bail!("文件不支持缩略图");
        }
        crate::shell::menu_trace("thumbnail: generate");
        self.ensure_process()?;
        let result = (|| -> Result<ThumbnailAsset> {
            let process = self.process.as_mut().unwrap();
            let start = Instant::now();
            let deadline = preview_deadline(key);
            let mut bytes = serde_json::to_vec(&Request::Thumbnail { item: key.clone() })?;
            bytes.push(b'\n');
            process
                .requests
                .try_send(bytes)
                .context("缩略图服务请求未接收")?;
            loop {
                if cancel.load(Ordering::Relaxed) || start.elapsed() > deadline {
                    bail!("缩略图取消或超时");
                }
                match process.replies.recv_timeout(Duration::from_millis(10)) {
                    Ok(line) => match serde_json::from_str::<Response>(&line) {
                        Ok(Response::Thumbnail(asset)) => return Ok(asset),
                        Ok(Response::ThumbnailError(error)) => {
                            // Only a genuine unsupported-provider result is a
                            // negative cache entry. Disk/SQLite/codec errors may
                            // succeed on retry and must keep their real cause.
                            return Err(anyhow::anyhow!(error));
                        }
                        _ => bail!("无效缩略图响应"),
                    },
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        bail!("缩略图服务已退出")
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
        })();
        // Unsupported files leave the codec process reusable. A timeout, broken
        // pipe or cancellation tears it down so it cannot retain a stale reply.
        if result
            .as_ref()
            .is_err_and(|error| error.to_string() != "文件不支持缩略图")
        {
            self.stop();
        }
        result
    }
}
impl Drop for ThumbnailSession {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "re-cache-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }
    #[test]
    fn plain_folders_share_an_icon_while_desktop_ini_folders_keep_their_own() {
        let a = IconRequest::file(std::path::Path::new("C:\\Windows\\System32"), true, 96);
        let b = IconRequest::file(std::path::Path::new("D:\\Games"), true, 96);
        // Ordinary folders share one cached stock icon without asking the helper, so
        // dragging through a 16k-folder directory never waits for an icon round trip.
        assert_eq!(a.path, PathBuf::from("folder"));
        assert_eq!(a.path, b.path);
        assert_eq!(a.epoch, 0);

        // A folder with its own artwork keeps its path so the Shell can resolve it.
        let custom = temp_root("custom-folder");
        std::fs::write(
            custom.join("desktop.ini"),
            b"[.ShellClassInfo]\r\nIconResource=C:\\x.ico,0\r\n",
        )
        .unwrap();
        // The render-side constructor first returns a shared fallback. Background
        // classification later promotes a customized folder to its per-path icon.
        let deadline = Instant::now() + Duration::from_secs(2);
        let request = loop {
            poll_folder_artwork();
            let request = IconRequest::file(&custom, true, 96);
            if request.path == custom {
                break request;
            }
            assert!(
                Instant::now() < deadline,
                "custom icon probe did not complete"
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(request.path, custom);
        assert!(request.epoch > 0);
        assert_eq!(share_plain_folder_icon(request.clone()).path, request.path);
        std::fs::remove_dir_all(&custom).unwrap();

        // Executables keep a per-path icon and are never shared or normalized.
        let exe = IconRequest::file(std::path::Path::new("C:\\a\\app.exe"), false, 96);
        let other = IconRequest::file(std::path::Path::new("C:\\b\\other.exe"), false, 96);
        assert_ne!(exe.path, other.path);
        assert_eq!(share_plain_folder_icon(exe.clone()).path, exe.path);

        let drive = IconRequest::file(std::path::Path::new("C:\\"), true, 96);
        assert_eq!(drive.path, PathBuf::from("C:\\"));
        // A drive icon is re-read once per process launch (its badges are Shell
        // state, not path state), so the key is stable inside one run only.
        assert!(drive.epoch > 0);
        assert_eq!(
            IconRequest::file(std::path::Path::new("C:\\"), true, 96).epoch,
            drive.epoch
        );
        // Drive roots keep their own icon: it carries the drive type and overlays,
        // so it must never be replaced by the generic folder icon.
        assert_eq!(share_plain_folder_icon(drive.clone()).path, drive.path);
        assert_eq!(share_plain_folder_icon(drive.clone()).epoch, drive.epoch);
        let unc = IconRequest::file(std::path::Path::new("\\\\server\\share"), true, 96);
        assert_eq!(share_plain_folder_icon(unc.clone()).path, unc.path);
        // A network root draws a connection overlay, which changes without the
        // path changing: its key must not be frozen, or a mapping that was
        // reconnected keeps the cached red X.
        assert!(unc.epoch > 0);
        let before = unc.epoch;
        refresh_network_icons();
        let after = IconRequest::file(std::path::Path::new("\\\\server\\share"), true, 96).epoch;
        assert!(after > before);

        // "Refresh" in This PC re-reads every drive root, local ones included.
        let before = IconRequest::file(std::path::Path::new("C:\\"), true, 96).epoch;
        refresh_drive_icons();
        let after = IconRequest::file(std::path::Path::new("C:\\"), true, 96).epoch;
        assert!(after > before);

        let text = IconRequest::file(std::path::Path::new("C:\\a\\x.txt"), false, 96);
        let upper = IconRequest::file(std::path::Path::new("D:\\b\\y.TXT"), false, 96);
        assert_eq!(text.path, PathBuf::from("file.txt"));
        assert_eq!(text.path, upper.path);
    }
    #[test]
    fn slow_folder_artwork_probe_does_not_block_request_construction() {
        static RELEASE: AtomicBool = AtomicBool::new(false);
        static STARTED: AtomicBool = AtomicBool::new(false);
        fn blocked_probe(_: &Path) -> bool {
            STARTED.store(true, Ordering::SeqCst);
            while !RELEASE.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(1));
            }
            true
        }
        struct Release;
        impl Drop for Release {
            fn drop(&mut self) {
                RELEASE.store(true, Ordering::SeqCst);
            }
        }
        RELEASE.store(false, Ordering::SeqCst);
        STARTED.store(false, Ordering::SeqCst);
        let _release = Release;
        let mut cache = super::FolderArtwork::new(blocked_probe);
        let path = Path::new("C:\\blocked-artwork-fixture");
        assert!(!cache.get(path));
        let deadline = Instant::now() + Duration::from_secs(2);
        while !STARTED.load(Ordering::SeqCst) {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!cache.get(path));
        assert_eq!(cache.pending.len(), 1);
        RELEASE.store(true, Ordering::SeqCst);
        while !cache.cache.contains_key(path) {
            cache.drain();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(cache.get(path));
        cache.clear();
        assert!(cache.cache.is_empty());
    }
    #[test]
    fn encoded_images_are_png_bytes_without_a_filesystem_cache() {
        assert_eq!(
            super::sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let root = temp_root("encode");
        let rgba = vec![128u8; 4 * 4 * 4];
        let image = super::encode_png(4, 4, &rgba).unwrap();
        assert!(image.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(std::fs::read_dir(&root).unwrap().next().is_none());
        std::fs::remove_dir_all(&root).unwrap();
    }
    #[test]
    fn clearing_generated_cache_preserves_other_files_and_directories() {
        let root = temp_root("clear");
        std::fs::create_dir_all(root.join(super::THUMBNAIL_DIR)).unwrap();
        std::fs::create_dir_all(root.join(super::LEGACY_THUMBNAIL_DIR)).unwrap();
        std::fs::create_dir_all(root.join(super::ICON_DIR)).unwrap();
        std::fs::create_dir(root.join("other")).unwrap();
        for file in [
            "icon.png",
            "keep.txt",
            "thumbnails-v7/thumb.png",
            "thumbnails-v7/index.json",
            "thumbnails-v7/keep.txt",
            "thumbnails-v6/old.png",
            "icons-v2/glyph.png",
            "other/untouched.png",
        ] {
            std::fs::write(root.join(file), b"fixture").unwrap();
        }
        assert_eq!(super::clear_cache_at(&root).unwrap(), (5, 0));
        for gone in [
            "icon.png",
            "thumbnails-v7/thumb.png",
            "thumbnails-v7/index.json",
            "thumbnails-v6/old.png",
            "icons-v2/glyph.png",
        ] {
            assert!(!root.join(gone).exists(), "{gone} should be cleared");
        }
        for kept in ["keep.txt", "thumbnails-v7/keep.txt", "other/untouched.png"] {
            assert!(root.join(kept).exists(), "{kept} should be kept");
            std::fs::remove_file(root.join(kept)).unwrap();
        }
        std::fs::remove_dir(root.join(super::THUMBNAIL_DIR)).unwrap();
        std::fs::remove_dir(root.join(super::LEGACY_THUMBNAIL_DIR)).unwrap();
        std::fs::remove_dir(root.join(super::ICON_DIR)).unwrap();
        std::fs::remove_dir(root.join("other")).unwrap();
        std::fs::remove_dir(&root).unwrap();
        assert_eq!(super::clear_cache_at(&root).unwrap(), (0, 0));
    }
}
