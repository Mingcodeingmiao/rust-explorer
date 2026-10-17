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
pub fn read(path: &Path) -> Result<VolumeLabel> {
    ensure!(is_drive_root(path), "请选择驱动器根目录");
    let path = wide(path);
    let mut label = [0u16; 261];
    let mut filesystem = [0u16; 64];
    let mut flags = 0;
    unsafe {
        let _ = windows::Win32::System::Diagnostics::Debug::SetThreadErrorMode(
            windows::Win32::System::Diagnostics::Debug::SEM_FAILCRITICALERRORS,
            None,
        );
        ensure!(
            GetDriveTypeW(PCWSTR(path.as_ptr())) != 4,
            "网络驱动器请使用系统的重命名功能"
        );
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
    let fat = matches!(filesystem.to_ascii_uppercase().as_str(), "FAT" | "FAT32");
    ensure!(
        label.encode_utf16().count() <= if fat { 11 } else { 32 },
        "卷标过长（FAT/FAT32 最多 11 个字符，其他卷最多 32 个 UTF-16 单元）"
    );
    ensure!(
        !label
            .chars()
            .any(|c| c.is_control() || "\\/:*?\"<>|".contains(c)),
        "卷标包含无效字符"
    );
    if fat {
        ensure!(
            !label.chars().any(|c| "+=;,[]".contains(c)),
            "FAT 卷标包含无效字符"
        );
    }
    Ok(())
}
pub fn rename(path: &Path, label: &str) -> Result<()> {
    let current = read(path)?;
    ensure!(!current.read_only, "此驱动器是只读的");
    validate(label, &current.filesystem)?;
    if current.label == label {
        return Ok(());
    }
    unsafe {
        SetVolumeLabelW(PCWSTR(wide(path).as_ptr()), PCWSTR(wide(label).as_ptr()))?;
    }
    Ok(())
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
        assert!(validate("data/backup", "NTFS").is_err());
        assert!(validate("bad\0label", "NTFS").is_err());
    }
}
