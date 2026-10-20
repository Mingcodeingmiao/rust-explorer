# Changelog

Notable changes are recorded here. Versions before 1.0 may change behavior and configuration formats.

## [0.7.6] - 2026-10-20

See [complete release notes](docs/v0.7.6.md) and the [roadmap](ROADMAP.md).

Shortcut icons in the file view.

### Fixed

- `.url` shortcuts get an icon preview again. A web shortcut has no file target, so resolving it produced `文件不支持缩略图`; the Shell is now asked for the shortcut file's own icon (the site icon when Windows has one).
- Shortcut previews drawn from the Shell's icon fallback now carry the link arrow in the bottom-left. `IShellItemImageFactory` never adds the overlay, so shortcuts looked like ordinary files; the badge is composed into the bitmap with Windows' up-left arrow.
- Items without a Shell thumbnail fall back to their Shell icon, which is what Explorer shows. Previously only the video whitelist asked the Shell, so a shortcut to an application (for example `Telegram.lnk`) produced no image at all.
- The address bar's leading icon always uses the system glyph U+E7F4 (Segoe Fluent Icons on Windows 11, Segoe MDL2 Assets on Windows 10); Windows 11 was drawing the embedded SVG instead.
- The self-drawn link badge follows Explorer's proportions: 27.5 % of the icon at extra-large, 25 % at large and 15.6 % at medium icons. Tiles, list and details keep their previous overlay size.
- Thumbnail cache versions were raised (`native-v6-r4`, `thumbnails-v5`) so existing caches cannot serve the old, badge-less images.

## [0.7.5] - 2026-10-20

See [complete release notes](docs/v0.7.5.md) and the [roadmap](ROADMAP.md).

This release fixes the context menu and how new windows are opened, and trims menu/window startup costs.

### Fixed

- Context menus were cancelled by Windows 10-20 ms after they appeared while shown by a helper process, which also took activation away from the application (its taskbar button flipped to the inactive look). Menus are now built and shown **inside the application process**, owned by the application window; that owner is a child window, so the main window keeps its DWM active frame (shadow) and the application stays the active process.
- The file hover card no longer interferes with the context menu. It is a separate click-through native window (`WS_EX_TRANSPARENT`, `HTTRANSPARENT`, `MA_NOACTIVATE`) placed below-right of the pointer like Explorer, dismissed when a menu opens, and sized from `rem_size x monitor DPI scale`.
- Right-click jitter is measured strictly between the press and the release (12 px), so moving the pointer while the menu is still being built no longer suppresses it. Menus raised from button-down still wait for the release, plus a 30 ms settle.
- Menu results are returned without a trailing newline. The application compares exact strings (`command:paste`, `rename`, `properties`, `refresh`, `command:quick-pinned`), so the newline had silently disabled paste, open-in-new-tab, open-in-new-window, delete and the Quick Access refresh.
- The Shell's `delete` and `paste` verbs now run through the application's own implementations instead of blocking the menu task on their confirmation/progress dialogs (the UI used to stay on "正在处理系统操作").
- Verbs with their own modal dialog (`format`, `print`, `share`, `burn`) are delegated to a detached helper, and in-process `InvokeCommand` no longer sets `SEE_MASK_NOASYNC`, so no menu action can block the application.
- The drive "Format" command is not a registry verb (`HKCR\Drive\shell\format` does not exist), so `ShellExecuteExW("format")` led nowhere after the UAC prompt. It now relaunches this executable elevated (`runas` + `--format-drive <letter>`), which calls `SHFormatDrive`.
- A folder is now listed the way the settings resolve it: the view saved for that folder, otherwise the built-in default (This PC / portable devices use tiles, other folders use details). A window opened for a drive no longer keeps the default folder's presentation (tiles + type grouping), because the window now re-resolves the view when its tab is replaced.
- Quick Access: a folder that is already pinned shows "从快速访问取消固定" (routed to `unpinfromhome`), the list refreshes immediately after a pin or unpin, and the background snapshot resolves per-entry pin state within a bounded probe budget.
- The "re-show a menu that vanished within 120 ms" retry was removed: it existed to paper over the cross-process activation race.

