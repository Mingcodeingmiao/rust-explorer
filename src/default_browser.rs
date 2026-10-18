//! Opt-in, current-user folder associations. Never replace Explorer's open verb.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::PathBuf,
};
use windows_registry::{CURRENT_USER, Type};

const VERB: &str = "RustExplorer.Open";
const ROOT: &str = "Software\\Classes";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Raw {
    ty: u32,
    bytes: Vec<u8>,
}
impl Raw {
    fn string(s: &str) -> Self {
        Self {
            ty: 1,
            bytes: s
                .encode_utf16()
                .chain(Some(0))
                .flat_map(u16::to_le_bytes)
                .collect(),
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct Change {
    path: String,
    before: Option<Raw>,
    installed: Raw,
}
#[derive(Serialize, Deserialize)]
struct Backup {
    version: u32,
    changes: Vec<Change>,
}

trait Registry {
    fn read(&self, path: &str) -> Result<Option<Raw>>;
    fn write(&self, path: &str, value: Option<&Raw>) -> Result<()>;
}
struct RegistryStore {
    root: String,
}
fn optional<T>(r: windows_registry::Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.code().0 == 0x80070002u32 as i32 || e.code().0 == 0x80070003u32 as i32 => {
            Ok(None)
        }
        Err(e) => Err(e.into()),
    }
}
impl Registry for RegistryStore {
    fn read(&self, path: &str) -> Result<Option<Raw>> {
        let Some(key) = optional(CURRENT_USER.open(format!("{}\\{path}", self.root)))? else {
            return Ok(None);
        };
        Ok(optional(key.get_value(""))?.map(|v| Raw {
            ty: v.ty().into(),
            bytes: v.to_vec(),
        }))
    }
    fn write(&self, path: &str, value: Option<&Raw>) -> Result<()> {
        let path = format!("{}\\{path}", self.root);
        if let Some(value) = value {
            CURRENT_USER
                .create(path)?
                .set_bytes("", Type::from(value.ty), &value.bytes)?;
        } else if let Some(key) = optional(CURRENT_USER.options().read().write().open(path))? {
            optional(key.remove_value(""))?;
        }
        // Keep empty keys: recursive deletion could destroy another application's values.
        Ok(())
    }
}
fn plan(exe: &std::path::Path) -> Result<Vec<(String, Raw)>> {
    ensure!(exe.is_file(), "Application executable is missing");
    ensure!(
        exe.file_name().is_some_and(|n| n
            .to_string_lossy()
            .eq_ignore_ascii_case("rust-explorer.exe")),
        "Unexpected executable name"
    );
    let text = exe
        .to_str()
        .context("Executable path is not valid Unicode")?;
    ensure!(
        !text.contains(['"', '\r', '\n', '\0']),
        "Invalid executable path"
    );
    // A trailing dot avoids the Windows argv trailing-backslash/quote ambiguity for C:\.
    let command = Raw::string(&format!("\"{text}\" --open-folder \"%1\\.\""));
    let mut values = vec![];
    for class in ["Directory", "Drive"] {
        let shell = format!("{class}\\shell");
        // Install command before selecting the verb; restore reverses this order.
        values.push((format!("{shell}\\{VERB}\\command"), command.clone()));
        values.push((
            format!("{shell}\\{VERB}"),
            Raw::string("Open in Rust Explorer"),
        ));
        values.push((shell, Raw::string(VERB)));
    }
    Ok(values)
}
fn validate(backup: &Backup) -> Result<()> {
    ensure!(
        backup.version == 1 && backup.changes.len() == 6,
        "Invalid association backup; no registry values were changed"
    );
    let mut expected = vec![];
    for class in ["Directory", "Drive"] {
        expected.extend([
            format!("{class}\\shell\\{VERB}\\command"),
            format!("{class}\\shell\\{VERB}"),
            format!("{class}\\shell"),
        ]);
    }
    ensure!(
        backup
            .changes
            .iter()
            .zip(expected)
            .all(|(c, p)| c.path == p),
        "Association backup paths are invalid"
    );
    Ok(())
}
fn restore(reg: &impl Registry, backup: &Backup) -> Result<()> {
    validate(backup)?;
    let mut errors = vec![];
    for c in backup.changes.iter().rev() {
        let result = (|| -> Result<()> {
            let current = reg.read(&c.path)?;
            if current == c.before {
                return Ok(());
            }
            ensure!(
                current.as_ref() == Some(&c.installed),
                "Value changed by another program; preserved: {}",
                c.path
            );
            reg.write(&c.path, c.before.as_ref())?;
            ensure!(
                reg.read(&c.path)? == c.before,
                "Restore verification failed: {}",
                c.path
            );
            Ok(())
        })();
        if let Err(e) = result {
            errors.push(e.to_string());
        }
    }
    ensure!(errors.is_empty(), "{}", errors.join("; "));
    Ok(())
}
fn install(reg: &impl Registry, backup: &Backup) -> Result<()> {
    for c in &backup.changes {
        ensure!(
            reg.read(&c.path)? == c.before,
            "Association changed while preparing; aborting"
        );
        reg.write(&c.path, Some(&c.installed))?;
        ensure!(
            reg.read(&c.path)?.as_ref() == Some(&c.installed),
            "Association verification failed"
        );
    }
    Ok(())
}
pub fn recovery_directory() -> Result<PathBuf> {
    Ok(
        PathBuf::from(std::env::var_os("LOCALAPPDATA").context("LOCALAPPDATA unavailable")?)
            .join("RustExplorer")
            .join("folder-association"),
    )
}
fn lock() -> Result<(PathBuf, File)> {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = recovery_directory()?;
    fs::create_dir_all(&dir)?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .share_mode(0)
        .open(dir.join("operation.lock"))
        .context("Another association operation is running")?;
    Ok((dir, file))
}
fn read_backup(dir: &std::path::Path) -> Result<Backup> {
    let b: Backup = serde_json::from_slice(
        &fs::read(dir.join("backup.json"))
            .context("Backup missing: refusing to guess original associations")?,
    )
    .context("Backup damaged: use restore.reg or recover the backup")?;
    validate(&b)?;
    Ok(b)
}
fn recovery_reg(backup: &Backup, root: &str) -> String {
    let mut text = String::from(
        "Windows Registry Editor Version 5.00\r\n; Rust Explorer original per-user folder associations\r\n",
    );
    for c in backup.changes.iter().rev() {
        text.push_str(&format!("\r\n[HKEY_CURRENT_USER\\{root}\\{}]\r\n", c.path));
        match &c.before {
            None => text.push_str("@=-\r\n"),
            Some(v) => {
                text.push_str(&format!("@=hex({:x}):", v.ty));
                for (i, byte) in v.bytes.iter().enumerate() {
                    if i != 0 {
                        text.push(',');
                    }
                    if i != 0 && i % 24 == 0 {
                        text.push_str("\\\r\n  ");
                    }
                    text.push_str(&format!("{byte:02x}"));
                }
                text.push_str("\r\n");
            }
        }
    }
    text
}
fn durable_create(path: PathBuf, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
fn notify_shell() {
    use windows::Win32::UI::Shell::*;
    unsafe {
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
    }
}
pub fn status() -> Result<String> {
    let dir = recovery_directory()?;
    let reg = RegistryStore { root: ROOT.into() };
    if !dir.join("backup.json").exists() {
        for class in ["Directory", "Drive"] {
            ensure!(
                reg.read(&format!("{class}\\shell"))? != Some(Raw::string(VERB)),
                "检测到 RE 关联但备份缺失，请使用原 restore.reg 恢复；不会猜测或覆盖原值"
            );
        }
        return Ok("未启用（系统关联未由本功能接管）".into());
    }
    let backup = read_backup(&dir)?;
    for c in &backup.changes {
        if reg.read(&c.path)?.as_ref() != Some(&c.installed) {
            return Ok("关联已变化或操作未完成，请先恢复原样".into());
        }
    }
    Ok("已启用普通文件夹和驱动器关联".into())
}
pub fn enable() -> Result<()> {
    let (dir, _lock) = lock()?;
    let reg = RegistryStore { root: ROOT.into() };
    // Older contributor builds overwrote open\command directly. Do not layer a
    // new journal over that state and later call it a restoration of Explorer.
    if !dir.join("backup.json").exists() {
        for class in ["Directory", "Drive", "Folder"] {
            if let Some(value) = reg.read(&format!("{class}\\shell\\open\\command"))? {
                let text = String::from_utf16_lossy(
                    &value
                        .bytes
                        .chunks_exact(2)
                        .map(|c| u16::from_le_bytes([c[0], c[1]]))
                        .collect::<Vec<_>>(),
                );
                ensure!(
                    !text.to_lowercase().contains("rust-explorer"),
                    "Legacy Rust Explorer open-command association detected. Restore it using its original backup before enabling this version"
                );
            }
        }
    }
    for class in ["Directory", "Drive"] {
        let verb = format!("{ROOT}\\{class}\\shell\\{VERB}");
        ensure!(
            dir.join("backup.json").exists() || optional(CURRENT_USER.open(verb))?.is_none(),
            "Existing Rust Explorer verb has no recovery journal; refusing to overwrite"
        );
    }
    let result = enable_at(&reg, &dir, &std::env::current_exe()?.canonicalize()?);
    notify_shell();
    result
}
fn enable_at(reg: &impl Registry, dir: &std::path::Path, exe: &std::path::Path) -> Result<()> {
    let values = plan(exe)?;
    if dir.join("backup.json").exists() {
        let b = read_backup(dir)?;
        if b.changes.iter().zip(&values).all(|(c, (_, desired))| {
            c.installed == *desired
                && reg
                    .read(&c.path)
                    .is_ok_and(|v| v.as_ref() == Some(&c.installed))
        }) {
            return Ok(());
        }
        bail!("Existing backup found; restore original associations first");
    }
    let mut changes = vec![];
    for (path, installed) in values {
        let before = reg.read(&path)?;
        if path.contains(VERB) {
            ensure!(
                before.is_none(),
                "Rust Explorer verb already exists without a backup; refusing to overwrite"
            );
        }
        changes.push(Change {
            path,
            before,
            installed,
        });
    }
    let backup = Backup {
        version: 1,
        changes,
    };
    durable_create(
        dir.join("backup.json"),
        &serde_json::to_vec_pretty(&backup)?,
    )?;
    let recovery: Vec<u8> = std::iter::once(0xfeffu16)
        .chain(recovery_reg(&backup, ROOT).encode_utf16())
        .flat_map(u16::to_le_bytes)
        .collect();
    // No registry writes until BOTH recovery files are safely on disk.
    if let Err(e) = durable_create(dir.join("restore.reg"), &recovery) {
        return Err(e.context("Recovery file could not be saved; associations unchanged"));
    }
    if let Err(e) = install(reg, &backup) {
        let rollback = restore(reg, &backup);
        return match rollback {
            Ok(()) => {
                Err(e.context("Enable failed; original values restored. Recovery files retained"))
            }
            Err(r) => Err(anyhow::anyhow!(
                "Enable failed: {e}; rollback incomplete: {r}. Recovery: {}",
                dir.display()
            )),
        };
    }
    Ok(())
}
/// Validate and normalize a Shell handler argument before opening a window.
pub fn folder_argument(path: &std::path::Path) -> Result<PathBuf> {
    ensure!(
        path.is_absolute() && path.is_dir(),
        "Folder handler requires an existing absolute directory"
    );
    let path = path.canonicalize()?;
    let text = path.to_string_lossy();
    if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        return Ok(PathBuf::from(format!(r"\\{unc}")));
    }
    Ok(text
        .strip_prefix(r"\\?\")
        .map(PathBuf::from)
        .unwrap_or(path))
}
pub fn disable() -> Result<()> {
    let (dir, _lock) = lock()?;
    let backup = read_backup(&dir)?;
    let restored = restore(&RegistryStore { root: ROOT.into() }, &backup);
    notify_shell();
    restored?;
    // Retain an archived copy for inspection, freeing the active journal only on success.
    fs::copy(dir.join("backup.json"), dir.join("last-restored.json"))?;
    if dir.join("restore.reg").exists() {
        fs::remove_file(dir.join("restore.reg"))?;
    }
    fs::remove_file(dir.join("backup.json"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::{Cell, RefCell},
        collections::BTreeMap,
    };
    #[derive(Default)]
    struct Memory {
        values: RefCell<BTreeMap<String, Raw>>,
        writes: Cell<usize>,
        fail: Cell<usize>,
    }
    impl Registry for Memory {
        fn read(&self, p: &str) -> Result<Option<Raw>> {
            Ok(self.values.borrow().get(p).cloned())
        }
        fn write(&self, p: &str, v: Option<&Raw>) -> Result<()> {
            let n = self.writes.get() + 1;
            self.writes.set(n);
            ensure!(n != self.fail.get(), "Injected write failure");
            if let Some(v) = v {
                self.values.borrow_mut().insert(p.into(), v.clone());
            } else {
                self.values.borrow_mut().remove(p);
            }
            Ok(())
        }
    }
    fn backup() -> Backup {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let exe = std::env::temp_dir()
            .join(format!(
                "re-association-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ))
            .join("rust-explorer.exe");
        fs::create_dir_all(exe.parent().unwrap()).unwrap();
        fs::write(&exe, b"fixture").unwrap();
        let changes = plan(&exe)
            .unwrap()
            .into_iter()
            .enumerate()
            .map(|(i, (path, installed))| Change {
                path,
                installed,
                before: (i % 3 == 2).then(|| Raw {
                    ty: 2,
                    bytes: vec![65, 0, 0, 0],
                }),
            })
            .collect();
        fs::remove_file(&exe).unwrap();
        fs::remove_dir(exe.parent().unwrap()).unwrap();
        Backup {
            version: 1,
            changes,
        }
    }
    #[test]
    fn every_failed_write_rolls_back_exactly() {
        let b = backup();
        for fail in 1..=6 {
            let reg = Memory::default();
            for c in &b.changes {
                reg.write(&c.path, c.before.as_ref()).unwrap();
            }
            let original = reg.values.borrow().clone();
            reg.writes.set(0);
            reg.fail.set(fail);
            assert!(install(&reg, &b).is_err());
            restore(&reg, &b).unwrap();
            assert_eq!(*reg.values.borrow(), original);
        }
    }
    #[test]
    fn restore_preserves_foreign_changes_and_rejects_bad_backup() {
        let mut b = backup();
        let reg = Memory::default();
        for c in &b.changes {
            reg.write(&c.path, c.before.as_ref()).unwrap();
        }
        install(&reg, &b).unwrap();
        let foreign = Raw::string("OtherBrowser");
        reg.write(&b.changes[2].path, Some(&foreign)).unwrap();
        assert!(restore(&reg, &b).is_err());
        assert_eq!(reg.read(&b.changes[2].path).unwrap(), Some(foreign));
        b.changes[0].path = "Unrelated".into();
        assert!(restore(&reg, &b).is_err());
    }
    #[test]
    fn isolated_windows_registry_round_trip() {
        let b = backup();
        let root = format!(
            "Software\\RustExplorer\\AssociationTests\\{}",
            std::process::id()
        );
        let reg = RegistryStore { root: root.clone() };
        for c in &b.changes {
            reg.write(&c.path, c.before.as_ref()).unwrap();
        }
        install(&reg, &b).unwrap();
        restore(&reg, &b).unwrap();
        for c in &b.changes {
            assert_eq!(reg.read(&c.path).unwrap(), c.before);
        }
        // This root is an isolated fixture, never Software\Classes.
        CURRENT_USER.remove_tree(root).unwrap();
    }
    #[test]
    fn journal_is_not_overwritten_and_corruption_never_changes_registry() {
        let dir = std::env::temp_dir().join(format!("re-journal-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let exe = dir.join("rust-explorer.exe");
        fs::write(&exe, b"fixture").unwrap();
        let reg = Memory::default();
        enable_at(&reg, &dir, &exe).unwrap();
        let saved = fs::read(dir.join("backup.json")).unwrap();
        let count = reg.writes.get();
        enable_at(&reg, &dir, &exe).unwrap();
        assert_eq!(reg.writes.get(), count);
        assert_eq!(fs::read(dir.join("backup.json")).unwrap(), saved);
        let recovery = fs::read(dir.join("restore.reg")).unwrap();
        assert_eq!(&recovery[..2], &[255, 254]);
        fs::write(dir.join("backup.json"), b"broken").unwrap();
        assert!(enable_at(&reg, &dir, &exe).is_err());
        assert_eq!(reg.writes.get(), count);
        for file in ["rust-explorer.exe", "backup.json", "restore.reg"] {
            fs::remove_file(dir.join(file)).unwrap();
        }
        fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn recovery_file_failure_prevents_all_registry_writes() {
        let dir = std::env::temp_dir().join(format!("re-recovery-failure-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let exe = dir.join("rust-explorer.exe");
        fs::write(&exe, b"fixture").unwrap();
        fs::write(
            dir.join("restore.reg"),
            b"existing recovery must not be overwritten",
        )
        .unwrap();
        let reg = Memory::default();
        assert!(enable_at(&reg, &dir, &exe).is_err());
        assert_eq!(reg.writes.get(), 0);
        assert!(read_backup(&dir).is_ok());
        for file in ["rust-explorer.exe", "backup.json", "restore.reg"] {
            fs::remove_file(dir.join(file)).unwrap();
        }
        fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn shell_command_preserves_roots_spaces_and_unicode() {
        use windows::{
            Win32::{
                Foundation::{HLOCAL, LocalFree},
                UI::Shell::CommandLineToArgvW,
            },
            core::PCWSTR,
        };
        let b = backup();
        let command = String::from_utf16(
            &b.changes[0]
                .installed
                .bytes
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .take_while(|n| *n != 0)
                .collect::<Vec<_>>(),
        )
        .unwrap();
        for folder in [
            r"C:\",
            r"C:\folder with spaces",
            "C:\\中文目录",
            r"\\server\share\",
        ] {
            let line: Vec<u16> = command
                .replace("%1", folder)
                .encode_utf16()
                .chain(Some(0))
                .collect();
            unsafe {
                let mut count = 0;
                let args = CommandLineToArgvW(PCWSTR(line.as_ptr()), &mut count);
                assert!(!args.is_null());
                assert_eq!(count, 3);
                assert_eq!((*args.add(1)).to_string().unwrap(), "--open-folder");
                assert_eq!((*args.add(2)).to_string().unwrap(), format!("{folder}\\."));
                let _ = LocalFree(Some(HLOCAL(args.cast())));
            }
        }
        let root = PathBuf::from(r"C:\\.");
        assert_eq!(folder_argument(&root).unwrap(), PathBuf::from(r"C:\"));
        assert!(folder_argument(std::path::Path::new("relative")).is_err());
    }
    #[test]
    fn standalone_reg_file_restores_isolated_hive_without_application() {
        use std::os::windows::process::CommandExt;
        let root = format!(
            "Software\\RustExplorer\\AssociationTests\\reg-file-{}",
            std::process::id()
        );
        let reg = RegistryStore { root: root.clone() };
        let b = backup();
        for c in &b.changes {
            reg.write(&c.path, c.before.as_ref()).unwrap();
        }
        install(&reg, &b).unwrap();
        let path = std::env::temp_dir().join(format!("re-restore-test-{}.reg", std::process::id()));
        let bytes: Vec<u8> = std::iter::once(0xfeffu16)
            .chain(recovery_reg(&b, &root).encode_utf16())
            .flat_map(u16::to_le_bytes)
            .collect();
        fs::write(&path, bytes).unwrap();
        let output = std::process::Command::new("reg.exe")
            .args(["import"])
            .arg(&path)
            .creation_flags(0x08000000)
            .output()
            .unwrap();
        assert!(output.status.success(), "reg import failed: {:?}", output);
        for c in &b.changes {
            assert_eq!(reg.read(&c.path).unwrap(), c.before);
        }
        CURRENT_USER.remove_tree(root).unwrap();
        fs::remove_file(path).unwrap();
    }
}
