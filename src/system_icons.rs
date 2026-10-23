//! Windows-owned icons, loaded only in a background/helper process.
//!
//! No icon extraction is performed on the GPUI render thread. Resource modules
//! are opened as data/images, never as executable libraries. The caller owns
//! caching and deadlines; all Windows handles stay local to this call.
use anyhow::{Context, Result, bail};
use std::{
    mem::size_of,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    ptr,
};
use windows::{
    Win32::{
        Foundation::{FreeLibrary, HINSTANCE, HMODULE},
        Graphics::Gdi::*,
        Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL},
        System::{LibraryLoader::*, SystemInformation::GetSystemDirectoryW},
        UI::{Shell::*, WindowsAndMessaging::*},
    },
    core::{Interface, PCWSTR},
};

#[derive(Clone, Debug)]
pub struct IconBitmap {
    pub width: u32,
    pub height: u32,
    /// Straight (not premultiplied) sRGB RGBA, top row first.
    pub rgba: Vec<u8>,
}
fn trim_icon_padding(icon: IconBitmap) -> IconBitmap {
    let Some(image) = image::RgbaImage::from_raw(icon.width, icon.height, icon.rgba.clone()) else {
        return icon;
    };
    let (mut left, mut top, mut right, mut bottom) = (icon.width, icon.height, 0, 0);
    for (x, y, pixel) in image.enumerate_pixels() {
        if pixel[3] > 16 {
            left = left.min(x);
            top = top.min(y);
            right = right.max(x + 1);
            bottom = bottom.max(y + 1);
        }
    }
    if right <= left || bottom <= top {
        return icon;
    }
    let crop = image::imageops::crop_imm(&image, left, top, right - left, bottom - top).to_image();
    let scale =
        (icon.width.min(icon.height) as f32 * 0.92) / crop.width().max(crop.height()) as f32;
    let resized = image::imageops::resize(
        &crop,
        (crop.width() as f32 * scale).round().max(1.) as u32,
        (crop.height() as f32 * scale).round().max(1.) as u32,
        image::imageops::FilterType::Lanczos3,
    );
    let mut canvas = image::RgbaImage::new(icon.width, icon.height);
    image::imageops::overlay(
        &mut canvas,
        &resized,
        ((icon.width - resized.width()) / 2) as i64,
        ((icon.height - resized.height()) / 2) as i64,
    );
    IconBitmap {
        width: icon.width,
        height: icon.height,
        rgba: canvas.into_raw(),
    }
}

fn wide(value: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(Some(0)).collect()
}

fn pixels(value: u32) -> Result<u32> {
    if !(8..=256).contains(&value) {
        bail!("icon size must be 8..=256 pixels");
    }
    Ok(value)
}

/// STA apartment for Shell icon handlers that are COM servers. Returns `None`
/// when the thread already lives in an apartment (MTA or STA), so the caller
/// never changes an existing COM configuration.
fn sta_apartment() -> Option<Apartment> {
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
        .is_ok()
        .then_some(Apartment)
}
struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { windows::Win32::System::Com::CoUninitialize() };
    }
}

struct Icon(HICON);
impl Drop for Icon {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyIcon(self.0);
        }
    }
}
struct Module(HMODULE);
impl Drop for Module {
    fn drop(&mut self) {
        unsafe {
            let _ = FreeLibrary(self.0);
        }
    }
}

/// Folders can define a custom class/icon in desktop.ini (for example
/// `C:\Windows\Offline Web Pages`). Only those folders need a per-item Shell
/// lookup; ordinary folders keep the cheap cached stock folder icon.
pub fn folder_has_custom_icon(path: &Path) -> bool {
    let Ok(data) = std::fs::read(path.join("desktop.ini")) else {
        return false;
    };
    let ascii = String::from_utf8_lossy(&data).to_ascii_lowercase();
    if ["iconresource", "iconfile", "clsid="]
        .iter()
        .any(|needle| ascii.contains(needle))
    {
        return true;
    }
    if data.len() % 2 == 0 {
        let units: Vec<u16> = data
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        let utf16 = String::from_utf16_lossy(&units).to_ascii_lowercase();
        return ["iconresource", "iconfile", "clsid="]
            .iter()
            .any(|needle| utf16.contains(needle));
    }
    false
}

