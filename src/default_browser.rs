//! Opt-in, journaled per-user folder integration with independent recovery.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::PathBuf,
};
use windows_registry::{CURRENT_USER, Type};
#[path = "default_browser_guard.rs"]
mod guard;
pub use guard::{confirm_trial, guard_main, trial_pending};

const VERB: &str = "RustExplorer.Open";
const FALLBACK: &str = "RustExplorer.SystemExplorer";
const ROOT: &str = "Software\\Classes";
fn registry_root() -> Result<String> {
    // Debug-only process tests fail closed; release builds have no override.
    #[cfg(debug_assertions)]
    if let Some(root) = std::env::var_os("RUST_EXPLORER_ASSOCIATION_TEST_ROOT") {
        let root = root.to_str().context("Invalid registry test root")?;
        let suffix = root
            .strip_prefix("Software\\RustExplorer\\AssociationTests\\")
            .context("Unsafe registry test root")?;
        ensure!(
            !suffix.is_empty()
                && suffix
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-'),
            "Invalid registry test suffix"
        );
        return Ok(root.into());
    }
    Ok(ROOT.into())
}

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
    #[serde(default)]
    name: String,
    before: Option<Raw>,
    installed: Raw,
}
impl Change {
    fn current(&self, reg: &impl Registry) -> Result<Option<Raw>> {
        reg.read_named(&self.path, &self.name)
    }
    fn write(&self, reg: &impl Registry, value: Option<&Raw>) -> Result<()> {
        reg.write_named(&self.path, &self.name, value)
    }
}
struct Target {
    path: String,
    name: String,
    installed: Raw,
}
fn target(path: String, name: &str, installed: Raw) -> Target {
    Target {
        path,
        name: name.into(),
        installed,
    }
}
#[derive(Serialize, Deserialize)]
struct Backup {
    version: u32,
    changes: Vec<Change>,
    #[serde(default)]
    created_keys: Vec<String>,
}

