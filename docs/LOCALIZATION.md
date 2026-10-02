# UI localization

`puzzella-ui` owns the Fluent catalogs, language preferences, platform locale
resolution and embedded Japanese font. `game`, `core`, `puzzle` and networking
do not depend on Fluent. Localization runs only while drawing UI or presenting
notifications, never in piece, geometry, GPU, picking, snapping or replication paths.

## Catalogs and API

`ui/i18n/en-US.ftl` is canonical; `ui/i18n/ja.ftl` supplies Japanese. `ui/build.rs`
discovers `.ftl` files and embeds them at compile time. Resources are parsed once
into concurrent bundles in the Bevy `Localization` resource, so switching locale
does not reparse files. UI code uses `text(key)` or `format(key, arguments)`; the
`Argument` wrapper supports text and numbers without exposing Fluent to screens.
Complete sentences, including variable placement, belong in the catalogs.

Add a BCP-47-named `.ftl` file with the canonical keys and variables plus
`language-native-name`. It is automatically registered in the language selector
and OS resolver; no enum or screen changes are necessary. Also provide any font
coverage the new language needs. The tests enforce matching key and argument sets,
valid syntax, successful direct resolution and embedded font coverage.

The migrated screens include title, new-game setup, in-game HUD/player list,
pause menu, generation progress/errors, completion, settings, save/load/delete,
thumbnail placeholders/tooltips and performance statistics. Widget IDs stay stable
across locale changes. Dynamic user titles, filenames, player names and diagnostic
system names are preserved.

## Preferences and platform boundary

Settings → Language offers Automatic, English and 日本語. A selection updates
the live resource immediately and the next frame uses the new translations.
Language preferences are saved independently of the display preview/confirmation
to `puzzella/ui-settings.json` in `directories::BaseDirs::data_local_dir()`
(Windows: `%LOCALAPPDATA%\puzzella\ui-settings.json`). This keeps the existing
`settings.json` display format intact and prevents display reversion from undoing
language selection. Writes use a synced temporary file and atomic replacement.

```json
{ "language": "auto" }
```

The other stored IDs are `en-US` and `ja`. Missing preferences default to Auto.
Unreadable, malformed or unsupported preferences select Auto and show a localized
error. A failed write keeps the chosen language usable in the running application
and shows the failure; the previous persisted file remains intact.

Explicit preferences take priority. Auto asks the injected
`PlatformLocaleProvider`; the default `OsLocaleProvider` uses the lightweight
`sys-locale` crate. Locale resolution tries an exact catalog ID, then its base
language (`ja-JP` → `ja`, `en-GB` → `en-US`). Unsupported/unavailable OS locales
fall back to `en-US`.

A future Steam adapter can implement `PlatformLocaleProvider`, convert Steam
language codes to `Locale` in that adapter, and insert
`Localization::new(Box::new(provider))` before `GameUiPlugin`. An adapter may
delegate to `OsLocaleProvider` when Steam has no supported locale. No Steamworks
dependency or Steam language code is used by localization core.

## Fallback and outcomes

Resolution tries the selected locale, then `en-US`. Missing keys, missing message
values and Fluent formatting errors produce warning logs once per diagnostic.
If neither locale resolves the message, a visible `[key]` marker is returned;
runtime catalog problems do not panic or crash release builds. Catalog syntax
and duplicate-key errors are also logged on initialization and caught by tests.

`DisplaySettingsError`, `DisplayValidationError` and `DisplaySettingsNotice`
replace display UI strings. `PersistenceError` and `PersistenceNotice` retain
semantic save outcomes, including worker/definition failures, import errors and
typed `SaveError` values. `SaveTitleError` identifies title validation failures.
`GenerationError` retains the category of generation failures. `ui/src/messages.rs`
maps these values to localized messages at display time, so existing errors and
notices follow language changes too. An unnamed local player carries `None`;
the UI supplies the localized default player name. OS/library errors and corruption/checkpoint/
renderer diagnostics are technical details inserted as `$reason`; diagnostic
`Display` implementations are not used as the complete user-facing message.

## Fonts and intentionally invariant text

`ui/fonts/MPLUS1p-Regular.ttf` is embedded after egui's default Latin fonts in
both proportional and monospace fallback chains. Neither the font nor catalogs
require a runtime file or depend on the launch directory. `ui/fonts/OFL.txt`
contains the SIL Open Font License 1.1 and copyright notice; include it in release
distributions. Provenance and SHA-256 are in `ui/fonts/README.md`.

There are no remaining English UI sentences in `ui/src` outside tests. Invariant
text includes `Puzzella`, version numbers, `PNG / JPEG / WebP / BMP`, file extension
filters, resolution dimensions, numeric preset values and unit symbols (`FPS`,
`px`, `x`). Save timestamps currently retain the neutral `YYYY-MM-DD HH:MM` format.
Technical `$reason` details may be English. Internal widget/texture/system IDs and
logs are not translated.

## Verification

```powershell
cargo fmt --all --check
cargo clippy --workspace --locked --all-targets -- -D warnings
cargo test --workspace --locked
cargo build --workspace --locked
cargo test --locked -p puzzella-ui native_settings_ui_probe -- --ignored --nocapture --test-threads=1
```

Catalog tests cover every canonical key, syntax, matching variables, formatting,
missing translations/keys/arguments, warning deduplication, provider priority,
preference round trips, malformed preferences, failed writes and glyph coverage.
Real egui widget tests select Japanese, check persistence and the next frame's
labels, and check English/Japanese settings at small window sizes. The native
probe renders screenshots without reading or writing the user's preferences.

Verified on Windows on 2026-10-03: formatting, Clippy with all targets and warnings
denied, the ordinary workspace build, and workspace tests (320 passed, zero failed;
25 normally ignored), plus the Japanese delete-confirmation regression test. The
native probe was run separately and passed, producing English settings plus
Japanese title/settings/setup screenshots. Japanese display
confirmation was visually inspected at 640 × 360. Both catalogs have 151 keys;
Japanese settings and load actions also pass viewport checks at 320 × 360.