### Performance

- Menu build cost: about 250-300 ms cold and 40-80 ms warm, with a `GetCommandString` cache that removes two redundant passes per menu.
- Drives and Quick Access are cached per process (20 s TTL each): a window that opens later reuses the snapshot instead of enumerating drives and spawning another Quick Access helper, so the sidebar appears immediately.
- Only the target folder is passed to a new window.

### Build

- The Shell-link/Internet-shortcut test now runs as an integration test (`tests/shortcut_targets.rs`). It initializes a COM apartment, and the unit tests share one thread: uninitializing there made the next test crash with `STATUS_ACCESS_VIOLATION`. `scripts/check.ps1` runs that test in its own process.

## [0.7.4] - 2026-10-19

See [complete release notes](docs/v0.7.4.md) and the [roadmap](ROADMAP.md).

Maintenance release for Shell/COM and Win32 usage defects found in an audit.

### Fixed

- Junction and directory-symlink thumbnails were always rejected as "file changed": the identity check followed reparse points while the entry itself was built from `symlink_metadata`.
- Drive-root drop containment: a canonicalized root (`\\?\C:\`) carries a trailing separator, so the string-prefix check let a drive root be dropped into its own subfolder. Paths are compared by component now.
- Portable-device (MTP) property sheets were opened without holding an STA and without a message pump, so they could close immediately or never appear.
- Volume queries now install `SEM_FAILCRITICALERRORS` before the first call and restore the previous mode; `DRIVE_UNKNOWN`/`DRIVE_NO_ROOT_DIR` are reported as unknown/disconnected instead of "local disk".
- `SHGetKnownFolderPath` results are released even when the string conversion fails.
- Multi-selection context menus no longer reject a shared parent folder that differs only in letter case.
- Path completion and icon reads use the same `symlink_metadata` source as the entry, and Shell calls run with an initialized apartment and a message pump.

### Build

- `cargo test` runs on Windows again: the embedded application manifest (ComCtl32 v6) is now linked into every target, so the `TaskDialogIndirect` import that gpui pulls in resolves in test binaries. Core suite: 96 passed, 1 ignored.

## [0.7.3] - 2026-10-19

See [complete release notes](docs/v0.7.3.md) and the [roadmap](ROADMAP.md).

### Added

- Opt-in experimental folder/drive associations with backups, a 120-second trial guard and independent recovery; Shell file reveal and separate tabs for different targets in the same folder.
- Internal Recycle Bin browsing, original-location/deletion-date columns, native properties and an empty-bin action in the main toolbar.
- Startup-page and new-tab destination preferences, Quick Access folder opening in new tabs, and a version row in Folder Options.
- Automatic restoration of valid one-hour folder-size caches, selected-size totals and a large-icon details fallback for unsupported previews.

### Fixed

- Address input selection/caret sequencing, background, long-path scrolling and prompt restoration after clearing without entering a new path.
- Overlapping third-level submenu hitboxes and menu height limits in small windows.
- System icon mappings, official Fluent SVG fallbacks, tab close alignment, Quick Access pin spacing and application icon padding.
- Native deletion progress and post-operation refresh, preservation of scroll position on refresh, and immediate inline rename for newly created folders.

### Maintenance and plans

- Serial core IPC tests for consistent Windows message timing; documented validation limits and recovery scope.
- Existing contributor and dependency PRs remain unmerged. Future work prioritizes file drag/drop improvements, performance and bug fixes.

## 0.7.2 integration and validation

PR #11 at `f929129` is integrated with the maintainer's changes. MTP parent navigation and light active-tab contrast are fixed. Formatting, 76 core tests and desktop compilation passed; device navigation, upload/download and rename were tested on Xiaomi 14. MTP/WPD transfers are documented as a device/protocol-path capability boundary: some paths require serialized I/O, and same-device copy/move may return ERROR_BUSY. See [integration validation](docs/pr-11-integration-072.md) and [device validation](docs/mtp-tabs-validation-2026-10-18.md). Default-browser association remains deferred; the content first column uses 160 logical pixels (280 physical pixels at 175% DPI). Version 0.7.2 is published as a pre-release.

## [0.7.2] - 2026-10-18

完整说明见 [docs/v0.7.2.md](docs/v0.7.2.md)。

### Fixed

- The traditional Shell file menu now writes CF_HDROP directly for Copy/Cut, so copied files reliably reach the Windows clipboard instead of leaving it empty after the short-lived menu helper exits. Internal MTP clipboard state is invalidated when the system clipboard sequence changes.
- Right-clicking any part of a file row now opens the file menu; previously right-clicking outside the filename could clear the selection and open the folder background menu.
- The traditional background menu places Paste directly below Refresh.
- Folders with a custom `desktop.ini` class or icon (for example `C:\Windows\Offline Web Pages`) are resolved through the Shell instead of the generic folder icon.

## [0.7.1] - 2026-10-18

完整说明见 [docs/v0.7.1.md](docs/v0.7.1.md)。

### Added

- Folder Options: opt-in high-performance list mode with a fixed visible-row widget pool (components stay in place while their data changes), plus Shift-open-in-new-tab and a risk-gated per-user default-file-browser association.
- Portable-device browsing: MTP/Android phones and cameras appear under This PC, are enumerated through the Shell namespace in a killable helper, and support opening, PIDL-based icons, copy to/from the device, delete and drag/drop. MTP breadcrumbs now show the full device/folder chain, and child folders use cached stock icons instead of one slow Shell image request per folder.
- The address autocomplete dropdown is positioned from the actual address-field bounds, uses an explicit one-to-nine-row height, sits directly under the field and lists only item icons and names. Clicking the right side of the address bar now places the caret at the end; the navigation bar uses the active tab fill color and joins it without a gap.
- The classic Shell folder menu now offers "open in new tab" and "open in new window"; Ctrl+double-click opens every selected file and every selected folder in a new window, while Ctrl+single-click still toggles selection. Shift+double-click opens a folder in a new tab when enabled in Folder Options.
- High-performance wheel direction now matches normal scrolling; MTP breadcrumbs show the full device/folder chain, and MTP child folders use cached stock icons instead of per-folder Shell image requests.
- Portable-device roots open in Tiles with a drive icon, capacity bar and "available / total" text; their Details view uses Name, Type, Free space and Total space columns. MTP folder/file context menus now expose open, new tab/window, cut, copy, delete, rename and properties as applicable.
- The classic folder menu places the new-tab/new-window commands directly below Open; the app-owned file menu also has "open in new window". Unsupported high-performance views (large/medium/extra-large icons, content) transparently use the traditional implementation instead of being disabled.
- Caption buttons are flush with the window top, and the tab strip joins the command bar without a gap. Extra-large link overlays are 200% of their previous size; small/list/detail overlays share the 8 px size.
- Portable-device items always use the app-owned context menu (even when the full native menu option is on), avoiding incomplete MTP Shell menus.
- With full paths enabled, tabs show the complete folder path, shorten to `..\<folder>` when it does not fit, clip the remaining overflow, and show the complete path in a native tooltip placed fully above the tab strip (never over the tab's clickable height).
- Editing the address bar for an MTP folder now starts with the complete breadcrumb path (`此电脑\device\storage\folder`) instead of only the current folder name.
- Background polling and icon/thumbnail notifications pause while the left mouse button is held, so they cannot force repaints during a native window move or resize.
- MTP address submission resolves the displayed `此电脑\device\storage\folder` path back to its PIDL, and also accepts `mtp:` / `mtp://` addresses. Portable devices can now be expanded in the navigation tree and enumerate their storage/folders through the Shell helper.
- The application icon's transparent rim was trimmed per icon size and the artwork was rescaled to fill the taskbar slot.
- The classic background menu now contains a Paste command (enabled only when a file clipboard exists); MTP storage-root pages reject paste while MTP folders support copy/paste, including MTP-to-MTP operations through `IFileOperation`.
- Deletion checks whether the target volume has a usable recycle bin. MTP, network shares and removable drives without Recycle Bin are permanently deleted after the irreversible confirmation; Shift+clicking Delete and Shift+Delete force permanent deletion.
- Address bar: hollow computer icon, trailing chevron and a wide edit-mode click target, with path autocomplete (keyboard-selectable, nine-row dropdown) and blur-to-exit.
- Content view: responsive two-column layout with type under the name, image type/resolution in the middle, and date/size on the right; the first column is capped at 280 logical pixels.
- Context-aware search placeholder (`在System32中搜索` / `在本地磁盘(C:)中搜索`) and a native Advanced Sharing entry for drive roots.

### Fixed

- Extra-large/tile rendering no longer panics on zero-width break opportunities in CJK labels.
- Text preview detects UTF-16 and GB18030/GB2312 instead of showing mojibake.
- Select all/none/invert menu icons are distinct; drive tile availability text matches the drive name color.
- Drive tile icons are 17% larger and capacity bars are 20% taller/3% wider.
- Tab close glyph is two-thirds size, tabs are 15% wider, and the refresh control is 20% smaller. Tab text and height are 90% of their previous values; the navigation row matches the active tab fill and its vertical padding is halved.
- Idle theme polling is throttled to two seconds and only runs in system-theme mode, reducing background registry traffic and CPU wakeups while moving the window.
- The top window border is reachable again for resizing; Quick Access pins clear the overlay scrollbar.

## [0.6.4] - 2026-10-17

### Fixed

- This PC tiles now retain a fixed 250×55 logical-pixel size while the window changes width, corresponding to approximately 438×96 physical pixels at 175% DPI and 100% application zoom.
- Detail-view filter menus use the same 26 logical-pixel item height as compact file rows, with matching popup-height calculations and single-line truncated labels.
- Shortcut resolution tests compare canonical target paths so Windows runner 8.3 TEMP aliases do not cause false failures.

## [0.6.3] - 2026-10-17

### Fixed

- Folder Options search descriptions now wrap inside the dialog instead of overflowing horizontally.
- Options content and viewport allow width shrinking; resource descriptions, setting labels and footer text wrap, and action rows wrap in narrow layouts.
- The dialog fits the available window with outer spacing and retains vertical scrolling for long content.
- Folder compositions no longer shrink when the Shell returns a low-resolution cached thumbnail for a jumbo request; regular image thumbnails retain their native-size behavior.
- Breadcrumb overflow hides the minimum leading ancestors one by one and keeps the fitting suffix visible; its menu lists only the hidden ancestors.

## Repository preparation

- Prepare the public repository: contribution and security policies, issue/PR templates, Windows CI, packaging workflow and third-party attribution.

## [0.6.2] - 2026-10-17

### Added

- Receipt-based file undo/redo with identity and conflict checks, bounded history and large-request stdin transport.
- Native Shell background menus preserving installed entries and New templates; dedicated volume-label editor.
- System Quick Access pin/unpin synchronization; independently adjustable This PC detail columns and sorting.

### Fixed

- Native menu cancellation/focus behavior, view-switch layout reuse, shifted-digit view shortcuts and End navigation.
- Four-line name ellipsis, group selection state and video associated-player badges.
- Quick Access enumeration delays and network volume read-only metadata.

See [release details](docs/v0.6.2.md) and [known limitations](KNOWN_ISSUES.md).

## Earlier versions

- [0.6.1](docs/v0.6.1.md): native menus, shortcut navigation, rename layout and four-line names.
- [0.6.0](docs/v0.6.0.md): marquee selection, column filters, grouping, font previews and drive views.
- [0.5.0](docs/v0.5.0.md): dynamic navigation, file hints and drag/drop behavior.
- [0.4.4](docs/v0.4.4.md): asynchronous native thumbnails.
- [0.4.3](docs/v0.4.3.md): window controls and title-bar behavior.
- [0.4.2](docs/v0.4.2.md): original multi-resolution application icon.
