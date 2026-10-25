# Changelog

## [Unreleased]

### Changed

- Icons, thumbnails and PDF/video/font previews now share one SQLite image cache. The UI receives PNG bytes directly; no per-icon, per-thumbnail or preview PNG/JSON files are created, and legacy image-cache files are removed on upgrade. A deleted database is recreated automatically, with WAL/locking recovery and bounded icon/thumbnail/preview LRU tables.
- Network drive names accept Shell display-name characters such as `\ : ( )`, show the Shell name instead of the UNC path, and are excluded from Everything search. Long drive names use two compact lines and keep the capacity bar (the numeric readout is hidden); clicking an already selected drive starts its rename, like a file.
- Volume-label validation now treats NTFS, exFAT, ReFS and mapped network drives permissively (path characters allowed), while FAT/FAT32 retain the strict 11-unit rule and reject `\ / : * ? " < > |`. The drive rename editor uses 23-character display lines and grows to show them while typing; display newlines are removed before saving.
- Back/Forward navigation now resolves the destination folder's view from saved folder views and then the built-in default, instead of inheriting the view of the folder being left.
- Toolbar Sort/View/More menus are anchored to their buttons, toggle closed when the same button is clicked again, and close when a folder or blank area is clicked. Sort/View align to the button centre and More aligns its right edge to the More button. Menu widths now follow their widest item instead of a fixed 260px. Icon-only toolbar buttons are 32x32 with centred icons, and separators disappear with their adjacent hidden button groups.
- Switching tabs now restores that tab's scroll position, selection and display mode instead of recalculating the incoming tab's view from the outgoing folder.
- On first launch the UI language follows the Windows display language: every Chinese locale uses Simplified Chinese, supported Japanese/Korean locales use their catalogs, and all other languages fall back to English.

## [0.7.7-part1] - 2026-10-24

> **Known issue: Windows native context menus are not fully fixed.** Inconsistent file-menu styling can still occur on Windows 10. See [release notes](docs/v0.7.7-part1.md).

- Add an opt-in blank-space double-click Up setting, fix repeated stationary double-click navigation, and move tab close buttons slightly right.
- Store thumbnails in a bounded, deduplicated SQLite cache with a configurable local directory; preserve background-only I/O and codec isolation.
- Bridge native sharing submenu commands to Windows sharing workflows; preserve in-process STA/owner menu boundaries.
- Root sharing menus offer Advanced Sharing; child folders offer Remove Access and Specific Users. Improve Windows 10 theme API version selection and reapply menu themes after extension initialization, without claiming all menu issues resolved.
- Gate Everything by filesystem and provide filesystem fallback; show search paths with independent search column layout. See [implementation and validation notes](docs/ui-cache-search.md).

- Add embedded Simplified Chinese, English, Japanese and Korean interface packs. Default to English; manually select and persist English, Chinese, Japanese or Korean in the General options language dropdown, applying changes immediately. Localize application controls, options, status text and application-added menus while preserving filenames, paths, command IDs and native Shell behavior. See [language pack documentation](resources/locales/README.md).

Notable changes are recorded here. Versions before 1.0 may change behavior and configuration formats.

## [0.7.7] - 2026-10-23

Merged PR #33. Release review keeps desktop.ini and disk icon-cache probes off GUI layout through bounded background work; corrects extended UNC/already-prefixed long directory patterns. 129 core tests and one shortcut integration test passed (one interactive deletion test skipped). Performance figures below are contributor measurements for the stated workload, not universal guarantees. The 0.7.6 revision's in-process STA context-menu invariant remains unchanged.
See [complete release notes](docs/v0.7.7.md) and the [roadmap](ROADMAP.md). Collects the work that started with the context-menu latency fix after 0.7.6.

### Fixed

- Retain the 0.7.6 revision context-menu invariant: owner-backed UI menus use an in-process independent STA; file operations retain cancellable helpers. PR #33 routes This PC rename to inline editing without changing menu dispatch or apartment/message-pump ownership. Latency remains extension-dependent.
- File and tab hover cards are owned by the application window, never topmost, clamped inside it, and dismissed when the window is inactive, the pointer leaves, or a menu opens. Tab cards sit flush with the window top and flip below it at the screen edge; both scale with zoom × system DPI.
- An already selected item answers a right click anywhere inside its own cell; an unselected one still requires an icon, name or metadata hit, so right-clicking the empty part of a row keeps the background menu and does not change the selection.
- Address bar: the displayed names of Quick Access and the Recycle Bin (`快速访问`, `此电脑\回收站`, folders inside the bin) navigate back to their location instead of being resolved against the disk, including after This PC was renamed. Recycled folders show only their own name in the address bar and title.

