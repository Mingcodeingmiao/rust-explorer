//! Read-only Shell namespace enumeration for portable devices (MTP/Android,
//! cameras) and other non-filesystem folders. Runs in the killable helper
//! process; the UI never instantiates a namespace extension itself.
use crate::fs::Entry;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use windows::{
    Win32::{
        Foundation::{FILETIME, PROPERTYKEY},
        System::{
            Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize},
            SystemServices::{SFGAO_FLAGS, SFGAO_FOLDER, SFGAO_HIDDEN, SFGAO_LINK, SFGAO_SYSTEM},
        },
        UI::Shell::{
            BHID_EnumItems, Common::ITEMIDLIST, IEnumShellItems, ILGetSize, IShellItem,
            IShellItem2, SHCreateItemFromIDList, SHCreateItemFromParsingName,
            SHGetIDListFromObject, SIGDN_DESKTOPABSOLUTEPARSING, SIGDN_FILESYSPATH,
            SIGDN_NORMALDISPLAY,
        },
    },
    core::{GUID, Interface, PCWSTR, PWSTR},
};

const THIS_PC: &str = "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}";
const FMTID_STORAGE: GUID = GUID::from_u128(0xb725f130_47ef_101a_a5f1_02608c9eebac);
const PKEY_SIZE: PROPERTYKEY = PROPERTYKEY {
    fmtid: FMTID_STORAGE,
    pid: 12,
};
const PKEY_DATE_MODIFIED: PROPERTYKEY = PROPERTYKEY {
    fmtid: FMTID_STORAGE,
    pid: 14,
};
const PKEY_DATE_CREATED: PROPERTYKEY = PROPERTYKEY {
    fmtid: FMTID_STORAGE,
    pid: 15,
};
const PKEY_ITEM_TYPE_TEXT: PROPERTYKEY = PROPERTYKEY {
    fmtid: FMTID_STORAGE,
    pid: 4,
};
const FMTID_VOLUME: GUID = GUID::from_u128(0x9b174b35_40ff_11d2_a27e_00c04fc30871);
const PKEY_FREE_SPACE: PROPERTYKEY = PROPERTYKEY {
    fmtid: FMTID_VOLUME,
    pid: 2,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Listing {
    pub name: String,
    pub entries: Vec<Entry>,
    #[serde(default)]
    pub parents: Vec<(String, PathBuf)>,
}

struct Com(bool);
impl Com {
    fn new() -> Self {
        // RPC_E_CHANGED_MODE means COM is already initialized; use it as-is.
        Self(unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok())
    }
}
impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

fn wide(value: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    value.as_os_str().encode_wide().chain(Some(0)).collect()
}

fn encode_pidl_with_prefix(pidl: &[u8], prefix: &str) -> PathBuf {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(prefix.len() + pidl.len() * 2);
    text.push_str(prefix);
    for byte in pidl {
        text.push(HEX[(byte >> 4) as usize] as char);
        text.push(HEX[(byte & 0x0f) as usize] as char);
    }
    PathBuf::from(text)
}

fn encode_pidl(pidl: &[u8]) -> PathBuf {
    encode_pidl_with_prefix(pidl, "mtp:")
}

/// Device roots keep a distinct prefix so their Shell icon can be requested
/// once, while child folders use the cheap stock folder icon.
fn encode_device_pidl(pidl: &[u8]) -> PathBuf {
    encode_pidl_with_prefix(pidl, "mtp-root:")
}

fn decode_pidl(path: &Path) -> Option<Vec<u8>> {
    let text = path.to_string_lossy();
    let text = text
        .strip_prefix("mtp-root:")
        .or_else(|| text.strip_prefix("recycle:"))
        .or_else(|| text.strip_prefix("mtp:"))?;
    if text.len() % 2 != 0 {
        return None;
    }
    let mut bytes = Vec::with_capacity(text.len() / 2);
    for pair in text.as_bytes().chunks_exact(2) {
        let high = (pair[0] as char).to_digit(16)?;
        let low = (pair[1] as char).to_digit(16)?;
        bytes.push((high * 16 + low) as u8);
    }
    Some(bytes)
}

unsafe fn item_pidl(item: &IShellItem) -> Result<Vec<u8>> {
    let pidl = unsafe { SHGetIDListFromObject(item) }.context("无法读取 Shell 项目标识")?;
    if pidl.is_null() {
        anyhow::bail!("Shell 项目没有可用标识");
    }
    let size = unsafe { ILGetSize(Some(pidl)) } as usize;
    let bytes = unsafe { std::slice::from_raw_parts(pidl as *const u8, size) }.to_vec();
    unsafe { CoTaskMemFree(Some(pidl as _)) };
    Ok(bytes)
}

/// Build the Shell namespace chain (device root … current folder) so the
/// breadcrumb can show every folder. Stops before This PC or a filesystem
/// ancestor.
fn ancestors(root: &IShellItem) -> Result<Vec<(String, PathBuf)>> {
    let mut chain = Vec::new();
    let mut current = root.clone();
    for _ in 0..64 {
        let name = unsafe { display_name(&current, SIGDN_NORMALDISPLAY) }.unwrap_or_default();
        let bytes = unsafe { item_pidl(&current) }?;
        chain.push((name, encode_pidl(&bytes)));
        let Ok(parent) = (unsafe { current.GetParent() }) else {
            break;
        };
        if let Some(parsing) = unsafe { display_name(&parent, SIGDN_DESKTOPABSOLUTEPARSING) } {
            if parsing.eq_ignore_ascii_case(THIS_PC)
                || parsing.eq_ignore_ascii_case("shell:MyComputerFolder")
            {
                break;
            }
        }
        if unsafe { display_name(&parent, SIGDN_FILESYSPATH) }.is_some() {
            break;
        }
        let parent_bytes = unsafe { item_pidl(&parent) }?;
        if parent_bytes == bytes {
            break;
        }
        current = parent;
    }
    chain.reverse();
    Ok(chain)
}

/// Whether a Shell namespace item is a folder.
pub fn is_folder(path: &Path) -> bool {
    let _com = Com::new();
    let Ok(item) = item(path) else {
        return false;
    };
    unsafe { item.GetAttributes(SFGAO_FOLDER) }
        .map(|attributes| attributes.contains(SFGAO_FOLDER))
        .unwrap_or(false)
}

/// Resolve a stored parsing name or an `mtp:` PIDL path to a Shell item.
pub fn item(path: &Path) -> Result<IShellItem> {
    if let Some(bytes) = decode_pidl(path) {
        anyhow::ensure!(!bytes.is_empty(), "无效的 Shell 项目标识");
        return unsafe {
            SHCreateItemFromIDList(bytes.as_ptr() as *const ITEMIDLIST)
                .context("无法恢复便携设备项目")
        };
    }
    unsafe {
        SHCreateItemFromParsingName(PCWSTR(wide(path).as_ptr()), None)
            .with_context(|| format!("Windows 无法解析 {}", path.display()))
    }
}

pub fn pidl(path: &Path) -> Result<Vec<u8>> {
    if let Some(bytes) = decode_pidl(path) {
        return Ok(bytes);
    }
    let _com = Com::new();
    let item = item(path)?;
    unsafe { item_pidl(&item) }
}

/// Open a portable-device item through ShellExecuteEx with its PIDL.
pub fn properties(path: &Path) -> Result<()> {
    let pidl = pidl(path)?;
    use windows::Win32::{
        Foundation::HWND,
        UI::{
            Shell::{
                SEE_MASK_IDLIST, SEE_MASK_INVOKEIDLIST, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW,
                ShellExecuteExW,
            },
            WindowsAndMessaging::SW_SHOWNORMAL,
        },
    };
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC | SEE_MASK_IDLIST | SEE_MASK_INVOKEIDLIST,
        lpVerb: windows::core::w!("properties"),
        lpIDList: pidl.as_ptr() as *mut _,
        nShow: SW_SHOWNORMAL.0,
        hwnd: HWND::default(),
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info) }.context("当前系统无法打开此便携设备项目的属性")?;
    Ok(())
}

