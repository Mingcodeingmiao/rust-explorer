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
pub struct RecycleInfo {
    pub original_location: String,
    pub deleted: Option<SystemTime>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    #[serde(default)]
    pub recycle: Option<RecycleInfo>,
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

/// Labels for confirmation dialogs must use Shell display names, not PIDL tokens.
pub fn dialog_item_name(path: &Path, entries: &[Entry]) -> String {
    if let Some(entry) = entries.iter().find(|entry| entry.path == path) {
        if !entry.name.is_empty() {
            return entry.name.clone();
        }
    }
    if crate::shell_folder::is_shell_path(path) {
        return "所选 Shell 项目".into();
    }
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
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
            recycle: None,
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
    Original,
    Deleted,
    Modified,
    Created,
    Kind,
    Size,
}

pub fn sort(entries: &mut [Entry], column: Sort, descending: bool) {
    // Natural order compares lowercase names, and comparing them byte by byte
    // dominated huge-folder listings (WinSxS: 16k entries). Build one sort key per
    // entry (digit runs become marker + length + digits, so a plain byte
    // comparison yields the same order), sort an index permutation, then apply it.
    let keys: Vec<Vec<u8>> = entries
        .iter()
        .map(|entry| natural_key(&entry.name.to_lowercase()))
        .collect();
    let mut order: Vec<u32> = (0..entries.len() as u32).collect();
    order.sort_unstable_by(|left, right| {
        let (left, right) = (*left as usize, *right as usize);
        let (a, b) = (&entries[left], &entries[right]);
        let directory_order = if column == Sort::Modified {
            a.directory.cmp(&b.directory)
        } else {
            b.directory.cmp(&a.directory)
        };
        directory_order.then_with(|| {
            let (key_a, key_b) = (&keys[left], &keys[right]);
            let value = match column {
                Sort::Name => key_a.cmp(key_b),
                Sort::Modified => a.modified.cmp(&b.modified),
                Sort::Created => a.created.cmp(&b.created),
                Sort::Original => a
                    .recycle
                    .as_ref()
                    .map(|i| &i.original_location)
                    .cmp(&b.recycle.as_ref().map(|i| &i.original_location)),
                Sort::Deleted => a
                    .recycle
                    .as_ref()
                    .and_then(|i| i.deleted)
                    .cmp(&b.recycle.as_ref().and_then(|i| i.deleted)),
                Sort::Kind => a.kind.cmp(&b.kind),
                Sort::Size => a.size.cmp(&b.size),
            };
            let value = value.then_with(|| key_a.cmp(key_b));
            if descending { value.reverse() } else { value }
        })
    });
    // `order[i]` is the source index that belongs at position `i`. Turn it into a
    // destination map so the permutation can be applied with in-place swaps.
    let mut destination = vec![0u32; order.len()];
    for (position, source) in order.iter().enumerate() {
        destination[*source as usize] = position as u32;
    }
    for index in 0..destination.len() {
        while destination[index] as usize != index {
            let other = destination[index] as usize;
            entries.swap(index, other);
            destination.swap(index, other);
        }
    }
}

/// Byte key whose lexicographic order equals [`natural_cmp`] for lowercase names.
/// A digit run becomes `b'0'` (sorts like a digit against other characters), its
/// length as big-endian bytes and the digits with leading zeros removed.
pub fn natural_key(lowercase: &str) -> Vec<u8> {
    let bytes = lowercase.as_bytes();
    let mut key = Vec::with_capacity(bytes.len() + 4);
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_digit() {
            let start = index;
            while index < bytes.len() && bytes[index].is_ascii_digit() {
                index += 1;
            }
            let mut begin = start;
            while begin + 1 < index && bytes[begin] == b'0' {
                begin += 1;
            }
            let len = (index - begin).min(u16::MAX as usize);
            key.push(b'0');
            key.extend_from_slice(&(len as u16).to_be_bytes());
            key.extend_from_slice(&bytes[begin..begin + len]);
        } else {
            key.push(bytes[index]);
            index += 1;
        }
    }
    key
}

// Numeric runs compare without parsing, so very long numbers cannot overflow.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    natural_cmp_lower(&a.to_lowercase(), &b.to_lowercase())
}

/// `natural_cmp` for names that are already lowercase.
fn natural_cmp_lower(a: &str, b: &str) -> Ordering {
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
    // The consumed prefixes are equal; whatever is left decides, so a name that
    // only continues with more characters sorts after the shorter one. Total
    // length must not decide: `a1` and `a01` are the same number.
    match (i < a.len(), j < b.len()) {
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        _ => Ordering::Equal,
    }
}

