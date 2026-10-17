//! Native file information for delayed hover cards.
//!
//! `collect` is blocking and MUST run only in the `--file-hint` child process.
//! The parent kills that process on timeout; Shell property handlers may block.
//! Folder statistics have a second, short internal work budget and never follow
//! junctions, symlinks, cloud placeholders or offline files.
use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileHintRequest {
    pub path: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HintLine {
    pub label: String,
    pub value: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileHint {
    /// The UI displays this as the first line when its visible filename is clipped.
    pub name: String,
    pub lines: Vec<HintLine>,
    /// Folder size is an incomplete lower bound, never an invented full total.
    pub partial: bool,
}

impl FileHint {
    fn push(&mut self, label: &str, value: impl Into<String>) {
        let value = value.into();
        if !value.trim().is_empty() {
            self.lines.push(HintLine {
                label: label.into(),
                value,
            });
        }
    }
}

fn date(time: Result<SystemTime, std::io::Error>) -> Option<String> {
    time.ok().map(|time| {
        DateTime::<Local>::from(time)
            .format("%Y/%m/%d %H:%M")
            .to_string()
    })
}

fn duration(ticks: u64) -> String {
    // System.Media.Duration is documented as 100 ns units.
    let s = ticks / 10_000_000;
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

fn unavailable_for_content(meta: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    // REPARSE_POINT, OFFLINE, RECALL_ON_OPEN, RECALL_ON_DATA_ACCESS.
    meta.file_attributes() & (0x400 | 0x1000 | 0x40000 | 0x400000) != 0
}

#[derive(Default)]
struct FolderSample {
    bytes: u64,
    files: Vec<String>,
    folders: Vec<String>,
    partial: bool,
}

fn sample_folder(path: &Path, budget: Duration, max_entries: usize) -> FolderSample {
    let start = Instant::now();
    let mut result = FolderSample::default();
    let mut pending = vec![(path.to_path_buf(), true)];
    let mut visited = 0;
    'walk: while let Some((dir, root)) = pending.pop() {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => {
                result.partial = true;
                continue;
            }
        };
        for entry in entries {
            if visited >= max_entries || start.elapsed() >= budget {
                result.partial = true;
                break 'walk;
            }
            visited += 1;
            let entry = match entry {
                Ok(e) => e,
                Err(_) => {
                    result.partial = true;
                    continue;
                }
            };
            let meta = match fs::symlink_metadata(entry.path()) {
                Ok(meta) => meta,
                Err(_) => {
                    result.partial = true;
                    continue;
                }
            };
            if root {
                let names = if meta.is_dir() {
                    &mut result.folders
                } else {
                    &mut result.files
                };
                if names.len() < 2 {
                    names.push(entry.file_name().to_string_lossy().into_owned());
                }
            }
            if unavailable_for_content(&meta) {
                result.partial = true;
            } else if meta.is_dir() {
                pending.push((entry.path(), false));
            } else if meta.is_file() {
                result.bytes = result.bytes.saturating_add(meta.len());
            }
        }
    }
    result
}

