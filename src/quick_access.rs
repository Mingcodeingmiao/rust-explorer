//! Read Explorer's current Quick Access namespace without changing its pin records.
//! Run this in a disposable helper: third-party namespace extensions can block COM.
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use windows::{
    Win32::{
        System::{
            Com::*,
            SystemServices::{SFGAO_FILESYSTEM, SFGAO_FOLDER},
        },
        UI::Shell::*,
    },
    core::{PCSTR, PCWSTR, PSTR, w},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub name: String,
    pub path: PathBuf,
    /// None means the namespace did not expose reliable canonical pin verbs.
    #[serde(default)]
    pub pinned: Option<bool>,
}

/// Quick Access list read earlier in this process. A window that opens later reuses
/// it instead of spawning (and waiting for) another helper.
static CACHE: std::sync::Mutex<Option<(std::time::Instant, Vec<Item>)>> =
    std::sync::Mutex::new(None);
const CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(20);

/// Cached list from this process, if it was read recently enough to be reused.
pub fn cached() -> Option<Vec<Item>> {
    let guard = CACHE.lock().ok()?;
    let (taken, items) = guard.as_ref()?;
    (taken.elapsed() < CACHE_TTL).then(|| items.clone())
}

pub fn remember(items: &[Item]) {
    if let Ok(mut guard) = CACHE.lock() {
        *guard = Some((std::time::Instant::now(), items.to_vec()));
    }
}

struct MenuInventory {
    menu: IContextMenu,
    commands: Vec<(u32, String)>,
}
fn inventory(item: &IShellItem) -> Result<MenuInventory> {
    use windows::Win32::UI::WindowsAndMessaging::*;
    unsafe {
        let menu: IContextMenu = item.BindToHandler(None, &BHID_SFUIObject)?;
        let popup = CreatePopupMenu()?;
        struct Popup(HMENU);
        impl Drop for Popup {
            fn drop(&mut self) {
                unsafe {
                    let _ = DestroyMenu(self.0);
                }
            }
        }
        let _popup = Popup(popup);
        menu.QueryContextMenu(popup, 0, 1, 0x6fff, CMF_NORMAL)
            .ok()?;
        let mut commands = Vec::new();
        for index in 0..GetMenuItemCount(Some(popup)).max(0) {
            let id = GetMenuItemID(popup, index);
            if !(1..=0x6fff).contains(&id) {
                continue;
            }
            let mut verb = [0u8; 256];
            if menu
                .GetCommandString(
                    (id - 1) as usize,
                    GCS_VERBA,
                    None,
                    PSTR(verb.as_mut_ptr()),
                    verb.len() as u32,
                )
                .is_ok()
            {
                let n = verb.iter().position(|c| *c == 0).unwrap_or(verb.len());
                commands.push((
                    id - 1,
                    String::from_utf8_lossy(&verb[..n]).to_ascii_lowercase(),
                ));
            }
        }
        Ok(MenuInventory { menu, commands })
    }
}
fn pin_state(commands: &[(u32, String)]) -> Option<bool> {
    if commands.iter().any(|(_, verb)| verb == "unpinfromhome") {
        Some(true)
    } else if commands.iter().any(|(_, verb)| verb == "pintohome") {
        Some(false)
    } else {
        None
    }
}

unsafe fn namespace_item(path: &std::path::Path) -> Result<Option<IShellItem>> {
    unsafe {
        let root: IShellItem = SHCreateItemFromParsingName(
            w!("shell:::{679f85cb-0220-4080-b29b-5540cc05aab6}"),
            None,
        )?;
        let enumeration: IEnumShellItems = root.BindToHandler(None, &BHID_EnumItems)?;
        for _ in 0..256 {
            let mut fetched = 0;
            let mut batch = [None];
            enumeration.Next(&mut batch, Some(&mut fetched))?;
            if fetched == 0 {
                break;
            }
            if let Some(item) = batch[0].take() {
                if display_name(&item, SIGDN_FILESYSPATH)
                    .is_ok_and(|value| value.eq_ignore_ascii_case(&path.to_string_lossy()))
                {
                    return Ok(Some(item));
                }
            }
        }
        Ok(None)
    }
}

