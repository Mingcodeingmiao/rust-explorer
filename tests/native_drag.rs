//! Native OLE contract tests use only newly-created local fixtures.
use std::{
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
};
unsafe extern "C" {
    fn re_test_file_drop(
        source: *const u16,
        destination: *const u16,
        keys: u32,
        effect: *mut u32,
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
fn shell_data_object_round_trips_through_our_target_for_copy_and_move() {
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
    let destination = fixture.0.join("目标目录");
    std::fs::create_dir(&destination).unwrap();
    for (name, keys, expected_effect) in [("copy-文档.txt", 8, 1), ("move-文档.txt", 4, 2)] {
        let source = fixture.0.join(name);
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
}
