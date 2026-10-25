//! Volume labels are not file names. Read them in a bounded helper and change
//! them only after the dedicated rename dialog has been confirmed.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::Path;
use windows::{Win32::Storage::FileSystem::*, core::PCWSTR};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VolumeLabel {
    pub label: String,
    pub filesystem: String,
    pub read_only: bool,
}

pub fn is_drive_root(path: &Path) -> bool {
    let value = path.as_os_str().to_string_lossy();
    let bytes = value.as_bytes();
    bytes.len() == 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}
fn wide(value: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    value.as_ref().encode_wide().chain(Some(0)).collect()
}
fn string(value: &[u16]) -> String {
    String::from_utf16_lossy(&value[..value.iter().position(|c| *c == 0).unwrap_or(value.len())])
}
pub fn is_network_drive(path: &Path) -> bool {
    is_drive_root(path) && unsafe { GetDriveTypeW(PCWSTR(wide(path).as_ptr())) == 4 }
}
pub fn read(path: &Path) -> Result<VolumeLabel> {
    ensure!(is_drive_root(path), "请选择驱动器根目录");
    // Install the guard before the first query (network drives can block on a
    // reconnect dialog) and restore the previous mode on every exit path.
    let _quiet = crate::volumes::QuietVolumeErrors::suppress();
    let path = wide(path);
    let mut label = [0u16; 261];
    let mut filesystem = [0u16; 64];
    let mut flags = 0;
    unsafe {
        if GetDriveTypeW(PCWSTR(path.as_ptr())) == 4 {
            use windows::Win32::{System::Com::CoTaskMemFree, UI::Shell::*};
            let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(path.as_ptr()), None)?;
            let raw = item.GetDisplayName(SIGDN_PARENTRELATIVEEDITING)?;
            let label = raw.to_string();
            CoTaskMemFree(Some(raw.0.cast()));
            return Ok(VolumeLabel {
                label: label?,
                filesystem: "NETWORK".into(),
                read_only: false,
            });
        }
        GetVolumeInformationW(
            PCWSTR(path.as_ptr()),
            Some(&mut label),
            None,
            None,
            Some(&mut flags),
            Some(&mut filesystem),
        )?;
    }
    Ok(VolumeLabel {
        label: string(&label),
        filesystem: string(&filesystem),
        read_only: flags & 0x00080000 != 0,
    })
}

pub fn validate(label: &str, filesystem: &str) -> Result<()> {
    let filesystem = filesystem.to_ascii_uppercase();
    if filesystem == "NETWORK" {
        // A mapped drive's name is a Shell display name, not a kernel volume
        // label or a file name. Shell permits \ : ( ) and other path-like
        // characters that fs::valid_name rejects.
        ensure!(!label.is_empty(), "网络驱动器名称不能为空");
        ensure!(
            label.encode_utf16().count() <= 255,
            "网络驱动器名称过长（最多 255 个 UTF-16 单元）"
        );
        ensure!(
            !label.chars().any(|c| c.is_control()),
            "网络驱动器名称包含控制字符"
        );
        return Ok(());
    }
    let fat = matches!(filesystem.as_str(), "FAT" | "FAT32");
    ensure!(!label.chars().any(|c| c.is_control()), "卷标包含控制字符");
    if fat {
        // FAT/FAT32 inherit the old, strict 11-character rule and reject the
        // Windows path characters plus the FAT-specific punctuation set.
        ensure!(
            label.encode_utf16().count() <= 11,
            "FAT/FAT32 卷标最多 11 个 UTF-16 单元"
        );
        ensure!(
            !label.chars().any(|c| "\\/:*?\"<>|+=;,[]".contains(c)),
            "FAT/FAT32 卷标包含无效字符"
        );
        return Ok(());
    }
    // NTFS, exFAT, ReFS and other modern/unknown filesystems accept path-like
    // characters; the kernel still limits a local volume label to 32 units.
    ensure!(
        label.encode_utf16().count() <= 32,
        "卷标过长（其他卷最多 32 个 UTF-16 单元）"
    );
    Ok(())
}
pub fn rename(path: &Path, label: &str) -> Result<()> {
    let current = read(path)?;
    ensure!(!current.read_only, "此驱动器是只读的");
    validate(label, &current.filesystem)?;
    if current.label == label {
        return Ok(());
    }
    if current.filesystem == "NETWORK" {
        use windows::Win32::{System::Com::CoTaskMemFree, UI::Shell::*};
        unsafe {
            let mut pidl = std::ptr::null_mut();
            SHParseDisplayName(PCWSTR(wide(path).as_ptr()), None, &mut pidl, 0, None)?;
            let result = (|| -> Result<()> {
                let parent: IShellFolder = SHBindToParent(pidl, None)?;
                parent.SetNameOf(
                    None,
                    ILFindLastID(pidl),
                    PCWSTR(wide(label).as_ptr()),
                    SHGDN_NORMAL,
                    None,
                )?;
                SHChangeNotify(SHCNE_UPDATEITEM, SHCNF_IDLIST, Some(pidl.cast()), None);
                Ok(())
            })();
            CoTaskMemFree(Some(pidl.cast()));
            return result;
        }
    }
    unsafe {
        SetVolumeLabelW(PCWSTR(wide(path).as_ptr()), PCWSTR(wide(label).as_ptr()))?;
    }
    Ok(())
}

