//! Restoring network locations Windows has not mounted yet.
//!
//! A mapped drive whose server was unreachable at sign-in stays in the user's
//! profile as a disconnected mapping: the drive letter is listed, but opening it
//! fails with "path not found" because the connection behind it is gone.
//! Explorer reconnects such a drive when it is opened; this module does the same
//! before a folder is listed.
//!
//! A bare `\\server` address is the other shape of the same problem. It has no
//! filesystem entry point at all — the folder content is the list of shares the
//! server publishes, which is what Explorer shows for that address.
//!
//! Mounting and share enumeration block for as long as the SMB redirector needs
//! (many seconds when the server is unreachable), so the GUI only runs them in
//! the disposable helper process: `--network-mount` and `--network-shares` in
//! `main.rs`, reached through [`mount_bounded`] and [`shares_bounded`].

use crate::fs::Entry;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    mem::size_of,
    path::{Path, PathBuf},
    time::Duration,
};
use windows::{
    Win32::{
        Foundation::{
            ERROR_ALREADY_ASSIGNED, ERROR_CONNECTION_UNAVAIL, ERROR_DEVICE_ALREADY_REMEMBERED,
            ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_SESSION_CREDENTIAL_CONFLICT, ERROR_SUCCESS,
            HANDLE, WIN32_ERROR,
        },
        NetworkManagement::WNet::{
            CONNECT_INTERACTIVE, NET_CONNECT_FLAGS, NETRESOURCEW, RESOURCE_GLOBALNET,
            RESOURCETYPE_DISK, RESOURCEUSAGE_CONNECTABLE, WNetAddConnection2W, WNetCloseEnum,
            WNetEnumResourceW, WNetGetConnectionW, WNetOpenEnumW,
        },
        Storage::FileSystem::GetDriveTypeW,
    },
    core::{PCWSTR, PWSTR},
};

/// `DRIVE_REMOTE` is not exported by the windows crate; it is the stable
/// `GetDriveTypeW` value for a connected network drive.
const DRIVE_REMOTE: u32 = 4;

/// A share published by an SMB server, shown when a `\\server` address is
/// opened.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Share {
    pub name: String,
    /// UNC path of the share root (`\\server\share`).
    pub path: String,
    /// Server comment for the share, empty when it has none.
    #[serde(default)]
    pub comment: String,
}

impl Share {
    /// Folder entry for the share. Its path is a real UNC path, so opening it
    /// navigates normally instead of entering a namespace of its own.
    pub fn entry(&self) -> Entry {
        Entry {
            recycle: None,
            path: PathBuf::from(&self.path),
            name: self.name.clone(),
            directory: true,
            hidden: false,
            system: false,
            link: false,
            size: 0,
            total: None,
            free: None,
            modified: None,
            created: None,
            kind: crate::i18n::text("文件夹").into(),
        }
    }
}

/// A `\\server` (or `\\server\`) address: a server root has no filesystem
/// listing, only shares.
pub fn is_server_root(path: &Path) -> bool {
    unc_parts(path).is_some_and(|parts| parts.share.is_none())
}

/// The `\\server\share` root of a UNC path, if it is one.
pub fn share_root(path: &Path) -> Option<PathBuf> {
    let parts = unc_parts(path)?;
    let share = parts.share?;
    Some(PathBuf::from(format!(r"\\{}\{}", parts.server, share)))
}

/// The `\\server` address of a UNC path.
pub fn server_root(path: &Path) -> Option<PathBuf> {
    unc_parts(path).map(|parts| PathBuf::from(format!(r"\\{}", parts.server)))
}

/// Whether the path is exactly a share root (`\\server\share`, nothing deeper).
pub fn is_share_root(path: &Path) -> bool {
    unc_parts(path).is_some_and(|parts| parts.share.is_some() && parts.deeper.is_empty())
}

struct UncParts {
    server: String,
    share: Option<String>,
    /// Components below the share, in order.
    deeper: Vec<String>,
}

fn unc_parts(path: &Path) -> Option<UncParts> {
    let text = path.to_string_lossy();
    let rest = text.strip_prefix(r"\\")?;
    // `\\?\` and `\\.\` are device paths, not network addresses.
    if rest.starts_with('?') || rest.starts_with('.') {
        return None;
    }
    let mut parts = rest.split(['\\', '/']).filter(|part| !part.is_empty());
    let server = parts.next()?;
    let share = parts.next().map(str::to_string);
    let deeper = parts.map(str::to_string).collect();
    Some(UncParts {
        server: server.to_string(),
        share,
        deeper,
    })
}

