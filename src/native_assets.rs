//! Native providers run in a bounded helper process; the UI only sees cached PNGs.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    hash::{Hash, Hasher},
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

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
        // is remembered for this process.
        let custom_folder = directory && path.is_absolute() && folder_has_own_artwork(path);
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
            // A drive icon is cached under a stable key so a new process can paint it
            // from disk on the first frame (an outdated badge is better than a blank
            // square; "clear cache" refreshes it). Artwork folders and portable-device
            // roots keep the time bucket because their icon can change on its own.
            epoch: if shell_root || custom_folder {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
                    / 30
            } else {
                0
            },
        }
    }
}
/// The already generated icon for this request, if any. Read from disk without a
/// helper round trip so a restarted application paints cached icons immediately.
pub fn cached_icon_path(key: &IconRequest) -> Option<PathBuf> {
    icon_cache_path(key).ok().filter(|path| path.is_file())
}
/// Whether a folder declares its own icon/class in `desktop.ini`. Memoized for the
/// process: the answer only changes when the folder changes, and a long listing asks
/// once per folder. Cheap enough to run while laying out a row (~10 µs), and it keeps
/// the common case on the shared cached folder icon.
fn folder_has_own_artwork(path: &Path) -> bool {
    use std::cell::RefCell;
    thread_local! {
        static CACHE: RefCell<std::collections::HashMap<PathBuf, bool>> =
            RefCell::new(std::collections::HashMap::new());
    }
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(found) = cache.get(path) {
            return *found;
        }
        if cache.len() >= 32768 {
            cache.clear();
        }
        let found = crate::system_icons::folder_has_custom_icon(path);
        cache.insert(path.to_path_buf(), found);
        found
    })
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
    pub path: PathBuf,
    pub pages: u32,
    pub elapsed_ms: u128,
}
/// One cached thumbnail: the content-addressed PNG plus the decorations that
/// belong to this *item*. A link badge or a photo resolution is not part of the
/// image, so entries sharing one PNG still keep their own presentation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThumbnailAsset {
    pub image: PathBuf,
    pub presentation: crate::thumbnails::Presentation,
}
#[derive(Serialize, Deserialize)]
enum Response {
    ShortcutTarget(PathBuf),
    Thumbnail(ThumbnailAsset),
    ThumbnailError(String),
    Preview(PreviewAsset),
    Icons(Vec<Option<PathBuf>>),
}

/// Thumbnails are stored under the SHA-256 of their PNG bytes, so every item
/// that renders the same image shares one file. The per-item index keeps the
/// mapping and is what a cache hit reads.
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
    clear_cache_at(&cache_root())
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
fn cache_path(key: impl Hash) -> Result<PathBuf> {
    let root = cache_root();
    std::fs::create_dir_all(&root)?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hash);
    Ok(root.join(format!("{:016x}.png", hash.finish())))
}

/// Icons live in their own revision directory: the resolution rules changed (real
/// per-file icons for `.url`/`.lnk`/`.ico`), so older cached bitmaps must not be
/// reused after an upgrade.
fn icon_root() -> PathBuf {
    cache_root().join(ICON_DIR)
}

fn icon_hash_path(item: &IconRequest) -> Result<PathBuf> {
    let root = icon_root();
    std::fs::create_dir_all(&root)?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    item.hash(&mut hash);
    Ok(root.join(format!("{:016x}.png", hash.finish())))
}

fn thumbnail_root() -> PathBuf {
    cache_root().join(THUMBNAIL_DIR)
}

/// Stable per-item name for the index, derived from the thumbnail key only.
fn thumbnail_index_path(key: &crate::thumbnails::Key) -> Result<PathBuf> {
    let root = thumbnail_root();
    std::fs::create_dir_all(&root)?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hash);
    Ok(root.join(format!("{:016x}.json", hash.finish())))
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
    let bytes = std::fs::read(index).ok()?;
    let asset: ThumbnailAsset = serde_json::from_slice(&bytes).ok()?;
    asset.image.is_file().then_some(asset)
}

