//! Pure drag/drop policy. Keep disk and Shell calls off the pointer-move path.
use std::path::{Component, Path, PathBuf, Prefix};

use crate::shell::TransferMode;

pub fn mode(control: bool, shift: bool, alt: bool) -> TransferMode {
    if alt || (control && shift) {
        TransferMode::Link
    } else if control {
        TransferMode::Copy
    } else if shift {
        TransferMode::Move
    } else {
        TransferMode::Auto
    }
}

pub fn allows(paths: &[PathBuf], target: &Path, _mode: TransferMode) -> bool {
    !paths.is_empty()
        && !paths.iter().any(|source| {
            // Windows path comparisons are case insensitive. This check is
            // lexical only; the Shell remains responsible for reparse points.
            let source = PathBuf::from(source.to_string_lossy().to_lowercase());
            let target = PathBuf::from(target.to_string_lossy().to_lowercase());
            // A drop into the source's own folder is accepted here and turned
            // into "源和目标相同，已忽略" by the drop handler, so the user gets
            // a prompt instead of a silent no-op/rejected drop.
            target == source || target.starts_with(&source)
        })
}

/// True when every source already lives directly in the destination folder.
/// Such a drop is a move no-op or an in-place duplicate, so the UI ignores it
/// instead of asking the Shell to copy/move a file onto itself.
pub fn same_location(paths: &[PathBuf], target: &Path) -> bool {
    fn normalized(path: &Path) -> String {
        path.to_string_lossy()
            .trim_end_matches(|c| c == '\\' || c == '/')
            .to_lowercase()
    }
    if paths.is_empty() {
        return false;
    }
    let target = normalized(target);
    paths.iter().all(|source| {
        source
            .parent()
            .is_some_and(|parent| normalized(parent) == target)
    })
}

/// Immediate visual estimate for normal drive paths. Mounted folders are
/// resolved by the Shell's volume GUID checks when the transfer executes.
pub fn feedback_mode(paths: &[PathBuf], target: &Path, mode: TransferMode) -> TransferMode {
    if mode != TransferMode::Auto {
        return mode;
    }
    let drive = |path: &Path| match path.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                Some(letter.to_ascii_uppercase())
            }
            _ => None,
        },
        _ => None,
    };
    let target_drive = drive(target);
    if target_drive.is_some()
        && !paths.is_empty()
        && paths.iter().all(|path| drive(path) == target_drive)
    {
        TransferMode::Move
    } else {
        TransferMode::Copy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explorer_modifier_precedence() {
        assert_eq!(mode(true, true, false), TransferMode::Link);
        assert_eq!(mode(true, false, false), TransferMode::Copy);
        assert_eq!(mode(false, true, false), TransferMode::Move);
        assert_eq!(mode(false, false, true), TransferMode::Link);
        assert_eq!(mode(false, false, false), TransferMode::Auto);
    }

    #[test]
    fn copy_between_drives_or_unknown_volumes() {
        let paths = vec![PathBuf::from(r"C:\test\a.txt")];
        assert_eq!(
            feedback_mode(&paths, Path::new(r"c:\target"), TransferMode::Auto),
            TransferMode::Move
        );
        assert_eq!(
            feedback_mode(&paths, Path::new(r"D:\target"), TransferMode::Auto),
            TransferMode::Copy
        );
        assert_eq!(
            feedback_mode(&paths, Path::new(r"\\server\share"), TransferMode::Auto),
            TransferMode::Copy
        );
    }

    #[test]
    fn reject_self_and_descendant_but_accept_same_folder_for_prompt() {
        let paths = vec![PathBuf::from(r"C:\test\folder")];
        assert!(!allows(
            &paths,
            Path::new(r"c:\TEST\folder\child"),
            TransferMode::Copy
        ));
        // Same-folder drops are accepted so the UI can show
        // "源和目标相同，已忽略" instead of a silent rejected drop.
        assert!(allows(&paths, Path::new(r"C:\test"), TransferMode::Auto));
        assert!(allows(&paths, Path::new(r"C:\test"), TransferMode::Copy));
        assert!(allows(&paths, Path::new(r"D:\target"), TransferMode::Auto));
    }

    #[test]
    fn same_location_ignores_drops_into_the_containing_folder() {
        let file = vec![PathBuf::from(r"C:\test\a.txt")];
        assert!(same_location(&file, Path::new(r"c:\TEST")));
        assert!(!same_location(&file, Path::new(r"C:\test\sub")));
        assert!(!same_location(&file, Path::new(r"D:\target")));
        let mixed = vec![
            PathBuf::from(r"C:\test\a.txt"),
            PathBuf::from(r"C:\other\b.txt"),
        ];
        assert!(!same_location(&mixed, Path::new(r"C:\test")));
        assert!(!same_location(
            &[PathBuf::from(r"C:\")],
            Path::new(r"C:\test")
        ));
    }
}
