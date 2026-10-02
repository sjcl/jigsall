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
fn preferences_round_trip_use_internal_ids_and_do_not_touch_display_settings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ui-settings.json");
    let display = dir.path().join("settings.json");
    std::fs::write(&display, b"existing display preferences").unwrap();
    let mut preferences = UiPreferences::load(Some(path.clone()));
    let mut i18n = english();
    for (language, id) in [
        (LanguagePreference::Auto, "auto"),
        (LanguagePreference::Locale(Locale::EN_US), "en-US"),
        (LanguagePreference::Locale(Locale::JA), "ja"),
    ] {
        preferences.set_language(language, &mut i18n);
        assert!(preferences.error.is_none());
        assert_eq!(UiPreferences::load(Some(path.clone())).language, language);
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(json["language"], id);
    }
    assert_eq!(
        std::fs::read(display).unwrap(),
        b"existing display preferences"
    );
    std::fs::write(&path, b"{\"language\":\"unsupported\"}").unwrap();
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
    let mut preferences = UiPreferences::load(Some(blocked.join("ui-settings.json")));
    let mut i18n = english();
    preferences.set_language(LanguagePreference::Locale(Locale::JA), &mut i18n);
    assert!(preferences.error.is_some());
    assert_eq!(i18n.text("settings-title"), "設定");
    assert_eq!(std::fs::read(blocked).unwrap(), b"keep");
}

#[test]
fn embedded_font_covers_catalog_glyphs_and_both_ui_families() {
    use ab_glyph::{Font, FontRef};
    let font = FontRef::try_from_slice(include_bytes!("../../fonts/MPLUS1p-Regular.ttf")).unwrap();
    for (_, source) in CATALOGS {
        for ch in source
            .chars()
            .filter(|ch| !ch.is_control() && !ch.is_whitespace())
        {
            assert_ne!(font.glyph_id(ch).0, 0, "missing glyph {ch}");
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
