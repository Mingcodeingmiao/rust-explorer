use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use windows::{Win32::Storage::FileSystem::*, core::PCWSTR};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Volume {
    pub path: PathBuf,
    pub label: String,
    pub kind: u32,
    pub total: Option<u64>,
    pub free: Option<u64>,
    pub filesystem: String,
    pub remote: Option<String>,
    #[serde(default)]
    pub read_only: bool,
}
pub fn computer() -> PathBuf {
    PathBuf::from("::ThisPC")
}
pub fn is_computer(path: &Path) -> bool {
    path == computer()
}
fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}
pub fn roots() -> Vec<PathBuf> {
    let mut roots = crate::shell::drives();
    if let Ok(network) = windows_registry::CURRENT_USER.open("Network") {
        if let Ok(keys) = network.keys() {
            for key in keys {
                if key.len() == 1 && key.as_bytes()[0].is_ascii_alphabetic() {
                    let p = PathBuf::from(format!("{}:\\", key.to_ascii_uppercase()));
                    if !roots.contains(&p) {
                        roots.push(p);
                    }
                }
            }
        }
    }
    roots.sort();
    roots
}
pub fn basic(path: &Path) -> Volume {
    let root = wide(path);
    let mut kind = unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) };
    let letter = path.to_string_lossy().chars().next().unwrap_or('?');
    let remote = windows_registry::CURRENT_USER
        .open(format!("Network\\{letter}"))
        .and_then(|k| k.get_string("RemotePath"))
        .ok();
    if remote.is_some() {
        kind = 4;
    }
    let name = match kind {
        // DRIVE_UNKNOWN / DRIVE_NO_ROOT_DIR: a removed or disconnected mapping
        // still has a letter in HKCU\Network, so never call it a local disk.
        0 => "未知设备",
        1 => "未连接",
        2 => "可移动磁盘",
        4 => "网络驱动器",
        5 => "DVD 驱动器",
        6 => "RAM 磁盘",
        _ => "本地磁盘",
    };
    Volume {
        path: path.to_path_buf(),
        label: format!("{} ({letter}:)", remote.as_deref().unwrap_or(name)),
        kind,
        total: None,
        free: None,
        filesystem: String::new(),
        remote,
        read_only: kind == 5,
    }
}
/// `SEM_FAILCRITICALERRORS` must already be active for the *first* query against
/// a possibly-disconnected network drive, and the previous mode must be restored
/// afterwards. Both were wrong before: the guard was installed after the first
/// `GetDiskFreeSpaceExW` and never undone.
pub(crate) struct QuietVolumeErrors {
    previous: windows::Win32::System::Diagnostics::Debug::THREAD_ERROR_MODE,
    active: bool,
}
impl QuietVolumeErrors {
    pub(crate) fn suppress() -> Self {
        use windows::Win32::System::Diagnostics::Debug::{
            SEM_FAILCRITICALERRORS, SetThreadErrorMode, THREAD_ERROR_MODE,
        };
        let mut previous = THREAD_ERROR_MODE(0);
        let active =
            unsafe { SetThreadErrorMode(SEM_FAILCRITICALERRORS, Some(&mut previous)) }.is_ok();
        Self { previous, active }
    }
}
impl Drop for QuietVolumeErrors {
    fn drop(&mut self) {
        if self.active {
            unsafe {
                let _ = windows::Win32::System::Diagnostics::Debug::SetThreadErrorMode(
                    self.previous,
                    None,
                );
            }
        }
    }
}
pub fn info(path: &Path) -> Volume {
    let _quiet = QuietVolumeErrors::suppress();
    let mut volume = basic(path);
    if volume.kind == 4 {
        // This function runs only in a killable helper (800 ms total budget).
        // Query capacity and read-only flags without trying to reconnect or write.
        let path = wide(path);
        let (mut total, mut free) = (0, 0);
        unsafe {
            if GetDiskFreeSpaceExW(
                PCWSTR(path.as_ptr()),
                None,
                Some(&mut total),
                Some(&mut free),
            )
            .is_ok()
            {
                volume.total = Some(total);
                volume.free = Some(free);
            }
        }
        let mut flags = 0;
        unsafe {
            if GetVolumeInformationW(
                PCWSTR(path.as_ptr()),
                None,
                None,
                None,
                Some(&mut flags),
                None,
            )
            .is_ok()
            {
                volume.read_only = flags & 0x00080000 != 0;
            }
        }
        return volume;
    }
    let path = wide(path);
    let mut name = [0u16; 261];
    let mut filesystem = [0u16; 64];
    let mut total = 0;
    let mut free = 0;
    let mut flags = 0;
    unsafe {
        // Empty optical/card readers must not raise a system error dialog.
        if GetVolumeInformationW(
            PCWSTR(path.as_ptr()),
            Some(&mut name),
            None,
            None,
            Some(&mut flags),
            Some(&mut filesystem),
        )
        .is_ok()
        {
            volume.read_only = flags & 0x00080000 != 0;
            let label = String::from_utf16_lossy(
                &name[..name.iter().position(|c| *c == 0).unwrap_or(name.len())],
            );
            if !label.is_empty() {
                let letter = volume.path.to_string_lossy().chars().next().unwrap_or('?');
                volume.label = format!("{label} ({letter}:)");
            }
            volume.filesystem = String::from_utf16_lossy(
                &filesystem[..filesystem
                    .iter()
                    .position(|c| *c == 0)
                    .unwrap_or(filesystem.len())],
            );
        }
        if GetDiskFreeSpaceExW(
            PCWSTR(path.as_ptr()),
            None,
            Some(&mut total),
            Some(&mut free),
        )
        .is_ok()
        {
            volume.total = Some(total);
            volume.free = Some(free);
        }
    }
    volume
}
pub fn bounded<T: serde::de::DeserializeOwned>(
    flag: &str,
    json: &str,
    timeout: Duration,
) -> Result<T> {
    use std::os::windows::process::CommandExt;
    let mut child = Command::new(std::env::current_exe()?)
        .arg(flag)
        .arg(json)
        .creation_flags(0x08000000)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().context("helper stdout")?;
    let stderr = child.stderr.take().context("helper stderr")?;
    let read = std::thread::spawn(move || {
        use std::io::Read;
        let mut output = Vec::new();
        let _ = stdout.take(8 * 1024 * 1024).read_to_end(&mut output);
        output
    });
    let err = std::thread::spawn(move || {
        use std::io::Read;
        let mut output = Vec::new();
        let _ = stderr.take(64 * 1024).read_to_end(&mut output);
        output
    });
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let output = read.join().unwrap_or_default();
    let error = err.join().unwrap_or_default();
    anyhow::ensure!(
        status.is_some_and(|s| s.success()),
        "系统查询失败或超时：{}",
        String::from_utf8_lossy(&error)
    );
    Ok(serde_json::from_slice(&output)?)
}
/// Snapshot taken earlier in this process. All windows live in one process, so a
/// window that opens later can reuse the drive list instead of enumerating every
/// drive (and spawning a helper per drive) again.
static SNAPSHOT_CACHE: std::sync::Mutex<Option<(std::time::Instant, Vec<Volume>)>> =
    std::sync::Mutex::new(None);