/// A provider that reported "no thumbnail" for this exact key is remembered next
/// to the index, so reopening the folder (or restarting the application) does not
/// pay the same Shell round trip again. The key already contains size and mtime.
const MISS_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
fn miss_path(index: &Path) -> PathBuf {
    index.with_extension("miss")
}
fn missing_thumbnail(index: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(miss_path(index)) else {
        return false;
    };
    let Ok(seconds) = text.trim().parse::<u64>() else {
        return false;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    now.saturating_sub(seconds) < MISS_TTL.as_secs() && seconds <= now
}
pub fn remember_missing_thumbnail(key: &crate::thumbnails::Key) {
    let Ok(index) = thumbnail_index_path(key) else {
        return;
    };
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let _ = std::fs::write(miss_path(&index), seconds.to_string());
}

fn save_thumbnail_index(index: &Path, asset: &ThumbnailAsset) -> Result<()> {
    let temporary = index.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&temporary, serde_json::to_vec(asset)?)?;
    if std::fs::rename(&temporary, index).is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    Ok(())
}

fn prune_thumbnail_cache(root: &Path) {
    // Only helpers touch disk. At most once per five minutes, cap the shared
    // images at 4096 PNGs / 128 MiB and the per-item indexes at 8192. Dropping an
    // image only costs a regeneration: the same bytes map back to the same name.
    let marker = root.join("last-maintenance");
    if std::fs::metadata(&marker)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|d| d < Duration::from_secs(300))
    {
        return;
    }
    let _ = std::fs::write(&marker, []);
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut images = Vec::new();
    let mut indexes = Vec::new();
    for entry in entries.flatten().take(16384) {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let record = (metadata.modified().ok(), metadata.len(), entry.path());
        match entry.path().extension().and_then(|e| e.to_str()) {
            Some("png") => images.push(record),
            Some("json" | "miss") => indexes.push(record),
            _ => {}
        }
    }
    images.sort_unstable_by_key(|(modified, _, _)| *modified);
    let mut bytes: u64 = images.iter().map(|(_, size, _)| *size).sum();
    let mut count = images.len();
    for (_, size, path) in images {
        if bytes <= 128 * 1024 * 1024 && count <= 4096 {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            bytes = bytes.saturating_sub(size);
            count -= 1;
        }
    }
    if indexes.len() > 8192 {
        indexes.sort_unstable_by_key(|(modified, _, _)| *modified);
        for (_, _, path) in indexes.drain(..indexes.len() - 8192) {
            let _ = std::fs::remove_file(path);
        }
    }
    remove_legacy_thumbnail_cache(root);
}

/// The previous cache stored one PNG per item. Drop it once the new directory
/// exists so an upgrade does not keep the duplicates around.
fn remove_legacy_thumbnail_cache(root: &Path) {
    let Some(parent) = root.parent() else {
        return;
    };
    let legacy = parent.join(LEGACY_THUMBNAIL_DIR);
    let Ok(entries) = std::fs::read_dir(&legacy) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file()
            && matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("png" | "json" | "tmp")
            )
        {
            let _ = std::fs::remove_file(&path);
        }
    }
    let _ = std::fs::remove_dir(&legacy);
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