pub fn open(path: &Path) -> Result<()> {
    let pidl = pidl(path)?;
    use windows::Win32::{
        Foundation::HWND,
        UI::{
            Shell::{SEE_MASK_IDLIST, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW},
            WindowsAndMessaging::SW_SHOWNORMAL,
        },
    };
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC | SEE_MASK_IDLIST,
        lpVerb: windows::core::w!("open"),
        lpIDList: pidl.as_ptr() as *mut _,
        nShow: SW_SHOWNORMAL.0,
        hwnd: HWND::default(),
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info) }.context("当前系统无法打开此便携设备项目")?;
    Ok(())
}

unsafe fn take_pwstr(value: PWSTR) -> String {
    let text = unsafe { value.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(value.0 as _)) };
    text
}

unsafe fn display_name(
    item: &IShellItem,
    kind: windows::Win32::UI::Shell::SIGDN,
) -> Option<String> {
    unsafe { item.GetDisplayName(kind) }
        .ok()
        .map(|value| unsafe { take_pwstr(value) })
        .filter(|value| !value.is_empty())
}

fn file_time(value: FILETIME) -> Option<SystemTime> {
    let ticks = ((value.dwHighDateTime as u64) << 32) | value.dwLowDateTime as u64;
    if ticks == 0 {
        return None;
    }
    let seconds = ticks / 10_000_000;
    if seconds < 11_644_473_600 {
        return None;
    }
    Some(UNIX_EPOCH + Duration::from_secs(seconds - 11_644_473_600))
}