const SNAPSHOT_TTL: std::time::Duration = std::time::Duration::from_secs(20);

/// Snapshot from this process, if one was taken recently enough to be reused.
pub fn cached_snapshot() -> Option<Vec<Volume>> {
    let guard = SNAPSHOT_CACHE.lock().ok()?;
    let (taken, volumes) = guard.as_ref()?;
    (taken.elapsed() < SNAPSHOT_TTL).then(|| volumes.clone())
}

pub fn remember_snapshot(volumes: &[Volume]) {
    if let Ok(mut guard) = SNAPSHOT_CACHE.lock() {
        *guard = Some((std::time::Instant::now(), volumes.to_vec()));
    }
}
pub fn snapshot() -> Vec<Volume> {
    let mut result: Vec<Volume> = roots()
        .into_iter()
        .map(|path| {
            let basic = basic(&path);
            bounded(
                "--volume-info",
                &serde_json::to_string(&path).unwrap(),
                Duration::from_millis(800),
            )
            .unwrap_or(basic)
        })
        .collect();
    // Portable/MTP devices have no filesystem path; list them through the
    // Shell namespace in an isolated helper.
    if let Ok(devices) =
        bounded::<Vec<(String, PathBuf)>>("--shell-devices", "", Duration::from_millis(2500))
    {
        for (label, path) in devices {
            if result.iter().any(|volume| volume.path == path) {
                continue;
            }
            result.push(Volume {
                path,
                label,
                kind: 7,
                total: None,
                free: None,
                filesystem: "MTP".into(),
                remote: None,
                read_only: false,
            });
        }
    }
    remember_snapshot(&result);
    result
}
#[derive(Serialize, Deserialize)]
pub struct TreeRequest {
    pub path: PathBuf,
    pub hidden: bool,
    pub protected: bool,
}
pub fn children(request: TreeRequest) -> Result<Vec<(String, PathBuf)>> {
    Ok(children_with_attributes(request)?
        .into_iter()
        .map(|e| (e.name, e.path))
        .collect())
}
#[derive(Serialize, Deserialize)]
pub struct TreeEntry {
    pub name: String,
    pub path: PathBuf,
    pub hidden: bool,
}
pub fn children_with_attributes(request: TreeRequest) -> Result<Vec<TreeEntry>> {
    use std::os::windows::fs::MetadataExt;
    let mut out = vec![];
    for entry in std::fs::read_dir(&request.path)? {
        let Ok(entry) = entry else {
            continue;
        };
        let Ok(m) = entry.metadata() else {
            continue;
        };
        if !m.is_dir()
            || (!request.hidden && m.file_attributes() & 2 != 0)
            || (!request.protected && m.file_attributes() & 4 != 0)
        {
            continue;
        }
        out.push(TreeEntry {
            name: entry.file_name().to_string_lossy().into_owned(),
            path: entry.path(),
            hidden: m.file_attributes() & 2 != 0,
        });
        if out.len() >= 2000 {
            break;
        }
    }
    out.sort_by(|a, b| crate::fs::natural_cmp(&a.name, &b.name));
    Ok(out)
}

