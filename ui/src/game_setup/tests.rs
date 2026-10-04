use super::*;
use crate::localization::{LanguagePreference, Locale};

#[test]
fn seed_input_preserves_all_u64_digits_during_display_and_paste() {
    let ctx = egui::Context::default();
    let render = |seed: &mut u64, events: Vec<egui::Event>, focus: bool| {
        ctx.run_ui(
            egui::RawInput {
                events,
                ..default()
            },
            |ui| {
                let response = seed_input(ui, seed);
                if focus {
                    response.request_focus();
                } else {
                    response.surrender_focus();
                }
            },
        )
    };
    let select_all = egui::Event::Key {
        key: egui::Key::A,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::COMMAND,
    };
    let mut seed = 9_007_199_254_740_993;
    for value in [seed, u64::MAX, 0, 42] {
        seed = value;
        let output = render(&mut seed, vec![], true);
        assert!(labels(&output).contains(&value.to_string()));
        assert_eq!(seed, value, "display must not round the seed");
        output.drop_without_applying_deltas();
        let output = render(&mut seed, vec![select_all.clone(), egui::Event::Copy], true);
        assert!(output.platform_output.commands.iter().any(|command| {
            matches!(command, egui::OutputCommand::CopyText(text) if text == &value.to_string())
        }));
        output.drop_without_applying_deltas();
    }
    for value in [u64::MAX, 9_007_199_254_740_993, 0] {
        render(
            &mut seed,
            vec![select_all.clone(), egui::Event::Paste(value.to_string())],
            true,
        )
        .drop_without_applying_deltas();
        assert_eq!(seed, value, "pasted seed must remain exact");
    }
    for invalid in ["", "-1", "18446744073709551616", "not a seed"] {
        render(
            &mut seed,
            vec![select_all.clone(), egui::Event::Paste(invalid.into())],
            true,
        )
        .drop_without_applying_deltas();
        assert_eq!(seed, 0, "invalid input must not replace the seed");
    }
    render(&mut seed, vec![], false).drop_without_applying_deltas();
    render(&mut seed, vec![], false).drop_without_applying_deltas();
    let output = render(&mut seed, vec![], false);
    assert!(labels(&output).contains(&"0".into()));
    output.drop_without_applying_deltas();
}

#[test]
fn rotation_toggle_changes_new_game_config_in_both_locales() {
    let mut i18n = crate::localization::tests::english();
    for locale in [Locale::EN_US, Locale::JA] {
        i18n.set_preference(LanguagePreference::Locale(locale));
        let ctx = egui::Context::default();
        let mut config = PuzzleConfig::default();
        assert!(!config.rotation_enabled);
        let render = |config: &mut PuzzleConfig, events| {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(440., 800.),
                    )),
                    events,
                    ..default()
                },
                |ui| {
                    theme::prepare(ui.ctx());
                    ui.set_width(360.);
                    piece_section(ui, config, &i18n);
                },
            )
        };
        render(&mut config, vec![]).drop_without_applying_deltas();
        let output = render(&mut config, vec![]);
        let pos = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == i18n.text("setup-rotation") => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .expect("localized rotation checkbox");
        output.drop_without_applying_deltas();
        for enabled in [true, false] {
            for pressed in [true, false] {
                render(
                    &mut config,
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: default(),
                        },
                    ],
                )
                .drop_without_applying_deltas();
            }
            assert_eq!(config.rotation_enabled, enabled);
        }
    }
}

#[test]
fn failed_image_shows_reason_and_retry_instead_of_preparing() {
    let mut i18n = crate::localization::tests::english();
    let config = PuzzleConfig {
        image_path: "broken.png".into(),
        ..default()
    };
    let error = ImageLoadError {
        virtual_key: config.image_path.clone(),
        reason: "Image is too large".into(),
    };
    let registry = ExternalFileRegistry::default();
    for locale in [Locale::EN_US, Locale::JA] {
        i18n.set_preference(LanguagePreference::Locale(locale));
        for height in [500.0, 800.0] {
            let ctx = egui::Context::default();
            let render = |error| {
                ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(400.0, height),
                        )),
                        ..default()
                    },
                    |ui| {
                        theme::prepare(ui.ctx());
                        ui.set_width(320.0);
                        image_section(ui, &config, None, error, None, &registry, &i18n);
                    },
                )
            };
            render(None).drop_without_applying_deltas();
            let output = render(None);
            assert!(labels(&output).contains(&i18n.text("setup-preparing-image")));
            output.drop_without_applying_deltas();

            let output = render(Some(&error));
            let labels = labels(&output);
            assert!(labels.contains(&i18n.format(
                "setup-image-load-failed",
                &[("reason", error.reason.as_str().into())],
            )));
            assert!(labels.contains(&i18n.text("setup-image-load-retry")));
            assert!(labels.contains(&i18n.text("setup-change-image")));
            assert!(!labels.contains(&i18n.text("setup-preparing-image")));
            output.drop_without_applying_deltas();
        }
    }
}

fn labels(output: &egui::FullOutput) -> Vec<String> {
    output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text.galley.job.text.clone()),
            _ => None,
        })
        .collect()
}