/// Address-bar segments of a UNC path, or `None` for a non-network path.
///
/// Rust treats `\\server\share` as one atomic prefix, so neither `file_name` nor
/// `ancestors` can separate the server from the share. They are different
/// locations here — `\\server` lists the shares — so the breadcrumb has to be
/// built from the components instead.
pub fn breadcrumbs(path: &Path) -> Option<Vec<(String, PathBuf)>> {
    let parts = unc_parts(path)?;
    let mut segments = Vec::with_capacity(1 + parts.share.is_some() as usize + parts.deeper.len());
    let mut current = PathBuf::from(format!(r"\\{}", parts.server));
    segments.push((parts.server.clone(), current.clone()));
    let Some(share) = parts.share else {
        return Some(segments);
    };
    current = current.join(&share);
    segments.push((share, current.clone()));
    for part in parts.deeper {
        current = current.join(&part);
        segments.push((part, current.clone()));
    }
    Some(segments)
}

/// Drive letter of a Windows path (`Z:`, `Z:\folder`), normalized to upper case.
fn drive_letter(path: &Path) -> Option<char> {
    let text = path.to_string_lossy();
    let mut chars = text.chars();
    let letter = chars.next()?;
    if !letter.is_ascii_alphabetic() || chars.next()? != ':' {
        return None;
    }
    match chars.next() {
        None | Some('\\') | Some('/') => Some(letter.to_ascii_uppercase()),
        // `Z:relative` is drive-relative, not a drive root.
        Some(_) => None,
    }
}

fn drive_type(root: &str) -> u32 {
    let root = wide(root);
    unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) }
}

/// Remote path of a mapped drive, including a mapping that is currently
/// disconnected. The per-user mapping is the fallback because the redirector
/// may have forgotten the connection entirely.
pub fn mapped_remote(letter: char) -> Option<String> {
    connection_remote(letter)
        .or_else(|| {
            windows_registry::CURRENT_USER
                .open(format!("Network\\{letter}"))
                .and_then(|key| key.get_string("RemotePath"))
                .ok()
        })
        .filter(|remote| !remote.trim().is_empty())
}