pub fn read_directory(path: &Path, cancelled: &Arc<AtomicBool>) -> Result<(Vec<Entry>, usize)> {
    #[cfg(windows)]
    {
        return read_directory_find(path, cancelled);
    }
    #[cfg(not(windows))]
    {
        let mut entries = Vec::new();
        let mut skipped = 0;
        for item in fs::read_dir(path).with_context(|| format!("无法打开 {}", path.display()))?
        {
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
}

/// Directory scan for Windows.
///
/// `FindNextFileW` returns the name, attributes, size, timestamps *and* the reparse
/// tag in one call, so a 16k-entry folder (WinSxS) is listed in one pass instead of
/// one path lookup plus one reparse probe per entry. `DirEntry::metadata`-based
/// enumeration was ~20x slower on such folders.
#[cfg(windows)]
fn read_directory_find(path: &Path, cancelled: &Arc<AtomicBool>) -> Result<(Vec<Entry>, usize)> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        Win32::Storage::FileSystem::{FindClose, FindFirstFileW, FindNextFileW, WIN32_FIND_DATAW},
        core::PCWSTR,
    };
    let mut pattern = path.to_path_buf();
    pattern.push("*");
    // FindFirstFileW needs the extended prefix for paths beyond MAX_PATH.
    let wide: Vec<u16> = if pattern.as_os_str().len() > 240 && pattern.is_absolute() {
        let mut prefixed = std::ffi::OsString::from(r"\\?\");
        prefixed.push(pattern.as_os_str());
        prefixed.encode_wide().chain(Some(0)).collect()
    } else {
        pattern.as_os_str().encode_wide().chain(Some(0)).collect()
    };
    let mut data = WIN32_FIND_DATAW::default();
    let handle = match unsafe { FindFirstFileW(PCWSTR(wide.as_ptr()), &mut data) } {
        Ok(handle) => handle,
        // An empty folder reports "file not found"; anything else is a real error.
        Err(error) => {
            if error.code() == windows::Win32::Foundation::ERROR_FILE_NOT_FOUND.to_hresult() {
                return Ok((Vec::new(), 0));
            }
            return Err(error).with_context(|| format!("无法打开 {}", path.display()));
        }
    };
    let mut entries = Vec::new();
    let mut skipped = 0;
    loop {
        if cancelled.load(AtomicOrdering::Relaxed) {
            break;
        }
        // Keep the raw wide name: `Entry::path` must stay valid for names that are
        // not representable as UTF-8, even though `Entry::name` is lossy.
        let raw = os_string_from_wide(&data.cFileName);
        if raw != std::ffi::OsStr::new(".") && raw != std::ffi::OsStr::new("..") {
            match find_data_entry(path, raw, &data) {
                Some(entry) => entries.push(entry),
                None => skipped += 1,
            }
        }
        if unsafe { FindNextFileW(handle, &mut data) }.is_err() {
            break;
        }
    }
    let _ = unsafe { FindClose(handle) };
    Ok((entries, skipped))
}

#[cfg(windows)]
fn os_string_from_wide(buffer: &[u16]) -> std::ffi::OsString {
    use std::os::windows::ffi::OsStringExt;
    let len = buffer
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(buffer.len());
    std::ffi::OsString::from_wide(&buffer[..len])
}

#[cfg(windows)]
fn find_data_entry(
    parent: &Path,
    raw: std::ffi::OsString,
    data: &windows::Win32::Storage::FileSystem::WIN32_FIND_DATAW,
) -> Option<Entry> {
    use windows::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_ATTRIBUTE_SYSTEM,
    };
    let attributes = data.dwFileAttributes;
    let directory = attributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0;
    if raw.is_empty() {
        return None;
    }
    let name = raw.to_string_lossy().into_owned();
    let path = parent.join(&raw);
    // The reparse tag comes with the enumeration data, so a link needs no second
    // path lookup (WinSxS-like folders have thousands of reparse points).
    let link = if Path::new(&raw)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("lnk") || ext.eq_ignore_ascii_case("url"))
    {
        true
    } else if attributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        matches!(data.dwReserved0, 0xA0000003 | 0xA000000C)
    } else {
        false
    };
    let kind = if directory {
        "文件夹".into()
    } else {
        Path::new(&raw)
            .extension()
            .map(|value| format!("{} 文件", value.to_string_lossy().to_uppercase()))
            .unwrap_or_else(|| "文件".into())
    };
    Some(Entry {
        recycle: None,
        path,
        name,
        directory,
        link,
        hidden: attributes & FILE_ATTRIBUTE_HIDDEN.0 != 0,
        system: attributes & FILE_ATTRIBUTE_SYSTEM.0 != 0,
        size: (u64::from(data.nFileSizeHigh) << 32) | u64::from(data.nFileSizeLow),
        total: None,
        free: None,
        modified: find_time(data.ftLastWriteTime),
        created: find_time(data.ftCreationTime),
        kind,
    })
}

