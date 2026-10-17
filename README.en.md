# Rust Explorer

A native Windows file manager built with **Rust and GPUI**, without a browser or WebView2 runtime.

[中文](README.md) · [Downloads](https://github.com/Mingcodeingmiao/rust-explorer/releases) · [Changelog](CHANGELOG.md) · [Known limitations](KNOWN_ISSUES.md)

Version **0.6.4** is an early-stage release, primarily validated on Windows 11 x64. It does not claim full Explorer parity. Back up important files before trying new builds.

## Features

- Tabs, breadcrumbs, navigation history and keyboard shortcuts.
- Eight views, adjustable columns, sorting, grouping, filters and marquee selection.
- System Quick Access synchronization, resizable panes and drive capacity views.
- Native clipboard/Shell operations, drag/drop confirmation and receipt-based undo/redo.
- Actual Windows Shell menus for files, folders, drives and backgrounds.
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
