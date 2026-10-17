//! Blocking native preview work. Call only in the isolated preview helper process.
//!
//! Shell thumbnail providers and PDF parsers may block on third party codecs,
//! remote files, or malformed inputs. The caller owns the helper deadline.
use anyhow::{Context, Result, bail, ensure};
use std::{
    mem::size_of,
    path::{Path, PathBuf},
};
use windows::{
    Data::Pdf::{PdfDocument, PdfPage, PdfPageRenderOptions},
    Graphics::Imaging::{
        BitmapAlphaMode, BitmapDecoder, BitmapPixelFormat, BitmapTransform, ColorManagementMode,
        ExifOrientationMode,
    },
    Storage::{StorageFile, Streams::InMemoryRandomAccessStream},
    Win32::{
        Foundation::SIZE,
        Graphics::Gdi::*,
        System::{
            Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize},
            WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
        },
        UI::Shell::{
            IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_INCACHEONLY,
            SIIGBF_THUMBNAILONLY,
        },
    },
    core::{HSTRING, PCWSTR},
};

/// Top-down, tightly packed, straight-alpha RGBA8 pixels.
#[derive(Debug, Clone)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

struct Com;
impl Com {
    fn new() -> Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        }
        Ok(Self)
    }
}
impl Drop for Com {
    fn drop(&mut self) {
        unsafe { CoUninitialize() }
    }
}
struct WinRt;
impl WinRt {
    fn new() -> Result<Self> {
        unsafe {
            RoInitialize(RO_INIT_MULTITHREADED)?;
        }
        Ok(Self)
    }
}
impl Drop for WinRt {
    fn drop(&mut self) {
        unsafe { RoUninitialize() }
    }
}
pub(crate) struct ThumbnailRuntime {
    _com: Com,
    _runtime: WinRt,
}
pub(crate) fn thumbnail_runtime() -> Result<ThumbnailRuntime> {
    Ok(ThumbnailRuntime {
        _com: Com::new()?,
        _runtime: WinRt::new()?,
    })
}
struct OwnedBitmap(HBITMAP);
impl Drop for OwnedBitmap {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.0.into());
        }
    }
}
struct MemoryDc(HDC);
impl Drop for MemoryDc {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteDC(self.0);
        }
    }
}
struct OpenPdfPage(PdfPage);
impl Drop for OpenPdfPage {
    fn drop(&mut self) {
        let _ = self.0.Close();
    }
}
struct MemoryStream(InMemoryRandomAccessStream);
impl Drop for MemoryStream {
    fn drop(&mut self) {
        let _ = self.0.Close();
    }
}

fn pixel_len(width: u32, height: u32) -> Result<usize> {
    ensure!(
        width > 0 && height > 0 && width <= 4096 && height <= 4096,
        "预览图像尺寸超出限制"
    );
    Ok(width as usize * height as usize * 4)
}

/// Request one genuine Shell thumbnail, never fall back to a video file icon.
/// Uses Explorer's thumbnail cache first, then the registered thumbnail provider.
/// A missing/unsupported codec returns an error and should hide video preview.
pub fn video_thumbnail(path: &Path, size: u32) -> Result<Bitmap> {
    file_thumbnail(path, size)
}