### Changed

- Folder previews are faster to reuse: a folder keeps one cache key (invalidated by the folder's own mtime instead of a new key every 30 seconds, which only shortcuts still use), and a cached entry is read directly without a helper round trip. Slow folder and portable-device previews get 3 s / 4 s instead of 800 ms, so a large composition is not killed halfway and redone, and the viewport settle delay dropped from 100 ms to 60 ms.
- Directory enumeration and natural sorting are optimized (contributor benchmark): one `FindNextFileW` pass returns name, attributes, size, times and the reparse tag, so there is no per-item path stat and no second probe per junction (`C:\Windows\WinSxS`, 16306 entries: 325 ms → about 32 ms). Sorting builds one natural-order byte key per entry (digit runs become marker + length + digits) and applies an index permutation: 57 ms → about 30 ms. Leading zeros no longer change a name's rank (`a1` equals `a01`, `a1` before `a1b`).
- Internet shortcuts (`.url`) are icon-only again in the medium/large/content views: those views asked the thumbnail pipeline for a `.url` preview, where the Shell can only return a generic page. The real artwork comes from the icon pipeline (STA apartment + icon handle), so `.url` no longer creates thumbnail requests; `.lnk` still previews its target.
- Internet shortcuts and `.ico`/`.cur` files show their real icon: those icons come from a COM handler and only as an icon handle (their image-list index points at a different, generic icon). The icon helper now asks for an STA apartment (it leaves an existing apartment untouched) and draws the returned handle for `.url`/`.ico`/`.cur`, falling back to the handle for a `.lnk` whose icon is not in the image list. Icons moved to a fresh `icons-v2` cache directory so old, wrong bitmaps are not reused.
- Ordinary folders share one type icon while customized folders retain per-path artwork. desktop.ini classification runs on a bounded background queue; the GUI initially uses a generic icon and refreshes when classification completes. Drive/UNC roots retain their own icons. Icon batches grow from 8 to 32 with scaled deadlines.
- Background icon workers check disk cache before asking a helper, while GUI layout only consults memory and queues requests. Stable drive keys and bounded per-folder prefetch improve reuse without filesystem probes in layout.
- Opening a folder generates the previews for the whole folder in the background instead of only for the rows that are scrolled into view: the visible rows keep priority, the rest follows in listing order, and scrolling no longer cancels queued work, so scrolling back reveals finished previews. One folder queues at most 8000 previewable items, of which at most 3000 are folders (folder previews are composed by the Shell at 7–125 ms each; `C:\Windows\System32` has 253 previewable entries which take about 10 s, and `C:\Windows\WinSxS` holds 16k folders); the in-memory images stay under 32 MiB and an evicted preview is read back from the disk cache without touching the Shell. Thumbnail helpers are also started before the first preview is requested: 120 images measured 105 ms to fill the first screen (was 137 ms) and 805 ms for all of them, about 6.7 ms per image on four workers.
- A file whose thumbnail provider reports "no thumbnail" is remembered next to its cache index (`.miss`, valid for 7 days), so reopening the folder or restarting the application does not ask the Shell again; timeouts and helper failures are not remembered.
- Preview generation is parallel: one worker per core beyond the two kept free (cores − 2), capped at four because measurements (2/4/8 workers: 13.7/10.2/9.9 ms for 12 folder previews) show no gain beyond that while every worker owns a helper process. Portable-device items are handed out one at a time because a phone rejects concurrent thumbnail sessions.
- Portable-device (MTP) photos and folders are cached like local items, so re-opening a window or refreshing the listing no longer fetches them from the device again.
- The last portable-device enumeration is kept for a minute and a `--shell-devices` timeout keeps the previous list instead of dropping it, so a connected phone no longer disappears and reappears when another window opens.
- Thumbnail cache is content-addressed: every PNG is stored once under the SHA-256 of its bytes (`<sha256>.png`), so identical thumbnails (stock folder art, the same picture copied around, generic file icons) share a single file. A small per-item index keeps the key-to-image mapping plus the link/photo decorations, the previous per-item PNG cache is removed on upgrade, and cache clearing covers both layouts.
- The application icon no longer carries transparent margins: the master artwork is hand-cropped, and the export scales the whole square canvas straight to every size (no trimming, recentring, padding or 94% inset), so the artwork looks larger in the taskbar and window. The checked-in PNGs and `rust-explorer.ico` were regenerated and the GIMP source `resources/app-icon/rust-explorer-master.xcf` is now part of the repository.
- Folder options gained an **Advanced** tab that collects cache clearing, the experimental default-folder association, the high-performance toggle and the UI resource source, so the General and View tabs only keep everyday settings. The "清空缓存" entry in the `…` selection menu was removed; the Advanced tab is the only place to clear caches now.
- Folder options also gained a **Details** tab that holds the tab and sorting preferences (Quick Access in a new tab, new tabs open This PC, Shift opens a new tab, folders first) plus a new option, on by default: opening a folder in a new tab switches to an existing tab on the same folder instead of duplicating it.
- Double-clicking the empty list area goes up one level, like the ↑ button.
- Renaming a drive or "This PC" edits the name in place (the drive card or its navigation row) instead of opening a separate dialog; Enter applies it. A drive label is written directly when allowed and otherwise through a `runas` restart of the application (UAC), while "This PC" is renamed in a helper process through the Shell namespace, so the navigation label refreshes immediately. Read-only drives are rejected up front.
- The three "open at startup" choices on the General tab are laid out in one row instead of three.
- Back/forward/up are greyed out when they cannot navigate. Going back, forward or up restores the scroll position and re-selects the item that was being worked on (↑ selects the folder just left), per tab, like Explorer; if that item is gone, only the scroll position is restored. Clicking an ancestor in the address bar selects the folder you came from.
- The mouse side buttons (XBUTTON1/XBUTTON2) navigate back and forward, same as the toolbar buttons and Alt+←/→.

### Maintenance

- A warning comment, a `debug_assert` in `execute_controlled`, the regression test `context_menus_keep_an_owner_and_stay_out_of_the_one_shot_helper` and a `CONTRIBUTING.md` rule keep menu requests out of the one-shot helper.
- Local checks: 127 core tests and one shortcut integration test passed, one interactive test skipped; all-target check passed. Preview workers are capped at four (each owns an isolated helper process).

## [0.7.6] - 2026-10-20

See [complete release notes](docs/v0.7.6.md) and the [roadmap](ROADMAP.md). Includes merged PR #27 and subsequent fixes.

### 2026-10-21 revision (same version, PR #30)

- Restore owner-backed context menus to an in-process independent STA thread. The original package spawned a fresh helper for every right-click, repeatedly cold-loading Shell extensions and causing latency.
- Retain cancellable helpers for file operations; add prominent source/contributor/agent warnings against merging the menu and operation paths again.
- Correct file/tab tooltip ownership, placement and scaling; remove global topmost behavior over other applications.
- Replace the Windows ZIP and checksum, and retarget v0.7.6 to the revised source. Original release source remains available at b070f8c917805936e1ec40959d3622ddd3cba2de; the previous archive SHA-256 is recorded in the release notes.

### Fixed

- Recycle Bin column resize/order/visibility settings and sorting, separately persisted from normal folders (#23).
- Unicode deletion-confirmation names instead of internal PIDL identifiers (#22); clearer dark selection contrast (#21).
- Startup drive capacity and icon loading: publish local volumes before portable-device enumeration, separate navigation icon work, isolate slow drive requests, and move desktop.ini reads off the GUI thread (#28).
- Quick Access enumeration no longer depends on Shell-menu pin probes.
- Owner-backed native menus use an independent in-process STA; file operations use isolated helpers. Initialization failures are propagated rather than ignoring RPC_E_CHANGED_MODE on shared executor threads.
- Native new-folder commands use asynchronous creation and inline rename. Format-dialog launch validates drive roots and provides a valid parent; it no longer waits for the external host or mistakes closing it for a failed format.
- Right-click item targeting follows visible icons, text and metadata rather than empty row/card space.
- Address/rename caret selection and text menus; light text selection and inactive title/tab colors; maximized-window drag restoration.
- Modified-date sorting keeps files before folders in both directions.
- Shortcut icon/thumbnail fallback, Internet shortcuts, link badges and updated cache versions.

### Added and optimized

- System-operation history with elapsed time, outcome, graceful stop and per-operation force-stop.
- Clear generated caches from the selection menu; bounded icon/image caches and improved child-process/Win32 handle cleanup.
- Windows x64 package, notices, documentation and SHA-256 checksum. Local checks: 103 core tests and one shortcut integration test passed, one interactive test skipped; all-target check and release build passed. Validation limits are documented in the release notes.

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