/// Obtain Explorer's associated file-type icon without reading normal files.
/// Executables, shortcuts and .ico files retain their per-item Shell icon.
pub fn file_icon(path: &Path, directory: bool, size: u32) -> Result<IconBitmap> {
    // Internet shortcuts (`.url` with `IconFile=`) and several shell handlers are
    // COM servers. Without an apartment the Shell silently returns the generic
    // page for them, so ask for one STA apartment when this thread has none. A
    // thread already in an MTA keeps working as before.
    let _apartment = sta_apartment();
    let size = pixels(size)?;
    let shell_root = path.to_string_lossy().starts_with("mtp-root:");
    if directory {
        if shell_root {
            // Request the portable-device icon only for the device root; child
            // folders use the stock folder icon below.
            if let Ok(icon) = shell_image(path, true, size, false) {
                return Ok(icon);
            }
        }
        if path.parent().is_none() && path.has_root() {
            // Let the Shell supply the current Windows/BitLocker/optical overlays.
            if let Ok(icon) = shell_image(path, true, size, false) {
                return Ok(icon);
            }
            return stock_icon(SIID_DRIVEFIXED, size);
        }
        if path.is_absolute() && folder_has_custom_icon(path) {
            // Only reached for a folder requested by its real path (drive roots and
            // other per-item callers); ordinary folders share one attribute icon.
            if let Ok(icon) = shell_image(path, true, size, false) {
                return Ok(icon);
            }
        }
        // Preserve the native icons of the user's actual known folders, even
        // when OneDrive or a policy has redirected them to a different drive.
        if let Some(name) = known_folder_name(path) {
            if let Ok(icon) = ui_icon(name, size) {
                return Ok(icon);
            }
        }
        return stock_icon(SIID_FOLDER, size);
    }
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let per_item = shell_root
        || (path.is_absolute()
            && matches!(extension.as_str(), "exe" | "lnk" | "url" | "ico" | "cur"));
    let lookup = if per_item {
        path.to_path_buf()
    } else {
        PathBuf::from(if extension.is_empty() {
            "file".into()
        } else {
            format!("file.{extension}")
        })
    };
    if size > 32 {
        if let Ok(icon) = shell_image(&lookup, false, size, !per_item) {
            return Ok(icon);
        }
    }
    let name = wide(&lookup);
    let mut info = SHFILEINFOW::default();
    let mut flags = SHGFI_ICON
        | if size <= 16 {
            SHGFI_SMALLICON
        } else {
            SHGFI_LARGEICON
        };
    if !per_item {
        flags |= SHGFI_USEFILEATTRIBUTES;
    }
    if matches!(extension.as_str(), "lnk" | "url") {
        flags |= SHGFI_LINKOVERLAY | SHGFI_ADDOVERLAYS;
    }
    let result = unsafe {
        SHGetFileInfoW(
            PCWSTR(name.as_ptr()),
            FILE_ATTRIBUTE_NORMAL,
            Some(&mut info),
            size_of::<SHFILEINFOW>() as u32,
            flags,
        )
    };
    if result == 0 || info.hIcon.is_invalid() {
        if !info.hIcon.is_invalid() {
            drop(Icon(info.hIcon));
        }
        bail!("Windows has no associated icon for {}", path.display());
    }
    let icon = Icon(info.hIcon);
    // Windows can have no registered default app for media types. Keep their
    // native Shell media stock icon instead of showing an unassociated sheet.
    if generic_file_index() == Some(info.iIcon) {
        let fallback = match extension.as_str() {
            "mp4" | "m4v" | "webm" | "mov" | "mkv" | "avi" | "wmv" | "mpeg" | "mpg" => {
                Some(SIID_VIDEOFILES)
            }
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "heic" | "tif" | "tiff" => {
                Some(SIID_IMAGEFILES)
            }
            "mp3" | "wav" | "flac" | "aac" | "m4a" | "ogg" | "wma" => Some(SIID_AUDIOFILES),
            _ => None,
        };
        if let Some(id) = fallback {
            if let Ok(bitmap) = stock_icon(id, size) {
                return Ok(bitmap);
            }
        }
    }
    bitmap(icon.0, size)
}