/// Retry only a volume-label access failure with a narrowly scoped UAC helper.
/// The normal UI and mapped-drive rename remain in the user's session.
pub fn rename_with_elevation(path: &Path, label: &str) -> Result<()> {
    match rename(path, label) {
        Ok(()) => Ok(()),
        Err(error)
            if error
                .downcast_ref::<windows::core::Error>()
                .is_some_and(|e| e.code() == windows::core::HRESULT(0x80070005u32 as i32))
                && read(path)?.filesystem != "NETWORK" =>
        {
            use windows::Win32::{
                Foundation::*,
                System::Threading::*,
                UI::{Shell::*, WindowsAndMessaging::SW_SHOWNORMAL},
            };
            let payload = serde_json::to_string(&(path, label))?;
            // Encode the JSON as hex so no path/name can inject a command-line option.
            let encoded: String = payload
                .as_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            let executable = wide(std::env::current_exe()?);
            let arguments = wide(format!("--rename-volume-admin {encoded}"));
            let mut info = SHELLEXECUTEINFOW {
                cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
                fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
                lpVerb: windows::core::w!("runas"),
                lpFile: PCWSTR(executable.as_ptr()),
                lpParameters: PCWSTR(arguments.as_ptr()),
                nShow: SW_SHOWNORMAL.0,
                ..Default::default()
            };
            unsafe {
                ShellExecuteExW(&mut info)?;
                let wait = WaitForSingleObject(info.hProcess, INFINITE);
                let mut code = 1;
                let result = GetExitCodeProcess(info.hProcess, &mut code);
                let _ = CloseHandle(info.hProcess);
                result?;
                ensure!(
                    wait == WAIT_OBJECT_0 && code == 0,
                    "管理员未完成驱动器重命名，请检查权限或磁盘状态"
                );
            }
            Ok(())
        }
        Err(error) => Err(error),
    }
}

pub fn rename_admin(encoded: &str) -> Result<()> {
    // This elevated helper starts without an apartment; reading a mapped network
    // drive goes through the Shell, which needs one.
    let _ = unsafe {
        windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
        )
    };
    ensure!(
        encoded.len() <= 32768 && encoded.len() % 2 == 0 && encoded.is_ascii(),
        "Invalid volume rename request"
    );
    let bytes = (0..encoded.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&encoded[i..i + 2], 16))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let (path, label): (std::path::PathBuf, String) = serde_json::from_slice(&bytes)?;
    rename(&path, &label)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_absolute_drive_roots() {
        for valid in ["C:\\", "d:/"] {
            assert!(is_drive_root(Path::new(valid)));
        }
        for invalid in [
            "C:",
            "C:\\folder",
            "\\\\server\\share\\",
            "::ThisPC",
            "",
            "1:\\",
        ] {
            assert!(!is_drive_root(Path::new(invalid)));
        }
    }
    #[test]
    fn label_rules_allow_clear_and_reject_invalid_names() {
        assert!(validate("", "NTFS").is_ok());
        assert!(validate("Data 资料", "NTFS").is_ok());
        assert!(validate(&"a".repeat(32), "NTFS").is_ok());
        assert!(validate(&"a".repeat(33), "NTFS").is_err());
        assert!(validate(&"a".repeat(12), "FAT32").is_err());
        assert!(validate("data/backup", "NTFS").is_ok());
        assert!(validate("a\\b:c*d?e\"f<g>h|i", "NTFS").is_ok());
        assert!(validate("same:name", "exFAT").is_ok());
        assert!(validate("same:name", "ReFS").is_ok());
        assert!(validate("data/backup", "FAT32").is_err());
        assert!(validate("a+b=c;d,e[f]", "FAT").is_err());
        assert!(validate("bad\0label", "NTFS").is_err());
        assert!(validate(&"n".repeat(64), "NETWORK").is_ok());
        assert!(validate("pri/v\\1.:2 (Z:)", "NETWORK").is_ok());
        assert!(validate("invalid/name", "NETWORK").is_ok());
        assert!(validate(&"n".repeat(256), "NETWORK").is_err());
        assert!(validate("bad\0label", "NETWORK").is_err());
    }
}
