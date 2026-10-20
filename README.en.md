# Rust Explorer

A native Windows file manager built with **Rust and GPUI**, without a browser or WebView2 runtime.

[中文](README.md) · [Downloads](https://github.com/Mingcodeingmiao/rust-explorer/releases) · [Changelog](CHANGELOG.md) · [Known limitations](KNOWN_ISSUES.md)

Version **0.7.6** fixes shortcut icons in the file view: `.url` shortcuts get an icon preview again (the Shell is asked for the shortcut's own icon instead of resolving a web target), shortcut previews are composed with the Windows link arrow, and the self-drawn link badge follows Explorer's proportions (27.5 % extra-large, 25 % large, 15.6 % medium; other sizes unchanged). See the [release notes](docs/v0.7.6.md) and [roadmap](ROADMAP.md). 0.7.5 fixed the context menu and how new windows are opened ([notes](docs/v0.7.5.md)); 0.7.4 fixed the Shell/COM and Win32 defects found in an audit ([notes](docs/v0.7.4.md)). Folder associations from 0.7.3 are disabled unless explicitly enabled in General options; a 120-second trial guard, backups and recovery are provided. Read the [scope and recovery guide](docs/default-folder-browser.md) before enabling them. Win+E, the desktop Shell and programs that explicitly start Explorer are not replaced. MTP/WPD paths may still require serialized I/O; same-device copy/move depends on device capabilities and can return ERROR_BUSY. Use the PC as an intermediate location when needed. Full Explorer parity is not claimed.

Latest published download: [Rust Explorer 0.7.5](https://github.com/Mingcodeingmiao/rust-explorer/releases/tag/v0.7.5). The 0.7.6 changes described here are on the main branch and are not packaged yet; unzip the Windows x64 archive and verify `SHA256SUMS.txt`.

## Features

- Tabs, breadcrumbs, navigation history and keyboard shortcuts.
- Eight views, adjustable columns, sorting, grouping, filters and marquee selection; an optional fixed-pool high-performance mode falls back to the traditional implementation for unsupported views.
- System Quick Access synchronization, resizable panes and drive capacity views. Portable/MTP devices support full breadcrumbs, tree expansion, storage-capacity pages, Shell copy/paste and context menus.
- Native clipboard/Shell operations, drag/drop confirmation and receipt-based undo/redo. MTP items are permanently deleted; removable drives are checked for a usable Recycle Bin first.
- Actual Windows Shell menus for files, folders, drives and backgrounds, including paste in the background menu and open-in-tab/window for folders.
- Asynchronous image, video-frame, PDF, font and folder thumbnails; shortcut arrows and translucent hidden items.
- Light/dark/system themes, zoom, high-DPI icons and optional Everything integration.

Providers run in background queues or helpers with timeouts and fallbacks. Helper isolation is not a security sandbox. Without usable Everything integration, search explicitly falls back to current-folder name filtering.

## Build and run

Requires Windows 10/11 x64, stable Rust with the 2024 edition, Visual Studio C++ Build Tools and Windows SDK (`fxc.exe` for release shaders). MSVC is recommended.

```powershell
git clone https://github.com/Mingcodeingmiao/rust-explorer.git
cd rust-explorer
# Optional: download the official Everything SDK
.\scripts\bootstrap.ps1
.\scripts\build.ps1 -Release
.\dist\RustExplorer-windows-x64-native\rust-explorer.exe 'C:\'
```

For MSYS2 UCRT64, install `stable-x86_64-pc-windows-gnu` and use `scripts/build.ps1 -Gnu -Release`; the default GCC location is `C:\msys64\ucrt64\bin`. Review GNU runtime licenses before redistribution.

Download and extract the entire ZIP from Releases. Builds are currently unsigned; compare the published checksum. Settings and caches live in `%LOCALAPPDATA%\RustExplorer`. Everything requires an installed usable client and is optional.

## Development

Run `scripts/check.ps1` for formatting, core library tests and desktop compilation checks. `scripts/package.ps1` packages the most recent build without building or testing it. Use disposable files for operation tests.

See [CONTRIBUTING.md](CONTRIBUTING.md), [SECURITY.md](SECURITY.md), [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) and the [0.6.2 validation report](docs/audit-0.6.2.md). Some technical notes are currently in Chinese.

## License

[Apache-2.0](LICENSE), with attribution in [NOTICE](NOTICE) and [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). Windows icons load from the user's system at runtime; extracted system artwork and DLLs are not committed. The app icon is original AI-assisted artwork with retained creation notes. This independent project is not affiliated with Microsoft or the Rust Foundation.