/// Resolve the associated application's executable in the helper process.
/// A file-type icon can differ from the application's own taskbar icon.
pub fn associated_app_icon(path: &Path, size: u32) -> Result<IconBitmap> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    let association = wide(format!(".{extension}"));
    let mut executable = vec![0u16; 32768];
    let mut length = executable.len() as u32;
    unsafe {
        AssocQueryStringW(
            ASSOCF_NONE,
            ASSOCSTR_EXECUTABLE,
            PCWSTR(association.as_ptr()),
            PCWSTR::null(),
            Some(windows::core::PWSTR(executable.as_mut_ptr())),
            &mut length,
        )
        .ok()?;
    }
    let end = executable
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(executable.len());
    let executable = PathBuf::from(String::from_utf16_lossy(&executable[..end]));
    file_icon(&executable, false, size)
}

fn shell_item_icon(path: &Path, size: u32) -> Result<IconBitmap> {
    use windows::Win32::{
        Foundation::SIZE,
        System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
        UI::Shell::{IShellItemImageFactory, SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY},
    };
    let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
    let result = (|| {
        let item = crate::shell_folder::item(path)?;
        let factory: IShellItemImageFactory = item.cast()?;
        let hbitmap = unsafe {
            factory.GetImage(
                SIZE {
                    cx: size as i32,
                    cy: size as i32,
                },
                SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK,
            )?
        };
        let mut metrics = BITMAP::default();
        let copied = unsafe {
            GetObjectW(
                HGDIOBJ(hbitmap.0),
                size_of::<BITMAP>() as i32,
                Some(&mut metrics as *mut _ as *mut _),
            )
        };
        if copied == 0 || metrics.bmWidth <= 0 || metrics.bmHeight <= 0 {
            let _ = unsafe { DeleteObject(HGDIOBJ(hbitmap.0)) };
            bail!("Shell icon has no bitmap size");
        }
        let mask = unsafe { CreateBitmap(metrics.bmWidth, metrics.bmHeight, 1, 1, None) };
        let info = ICONINFO {
            fIcon: true.into(),
            hbmMask: mask,
            hbmColor: hbitmap,
            ..Default::default()
        };
        let icon = unsafe { CreateIconIndirect(&info) };
        let _ = unsafe { DeleteObject(HGDIOBJ(hbitmap.0)) };
        if !mask.is_invalid() {
            let _ = unsafe { DeleteObject(HGDIOBJ(mask.0)) };
        }
        let icon = Icon(icon?);
        bitmap(icon.0, size)
    })();
    if initialized {
        unsafe { CoUninitialize() };
    }
    result
}

