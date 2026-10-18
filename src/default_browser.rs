//! Opt-in, per-user folder association. Windows has no supported API for a
//! third-party file manager to become the shell's default browser, so this is
//! deliberately reversible and guarded by an explicit risk prompt in the UI.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const DIRECTORY: &str = "Software\\Classes\\Directory\\shell\\open\\command";
const FOLDER: &str = "Software\\Classes\\Folder\\shell\\open\\command";
const DRIVE: &str = "Software\\Classes\\Drive\\shell\\open\\command";
const MARKER_KEY: &str = "Software\\RustExplorer";
const MARKER_VALUE: &str = "DefaultBrowser";

#[derive(Default, Serialize, Deserialize)]
struct Backup {
    directory: Option<String>,
    folder: Option<String>,
    drive: Option<String>,
    #[serde(default)]
    directory_existed: bool,
    #[serde(default)]
    folder_existed: bool,
    #[serde(default)]
    drive_existed: bool,
}

fn backup_path() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("RustExplorer/default-browser-backup.json")
}

fn current_command() -> Result<String> {
    let exe = std::env::current_exe().context("无法确定 Rust Explorer 的程序路径")?;
    ensure!(
        exe.file_name().is_some_and(|name| name
            .to_string_lossy()
            .eq_ignore_ascii_case("rust-explorer.exe")),
        "只有已安装的 rust-explorer.exe 才能设为默认文件浏览器"
    );
    Ok(format!("\"{}\" \"%1\"", exe.display()))
}

fn read(key: &str) -> Option<String> {
    windows_registry::CURRENT_USER
        .open(key)
        .ok()
        .and_then(|key| key.get_string("").ok())
}

fn key_exists(key: &str) -> bool {
    windows_registry::CURRENT_USER.open(key).is_ok()
}

fn write(key: &str, value: &str) -> Result<()> {
    windows_registry::CURRENT_USER
        .create(key)
        .with_context(|| format!("无法写入注册表 {key}"))?
        .set_string("", value)
        .with_context(|| format!("无法写入注册表 {key}"))?;
    Ok(())
}

fn set_marker() -> Result<()> {
    windows_registry::CURRENT_USER
        .create(MARKER_KEY)
        .context("无法写入 Rust Explorer 设置项")?
        .set_string(MARKER_VALUE, "1")
        .context("无法写入 Rust Explorer 设置项")?;
    Ok(())
}

pub fn is_enabled() -> bool {
    if windows_registry::CURRENT_USER
        .open(MARKER_KEY)
        .ok()
        .and_then(|key| key.get_string(MARKER_VALUE).ok())
        .as_deref()
        != Some("1")
    {
        return false;
    }
    let marker = read(FOLDER).unwrap_or_default();
    marker.to_ascii_lowercase().contains("rust-explorer")
}

pub fn enable() -> Result<()> {
    let command = current_command()?;
    let backup = Backup {
        directory: read(DIRECTORY),
        folder: read(FOLDER),
        drive: read(DRIVE),
        directory_existed: key_exists(DIRECTORY),
        folder_existed: key_exists(FOLDER),
        drive_existed: key_exists(DRIVE),
    };
    let path = backup_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_vec_pretty(&backup)?)
        .with_context(|| format!("无法保存原文件夹关联到 {}", path.display()))?;
    let result = (|| -> Result<()> {
        for key in [DIRECTORY, FOLDER, DRIVE] {
            write(key, &command)?;
        }
        set_marker()?;
        Ok(())
    })();
    if let Err(error) = result {
        // Restore any keys written before the failure.
        let _ = disable();
        return Err(error);
    }
    Ok(())
}

pub fn disable() -> Result<()> {
    let path = backup_path();
    let backup: Backup = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    for (key, previous, existed) in [
        (DIRECTORY, backup.directory, backup.directory_existed),
        (FOLDER, backup.folder, backup.folder_existed),
        (DRIVE, backup.drive, backup.drive_existed),
    ] {
        match previous {
            Some(value) => write(key, &value)?,
            None if existed => {
                // The key predated us but had no default command. Remove only
                // the value this application wrote.
                if read(key)
                    .is_some_and(|current| current.to_ascii_lowercase().contains("rust-explorer"))
                {
                    if let Ok(key) = windows_registry::CURRENT_USER.open(key) {
                        let _ = key.remove_value("");
                    }
                }
            }
            None => {
                // We created this command key; remove it entirely.
                let _ = windows_registry::CURRENT_USER.remove_tree(key);
            }
        }
    }
    if let Ok(key) = windows_registry::CURRENT_USER.open(MARKER_KEY) {
        let _ = key.remove_value(MARKER_VALUE);
    }
    let _ = std::fs::remove_file(path);
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn backup_shape_is_stable() {
        let backup = super::Backup {
            directory: Some("a".into()),
            folder: None,
            drive: Some("c".into()),
            directory_existed: true,
            folder_existed: false,
            drive_existed: true,
        };
        let json = serde_json::to_string(&backup).unwrap();
        assert!(json.contains("\"directory\":\"a\""));
        let decoded: super::Backup = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.folder, None);
        assert_eq!(decoded.drive.as_deref(), Some("c"));
    }
}
