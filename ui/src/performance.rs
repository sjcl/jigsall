use crate::localization::Localization;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use jigsall_game::resources::{PerformanceDebugLevel, PerformanceMonitor, PieceDataStore};

pub fn draw_performance_overlay(
    mut contexts: EguiContexts,
    perf: Res<PerformanceMonitor>,
    i18n: Res<Localization>,
    store: Res<PieceDataStore>,
) {
    if perf.debug_level == PerformanceDebugLevel::Off {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    paint_overlay(ctx, &perf, store.len(), &i18n);
}

fn paint_overlay(
    ctx: &egui::Context,
    perf: &PerformanceMonitor,
    piece_count: usize,
    i18n: &Localization,
) {
    if perf.debug_level == PerformanceDebugLevel::Off {
        return;
    }
    // Paint without widgets or an Area so the overlay never captures game input.
    let viewport = ctx.viewport_rect();
    let painter = ctx
        .layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("performance_overlay"),
        ))
        .with_clip_rect(viewport.shrink(8.0));
    let font_size = if perf.debug_level == PerformanceDebugLevel::Fps {
        16.0
    } else {
        13.0
    };
    let galley = painter.layout_no_wrap(
        overlay_text(perf, piece_count, i18n),
        egui::FontId::monospace(font_size),
        egui::Color32::WHITE,
    );
    let padding = egui::vec2(10.0, 8.0);
    let size = galley.size() + padding * 2.0;
    // Keep clear of the progress/player HUD at the top of the game viewport.
    let position = egui::pos2(viewport.left() + 12.0, viewport.top() + 40.0);
    painter.rect_filled(
        egui::Rect::from_min_size(position, size),
        4.0,
        egui::Color32::from_black_alpha(200),
    );
    painter.galley(position + padding, galley, egui::Color32::WHITE);
}

fn overlay_text(perf: &PerformanceMonitor, piece_count: usize, i18n: &Localization) -> String {
    let fps = format!("{:.1}", perf.get_fps());
    if perf.debug_level != PerformanceDebugLevel::Verbose {
        return i18n.format("performance-fps", &[("fps", fps.as_str().into())]);
    }
    let min_frame = perf.frame_times.iter().min().copied().unwrap_or_default();
    let max_frame = perf.frame_times.iter().max().copied().unwrap_or_default();
    let mut text = i18n.format(
        "performance-summary",
        &[
            ("fps", fps.as_str().into()),
            (
                "average",
                format!("{:.2}", perf.get_frame_time_ms()).as_str().into(),
            ),
            (
                "min",
                format!("{:.2}", min_frame.as_secs_f32() * 1000.0)
                    .as_str()
                    .into(),
            ),
            (
                "max",
                format!("{:.2}", max_frame.as_secs_f32() * 1000.0)
                    .as_str()
                    .into(),
            ),
            ("pieces", piece_count.into()),
            ("samples", perf.frame_count.into()),
            ("window", perf.frame_times.len().into()),
        ],
    );
    let mut systems: Vec<_> = perf.system_timings.iter().collect();
    systems.sort_by(|(name_a, a), (name_b, b)| {
        b.average_duration()
            .cmp(&a.average_duration())
            .then_with(|| name_a.cmp(name_b))
    });
    for (name, timing) in systems {
        text.push('\n');
        text.push_str(
            &i18n.format(
                "performance-system",
                &[
                    ("name", name.as_str().into()),
                    (
                        "last",
                        format!("{:.2}", timing.last_duration.as_secs_f32() * 1000.0)
                            .as_str()
                            .into(),
                    ),
                    (
                        "average",
                        format!("{:.2}", timing.average_duration().as_secs_f32() * 1000.0)
                            .as_str()
                            .into(),
                    ),
                    (
                        "min",
                        format!("{:.2}", timing.min_duration.as_secs_f32() * 1000.0)
                            .as_str()
                            .into(),
                    ),
                    (
                        "max",
                        format!("{:.2}", timing.max_duration.as_secs_f32() * 1000.0)
                            .as_str()
                            .into(),
                    ),
                    ("calls", timing.call_count.into()),
                ],
            ),
        );
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use jigsall_game::resources::SystemTiming;
    use std::time::Duration;

    #[test]
    fn overlay_paints_each_mode_without_capturing_pointer_input() {
        let ctx = egui::Context::default();
        let mut i18n = Localization::default();
        i18n.set_preference(crate::localization::LanguagePreference::Locale(
            crate::localization::Locale::EN_US,
        ));
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let mut perf = PerformanceMonitor::default();
        perf.frame_times.push_back(Duration::from_millis(20));
        let mut timing = SystemTiming::new();
        timing.record_timing(Duration::from_millis(2));
        perf.system_timings
            .insert("apply_piece_commands".into(), timing);
        for mode in [
            PerformanceDebugLevel::Fps,
            PerformanceDebugLevel::Verbose,
            PerformanceDebugLevel::Off,
        ] {
            perf.debug_level = mode;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(viewport),
                    ..default()
                },
                |ui| paint_overlay(ui.ctx(), &perf, 1000, &i18n),
            );
            output.textures_delta.clear();
            let text = output.shapes.iter().find_map(|shape| {
                if let egui::Shape::Text(text) = &shape.shape {
                    Some(text.galley.job.text.as_str())
                } else {
                    None
                }
            });
            match mode {
                PerformanceDebugLevel::Fps => assert_eq!(text, Some("FPS: 50.0")),
                PerformanceDebugLevel::Verbose => {
                    let text = text.unwrap();
                    assert!(text.starts_with("FPS: 50.0\nFrame: 20.00 ms"));
                    assert!(text.contains("Pieces: 1000"));
                    assert!(text.contains("apply_piece_commands\n  last: 2.00 / avg: 2.00"));
                }
                PerformanceDebugLevel::Off => assert!(text.is_none()),
            }
            let rect = output.shapes.iter().find_map(|shape| {
                if let egui::Shape::Rect(rect) = &shape.shape {
                    Some(rect.rect)
                } else {
                    None
                }
            });
            output.drop_without_applying_deltas();
            if let Some(rect) = rect {
                assert!(viewport.contains_rect(rect));
                let pointer = rect.center();
                let output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(viewport),
                        events: vec![
                            egui::Event::PointerMoved(pointer),
                            egui::Event::PointerButton {
                                pos: pointer,
                                button: egui::PointerButton::Primary,
                                pressed: true,
                                modifiers: egui::Modifiers::default(),
                            },
                        ],
                        ..default()
                    },
                    |ui| paint_overlay(ui.ctx(), &perf, 1000, &i18n),
                );
                assert!(!ctx.egui_wants_pointer_input());
                output.drop_without_applying_deltas();
            }
        }
    }
}
