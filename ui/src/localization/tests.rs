use super::*;
use crate::preferences::UiPreferences;
use fluent_syntax::ast::{Entry, Expression, InlineExpression, Pattern, PatternElement};
use std::collections::{BTreeMap, BTreeSet};

struct FixedLocale(Option<Locale>);
impl PlatformLocaleProvider for FixedLocale {
    fn current_locale(&self) -> Option<Locale> {
        self.0
    }
}

pub(crate) fn english() -> Localization {
    Localization::new(Box::new(FixedLocale(Some(Locale::EN_US))))
}

fn variables(pattern: &Pattern<&str>, names: &mut BTreeSet<String>) {
    fn inline(expression: &InlineExpression<&str>, names: &mut BTreeSet<String>) {
        match expression {
            InlineExpression::VariableReference { id } => {
                names.insert(id.name.into());
            }
            InlineExpression::Placeable { expression } => visit(expression, names),
            InlineExpression::FunctionReference { arguments, .. } => {
                for argument in &arguments.positional {
                    inline(argument, names);
                }
                for argument in &arguments.named {
                    inline(&argument.value, names);
                }
            }
            _ => {}
        }
    }
    fn visit(expression: &Expression<&str>, names: &mut BTreeSet<String>) {
        match expression {
            Expression::Inline(expression) => inline(expression, names),
            Expression::Select { selector, variants } => {
                inline(selector, names);
                for variant in variants {
                    variables(&variant.value, names);
                }
            }
        }
    }
    for element in &pattern.elements {
        if let PatternElement::Placeable { expression } = element {
            visit(expression, names);
        }
    }
}

fn contract(source: &str) -> BTreeMap<String, BTreeSet<String>> {
    let parsed = fluent_syntax::parser::parse(source).expect("valid Fluent syntax");
    let mut messages = BTreeMap::new();
    for entry in parsed.body {
        if let Entry::Message(message) = entry {
            let mut args = BTreeSet::new();
            variables(
                &message.value.expect("each UI message has a value"),
                &mut args,
            );
            assert!(
                messages.insert(message.id.name.into(), args).is_none(),
                "duplicate key"
            );
        }
    }
    messages
}

#[test]
fn catalogs_have_identical_keys_arguments_and_every_message_resolves() {
    let canonical = contract(CATALOGS.iter().find(|(id, _)| *id == "en-US").unwrap().1);
    let mut i18n = english();
    for (id, source) in CATALOGS {
        assert!(
            id.parse::<unic_langid::LanguageIdentifier>().is_ok(),
            "invalid BCP-47 catalog ID: {id}"
        );
        assert_eq!(
            contract(source),
            canonical,
            "key/argument contract for {id}"
        );
        let locale = Locale::from_id(id).unwrap();
        i18n.set_preference(LanguagePreference::Locale(locale));
        for (key, names) in &canonical {
            let args: Vec<_> = names
                .iter()
                .map(|name| (name.as_str(), Argument::Number(42.0)))
                .collect();
            // Inspect the selected bundle directly so fallback cannot mask broken translations.
            let mut fluent_args = FluentArgs::new();
            for name in names {
                fluent_args.set(name.as_str(), 42);
            }
            let resolved = i18n.resolve(locale, key, Some(&fluent_args)).expect(key);
            assert!(!resolved.is_empty(), "empty message: {id}:{key}");
            assert_eq!(i18n.format(key, &args), resolved);
        }
        assert!(i18n.reported.lock().unwrap().is_empty());
    }
}

#[test]
fn fallback_reports_missing_keys_and_format_errors_without_panicking() {
    let mut i18n = english();
    i18n.bundles
        .insert(Locale::JA, bundle(Locale::JA, "settings-title = 設定\n"));
    i18n.set_preference(LanguagePreference::Locale(Locale::JA));
    assert_eq!(i18n.text("menu-new-game"), "New Game");
    assert_eq!(i18n.text("unknown-key"), "[unknown-key]");
    assert_eq!(
        i18n.text("settings-display-confirm"),
        "[settings-display-confirm]"
    );
    assert_eq!(
        i18n.format("settings-display-confirm", &[("seconds", 15u64.into())]),
        "Keep these display settings? Reverting in 15s."
    );
    let reports = i18n.reported.lock().unwrap().len();
    assert!(reports >= 3);
    i18n.text("unknown-key");
    assert_eq!(
        i18n.reported.lock().unwrap().len(),
        reports,
        "warn only once"
    );
    i18n.bundles.insert(
        Locale::JA,
        bundle(Locale::JA, "menu-new-game = { $missing }\n"),
    );
    assert_eq!(
        i18n.text("menu-new-game"),
        "New Game",
        "broken translation falls back"
    );
}