/// Resolve a shortcut without launching it, searching for missing targets or
/// contacting a network location. Called exclusively inside the bounded helper.
pub fn shortcut_target(path: &Path) -> Result<PathBuf> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::{
        System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READ},
        UI::Shell::{IShellLinkW, SLGP_RAWPATH, ShellLink},
    };
    use windows::core::Interface;
    let extension = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    let target = if extension == "lnk" {
        let _com = Com::new()?;
        let link: IShellLinkW =
            unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)? };
        let persist: IPersistFile = link.cast()?;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        unsafe {
            persist.Load(PCWSTR(wide.as_ptr()), STGM_READ)?;
        }
        let mut target = [0_u16; 32768];
        unsafe {
            link.GetPath(&mut target, std::ptr::null_mut(), SLGP_RAWPATH.0 as u32)?;
        }
        let length = target
            .iter()
            .position(|ch| *ch == 0)
            .unwrap_or(target.len());
        PathBuf::from(String::from_utf16_lossy(&target[..length]))
    } else if extension == "url" {
        ensure!(
            std::fs::metadata(path)?.len() <= 64 * 1024,
            "Internet shortcut is too large"
        );
        let bytes = std::fs::read(path)?;
        let text = if bytes.starts_with(&[0xff, 0xfe]) {
            String::from_utf16_lossy(
                &bytes[2..]
                    .chunks_exact(2)
                    .map(|v| u16::from_le_bytes([v[0], v[1]]))
                    .collect::<Vec<_>>(),
            )
        } else {
            String::from_utf8_lossy(&bytes)
                .trim_start_matches('\u{feff}')
                .to_string()
        };
        let value = text
            .lines()
            .find_map(|line| line.trim().strip_prefix("URL="))
            .context("No URL target")?;
        ensure!(
            value.to_ascii_lowercase().starts_with("file:///"),
            "Only local file shortcuts have thumbnails"
        );
        // WinRT URI decoding handles percent escaped Unicode and drive letters.
        let uri = windows::Foundation::Uri::CreateUri(&HSTRING::from(value))?;
        let decoded = windows::Foundation::Uri::UnescapeComponent(&uri.Path()?)?.to_string();
        PathBuf::from(decoded.trim_start_matches('/').replace('/', "\\"))
    } else {
        return Ok(path.to_path_buf());
    };
    ensure!(
        target.is_absolute() && !target.to_string_lossy().starts_with("\\\\"),
        "Shortcut has no local absolute target"
    );
    Ok(target)
}

#[cfg(test)]
mod shortcut_tests {
    use super::*;
    #[test]
    fn font_samples_use_private_real_font_and_reject_malformed_tables() -> Result<()> {
        assert!(font_family(b"not a font").is_none());
        assert!(font_family(&[0; 128]).is_none());
        let mut windows_dir = [0u16; 32768];
        let count = unsafe {
            windows::Win32::System::SystemInformation::GetWindowsDirectoryW(Some(&mut windows_dir))
        } as usize;
        ensure!(
            count > 0 && count < windows_dir.len(),
            "Windows directory unavailable"
        );
        let fonts = PathBuf::from(String::from_utf16_lossy(&windows_dir[..count])).join("Fonts");
        for name in ["segoeui.ttf", "msyh.ttc"] {
            let path = fonts.join(name);
            if !path.is_file() {
                continue;
            }
            let sample = font_sample(&path, 128)?;
            assert_eq!((sample.width, sample.height), (128, 96));
            assert!(
                sample
                    .rgba
                    .chunks_exact(4)
                    .filter(|pixel| pixel[0] < 200)
                    .count()
                    > 10
            );
            assert!(sample.rgba.chunks_exact(4).all(|pixel| pixel[3] == 255));
        }
        if fonts.join("segoeui.ttf").is_file()
            && fonts.join("segoeuii.ttf").is_file()
            && fonts.join("segoeuib.ttf").is_file()
        {
            let regular = font_sample(&fonts.join("segoeui.ttf"), 128)?.rgba;
            assert_ne!(
                regular,
                font_sample(&fonts.join("segoeuii.ttf"), 128)?.rgba,
                "Italic file was substituted with regular font"
            );
            assert_ne!(
                regular,
                font_sample(&fonts.join("segoeuib.ttf"), 128)?.rgba,
                "Bold file was substituted with regular font"
            );
        }
        Ok(())
    }
    #[test]
    fn shell_link_and_local_url_resolve_without_executing_target() -> Result<()> {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::{
            System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile},
            UI::Shell::{IShellLinkW, ShellLink},
        };
        use windows::core::Interface;
        let _runtime = thumbnail_runtime()?;
        let root = std::env::temp_dir().join(format!(
            "rust-explorer-link-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        std::fs::create_dir_all(&root)?;
        let target = root.join("photo with spaces.png");
        std::fs::write(&target, b"target is never executed")?;
        let destination = root.join("photo.lnk");
        let link: IShellLinkW =
            unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)? };
        let target_wide: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
        let destination_wide: Vec<u16> = destination
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        unsafe {
            link.SetPath(PCWSTR(target_wide.as_ptr()))?;
        }
        let persist: IPersistFile = link.cast()?;
        unsafe {
            persist.Save(PCWSTR(destination_wide.as_ptr()), true)?;
        }
        assert_eq!(shortcut_target(&destination)?, target);
        let internet = root.join("photo.url");
        let uri = format!(
            "[InternetShortcut]\r\nURL=file:///{}\r\n",
            target
                .to_string_lossy()
                .replace('\\', "/")
                .replace(' ', "%20")
        );
        std::fs::write(&internet, uri)?;
        assert_eq!(shortcut_target(&internet)?, target);
        std::fs::write(
            &internet,
            "[InternetShortcut]\r\nURL=https://example.com/image.png\r\n",
        )?;
        assert!(shortcut_target(&internet).is_err());
        std::fs::remove_file(destination)?;
        std::fs::remove_file(internet)?;
        std::fs::remove_file(target)?;
        std::fs::remove_dir(root)?;
        Ok(())
    }
}

