use super::*;
use crate::localization::{LanguagePreference, Locale};

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
