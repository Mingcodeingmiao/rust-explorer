//! Read-only helpers must not trigger Windows' application-starting cursor.
//! CREATE_NO_WINDOW alone only controls console windows; GUI-subsystem helpers
//! also need STARTF_FORCEOFFFEEDBACK in STARTUPINFO. Keep explicit Shell actions
//! and owner-backed menus out of this launcher.
use std::{
    ffi::OsStr,
    fs::File,
    io,
    mem::size_of,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::ExitStatusExt,
    },
    process::ExitStatus,
};
use windows::{
    Win32::{
        Foundation::{
            GENERIC_READ, GENERIC_WRITE, HANDLE, HANDLE_FLAG_INHERIT, HANDLE_FLAGS,
            SetHandleInformation, WAIT_OBJECT_0, WAIT_TIMEOUT,
        },
        Security::SECURITY_ATTRIBUTES,
        Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        },
        System::{Pipes::CreatePipe, Threading::*},
    },
    core::{PCWSTR, PWSTR, w},
};

fn raw(handle: &OwnedHandle) -> HANDLE {
    HANDLE(handle.as_raw_handle())
}
unsafe fn owned(handle: HANDLE) -> OwnedHandle {
    unsafe { OwnedHandle::from_raw_handle(handle.0) }
}
fn inheritable() -> SECURITY_ATTRIBUTES {
    SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        bInheritHandle: true.into(),
        ..Default::default()
    }
}
fn pipe(parent_reads: bool) -> io::Result<(OwnedHandle, OwnedHandle)> {
    let (mut read, mut write) = (HANDLE::default(), HANDLE::default());
    unsafe { CreatePipe(&mut read, &mut write, Some(&inheritable()), 0) }
        .map_err(io::Error::other)?;
    let (read, write) = unsafe { (owned(read), owned(write)) };
    let (parent, child) = if parent_reads {
        (read, write)
    } else {
        (write, read)
    };
    unsafe { SetHandleInformation(raw(&parent), HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0)) }
        .map_err(io::Error::other)?;
    Ok((parent, child))
}
fn nul(read: bool) -> io::Result<OwnedHandle> {
    let handle = unsafe {
        CreateFileW(
            w!("NUL"),
            if read {
                GENERIC_READ.0
            } else {
                GENERIC_WRITE.0
            },
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            Some(&inheritable()),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    }
    .map_err(io::Error::other)?;
    Ok(unsafe { owned(handle) })
}

struct Attributes {
    list: LPPROC_THREAD_ATTRIBUTE_LIST,
    _storage: Vec<usize>,
}
impl Attributes {
    fn new(handles: &[HANDLE]) -> io::Result<Self> {
        let mut bytes = 0;
        // The first call deliberately queries the required buffer length.
        let _ = unsafe { InitializeProcThreadAttributeList(None, 1, None, &mut bytes) };
        if bytes == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
        let list = LPPROC_THREAD_ATTRIBUTE_LIST(storage.as_mut_ptr().cast());
        unsafe { InitializeProcThreadAttributeList(Some(list), 1, None, &mut bytes) }
            .map_err(io::Error::other)?;
        let attributes = Self {
            list,
            _storage: storage,
        };
        // A multithreaded parent may have other inheritable pipes in flight.
        // Inherit only these three child ends, never another helper's handles.
        unsafe {
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                Some(handles.as_ptr().cast()),
                size_of_val(handles),
                None,
                None,
            )
        }
        .map_err(io::Error::other)?;
        Ok(attributes)
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        unsafe { DeleteProcThreadAttributeList(self.list) };
    }
}

fn quote(value: &OsStr, output: &mut Vec<u16>) -> io::Result<()> {
    output.push(b'"' as u16);
    let mut slashes = 0;
    for ch in value.encode_wide() {
        if ch == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "NUL in helper argument",
            ));
        }
        if ch == b'\\' as u16 {
            slashes += 1;
            continue;
        }
        output.extend(std::iter::repeat_n(
            b'\\' as u16,
            slashes * if ch == b'"' as u16 { 2 } else { 1 },
        ));
        slashes = 0;
        if ch == b'"' as u16 {
            output.push(b'\\' as u16);
        }
        output.push(ch);
    }
    output.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
    output.push(b'"' as u16);
    Ok(())
}

pub struct Child {
    process: OwnedHandle,
    pub stdin: Option<File>,
    pub stdout: Option<File>,
    pub stderr: Option<File>,
}
impl Child {
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        match unsafe { WaitForSingleObject(raw(&self.process), 0) } {
            WAIT_TIMEOUT => Ok(None),
            WAIT_OBJECT_0 => self.status().map(Some),
            _ => Err(io::Error::last_os_error()),
        }
    }
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        if unsafe { WaitForSingleObject(raw(&self.process), INFINITE) } != WAIT_OBJECT_0 {
            return Err(io::Error::last_os_error());
        }
        self.status()
    }
    fn status(&self) -> io::Result<ExitStatus> {
        let mut code = 0;
        unsafe { GetExitCodeProcess(raw(&self.process), &mut code) }.map_err(io::Error::other)?;
        Ok(ExitStatus::from_raw(code))
    }
    pub fn kill(&mut self) -> io::Result<()> {
        unsafe { TerminateProcess(raw(&self.process), 1) }.map_err(io::Error::other)
    }
}