/// Genuine Explorer thumbnail for photos, videos or other registered providers.
pub fn file_thumbnail(path: &Path, size: u32) -> Result<Bitmap> {
    shell_thumbnail(path, size, false)
}

/// Folder compositions need the alpha that an ordinary opaque video frame does not.
pub fn folder_thumbnail(path: &Path, size: u32) -> Result<Bitmap> {
    shell_thumbnail(path, size, true)
}

fn shell_thumbnail(path: &Path, size: u32, folder_alpha: bool) -> Result<Bitmap> {
    let _com = Com::new()?;
    use std::os::windows::ffi::OsStrExt;
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let factory: IShellItemImageFactory =
        unsafe { SHCreateItemFromParsingName(PCWSTR(path.as_ptr()), None) }
            .context("创建 Windows 缩略图提供程序")?;
    let extent = size.clamp(32, 2048) as i32;
    let bounds = SIZE {
        cx: extent,
        cy: extent,
    };
    // Folder compositions are icons, not small photos. Ask the Shell to use
    // larger cached artwork when available and to fill the requested size.
    let flags = if folder_alpha {
        SIIGBF_THUMBNAILONLY
            | windows::Win32::UI::Shell::SIIGBF_BIGGERSIZEOK
            | windows::Win32::UI::Shell::SIIGBF_SCALEUP
    } else {
        SIIGBF_THUMBNAILONLY
    };
    let bitmap = OwnedBitmap(
        unsafe {
            factory
                .GetImage(bounds, flags | SIIGBF_INCACHEONLY)
                .or_else(|_| factory.GetImage(bounds, flags))
        }
        .context("系统未提供此文件的缩略图")?,
    );
    bitmap_pixels(bitmap.0, folder_alpha)
}

