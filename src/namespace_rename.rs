//! Explicit native prompt for a namespace display name, not the machine name.
use anyhow::{Result, ensure};
use std::mem::size_of;
use windows::{
    Win32::{
        Foundation::*,
        System::Com::CoTaskMemFree,
        UI::{Shell::*, WindowsAndMessaging::*},
    },
    core::{PCWSTR, w},
};

struct Prompt {
    initial: String,
    name: Option<String>,
    dark: bool,
}
unsafe extern "system" fn dialog_proc(hwnd: HWND, message: u32, wp: WPARAM, lp: LPARAM) -> isize {
    unsafe {
        if message == WM_INITDIALOG {
            let state = &mut *(lp.0 as *mut Prompt);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, lp.0);
            super::native_menu::configure_theme(hwnd, state.dark);
            let _ = SetDlgItemTextW(hwnd, 1001, PCWSTR(super::wide(&state.initial).as_ptr()));
            if let Ok(edit) = GetDlgItem(Some(hwnd), 1001) {
                SendMessageW(
                    edit,
                    windows::Win32::UI::Controls::EM_SETSEL,
                    Some(WPARAM(0)),
                    Some(LPARAM(-1)),
                );
                let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(edit));
            }
            return 0;
        }
        if message == WM_CLOSE || (message == WM_COMMAND && wp.0 & 0xffff == IDCANCEL.0 as usize) {
            let _ = EndDialog(hwnd, 0);
            return 1;
        }
        if message == WM_COMMAND && wp.0 & 0xffff == IDOK.0 as usize {
            let mut text = [0u16; 256];
            let count = GetDlgItemTextW(hwnd, 1001, &mut text) as usize;
            let name = String::from_utf16_lossy(&text[..count]);
            if let Err(error) = crate::fs::valid_name(&name) {
                MessageBoxW(
                    Some(hwnd),
                    PCWSTR(super::wide(error.to_string()).as_ptr()),
                    w!("名称无效"),
                    MB_OK | MB_ICONWARNING,
                );
                return 1;
            }
            let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Prompt;
            if !pointer.is_null() {
                (*pointer).name = Some(name);
            }
            let _ = EndDialog(hwnd, 1);
            return 1;
        }
        0
    }
}
fn word(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}
fn dword(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}
fn text(bytes: &mut Vec<u8>, value: &str) {
    for value in value.encode_utf16().chain(Some(0)) {
        word(bytes, value);
    }
}
fn control(
    bytes: &mut Vec<u8>,
    style: u32,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    id: u16,
    class: u16,
    title: &str,
) {
    while bytes.len() % 4 != 0 {
        bytes.push(0);
    }
    dword(bytes, style);
    dword(bytes, 0);
    for value in [x, y, width, height, id, 0xffff, class] {
        word(bytes, value);
    }
    text(bytes, title);
    word(bytes, 0);
}
fn template() -> Vec<u32> {
    let mut bytes = Vec::new();
    dword(
        &mut bytes,
        WS_POPUP.0
            | WS_CAPTION.0
            | WS_SYSMENU.0
            | DS_MODALFRAME as u32
            | DS_SETFONT as u32
            | DS_CENTER as u32,
    );
    dword(&mut bytes, 0);
    for value in [4, 0, 0, 250, 98, 0, 0] {
        word(&mut bytes, value);
    }
    text(&mut bytes, "重命名 此电脑");
    word(&mut bytes, 9);
    text(&mut bytes, "Segoe UI");
    let child = WS_CHILD.0 | WS_VISIBLE.0;
    control(
        &mut bytes,
        child,
        12,
        10,
        226,
        17,
        1000,
        0x0082,
        "仅修改资源管理器中的显示名称。",
    );
    control(
        &mut bytes,
        child | WS_BORDER.0 | WS_TABSTOP.0 | ES_AUTOHSCROLL as u32,
        12,
        30,
        226,
        16,
        1001,
        0x0081,
        "",
    );
    control(
        &mut bytes,
        child | WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32,
        126,
        65,
        52,
        16,
        IDOK.0 as u16,
        0x0080,
        "确定",
    );
    control(
        &mut bytes,
        child | WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
        186,
        65,
        52,
        16,
        IDCANCEL.0 as u16,
        0x0080,
        "取消",
    );
    while bytes.len() % 4 != 0 {
        bytes.push(0);
    }
    bytes
        .chunks_exact(4)
        .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
        .collect()
}
/// A namespace display name is not a file name on disk, but the Shell applies the
/// same basic restrictions; validate here so the inline editor can report it.
pub fn valid_computer_name(name: &str) -> Result<()> {
    ensure!(!name.trim().is_empty(), "名称不能为空");
    ensure!(
        !name
            .chars()
            .any(|c| c.is_control() || "\\/:*?\"<>|".contains(c)),
        "名称包含无效字符"
    );
    Ok(())
}