fn item_entry(item: &IShellItem) -> Option<Entry> {
    let name = unsafe { display_name(item, SIGDN_NORMALDISPLAY) }?;
    let path = unsafe { item_pidl(item) }
        .map(|bytes| encode_pidl(&bytes))
        .or_else(|_| {
            unsafe { display_name(item, SIGDN_DESKTOPABSOLUTEPARSING) }
                .map(PathBuf::from)
                .ok_or_else(|| anyhow::anyhow!("Shell 项目没有可用标识"))
        })
        .unwrap_or_else(|_| PathBuf::from(&name));
    let attributes =
        unsafe { item.GetAttributes(SFGAO_FOLDER | SFGAO_HIDDEN | SFGAO_LINK | SFGAO_SYSTEM) }
            .unwrap_or_else(|_| SFGAO_FLAGS(0));
    let directory = attributes.contains(SFGAO_FOLDER);
    let mut size = 0;
    let mut total = None;
    let mut free = None;
    let mut modified = None;
    let mut created = None;
    let mut kind = String::new();
    if let Ok(item2) = item.cast::<IShellItem2>() {
        size = unsafe { item2.GetUInt64(&PKEY_SIZE) }.unwrap_or(0);
        if directory {
            let capacity = unsafe { item2.GetUInt64(&PKEY_SIZE) }.ok();
            let available = unsafe { item2.GetUInt64(&PKEY_FREE_SPACE) }.ok();
            if let (Some(capacity), Some(available)) = (capacity, available)
                && capacity > 0
            {
                total = Some(capacity);
                free = Some(available);
                kind = "磁盘驱动器".into();
            }
        }
        modified = unsafe { item2.GetFileTime(&PKEY_DATE_MODIFIED) }
            .ok()
            .and_then(file_time);
        created = unsafe { item2.GetFileTime(&PKEY_DATE_CREATED) }
            .ok()
            .and_then(file_time);
        if kind.is_empty() {
            kind = unsafe { item2.GetString(&PKEY_ITEM_TYPE_TEXT) }
                .ok()
                .map(|value| unsafe { take_pwstr(value) })
                .unwrap_or_default();
        }
    }
    if kind.is_empty() {
        kind = if directory {
            "文件夹".into()
        } else {
            Path::new(&name)
                .extension()
                .map(|value| format!("{} 文件", value.to_string_lossy().to_uppercase()))
                .unwrap_or_else(|| "文件".into())
        };
    }
    Some(Entry {
        recycle: None,
        path,
        name,
        directory,
        link: attributes.contains(SFGAO_LINK),
        hidden: attributes.contains(SFGAO_HIDDEN),
        system: attributes.contains(SFGAO_SYSTEM),
        size: if directory { 0 } else { size },
        total,
        free,
        modified,
        created,
        kind,
    })
}

