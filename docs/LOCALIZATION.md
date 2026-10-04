# UI localization

`jigsall-ui` owns the Fluent catalogs, language preferences, platform locale
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
Language preferences are saved in the `preferences` section of
`jigsall/settings.json` in `directories::BaseDirs::data_local_dir()`
(Windows: `%LOCALAPPDATA%\jigsall\settings.json`). The shared helper updates only
this section, so display preview/confirmation and reversion cannot undo language
selection. Writes use a synced temporary file and atomic replacement. Older
standalone settings files and formats are not imported.

```json
{ "preferences": { "language": "auto" } }
```

The other stored IDs are `en-US` and `ja`. Missing preferences default to Auto.
Unreadable, malformed or unsupported preferences select Auto and show a localized
error. A failed write keeps the chosen language usable in the running application
and shows the failure; the previous persisted file remains intact.

Explicit preferences take priority. Auto asks the injected
`PlatformLocaleProvider`; the default `OsLocaleProvider` uses the lightweight
`sys-locale` crate. Locale resolution tries an exact catalog ID, then a base-language
match only when exactly one catalog has that language (`ja-JP` → `ja`,
`en-GB` → `en-US`). With both `zh-CN` and `zh-TW`, an unmatched `zh-HK` or
`zh-Hant-HK` is unresolved regardless of catalog order. The same rule applies to
regional variants of Spanish and Portuguese. Script/region aliases must be mapped
explicitly in a platform adapter or a reviewed alias table; catalog order never
chooses a variant. Unsupported, ambiguous or unavailable OS locales ultimately
fall back to `en-US` if the provider finds no supported locale.

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

`ui/src/fonts.rs` defines the ordered `EMBEDDED_FALLBACK_FONTS` registry and builds
the font definitions shared by the theme and coverage tests. Its current entry,
`ui/fonts/MPLUS1p-Regular.ttf`, follows egui's default Latin fonts in both
proportional and monospace chains. Add another embedded entry when a language
needs more glyphs, with its license and provenance in `ui/fonts`. Tests check the
union of all fonts actually registered for each family, so no single font must
cover every catalog. Neither fonts nor catalogs require runtime files or depend
on the launch directory. `ui/fonts/OFL.txt` contains the current font's SIL Open
Font License 1.1 and copyright notice; include it in release distributions.
Provenance and SHA-256 are in `ui/fonts/README.md`.

Fluent directionality isolation is disabled only for the shipped `en-US` and `ja`
bundles to preserve their existing HUD formatting. New locales keep Fluent's
default isolation around interpolated values. This alone does not establish RTL
support: Arabic/Hebrew additions also require validating bidi ordering, shaping,
mixed-direction values and layout in egui, together with fonts and this policy.

There are no remaining English UI sentences in `ui/src` outside tests. Invariant
text includes `Jigsall`, version numbers, `PNG / JPEG / WebP / BMP`, file extension
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
cargo test --locked -p jigsall-ui native_settings_ui_probe -- --ignored --nocapture --test-threads=1
```

Catalog tests cover every canonical key, syntax, matching variables, formatting,
missing translations/keys/arguments, warning deduplication, provider priority,
ambiguous regional catalogs in either order, new-locale isolation, preference
round trips, malformed preferences, failed writes and glyph coverage across each
family's registered fonts.
Real egui widget tests select Japanese, check persistence and the next frame's
labels, and check English/Japanese settings at small window sizes. The native
probe renders screenshots without reading or writing the user's preferences.

Verified on Windows on 2026-10-03: formatting, Clippy with all targets and warnings
denied, the ordinary workspace build, and workspace tests (342 passed, zero failed;
26 normally ignored). The native probe was run separately and passed, producing
English settings plus Japanese title/settings/setup screenshots. Japanese display
confirmation was visually inspected at 640 × 360. Both catalogs have 151 keys;
Japanese settings and load actions also pass viewport checks at 320 × 360.
