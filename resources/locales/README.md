# Interface language packs

RustExplorer embeds `zh-CN.json`, `en-US.json`, `ja-JP.json` and `ko-KR.json` in the executable. A standalone executable therefore works without downloading language files. The distributed copies document the translations; editing those copies does not change the embedded interface.

The default interface language is English, including existing configurations without a language preference. Select English, 简体中文, 日本語 or 한국어 from Folder Options → General. Apply or OK changes the interface immediately and saves the preference for subsequent launches. Windows display language does not override this preference.

Application controls, options, status messages and application-added menu commands use these packs. Windows-owned dialogs, native Shell extensions and provider metadata retain the language supplied by Windows or that provider. User filenames, paths, disk labels and command identifiers are not translated.

## Maintaining translations

- Keys are stable source messages. Keep the same keys in all four JSON files.
- Use `i18n::text` for static interface labels and `trf!` for formatted messages. Translate metadata only at the display boundary; keep filter identifiers and stored values unchanged.
- Preserve every placeholder and format specifier, including `{error:#}` and `{elapsed:.1}`. Positional arguments may be reordered using `{0}`, `{1}`, etc. Interpolated paths are rendered verbatim.
- Run `scripts/check.ps1`; catalog tests verify keys and placeholders, language fallback and argument integrity. Format the local macro crate with `cargo fmt --manifest-path crates/i18n-macros/Cargo.toml` as well.
- For isolated interface tests, launch a fresh process with `RUST_EXPLORER_LANGUAGE` set to `zh-CN`, `en-US`, `ja-JP` or `ko-KR`. This diagnostic override does not change Windows settings. Normal launches should leave it unset.

The local `rust-explorer-i18n-macros` crate formats arguments once before inserting them into a translated template. It uses the already present `syn`, `quote` and `proc-macro2` build dependencies; no translation service or network request is used.