fn enumerate_item(item: &IShellItem) -> Result<Vec<IShellItem>> {
    unsafe {
        let enumeration: IEnumShellItems = item
            .BindToHandler(None, &BHID_EnumItems)
            .context("Windows 无法枚举此位置")?;
        let mut result = Vec::new();
        loop {
            let mut batch = [None; 1];
            let mut fetched = 0;
            let _ = enumeration.Next(&mut batch, Some(&mut fetched));
            if fetched == 0 {
                break;
            }
            if let Some(item) = batch[0].take() {
                result.push(item);
            }
        }
        Ok(result)
    }
}

fn enumerate(path: &Path) -> Result<Vec<IShellItem>> {
    enumerate_item(&item(path)?)
}

/// Resolve an address-bar path. Filesystem paths are handled by the caller;
/// this resolves the `This PC\device\storage\folder` form used for portable
/// devices, plus `mtp:` and `mtp://` PIDL addresses.
pub fn resolve_display_path(text: &str) -> Result<PathBuf> {
    let text = text.trim().trim_matches('"').trim();
    if let Some(hex) = text.strip_prefix("mtp://") {
        let hex = hex.replace(['-', ' '], "");
        if hex.is_empty() || hex.len() % 2 != 0 {
            anyhow::bail!("无效的 MTP 地址");
        }
        let mut bytes = Vec::with_capacity(hex.len() / 2);
        for pair in hex.as_bytes().chunks_exact(2) {
            let high = (pair[0] as char).to_digit(16).context("无效的 MTP 地址")?;
            let low = (pair[1] as char).to_digit(16).context("无效的 MTP 地址")?;
            bytes.push((high * 16 + low) as u8);
        }
        return Ok(encode_pidl(&bytes));
    }
    if text.starts_with("mtp:") || text.starts_with("mtp-root:") {
        return Ok(PathBuf::from(text));
    }
    let _com = Com::new();
    let root = item(Path::new(THIS_PC))?;
    let root_name = unsafe { display_name(&root, SIGDN_NORMALDISPLAY) }.unwrap_or_default();
    let mut parts: Vec<&str> = text
        .split(['\\', '/'])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    if parts
        .first()
        .is_some_and(|part| part.eq_ignore_ascii_case(&root_name) || *part == "此电脑")
    {
        parts.remove(0);
    }
    if parts.is_empty() {
        return Ok(PathBuf::from(THIS_PC));
    }
    let mut current = root;
    let mut entered = 0;
    for part in parts {
        let children = enumerate_item(&current)?;
        let Some(child) = children.into_iter().find(|child| {
            unsafe { display_name(child, SIGDN_NORMALDISPLAY) }
                .is_some_and(|name| name.eq_ignore_ascii_case(part))
        }) else {
            anyhow::bail!("在便携设备中找不到“{part}”");
        };
        current = child;
        entered += 1;
    }
    let pidl = unsafe { item_pidl(&current) }?;
    Ok(if entered == 1 {
        encode_device_pidl(&pidl)
    } else {
        encode_pidl(&pidl)
    })
}