fn shell_image(path: &Path, directory: bool, size: u32, attributes: bool) -> Result<IconBitmap> {
    use windows::Win32::UI::Controls::{IImageList, ILD_TRANSPARENT};
    if crate::shell_folder::is_shell_path(path) {
        if let Ok(icon) = shell_item_icon(path, size) {
            return Ok(icon);
        }
    }
    let mut info = SHFILEINFOW::default();
    let mut flags = SHGFI_SYSICONINDEX | SHGFI_ICON | SHGFI_ADDOVERLAYS | SHGFI_OVERLAYINDEX;
    if attributes {
        flags |= SHGFI_USEFILEATTRIBUTES;
    }
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("lnk") || e.eq_ignore_ascii_case("url"))
    {
        flags |= SHGFI_LINKOVERLAY;
    }
    if size <= 16 {
        flags |= SHGFI_SMALLICON;
    }
    let pidl = if crate::shell_folder::is_shell_path(path) {
        Some(crate::shell_folder::pidl(path)?)
    } else {
        None
    };
    if pidl.is_some() {
        flags |= SHGFI_PIDL;
    }
    let name = wide(path);
    let name_ptr = pidl
        .as_ref()
        .map(|pidl| pidl.as_ptr() as *const u16)
        .unwrap_or_else(|| name.as_ptr());
    let result = unsafe {
        SHGetFileInfoW(
            PCWSTR(name_ptr),
            if directory {
                FILE_ATTRIBUTE_DIRECTORY
            } else {
                FILE_ATTRIBUTE_NORMAL
            },
            Some(&mut info),
            size_of::<SHFILEINFOW>() as u32,
            flags,
        )
    };
    anyhow::ensure!(result != 0, "Shell icon unavailable");
    let small = Icon(info.hIcon);
    let extension = path
        .extension()
        .map(|value| value.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    // An internet shortcut's icon comes from the file itself (`IconFile=`) and only
    // exists as an icon handle: its image-list index points at a *different* icon in
    // the extra-large list, which is why `.url` files used to appear as blank pages.
    // `.ico` and `.cur` are their own artwork for the same reason.
    let file_icon = path.is_absolute() && matches!(extension.as_str(), "url" | "ico" | "cur");
    if file_icon && !small.0.is_invalid() {
        return bitmap(small.0, size);
    }
    if size <= 32 && !small.0.is_invalid() {
        return bitmap(small.0, size);
    }
    let index = info.iIcon & 0x00ffffff;
    // A shortcut whose target icon cannot be placed in the image lists comes back
    // with index 0 (the generic page), so keep its handle instead.
    let self_icon = path.is_absolute() && extension == "lnk";
    if index == 0 && self_icon && !small.0.is_invalid() {
        return bitmap(small.0, size);
    }
    let list: IImageList = unsafe {
        SHGetImageList(if size > 48 {
            SHIL_JUMBO
        } else {
            SHIL_EXTRALARGE
        } as i32)?
    };
    let overlay = ((info.iIcon as u32 >> 24) & 0xff) << 8;
    let drawn = unsafe { list.GetIcon(index, ILD_TRANSPARENT.0 | overlay) };
    let icon = match drawn {
        Ok(icon) => Icon(icon),
        // The list has no such entry: fall back to the icon handle we received.
        Err(_) if !small.0.is_invalid() => small,
        Err(error) => return Err(error.into()),
    };
    bitmap(icon.0, size)
}

fn generic_file_index() -> Option<i32> {
    use std::sync::OnceLock;
    static INDEX: OnceLock<Option<i32>> = OnceLock::new();
    *INDEX.get_or_init(|| {
        let mut info = SHSTOCKICONINFO {
            cbSize: size_of::<SHSTOCKICONINFO>() as u32,
            ..Default::default()
        };
        unsafe {
            SHGetStockIconInfo(SIID_DOCNOASSOC, SHGSI_SYSICONINDEX, &mut info).ok()?;
        }
        Some(info.iSysImageIndex)
    })
}

