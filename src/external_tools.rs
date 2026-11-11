//! Optional application integration. Discover off the UI thread and pass paths
//! as separate arguments so file names are never interpreted as shell commands.
use std::path::{Path, PathBuf};

pub fn bandizip() -> Option<PathBuf> {
    [
        windows_registry::CURRENT_USER,
        windows_registry::LOCAL_MACHINE,
    ]
    .into_iter()
    .find_map(|root| {
        let key = root
            .open(r"Software\Microsoft\Windows\CurrentVersion\App Paths\Bandizip.exe")
            .ok()?;
        let path = PathBuf::from(key.get_string("").ok()?);
        path.is_absolute().then_some(path).filter(|p| p.is_file())
    })
}

pub fn launch_bandizip(executable: &Path, paths: &[PathBuf], extract: bool) -> anyhow::Result<()> {
    anyhow::ensure!(
        !paths.is_empty() && paths.iter().all(|p| p.is_absolute()),
        "No files selected"
    );
    let mut command = std::process::Command::new(executable);
    if extract {
        anyhow::ensure!(paths.len() == 1 && paths[0].is_file(), "Select one archive");
        command.args(["x", "-target:dlg", "-"]);
    } else {
        // cd takes an output archive before its input files. Only open the
        // dialog: the user chooses the format, destination and confirmation.
        let name = paths[0]
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("Select a file or folder, not a drive root"))?;
        let mut archive_name = name.to_os_string();
        archive_name.push(".zip");
        let parent = paths[0]
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Missing parent folder"))?;
        command.args(["cd", "-"]).arg(parent.join(archive_name));
    }
    command.args(paths);
    // On Windows, dropping Child closes our handles without terminating the app.
    // No waiting thread is needed for an independently owned application window.
    drop(command.spawn()?);
    Ok(())
}
