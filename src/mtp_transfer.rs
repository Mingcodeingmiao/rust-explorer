//! WPD providers commonly reject a direct device-to-device CopyItem/MoveItem.
//! Stage through the PC so each Shell operation uses one device endpoint.
use super::{Request, dispatch};
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
};

pub(super) fn needed(paths: &[PathBuf], destination: &Path) -> bool {
    destination.to_string_lossy().starts_with("mtp:")
        && paths
            .iter()
            .any(|path| path.to_string_lossy().starts_with("mtp:"))
}

fn protected_root(path: &Path, parent: &Path, has_capacity: bool) -> bool {
    has_capacity
        || path.to_string_lossy().starts_with("mtp-root:")
        || parent.to_string_lossy().starts_with("mtp-root:")
}

/// The Shell's recursive WPD delete can fail with 0x8000FFFF. Queue one leaf
/// at a time, then its empty parent, rather than letting the engine recurse.
pub(super) fn delete(paths: &[PathBuf]) -> Result<()> {
    use windows::Win32::{System::Com::*, UI::Shell::*};
    for path in paths {
        let root = crate::shell_folder::entry(path)?;
        ensure!(
            !protected_root(
                path,
                &crate::shell_folder::parent(path)?,
                root.total.is_some() && root.free.is_some()
            ),
            "不能删除设备存储根目录"
        );
        let mut pending = vec![(path.clone(), false)];
        let mut visited = 0;
        while let Some((path, expanded)) = pending.pop() {
            ensure!(!super::stop_requested(), "操作已取消");
            visited += 1;
            ensure!(visited < 200_000, "删除项目过多，操作已停止");
            let entry = crate::shell_folder::entry(&path)?;
            if entry.directory && !expanded {
                pending.push((path, true));
                for child in crate::shell_folder::list_complete(&entry.path)?.entries {
                    pending.push((child.path, false));
                }
                continue;
            }
            if entry.directory {
                ensure!(
                    crate::shell_folder::list_complete(&path)?
                        .entries
                        .is_empty(),
                    "目录中出现新内容，已停止删除"
                );
            }
            let parent = crate::shell_folder::parent(&path)?;
            unsafe {
                let op: IFileOperation =
                    CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER)?;
                let cancel = super::attach_cancellation(&op)?;
                op.SetOperationFlags(FILEOPERATION_FLAGS(
                    FOF_NOCONFIRMATION.0 | FOF_NOERRORUI.0 | FOFX_EARLYFAILURE.0,
                ))?;
                let item = super::item(&path)?;
                op.DeleteItem(&item, None)?;
                op.PerformOperations()?;
                ensure!(
                    !op.GetAnyOperationsAborted()?.as_bool(),
                    "便携设备删除失败或已取消"
                );
                drop(item);
                drop(cancel);
                drop(op);
            }
            ensure!(
                !crate::shell_folder::list_complete(&parent)?
                    .entries
                    .iter()
                    .any(|child| child.name == entry.name),
                "便携设备未完成删除，请确认设备状态"
            );
        }
    }
    Ok(())
}

