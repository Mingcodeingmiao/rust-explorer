# Known limitations / 已知边界

- Windows 10/11 x64 only. Current validation is primarily on Windows 11 with the GNU toolchain; hosted MSVC CI provides additional build coverage. There is no claim of full Explorer parity.
- Undo/redo covers operations successfully recorded by this application. Overwrite, directory merge, permanent deletion or operations without safe receipts create a history barrier. External changes and destination conflicts may prevent replay. See [file history](docs/file-history-062.md).
- USB eject, BitLocker, formatting, network disconnect, volume-label changes and sharing permission changes are delegated to the OS. Hardware/permission combinations have not all been tested. Automated tests do not perform destructive device operations.
- Thumbnails, native menus and rich properties depend on installed Windows providers. Slow or unsupported providers may time out and fall back to an icon/basic information. Helper isolation improves reliability, not security.
- Imported resource files are referenced by path and known resource mappings. Arbitrary DLLs do not guarantee a complete appearance from another Windows version. No extracted Windows artwork is shipped.
- DPI, IME, narrow windows, all filter combinations and third-party Shell extensions have not been exhaustively tested. Third-party software compatibility layers are intentionally outside the project's scope.
- Everything is optional. Without a usable installed Everything client/SDK, search falls back to filtering names in the current directory; it does not silently scan the entire disk.
- Release binaries are currently unsigned. Verify the release checksum and source before running.

See the detailed [0.6.2 audit](docs/audit-0.6.2.md) for the distinction between source checks and actual GUI validation. Please report reproducible issues using disposable files and the repository's bug template.

## 本地候选版实机回归（2026-10-18）

- Xiaomi 14 实测：MTP 多级向上导航、容量页、上传、下载和重命名通过。当前同一手机内部直接复制/移动由 Windows Shell 返回 `0x800700AA`（请求的资源正在使用中），重新连接后仍可复现。0.7.2 按预发布版披露此问题，不代表 MTP 全流程已通过；详见 `docs/mtp-tabs-validation-2026-10-18.md`。