fn bitmap_pixels(bitmap: HBITMAP, folder_alpha: bool) -> Result<Bitmap> {
    let mut metadata = BITMAP::default();
    let copied = unsafe {
        GetObjectW(
            bitmap.into(),
            size_of::<BITMAP>() as i32,
            Some((&mut metadata as *mut BITMAP).cast()),
        )
    };
    ensure!(
        copied == size_of::<BITMAP>() as i32,
        "无法读取系统缩略图尺寸"
    );
    let width = metadata.bmWidth.unsigned_abs();
    let height = metadata.bmHeight.unsigned_abs();
    let mut pixels = vec![0u8; pixel_len(width, height)?];
    let dc = MemoryDc(unsafe { CreateCompatibleDC(None) });
    ensure!(!dc.0.is_invalid(), "无法创建缩略图设备上下文");
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let lines = unsafe {
        GetDIBits(
            dc.0,
            bitmap,
            0,
            height,
            Some(pixels.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        )
    };
    ensure!(lines == height as i32, "无法读取系统缩略图像素");
    // Shell folder compositions often carry real premultiplied alpha. Keep it
    // when present; opaque media providers historically leave this channel zero.
    let has_alpha = folder_alpha && pixels.chunks_exact(4).any(|pixel| pixel[3] != 0);
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
        if !has_alpha {
            pixel[3] = 255;
        } else if pixel[3] > 0 && pixel[3] < 255 {
            let alpha = u32::from(pixel[3]);
            for channel in &mut pixel[..3] {
                *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
    if folder_alpha {
        pixels = crate::icon_layout::transparent_folder_border(width, height, pixels);
    }
    Ok(Bitmap {
        width,
        height,
        rgba: pixels,
    })
}

/// Bounded SFNT name-table decoding; TTC collections preview their first face.
/// Invalid/unrecognized names fail instead of silently drawing a fallback font.
fn font_family_names(bytes: &[u8]) -> Option<Vec<(u32, String)>> {
    fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
        Some(u16::from_be_bytes(
            bytes.get(at..at.checked_add(2)?)?.try_into().ok()?,
        ))
    }
    fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
        Some(u32::from_be_bytes(
            bytes.get(at..at.checked_add(4)?)?.try_into().ok()?,
        ))
    }
    let base = if bytes.get(..4)? == b"ttcf" {
        u32_at(bytes, 12)? as usize
    } else {
        0
    };
    let tag = bytes.get(base..base.checked_add(4)?)?;
    if tag != [0, 1, 0, 0] && tag != b"OTTO" && tag != b"true" {
        return None;
    }
    let count = (u16_at(bytes, base + 4)? as usize).min(1024);
    let name = (0..count).find_map(|index| {
        let record = base.checked_add(12 + index * 16)?;
        if bytes.get(record..record + 4)? == b"name" {
            Some((
                u32_at(bytes, record + 8)? as usize,
                u32_at(bytes, record + 12)? as usize,
            ))
        } else {
            None
        }
    })?;
    let table = bytes.get(name.0..name.0.checked_add(name.1)?)?;
    let records = (u16_at(table, 2)? as usize).min(4096);
    let strings = u16_at(table, 4)? as usize;
    let mut names = Vec::new();
    for index in 0..records {
        let record = 6 + index * 12;
        let platform = u16_at(table, record)?;
        let language = u16_at(table, record + 4)?;
        let name_id = u16_at(table, record + 6)?;
        if !matches!(platform, 0 | 3) || !matches!(name_id, 1 | 16) {
            continue;
        }
        let length = u16_at(table, record + 8)? as usize;
        let offset = strings.checked_add(u16_at(table, record + 10)? as usize)?;
        let encoded = table.get(offset..offset.checked_add(length)?)?;
        let text = String::from_utf16(
            &encoded
                .chunks_exact(2)
                .map(|v| u16::from_be_bytes([v[0], v[1]]))
                .collect::<Vec<_>>(),
        )
        .ok()?;
        if text.is_empty() || text.chars().count() > 100 || text.contains('\0') {
            continue;
        }
        // GDI's LOGFONT family selection follows legacy family name ID 1.
        let rank = if name_id == 1 { 100 } else { 0 }
            + if language == 0x409 { 10 } else { 0 }
            + if platform == 3 { 1 } else { 0 };
        names.push((rank, text));
    }
    Some(names)
}
fn font_family(bytes: &[u8]) -> Option<String> {
    font_family_names(bytes)?
        .into_iter()
        .max_by_key(|(rank, _)| *rank)
        .map(|(_, name)| name)
}
fn font_weight_and_italic(bytes: &[u8]) -> (i32, bool) {
    let read16 = |at: usize| {
        bytes
            .get(at..at + 2)
            .and_then(|value| value.try_into().ok())
            .map(u16::from_be_bytes)
    };
    let read32 = |at: usize| {
        bytes
            .get(at..at + 4)
            .and_then(|value| value.try_into().ok())
            .map(u32::from_be_bytes)
    };
    let base = if bytes.starts_with(b"ttcf") {
        read32(12).unwrap_or(0) as usize
    } else {
        0
    };
    for index in 0..read16(base + 4).unwrap_or(0).min(1024) as usize {
        let record = base + 12 + index * 16;
        if bytes.get(record..record + 4) == Some(b"OS/2".as_slice()) {
            let Some(offset) = read32(record + 8).map(|value| value as usize) else {
                break;
            };
            let length = read32(record + 12).unwrap_or(0);
            if length < 64 {
                break;
            }
            return (
                read16(offset + 4).unwrap_or(400).clamp(1, 1000) as i32,
                read16(offset + 62).unwrap_or(0) & 1 != 0,
            );
        }
    }
    (400, false)
}

/// Rasterize a font sample inside the isolated helper. FR_PRIVATE avoids any
/// installation or registry changes; each font is removed before returning.
pub fn font_sample(path: &Path, size: u32) -> Result<Bitmap> {
    use std::os::windows::ffi::OsStrExt;
    let extension = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    ensure!(
        matches!(extension.as_str(), "ttf" | "otf" | "ttc" | "fon"),
        "Unsupported font type"
    );
    if extension == "fon" {
        return file_thumbnail(path, size);
    }
    ensure!(
        std::fs::metadata(path)?.len() <= 32 * 1024 * 1024,
        "Font exceeds preview limit"
    );
    let bytes = std::fs::read(path)?;
    let family = font_family(&bytes).context("Cannot read a supported font family name")?;
    let family_names = font_family_names(&bytes).unwrap_or_default();
    let (weight, italic) = font_weight_and_italic(&bytes);
    let path_wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    ensure!(
        unsafe { AddFontResourceExW(PCWSTR(path_wide.as_ptr()), FR_PRIVATE, None) } > 0,
        "Windows cannot load this font"
    );
    struct PrivateFont(Vec<u16>);
    impl Drop for PrivateFont {
        fn drop(&mut self) {
            unsafe {
                let _ = RemoveFontResourceExW(PCWSTR(self.0.as_ptr()), FR_PRIVATE.0, None);
            }
        }
    }
    let _private = PrivateFont(path_wide);
    let width = size.clamp(48, 1600);
    let height = (width * 3 / 4).max(48);
    let len = pixel_len(width, height)?;
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let dc = MemoryDc(unsafe { CreateCompatibleDC(None) });
    ensure!(!dc.0.is_invalid(), "Cannot create font drawing context");
    let mut address = std::ptr::null_mut();
    let bitmap = OwnedBitmap(unsafe {
        CreateDIBSection(Some(dc.0), &info, DIB_RGB_COLORS, &mut address, None, 0)?
    });
    let old_bitmap = unsafe { SelectObject(dc.0, bitmap.0.into()) };
    let result = (|| -> Result<Bitmap> {
        let pixels = unsafe { std::slice::from_raw_parts_mut(address.cast::<u8>(), len) };
        pixels.fill(255);
        unsafe {
            SetBkMode(dc.0, TRANSPARENT);
            SetTextColor(dc.0, windows::Win32::Foundation::COLORREF(0x242424));
        }
        let face: Vec<u16> = family.encode_utf16().take(31).chain(Some(0)).collect();
        let samples: Vec<(&str, f32, f32)> = if width < 300 {
            vec![("Aa", 0.40, 0.12), ("Bb 0123", 0.12, 0.66)]
        } else {
            vec![
                (family.as_str(), 0.045, 0.06),
                ("Aa Bb Cc 0123456789", 0.08, 0.20),
                ("The quick brown fox jumps over the lazy dog.", 0.035, 0.46),
                ("ABCDEFGHIJKLMNOPQRSTUVWXYZ", 0.035, 0.59),
                ("abcdefghijklmnopqrstuvwxyz 0123456789", 0.035, 0.70),
                ("Rust Explorer", 0.045, 0.82),
            ]
        };
        for (text, scale, top) in samples {
            let font = unsafe {
                CreateFontW(
                    -((width as f32 * scale) as i32).max(8),
                    0,
                    0,
                    0,
                    weight,
                    u32::from(italic),
                    0,
                    0,
                    DEFAULT_CHARSET,
                    OUT_DEFAULT_PRECIS,
                    CLIP_DEFAULT_PRECIS,
                    ANTIALIASED_QUALITY,
                    DEFAULT_PITCH.0 as u32,
                    PCWSTR(face.as_ptr()),
                )
            };
            ensure!(!font.is_invalid(), "Cannot select font");
            let previous = unsafe { SelectObject(dc.0, font.into()) };
            let mut selected = [0u16; 128];
            let selected_count = unsafe { GetTextFaceW(dc.0, Some(&mut selected)) };
            let selected = String::from_utf16_lossy(
                &selected[..selected_count.max(0) as usize]
                    .split(|value| *value == 0)
                    .next()
                    .unwrap_or_default(),
            );
            let encoded: Vec<u16> = text.encode_utf16().collect();
            let drawn = unsafe {
                TextOutW(
                    dc.0,
                    (width / 12) as i32,
                    (height as f32 * top) as i32,
                    &encoded,
                )
            };
            unsafe {
                SelectObject(dc.0, previous);
                let _ = DeleteObject(font.into());
            }
            ensure!(
                family_names
                    .iter()
                    .any(|(_, name)| selected.eq_ignore_ascii_case(name)
                        || selected
                            .eq_ignore_ascii_case(&name.chars().take(31).collect::<String>())),
                "Windows selected {selected:?} instead of {family:?}; no misleading sample is shown"
            );
            ensure!(drawn.as_bool(), "Font sample drawing failed");
        }
        let mut rgba = pixels.to_vec();
        for pixel in rgba.chunks_exact_mut(4) {
            pixel.swap(0, 2);
            pixel[3] = 255;
        }
        Ok(Bitmap {
            width,
            height,
            rgba,
        })
    })();
    unsafe {
        SelectObject(dc.0, old_bitmap);
    }
    result
}

/// Decode directly to a small, EXIF-oriented bitmap with the Windows image codec.
pub fn photo_thumbnail(path: &Path, size: u32) -> Result<Bitmap> {
    let _runtime = WinRt::new()?;
    let path: PathBuf = std::path::absolute(path)?.components().collect();
    let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(path.as_os_str()))?.get()?;
    let stream = file.OpenReadAsync()?.get()?;
    let result = (|| -> Result<Bitmap> {
        let decoder = BitmapDecoder::CreateAsync(&stream)?.get()?;
        let raw_width = decoder.PixelWidth()?;
        let raw_height = decoder.PixelHeight()?;
        ensure!(
            raw_width > 0 && raw_height > 0 && raw_width as u64 * raw_height as u64 <= 100_000_000,
            "照片尺寸超出自动缩略图限制"
        );
        let edge = size.clamp(32, 256) as f32;
        let scale = (edge / raw_width.max(raw_height) as f32).min(1.);
        let mut width = (raw_width as f32 * scale).round().max(1.) as u32;
        let mut height = (raw_height as f32 * scale).round().max(1.) as u32;
        let transform = BitmapTransform::new()?;
        transform.SetScaledWidth(width)?;
        transform.SetScaledHeight(height)?;
        let provider = decoder
            .GetPixelDataTransformedAsync(
                BitmapPixelFormat::Rgba8,
                BitmapAlphaMode::Straight,
                &transform,
                ExifOrientationMode::RespectExifOrientation,
                ColorManagementMode::ColorManageToSRgb,
            )?
            .get()?;
        if decoder.OrientedPixelWidth()? != raw_width {
            std::mem::swap(&mut width, &mut height);
        }
        let rgba = provider.DetachPixelData()?.to_vec();
        ensure!(
            rgba.len() == pixel_len(width, height)?,
            "照片缩略图尺寸无效"
        );
        Ok(Bitmap {
            width,
            height,
            rgba,
        })
    })();
    let _ = stream.Close();
    result
}