fn icon_cache_path(item: &IconRequest) -> Result<PathBuf> {
    if item.epoch == 0 {
        return icon_hash_path(item);
    }
    let mut stable = item.clone();
    stable.epoch = 0;
    let base = icon_hash_path(&stable)?;
    let prefix = format!(
        "{}-drive-",
        base.file_stem().unwrap_or_default().to_string_lossy()
    );
    let output = base.with_file_name(format!("{prefix}{}.png", item.epoch));
    if let Some(parent) = base.parent() {
        if let Ok(entries) = std::fs::read_dir(parent) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if let Some(epoch) = name
                    .strip_prefix(&prefix)
                    .and_then(|v| v.strip_suffix(".png"))
                    .and_then(|v| v.parse::<u64>().ok())
                {
                    if epoch.saturating_add(1) < item.epoch {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
        }
    }
    Ok(output)
}
/// Encode `rgba` and store it under the SHA-256 of the encoded PNG bytes.
/// Identical thumbnails (stock folder art, the same document copied around, a
/// generic file icon) therefore occupy one file.
fn store_image(dir: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<PathBuf> {
    anyhow::ensure!(
        width > 0 && height > 0 && width <= 4096 && height <= 4096,
        "原生图像尺寸无效"
    );
    let temporary = dir.join(format!("thumb-{}.tmp", std::process::id()));
    image::save_buffer_with_format(
        &temporary,
        rgba,
        width,
        height,
        image::ColorType::Rgba8,
        image::ImageFormat::Png,
    )?;
    let bytes = std::fs::read(&temporary)?;
    let output = dir.join(format!("{}.png", sha256_hex(&bytes)));
    if output.is_file() {
        let _ = std::fs::remove_file(&temporary);
        return Ok(output);
    }
    if let Err(error) = std::fs::rename(&temporary, &output) {
        let _ = std::fs::remove_file(&temporary);
        if !output.is_file() {
            return Err(error.into());
        }
    }
    Ok(output)
}
fn save_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    anyhow::ensure!(
        width > 0 && height > 0 && width <= 4096 && height <= 4096,
        "原生图像尺寸无效"
    );
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    image::save_buffer_with_format(
        &temporary,
        rgba,
        width,
        height,
        image::ColorType::Rgba8,
        image::ImageFormat::Png,
    )?;
    if let Err(e) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        if !path.is_file() {
            return Err(e.into());
        }
    }
    Ok(())
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
                let image_root = thumbnail_root();
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
                let image = store_image(&image_root, bitmap.width, bitmap.height, &bitmap.rgba)?;
                prune_thumbnail_cache(&image_root);
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
            let output = cache_path((
                &path,
                metadata.len(),
                metadata.modified().ok(),
                kind,
                page,
                pixels,
            ))?;
            let info = output.with_extension("json");
            if output.is_file() && info.is_file() {
                let asset: PreviewAsset = serde_json::from_slice(&std::fs::read(info)?)?;
                Response::Preview(asset)
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
                save_png(&output, width, height, &rgba)?;
                let asset = PreviewAsset {
                    path: output,
                    pages,
                    elapsed_ms: start.elapsed().as_millis(),
                };
                std::fs::write(info, serde_json::to_vec(&asset)?)?;
                Response::Preview(asset)
            }
        }
        Request::Icons { items } => {
            let mut paths = Vec::with_capacity(items.len());
            for item in items {
                let item = share_plain_folder_icon(item);
                let result = (|| -> Result<PathBuf> {
                    let output = icon_cache_path(&item)?;
                    if !output.is_file() {
                        let bitmap = if item.name == "associated-app" {
                            crate::system_icons::associated_app_icon(&item.path, item.pixels)?
                        } else if item.name == "file" {
                            crate::system_icons::file_icon(&item.path, item.directory, item.pixels)?
                        } else {
                            crate::system_icons::ui_icon(&item.name, item.pixels)?
                        };
                        save_png(&output, bitmap.width, bitmap.height, &bitmap.rgba)?;
                    }
                    Ok(output)
                })();
                paths.push(result.ok());
            }
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
            .take(512 * 1024)
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
pub fn icons(items: Vec<IconRequest>) -> Vec<Option<PathBuf>> {
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
        anyhow::ensure!(line.len() <= 256 * 1024, "缩略图请求过大");
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
            crate::shell::menu_trace("thumbnail: disk hit");
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
                        Ok(Response::ThumbnailError(_)) => {
                            return Err(anyhow::anyhow!("文件不支持缩略图"));
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
        let request = IconRequest::file(&custom, true, 96);
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
        // A drive icon uses a stable key: a fresh process paints the cached bitmap
        // immediately instead of waiting for the Shell (an outdated badge is fine).
        assert_eq!(drive.epoch, 0);
        // Drive roots keep their own icon: it carries the drive type and overlays,
        // so it must never be replaced by the generic folder icon.
        assert_eq!(share_plain_folder_icon(drive.clone()).path, drive.path);
        assert_eq!(share_plain_folder_icon(drive).epoch, 0);
        let unc = IconRequest::file(std::path::Path::new("\\\\server\\share"), true, 96);
        assert_eq!(share_plain_folder_icon(unc.clone()).path, unc.path);

        let text = IconRequest::file(std::path::Path::new("C:\\a\\x.txt"), false, 96);
        let upper = IconRequest::file(std::path::Path::new("D:\\b\\y.TXT"), false, 96);
        assert_eq!(text.path, PathBuf::from("file.txt"));
        assert_eq!(text.path, upper.path);
    }
    #[test]
    fn provider_negative_results_expire_instead_of_being_retried_every_session() {
        let root = temp_root("miss");
        let index = root.join("0123456789abcdef.json");
        assert!(!super::missing_thumbnail(&index));
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        std::fs::write(super::miss_path(&index), now.to_string()).unwrap();
        assert!(super::missing_thumbnail(&index));
        // A stale marker is ignored so a newly installed codec gets a chance.
        let stale = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as f64
            - super::MISS_TTL.as_secs() as f64
            - 60.) as u64;
        std::fs::write(super::miss_path(&index), stale.to_string()).unwrap();
        assert!(!super::missing_thumbnail(&index));
        std::fs::remove_dir_all(&root).unwrap();
    }
    #[test]
    fn cache_images_are_named_by_the_sha256_of_their_bytes() {
        assert_eq!(
            super::sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let root = temp_root("sha");
        let rgba = vec![128u8; 4 * 4 * 4];
        let image = super::store_image(&root, 4, 4, &rgba).unwrap();
        let bytes = std::fs::read(&image).unwrap();
        let name = image.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(name, format!("{}.png", super::sha256_hex(&bytes)));
        assert_eq!(name.len(), 64 + 4);
        std::fs::remove_dir_all(&root).unwrap();
    }
    #[test]
    fn identical_thumbnails_share_one_file_and_the_index_keeps_their_presentation() {
        let root = temp_root("dedupe");
        let rgba = vec![90u8; 4 * 4 * 4];
        let first = super::store_image(&root, 4, 4, &rgba).unwrap();
        let second = super::store_image(&root, 4, 4, &rgba).unwrap();
        assert_eq!(first, second);
        assert_eq!(std::fs::read_dir(&root).unwrap().flatten().count(), 1);
        // One different pixel is a different image and gets its own file.
        let mut other = rgba.clone();
        other[0] = 255;
        let third = super::store_image(&root, 4, 4, &other).unwrap();
        assert_ne!(first, third);
        assert_eq!(std::fs::read_dir(&root).unwrap().flatten().count(), 2);
        let index = root.join("0000000000000001.json");
        let asset = ThumbnailAsset {
            image: first.clone(),
            presentation: crate::thumbnails::Presentation {
                directory: true,
                width: 4,
                height: 4,
                ..Default::default()
            },
        };
        super::save_thumbnail_index(&index, &asset).unwrap();
        let loaded = super::load_thumbnail(&index).unwrap();
        assert_eq!(loaded.image, first);
        assert!(loaded.presentation.directory);
        assert!(!loaded.presentation.shortcut);
        assert_eq!(loaded.presentation.width, 4);
        // A pruned image turns the entry into a miss instead of a broken path.
        std::fs::remove_file(&first).unwrap();
        assert!(super::load_thumbnail(&index).is_none());
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
