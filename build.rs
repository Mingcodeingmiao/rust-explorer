fn main() {
    println!("cargo:rerun-if-changed=resources/app.rc");
    println!("cargo:rerun-if-changed=resources/app.manifest");
    println!("cargo:rerun-if-changed=resources/app-icon/rust-explorer.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        for file in [
            "OpenInFolder.cpp",
            "OpenInFolder.h",
            "ComSupport.h",
            "bridge.cpp",
            "drag_drop.cpp",
        ] {
            println!("cargo:rerun-if-changed=native/shell_bridge/{file}");
        }
        let mut bridge = cc::Build::new();
        bridge
            .cpp(true)
            .std("c++17")
            .define("UNICODE", None)
            .define("_UNICODE", None)
            .file("native/shell_bridge/OpenInFolder.cpp")
            .file("native/shell_bridge/bridge.cpp")
            .file("native/shell_bridge/drag_drop.cpp");
        if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu") {
            bridge.cpp_link_stdlib_static(true);
        }
        bridge.compile("re_shell_bridge");
        for library in [
            "ole32", "oleaut32", "shell32", "uuid", "propsys", "comctl32",
        ] {
            println!("cargo:rustc-link-lib={library}");
        }
        // Link the resource archive (manifest + version info + icon) into every
        // target exactly once. `compile` would emit `rustc-link-arg-bins` only, so
        // the unit-test binary of the lib target loaded ComCtl32 v5 and died with
        // STATUS_ENTRYPOINT_NOT_FOUND on the `TaskDialogIndirect` import that gpui
        // pulls in; combining `compile` with an extra all-target link argument
        // duplicates the manifest and makes the linker report a .rsrc merge failure.
        embed_resource::compile_for_everything("resources/app.rc", embed_resource::NONE)
            .manifest_required()
            .expect("Failed to embed the Windows application manifest");
    }
}