/// Rename "This PC". With `name` the inline editor already collected the value;
/// without it the explicit native prompt is used (the Shell's own verb path).
pub(super) fn computer(dark: bool, name: Option<String>) -> Result<()> {
    unsafe {
        let mut raw = std::ptr::null_mut();
        SHParseDisplayName(
            w!("::{20D04FE0-3AEA-1069-A2D8-08002B30309D}"),
            None,
            &mut raw,
            0,
            None,
        )?;
        let pidl = super::Pidl(raw);
        let current = SHGetNameFromIDList(pidl.0, SIGDN_NORMALDISPLAY)?;
        let initial = current.to_string().unwrap_or_else(|_| "此电脑".into());
        CoTaskMemFree(Some(current.0.cast()));
        let chosen = if let Some(name) = name {
            valid_computer_name(&name)?;
            name.trim().to_string()
        } else {
            let mut state = Prompt {
                initial: initial.clone(),
                name: None,
                dark,
            };
            let bytes = template();
            ensure!(
                bytes.len() * 4 >= size_of::<DLGTEMPLATE>(),
                "Invalid rename dialog template"
            );
            let result = DialogBoxIndirectParamW(
                None,
                bytes.as_ptr().cast(),
                None,
                Some(dialog_proc),
                LPARAM((&mut state as *mut Prompt) as isize),
            );
            ensure!(result != -1, "Windows could not open the rename dialog");
            match state.name {
                Some(name) => name,
                None => return Ok(()),
            }
        };
        if chosen == initial {
            return Ok(());
        }
        let parent: IShellFolder = SHBindToParent(pidl.0, None)?;
        parent.SetNameOf(
            None,
            ILFindLastID(pidl.0),
            PCWSTR(super::wide(&chosen).as_ptr()),
            SHGDN_NORMAL,
            None,
        )?;
        SHChangeNotify(SHCNE_UPDATEITEM, SHCNF_IDLIST, Some(pidl.0.cast()), None);
        println!("computer-name:{chosen}");
        Ok(())
    }
}
pub(super) fn computer_name() -> Result<String> {
    unsafe {
        let mut raw = std::ptr::null_mut();
        SHParseDisplayName(
            w!("::{20D04FE0-3AEA-1069-A2D8-08002B30309D}"),
            None,
            &mut raw,
            0,
            None,
        )?;
        let pidl = super::Pidl(raw);
        let name = SHGetNameFromIDList(pidl.0, SIGDN_NORMALDISPLAY)?;
        let result = name.to_string();
        CoTaskMemFree(Some(name.0.cast()));
        Ok(result?)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn computer_names_reject_empty_and_shell_invalid_values() {
        assert!(super::valid_computer_name("我的电脑 2026").is_ok());
        assert!(super::valid_computer_name("   ").is_err());
        assert!(super::valid_computer_name("a/b").is_err());
        assert!(super::valid_computer_name("a\"b").is_err());
        assert!(super::valid_computer_name("a\nb").is_err());
        // Leading/trailing spaces are trimmed by the caller, not rejected.
        assert!(super::valid_computer_name("  工作机  ").is_ok());
    }
}
