//! Shell selection callbacks for default-folder launches; association recovery stays independent.
use crate::launch_request::{self, LaunchRequest};
use anyhow::{Context, Result, ensure};
use std::{
    ffi::{OsString, c_void},
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
};

unsafe extern "C" {
    fn re_shell_bridge(
        folder: *const u16,
        delivery: extern "C" fn(*const u16, i32, *mut c_void) -> i32,
        context: *mut c_void,
        initial_ms: u32,
        lifetime_ms: u32,
    ) -> i32;
}

struct Delivery {
    executable: PathBuf,
    delivered: bool,
    error: Option<String>,
}
fn dispatch(executable: &Path, request: &LaunchRequest) -> Result<()> {
    let (_guard, forwarded) = crate::instance::handoff(Some(request));
    if !forwarded {
        let (flag, paths) = if request.select.is_empty() {
            ("--browse-folder", vec![request.folder.as_path()])
        } else {
            (
                "--select",
                request.select.iter().map(PathBuf::as_path).collect(),
            )
        };
        spawn_ui(executable, flag, &paths)?;
    }
    Ok(())
}
fn spawn_ui(executable: &Path, flag: &str, paths: &[&Path]) -> Result<()> {
    use windows::{
        Win32::{Foundation::CloseHandle, System::Threading::*},
        core::{PCWSTR, PWSTR},
    };
    fn quote(path: &Path) -> Result<String> {
        let text = path.to_str().context("Invalid launch path")?;
        ensure!(!text.contains(['"', '\0']), "Invalid launch path quoting");
        let trailing = text.chars().rev().take_while(|c| *c == '\\').count();
        Ok(format!("\"{text}{}\"", "\\".repeat(trailing)))
    }
    let application: Vec<u16> = executable
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let mut text = format!("{} {flag}", quote(executable)?);
    for path in paths {
        text.push(' ');
        text.push_str(&quote(path)?);
    }
    let mut command: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    let startup = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut process = PROCESS_INFORMATION::default();
    // Explicit bInheritHandles=false: a long-lived GUI must not retain its caller's pipes.
    unsafe {
        CreateProcessW(
            PCWSTR(application.as_ptr()),
            Some(PWSTR(command.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_NO_WINDOW,
            None,
            PCWSTR::null(),
            &startup,
            &mut process,
        )
        .context("无法启动文件夹窗口")?;
        let _ = CloseHandle(process.hThread);
        let _ = CloseHandle(process.hProcess);
    }
    Ok(())
}
extern "C" fn receive(path: *const u16, kind: i32, context: *mut c_void) -> i32 {
    if kind == 0 {
        return 1;
    }
    // Native code owns the UTF-16 buffer for this synchronous callback only.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
        ensure!(
            !path.is_null() && !context.is_null(),
            "Invalid Shell callback"
        );
        let mut length = 0;
        unsafe {
            while length < 32768 && *path.add(length) != 0 {
                length += 1;
            }
        }
        ensure!(length < 32768, "Shell callback path is too long");
        let target = unsafe { OsString::from_wide(std::slice::from_raw_parts(path, length)) };
        let request = launch_request::parse(&[
            OsString::from(if kind == 2 {
                "--select"
            } else {
                "--open-folder"
            }),
            target,
        ])?
        .context("Invalid Shell target")?;
        let delivery = unsafe { &mut *(context as *mut Delivery) };
        dispatch(&delivery.executable, &request)?;
        delivery.delivered = true;
        Ok(())
    }));
    match result {
        Ok(Ok(())) => 1,
        other => {
            if !context.is_null() {
                let delivery = unsafe { &mut *(context as *mut Delivery) };
                delivery.error = Some(match other {
                    Ok(Err(error)) => format!("{error:#}"),
                    _ => "Shell callback failed".into(),
                });
            }
            0
        }
    }
}

pub fn open(path: &Path) -> Result<()> {
    let request = launch_request::parse(&["--open-folder".into(), path.as_os_str().into()])?
        .context("Missing folder request")?;
    let executable = std::env::current_exe()?;
    if !request.select.is_empty() {
        return dispatch(&executable, &request);
    }
    let folder: Vec<u16> = request
        .folder
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let mut delivery = Delivery {
        executable,
        delivered: false,
        error: None,
    };
    let code = unsafe {
        re_shell_bridge(
            folder.as_ptr(),
            receive,
            &mut delivery as *mut _ as *mut c_void,
            100,
            8000,
        )
    };
    // Missing/unavailable Shell services must still open the requested folder.
    if !delivery.delivered {
        dispatch(&delivery.executable, &request)?;
    }
    if let Some(error) = delivery.error {
        eprintln!("Shell selection fallback ({code}): {error}");
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    extern "C" fn observe(_: *const u16, kind: i32, context: *mut c_void) -> i32 {
        unsafe { &mut *(context as *mut Vec<i32>) }.push(kind);
        1
    }
    #[test]
    fn native_session_delivers_directory_and_finishes_without_a_selection() {
        let folder = std::env::temp_dir().join(format!("RE-core-shell-{}", std::process::id()));
        std::fs::create_dir(&folder).unwrap();
        let path: Vec<u16> = folder.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut events = Vec::<i32>::new();
        let started = std::time::Instant::now();
        let code = unsafe {
            re_shell_bridge(
                path.as_ptr(),
                observe,
                &mut events as *mut _ as *mut c_void,
                5,
                40,
            )
        };
        std::fs::remove_dir(folder).unwrap();
        assert_eq!(code, 0);
        assert_eq!(events, vec![0, 1]);
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
    }
}
