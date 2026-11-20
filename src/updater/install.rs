//! Replace the portable executable at its existing path after the UI exits.
use super::{Control, Downloaded, LIMIT, copy_checked, version};
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt},
    path::{Path, PathBuf},
    process::{ChildStdin, Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows::{
    Win32::{
        Storage::FileSystem::{
            FILE_FLAG_DELETE_ON_CLOSE, FILE_SHARE_READ, MOVEFILE_REPLACE_EXISTING,
            MOVEFILE_WRITE_THROUGH, MoveFileExW,
        },
        UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW},
    },
    core::{PCWSTR, w},
};

const EXE: &str = "rust-explorer.exe";
const BACKUP: &str = "previous-rust-explorer.exe";
const STAGED: &str = "next-rust-explorer.exe";
const PREFIX: &str = ".rust-explorer-update-";

pub struct Prepared {
    directory: PathBuf,
    filename: OsString,
    executable_hash: [u8; 32],
    handed_off: bool,
}

impl Drop for Prepared {
    fn drop(&mut self) {
        if !self.handed_off {
            let _ = fs::remove_file(self.directory.join(STAGED));
            let _ = fs::remove_file(self.directory.join(BACKUP));
            let _ = fs::remove_dir(&self.directory);
        }
    }
}

fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(Some(0)).collect()
}

pub fn prepare(download: &Downloaded, control: &Control) -> Result<Prepared> {
    ensure!(
        version(&download.version)? > version(env!("CARGO_PKG_VERSION"))?,
        "The downloaded release is no longer newer"
    );
    prepare_at(&std::env::current_exe()?, download, control)
}

fn prepare_at(current: &Path, download: &Downloaded, control: &Control) -> Result<Prepared> {
    control.checkpoint()?;
    let current = current.canonicalize()?;
    let parent = current.parent().context("Missing installation directory")?;
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let directory = parent.join(format!("{PREFIX}{}-{nonce}", std::process::id()));
    fs::create_dir(&directory).context("The installation directory must be writable")?;
    let mut prepared = Prepared {
        directory,
        filename: current
            .file_name()
            .context("Missing application name")?
            .into(),
        handed_off: false,
        executable_hash: [0; 32],
    };
    // Deny writes and replacement while the same file is verified and read.
    let mut archive = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .open(&download.path)?;
    copy_checked(&mut archive, &mut io::sink(), &download.sha256, control)?;
    archive.seek(SeekFrom::Start(0))?;
    let mut archive = zip::ZipArchive::new(archive)?;
    let mut member = archive
        .by_name(EXE)
        .context("Cannot read the release executable")?;
    ensure!(
        member.is_file() && !member.is_symlink(),
        "Invalid executable entry"
    );
    ensure!(
        (64..=LIMIT).contains(&member.size()),
        "Invalid executable size"
    );
    let mut signature = [0; 2];
    member.read_exact(&mut signature)?;
    ensure!(signature == *b"MZ", "Invalid executable header");
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(prepared.directory.join(STAGED))?;
    output.write_all(&signature)?;
    let mut hash = Sha256::new();
    hash.update(signature);
    let limit = member.size() - 2;
    let mut copied = 0;
    let mut buffer = [0; 65536];
    loop {
        control.checkpoint()?;
        let n = member.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        copied += n as u64;
        ensure!(copied <= limit, "Executable exceeds its recorded size");
        output.write_all(&buffer[..n])?;
        hash.update(&buffer[..n]);
    }
    ensure!(copied == limit, "Truncated executable");
    output.sync_all()?;
    prepared.executable_hash = hash.finalize().into();
    fs::copy(&current, prepared.directory.join(BACKUP))?;
    OpenOptions::new()
        .write(true)
        .open(prepared.directory.join(BACKUP))?
        .sync_all()?;
    control.checkpoint()?;
    Ok(prepared)
}

