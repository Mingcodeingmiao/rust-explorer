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
            .file("native/shell_bridge/bridge.cpp");
        if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu") {
            bridge.cpp_link_stdlib_static(true);
        }
        bridge.compile("re_shell_bridge");
        for library in ["ole32", "oleaut32", "shell32", "uuid", "propsys"] {
            println!("cargo:rustc-link-lib={library}");
        }
        embed_resource::compile("resources/app.rc", embed_resource::NONE)
            .manifest_required()
            .expect("Failed to embed the Windows application manifest");
    }
}
