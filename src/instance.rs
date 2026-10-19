//! Bounded handoff to an existing UI. No registry changes or resident service.
use crate::launch_request::LaunchRequest;
use std::{
    collections::VecDeque,
    hash::{Hash, Hasher},
    sync::Mutex,
    time::{Duration, Instant},
};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::{
    Win32::{Foundation::*, System::Threading::CreateMutexW, UI::WindowsAndMessaging::*},
    core::PCWSTR,
};
use windows_core::BOOL;

const TAG: usize = 0x52455832;
const MAX_PACKET: usize = 64 * 1024;
static REQUESTS: Mutex<VecDeque<(isize, LaunchRequest)>> = Mutex::new(VecDeque::new());

fn identity() -> String {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    std::env::current_exe().unwrap_or_default().hash(&mut hash);
    std::env::var_os("LOCALAPPDATA").hash(&mut hash);
    format!("RustExplorer.Open.v1.{:016x}", hash.finish())
}
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

pub struct InstanceGuard(HANDLE);
impl Drop for InstanceGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Only explicit external launches reuse windows. A normal launch still opens a new window.
pub fn handoff(request: Option<&LaunchRequest>) -> (Option<InstanceGuard>, bool) {
    let name = wide(&format!("Local\\{}", identity()));
    let guard = unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) };
    let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    let guard = guard.ok().map(InstanceGuard);
    let Some(request) = request else {
        return (guard, false);
    };
    let Ok(bytes) = serde_json::to_vec(request) else {
        return (guard, false);
    };
    if bytes.len() > MAX_PACKET {
        return (guard, false);
    }
    let start = Instant::now();
    loop {
        let property = wide(&identity());
        let mut search = Search {
            property: property.as_ptr(),
            hwnd: HWND::default(),
        };
        unsafe {
            let _ = EnumWindows(Some(find_window), LPARAM(&mut search as *mut _ as isize));
        }
        if !search.hwnd.is_invalid() {
            let packet = COPYDATASTRUCT {
                dwData: TAG,
                cbData: bytes.len() as u32,
                lpData: bytes.as_ptr() as *mut _,
            };
            let mut reply = 0;
            let accepted = unsafe {
                let mut pid = 0;
                GetWindowThreadProcessId(search.hwnd, Some(&mut pid));
                let _ = AllowSetForegroundWindow(pid);
                SendMessageTimeoutW(
                    search.hwnd,
                    WM_COPYDATA,
                    WPARAM(0),
                    LPARAM(&packet as *const _ as isize),
                    SMTO_ABORTIFHUNG | SMTO_BLOCK | SMTO_ERRORONEXIT,
                    750,
                    Some(&mut reply),
                )
            };
            return (guard, accepted.0 != 0 && reply == 1);
        }
        // A concurrently starting UI has not published its window yet.
        if !existed || start.elapsed() >= Duration::from_millis(1500) {
            return (guard, false);
        }
        std::thread::sleep(Duration::from_millis(15));
    }
}
struct Search {
    property: *const u16,
    hwnd: HWND,
}
unsafe extern "system" fn find_window(hwnd: HWND, param: LPARAM) -> BOOL {
    unsafe {
        let search = &mut *(param.0 as *mut Search);
        if !GetPropW(hwnd, PCWSTR(search.property)).is_invalid() {
            search.hwnd = hwnd;
            return BOOL(0);
        }
        BOOL(1)
    }
}
pub fn advertise(hwnd: isize) {
    let name = wide(&identity());
    unsafe {
        let _ = SetPropW(
            HWND(hwnd as *mut _),
            PCWSTR(name.as_ptr()),
            Some(HANDLE(1usize as *mut _)),
        );
    }
}
pub fn withdraw(hwnd: isize) {
    let name = wide(&identity());
    unsafe {
        let _ = RemovePropW(HWND(hwnd as *mut _), PCWSTR(name.as_ptr()));
    }
    if let Ok(mut queue) = REQUESTS.lock() {
        queue.retain(|(target, _)| *target != hwnd);
    }
}
/// Explicit folder requests must be visible even if a caller used SW_HIDE.
pub fn reveal_window(handle: isize) {
    let hwnd = HWND(handle as *mut _);
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        } else if !IsWindowVisible(hwnd).as_bool() {
            // SetWindowPos is not overridden by the caller's STARTUPINFO show mode.
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_SHOWWINDOW,
            );
        }
        let _ = SetForegroundWindow(hwnd);
    }
}
/// Called only by the window procedure for WM_COPYDATA; Windows owns packet memory.
pub unsafe fn receive(hwnd: HWND, param: LPARAM) -> LRESULT {
    if param.0 == 0 {
        return LRESULT(0);
    }
    let packet = unsafe { &*(param.0 as *const COPYDATASTRUCT) };
    if packet.dwData != TAG || packet.cbData as usize > MAX_PACKET || packet.lpData.is_null() {
        return LRESULT(0);
    }
    let bytes =
        unsafe { std::slice::from_raw_parts(packet.lpData as *const u8, packet.cbData as usize) };
    let Ok(request) = serde_json::from_slice::<LaunchRequest>(bytes) else {
        return LRESULT(0);
    };
    if !request.folder.is_absolute()
        || request
            .select
            .iter()
            .any(|p| p.parent() != Some(request.folder.as_path()))
    {
        return LRESULT(0);
    }
    let Ok(mut queue) = REQUESTS.lock() else {
        return LRESULT(0);
    };
    if queue.len() >= 32 {
        return LRESULT(0);
    }
    queue.push_back((hwnd.0 as isize, request));
    drop(queue);
    reveal_window(hwnd.0 as isize);
    LRESULT(1)
}
pub fn take(hwnd: isize) -> Option<LaunchRequest> {
    let mut queue = REQUESTS.lock().ok()?;
    let index = queue.iter().position(|(target, _)| *target == hwnd)?;
    queue.remove(index).map(|(_, request)| request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };

    #[test]
    fn handoff_delivers_to_live_window_and_recovers_after_window_closes() {
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::spawn(move || unsafe {
            let hwnd = CreateWindowExW(
                Default::default(),
                windows::core::w!("STATIC"),
                windows::core::w!("RE IPC test"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            crate::window_frame::install(hwnd.0 as isize).unwrap();
            advertise(hwnd.0 as isize);
            tx.send(hwnd.0 as isize).unwrap();
            while !stopped.load(Ordering::Relaxed) {
                let mut message = MSG::default();
                while PeekMessageW(&mut message, Some(hwnd), 0, 0, PM_REMOVE).as_bool() {
                    DispatchMessageW(&message);
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            DestroyWindow(hwnd).unwrap();
        });
        let hwnd = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let folder = std::env::temp_dir();
        let request = LaunchRequest {
            folder: folder.clone(),
            select: vec![folder.join("selected.txt")],
        };
        for _ in 0..3 {
            let (_, forwarded) = handoff(Some(&request));
            assert!(forwarded);
            assert_eq!(take(hwnd), Some(request.clone()));
        }
        let invalid = LaunchRequest {
            folder: "relative".into(),
            select: vec![],
        };
        assert!(!handoff(Some(&invalid)).1);
        assert!(take(hwnd).is_none());
        stop.store(true, Ordering::Relaxed);
        thread.join().unwrap();
        assert!(take(hwnd).is_none());
        assert!(!handoff(Some(&request)).1);
    }
}
