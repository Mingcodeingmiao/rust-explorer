# Known limitations / 已知边界

- Windows 10/11 x64 only. Current validation is primarily on Windows 11 with the GNU toolchain; hosted MSVC CI provides additional build coverage. There is no claim of full Explorer parity.
- Undo/redo covers operations successfully recorded by this application. Overwrite, directory merge, permanent deletion or operations without safe receipts create a history barrier. External changes and destination conflicts may prevent replay. See [file history](docs/file-history-062.md).
- USB eject, BitLocker, formatting, network disconnect, volume-label changes and sharing permission changes are delegated to the OS. Hardware/permission combinations have not all been tested. Automated tests do not perform destructive device operations.
- Thumbnails, native menus and rich properties depend on installed Windows providers. Slow or unsupported providers may time out and fall back to an icon/basic information. Helper isolation improves reliability, not security.
- Icons are shared by file type: ordinary folders reuse one cached system folder icon after a single `desktop.ini` probe, while folders that declare their own artwork still resolve their real icon. Executables, shortcuts, drive roots and portable-device roots keep per-item icons, so a folder containing many of those needs one Shell lookup per item.
- Imported resource files are referenced by path and known resource mappings. Arbitrary DLLs do not guarantee a complete appearance from another Windows version. No extracted Windows artwork is shipped.
- DPI, IME, narrow windows, all filter combinations and third-party Shell extensions have not been exhaustively tested. Third-party software compatibility layers are intentionally outside the project's scope.
- Everything is optional. Without a usable installed Everything client/SDK, search falls back to filtering names in the current directory; it does not silently scan the entire disk.
- Release binaries are currently unsigned. Verify the release checksum and source before running.
- Version 0.7.6 includes experimental folder/drive associations (introduced in 0.7.3), disabled by default. They do not replace Win+E, the desktop Shell or software that explicitly starts Explorer. Keep the recovery files and restore the association before moving/removing the executable. See [recovery instructions](docs/default-folder-browser.md).
- Dual-pane browsing, a separate top favorites bar and double-clicking folder whitespace to go up are not implemented in 0.7.6. Quick Access can optionally open folders in new tabs. See the [roadmap](ROADMAP.md) for planned work; open contributor PRs are not automatically included.
- In the 2026-10-21 revision of 0.7.6 (PR #30), owner-backed UI context menus run on an independent STA thread inside the application process, avoiding a new helper and cold extension loading on every click. A hanging/crashing third-party menu extension can still affect that thread/process; menus close through normal menu interaction, not helper force-stop. File operations retain separate cancellable helpers. Stopping them does not roll back completed changes, and external/elevated Windows hosts may need manual closing. Format completion means the dialog launch request completed, not that the drive was formatted.

See the detailed [0.6.2 audit](docs/audit-0.6.2.md) for the distinction between source checks and actual GUI validation. Please report reproducible issues using disposable files and the repository's bug template.

## 本地候选版实机回归（2026-10-18）

- **MTP/WPD 串行传输与设备能力限制**：手机通过 MTP 暴露对象，并非可并行读写的普通磁盘。对仅允许一个活动读写事务的设备/传输路径，应等待当前读取或写入结束后再开始下一项；同一设备内部复制/移动涉及的读写或设备端命令可能受限，Windows Shell 可返回 `0x800700AA`（设备忙）。Xiaomi 14 实测导航、容量、上传、下载和重命名通过，但同设备内直接复制/移动未通过。建议串行传输，必要时先复制到电脑，完成后再传回手机；需要移动时，核对目标完整后再自行删除源文件。不同设备的原生 Copy/Move 能力存在差异，不能概括为所有 MTP 设备均不支持内部操作，也不能仅凭 ERROR_BUSY 断定具体根因。详见 [实测与协议边界说明](docs/mtp-tabs-validation-2026-10-18.md)。
