//! Shell-link and Internet-shortcut resolution.
//!
//! This lives in an integration test on purpose: it initializes a COM apartment, and
//! the unit tests run on a single shared thread — tearing the apartment down there
//! made whichever test followed crash with `STATUS_ACCESS_VIOLATION`. An integration
//! test gets its own process, so its COM state cannot leak into the unit suite.
use anyhow::Result;
use rust_explorer::native_preview::shortcut_target;
use windows::Win32::{
    System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx},
    UI::Shell::{IShellLinkW, ShellLink},
};
use windows::core::{Interface, PCWSTR};

struct Com;

impl Com {
    fn new() -> Result<Self> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok()? };
        Ok(Self)
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        unsafe { windows::Win32::System::Com::CoUninitialize() }
    }
}

#[test]
fn shell_link_and_local_url_resolve_without_executing_target() -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::System::Com::IPersistFile;
    let _com = Com::new()?;
    let root = std::env::temp_dir().join(format!(
        "rust-explorer-link-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    std::fs::create_dir_all(&root)?;
    let target = root.join("photo with spaces.png");
    std::fs::write(&target, b"target is never executed")?;
    let destination = root.join("photo.lnk");
    let link: IShellLinkW = unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)? };
    let target_wide: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination_wide: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    unsafe {
        link.SetPath(PCWSTR(target_wide.as_ptr()))?;
    }
    let persist: IPersistFile = link.cast()?;
    unsafe {
        persist.Save(PCWSTR(destination_wide.as_ptr()), true)?;
    }
    // Windows may expand the runner's 8.3 TEMP alias (RUNNER~1) when resolving a
    // Shell link. Compare actual targets, not path spelling.
    assert_eq!(
        std::fs::canonicalize(shortcut_target(&destination)?)?,
        std::fs::canonicalize(&target)?
    );
    let internet = root.join("photo.url");
    let uri = format!(
        "[InternetShortcut]\r\nURL=file:///{}\r\n",
        target
            .to_string_lossy()
            .replace('\\', "/")
            .replace(' ', "%20")
    );
    std::fs::write(&internet, uri)?;
    assert_eq!(
        std::fs::canonicalize(shortcut_target(&internet)?)?,
        std::fs::canonicalize(&target)?
    );
    std::fs::write(
        &internet,
        "[InternetShortcut]\r\nURL=https://example.com/image.png\r\n",
    )?;
    assert!(shortcut_target(&internet).is_err());
    std::fs::remove_file(destination)?;
    std::fs::remove_file(internet)?;
    std::fs::remove_file(target)?;
    std::fs::remove_dir(root)?;
    Ok(())
}
