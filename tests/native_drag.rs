//! Native OLE contract tests use only newly-created local fixtures.
use std::{
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
unsafe extern "C" {
    fn re_test_file_drop(
        source: *const u16,
        destination: *const u16,
        keys: u32,
        effect: *mut u32,
    ) -> i32;
    fn re_test_drop_handoff(
        source: *const u16,
        destination: *const u16,
        keys: u32,
        effect: *mut u32,
        marked: bool,
        queued: *mut u32,
    ) -> i32;
}
fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}
struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[test]
#[ignore = "requires an interactive Windows Shell desktop; scripts/check.ps1 runs it locally"]
fn shell_data_object_round_trips_through_our_target_for_copy_and_move() {
    // Shell providers can show modal UI or wait indefinitely on a headless
    // runner. Keep that work in a child process with a hard deadline, never a
    // watchdog thread that would leave the blocked COM call alive.
    if let Some(root) = std::env::var_os("RE_OLE_TEST_FIXTURE") {
        return transfer_fixture(Path::new(&root));
    }
    // Referencing the Rust wrapper also links the native bridge into this test.
    let _ = rust_explorer::native_drag::trace as fn(&str);
    let root = std::env::temp_dir().join(format!(
        "re-ole-drop-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let fixture = Fixture(root);
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "shell_data_object_round_trips_through_our_target_for_copy_and_move",
            "--nocapture",
        ])
        .env("RE_OLE_TEST_FIXTURE", &fixture.0)
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "OLE test child failed: {status}");
            break;
        }
        if started.elapsed() > Duration::from_secs(60) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("OLE Shell test exceeded 60 seconds; child process terminated");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn transfer_fixture(root: &Path) {
    let destination = root.join("目标目录");
    std::fs::create_dir(&destination).unwrap();
    for (name, keys, expected_effect) in [("copy-文档.txt", 8, 1), ("move-文档.txt", 4, 2)] {
        let source = root.join(name);
        std::fs::write(&source, b"OLE transfer fixture").unwrap();
        let mut effect = 0;
        let hr = unsafe {
            re_test_file_drop(
                wide(&source).as_ptr(),
                wide(&destination).as_ptr(),
                keys,
                &mut effect,
            )
        };
        assert!(hr >= 0, "OLE transfer failed: 0x{:08X}", hr as u32);
        assert_eq!(
            std::fs::read(destination.join(name)).unwrap(),
            b"OLE transfer fixture"
        );
        assert_eq!(source.exists(), expected_effect == 1);
        assert_eq!(effect, expected_effect);
    }
    // Returning early is safe only for our own source, which leaves the entire
    // move to the helper. A foreign source must see a completed Shell transfer.
    for marked in [false, true] {
        let name = if marked {
            "owned-source.txt"
        } else {
            "foreign-source.txt"
        };
        let source = root.join(name);
        std::fs::write(&source, b"deferred transfer contract").unwrap();
        let mut effect = 0;
        let mut queued = 0;
        let hr = unsafe {
            re_test_drop_handoff(
                wide(&source).as_ptr(),
                wide(&destination).as_ptr(),
                8,
                &mut effect,
                marked,
                &mut queued,
            )
        };
        assert!(hr >= 0, "drop handoff failed: 0x{:08X}", hr as u32);
        assert_eq!(effect, 1);
        assert_eq!(queued, u32::from(marked));
        assert!(source.exists());
        assert_eq!(destination.join(name).exists(), !marked);
        if !marked {
            assert_eq!(
                std::fs::read(destination.join(name)).unwrap(),
                b"deferred transfer contract"
            );
        }
    }
}