trait Registry {
    fn read(&self, path: &str) -> Result<Option<Raw>>;
    fn write(&self, path: &str, value: Option<&Raw>) -> Result<()>;
    fn read_named(&self, path: &str, name: &str) -> Result<Option<Raw>> {
        ensure!(name.is_empty(), "Named registry values unsupported");
        self.read(path)
    }
    fn write_named(&self, path: &str, name: &str, value: Option<&Raw>) -> Result<()> {
        ensure!(name.is_empty(), "Named registry values unsupported");
        self.write(path, value)
    }
    fn key_exists(&self, _path: &str) -> Result<bool> {
        Ok(true)
    }
    fn remove_empty_key(&self, _path: &str) -> Result<()> {
        Ok(())
    }
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
    fn key_exists(&self, path: &str) -> Result<bool> {
        Ok(optional(CURRENT_USER.open(format!("{}\\{path}", self.root)))?.is_some())
    }
    fn remove_empty_key(&self, path: &str) -> Result<()> {
        let full = format!("{}\\{path}", self.root);
        let Some(key) = optional(CURRENT_USER.open(&full))? else {
            return Ok(());
        };
        if key.values()?.next().is_some() || key.keys()?.next().is_some() {
            return Ok(());
        }
        drop(key);
        use windows::{
            Win32::System::Registry::{HKEY, RegDeleteKeyW},
            core::PCWSTR,
        };
        let wide: Vec<u16> = full.encode_utf16().chain(Some(0)).collect();
        let result = unsafe { RegDeleteKeyW(HKEY(CURRENT_USER.as_raw()), PCWSTR(wide.as_ptr())) };
        if result.0 != 2 {
            result.ok()?;
        }
        Ok(())
    }
    fn read(&self, path: &str) -> Result<Option<Raw>> {
        self.read_named(path, "")
    }
    fn read_named(&self, path: &str, name: &str) -> Result<Option<Raw>> {
        let Some(key) = optional(CURRENT_USER.open(format!("{}\\{path}", self.root)))? else {
            return Ok(None);
        };
        Ok(optional(key.get_value(name))?.map(|v| Raw {
            ty: v.ty().into(),
            bytes: v.to_vec(),
        }))
    }
    fn write(&self, path: &str, value: Option<&Raw>) -> Result<()> {
        self.write_named(path, "", value)
    }
    fn write_named(&self, path: &str, name: &str, value: Option<&Raw>) -> Result<()> {
        let path = format!("{}\\{path}", self.root);
        if let Some(value) = value {
            CURRENT_USER
                .create(path)?
                .set_bytes(name, Type::from(value.ty), &value.bytes)?;
        } else if let Some(key) = optional(CURRENT_USER.options().read().write().open(path))? {
            optional(key.remove_value(name))?;
        }
        // Keep empty keys: recursive deletion could destroy another application's values.
        Ok(())
    }
}
fn plan(exe: &std::path::Path) -> Result<Vec<Target>> {
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
    // Shell execution rejects verbatim \\?\ executable paths even though
    // CreateProcess accepts them. Register a normal DOS/UNC command path.
    let text = if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else {
        text.strip_prefix(r"\\?\").unwrap_or(text).to_owned()
    };
    ensure!(
        !text.contains(['"', '\r', '\n', '\0']),
        "Invalid executable path"
    );
    // A trailing dot avoids the Windows argv trailing-backslash/quote ambiguity for C:\.
    let command = Raw::string(&format!("\"{text}\" --open-folder \"%1\\.\""));
    let explorer = PathBuf::from(std::env::var_os("SystemRoot").context("SystemRoot unavailable")?)
        .join("explorer.exe");
    ensure!(
        explorer.is_file(),
        "Windows Explorer fallback is unavailable"
    );
    let fallback_command = Raw::string(&format!("\"{}\" \"%1\\.\"", explorer.display()));
    let mut values = vec![];
    for class in ["Directory", "Drive"] {
        let shell = format!("{class}\\shell");
        // Install command before selecting the verb; restore reverses this order.
        values.extend([
            target(format!("{shell}\\{VERB}\\command"), "", command.clone()),
            target(
                format!("{shell}\\{VERB}"),
                "",
                Raw::string("在 Rust Explorer 中打开"),
            ),
            target(
                format!("{shell}\\{VERB}"),
                "Icon",
                Raw::string(&format!("\"{text}\",0")),
            ),
            target(
                format!("{shell}\\{VERB}"),
                "MultiSelectModel",
                Raw::string("Document"),
            ),
            target(
                format!("{shell}\\{FALLBACK}\\command"),
                "",
                fallback_command.clone(),
            ),
            target(
                format!("{shell}\\{FALLBACK}"),
                "",
                Raw::string("在 Windows 资源管理器中打开"),
            ),
            target(
                format!("{shell}\\{FALLBACK}"),
                "Icon",
                Raw::string(&format!("\"{}\",0", explorer.display())),
            ),
            target(format!("{shell}\\open\\command"), "", command.clone()),
            target(
                format!("{shell}\\open\\command"),
                "DelegateExecute",
                Raw::string(""),
            ),
            target(shell, "", Raw::string(VERB)),
        ]);
    }
    Ok(values)
}
fn validate(backup: &Backup) -> Result<()> {
    ensure!(
        (backup.version == 1 && backup.changes.len() == 6)
            || (backup.version == 2 && backup.changes.len() == 20),
        "Invalid association backup; no registry values were changed"
    );
    let mut expected = vec![];
    for class in ["Directory", "Drive"] {
        let shell = format!("{class}\\shell");
        if backup.version == 1 {
            expected.extend([
                (format!("{shell}\\{VERB}\\command"), ""),
                (format!("{shell}\\{VERB}"), ""),
                (shell, ""),
            ]);
        } else {
            expected.extend([
                (format!("{shell}\\{VERB}\\command"), ""),
                (format!("{shell}\\{VERB}"), ""),
                (format!("{shell}\\{VERB}"), "Icon"),
                (format!("{shell}\\{VERB}"), "MultiSelectModel"),
                (format!("{shell}\\{FALLBACK}\\command"), ""),
                (format!("{shell}\\{FALLBACK}"), ""),
                (format!("{shell}\\{FALLBACK}"), "Icon"),
                (format!("{shell}\\open\\command"), ""),
                (format!("{shell}\\open\\command"), "DelegateExecute"),
                (shell, ""),
            ]);
        }
    }
    ensure!(
        backup
            .changes
            .iter()
            .zip(&expected)
            .all(|(c, (p, n))| &c.path == p && c.name == *n),
        "Association backup paths are invalid"
    );
    let allowed_keys = key_paths(expected.iter().map(|(p, _)| p.as_str()));
    ensure!(
        backup.created_keys.iter().all(|p| allowed_keys.contains(p)),
        "Invalid created-key recovery paths"
    );
    Ok(())
}
fn key_paths<'a>(paths: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut keys = std::collections::BTreeSet::new();
    for path in paths {
        let mut current = String::new();
        for part in path.split('\\') {
            if !current.is_empty() {
                current.push('\\');
            }
            current.push_str(part);
            keys.insert(current.clone());
        }
    }
    let mut keys: Vec<_> = keys.into_iter().collect();
    keys.sort_by_key(|p| p.matches('\\').count());
    keys
}
fn restore(reg: &impl Registry, backup: &Backup) -> Result<()> {
    validate(backup)?;
    let mut errors = vec![];
    for c in backup.changes.iter().rev() {
        let result = (|| -> Result<()> {
            let current = c.current(reg)?;
            if current == c.before {
                return Ok(());
            }
            ensure!(
                current.as_ref() == Some(&c.installed),
                "Value changed by another program; preserved: {}",
                c.path
            );
            c.write(reg, c.before.as_ref())?;
            ensure!(
                c.current(reg)? == c.before,
                "Restore verification failed: {}",
                c.path
            );
            Ok(())
        })();
        if let Err(e) = result {
            errors.push(e.to_string());
        }
    }
    for path in backup.created_keys.iter().rev() {
        if let Err(e) = reg.remove_empty_key(path) {
            errors.push(format!("Empty-key recovery: {path}: {e}"));
        }
    }
    ensure!(errors.is_empty(), "{}", errors.join("; "));
    Ok(())
}
fn install(reg: &impl Registry, backup: &Backup) -> Result<()> {
    for c in &backup.changes {
        ensure!(
            c.current(reg)? == c.before,
            "Association changed while preparing; aborting"
        );
        c.write(reg, Some(&c.installed))?;
        ensure!(
            c.current(reg)?.as_ref() == Some(&c.installed),
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
        let value_name = if c.name.is_empty() {
            "@".to_owned()
        } else {
            format!("\"{}\"", c.name)
        };
        match &c.before {
            None => text.push_str(&format!("{value_name}=-\r\n")),
            Some(v) => {
                text.push_str(&format!("{value_name}=hex({:x}):", v.ty));
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
    // Emergency import is a snapshot restore. The normal helper instead checks
    // for foreign values and removes only still-empty, originally absent keys.
    text.push_str("\r\n; Remove only verb keys that did not exist before this trial.\r\n; Prefer the recovery helper if another program changed these keys later.\r\n");
    for path in backup.created_keys.iter().rev() {
        if path.contains("\\shell\\") {
            text.push_str(&format!("[-HKEY_CURRENT_USER\\{root}\\{path}]\r\n"));
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
    if registry_root().is_ok_and(|root| root != ROOT) {
        return;
    }
    use windows::Win32::UI::Shell::*;
    unsafe {
        SHChangeNotify(
            SHCNE_ASSOCCHANGED,
            SHCNF_IDLIST | SHCNF_FLUSHNOWAIT,
            None,
            None,
        );
    }
}
pub fn status() -> Result<String> {
    let dir = recovery_directory()?;
    let reg = RegistryStore {
        root: registry_root()?,
    };
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
        if c.current(&reg)?.as_ref() != Some(&c.installed) {
            return Ok("关联已变化或操作未完成，请先恢复原样".into());
        }
    }
    if trial_pending() {
        return Ok("临时试用中，未确认保留将自动恢复".into());
    }
    Ok("已启用普通文件夹和驱动器关联".into())
}
pub fn enable() -> Result<()> {
    begin_trial(120)
}
/// Begin a time-limited association trial guarded by an independent process.
pub fn begin_trial(seconds: u64) -> Result<()> {
    ensure!(
        (5..=180).contains(&seconds),
        "Trial duration must be 5..180 seconds"
    );
    let (dir, _lock) = lock()?;
    ensure!(
        !dir.join("trial.json").exists(),
        "已有待确认试用，请先确认保留或恢复原样"
    );
    let reg = RegistryStore {
        root: registry_root()?,
    };
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
        for owned_verb in [VERB, FALLBACK] {
            let verb = format!("{}\\{class}\\shell\\{owned_verb}", reg.root);
            ensure!(
                dir.join("backup.json").exists() || reusable_verb(&verb)?,
                "Existing Rust Explorer verb has no recovery journal; refusing to overwrite"
            );
        }
    }
    let result = enable_prepared(
        &reg,
        &dir,
        &std::env::current_exe()?.canonicalize()?,
        || guard::start(&dir, seconds),
    );
    notify_shell();
    result
}
fn reusable_verb(path: &str) -> Result<bool> {
    let Some(key) = optional(CURRENT_USER.open(path))? else {
        return Ok(true);
    };
    if key.values()?.next().is_some() {
        return Ok(false);
    }
    for child in key.keys()? {
        if !child.eq_ignore_ascii_case("command") {
            return Ok(false);
        }
        let command = key.open(child)?;
        if command.values()?.next().is_some() || command.keys()?.next().is_some() {
            return Ok(false);
        }
    }
    Ok(true)
}
#[cfg(test)]
fn enable_at(reg: &impl Registry, dir: &std::path::Path, exe: &std::path::Path) -> Result<()> {
    enable_prepared(reg, dir, exe, || Ok(()))
}
fn enable_prepared(
    reg: &impl Registry,
    dir: &std::path::Path,
    exe: &std::path::Path,
    ready: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let values = plan(exe)?;
    if dir.join("backup.json").exists() {
        let b = read_backup(dir)?;
        if b.changes.len() == values.len()
            && b.changes.iter().zip(&values).all(|(c, desired)| {
                c.installed == desired.installed
                    && c.name == desired.name
                    && c.path == desired.path
                    && reg
                        .read_named(&c.path, &c.name)
                        .is_ok_and(|v| v.as_ref() == Some(&c.installed))
            })
        {
            return Ok(());
        }
        bail!("Existing backup found; restore original associations first");
    }
    let mut changes = vec![];
    let mut created_keys = vec![];
    for path in key_paths(values.iter().map(|v| v.path.as_str())) {
        if !reg.key_exists(&path)? {
            created_keys.push(path);
        }
    }
    for Target {
        path,
        name,
        installed,
    } in values
    {
        let before = reg.read_named(&path, &name)?;
        if path.contains(VERB) || path.contains(FALLBACK) {
            ensure!(
                before.is_none(),
                "Rust Explorer verb already exists without a backup; refusing to overwrite"
            );
        }
        changes.push(Change {
            path,
            name,
            before,
            installed,
        });
    }
    let backup = Backup {
        version: 2,
        changes,
        created_keys,
    };
    durable_create(
        dir.join("backup.json"),
        &serde_json::to_vec_pretty(&backup)?,
    )?;
    let recovery: Vec<u8> = std::iter::once(0xfeffu16)
        .chain(recovery_reg(&backup, &registry_root()?).encode_utf16())
        .flat_map(u16::to_le_bytes)
        .collect();
    // No registry writes until BOTH recovery files are safely on disk.
    if let Err(e) = durable_create(dir.join("restore.reg"), &recovery) {
        return Err(e.context("Recovery file could not be saved; associations unchanged"));
    }
    ready().context("独立恢复保护未就绪，未修改任何关联")?;
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
    disable_at(&dir)
}
fn disable_at(dir: &std::path::Path) -> Result<()> {
    let backup = read_backup(dir)?;
    let restored = restore(
        &RegistryStore {
            root: registry_root()?,
        },
        &backup,
    );
    notify_shell();
    restored?;
    // Retain an archived copy for inspection, freeing the active journal only on success.
    fs::copy(dir.join("backup.json"), dir.join("last-restored.json"))?;
    if dir.join("restore.reg").exists() {
        fs::remove_file(dir.join("restore.reg"))?;
    }
    fs::remove_file(dir.join("backup.json"))?;
    guard::clear(dir)?;
    Ok(())
}

/// Query Windows' merged association view, rather than reporting only our HKCU writes.
pub fn diagnostics() -> Result<serde_json::Value> {
    use windows::{
        Win32::UI::Shell::*,
        core::{PCWSTR, PWSTR},
    };
    let mut commands = serde_json::Map::new();
    for class in ["Directory", "Drive"] {
        let class_w: Vec<u16> = class.encode_utf16().chain(Some(0)).collect();
        let mut buf = vec![0u16; 32768];
        let mut count = buf.len() as u32;
        let hr = unsafe {
            AssocQueryStringW(
                ASSOCF_NONE,
                ASSOCSTR_COMMAND,
                PCWSTR(class_w.as_ptr()),
                PCWSTR::null(),
                Some(PWSTR(buf.as_mut_ptr())),
                &mut count,
            )
        };
        let value = if hr.is_ok() {
            String::from_utf16_lossy(&buf[..buf.iter().position(|n| *n == 0).unwrap_or(buf.len())])
        } else {
            format!("Unavailable: {hr:?}")
        };
        commands.insert(class.into(), value.into());
    }
    Ok(
        serde_json::json!({"status": status()?, "recovery_directory": recovery_directory()?, "registry_scope": registry_root()?, "trial": trial_pending(), "shell_commands": commands}),
    )
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
        fn read_named(&self, p: &str, name: &str) -> Result<Option<Raw>> {
            self.read(&format!("{p}\0{name}"))
        }
        fn write_named(&self, p: &str, name: &str, value: Option<&Raw>) -> Result<()> {
            self.write(&format!("{p}\0{name}"), value)
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
            .map(
                |(
                    i,
                    Target {
                        path,
                        name,
                        installed,
                    },
                )| Change {
                    path,
                    name,
                    installed,
                    before: (i % 3 == 2).then(|| Raw {
                        ty: 2,
                        bytes: vec![65, 0, 0, 0],
                    }),
                },
            )
            .collect();
        fs::remove_file(&exe).unwrap();
        fs::remove_dir(exe.parent().unwrap()).unwrap();
        Backup {
            version: 2,
            changes,
            created_keys: vec![],
        }
    }
    #[test]
    fn every_failed_write_rolls_back_exactly() {
        let b = backup();
        for fail in 1..=b.changes.len() {
            let reg = Memory::default();
            for c in &b.changes {
                c.write(&reg, c.before.as_ref()).unwrap();
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
            c.write(&reg, c.before.as_ref()).unwrap();
        }
        install(&reg, &b).unwrap();
        let foreign = Raw::string("OtherBrowser");
        b.changes[9].write(&reg, Some(&foreign)).unwrap();
        assert!(restore(&reg, &b).is_err());
        assert_eq!(b.changes[9].current(&reg).unwrap(), Some(foreign));
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
            c.write(&reg, c.before.as_ref()).unwrap();
        }
        install(&reg, &b).unwrap();
        restore(&reg, &b).unwrap();
        for c in &b.changes {
            assert_eq!(c.current(&reg).unwrap(), c.before);
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
            c.write(&reg, c.before.as_ref()).unwrap();
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
            assert_eq!(c.current(&reg).unwrap(), c.before);
        }
        CURRENT_USER.remove_tree(root).unwrap();
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn empty_restored_verb_can_be_reused_but_foreign_metadata_cannot() {
        let path = format!(
            "Software\\RustExplorer\\AssociationTests\\empty-{}",
            std::process::id()
        );
        let command = CURRENT_USER.create(format!("{path}\\command")).unwrap();
        assert!(reusable_verb(&path).unwrap());
        command
            .set_string("DelegateExecute", "other handler")
            .unwrap();
        assert!(!reusable_verb(&path).unwrap());
        command.remove_value("DelegateExecute").unwrap();
        assert!(reusable_verb(&path).unwrap());
        CURRENT_USER.remove_tree(path).unwrap();
    }
    #[test]
    fn guard_readiness_failure_never_modifies_registry() {
        let dir = std::env::temp_dir().join(format!("re-guard-failure-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let exe = dir.join("rust-explorer.exe");
        fs::write(&exe, b"fixture").unwrap();
        let reg = Memory::default();
        assert!(enable_prepared(&reg, &dir, &exe, || bail!("guard failed")).is_err());
        assert_eq!(reg.writes.get(), 0);
        assert!(read_backup(&dir).is_ok());
        assert!(dir.join("restore.reg").is_file());
        for file in ["rust-explorer.exe", "backup.json", "restore.reg"] {
            fs::remove_file(dir.join(file)).unwrap();
        }
        fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn registered_executable_uses_shell_compatible_path() {
        let dir = std::env::temp_dir().join(format!("re-shell-path-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let exe = dir.join("rust-explorer.exe");
        fs::write(&exe, b"fixture").unwrap();
        let values = plan(&exe.canonicalize().unwrap()).unwrap();
        let command = String::from_utf16(
            &values[0]
                .installed
                .bytes
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert!(!command.contains(r"\\?\"));
        let (registered_exe, arguments) =
            command.strip_prefix('"').unwrap().split_once('"').unwrap();
        // TEMP can use an 8.3 alias on hosted Windows runners. Verify the same
        // executable and exact arguments rather than the original path spelling.
        assert_eq!(
            PathBuf::from(registered_exe).canonicalize().unwrap(),
            exe.canonicalize().unwrap()
        );
        assert_eq!(arguments.trim_end_matches('\0'), " --open-folder \"%1\\.\"");
        fs::remove_file(exe).unwrap();
        fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn version_one_backup_remains_restorable_and_names_are_whitelisted() {
        let mut b = backup();
        b.changes = b
            .changes
            .into_iter()
            .enumerate()
            .filter_map(|(i, c)| [0, 1, 9, 10, 11, 19].contains(&i).then_some(c))
            .collect();
        b.version = 1;
        let mut json = serde_json::to_value(&b).unwrap();
        for c in json["changes"].as_array_mut().unwrap() {
            c.as_object_mut().unwrap().remove("name");
        }
        json.as_object_mut().unwrap().remove("created_keys");
        let mut old: Backup = serde_json::from_value(json).unwrap();
        validate(&old).unwrap();
        let reg = Memory::default();
        for c in &old.changes {
            c.write(&reg, c.before.as_ref()).unwrap();
        }
        install(&reg, &old).unwrap();
        restore(&reg, &old).unwrap();
        old.changes[0].name = "OtherValue".into();
        assert!(validate(&old).is_err());
    }
}
