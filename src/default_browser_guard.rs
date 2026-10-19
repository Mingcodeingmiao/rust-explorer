//! A copied, independent executable rolls back unconfirmed trials even if the GUI dies.
use super::*;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
struct GuardProcess(HANDLE);
impl Drop for GuardProcess {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
impl GuardProcess {
    fn exited(&self) -> Result<bool> {
        let mut code = 0;
        unsafe {
            windows::Win32::System::Threading::GetExitCodeProcess(self.0, &mut code)?;
        }
        Ok(code != 259)
    }
}
fn spawn_guard(exe: &std::path::Path, token: &str) -> Result<GuardProcess> {
    use windows::{
        Win32::System::Threading::*,
        core::{PCWSTR, PWSTR},
    };
    let path = exe.to_str().context("Invalid recovery path")?;
    ensure!(!path.contains('"'), "Invalid recovery path quoting");
    let application: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    let mut command: Vec<u16> = format!("\"{path}\" --folder-association-guard {token}")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let startup = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut process = PROCESS_INFORMATION::default();
    // bInheritHandles=false is essential: inherited pipe/lock handles would tie
    // this recovery process to its caller even when stdout is redirected to NUL.
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
        )?;
        let _ = CloseHandle(process.hThread);
    }
    Ok(GuardProcess(process.hProcess))
}

#[derive(Serialize, Deserialize)]
struct Lease {
    token: String,
    deadline: u64,
    journal: Vec<u8>,
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn load(dir: &std::path::Path) -> Result<Lease> {
    let bytes = fs::read(dir.join("trial.json"))?;
    ensure!(bytes.len() <= 4 * 1024 * 1024, "Invalid trial journal size");
    let lease: Lease = serde_json::from_slice(&bytes)?;
    ensure!(
        !lease.token.is_empty()
            && lease.token.len() <= 64
            && lease.token.bytes().all(|b| b.is_ascii_hexdigit()),
        "Invalid trial token"
    );
    Ok(lease)
}
pub fn trial_pending() -> bool {
    recovery_directory().is_ok_and(|d| d.join("trial.json").exists())
}

fn copy_if_changed(source: &std::path::Path, target: &std::path::Path) -> Result<()> {
    if target.is_file() && fs::read(source)? == fs::read(target)? {
        return Ok(());
    }
    fs::copy(source, target)?;
    OpenOptions::new().write(true).open(target)?.sync_all()?;
    Ok(())
}
fn loaded_module(name: &str) -> Option<PathBuf> {
    use windows::{Win32::System::LibraryLoader::*, core::PCWSTR};
    let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let module = GetModuleHandleW(PCWSTR(name.as_ptr())).ok()?;
        let mut buf = vec![0u16; 32768];
        let n = GetModuleFileNameW(Some(module), &mut buf);
        (n > 0 && (n as usize) < buf.len())
            .then(|| PathBuf::from(String::from_utf16_lossy(&buf[..n as usize])))
    }
}
pub(super) fn start(dir: &std::path::Path, seconds: u64) -> Result<()> {
    let helper_dir = dir.join("recovery");
    fs::create_dir_all(&helper_dir)?;
    let source = std::env::current_exe()?;
    let helper = helper_dir.join("rust-explorer.exe");
    copy_if_changed(&source, &helper)?;
    for name in [
        "libgcc_s_seh-1.dll",
        "libwinpthread-1.dll",
        "Everything64.dll",
    ] {
        let beside = source
            .parent()
            .context("Executable has no parent")?
            .join(name);
        if let Some(path) = beside
            .is_file()
            .then_some(beside)
            .or_else(|| loaded_module(name))
        {
            copy_if_changed(&path, &helper_dir.join(name))?;
        }
    }
    let token = format!(
        "{:x}{:x}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    let lease = Lease {
        token: token.clone(),
        deadline: now() + seconds,
        journal: fs::read(dir.join("backup.json"))?,
    };
    durable_create(dir.join("trial.json"), &serde_json::to_vec(&lease)?)?;
    let child =
        spawn_guard(&helper, &token).context("Cannot start independent recovery process")?;
    let ready = dir.join(format!("ready-{token}"));
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        if ready.exists() {
            fs::remove_file(ready)?;
            return Ok(());
        }
        ensure!(!child.exited()?, "Recovery process exited before readiness");
        std::thread::sleep(Duration::from_millis(50));
    }
    bail!("Recovery process did not become ready; associations unchanged")
}
pub fn confirm_trial() -> Result<()> {
    let (dir, _lock) = lock()?;
    let lease = load(&dir).context("试用已结束或已经恢复，请重新检查关联状态")?;
    ensure!(
        now() < lease.deadline,
        "试用已超时，不能确认保留；请等待自动恢复"
    );
    ensure!(
        fs::read(dir.join("backup.json"))? == lease.journal,
        "Recovery journal changed"
    );
    let backup = read_backup(&dir)?;
    let reg = RegistryStore {
        root: registry_root()?,
    };
    for c in &backup.changes {
        ensure!(
            c.current(&reg)?.as_ref() == Some(&c.installed),
            "Association changed during trial; restore before retrying"
        );
    }
    clear(&dir)
}
pub(super) fn clear(dir: &std::path::Path) -> Result<()> {
    match fs::remove_file(dir.join("trial.json")) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
pub fn guard_main(token: &str) -> Result<()> {
    let dir = recovery_directory()?;
    let lease = load(&dir)?;
    ensure!(lease.token == token, "Recovery guard token mismatch");
    ensure!(
        fs::read(dir.join("backup.json"))? == lease.journal,
        "Recovery journal mismatch"
    );
    read_backup(&dir)?;
    durable_create(dir.join(format!("ready-{token}")), b"ready\n")?;
    let trial_started = Instant::now();
    let trial_limit = Duration::from_secs(lease.deadline.saturating_sub(now()).min(180));
    loop {
        if !dir.join("trial.json").exists() {
            return Ok(());
        }
        if load(&dir).is_ok_and(|current| current.token != token) {
            return Ok(());
        }
        if now() >= lease.deadline || trial_started.elapsed() >= trial_limit {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    // Retry contention with the GUI's transaction lock; never roll back a newer journal.
    let start = Instant::now();
    loop {
        if !dir.join("trial.json").exists() {
            return Ok(());
        }
        if load(&dir).is_ok_and(|current| current.token != token) {
            return Ok(());
        }
        let recovery = (|| -> Result<bool> {
            let (locked_dir, _lock) = lock()?;
            if !locked_dir.join("trial.json").exists() {
                return Ok(false);
            }
            if load(&locked_dir).is_ok_and(|current| current.token != token) {
                return Ok(false);
            }
            ensure!(
                fs::read(locked_dir.join("backup.json"))? == lease.journal,
                "Journal changed; recovery file retained"
            );
            disable_at(&locked_dir)?;
            Ok(true)
        })();
        match recovery {
            Ok(false) => return Ok(()),
            Ok(true) => {
                fs::write(
                    dir.join("last-trial-result.txt"),
                    "Unconfirmed trial automatically restored original associations.\n",
                )?;
                return Ok(());
            }
            Err(e) if start.elapsed() >= Duration::from_secs(15) => {
                fs::write(
                    dir.join("last-trial-result.txt"),
                    format!("Automatic recovery needs attention: {e:#}\nUse restore.reg.\n"),
                )?;
                return Err(e);
            }
            Err(_) => std::thread::sleep(Duration::from_millis(200)),
        }
    }
}