#[test]
fn automatic_uses_provider_and_explicit_preference_takes_priority() {
    let mut i18n = Localization::new(Box::new(FixedLocale(Some(Locale::JA))));
    assert_eq!(i18n.locale(), Locale::JA);
    i18n.set_preference(LanguagePreference::Locale(Locale::EN_US));
    assert_eq!(i18n.text("settings-title"), "Settings");
    i18n.set_preference(LanguagePreference::Auto);
    assert_eq!(i18n.text("settings-title"), "設定");
    assert_eq!(
        Localization::new(Box::new(FixedLocale(None))).locale(),
        Locale::EN_US
    );
    for tag in ["ja", "ja-JP", "ja_JP.UTF-8"] {
        assert_eq!(Locale::from_language_tag(tag), Some(Locale::JA));
    }
    assert_eq!(Locale::from_language_tag("en-GB"), Some(Locale::EN_US));
    assert_eq!(Locale::from_language_tag("xx-ZZ"), None);
    assert_eq!(Locale::from_language_tag("japanese"), None);
}

#[test]
fn language_fallback_rejects_ambiguous_catalogs_regardless_of_order() {
    for (ids, unknown_tags) in [
        (["zh-CN", "zh-TW"], &["zh", "zh-HK", "zh-Hant-HK"][..]),
        (["es-ES", "es-419"], &["es", "es-MX", "es-AR"][..]),
        (["pt-PT", "pt-BR"], &["pt", "pt-AO", "pt-MZ"][..]),
    ] {
        let locales = ids.map(Locale);
        for order in [locales, [locales[1], locales[0]]] {
            for locale in locales {
                assert_eq!(
                    Locale::from_language_tag_in(locale.id(), order),
                    Some(locale),
                    "exact match must win for {}",
                    locale.id()
                );
            }
            for tag in unknown_tags {
                assert_eq!(Locale::from_language_tag_in(tag, order), None, "{tag}");
            }
        }
        assert_eq!(
            Locale::from_language_tag_in(unknown_tags[1], [locales[0], Locale::JA]),
            Some(locales[0]),
            "a unique base-language candidate remains usable"
        );
    }
    assert_eq!(
        Locale::from_language_tag_in("ZH_tw.UTF-8", [Locale("zh-CN"), Locale("zh-TW")]),
        Some(Locale("zh-TW"))
    );
}

#[test]
fn new_locales_keep_fluent_directionality_isolation() {
    let mut args = FluentArgs::new();
    args.set("name", "Alice");
    for (locale, expected) in [
        (Locale::EN_US, "Player: Alice"),
        (Locale::JA, "Player: Alice"),
        (Locale("ar"), "Player: \u{2068}Alice\u{2069}"),
        (Locale("he"), "Player: \u{2068}Alice\u{2069}"),
    ] {
        let bundle = bundle(locale, "player = Player: { $name }\n");
        let pattern = bundle.get_message("player").unwrap().value().unwrap();
        let mut errors = vec![];
        assert_eq!(
            bundle.format_pattern(pattern, Some(&args), &mut errors),
            expected
        );
        assert!(errors.is_empty());
    }
}

