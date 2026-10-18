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
impl IconRequest {
    pub fn ui(name: &str, pixels: u32) -> Self {
        Self {
            name: name.into(),
            path: PathBuf::new(),
            directory: false,
            pixels,
            epoch: crate::fluent_icons::resource_epoch(),
        }
    }
    pub fn file(path: &Path, directory: bool, pixels: u32) -> Self {
        // Shell class icons are shared by extension; avoid per-file extension
        // handlers. Portable-device child folders/files use generic stock icons
        // so a phone folder does not issue one slow Shell icon request each.
        let shell_root = path.to_string_lossy().starts_with("mtp-root:");
        let per_item = shell_root
            || (path.is_absolute()
                && matches!(
                    path.extension()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_ascii_lowercase()
                        .as_str(),
                    "exe" | "lnk" | "url" | "ico" | "cur"
                ));
        let key = if per_item || (directory && path.has_root() && path.parent().is_none()) {
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
            epoch: if shell_root || (directory && path.has_root() && path.parent().is_none()) {
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
#[derive(Serialize, Deserialize)]
enum Response {
    ShortcutTarget(PathBuf),
    Thumbnail(PathBuf),
    ThumbnailError(String),
    Preview(PreviewAsset),
    Icons(Vec<Option<PathBuf>>),
}

fn cache_root() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("RustExplorer/cache/native-v6-r3")
}
fn cache_path(key: impl Hash) -> Result<PathBuf> {
    let root = cache_root();
    std::fs::create_dir_all(&root)?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hash);
    Ok(root.join(format!("{:016x}.png", hash.finish())))
}

fn thumbnail_cache_path(key: &crate::thumbnails::Key) -> Result<PathBuf> {
    let root = cache_root().join("thumbnails-v3");
    std::fs::create_dir_all(&root)?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hash);
    Ok(root.join(format!("{:016x}.png", hash.finish())))
}

fn png_dimensions(path: &Path) -> Result<(u32, u32)> {
    use std::io::Read;
    let mut header = [0u8; 24];
    std::fs::File::open(path)?.read_exact(&mut header)?;
    anyhow::ensure!(
        &header[..8] == b"\x89PNG\r\n\x1a\n" && &header[12..16] == b"IHDR",
        "Invalid thumbnail PNG header"
    );
    let width = u32::from_be_bytes(header[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(header[20..24].try_into().unwrap());
    anyhow::ensure!(
        width > 0 && height > 0 && width <= 4096 && height <= 4096,
        "Invalid thumbnail dimensions"
    );
    Ok((width, height))
}

fn prune_thumbnail_cache(root: &Path) {
    // Only helpers touch disk. At most once per five minutes, cap this cache
    // at 4096 PNGs / 128 MiB. Killing a slow helper also bounds this maintenance.
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
    let mut files: Vec<_> = entries
        .flatten()
        .take(8192)
        .filter_map(|entry| {
            if entry.path().extension().is_none_or(|e| e != "png") {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            Some((metadata.modified().ok(), metadata.len(), entry.path()))
        })
        .collect();
    files.sort_unstable_by_key(|(modified, _, _)| *modified);
    let mut bytes: u64 = files.iter().map(|(_, size, _)| *size).sum();
    let mut count = files.len();
    for (_, size, path) in files {
        if bytes <= 128 * 1024 * 1024 && count <= 4096 {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            let _ = std::fs::remove_file(path.with_extension("json"));
            bytes = bytes.saturating_sub(size);
            count -= 1;
        }
    }
}

fn icon_cache_path(item: &IconRequest) -> Result<PathBuf> {
    if item.epoch == 0 {
        return cache_path(item);
    }
    let mut stable = item.clone();
    stable.epoch = 0;
    let base = cache_path(&stable)?;
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
            let metadata = std::fs::metadata(&item.path)?;
            anyhow::ensure!(
                metadata.file_attributes() & (0x1000 | 0x40000 | 0x400000) == 0,
                "不自动下载脱机或云占位文件"
            );
            anyhow::ensure!(
                metadata.is_dir() == item.directory
                    && (if metadata.is_dir() { 0 } else { metadata.len() }) == item.size
                    && metadata.modified().ok() == item.modified,
                "文件已变化"
            );
            let shortcut = !metadata.is_dir()
                && matches!(
                    item.path
                        .extension()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_ascii_lowercase()
                        .as_str(),
                    "lnk" | "url"
                );
            let target = if shortcut {
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
            let resolved = crate::thumbnails::Key {
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
            let output = if shortcut {
                let root = cache_root().join("thumbnails-v3");
                std::fs::create_dir_all(&root)?;
                let mut hash = std::collections::hash_map::DefaultHasher::new();
                (&item, &resolved).hash(&mut hash);
                root.join(format!("{:016x}.png", hash.finish()))
            } else {
                thumbnail_cache_path(&item)?
            };
            if !output.is_file() {
                let pixels = item.pixels.clamp(32, 256);
                let extension = resolved
                    .path
                    .extension()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_ascii_lowercase();
                let bitmap = if resolved.directory {
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
                    anyhow::ensure!(
                        matches!(
                            extension.as_str(),
                            "mp4" | "m4v" | "webm" | "mov" | "mkv" | "avi" | "wmv"
                        ),
                        "文件不支持缩略图"
                    );
                    crate::native_preview::file_thumbnail(&resolved.path, pixels)?
                };
                save_png(&output, bitmap.width, bitmap.height, &bitmap.rgba)?;
                if let Some(root) = output.parent() {
                    prune_thumbnail_cache(root);
                }
            }
            let extension = resolved
                .path
                .extension()
                .unwrap_or_default()
                .to_string_lossy()
                .to_ascii_lowercase();
            let (width, height) = png_dimensions(&output)?;
            let presentation = crate::thumbnails::Presentation {
                directory: resolved.directory,
                shortcut,
                width,
                height,
                video: !resolved.directory
                    && matches!(
                        extension.as_str(),
                        "mp4" | "m4v" | "webm" | "mov" | "mkv" | "avi" | "wmv"
                    ),
            };
            std::fs::write(
                output.with_extension("json"),
                serde_json::to_vec(&presentation)?,
            )?;
            Response::Thumbnail(output)
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
    let count = items.len();
    let cancel = Arc::new(AtomicBool::new(false));
    match call(
        Request::Icons { items },
        Duration::from_millis(1800),
        &cancel,
    ) {
        Ok(Response::Icons(paths)) => paths,
        _ => vec![None; count],
    }
}

pub fn thumbnail(key: &crate::thumbnails::Key, cancel: &Arc<AtomicBool>) -> Result<PathBuf> {
    let output = thumbnail_cache_path(key)?;
    if cancel.load(Ordering::Relaxed) {
        bail!("缩略图已取消");
    }
    if key.shortcut_epoch == 0 && output.is_file() {
        return Ok(output);
    }
    match call(
        Request::Thumbnail { item: key.clone() },
        Duration::from_millis(800),
        cancel,
    )? {
        Response::Thumbnail(path) => Ok(path),
        _ => bail!("无效缩略图响应"),
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
    pub fn get(
        &mut self,
        key: &crate::thumbnails::Key,
        cancel: &Arc<AtomicBool>,
    ) -> Result<PathBuf> {
        use std::{
            io::{BufRead, Write},
            os::windows::process::CommandExt,
        };
        let output = thumbnail_cache_path(key)?;
        if cancel.load(Ordering::Relaxed) {
            bail!("缩略图已取消");
        }
        if key.shortcut_epoch == 0 && output.is_file() {
            return Ok(output);
        }
        if self.process.is_none() {
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
        }
        let result = (|| -> Result<PathBuf> {
            let process = self.process.as_mut().unwrap();
            let start = Instant::now();
            let mut bytes = serde_json::to_vec(&Request::Thumbnail { item: key.clone() })?;
            bytes.push(b'\n');
            process
                .requests
                .try_send(bytes)
                .context("缩略图服务请求未接收")?;
            loop {
                if cancel.load(Ordering::Relaxed) || start.elapsed() > Duration::from_millis(800) {
                    bail!("缩略图取消或超时");
                }
                match process.replies.recv_timeout(Duration::from_millis(10)) {
                    Ok(line) => match serde_json::from_str::<Response>(&line) {
                        Ok(Response::Thumbnail(path)) => return Ok(path),
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
