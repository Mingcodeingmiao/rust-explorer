# Third-party notices

Rust Explorer's own code is distributed under [Apache-2.0](LICENSE). Dependencies and optional components retain their own licenses; this document does not relicense them.

| Component | Use | License / notice |
| --- | --- | --- |
| [GPUI / Zed](https://github.com/zed-industries/zed) | Native desktop UI; `src/input.rs` adapts the GPUI input example | Apache-2.0; upstream attribution is retained in [NOTICE](NOTICE) and the source file |
| [rust-pinyin](https://github.com/mozillazg/rust-pinyin), version 0.11.0 | Name filtering by pinyin | MIT; [license text](resources/pinyin-LICENSE.txt) |
| [Files Community](https://github.com/files-community/Files), commit `6b721e819e4006670c5926325450fef9157aa6b4` | Native Shell selection bridge (`native/shell_bridge/OpenInFolder.*`), adapted for default-folder requests | MIT; source copyright retained and [full license](resources/files-MIT.txt) |
| [Everything SDK](https://www.voidtools.com/support/everything/sdk/) | Optional Everything search integration | David Carpenter's permissive SDK license; [license text](resources/Everything-SDK-License.txt). The SDK DLL is downloaded separately and is not committed to this repository. |
| Windows Shell and Windows SDK | Native file operations, icons, menus, previews and OS bindings | System components are not licensed by this project or bundled as artwork. Runtime access uses the user's installed Windows resources. |
| Other Rust dependencies | See the exact dependency graph in `Cargo.lock` | Each crate's published license applies. Use `cargo metadata --locked --format-version 1` to inspect dependency metadata and review license files before redistribution. |

Binary packaging includes this file, `NOTICE`, `LICENSE`, and the pinyin notice. Include the Everything SDK notice when redistributing `Everything64.dll`. GNU builds may depend on GCC runtime libraries; their upstream terms, including applicable runtime exceptions, must also be reviewed when redistributing them. The automated Windows release workflow uses MSVC and does not package GCC DLLs.

For GNU bundles, `libgcc_s_seh-1.dll` is covered by GPL-3.0-or-later with the GCC Runtime Library Exception 3.1; `libwinpthread-1.dll` uses the mingw-w64 winpthreads permissive license. License texts are retained in `resources/licenses/` and included by the packager. Upstream sources are available from [GCC](https://gcc.gnu.org/git.html) and [mingw-w64](https://www.mingw-w64.org/downloads/). No claim is made that the application's Apache-2.0 license replaces these terms.

The original application icon is AI-assisted artwork created for this project, not extracted from Windows. Its source and generation notes are in [resources/app-icon](resources/app-icon/README.md). The project's Apache-2.0 license grants rights only to the extent held by its contributors and does not grant rights to third-party trademarks.

## Microsoft Fluent UI System Icons

Toolbar fallback SVGs and the embedded application fallback icon use unmodified Microsoft Fluent UI System Icons artwork (MIT). Source commit and per-file URLs are recorded in `resources/fluent-icons/SOURCES.json`; full license is in `resources/fluent-icons/LICENSE`. Application PNG/ICO variants are rasterizations of the upstream Document Folder SVG.

Installed Windows artwork and system icon fonts are referenced at runtime only and are not redistributed.