/// Render a zero-based PDF page using Windows.Data.Pdf (no browser runtime).
/// `size` is the maximum output edge; returns the document's total page count.
pub fn pdf_page(path: &Path, page: u32, size: u32) -> Result<(Bitmap, u32)> {
    // WinRT requires Windows separators even when Rust accepts mixed '/' paths.
    let path: PathBuf = std::path::absolute(path)?.components().collect();
    let _runtime = WinRt::new()?;
    let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(path.as_os_str()))?
        .get()
        .context("打开 PDF")?;
    let document = PdfDocument::LoadFromFileAsync(&file)?
        .get()
        .context("读取 PDF")?;
    let total = document.PageCount()?;
    ensure!(page < total, "PDF 页码超出范围");
    let page = OpenPdfPage(document.GetPage(page)?);
    let dimensions = page.0.Size()?;
    ensure!(
        dimensions.Width.is_finite()
            && dimensions.Height.is_finite()
            && dimensions.Width > 0.0
            && dimensions.Height > 0.0,
        "PDF 页面尺寸无效"
    );
    let max_edge = size.clamp(32, 2048) as f32;
    let scale = max_edge / dimensions.Width.max(dimensions.Height);
    let width = (dimensions.Width * scale).round().max(1.0) as u32;
    let height = (dimensions.Height * scale).round().max(1.0) as u32;
    let options = PdfPageRenderOptions::new()?;
    options.SetDestinationWidth(width)?;
    options.SetDestinationHeight(height)?;
    let stream = MemoryStream(InMemoryRandomAccessStream::new()?);
    page.0
        .RenderWithOptionsToStreamAsync(&stream.0, &options)?
        .get()
        .context("渲染 PDF 页面")?;
    stream.0.Seek(0)?;
    let decoder = BitmapDecoder::CreateAsync(&stream.0)?.get()?;
    let decoded_width = decoder.PixelWidth()?;
    let decoded_height = decoder.PixelHeight()?;
    // Windows' PDF renderer can apply the current display scale to its output.
    // Enforce the caller's edge limit while decoding, independent of desktop DPI.
    pixel_len(decoded_width, decoded_height)?;
    let scale = (max_edge / decoded_width.max(decoded_height) as f32).min(1.0);
    let width = (decoded_width as f32 * scale).round().max(1.0) as u32;
    let height = (decoded_height as f32 * scale).round().max(1.0) as u32;
    let expected = pixel_len(width, height)?;
    let transform = BitmapTransform::new()?;
    transform.SetScaledWidth(width)?;
    transform.SetScaledHeight(height)?;
    let provider = decoder
        .GetPixelDataTransformedAsync(
            BitmapPixelFormat::Rgba8,
            BitmapAlphaMode::Straight,
            &transform,
            ExifOrientationMode::IgnoreExifOrientation,
            ColorManagementMode::ColorManageToSRgb,
        )?
        .get()?;
    let rgba = provider.DetachPixelData()?.to_vec();
    if rgba.len() != expected {
        bail!("PDF 页面像素大小无效");
    }
    Ok((
        Bitmap {
            width,
            height,
            rgba,
        },
        total,
    ))
}