fn connection_remote(letter: char) -> Option<String> {
    let local = wide(&format!("{letter}:"));
    let mut buffer = vec![0u16; 1024];
    let mut size = buffer.len() as u32;
    let error = unsafe {
        WNetGetConnectionW(
            PCWSTR(local.as_ptr()),
            Some(PWSTR(buffer.as_mut_ptr())),
            &mut size,
        )
    };
    // A disconnected mapping still reports its remote name, with
    // ERROR_CONNECTION_UNAVAIL instead of ERROR_SUCCESS.
    if error != ERROR_SUCCESS && error != ERROR_MORE_DATA && error != ERROR_CONNECTION_UNAVAIL {
        return None;
    }
    let text = String::from_utf16_lossy(&buffer);
    let text = text.trim_end_matches('\0').trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Whether this root belongs to a network location. Its Shell icon carries the
/// connection overlay — the red X on a mapped drive whose connection is gone —
/// which changes without the path changing.
pub fn is_network_root(path: &Path) -> bool {
    if unc_parts(path).is_some() {
        return true;
    }
    let Some(letter) = drive_letter(path) else {
        return false;
    };
    // A disconnected mapping does not report DRIVE_REMOTE in every case (a
    // connection the system dropped at sign-in does, one this process cancelled
    // does not), but the per-user mapping entry is there in all of them.
    drive_type(&format!("{letter}:\\")) == DRIVE_REMOTE
        || windows_registry::CURRENT_USER
            .open(format!("Network\\{letter}"))
            .is_ok()
}

/// Whether opening this path has to restore a connection first. Cheap: a drive
/// root costs one existence probe, a share root the same.
pub fn needs_mount(path: &Path) -> bool {
    if let Some(letter) = drive_letter(path) {
        // A disconnected mapping still reports DRIVE_REMOTE, so the drive type
        // cannot answer this; the root directory it would back is missing.
        if std::path::Path::new(&format!("{letter}:\\")).exists() {
            return false;
        }
        return mapped_remote(letter).is_some();
    }
    // The redirector connects a share on first access when it can; only the
    // "not mounted" state, where that failed, asks for an explicit mount.
    is_share_root(path) && share_root(path).is_some_and(|share| !share.exists())
}

/// Restore the connection behind `path` (runs in the helper process).
pub fn mount(path: &Path) -> Result<()> {
    if let Some(letter) = drive_letter(path) {
        if std::path::Path::new(&format!("{letter}:\\")).exists() {
            return Ok(());
        }
        // A drive without a saved mapping is not a network location; nothing to
        // restore, and the listing reports whatever is actually wrong with it.
        let Some(remote) = mapped_remote(letter) else {
            return Ok(());
        };
        connect(Some(&format!("{letter}:")), &remote)
            .with_context(|| format!("无法连接 {letter}: ({remote})"))
    } else if let Some(share) = share_root(path) {
        if share.exists() {
            return Ok(());
        }
        let remote = share.to_string_lossy().into_owned();
        connect(None, &remote).with_context(|| format!("无法连接 {remote}"))
    } else {
        // A `\\server` root is connected by the share enumeration itself.
        Ok(())
    }
}

/// Mount in the disposable helper process, which is killed after `timeout`.
pub fn mount_bounded(path: &Path) -> bool {
    let Ok(request) = serde_json::to_string(path) else {
        return false;
    };
    let mounted =
        crate::volumes::bounded::<bool>("--network-mount", &request, Duration::from_secs(20))
            .unwrap_or(false);
    if mounted {
        // The Shell overlaid a red X while the drive was disconnected and the
        // cached icon cannot notice the new connection on its own.
        crate::native_assets::refresh_network_icons();
    }
    mounted
}

/// Shares of a `\\server` root, enumerated in the disposable helper process.
pub fn shares_bounded(path: &Path) -> Result<Vec<Share>> {
    let request = serde_json::to_string(path)?;
    crate::volumes::bounded("--network-shares", &request, Duration::from_secs(20))
}

/// Enumerate the shares published by the server in `path`.
pub fn list_shares(path: &Path) -> Result<Vec<Share>> {
    let Some(parts) = unc_parts(path) else {
        bail!("{} 不是网络位置", path.display());
    };
    if parts.share.is_some() {
        bail!("{} 已经是共享路径", path.display());
    }
    let server = format!(r"\\{}", parts.server);
    let mut handle = HANDLE(std::ptr::null_mut());
    let mut error = open_server_enum(&server, &mut handle);
    if error != ERROR_SUCCESS {
        // Nothing has authenticated to this server yet. `net view` connects to
        // the server's IPC share first; do the same and retry once.
        let _ = connect(None, &format!(r"{server}\IPC$"));
        error = open_server_enum(&server, &mut handle);
    }
    if error != ERROR_SUCCESS {
        bail!("无法连接 {}：{}", server, os_error(error));
    }
    let mut shares = Vec::new();
    let mut buffer = vec![0u8; 16 * 1024];
    loop {
        let mut count = 32u32;
        let mut size = buffer.len() as u32;
        let error =
            unsafe { WNetEnumResourceW(handle, &mut count, buffer.as_mut_ptr().cast(), &mut size) };
        if error == ERROR_NO_MORE_ITEMS {
            break;
        }
        if error == ERROR_MORE_DATA {
            // The buffer records the size the next call needs.
            if size as usize > buffer.len() {
                buffer.resize(size as usize, 0);
                continue;
            }
            break;
        }
        if error != ERROR_SUCCESS {
            let _ = unsafe { WNetCloseEnum(handle) };
            bail!("无法读取 {} 的共享：{}", server, os_error(error));
        }
        for index in 0..count as usize {
            // The API returns an array of NETRESOURCEW followed by the strings
            // they point at; the buffer keeps both alive.
            let entry = unsafe {
                std::ptr::read_unaligned(
                    buffer.as_ptr().add(index * size_of::<NETRESOURCEW>()) as *const NETRESOURCEW
                )
            };
            let remote = unsafe { entry.lpRemoteName.to_string() }.unwrap_or_default();
            let Some(name) = remote
                .rsplit(['\\', '/'])
                .next()
                .filter(|name| !name.is_empty() && !name.ends_with('$'))
            else {
                continue;
            };
            if shares.iter().any(|share: &Share| share.name == name) {
                continue;
            }
            shares.push(Share {
                name: name.to_string(),
                path: format!(r"{server}\{name}"),
                comment: unsafe { entry.lpComment.to_string() }.unwrap_or_default(),
            });
        }
    }
    let _ = unsafe { WNetCloseEnum(handle) };
    shares.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(shares)
}

fn open_server_enum(server: &str, handle: &mut HANDLE) -> WIN32_ERROR {
    let remote = wide(server);
    let resource = NETRESOURCEW {
        dwType: RESOURCETYPE_DISK,
        dwUsage: RESOURCEUSAGE_CONNECTABLE.0,
        lpRemoteName: PWSTR(remote.as_ptr().cast_mut()),
        ..Default::default()
    };
    unsafe {
        WNetOpenEnumW(
            RESOURCE_GLOBALNET,
            RESOURCETYPE_DISK,
            RESOURCEUSAGE_CONNECTABLE,
            Some(&resource),
            handle,
        )
    }
}

/// Connect once with the logon session, then once through the credential
/// manager. The second call is what `net use` does when a server rejects the
/// implicit credentials; it is also what makes a mapping whose saved password
/// lives in the credential manager reconnect without asking the user.
fn connect(local: Option<&str>, remote: &str) -> Result<()> {
    let mut error = add_connection(local, remote, NET_CONNECT_FLAGS(0));
    if error != ERROR_SUCCESS && !already_connected(error) {
        error = add_connection(local, remote, CONNECT_INTERACTIVE);
    }
    if error == ERROR_SUCCESS || already_connected(error) {
        Ok(())
    } else {
        bail!("{}", os_error(error))
    }
}

fn add_connection(local: Option<&str>, remote: &str, flags: NET_CONNECT_FLAGS) -> WIN32_ERROR {
    let local = local.map(wide);
    let remote = wide(remote);
    let resource = NETRESOURCEW {
        dwType: RESOURCETYPE_DISK,
        lpLocalName: local
            .as_ref()
            .map_or(PWSTR::null(), |value| PWSTR(value.as_ptr().cast_mut())),
        lpRemoteName: PWSTR(remote.as_ptr().cast_mut()),
        ..Default::default()
    };
    unsafe { WNetAddConnection2W(&resource, PWSTR::null(), PWSTR::null(), flags) }
}

/// Errors that mean the connection the caller asked for already exists.
fn already_connected(error: WIN32_ERROR) -> bool {
    error == ERROR_ALREADY_ASSIGNED
        || error == ERROR_DEVICE_ALREADY_REMEMBERED
        || error == ERROR_SESSION_CREDENTIAL_CONFLICT
}

fn os_error(error: WIN32_ERROR) -> String {
    std::io::Error::from_raw_os_error(error.0 as i32).to_string()
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_roots_and_share_paths_are_told_apart() {
        assert!(is_server_root(Path::new(r"\\10.0.16.48")));
        assert!(is_server_root(Path::new(r"\\10.0.16.48\")));
        assert!(!is_server_root(Path::new(r"\\10.0.16.48\priv")));
        assert_eq!(
            share_root(Path::new(r"\\nas\media\2026\")),
            Some(PathBuf::from(r"\\nas\media"))
        );
        assert_eq!(
            server_root(Path::new(r"\\nas\media\2026")),
            Some(PathBuf::from(r"\\nas"))
        );
        assert_eq!(server_root(Path::new(r"C:\Windows")), None);
        assert!(is_share_root(Path::new(r"\\nas\media")));
        assert!(is_share_root(Path::new(r"\\nas\media\")));
        assert!(!is_share_root(Path::new(r"\\nas\media\2026")));
        assert_eq!(share_root(Path::new(r"C:\Windows")), None);
        assert!(!is_server_root(Path::new(r"C:\Windows")));
    }

    #[test]
    fn device_paths_are_not_network_locations() {
        assert!(!is_server_root(Path::new(r"\\?\C:\Windows")));
        assert!(!is_server_root(Path::new(r"\\.\pipe\name")));
        assert_eq!(share_root(Path::new(r"\\?\UNC\server\share")), None);
        assert!(!is_network_root(Path::new(r"\\?\C:\Windows")));
    }

    #[test]
    fn share_roots_are_network_roots_but_local_paths_are_not() {
        assert!(is_network_root(Path::new(r"\\nas\media")));
        assert!(is_network_root(Path::new(r"\\nas")));
        assert!(!is_network_root(Path::new(r"C:\")));
        assert!(!is_network_root(Path::new(r"C:\Windows")));
    }

    #[test]
    fn drive_letters_are_recognized_only_as_letter_paths() {
        assert_eq!(drive_letter(Path::new(r"Z:")), Some('Z'));
        assert_eq!(drive_letter(Path::new(r"z:\")), Some('Z'));
        assert_eq!(drive_letter(Path::new(r"Z:\folder")), Some('Z'));
        assert_eq!(drive_letter(Path::new(r"Z:relative")), None);
        assert_eq!(drive_letter(Path::new(r"\\server\share")), None);
    }

    #[test]
    fn unc_breadcrumbs_keep_the_server_and_share_as_separate_locations() {
        assert_eq!(
            breadcrumbs(Path::new(r"\\10.0.16.48\D\ProgramData")),
            Some(vec![
                ("10.0.16.48".to_string(), PathBuf::from(r"\\10.0.16.48")),
                ("D".to_string(), PathBuf::from(r"\\10.0.16.48\D")),
                (
                    "ProgramData".to_string(),
                    PathBuf::from(r"\\10.0.16.48\D\ProgramData")
                ),
            ])
        );
        assert_eq!(
            breadcrumbs(Path::new(r"\\10.0.16.48")),
            Some(vec![(
                "10.0.16.48".to_string(),
                PathBuf::from(r"\\10.0.16.48")
            )])
        );
        // Trailing separators and forward slashes must not add empty segments.
        assert_eq!(
            breadcrumbs(Path::new(r"\\nas/media/")),
            Some(vec![
                ("nas".to_string(), PathBuf::from(r"\\nas")),
                ("media".to_string(), PathBuf::from(r"\\nas\media")),
            ])
        );
        assert_eq!(breadcrumbs(Path::new(r"C:\Windows")), None);
    }
}