pub fn collect(path: &Path) -> Result<FileHint> {
    let meta =
        fs::symlink_metadata(path).with_context(|| format!("无法读取 {}", path.display()))?;
    let mut hint = FileHint {
        name: path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .into_owned(),
        lines: Vec::new(),
        partial: false,
    };
    if meta.is_dir() {
        if let Some(v) = date(meta.created()) {
            hint.push("创建日期", v);
        }
        if unavailable_for_content(&meta) || path.to_string_lossy().starts_with(r"\\") {
            hint.partial = true;
            hint.push("大小", "未统计");
        } else {
            let sample = sample_folder(path, Duration::from_millis(160), 4096);
            hint.partial = sample.partial;
            hint.push(
                "大小",
                format!(
                    "{}{}",
                    if sample.partial { "至少 " } else { "" },
                    crate::fs::size_label(sample.bytes)
                ),
            );
            if !sample.files.is_empty() {
                hint.push("文件", sample.files.join("、"));
            }
            if !sample.folders.is_empty() {
                hint.push("文件夹", sample.folders.join("、"));
            }
        }
        return Ok(hint);
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let fallback_type = if ext.is_empty() {
        "文件".into()
    } else {
        format!("{} 文件", ext.to_ascii_uppercase())
    };
    // Skip content property handlers for network items and placeholder links.
    let properties = if unavailable_for_content(&meta) || path.to_string_lossy().starts_with(r"\\")
    {
        None
    } else {
        NativeProperties::open(path).ok()
    };
    let file_type = properties
        .as_ref()
        .and_then(|p| p.text("System.ItemTypeText"))
        .unwrap_or(fallback_type);
    hint.push("项目类型", file_type);
    if let Some(p) = &properties {
        if let (Some(w), Some(h)) = (
            p.number("System.Image.HorizontalSize"),
            p.number("System.Image.VerticalSize"),
        ) {
            if w > 0 && h > 0 {
                hint.push("分辨率", format!("{w} × {h}"));
            }
        }
    }
    hint.push("大小", crate::fs::size_label(meta.len()));
    if let Some(v) = date(meta.modified()) {
        hint.push("修改日期", v);
    }
    if let Some(p) = &properties {
        if let Some(ticks) = p.number("System.Media.Duration").filter(|v| *v > 0) {
            hint.push("时长", duration(ticks));
        }
        if matches!(ext.as_str(), "exe" | "dll") {
            for (label, key) in [
                ("文件说明", "System.FileDescription"),
                ("文件版本", "System.FileVersion"),
                ("产品名称", "System.Software.ProductName"),
                ("产品版本", "System.Software.ProductVersion"),
                ("公司", "System.Company"),
                ("版权", "System.Copyright"),
                ("原始文件名", "System.OriginalFileName"),
            ] {
                if let Some(value) = p.text(key) {
                    hint.push(label, value);
                }
            }
        }
    }
    Ok(hint)
}

use windows::{
    Win32::{
        Foundation::PROPERTYKEY,
        System::Com::{
            COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize,
            StructuredStorage::{
                PROPVARIANT, PropVariantClear, PropVariantToString, PropVariantToUInt64,
            },
        },
        UI::Shell::PropertiesSystem::{
            GPS_BESTEFFORT, IPropertyStore, PSGetPropertyKeyFromName,
            SHGetPropertyStoreFromParsingName,
        },
    },
    core::HSTRING,
};

struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() }
    }
}
struct NativeProperties {
    store: IPropertyStore,
    _apartment: Apartment,
}
struct Value(PROPVARIANT);
impl Drop for Value {
    fn drop(&mut self) {
        unsafe {
            let _ = PropVariantClear(&mut self.0);
        }
    }
}
impl NativeProperties {
    fn open(path: &Path) -> Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        }
        let apartment = Apartment;
        // Shell parsing names require an absolute path; unlike canonicalize this
        // normalization does not follow a link or cause additional disk reads.
        let absolute = std::path::absolute(path)?;
        let store = unsafe {
            SHGetPropertyStoreFromParsingName(
                &HSTRING::from(absolute.as_os_str()),
                None,
                GPS_BESTEFFORT,
            )?
        };
        Ok(Self {
            store,
            _apartment: apartment,
        })
    }
    fn value(&self, name: &str) -> Option<Value> {
        let mut key = PROPERTYKEY::default();
        unsafe {
            PSGetPropertyKeyFromName(&HSTRING::from(name), &mut key).ok()?;
            let value = self.store.GetValue(&key).ok()?;
            Some(Value(value))
        }
    }
    fn text(&self, name: &str) -> Option<String> {
        let value = self.value(name)?;
        let mut buffer = [0u16; 2048];
        unsafe {
            PropVariantToString(&value.0, &mut buffer).ok()?;
        }
        let n = buffer.iter().position(|v| *v == 0).unwrap_or(buffer.len());
        let text = String::from_utf16_lossy(&buffer[..n]).trim().to_owned();
        (!text.is_empty()).then_some(text)
    }
    fn number(&self, name: &str) -> Option<u64> {
        let value = self.value(name)?;
        unsafe { PropVariantToUInt64(&value.0).ok() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn media_duration_units_are_100ns() {
        assert_eq!(duration(36_610_000_000), "01:01:01");
        assert_eq!(duration(9_999_999), "00:00:00");
    }
    #[test]
    fn folder_budget_never_reports_complete_size_when_cut_short() {
        let folder =
            std::env::temp_dir().join(format!("rust-explorer-hint-budget-{}", std::process::id()));
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("owned-fixture.txt"), b"12345").unwrap();
        let short = sample_folder(&folder, Duration::ZERO, 0);
        assert!(short.partial);
        assert_eq!(short.bytes, 0);
        let complete = sample_folder(&folder, Duration::from_secs(1), 100);
        assert!(!complete.partial);
        assert_eq!(complete.bytes, 5);
        assert_eq!(complete.files, ["owned-fixture.txt"]);
        fs::remove_file(folder.join("owned-fixture.txt")).unwrap();
        fs::remove_dir(&folder).unwrap();
    }
}