struct Staging(PathBuf);
impl Staging {
    fn new() -> Result<Self> {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("RustExplorer-MTP-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&path)?;
        Ok(Self(path))
    }
    fn folder(&self, name: &str) -> Result<PathBuf> {
        let path = self.0.join(name);
        std::fs::create_dir(&path)?;
        Ok(path)
    }
}
impl Drop for Staging {
    fn drop(&mut self) {
        // Delete only the unique directory created above, never a device path.
        if let (Ok(root), Ok(temp)) = (self.0.canonicalize(), std::env::temp_dir().canonicalize()) {
            if root.parent() == Some(temp.as_path())
                && root
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("RustExplorer-MTP-"))
                && std::fs::symlink_metadata(&self.0).is_ok_and(|m| !m.file_type().is_symlink())
            {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }
}

fn copy(paths: Vec<PathBuf>, destination: PathBuf) -> Result<()> {
    ensure!(!super::stop_requested(), "操作已取消");
    dispatch(Request::Copy {
        paths,
        destination,
        moving: false,
    })
}

fn manifest(root: &Path) -> Result<BTreeMap<PathBuf, Option<[u8; 32]>>> {
    let mut result = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        ensure!(!super::stop_requested(), "操作已取消");
        ensure!(
            result.len() < 100_000,
            "便携设备移动校验项目过多，已保留源项目"
        );
        let metadata = std::fs::symlink_metadata(&path)?;
        ensure!(!metadata.file_type().is_symlink(), "校验目录不能包含链接");
        let relative = path.strip_prefix(root)?.to_path_buf();
        if metadata.is_dir() {
            result.insert(relative, None);
            for child in std::fs::read_dir(path)? {
                pending.push(child?.path());
            }
        } else {
            let mut file = std::fs::File::open(path)?;
            let mut hash = Sha256::new();
            let mut bytes = [0u8; 65536];
            loop {
                ensure!(!super::stop_requested(), "操作已取消");
                let count = file.read(&mut bytes)?;
                if count == 0 {
                    break;
                }
                hash.update(&bytes[..count]);
            }
            result.insert(relative, Some(hash.finalize().into()));
        }
    }
    Ok(result)
}

type Inventory = BTreeMap<PathBuf, (bool, u64, Option<std::time::SystemTime>)>;
fn inventory(path: &Path) -> Result<Inventory> {
    let mut result = BTreeMap::new();
    let mut pending = vec![(PathBuf::new(), crate::shell_folder::entry(path)?)];
    while let Some((relative, entry)) = pending.pop() {
        ensure!(!super::stop_requested(), "操作已取消");
        ensure!(
            result.len() + pending.len() < 100_000,
            "便携设备项目过多，已停止传输"
        );
        result.insert(
            relative.clone(),
            (
                entry.directory,
                if entry.directory { 0 } else { entry.size },
                entry.modified,
            ),
        );
        if entry.directory {
            for child in crate::shell_folder::list_complete(&entry.path)?.entries {
                crate::fs::valid_name(&child.name)?;
                pending.push((relative.join(&child.name), child));
            }
        }
    }
    Ok(result)
}
fn verify_complete(local: &Path, expected: &Inventory) -> Result<()> {
    let actual = manifest(local)?;
    ensure!(
        actual.len() == expected.len(),
        "暂存副本项目不完整，已保留源项目"
    );
    for (relative, (directory, size, _)) in expected {
        let hash = actual
            .get(relative)
            .context("暂存副本缺少项目，已保留源项目")?;
        ensure!(hash.is_none() == *directory, "暂存副本类型不符");
        if !directory {
            ensure!(
                std::fs::metadata(local.join(relative))?.len() == *size,
                "暂存副本大小不符，已保留源项目"
            );
        }
    }
    Ok(())
}

fn same_contents_metadata(expected: &Inventory, actual: &Inventory) -> bool {
    expected.len() == actual.len()
        && expected.iter().all(|(path, (directory, size, _))| {
            actual
                .get(path)
                .is_some_and(|(found_directory, found_size, _)| {
                    directory == found_directory && size == found_size
                })
        })
}

pub(super) fn transfer(paths: &[PathBuf], destination: &Path, moving: bool) -> Result<()> {
    let targets = crate::shell_folder::list_complete(destination)?;
    // Refuse collisions and descendant targets before making any changes. The
    // PIDL ancestors come from the same Shell namespace as the source entries.
    let sources: Vec<_> = paths
        .iter()
        .map(|path| crate::shell_folder::entry(path))
        .collect::<Result<_>>()?;
    let mut names = std::collections::BTreeSet::new();
    for source in &sources {
        ensure!(
            !moving
                || !protected_root(
                    &source.path,
                    &crate::shell_folder::parent(&source.path)?,
                    source.total.is_some() && source.free.is_some()
                ),
            "不能移动设备存储根目录"
        );
        let source_item = super::item(&source.path)?;
        let mut ancestor = super::item(destination)?;
        let mut reached_root = false;
        for _ in 0..128 {
            ensure!(
                unsafe {
                    source_item.Compare(
                        &ancestor,
                        windows::Win32::UI::Shell::SICHINT_CANONICAL.0 as u32,
                    )
                }? != 0,
                "不能将项目放入自身或其子文件夹"
            );
            match unsafe { ancestor.GetParent() } {
                Ok(parent) => ancestor = parent,
                Err(_) => {
                    reached_root = true;
                    break;
                }
            }
        }
        ensure!(reached_root, "无法确认目标层级，已停止传输");
        crate::fs::valid_name(&source.name)?;
        ensure!(
            names.insert(source.name.to_lowercase()),
            "源项目有重名，请分批传输"
        );
        ensure!(
            !targets
                .entries
                .iter()
                .any(|entry| entry.name.eq_ignore_ascii_case(&source.name)),
            "目标已有同名项目，请先重命名或选择其他目录"
        );
        ensure!(
            !targets.parents.iter().any(|(_, path)| path == &source.path),
            "不能将项目放入自身或其子文件夹"
        );
    }
    let staging = Staging::new()?;
    let mut copies = Vec::new();
    let mut inventories = Vec::new();
    for (index, source) in sources.iter().enumerate() {
        let expected = inventory(&source.path)?;
        let folder = staging.folder(&format!("source-{index}"))?;
        copy(vec![source.path.clone()], folder.clone())
            .context("从手机读取暂存副本失败，源项目未删除")?;
        let local = folder.join(&source.name);
        ensure!(local.exists(), "手机暂存副本不完整，源项目未删除");
        verify_complete(&local, &expected)?;
        inventories.push(expected);
        copies.push(local);
    }
    copy(copies.clone(), destination.to_path_buf()).context("写入手机目标失败，源项目未删除")?;
    let uploaded = crate::shell_folder::list_complete(destination)?;
    let mut destinations = Vec::new();
    for (index, source) in sources.iter().enumerate() {
        let target = uploaded
            .entries
            .iter()
            .find(|entry| entry.name == source.name)
            .context("目标项目缺失，已保留源项目")?;
        ensure!(
            same_contents_metadata(&inventories[index], &inventory(&target.path)?),
            "目标清单或大小不完整，已保留源项目"
        );
        destinations.push(target.path.clone());
    }
    if moving {
        // Verify both the uploaded destination and a fresh source read against
        // the staged bytes. Never delete a source based on PerformOperations
        // alone, or after a skipped/partial copy or an intervening source edit.
        for (index, source) in sources.iter().enumerate() {
            let verify = staging.folder(&format!("verify-{index}"))?;
            copy(vec![destinations[index].clone()], verify.clone())?;
            let original = staging.folder(&format!("original-{index}"))?;
            copy(vec![source.path.clone()], original.clone())?;
            let expected = manifest(&copies[index])?;
            ensure!(
                manifest(&verify.join(&source.name))? == expected,
                "目标内容校验失败，已保留源项目"
            );
            ensure!(
                manifest(&original.join(&source.name))? == expected,
                "源项目内容发生变化，已保留源项目"
            );
            ensure!(
                inventory(&source.path)? == inventories[index],
                "源项目清单发生变化，已保留源项目"
            );
        }
        ensure!(!super::stop_requested(), "操作已取消，已保留源项目");
        // A move already authorizes removing the verified source. Do not use
        // Delete's WANTNUKEWARNING here: that opens a second confirmation and
        // leaves a background move waiting even though the target is complete.
        dispatch(Request::PermanentDelete {
            paths: paths.to_vec(),
        })
        .context("目标复制并校验成功，但源项目删除未完成")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn upload_validation_allows_new_timestamps_but_rejects_missing_or_truncated_files() {
        let expected = Inventory::from([("file".into(), (false, 10, None))]);
        let mut actual = Inventory::from([(
            "file".into(),
            (false, 10, Some(std::time::SystemTime::now())),
        )]);
        assert!(same_contents_metadata(&expected, &actual));
        actual.get_mut(Path::new("file")).unwrap().1 = 9;
        assert!(!same_contents_metadata(&expected, &actual));
        actual.clear();
        assert!(!same_contents_metadata(&expected, &actual));
    }
    #[test]
    fn device_and_storage_roots_are_protected_even_without_capacity_metadata() {
        assert!(protected_root(
            Path::new("mtp-root:phone"),
            Path::new("::ThisPC"),
            false
        ));
        assert!(protected_root(
            Path::new("mtp:storage"),
            Path::new("mtp-root:phone"),
            false
        ));
        assert!(protected_root(
            Path::new("mtp:storage"),
            Path::new("mtp:unknown"),
            true
        ));
        assert!(!protected_root(
            Path::new("mtp:file"),
            Path::new("mtp:storage"),
            false
        ));
        assert!(!protected_root(
            Path::new("mtp:folder"),
            Path::new("mtp:storage"),
            false
        ));
    }
    #[test]
    fn incomplete_staging_is_rejected_before_upload_or_source_deletion() {
        let staging = Staging::new().unwrap();
        let folder = staging.folder("fixture").unwrap();
        std::fs::write(folder.join("file"), b"one").unwrap();
        let mut expected = Inventory::from([
            (PathBuf::new(), (true, 0, None)),
            (PathBuf::from("file"), (false, 3, None)),
        ]);
        verify_complete(&folder, &expected).unwrap();
        expected.insert("missing".into(), (false, 1, None));
        assert!(verify_complete(&folder, &expected).is_err());
        expected.remove(Path::new("missing"));
        expected.get_mut(Path::new("file")).unwrap().1 = 4;
        assert!(verify_complete(&folder, &expected).is_err());
    }
    #[test]
    fn only_device_to_device_transfers_need_staging() {
        assert!(needed(&["mtp:source".into()], Path::new("mtp:target")));
        assert!(!needed(&["C:\\source".into()], Path::new("mtp:target")));
        assert!(!needed(&["mtp:source".into()], Path::new("C:\\target")));
        assert!(!needed(&["recycle:source".into()], Path::new("mtp:target")));
    }
    #[test]
    fn manifests_detect_content_and_empty_directory_changes_and_cleanup_is_owned() {
        let root = {
            let staging = Staging::new().unwrap();
            let root = staging.0.clone();
            let a = staging.folder("a").unwrap();
            let b = staging.folder("b").unwrap();
            std::fs::write(a.join("file"), b"one").unwrap();
            std::fs::write(b.join("file"), b"one").unwrap();
            assert_eq!(manifest(&a).unwrap(), manifest(&b).unwrap());
            std::fs::write(b.join("file"), b"two").unwrap();
            assert_ne!(manifest(&a).unwrap(), manifest(&b).unwrap());
            std::fs::write(b.join("file"), b"one").unwrap();
            std::fs::create_dir(b.join("empty")).unwrap();
            assert_ne!(manifest(&a).unwrap(), manifest(&b).unwrap());
            root
        };
        assert!(!root.exists());
    }
}
