//! On-demand, cancellable, single-worker traversal. Never follows reparse points.
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
#[derive(Clone, Debug, Default)]
pub struct Usage {
    pub bytes: u64,
    pub files: u64,
    pub skipped: u64,
    pub complete: bool,
}
fn reparse(m: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        m.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        m.file_type().is_symlink()
    }
}
pub fn calculate(path: &Path, cancel: &Arc<AtomicBool>) -> Usage {
    let mut result = Usage::default();
    let mut directories = Vec::<std::fs::ReadDir>::new();
    let mut next = Some(path.to_path_buf());
    loop {
        if cancel.load(Ordering::Relaxed) {
            return result;
        }
        let path = if let Some(path) = next.take() {
            path
        } else {
            let Some(directory) = directories.last_mut() else {
                break;
            };
            match directory.next() {
                Some(Ok(entry)) => entry.path(),
                Some(Err(_)) => {
                    result.skipped += 1;
                    continue;
                }
                None => {
                    directories.pop();
                    continue;
                }
            }
        };
        let m = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(_) => {
                result.skipped += 1;
                continue;
            }
        };
        if reparse(&m) {
            result.skipped += 1;
            continue;
        }
        if m.is_dir() {
            match std::fs::read_dir(path) {
                Ok(items) => directories.push(items),
                Err(_) => result.skipped += 1,
            }
        } else {
            result.bytes = result.bytes.saturating_add(m.len());
            result.files += 1;
        }
    }
    result.complete = true;
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn totals_and_cancellation() {
        let root = std::env::temp_dir().join(format!("rust-explorer-du-{}", std::process::id()));
        std::fs::create_dir_all(root.join("nested")).unwrap();
        std::fs::write(root.join("a"), [0; 17]).unwrap();
        std::fs::write(root.join("nested/b"), [0; 29]).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let usage = calculate(&root, &cancel);
        assert_eq!((usage.bytes, usage.files, usage.complete), (46, 2, true));
        cancel.store(true, Ordering::Relaxed);
        assert!(!calculate(&root, &cancel).complete);
        std::fs::remove_dir_all(root).unwrap();
    }
}