impl Prepared {
    /// The pipe doubles as a lifetime signal. The helper waits for EOF before
    /// touching the executable. Keep the writer alive until the UI process exits.
    pub fn start(mut self) -> Result<ChildStdin> {
        let filename = wide(&self.filename);
        let mut child = Command::new(self.directory.join(BACKUP))
            .arg("--apply-update")
            .stdin(Stdio::piped())
            .spawn()?;
        let mut writer = child.stdin.take().context("Missing installer pipe")?;
        let mut bytes = self.executable_hash.to_vec();
        bytes.extend(filename.into_iter().flat_map(u16::to_le_bytes));
        if let Err(error) = writer.write_all(&bytes) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error.into());
        }
        self.handed_off = true;
        Ok(writer)
    }
}

fn replace(from: &Path, to: &Path) -> Result<()> {
    let from = wide(from.as_os_str());
    let to = wide(to.as_os_str());
    unsafe {
        MoveFileExW(
            PCWSTR(from.as_ptr()),
            PCWSTR(to.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )?;
    }
    Ok(())
}

fn digest(path: &Path) -> Result<[u8; 32]> {
    let mut hash = Sha256::new();
    io::copy(&mut File::open(path)?, &mut hash)?;
    Ok(hash.finalize().into())
}

fn wait_until_writable(target: &Path) -> Result<()> {
    // Background thumbnail/Shell processes can outlive the UI briefly.
    // Never stop other instances on the user's behalf.
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        match OpenOptions::new().write(true).open(target) {
            Ok(_) => return Ok(()),
            Err(error)
                if matches!(error.raw_os_error(), Some(32 | 33 | 1224))
                    && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(150));
            }
            Err(error) => {
                return Err(error).context(
                    "Cannot write the application executable. Close other Rust Explorer instances and check directory permissions",
                );
            }
        }
    }
}

fn apply(
    directory: &Path,
    filename: &OsStr,
    executable_hash: &[u8; 32],
    restart: impl Fn(&Path) -> Result<()>,
) -> Result<()> {
    let parent = directory
        .parent()
        .context("Missing installation directory")?;
    let target = parent.join(filename);
    let backup = directory.join(BACKUP);
    let staged = directory.join(STAGED);
    // Allow only one installer per directory, including different UI processes.
    // Windows deletes the lock when its open handle is closed.
    let _lock = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .custom_flags(FILE_FLAG_DELETE_ON_CLOSE.0)
        .open(parent.join(".rust-explorer-install.lock"))
        .context("An installation is already running or the directory is read-only")?;
    ensure!(
        digest(&target)? == digest(&backup)?,
        "Application changed before installation"
    );
    ensure!(
        &digest(&staged)? == executable_hash,
        "Staged update changed before installation"
    );
    // A writable handle conflicts with another process's mapped executable.
    // The replacement itself is a same-volume rename and may also reject locks.
    wait_until_writable(&target)?;
    replace(&staged, &target).context("Could not replace the application executable")?;
    if let Err(error) = restart(&target) {
        fs::copy(&backup, &staged).context("Cannot stage the saved executable for recovery")?;
        replace(&staged, &target).context("Could not restore the saved executable")?;
        restart(&target).context("Previous version restored, but could not be restarted")?;
        return Err(error.context("Previous version restored and restarted"));
    }
    Ok(())
}

