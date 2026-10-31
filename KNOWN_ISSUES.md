# Known limitations / 已知边界

- **0.8.0 拖放验证边界**：Rust Explorer 窗口之间的拖放已改为 Drop 立即返回、由可取消 helper 异步复制/移动；与系统资源管理器双向拖动仍需更多人工验证，外部 Explorer 作为目标时其同步 Drop 仍可能等待复制完成。悬停展开、边缘自动滚动、右键跨窗口拖动尚未完善。协议集成测试需要交互式 Windows 桌面，托管 CI 只编译该测试，本地发布检查单独执行并限制为 60 秒。
- 网络共享重连依赖当前 Windows 登录凭据与服务器权限；网络不可达或凭据冲突仍可能失败，不能保证所有 SMB/NAS 环境可用。

> [!WARNING]
> **0.7.7 part1：Windows 原生右键菜单仍有问题，尚未完全修复。** Windows 10 下文件菜单可能仍有深浅色或样式不一致；不同系统和 Shell 扩展的行为仍需继续排查。本轮主要在 Windows 11 验证，不能据此宣称 Windows 10 菜单已经修复。
>
> **Windows native context menus remain a known issue.** Inconsistent styling may persist on Windows 10; broader OS and Shell-extension validation is still needed.

- Windows 10/11 x64 only. Current validation is primarily on Windows 11 with the GNU toolchain; hosted MSVC CI provides additional build coverage. There is no claim of full Explorer parity.
- Undo/redo covers operations successfully recorded by this application. Overwrite, directory merge, permanent deletion or operations without safe receipts create a history barrier. External changes and destination conflicts may prevent replay. See [file history](docs/file-history-062.md).
- USB eject, BitLocker, formatting, network disconnect, volume-label changes and sharing permission changes are delegated to the OS. Hardware/permission combinations have not all been tested. Automated tests do not perform destructive device operations.
- Thumbnails, native menus and rich properties depend on installed Windows providers. Slow or unsupported providers may time out and fall back to an icon/basic information. Helper isolation improves reliability, not security.
- Imported resource files are referenced by path and known resource mappings. Arbitrary DLLs do not guarantee a complete appearance from another Windows version. No extracted Windows artwork is shipped.
- DPI, IME, narrow windows, all filter combinations and third-party Shell extensions have not been exhaustively tested. Third-party software compatibility layers are intentionally outside the project's scope.
- Everything is optional. Without a usable installed Everything client/SDK, search falls back to filtering names in the current directory; it does not silently scan the entire disk.
- Release binaries are currently unsigned. Verify the release checksum and source before running.
- Version 0.7.6 includes experimental folder/drive associations (introduced in 0.7.3), disabled by default. They do not replace Win+E, the desktop Shell or software that explicitly starts Explorer. Keep the recovery files and restore the association before moving/removing the executable. See [recovery instructions](docs/default-folder-browser.md).
- Dual-pane browsing, a separate top favorites bar and folder backgrounds are not implemented in 0.7.7. Blank-area double-click-up and optional reuse of an existing folder tab are implemented. Quick Access can optionally open folders in new tabs. See the [roadmap](ROADMAP.md); other contributor PRs are not automatically included.
- In the 2026-10-21 revision of 0.7.6 (PR #30), owner-backed UI context menus run on an independent STA thread inside the application process, avoiding a new helper and cold extension loading on every click. A hanging/crashing third-party menu extension can still affect that thread/process; menus close through normal menu interaction, not helper force-stop. File operations retain separate cancellable helpers. Stopping them does not roll back completed changes, and external/elevated Windows hosts may need manual closing. Format completion means the dialog launch request completed, not that the drive was formatted.

See the detailed [0.6.2 audit](docs/audit-0.6.2.md) for the distinction between source checks and actual GUI validation. Please report reproducible issues using disposable files and the repository's bug template.

## 本地候选版实机回归（2026-10-18）

- **MTP/WPD 串行传输与设备能力限制**：手机通过 MTP 暴露对象，并非可并行读写的普通磁盘。对仅允许一个活动读写事务的设备/传输路径，应等待当前读取或写入结束后再开始下一项；同一设备内部复制/移动涉及的读写或设备端命令可能受限，Windows Shell 可返回 `0x800700AA`（设备忙）。Xiaomi 14 实测导航、容量、上传、下载和重命名通过，但同设备内直接复制/移动未通过。建议串行传输，必要时先复制到电脑，完成后再传回手机；需要移动时，核对目标完整后再自行删除源文件。不同设备的原生 Copy/Move 能力存在差异，不能概括为所有 MTP 设备均不支持内部操作，也不能仅凭 ERROR_BUSY 断定具体根因。详见 [实测与协议边界说明](docs/mtp-tabs-validation-2026-10-18.md)。