#[cfg(windows)]
fn find_time(time: windows::Win32::Foundation::FILETIME) -> Option<SystemTime> {
    let ticks = (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    if ticks == 0 {
        return None;
    }
    let seconds = ticks / 10_000_000;
    if seconds < 11_644_473_600 {
        return None;
    }
    let nanos = ((ticks % 10_000_000) * 100).try_into().ok()?;
    Some(std::time::UNIX_EPOCH + std::time::Duration::new(seconds - 11_644_473_600, nanos))
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

/// The folder on the way to `dir` that sits directly under `target`. Going to an
/// ancestor (`↑`, breadcrumb click) re-selects it, matching Explorer.
pub fn child_towards(dir: &Path, target: &Path) -> Option<PathBuf> {
    if dir == target {
        return None;
    }
    let rest = dir.strip_prefix(target).ok()?;
    let first = rest.components().next()?;
    Some(target.join(first))
}

/// View state remembered for one history entry, so going back returns to the
/// same scroll position with the same item selected (like Explorer).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HistoryState {
    /// First visible row (fractional) used when no item was selected.
    pub row: f32,
    /// The item the user was acting on.
    pub focus: Option<PathBuf>,
    pub selection: Vec<PathBuf>,
}

#[derive(Clone)]
pub struct History {
    paths: Vec<PathBuf>,
    states: Vec<Option<HistoryState>>,
    index: usize,
}
impl History {
    pub fn new(path: PathBuf) -> Self {
        Self {
            paths: vec![path],
            states: vec![None],
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
        self.states.truncate(self.index + 1);
        self.paths.push(path);
        self.states.push(None);
        self.index += 1;
    }
    pub fn can_back(&self) -> bool {
        self.index > 0
    }
    pub fn can_forward(&self) -> bool {
        self.index + 1 < self.paths.len()
    }
    pub fn back(&mut self) -> bool {
        if self.can_back() {
            self.index -= 1;
            true
        } else {
            false
        }
    }
    pub fn forward(&mut self) -> bool {
        if self.can_forward() {
            self.index += 1;
            true
        } else {
            false
        }
    }
    /// Remember how the current entry was left (scroll position and selection).
    pub fn remember(&mut self, state: HistoryState) {
        if let Some(slot) = self.states.get_mut(self.index) {
            *slot = Some(state);
        }
    }
    /// State saved when this entry was left, if any.
    pub fn state(&self) -> Option<HistoryState> {
        self.states.get(self.index).cloned().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn modified_sort_keeps_files_before_folders_in_both_directions() {
        let file = recycled("file", "recycle:0000", "D:\\", 1);
        let mut folder = recycled("folder", "recycle:0100", "D:\\", 1);
        folder.directory = true;
        for descending in [false, true] {
            let mut entries = vec![folder.clone(), file.clone()];
            sort(&mut entries, Sort::Modified, descending);
            assert!(!entries[0].directory);
            assert!(entries[1].directory);
        }
    }
    fn recycled(name: &str, token: &str, original: &str, deleted: u64) -> Entry {
        Entry {
            recycle: Some(RecycleInfo {
                original_location: original.into(),
                deleted: Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(deleted)),
            }),
            path: token.into(),
            name: name.into(),
            directory: false,
            link: false,
            hidden: false,
            system: false,
            size: 0,
            total: None,
            free: None,
            modified: None,
            created: None,
            kind: "TXT 文件".into(),
        }
    }
    #[test]
    fn recycle_confirmation_uses_unicode_display_names_not_pidl_tokens() {
        let entry = recycled("测试报告💾.txt", "recycle:00001400abcd", "D:\\报告", 2);
        assert_eq!(
            dialog_item_name(&entry.path, std::slice::from_ref(&entry)),
            "测试报告💾.txt"
        );
        let fallback = dialog_item_name(Path::new("recycle:00001400abcd"), &[]);
        assert!(!fallback.contains("00001400"));
        assert_eq!(
            dialog_item_name(Path::new("C:\\folder\\报告.txt"), &[]),
            "报告.txt"
        );
    }
    #[test]
    fn recycle_columns_sort_by_original_location_and_deletion_date() {
        let mut entries = vec![
            recycled("a.txt", "recycle:0000", "D:\\z", 1),
            recycled("z.txt", "recycle:0100", "D:\\a", 2),
        ];
        sort(&mut entries, Sort::Original, false);
        assert_eq!(entries[0].name, "z.txt");
        sort(&mut entries, Sort::Deleted, false);
        assert_eq!(entries[0].name, "a.txt");
        sort(&mut entries, Sort::Deleted, true);
        assert_eq!(entries[0].name, "z.txt");
    }
    #[test]
    fn older_entry_payloads_default_to_not_a_link() {
        let old = r#"{"path":"old.txt","name":"old.txt","directory":false,"hidden":false,"size":4,"modified":null,"kind":"TXT"}"#;
        let entry: Entry = serde_json::from_str(old).unwrap();
        assert!(!entry.link);
        assert!(!entry.system);
    }
    #[test]
    fn natural_sort_key_orders_like_the_comparer() {
        let names = [
            "图2.png",
            "图10.png",
            "图010.png",
            "a",
            "a1",
            "a01",
            "a1b",
            "a2",
            "a!",
            "a b",
            "a:b",
            "amd64_microsoft-windows-netcfg_31bf3856ad364e35_10.0.26100.1",
            "amd64_microsoft-windows-netcfg_31bf3856ad364e35_10.0.26100.1150",
        ];
        for left in names {
            for right in names {
                let mut by_key = vec![left, right];
                by_key.sort_by_key(|name| natural_key(&name.to_lowercase()));
                let mut by_comparer = vec![left, right];
                by_comparer.sort_by(|a, b| natural_cmp(a, b));
                assert_eq!(
                    by_key, by_comparer,
                    "key order differs for {left:?} vs {right:?}"
                );
            }
        }
    }
    #[test]
    fn numeric_names() {
        assert_eq!(natural_cmp("图2.png", "图10.png"), Ordering::Less);
        assert_eq!(
            natural_cmp("a999999999999999999999", "a1000000000000000000000"),
            Ordering::Less
        );
        // Leading zeros do not change the number, and a prefix sorts first.
        assert_eq!(natural_cmp("a01", "a1"), Ordering::Equal);
        assert_eq!(natural_cmp("图010.png", "图10.png"), Ordering::Equal);
        assert_eq!(natural_cmp("a1", "a1b"), Ordering::Less);
        assert_eq!(natural_cmp("a01", "a1b"), Ordering::Less);
    }
    #[test]
    fn rejects_unsafe_names() {
        for n in ["", "..", "CON.txt", "a/b", "x ", "x.", "COM1"] {
            assert!(valid_name(n).is_err(), "{n}");
        }
        assert!(valid_name("报告 2026.pdf").is_ok());
    }
    #[test]
    fn child_towards_selects_the_folder_on_the_way_to_the_ancestor() {
        assert_eq!(
            child_towards(
                Path::new("C:\\Windows\\system32\\drivers"),
                Path::new("C:\\Windows")
            ),
            Some(PathBuf::from("C:\\Windows\\system32"))
        );
        assert_eq!(
            child_towards(Path::new("C:\\Windows\\system32"), Path::new("C:\\")),
            Some(PathBuf::from("C:\\Windows"))
        );
        // The current folder itself and unrelated paths have no child.
        assert_eq!(
            child_towards(Path::new("C:\\Windows"), Path::new("C:\\Windows")),
            None
        );
        assert_eq!(
            child_towards(Path::new("C:\\Windows"), Path::new("D:\\work")),
            None
        );
        assert_eq!(
            child_towards(Path::new("C:\\Windows"), Path::new("D:\\")),
            None
        );
    }
    #[test]
    fn find_scan_reads_names_sizes_and_directory_flags() {
        let root = std::env::temp_dir().join(format!(
            "re-find-scan-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("子目录")).unwrap();
        std::fs::write(root.join("data.bin"), b"12345").unwrap();
        std::fs::write(root.join("link.lnk"), b"x").unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let (mut entries, skipped) = read_directory(&root, &cancel).unwrap();
        assert_eq!(skipped, 0);
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(entries.len(), 3);
        let data = entries.iter().find(|e| e.name == "data.bin").unwrap();
        assert!(!data.directory && !data.link);
        assert_eq!(data.size, 5);
        assert!(data.modified.is_some());
        assert_eq!(data.kind, "BIN 文件");
        let folder = entries.iter().find(|e| e.name == "子目录").unwrap();
        assert!(folder.directory);
        assert_eq!(folder.kind, "文件夹");
        // A `.lnk` is a link without asking the Shell.
        let link = entries.iter().find(|e| e.name == "link.lnk").unwrap();
        assert!(link.link && !link.directory);
        // Paths keep the real entry names.
        assert_eq!(folder.path, root.join("子目录"));
        std::fs::remove_file(root.join("data.bin")).unwrap();
        std::fs::remove_file(root.join("link.lnk")).unwrap();
        std::fs::remove_dir(root.join("子目录")).unwrap();
        std::fs::remove_dir(&root).unwrap();
    }
    #[test]
    fn find_scan_reports_empty_and_missing_directories() {
        let root = std::env::temp_dir().join(format!(
            "re-find-empty-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let (entries, skipped) = read_directory(&root, &cancel).unwrap();
        assert!(entries.is_empty() && skipped == 0);
        assert!(read_directory(&root.join("missing"), &cancel).is_err());
        std::fs::remove_dir(&root).unwrap();
    }
    #[test]
    fn indexed_sort_applies_the_permutation_correctly() {
        let make = |name: &str, directory: bool| Entry {
            recycle: None,
            path: PathBuf::from(name),
            name: name.into(),
            directory,
            link: false,
            hidden: false,
            system: false,
            size: 1,
            total: None,
            free: None,
            modified: None,
            created: None,
            kind: if directory { "文件夹" } else { "TXT 文件" }.into(),
        };
        let mut entries = vec![
            make("b.txt", false),
            make("a_dir", true),
            make("c.txt", false),
            make("10.txt", false),
            make("2_dir", true),
        ];
        sort(&mut entries, Sort::Name, false);
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        // Folders first, natural order inside each group.
        assert_eq!(names, ["2_dir", "a_dir", "10.txt", "b.txt", "c.txt"]);
        sort(&mut entries, Sort::Name, true);
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        // Descending still keeps folders first and reverses within each group.
        assert_eq!(names, ["a_dir", "2_dir", "c.txt", "b.txt", "10.txt"]);
    }
    #[test]
    fn history_discards_forward_branch() {
        let mut h = History::new("C:\\a".into());
        h.push("C:\\b".into());
        assert!(h.can_back() && !h.can_forward());
        assert!(h.back());
        assert!(h.can_forward() && !h.can_back());
        h.push("C:\\c".into());
        assert!(!h.forward());
        assert_eq!(h.current(), Path::new("C:\\c"));
    }
    #[test]
    fn history_keeps_the_view_state_of_each_entry() {
        let mut h = History::new("C:\\a".into());
        h.push("C:\\b".into());
        let state = HistoryState {
            row: 12.5,
            focus: Some(PathBuf::from("C:\\a\\报告.pdf")),
            selection: vec![PathBuf::from("C:\\a\\报告.pdf")],
        };
        h.remember(state.clone());
        assert!(h.back());
        assert_eq!(h.state(), None);
        h.remember(HistoryState {
            row: 3.,
            ..Default::default()
        });
        assert!(h.forward());
        assert_eq!(h.state(), Some(state));
        assert!(h.back());
        assert_eq!(h.state().unwrap().row, 3.);
    }
    #[test]
    fn pushing_a_new_path_after_going_back_drops_the_old_state() {
        let mut h = History::new("C:\\a".into());
        h.push("C:\\b".into());
        h.remember(HistoryState {
            row: 7.,
            focus: None,
            selection: vec![],
        });
        assert!(h.back());
        h.push("C:\\c".into());
        assert!(!h.can_forward());
        assert_eq!(h.state(), None);
    }
    #[test]
    fn numbered_folder_skips_existing_files_and_reuses_gaps() {
        let parent = std::env::temp_dir().join(format!(
            "re-numbered-folder-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&parent).unwrap();
        std::fs::write(parent.join("新建文件夹 1"), b"owned fixture").unwrap();
        assert_eq!(
            create_numbered_folder(&parent).unwrap(),
            parent.join("新建文件夹 2")
        );
        assert_eq!(
            create_numbered_folder(&parent).unwrap(),
            parent.join("新建文件夹 3")
        );
        std::fs::remove_dir(parent.join("新建文件夹 2")).unwrap();
        assert_eq!(
            create_numbered_folder(&parent).unwrap(),
            parent.join("新建文件夹 2")
        );
        std::fs::remove_dir_all(parent).unwrap();
    }
}

/// Atomically reserve the first available numbered folder, including gaps.
pub fn create_numbered_folder(parent: &Path) -> std::io::Result<PathBuf> {
    for number in 1..=u32::MAX {
        let path = parent.join(format!("新建文件夹 {number}"));
        match std::fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::other("没有可用的新建文件夹名称"))
}