pub fn spawn<S: AsRef<OsStr>>(
    args: impl IntoIterator<Item = S>,
    pipe_stdin: bool,
    capture_stderr: bool,
) -> io::Result<Child> {
    let exe = std::env::current_exe()?;
    let mut application: Vec<u16> = exe.as_os_str().encode_wide().collect();
    application.push(0);
    let mut command = Vec::new();
    quote(exe.as_os_str(), &mut command)?;
    for arg in args {
        command.push(b' ' as u16);
        quote(arg.as_ref(), &mut command)?;
    }
    command.push(0);
    if command.len() > 32767 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Helper command line exceeds Windows limit",
        ));
    }
    let (stdin, child_in) = if pipe_stdin {
        let (parent, child) = pipe(false)?;
        (Some(File::from(parent)), child)
    } else {
        (None, nul(true)?)
    };
    let (stdout, child_out) = pipe(true)?;
    let (stderr, child_err) = if capture_stderr {
        let (parent, child) = pipe(true)?;
        (Some(File::from(parent)), child)
    } else {
        (None, nul(false)?)
    };
    let handles = [raw(&child_in), raw(&child_out), raw(&child_err)];
    let attributes = Attributes::new(&handles)?;
    let startup = STARTUPINFOEXW {
        StartupInfo: STARTUPINFOW {
            cb: size_of::<STARTUPINFOEXW>() as u32,
            dwFlags: STARTF_USESTDHANDLES | STARTF_FORCEOFFFEEDBACK,
            hStdInput: handles[0],
            hStdOutput: handles[1],
            hStdError: handles[2],
            ..Default::default()
        },
        lpAttributeList: attributes.list,
    };
    let mut process = PROCESS_INFORMATION::default();
    unsafe {
        CreateProcessW(
            PCWSTR(application.as_ptr()),
            Some(PWSTR(command.as_mut_ptr())),
            None,
            None,
            true,
            CREATE_NO_WINDOW | EXTENDED_STARTUPINFO_PRESENT,
            None,
            PCWSTR::null(),
            &startup.StartupInfo,
            &mut process,
        )
    }
    .map_err(io::Error::other)?;
    let (process, thread) = unsafe { (owned(process.hProcess), owned(process.hThread)) };
    drop(thread);
    Ok(Child {
        process,
        stdin,
        stdout: Some(File::from(stdout)),
        stderr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        time::{Duration, Instant},
    };

    const PROBE: &str = "background_process::tests::probe_child";

    fn wait_bounded(child: &mut Child) -> ExitStatus {
        let start = Instant::now();
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                return status;
            }
            if start.elapsed() > Duration::from_secs(10) {
                let _ = child.kill();
                let _ = child.wait();
                panic!("background helper probe timed out");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn silent_startup_preserves_unicode_json_and_all_standard_streams() {
        let payload = r#"payload={"path":"C:\\测试 space\\a\"b\\","text":"& | $ ()"}"#;
        let mut child = spawn(
            ["--ignored", "--exact", PROBE, "--nocapture", payload],
            true,
            true,
        )
        .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(b"probe input").unwrap();
        drop(stdin);
        assert!(wait_bounded(&mut child).success());
        let mut output = String::new();
        child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut output)
            .unwrap();
        let start = output.find("BACKGROUND_PROBE ").expect(&output) + "BACKGROUND_PROBE ".len();
        let result: serde_json::Value =
            serde_json::from_str(output[start..].lines().next().unwrap()).unwrap();
        let flags = result["flags"].as_u64().unwrap() as u32;
        assert_ne!(flags & STARTF_FORCEOFFFEEDBACK.0, 0);
        assert_ne!(flags & STARTF_USESTDHANDLES.0, 0);
        assert_eq!(flags & STARTF_FORCEONFEEDBACK.0, 0);
        assert_eq!(result["argument"], payload);
        assert_eq!(result["input"], "probe input");
        let mut errors = String::new();
        child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut errors)
            .unwrap();
        assert!(errors.contains("background probe stderr"));
    }

    #[test]
    fn silent_helper_remains_killable_and_reapable() {
        let mut child = spawn(
            ["--ignored", "--exact", PROBE, "--nocapture", "wait"],
            true,
            false,
        )
        .unwrap();
        // The child waits for stdin EOF; keep the writer open to model a stuck provider.
        assert!(child.try_wait().unwrap().is_none());
        child.kill().unwrap();
        assert!(!wait_bounded(&mut child).success());
        assert!(!child.wait().unwrap().success());
    }

    #[test]
    #[ignore = "child entry point used by background-process contract tests"]
    fn probe_child() {
        let argument = std::env::args().last().unwrap();
        if !argument.starts_with("payload=") && argument != "wait" {
            // A developer running all ignored tests must not block on stdin.
            return;
        }
        let mut info = STARTUPINFOW::default();
        unsafe { GetStartupInfoW(&mut info) };
        let mut input = String::new();
        std::io::stdin().read_to_string(&mut input).unwrap();
        println!(
            "BACKGROUND_PROBE {}",
            serde_json::json!({
                "flags": info.dwFlags.0,
                "argument": argument,
                "input": input,
            })
        );
        eprintln!("background probe stderr");
    }
}