/// Invoke only the requested built-in Shell verb; never choose a localized label.
/// Called in the disposable --shell worker, never on the UI thread.
pub fn set_pinned(path: &std::path::Path, pinned: bool) -> Result<()> {
    use windows::{Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL, core::HSTRING};
    ensure!(path.is_dir(), "系统快速访问仅支持文件夹");
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let _apartment = Apartment;
        // The filesystem folder menu can expose "pintohome" even when the
        // folder is pinned. Unpin must use its Quick Access namespace identity.
        let item: IShellItem = match namespace_item(path)? {
            Some(item) => item,
            None if !pinned => return Ok(()),
            None => SHCreateItemFromParsingName(&HSTRING::from(path.as_os_str()), None)?,
        };
        let inventory = inventory(&item)?;
        if pinned && pin_state(&inventory.commands) == Some(true) {
            return Ok(());
        }
        let expected = if pinned {
            &["pintohome"][..]
        } else {
            &["unpinfromhome", "removefromhome"][..]
        };
        let Some((id, _)) = expected
            .iter()
            .find_map(|expected| inventory.commands.iter().find(|(_, verb)| verb == expected))
        else {
            bail!(
                "Windows 未提供此文件夹的快速访问{}命令",
                if pinned { "固定" } else { "取消固定" }
            );
        };
        let info = CMINVOKECOMMANDINFOEX {
            cbSize: std::mem::size_of::<CMINVOKECOMMANDINFOEX>() as u32,
            fMask: SEE_MASK_UNICODE,
            lpVerb: PCSTR(*id as usize as *const u8),
            lpVerbW: PCWSTR(*id as usize as *const u16),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        inventory
            .menu
            .InvokeCommand(&info as *const _ as *const CMINVOKECOMMANDINFO)?;
    }
    Ok(())
}

struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() }
    }
}

unsafe fn display_name(item: &IShellItem, kind: SIGDN) -> Result<String> {
    let value = unsafe { item.GetDisplayName(kind)? };
    let result = unsafe { value.to_string() };
    unsafe { CoTaskMemFree(Some(value.0.cast())) };
    Ok(result?)
}

pub fn snapshot() -> Result<Vec<Item>> {
    let trace = std::env::var_os("RUST_EXPLORER_QUICK_ACCESS_TRACE").is_some();
    unsafe {
        if trace {
            eprintln!("quick-access: initialize");
        }
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let _apartment = Apartment;
        // The same shell namespace used by Explorer's Quick Access location.
        let root: IShellItem = SHCreateItemFromParsingName(
            w!("shell:::{679f85cb-0220-4080-b29b-5540cc05aab6}"),
            None,
        )?;
        if trace {
            eprintln!("quick-access: bind enumeration");
        }
        let enumeration: IEnumShellItems = root.BindToHandler(None, &BHID_EnumItems)?;
        let mut items = Vec::new();
        // Never instantiate context menus while loading the list. A single Shell
        // extension can block QueryContextMenu beyond the entire helper deadline;
        // checking elapsed time before that call cannot bound its duration. Pin
        // commands resolve their canonical verbs separately in set_pinned().
        loop {
            if trace {
                eprintln!("quick-access: next {}", items.len());
            }
            let mut fetched = 0;
            let mut batch = [None];
            enumeration.Next(&mut batch, Some(&mut fetched))?;
            if fetched == 0 {
                break;
            }
            let Some(item) = batch[0].take() else { break };
            // Do not stat the target: offline/cloud/remote folders stay cheap to enumerate.
            let Ok(attrs) = item.GetAttributes(SFGAO_FOLDER | SFGAO_FILESYSTEM) else {
                continue;
            };
            if !attrs.contains(SFGAO_FOLDER | SFGAO_FILESYSTEM) {
                continue;
            }
            let Ok(path) = display_name(&item, SIGDN_FILESYSPATH) else {
                continue;
            };
            if path.is_empty() || items.iter().any(|i: &Item| i.path == PathBuf::from(&path)) {
                continue;
            }
            let name = display_name(&item, SIGDN_NORMALDISPLAY).unwrap_or_else(|_| path.clone());
            items.push(Item {
                name,
                path: path.into(),
                pinned: None,
            });
            if items.len() >= 256 {
                break;
            }
        }
        Ok(items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_pin_verbs_distinguish_pinned_from_frequent_items() {
        assert_eq!(pin_state(&[(8, "unpinfromhome".into())]), Some(true));
        assert_eq!(pin_state(&[(9, "pintohome".into())]), Some(false));
        assert_eq!(pin_state(&[(1, "removefromhome".into())]), None);
        assert_eq!(pin_state(&[(2, "open".into())]), None);
    }
    #[test]
    fn older_snapshots_do_not_invent_pin_state() {
        let item: Item = serde_json::from_str(r#"{"name":"folder","path":"C:\\folder"}"#).unwrap();
        assert_eq!(item.pinned, None);
    }
}
