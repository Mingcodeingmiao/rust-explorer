//! Bounded address-bar completion. Enumeration runs in a disposable helper process.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Candidate {
    pub path: PathBuf,
    pub directory: bool,
}
pub fn query(text: &str) -> Option<(PathBuf, String)> {
    let text = text.trim().trim_matches('"');
    if text.is_empty() || text.starts_with("::") {
        return None;
    }
    let path = Path::new(text);
    if text.ends_with(['\\', '/']) {
        Some((path.to_path_buf(), String::new()))
    } else {
        Some((
            path.parent()?.to_path_buf(),
            path.file_name()?.to_string_lossy().to_lowercase(),
        ))
    }
}
pub fn collect(text: &str) -> anyhow::Result<Vec<Candidate>> {
    let Some((parent, prefix)) = query(text) else {
        return Ok(vec![]);
    };
    if !parent.is_absolute() {
        return Ok(vec![]);
    }
    let mut matches = vec![];
    for entry in std::fs::read_dir(parent)?.take(20_000).flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .to_lowercase()
            .starts_with(&prefix)
        {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        matches.push(Candidate {
            path: entry.path(),
            directory: kind.is_dir(),
        });
        if matches.len() >= 256 {
            break;
        }
    }
    matches
        .sort_by_cached_key(|item| (!item.directory, item.path.to_string_lossy().to_lowercase()));
    Ok(matches)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_prefix_and_trailing_separator() {
        assert_eq!(
            query(r"C:\Windows\Sys"),
            Some((PathBuf::from(r"C:\Windows"), "sys".into()))
        );
        assert_eq!(
            query("C:\\Windows\\"),
            Some((PathBuf::from("C:\\Windows\\"), String::new()))
        );
        assert!(query("::Computer").is_none());
        assert!(query("").is_none());
    }
    #[test]
    fn folders_and_files_are_sorted_and_filtered() {
        let root =
            std::env::temp_dir().join(format!("rust-explorer-complete-{}", std::process::id()));
        std::fs::create_dir_all(root.join("alpha-folder")).unwrap();
        std::fs::write(root.join("alpha-file.txt"), b"test").unwrap();
        let results = collect(&root.join("ALPHA").to_string_lossy()).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results[0].directory);
        assert!(!results[1].directory);
        std::fs::remove_file(root.join("alpha-file.txt")).unwrap();
        std::fs::remove_dir(root.join("alpha-folder")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
