# Installed Explorer SVG resources

The app reads the installed Windows artwork at runtime; none of these SVGs are
copied into this repository, bundled in the release, or written into its cache.

On the inspected machine (2026-10-16), Explorer PID 17052 loaded
`Windows.UI.FileExplorer.dll` and
`C:\Windows\SystemApps\MicrosoftWindows.Client.FileExp_cw5n1h2txyewy\FileExplorerExtensions.dll`.
The latter contains UTF-16 references to `windows.cut.svg`, `windows.copy.svg`,
`windows.newitem.svg`, `SortBy.svg`, and `View.svg` (literal byte checks). This
corroborates their relationship with Explorer, although it does not trace every
individual draw call.

The actual modern artwork lives in:

`%WINDIR%\SystemApps\MicrosoftWindows.Client.FileExp_cw5n1h2txyewy\FileExplorerExtensions\Assets\images\contrast-standard\theme-dark`

and the corresponding `theme-light` directory. The dark theme has 134 SVG files; additional
high-contrast variants exist in sibling `contrast-black` / `contrast-white`.
These are real multicolor SVG vector paths, not emoji or fonts. The toolbar
scissors, for example, use gray strokes and the system blue accent.

| UI action | Installed file |
| --- | --- |
| New / new folder | windows.newitem.svg / newfolder.svg |
| Cut / copy / paste | windows.cut.svg / windows.copy.svg / windows.paste.svg |
| Rename / share / delete | windows.rename.svg / Windows.ModernShare.svg / windows.ribbondelete.svg |
| Sort / view | SortBy.svg / View.svg |
| Refresh / preview | windows.refresh.svg / windows.previewpane.svg |
| Close / pin | windows.close.svg / windows.pintohome.svg |
| Options / properties | windows.folderoptions.svg / windows.properties.svg |
| View layouts | windows.iconsize.*.svg |

No semantically exact standalone back, forward, up, search or bare plus SVG was
found among this package's 134 dark-theme assets. Those use the app's uniform
canvas stroke icons; undo/redo or a new-window symbol are not substituted. Close
is a blue X in the installed resource; use its alpha mask tinted with ordinary
text color for a neutral tab close button and add background only on hover.

REA PE inventory evidence:

- `Windows.UI.FileExplorer.dll`: `ev_d0dea36d4a4d926eeba23404fa787756c5fe0fc42c0d39dc1c5f70fe9ebb4e0c`, SHA-256 `833fc67da020e958c75e71cec881cb270d289bfa2ba82893b3f34b7bb10274d5`.
- `FileExplorerExtensions.dll`: `ev_aa370d476b7c8ed0dc8e6bb2c83d70003bc64a38184dd61dd0135ed72b42030b`, SHA-256 `72a83823e376e4643c843fb0875f912375ea30382e2bdaffb43ca563a1020f37`.

These inventories contain only MUI/version resources and no icon groups or SVG
resources; the modern SVG assets are loose files in the package. The module list,
directory inventory and literal-reference checks above were separate read-only
PowerShell observations, not facts supplied by the PE parser. No binary session
was opened and no Windows resource images were exported.

`src/fluent_icons.rs` discovers `%WINDIR%` through `GetWindowsDirectoryW`, caches
only existing paths once, and returns a path for the requested theme. Call
`initialize()` before opening app windows. `img(path)` uses GPUI's existing
background image loader, including its native resvg branch, and retains original
colors. No browser or WebView is involved. For single-color alpha-mask rendering,
install `fluent_icons::Assets` using `Application::with_assets` then use
`svg().path(path.to_string_lossy().into_owned()).text_color(...)`.

The package identity, asset filenames and installation layout are private Windows
implementation details rather than stable public API. Missing resources return
`None` and must retain a matching drawn fallback. Actual paint-time corruption
or Windows servicing changes remain possible and are not ruled out by existence
checks.