/// Stable ordering for the independent This PC columns. Paths provide the
/// drive-letter order and deterministic tie break even with renamed volumes.
pub fn compare(a: &Volume, b: &Volume, column: &str, descending: bool) -> std::cmp::Ordering {
    let primary = match column {
        "kind" => a.kind.cmp(&b.kind),
        "total" => a.total.cmp(&b.total),
        "free" => a.free.cmp(&b.free),
        _ => a.path.cmp(&b.path),
    };
    let order = primary.then_with(|| a.path.cmp(&b.path));
    if descending { order.reverse() } else { order }
}
#[cfg(test)]
mod ordering_tests {
    use super::*;
    fn volume(letter: &str, label: &str, total: Option<u64>, free: Option<u64>) -> Volume {
        Volume {
            path: letter.into(),
            label: label.into(),
            kind: 3,
            total,
            free,
            filesystem: String::new(),
            remote: None,
            read_only: false,
        }
    }
    #[test]
    fn drive_letter_capacity_and_available_space_are_independent() {
        let a = volume("C:\\", "Zebra", Some(100), Some(90));
        let b = volume("D:\\", "Alpha", Some(200), Some(80));
        assert!(compare(&a, &b, "name", false).is_lt());
        assert!(compare(&a, &b, "total", false).is_lt());
        assert!(compare(&a, &b, "free", false).is_gt());
        assert!(compare(&a, &b, "free", true).is_lt());
        let unknown = volume("E:\\", "Offline", None, None);
        assert!(compare(&unknown, &a, "free", false).is_lt());
    }
}
