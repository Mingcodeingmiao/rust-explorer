# Changelog

Notable changes are recorded here. Versions before 1.0 may change behavior and configuration formats.

## [Unreleased]

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
