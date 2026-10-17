//! Invoke the installed Windows sharing commands rather than editing ACLs.
//! CLSIDs are the local CommandStore Windows.SharePrivate and
//! Windows.ShareSpecificUsers registrations, both hosted by ntshrui.dll.
use anyhow::{Result, ensure};
use serde::Serialize;
use std::path::Path;
use windows::{
    Win32::{System::Com::*, UI::Shell::*},
    core::{GUID, PCWSTR},
};
const PRIVATE: GUID = GUID::from_u128(0xc139c040_532d_451b_80ca_44d1f0839d2a);
const SPECIFIC: GUID = GUID::from_u128(0x1a0391bf_9564_4294_b0a4_06c298929ef9);

unsafe fn command(remove: bool) -> Result<IExplorerCommand> {
    Ok(unsafe {
        CoCreateInstance(
            if remove { &PRIVATE } else { &SPECIFIC },
            None,
            CLSCTX_INPROC_SERVER,
        )?
    })
}
unsafe fn selection(path: &Path) -> Result<IShellItemArray> {
    ensure!(
        path.is_absolute() && path.is_dir(),
        "请选择要设置共享的文件夹"
    );
    unsafe {
        let pidl = super::Pidl(ILCreateFromPathW(PCWSTR(super::wide(path).as_ptr())));
        ensure!(!pidl.0.is_null(), "Windows 无法解析此文件夹");
        Ok(SHCreateShellItemArrayFromIDLists(&[
            pidl.0 as *const Common::ITEMIDLIST
        ])?)
    }
}
pub(super) fn title(path: &Path, remove: bool) -> Result<String> {
    unsafe {
        let items = selection(path)?;
        let command = command(remove)?;
        let value = command.GetTitle(&items)?;
        let result = value.to_string().unwrap_or_default();
        CoTaskMemFree(Some(value.0 as _));
        return Ok(result);
    }
}

#[derive(Serialize)]
pub(super) struct CommandInfo {
    operation: &'static str,
    title: String,
    canonical: String,
}
/// Read-only regression support. Never call Invoke from this path.
pub(super) fn inspect(path: &Path) -> Result<Vec<CommandInfo>> {
    unsafe {
        let items = selection(path)?;
        let mut result = Vec::new();
        for (remove, operation) in [(true, "remove-access"), (false, "specific-users")] {
            let command = command(remove)?;
            let title = command.GetTitle(&items)?;
            let text = title.to_string().unwrap_or_default();
            CoTaskMemFree(Some(title.0 as _));
            let canonical = format!("{:?}", command.GetCanonicalName()?);
            result.push(CommandInfo {
                operation,
                title: text,
                canonical,
            });
        }
        Ok(result)
    }
}