pub fn windows_11() -> bool {
    static VERSION: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *VERSION.get_or_init(|| unsafe {
        use windows::Win32::System::SystemInformation::{GetVersionExW, OSVERSIONINFOW};
        let mut version = OSVERSIONINFOW {
            dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
            ..Default::default()
        };
        GetVersionExW(&mut version).is_ok() && version.dwBuildNumber >= 22000
    })
}
pub fn has_ui_icon(name: &str) -> bool {
    matches!(
        name,
        "network"
            | "pin"
            | "recycle-bin"
            | "folder"
            | "folder-open"
            | "drive"
            | "search"
            | "rename"
            | "delete"
            | "computer"
            | "computer-colored"
            | "desktop"
            | "video"
            | "video-file"
            | "image"
            | "image-file"
            | "audio"
            | "audio-file"
            | "file"
            | "up"
            | "refresh"
            | "new-folder"
            | "cut"
            | "copy"
            | "paste"
            | "preview"
            | "home"
            | "downloads"
            | "documents"
            | "pictures"
            | "music"
            | "videos"
            | "close"
    )
}
pub fn ui_icon(name: &str, size: u32) -> Result<IconBitmap> {
    let size = pixels(size)?;
    match name {
        "desktop" => return resource_icon("imageres.dll", 183, size),
        "network" => return resource_icon("imageres.dll", 25, size),
        "pin" => return resource_icon("imageres.dll", 5100, size).map(trim_icon_padding),
        "computer" | "computer-colored" => return resource_icon("imageres.dll", 109, size),
        _ => {}
    }
    let stock = match name {
        "recycle-bin" => Some(SIID_RECYCLER),
        "folder" => Some(SIID_FOLDER),
        "folder-open" => Some(SIID_FOLDEROPEN),
        "drive" => Some(SIID_DRIVEFIXED),
        "search" => Some(SIID_FIND),
        "rename" => Some(SIID_RENAME),
        "delete" => Some(SIID_DELETE),
        "computer" | "computer-colored" | "desktop" => Some(SIID_DESKTOPPC),
        "video" | "video-file" => Some(SIID_VIDEOFILES),
        "image" | "image-file" => Some(SIID_IMAGEFILES),
        "audio" | "audio-file" => Some(SIID_AUDIOFILES),
        "file" => Some(SIID_DOCNOASSOC),
        _ => None,
    };
    if let Some(id) = stock {
        return stock_icon(id, size);
    }
    let (dll, id) = match name {
        "up" => ("shell32.dll", 16817),
        "refresh" => ("shell32.dll", 16739),
        "new-folder" => ("shell32.dll", 16804),
        "cut" => ("shell32.dll", 16762),
        "copy" => ("shell32.dll", 243),
        "paste" => ("shell32.dll", 16763),
        "preview" => ("shell32.dll", 16757),
        "home" => ("shell32.dll", 51380),
        "downloads" => ("imageres.dll", 184),
        "documents" => ("imageres.dll", 112),
        "pictures" => ("imageres.dll", 113),
        "music" => ("imageres.dll", 108),
        "videos" => ("imageres.dll", 189),
        "close" => ("imageres.dll", 5102),
        _ => bail!("no verified Windows icon resource for {name}"),
    };
    resource_icon(dll, id, size)
}

fn stock_icon(id: SHSTOCKICONID, size: u32) -> Result<IconBitmap> {
    let mut info = SHSTOCKICONINFO {
        cbSize: size_of::<SHSTOCKICONINFO>() as u32,
        ..Default::default()
    };
    unsafe {
        SHGetStockIconInfo(
            id,
            SHGSI_ICON
                | SHGSI_SYSICONINDEX
                | if size <= 16 {
                    SHGSI_SMALLICON
                } else {
                    SHGSI_LARGEICON
                },
            &mut info,
        )?;
    }
    if info.hIcon.is_invalid() {
        bail!("Windows returned an empty stock icon");
    }
    let icon = Icon(info.hIcon);
    if size > 32 {
        use windows::Win32::UI::Controls::{IImageList, ILD_TRANSPARENT};
        if let Ok(list) = unsafe {
            SHGetImageList::<IImageList>(if size > 48 {
                SHIL_JUMBO
            } else {
                SHIL_EXTRALARGE
            } as i32)
        } {
            if let Ok(handle) = unsafe { list.GetIcon(info.iSysImageIndex, ILD_TRANSPARENT.0) } {
                return bitmap(Icon(handle).0, size);
            }
        }
    }
    bitmap(icon.0, size)
}

fn system_directory() -> Result<PathBuf> {
    let mut name = [0u16; 32768];
    let length = unsafe { GetSystemDirectoryW(Some(&mut name)) } as usize;
    if length == 0 || length >= name.len() {
        bail!("cannot locate the Windows system directory");
    }
    Ok(PathBuf::from(String::from_utf16_lossy(&name[..length])))
}

fn resource_icon(dll: &str, id: u16, size: u32) -> Result<IconBitmap> {
    let system = system_directory()?;
    let root = system
        .parent()
        .context("Windows system directory has no parent")?;
    // Modern Windows moves neutral resources into SystemResources/*.mun.
    // Try the public DLL first so Windows can resolve its normal MUI resources.
    for path in crate::fluent_icons::resource_module(dll)
        .into_iter()
        .chain([
            system.join(dll),
            root.join("SystemResources").join(format!("{dll}.mun")),
        ])
    {
        let path = wide(path);
        let Ok(module) = (unsafe {
            LoadLibraryExW(
                PCWSTR(path.as_ptr()),
                None,
                LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE,
            )
        }) else {
            continue;
        };
        let module = Module(module);
        if let Ok(handle) = unsafe {
            LoadImageW(
                Some(HINSTANCE(module.0.0)),
                PCWSTR(id as usize as *const u16),
                IMAGE_ICON,
                size as i32,
                size as i32,
                IMAGE_FLAGS(0),
            )
        } {
            let icon = Icon(HICON(handle.0));
            return bitmap(icon.0, size);
        }
    }
    bail!("Windows resource {dll},#{id} is unavailable")
}