#[test]
fn all_settings_round_trip_in_one_file_without_overwriting_other_sections() {
    use puzzella_game::{
        keybindings::{KeyAction, KeyBindingsState},
        persistence::autosave::AutosaveSettingsState,
    };

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let display = serde_json::json!({
        "resolution": [1920, 1080], "mode": "Windowed", "max_fps": 144
    });
    std::fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({"display": display})).unwrap(),
    )
    .unwrap();
    let mut preferences = UiPreferences::load(Some(path.clone()));
    let mut autosave = AutosaveSettingsState::load(Some(path.clone()));
    let mut keys = KeyBindingsState::load(Some(path.clone()));
    let mut bindings = keys.current.clone();
    bindings.binding_mut(KeyAction::Performance).primary = None;
    let mut i18n = english();
    for (language, id) in [
        (LanguagePreference::Auto, "auto"),
        (LanguagePreference::Locale(Locale::EN_US), "en-US"),
        (LanguagePreference::Locale(Locale::JA), "ja"),
    ] {
        preferences.set_language(language, &mut i18n);
        assert!(preferences.error.is_none());
        autosave.set_interval(None);
        keys.apply(bindings.clone());
        crate::preferences::wait_for_save(|| {
            preferences.poll_save();
            autosave.poll_save();
            keys.poll_save();
            preferences.is_save_pending() || autosave.is_save_pending() || keys.is_save_pending()
        });
        assert!(autosave.error.is_none());
        assert!(keys.error.is_none());
        assert_eq!(UiPreferences::load(Some(path.clone())).language, language);
        assert_eq!(KeyBindingsState::load(Some(path.clone())).current, bindings);
        assert_eq!(
            AutosaveSettingsState::load(Some(path.clone()))
                .current
                .interval_minutes,
            None
        );
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(json["preferences"]["language"], id);
        assert_eq!(json["display"], display);
        assert_eq!(json.as_object().unwrap().len(), 4);
    }
    std::fs::write(&path, br#"{"preferences":{"language":"unsupported"}}"#).unwrap();
    let preferences = UiPreferences::load(Some(path.clone()));
    assert_eq!(preferences.language, LanguagePreference::Auto);
    assert!(preferences.error.is_some());
    std::fs::write(&path, b"{}").unwrap();
    assert_eq!(
        UiPreferences::load(Some(path.clone())).language,
        LanguagePreference::Auto
    );
    std::fs::write(&path, b"{broken").unwrap();
    assert!(UiPreferences::load(Some(path)).error.is_some());
}

#[test]
fn failed_preference_write_keeps_live_translation_and_existing_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let blocked = dir.path().join("blocked");
    std::fs::write(&blocked, b"keep").unwrap();
    let mut preferences = UiPreferences::load(Some(blocked.join("settings.json")));
    let mut i18n = english();
    preferences.set_language(LanguagePreference::Locale(Locale::JA), &mut i18n);
    crate::preferences::wait_for_save(|| {
        preferences.poll_save();
        preferences.is_save_pending()
    });
    assert!(preferences.error.is_some());
    assert_eq!(i18n.text("settings-title"), "設定");
    assert_eq!(std::fs::read(blocked).unwrap(), b"keep");
}

#[test]
fn embedded_fallback_fonts_cover_catalog_glyphs_and_both_ui_families() {
    use ab_glyph::{Font, FontRef};
    let definitions = crate::fonts::definitions();
    for family in [
        bevy_egui::egui::FontFamily::Proportional,
        bevy_egui::egui::FontFamily::Monospace,
    ] {
        let fonts: Vec<_> = definitions.families[&family]
            .iter()
            .map(|name| {
                let data = &definitions.font_data[name];
                FontRef::try_from_slice_and_index(data.font.as_ref(), data.index)
                    .unwrap_or_else(|_| panic!("invalid embedded font {name}"))
            })
            .collect();
        for (locale, source) in CATALOGS {
            for ch in source
                .chars()
                .filter(|ch| !ch.is_control() && !ch.is_whitespace())
            {
                assert!(
                    fonts.iter().any(|font| font.glyph_id(ch).0 != 0),
                    "missing glyph {ch} for {locale} in {family:?}"
                );
            }
        }
    }
    let ctx = bevy_egui::egui::Context::default();
    crate::theme::prepare(&ctx);
    ctx.run_ui(Default::default(), |ui| {
        ui.ctx().fonts_mut(|fonts| {
            for family in [
                bevy_egui::egui::FontFamily::Proportional,
                bevy_egui::egui::FontFamily::Monospace,
            ] {
                let id = bevy_egui::egui::FontId::new(14.0, family);
                for ch in "設定日本語ABC".chars() {
                    // egui 0.36's has_glyph compares font faces and incorrectly rejects
                    // Latin glyphs sharing the replacement character's face. Check Latin
                    // metrics, and Japanese fallback-face resolution plus metrics.
                    assert!(
                        fonts.glyph_width(&id, ch) > 0.0,
                        "missing glyph metrics {ch}"
                    );
                    if !ch.is_ascii() {
                        assert!(fonts.has_glyph(&id, ch), "missing fallback glyph {ch}");
                    }
                }
            }
        });
    })
    .drop_without_applying_deltas();
}