fn restart(target: &Path) -> Result<()> {
    let parent = target.parent().context("Missing installation directory")?;
    let mut child = Command::new(target).current_dir(parent).spawn()?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait()? {
            anyhow::bail!("Application exited during startup: {status}");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

pub fn helper() -> Result<()> {
    use std::os::windows::ffi::OsStringExt;
    let helper = std::env::current_exe()?;
    let directory = helper.parent().context("Missing update directory")?;
    ensure!(
        helper.file_name() == Some(OsStr::new(BACKUP))
            && directory
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(PREFIX)),
        "Invalid update helper location"
    );
    let mut bytes = Vec::new();
    io::stdin().lock().take(1058).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() >= 36 && bytes.len() <= 1056 && bytes.len() % 2 == 0,
        "Invalid update request"
    );
    let executable_hash = bytes[..32].try_into()?;
    let mut filename: Vec<_> = bytes[32..]
        .chunks_exact(2)
        .map(|unit| u16::from_le_bytes([unit[0], unit[1]]))
        .collect();
    ensure!(
        filename.pop() == Some(0) && !filename.contains(&0),
        "Invalid executable name"
    );
    let filename = OsString::from_wide(&filename);
    ensure!(
        Path::new(&filename).file_name() == Some(filename.as_os_str())
            && Path::new(&filename)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("exe")),
        "Invalid executable name"
    );
    if let Err(error) = apply(directory, &filename, executable_hash, restart) {
        eprintln!("{error:#}");
        let message = format!(
            "{}\n\n{error:#}\n\n{}\n{}",
            crate::i18n::text("安装更新失败"),
            crate::i18n::text("旧版程序备份"),
            directory.join(BACKUP).display()
        );
        let message = wide(OsStr::new(&message));
        unsafe {
            MessageBoxW(
                None,
                PCWSTR(message.as_ptr()),
                w!("Rust Explorer"),
                MB_OK | MB_ICONERROR,
            )
        };
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_rechecks_zip_and_restores_original_after_failed_restart() -> Result<()> {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let root =
            std::env::temp_dir().join(format!("rex-update-test-{}-{nonce}", std::process::id()));
        fs::create_dir(&root)?;
        let current = root.join("Portable Rust Explorer.exe");
        fs::write(&current, b"original")?;
        let package = root.join("release.zip");
        let mut zip = zip::ZipWriter::new(File::create(&package)?);
        zip.start_file(EXE, zip::write::SimpleFileOptions::default())?;
        let mut fixture = [0u8; 64];
        fixture[..2].copy_from_slice(b"MZ");
        zip.write_all(&fixture)?;
        zip.start_file("../unrelated.txt", zip::write::SimpleFileOptions::default())?;
        zip.write_all(b"not part of the install")?;
        drop(zip.finish()?);
        let mut download = Downloaded {
            path: package,
            sha256: "0".repeat(64),
            version: "99.0.0".into(),
        };
        assert!(prepare_at(&current, &download, &Control::default()).is_err());
        assert_eq!(
            fs::read_dir(&root)?.count(),
            2,
            "failed staging leaves no directory"
        );
        download.sha256 = format!("{:x}", Sha256::digest(fs::read(&download.path)?));
        let prepared = prepare_at(&current, &download, &Control::default())?;
        assert_eq!(fs::read(prepared.directory.join(STAGED))?, fixture);
        let launches = std::cell::Cell::new(0);
        assert!(
            apply(&prepared.directory, &prepared.filename, &[0; 32], |_| {
                panic!("a changed executable must not start")
            })
            .is_err()
        );
        let result = apply(
            &prepared.directory,
            &prepared.filename,
            &prepared.executable_hash,
            |path| {
                launches.set(launches.get() + 1);
                if launches.get() == 1 {
                    assert_eq!(fs::read(path)?, fixture);
                    anyhow::bail!("simulated startup failure");
                }
                assert_eq!(fs::read(path)?, b"original");
                Ok(())
            },
        );
        assert!(format!("{:#}", result.unwrap_err()).contains("Previous version restored"));
        assert_eq!(launches.get(), 2);
        assert_eq!(fs::read(&current)?, b"original");
        assert!(!root.join("unrelated.txt").exists());
        assert!(!root.join(".rust-explorer-install.lock").exists());
        drop(prepared);
        fs::remove_file(current)?;
        fs::remove_file(download.path)?;
        fs::remove_dir(root)?;
        Ok(())
    }
}