/// Enumerate a portable device or other Shell namespace folder.
pub fn list(path: &Path) -> Result<Listing> {
    let _com = Com::new();
    let root = item(path)?;
    let items = enumerate(path)?;
    let name = unsafe { display_name(&root, SIGDN_NORMALDISPLAY) }
        .or_else(|| {
            path.file_name()
                .map(|value| value.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "便携设备".into());
    let recycle = is_recycle_bin_path(path);
    let entries = items
        .iter()
        .filter_map(|item| item_entry(item).map(|entry| (item, entry)))
        .map(|(item, mut entry)| {
            if recycle {
                if let Ok(properties) = item.cast::<IShellItem2>() {
                    let mut origin = PROPERTYKEY::default();
                    let mut deleted = PROPERTYKEY::default();
                    use windows::Win32::UI::Shell::PropertiesSystem::PSGetPropertyKeyFromName;
                    let original_location = unsafe {
                        PSGetPropertyKeyFromName(
                            windows::core::w!("System.Recycle.DeletedFrom"),
                            &mut origin,
                        )
                        .ok()
                        .and_then(|_| properties.GetString(&origin).ok())
                        .map(|value| take_pwstr(value))
                        .unwrap_or_default()
                    };
                    let date = unsafe {
                        PSGetPropertyKeyFromName(
                            windows::core::w!("System.Recycle.DateDeleted"),
                            &mut deleted,
                        )
                        .ok()
                        .and_then(|_| properties.GetFileTime(&deleted).ok())
                        .and_then(file_time)
                    };
                    entry.recycle = Some(crate::fs::RecycleInfo {
                        original_location,
                        deleted: date,
                    });
                    // Recycled folders have a stored aggregate size. Do not
                    // reinterpret their size/free-space properties as a drive.
                    entry.size = unsafe { properties.GetUInt64(&PKEY_SIZE) }.unwrap_or(0);
                    entry.total = None;
                    entry.free = None;
                    if entry.directory {
                        entry.kind = "文件夹".into();
                    }
                }
                if let Some(filename) = Path::new(&entry.name).file_name() {
                    entry.name = filename.to_string_lossy().into_owned();
                }
                if let Some(bytes) = decode_pidl(&entry.path) {
                    entry.path = encode_pidl_with_prefix(&bytes, "recycle:");
                }
                // The bin is a user-facing listing even when its physical storage
                // has hidden/system attributes.
                entry.hidden = false;
                entry.system = false;
            }
            entry
        })
        .collect::<Vec<_>>();
    let mut parents = ancestors(&root).unwrap_or_default();
    if recycle {
        if let Ok(bin) =
            item(Path::new("shell:RecycleBinFolder")).and_then(|bin| unsafe { item_pidl(&bin) })
        {
            if let Some(index) = parents
                .iter()
                .position(|(_, p)| decode_pidl(p).as_ref() == Some(&bin))
            {
                parents.drain(..index);
            }
        }
        for (_, p) in &mut parents {
            if let Some(bytes) = decode_pidl(p) {
                *p = encode_pidl_with_prefix(&bytes, "recycle:");
            }
        }
    }
    Ok(Listing {
        name,
        entries,
        parents,
    })
}

/// Items shown under This PC that do not expose a filesystem path. This
/// includes MTP phones, cameras and other portable devices.
pub fn device_paths() -> Result<Vec<(String, PathBuf)>> {
    let _com = Com::new();
    let this_pc = Path::new(THIS_PC);
    let mut result = Vec::new();
    for item in enumerate(this_pc)? {
        // Only non-filesystem items are portable devices. This also filters
        // cloud-drive placeholders that expose a normal local path.
        if unsafe { display_name(&item, SIGDN_FILESYSPATH) }.is_some() {
            continue;
        }
        let Some(name) = (unsafe { display_name(&item, SIGDN_NORMALDISPLAY) }) else {
            continue;
        };
        let parsing =
            unsafe { display_name(&item, SIGDN_DESKTOPABSOLUTEPARSING) }.unwrap_or_default();
        if parsing.contains("F02C1A0D-BE21-4350-88B0-7367FC96EF3C") // Network
            || parsing.contains("031E4825-7B94-4dc3-B131-E946B44C8DD5") // Libraries
            || parsing.contains("26EE0668-A00A-44D7-9371-BEB064C98683")
        // Control Panel
        {
            continue;
        }
        let Ok(pidl) = (unsafe { item_pidl(&item) }) else {
            continue;
        };
        result.push((name, encode_device_pidl(&pidl)));
    }
    Ok(result)
}

/// Shell parsing names used by portable devices are not filesystem paths.
pub fn is_shell_path(path: &Path) -> bool {
    let value = path.to_string_lossy();
    value.starts_with("mtp:")
        || value.starts_with("recycle:")
        || value.starts_with("mtp-root:")
        || value.starts_with("::{")
        || value.starts_with("shell:")
        || value.starts_with("\\\\?\\")
        || value.starts_with("\\\\.\\")
}
pub fn is_recycle_bin_path(path: &Path) -> bool {
    let value = path.to_string_lossy();
    value.eq_ignore_ascii_case("shell:RecycleBinFolder")
        || value.eq_ignore_ascii_case("::{645FF040-5081-101B-9F08-00AA002F954E}")
        || value.starts_with("recycle:")
}

/// PIDL aliases for a device root and its breadcrumb refer to the same item.
pub fn parent_from_chain(path: &Path, chain: &[(String, PathBuf)]) -> Option<PathBuf> {
    let current = decode_pidl(path);
    let index = chain.iter().position(|(_, candidate)| {
        candidate == path || (current.is_some() && decode_pidl(candidate) == current)
    })?;
    if index == 0 {
        return Some(crate::volumes::computer());
    }
    let parent = &chain[index - 1].1;
    if is_recycle_bin_path(path) {
        return Some(parent.clone());
    }
    Some(if index == 1 {
        decode_pidl(parent)
            .map(|bytes| encode_device_pidl(&bytes))
            .unwrap_or_else(|| parent.clone())
    } else {
        parent.clone()
    })
}

/// Resolve a parent through Shell, never by parsing a filesystem separator.
pub fn parent(path: &Path) -> Result<PathBuf> {
    if path
        .to_string_lossy()
        .eq_ignore_ascii_case("shell:RecycleBinFolder")
    {
        return Ok(crate::volumes::computer());
    }
    let _com = Com::new();
    let current = item(path)?;
    let parent = unsafe { current.GetParent() }?;
    let parsing =
        unsafe { display_name(&parent, SIGDN_DESKTOPABSOLUTEPARSING) }.unwrap_or_default();
    if parsing.eq_ignore_ascii_case(THIS_PC)
        || parsing.eq_ignore_ascii_case("shell:MyComputerFolder")
    {
        return Ok(crate::volumes::computer());
    }
    if let Some(path) = unsafe { display_name(&parent, SIGDN_FILESYSPATH) } {
        return Ok(PathBuf::from(path));
    }
    let chain = ancestors(&parent)?;
    let bytes = unsafe { item_pidl(&parent) }?;
    if is_recycle_bin_path(path) {
        return Ok(encode_pidl_with_prefix(&bytes, "recycle:"));
    }
    Ok(if chain.len() == 1 {
        encode_device_pidl(&bytes)
    } else {
        encode_pidl(&bytes)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn up_uses_shell_ancestry_and_restores_device_root_alias() {
        let root = encode_pidl(&[2, 0, 0, 0]);
        let storage = encode_pidl(&[2, 0, 1, 0]);
        let folder = encode_pidl(&[2, 0, 2, 0]);
        let chain = vec![
            ("device".into(), root.clone()),
            ("storage".into(), storage.clone()),
            ("folder".into(), folder.clone()),
        ];
        assert_eq!(parent_from_chain(&folder, &chain), Some(storage.clone()));
        assert_eq!(
            parent_from_chain(&storage, &chain),
            Some(encode_device_pidl(&[2, 0, 0, 0]))
        );
        assert_eq!(
            parent_from_chain(&encode_device_pidl(&[2, 0, 0, 0]), &chain),
            Some(crate::volumes::computer())
        );
        assert_eq!(parent_from_chain(&folder, &[]), None);
    }
    #[test]
    fn recycle_namespace_stays_in_app_and_keeps_its_parent_alias() {
        assert!(is_recycle_bin_path(Path::new("shell:RecycleBinFolder")));
        assert!(is_recycle_bin_path(Path::new(
            "::{645FF040-5081-101B-9F08-00AA002F954E}"
        )));
        let root = encode_pidl_with_prefix(&[2, 0, 1, 0], "recycle:");
        let child = encode_pidl_with_prefix(&[2, 0, 2, 0], "recycle:");
        assert!(is_shell_path(&child));
        let chain = vec![
            ("bin".into(), root.clone()),
            ("child".into(), child.clone()),
        ];
        assert_eq!(parent_from_chain(&child, &chain), Some(root));
        assert_eq!(decode_pidl(&child), Some(vec![2, 0, 2, 0]));
    }

    #[test]
    fn pidl_paths_round_trip_and_are_recognized() {
        let bytes = vec![0x14, 0x00, 0x1f, 0x50, 0xe0, 0x4f, 0xd0, 0x20, 0x00, 0x00];
        let path = encode_pidl(&bytes);
        assert!(is_shell_path(&path));
        assert_eq!(decode_pidl(&path), Some(bytes));
        assert!(!is_shell_path(Path::new("C:\\Windows")));
        assert_eq!(decode_pidl(Path::new("mtp:xyz")), None);
    }
}
