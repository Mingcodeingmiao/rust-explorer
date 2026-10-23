//! Application view commands added to the real Shell background menu.
//! The private Windows theme entry points are optional, version guarded and
//! never loaded from a user-supplied resource module.
use anyhow::Result;
use windows::{
    Win32::{
        Foundation::*,
        System::LibraryLoader::*,
        UI::{Controls::SetWindowTheme, WindowsAndMessaging::*},
    },
    core::{PCSTR, PCWSTR, w},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ThemeApi {
    Legacy1809,
    Preferred,
}

fn theme_api(major: u32, build: u32) -> Option<ThemeApi> {
    match (major, build) {
        (10, 17763) => Some(ThemeApi::Legacy1809),
        (10, 18362..) => Some(ThemeApi::Preferred),
        _ => None,
    }
}

fn installed_theme_api() -> Option<ThemeApi> {
    use std::sync::OnceLock;
    static API: OnceLock<Option<ThemeApi>> = OnceLock::new();
    *API.get_or_init(|| unsafe {
        // GetVersionExW may return a compatibility-manifest version. Never use
        // it to select the ABI of undocumented ordinal 135 (1809 uses bool).
        let ntdll = GetModuleHandleW(w!("ntdll.dll")).ok()?;
        let entry = GetProcAddress(ntdll, windows::core::s!("RtlGetNtVersionNumbers"))?;
        let version: unsafe extern "system" fn(*mut u32, *mut u32, *mut u32) =
            std::mem::transmute(entry);
        let (mut major, mut minor, mut build) = (0, 0, 0);
        version(&mut major, &mut minor, &mut build);
        theme_api(major, build & 0x0fff_ffff)
    })
}

pub(super) fn configure_theme(hwnd: HWND, dark: bool) {
    let Some(api) = installed_theme_api() else {
        return;
    };
    unsafe {
        let Ok(module) = LoadLibraryExW(w!("uxtheme.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32)
        else {
            return;
        };
        if let Some(entry) = GetProcAddress(module, PCSTR(135usize as *const u8)) {
            match api {
                ThemeApi::Legacy1809 => {
                    let allow: unsafe extern "system" fn(bool) -> bool = std::mem::transmute(entry);
                    allow(dark);
                }
                ThemeApi::Preferred => {
                    let set_mode: unsafe extern "system" fn(i32) -> i32 =
                        std::mem::transmute(entry);
                    set_mode(if dark { 2 } else { 3 });
                }
            }
        }
        if let Some(entry) = GetProcAddress(module, PCSTR(133usize as *const u8)) {
            let allow: unsafe extern "system" fn(HWND, bool) -> bool = std::mem::transmute(entry);
            allow(hwnd, dark);
        }
        let _ = SetWindowTheme(
            hwnd,
            if dark {
                w!("DarkMode_Explorer")
            } else {
                w!("Explorer")
            },
            PCWSTR::null(),
        );
        if let Some(entry) = GetProcAddress(module, PCSTR(136usize as *const u8)) {
            let flush: unsafe extern "system" fn() = std::mem::transmute(entry);
            flush();
        }
        let _ = FreeLibrary(module);
    }
}

#[cfg(test)]
mod theme_tests {
    use super::*;
    #[test]
    fn theme_ordinal_abi_is_selected_by_real_windows_build() {
        assert_eq!(theme_api(10, 17763), Some(ThemeApi::Legacy1809));
        for build in [18362, 19045, 22000, 26300] {
            assert_eq!(theme_api(10, build), Some(ThemeApi::Preferred));
        }
        for (major, build) in [(6, 9600), (10, 17134), (10, 18300), (11, 1)] {
            assert_eq!(theme_api(major, build), None);
        }
    }
}

pub(super) struct Options {
    pub view: u8,
    pub sort: String,
    pub descending: bool,
    pub group: String,
    pub undo: bool,
    pub redo: bool,
    pub folder: bool,
    pub folder_insert: u32,
    pub can_paste: bool,
}
struct Builder {
    commands: Vec<String>,
}
impl Builder {
    unsafe fn item(
        &mut self,
        menu: HMENU,
        title: &str,
        command: impl Into<String>,
        checked: bool,
        enabled: bool,
    ) -> Result<()> {
        let id = 0x9000 + self.commands.len();
        self.commands.push(command.into());
        unsafe {
            AppendMenuW(
                menu,
                MF_STRING
                    | if checked { MF_CHECKED } else { MF_UNCHECKED }
                    | if enabled { MF_ENABLED } else { MF_GRAYED },
                id,
                PCWSTR(super::wide(title).as_ptr()),
            )?;
        }
        Ok(())
    }
    unsafe fn root(
        &mut self,
        menu: HMENU,
        position: u32,
        title: &str,
        command: &str,
        enabled: bool,
    ) -> Result<()> {
        let id = 0x9000 + self.commands.len();
        self.commands.push(command.into());
        unsafe {
            InsertMenuW(
                menu,
                position,
                MF_BYPOSITION | MF_STRING | if enabled { MF_ENABLED } else { MF_GRAYED },
                id,
                PCWSTR(super::wide(title).as_ptr()),
            )?;
        }
        Ok(())
    }
    unsafe fn submenu(menu: HMENU, position: u32, title: &str) -> Result<HMENU> {
        let child = unsafe { CreatePopupMenu()? };
        if let Err(error) = unsafe {
            InsertMenuW(
                menu,
                position,
                MF_BYPOSITION | MF_POPUP | MF_STRING,
                child.0 as usize,
                PCWSTR(super::wide(title).as_ptr()),
            )
        } {
            unsafe {
                let _ = DestroyMenu(child);
            }
            return Err(error.into());
        }
        Ok(child)
    }
}
/// Preserve every native provider entry. Only host-owned view/history commands
/// are replaced, since Explorer's IShellView belongs to a different application.
pub(super) unsafe fn augment(
    menu: HMENU,
    options: &Options,
    background: bool,
) -> Result<Vec<String>> {
    unsafe {
        let mut builder = Builder {
            commands: Vec::new(),
        };
        let mut position = 0;
        if !background && options.folder {
            let start = options.folder_insert;
            builder.root(
                menu,
                start,
                crate::i18n::text("在新标签页中打开"),
                "open-new-tab",
                true,
            )?;
            builder.root(
                menu,
                start + 1,
                crate::i18n::text("在新窗口中打开"),
                "open-new-window",
                true,
            )?;
            InsertMenuW(
                menu,
                start + 2,
                MF_BYPOSITION | MF_SEPARATOR,
                0,
                PCWSTR::null(),
            )?;
            position = start + 3;
        }
        if background {
            let views = Builder::submenu(menu, position, crate::i18n::text("查看(&V)"))?;
            position += 1;
            for (index, label) in [
                crate::i18n::text("超大图标"),
                crate::i18n::text("大图标"),
                crate::i18n::text("中等图标"),
                crate::i18n::text("小图标"),
                crate::i18n::text("列表"),
                crate::i18n::text("详细信息"),
                crate::i18n::text("平铺"),
                crate::i18n::text("内容"),
            ]
            .iter()
            .enumerate()
            {
                builder.item(
                    views,
                    label,
                    format!("view:{index}"),
                    index == options.view as usize,
                    true,
                )?;
            }
            let sort = Builder::submenu(menu, position, crate::i18n::text("排序方式(&O)"))?;
            position += 1;
            for (key, label) in [
                ("name", crate::i18n::text("名称")),
                ("modified", crate::i18n::text("修改日期")),
                ("kind", crate::i18n::text("类型")),
                ("size", crate::i18n::text("大小")),
                ("created", crate::i18n::text("创建日期")),
            ] {
                builder.item(
                    sort,
                    label,
                    format!("sort:{key}"),
                    options.sort == key,
                    true,
                )?;
            }
            AppendMenuW(sort, MF_SEPARATOR, 0, PCWSTR::null())?;
            builder.item(
                sort,
                crate::i18n::text("递增"),
                "ascending",
                !options.descending,
                true,
            )?;
            builder.item(
                sort,
                crate::i18n::text("递减"),
                "descending",
                options.descending,
                true,
            )?;
            let groups = Builder::submenu(menu, position, crate::i18n::text("分组依据(&P)"))?;
            position += 1;
            for (key, label) in [
                ("", crate::i18n::text("（无）")),
                ("name", crate::i18n::text("名称")),
                ("modified", crate::i18n::text("修改日期")),
                ("kind", crate::i18n::text("类型")),
                ("size", crate::i18n::text("大小")),
                ("created", crate::i18n::text("创建日期")),
            ] {
                builder.item(
                    groups,
                    label,
                    format!("group:{key}"),
                    options.group == key,
                    true,
                )?;
            }
            builder.root(
                menu,
                position,
                crate::i18n::text("刷新(&E)"),
                "refresh-folder",
                true,
            )?;
            position += 1;
            builder.root(
                menu,
                position,
                crate::i18n::text("粘贴(&P)\tCtrl+V"),
                "paste",
                options.can_paste,
            )?;
            position += 1;
        }
        if background || options.undo || options.redo {
            builder.root(
                menu,
                position,
                crate::i18n::text("撤销(&U)\tCtrl+Z"),
                "undo",
                options.undo,
            )?;
            position += 1;
            builder.root(
                menu,
                position,
                crate::i18n::text("重做(&R)\tCtrl+Y"),
                "redo",
                options.redo,
            )?;
            position += 1;
            InsertMenuW(
                menu,
                position,
                MF_BYPOSITION | MF_SEPARATOR,
                0,
                PCWSTR::null(),
            )?;
        }
        Ok(builder.commands)
    }
}
