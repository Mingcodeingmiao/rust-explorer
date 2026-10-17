//! Application view commands added to the real Shell background menu.
//! The private Windows theme entry points are optional, version guarded and
//! never loaded from a user-supplied resource module.
use anyhow::Result;
use std::mem::size_of;
use windows::{
    Win32::{
        Foundation::*,
        System::{LibraryLoader::*, SystemInformation::*},
        UI::{Controls::SetWindowTheme, WindowsAndMessaging::*},
    },
    core::{PCSTR, PCWSTR, w},
};

pub(super) fn configure_theme(hwnd: HWND, dark: bool) {
    unsafe {
        let mut version = OSVERSIONINFOW {
            dwOSVersionInfoSize: size_of::<OSVERSIONINFOW>() as u32,
            ..Default::default()
        };
        if GetVersionExW(&mut version).is_err()
            || version.dwMajorVersion < 10
            || version.dwBuildNumber < 18362
        {
            return;
        }
        let Ok(module) = LoadLibraryExW(w!("uxtheme.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32)
        else {
            return;
        };
        if let Some(entry) = GetProcAddress(module, PCSTR(135usize as *const u8)) {
            let set_mode: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(entry);
            set_mode(if dark { 2 } else { 3 });
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

pub(super) struct Options {
    pub view: u8,
    pub sort: String,
    pub descending: bool,
    pub group: String,
    pub undo: bool,
    pub redo: bool,
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
        if background {
            let views = Builder::submenu(menu, position, "查看(&V)")?;
            position += 1;
            for (index, label) in [
                "超大图标",
                "大图标",
                "中等图标",
                "小图标",
                "列表",
                "详细信息",
                "平铺",
                "内容",
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
            let sort = Builder::submenu(menu, position, "排序方式(&O)")?;
            position += 1;
            for (key, label) in [
                ("name", "名称"),
                ("modified", "修改日期"),
                ("kind", "类型"),
                ("size", "大小"),
                ("created", "创建日期"),
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
            builder.item(sort, "递增", "ascending", !options.descending, true)?;
            builder.item(sort, "递减", "descending", options.descending, true)?;
            let groups = Builder::submenu(menu, position, "分组依据(&P)")?;
            position += 1;
            for (key, label) in [
                ("", "（无）"),
                ("name", "名称"),
                ("modified", "修改日期"),
                ("kind", "类型"),
                ("size", "大小"),
                ("created", "创建日期"),
            ] {
                builder.item(
                    groups,
                    label,
                    format!("group:{key}"),
                    options.group == key,
                    true,
                )?;
            }
            builder.root(menu, position, "刷新(&E)", "refresh-folder", true)?;
            position += 1;
        }
        if background || options.undo || options.redo {
            builder.root(menu, position, "撤销(&U)\tCtrl+Z", "undo", options.undo)?;
            position += 1;
            builder.root(menu, position, "重做(&R)\tCtrl+Y", "redo", options.redo)?;
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
