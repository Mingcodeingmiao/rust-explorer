use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    cmp::Ordering,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering as AtomicOrdering},
    },
    time::SystemTime,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub directory: bool,
    #[serde(default)]
    pub link: bool,
    pub hidden: bool,
    #[serde(default)]
    pub system: bool,
    pub size: u64,
    /// Portable-device storage capacity/free space (Shell properties).
    #[serde(default)]
    pub total: Option<u64>,
    #[serde(default)]
    pub free: Option<u64>,
    pub modified: Option<SystemTime>,
    #[serde(default)]
    pub created: Option<SystemTime>,
    pub kind: String,
}

impl Entry {
    pub fn from_path(path: PathBuf) -> Result<Self> {
        let metadata =
            fs::symlink_metadata(&path).with_context(|| format!("无法读取 {}", path.display()))?;
        #[cfg(windows)]
        let hidden = {
            use std::os::windows::fs::MetadataExt;
            metadata.file_attributes() & 2 != 0
        };
        #[cfg(not(windows))]
        let hidden = false;
        use std::os::windows::fs::MetadataExt;
        // Junction metadata describes the link itself; DIRECTORY remains set
        // even though FileType::is_dir() may be false for a name surrogate.
        let directory = metadata.file_attributes() & 0x10 != 0;
        let link = link_from_attributes(&path, metadata.file_attributes());
        let name = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .into_owned();
        let kind = if directory {
            "文件夹".into()
        } else {
            path.extension()
                .map(|v| format!("{} 文件", v.to_string_lossy().to_uppercase()))
                .unwrap_or("文件".into())
        };
        Ok(Self {
            path,
            name,
            directory,
            link,
            hidden,
            system: {
                use std::os::windows::fs::MetadataExt;
                metadata.file_attributes() & 4 != 0
            },
            size: metadata.len(),
            total: None,
            free: None,
            modified: metadata.modified().ok(),
            created: metadata.created().ok(),
            kind,
        })
    }
    pub fn date(&self) -> String {
        self.modified
            .map(|v| {
                chrono::DateTime::<chrono::Local>::from(v)
                    .format("%Y/%m/%d %H:%M")
                    .to_string()
            })
            .unwrap_or_default()
    }
    pub fn size_label(&self) -> String {
        if self.directory {
            String::new()
        } else {
            size_label(self.size)
        }
    }
}

/// Called only during directory/search enumeration in a worker. Not every
/// reparse point is a link: OneDrive placeholders must retain their own badges.
pub fn link_from_attributes(path: &Path, attributes: u32) -> bool {
    if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("lnk") || ext.eq_ignore_ascii_case("url"))
    {
        return true;
    }
    if attributes & 0x400 == 0 {
        return false;
    }
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        Win32::Storage::FileSystem::{FindClose, FindFirstFileW, WIN32_FIND_DATAW},
        core::PCWSTR,
    };
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut data = WIN32_FIND_DATAW::default();
    unsafe {
        let Ok(handle) = FindFirstFileW(PCWSTR(wide.as_ptr()), &mut data) else {
            return false;
        };
        let _ = FindClose(handle);
    }
    matches!(data.dwReserved0, 0xA0000003 | 0xA000000C)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sort {
    Name,
    Modified,
    Created,
    Kind,
    Size,
}

pub fn sort(entries: &mut [Entry], column: Sort, descending: bool) {
    entries.sort_unstable_by(|a, b| {
        b.directory.cmp(&a.directory).then_with(|| {
            let value = match column {
                Sort::Name => natural_cmp(&a.name, &b.name),
                Sort::Modified => a.modified.cmp(&b.modified),
                Sort::Created => a.created.cmp(&b.created),
                Sort::Kind => a.kind.cmp(&b.kind),
                Sort::Size => a.size.cmp(&b.size),
            };
            let value = value.then_with(|| natural_cmp(&a.name, &b.name));
            if descending { value.reverse() } else { value }
        })
    });
}