fn known_folder_name(path: &Path) -> Option<&'static str> {
    use std::sync::OnceLock;
    static FOLDERS: OnceLock<Vec<(&'static str, PathBuf)>> = OnceLock::new();
    let folders = FOLDERS.get_or_init(|| {
        [
            ("desktop", FOLDERID_Desktop),
            ("downloads", FOLDERID_Downloads),
            ("documents", FOLDERID_Documents),
            ("pictures", FOLDERID_Pictures),
            ("music", FOLDERID_Music),
            ("videos", FOLDERID_Videos),
        ]
        .into_iter()
        .filter_map(|(name, id)| unsafe {
            let result = SHGetKnownFolderPath(&id, KF_FLAG_DONT_VERIFY, None).ok()?;
            let text = result.to_string();
            windows::Win32::System::Com::CoTaskMemFree(Some(result.0.cast()));
            text.ok().map(|path| (name, PathBuf::from(path)))
        })
        .collect()
    });
    let requested = path.as_os_str().to_string_lossy();
    folders
        .iter()
        .find(|(_, folder)| {
            folder
                .as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(&requested)
        })
        .map(|(name, _)| *name)
}

/// Draw on black and white DIBs to recover both modern 32-bit alpha and legacy
/// AND-mask transparency. This also avoids the all-opaque black backgrounds
/// produced by treating every older HICON as a 32-bit alpha bitmap.
fn bitmap(icon: HICON, size: u32) -> Result<IconBitmap> {
    let black = draw(icon, size, 0)?;
    let white = draw(icon, size, 255)?;
    let mut rgba = vec![0; black.len()];
    for ((black, white), out) in black
        .chunks_exact(4)
        .zip(white.chunks_exact(4))
        .zip(rgba.chunks_exact_mut(4))
    {
        let delta = (0..3)
            .map(|c| white[c].saturating_sub(black[c]))
            .max()
            .unwrap_or(255);
        let alpha = 255u32 - u32::from(delta);
        out[3] = alpha as u8;
        if alpha > 0 {
            for c in 0..3 {
                out[c] = ((u32::from(black[2 - c]) * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
    if !rgba.chunks_exact(4).any(|px| px[3] != 0) {
        bail!("Windows icon is entirely transparent");
    }
    Ok(IconBitmap {
        width: size,
        height: size,
        rgba: crate::icon_layout::center_legacy_icon(size, rgba),
    })
}

struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
}
impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            let _ = SelectObject(self.dc, self.previous);
            let _ = DeleteObject(HGDIOBJ(self.bitmap.0));
            let _ = DeleteDC(self.dc);
        }
    }
}

fn draw(icon: HICON, size: u32, background: u8) -> Result<Vec<u8>> {
    unsafe {
        let dc = CreateCompatibleDC(None);
        if dc.is_invalid() {
            bail!("cannot create icon drawing context");
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size as i32,
                biHeight: -(size as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = ptr::null_mut();
        let bitmap = match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(bitmap) => bitmap,
            Err(err) => {
                let _ = DeleteDC(dc);
                return Err(err.into());
            }
        };
        let previous = SelectObject(dc, HGDIOBJ(bitmap.0));
        let _surface = Surface {
            dc,
            bitmap,
            previous,
        };
        if bits.is_null() {
            bail!("icon DIB has no pixel storage");
        }
        let length = (size * size * 4) as usize;
        let slice = std::slice::from_raw_parts_mut(bits.cast::<u8>(), length);
        slice.fill(background);
        DrawIconEx(dc, 0, 0, icon, size as i32, size as i32, 0, None, DI_NORMAL)?;
        let _ = GdiFlush();
        Ok(slice.to_vec())
    }
}