// Numeric runs compare without parsing, so very long numbers cannot overflow.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.to_lowercase(), b.to_lowercase());
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let (si, sj) = (i, j);
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let (mut ai, mut bj) = (si, sj);
            while ai + 1 < i && a[ai] == b'0' {
                ai += 1;
            }
            while bj + 1 < j && b[bj] == b'0' {
                bj += 1;
            }
            let order = (i - ai)
                .cmp(&(j - bj))
                .then_with(|| a[ai..i].cmp(&b[bj..j]));
            if order != Ordering::Equal {
                return order;
            }
        } else {
            let order = a[i].cmp(&b[j]);
            if order != Ordering::Equal {
                return order;
            }
            i += 1;
            j += 1;
        }
    }
    a.len().cmp(&b.len())
}

pub fn read_directory(path: &Path, cancelled: &Arc<AtomicBool>) -> Result<(Vec<Entry>, usize)> {
    let mut entries = Vec::new();
    let mut skipped = 0;
    for item in fs::read_dir(path).with_context(|| format!("无法打开 {}", path.display()))? {
        if cancelled.load(AtomicOrdering::Relaxed) {
            break;
        }
        match item.and_then(|v| Ok(v.path())) {
            Ok(path) => match Entry::from_path(path) {
                Ok(v) => entries.push(v),
                Err(_) => skipped += 1,
            },
            Err(_) => skipped += 1,
        }
    }
    Ok((entries, skipped))
}

pub fn valid_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.ends_with(['.', ' '])
        || name.chars().any(|v| v < ' ' || "<>:\"/\\|?*".contains(v))
    {
        bail!("名称包含无效字符，或以空格/句点结尾");
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    if [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ]
    .contains(&stem.as_str())
    {
        bail!("这是 Windows 保留名称");
    }
    Ok(())
}

pub fn size_label(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let units = ["KB", "MB", "GB", "TB"];
    let mut value = bytes as f64 / 1024.;
    let mut index = 0;
    while value >= 1024. && index < 3 {
        value /= 1024.;
        index += 1;
    }
    format!("{value:.1} {}", units[index])
}

#[derive(Clone)]
pub struct History {
    paths: Vec<PathBuf>,
    index: usize,
}
impl History {
    pub fn new(path: PathBuf) -> Self {
        Self {
            paths: vec![path],
            index: 0,
        }
    }
    pub fn current(&self) -> &Path {
        &self.paths[self.index]
    }
    pub fn push(&mut self, path: PathBuf) {
        if self.current() == path {
            return;
        }
        self.paths.truncate(self.index + 1);
        self.paths.push(path);
        self.index += 1;
    }
    pub fn back(&mut self) -> bool {
        if self.index > 0 {
            self.index -= 1;
            true
        } else {
            false
        }
    }
    pub fn forward(&mut self) -> bool {
        if self.index + 1 < self.paths.len() {
            self.index += 1;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn older_entry_payloads_default_to_not_a_link() {
        let old = r#"{"path":"old.txt","name":"old.txt","directory":false,"hidden":false,"size":4,"modified":null,"kind":"TXT"}"#;
        let entry: Entry = serde_json::from_str(old).unwrap();
        assert!(!entry.link);
        assert!(!entry.system);
    }
    #[test]
    fn numeric_names() {
        assert_eq!(natural_cmp("图2.png", "图10.png"), Ordering::Less);
        assert_eq!(
            natural_cmp("a999999999999999999999", "a1000000000000000000000"),
            Ordering::Less
        );
    }
    #[test]
    fn rejects_unsafe_names() {
        for n in ["", "..", "CON.txt", "a/b", "x ", "x.", "COM1"] {
            assert!(valid_name(n).is_err(), "{n}");
        }
        assert!(valid_name("报告 2026.pdf").is_ok());
    }
    #[test]
    fn history_discards_forward_branch() {
        let mut h = History::new("C:\\a".into());
        h.push("C:\\b".into());
        assert!(h.back());
        h.push("C:\\c".into());
        assert!(!h.forward());
        assert_eq!(h.current(), Path::new("C:\\c"));
    }
}
