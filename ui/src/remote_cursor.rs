//! Paint-only overlay. Its inputs contain no network runtime or gameplay store.
use crate::localization::Localization;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use puzzella_core::PlayerId;
use puzzella_game::{
    players::PlayerRoster,
    resources::{remote_cursor::RemoteCursorPresentation, LocalPlayerId},
    MainCamera,
};

fn player_color(player: PlayerId) -> egui::Color32 {
    const PALETTE: [[u8; 3]; 8] = [
        [255, 116, 113],
        [101, 190, 255],
        [115, 222, 157],
        [241, 199, 92],
        [189, 152, 255],
        [255, 151, 212],
        [100, 220, 224],
        [255, 173, 110],
    ];
    // Stable on every OS/client; adjacent IDs select different palette entries.
    let rgb = PALETTE[(player.0 % PALETTE.len() as u64) as usize];
    egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2])
}
fn cursor_name<'a>(
    roster: &'a PlayerRoster,
    local: PlayerId,
    player: PlayerId,
    fallback: &'a str,
) -> Option<&'a str> {
    if player == local {
        return None;
    }
    let info = roster.get(player)?;
    Some(
        info.display_name
            .as_ref()
            .map(|n| n.as_ref())
            .unwrap_or(fallback),
    )
}
fn project(
    camera: &Camera,
    transform: &Transform,
    world: Vec2,
    pixels_per_point: f32,
) -> Option<(egui::Pos2, egui::Rect)> {
    if !world.is_finite()
        || !transform.to_matrix().is_finite()
        || !pixels_per_point.is_finite()
        || pixels_per_point <= 0.0
    {
        return None;
    }
    let viewport = camera.logical_viewport_rect()?;
    let point = camera
        .world_to_viewport(&GlobalTransform::from(*transform), world.extend(0.0))
        .ok()?;
    if !point.is_finite() || !viewport.contains(point) {
        return None;
    }
    // Camera projection uses logical window pixels; egui uses its own UI points.
    let scale = camera.computed.target_info.as_ref()?.scale_factor / pixels_per_point;
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let pos = |p: Vec2| egui::pos2(p.x * scale, p.y * scale);
    Some((
        pos(point),
        egui::Rect::from_min_max(pos(viewport.min), pos(viewport.max)),
    ))
}
fn paint_cursor(painter: &egui::Painter, point: egui::Pos2, player: PlayerId, name: &str) {
    let color = player_color(player);
    painter.add(egui::Shape::convex_polygon(
        vec![
            point,
            point + egui::vec2(4.0, 16.0),
            point + egui::vec2(15.0, 8.0),
        ],
        color,
        egui::Stroke::new(1.0, egui::Color32::from_black_alpha(220)),
    ));
    let galley = painter.layout_no_wrap(
        name.to_owned(),
        egui::FontId::proportional(12.0),
        egui::Color32::WHITE,
    );
    let origin = point + egui::vec2(17.0, 9.0);
    painter.rect_filled(
        egui::Rect::from_min_size(
            origin - egui::vec2(4.0, 2.0),
            galley.size() + egui::vec2(8.0, 4.0),
        ),
        3.0,
        egui::Color32::from_black_alpha(190),
    );
    painter.galley(origin, galley, egui::Color32::WHITE);
}
pub(crate) fn draw_remote_cursors(
    mut contexts: EguiContexts,
    presentation: Res<RemoteCursorPresentation>,
    roster: Res<PlayerRoster>,
    local: Res<LocalPlayerId>,
    cameras: Query<(&Camera, &Transform), With<MainCamera>>,
    i18n: Res<Localization>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let Ok((camera, transform)) = cameras.single() else {
        return;
    };
    // Shared background layer is painted before HUD; menus/dialogs use higher layers.
    // No Ui, Area, Response, widget, interaction rectangle or focus registration.
    let painter = ctx.layer_painter(egui::LayerId::background());
    let fallback = i18n.text("game-default-player");
    for (player, cursor) in presentation.cursors() {
        let Some(name) = cursor_name(&roster, local.0, player, &fallback) else {
            continue;
        };
        let Some((point, clip)) = project(
            camera,
            transform,
            cursor.displayed_world_position,
            ctx.pixels_per_point(),
        ) else {
            continue;
        };
        paint_cursor(&painter.with_clip_rect(clip), point, player, name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::camera::{ComputedCameraValues, RenderTargetInfo, Viewport};
    #[test]
    fn cursor_projection_pan_zoom_viewport_offset_hidpi_without_new_network_target() {
        let camera = Camera {
            computed: ComputedCameraValues {
                clip_from_view: Mat4::orthographic_rh(-500., 500., -400., 400., 0., 1000.),
                target_info: Some(RenderTargetInfo {
                    physical_size: UVec2::new(2000, 1600),
                    scale_factor: 2.0,
                }),
                ..default()
            },
            ..default()
        };
        let target = Vec2::new(200., 100.);
        let p = |transform: Transform| project(&camera, &transform, target, 2.0).unwrap().0;
        assert_eq!(p(Transform::IDENTITY), egui::pos2(700., 300.));
        assert_eq!(
            p(Transform::from_xyz(100., -100., 0.)),
            egui::pos2(600., 200.)
        );
        assert_eq!(
            p(Transform::from_scale(Vec3::splat(2.))),
            egui::pos2(600., 350.)
        );
        assert_eq!(target, Vec2::new(200., 100.));
        assert_eq!(
            project(&camera, &Transform::IDENTITY, target, 4.0)
                .unwrap()
                .0,
            egui::pos2(350., 150.)
        );
        assert!(project(&camera, &Transform::IDENTITY, Vec2::splat(10000.), 2.).is_none());
        assert!(project(&camera, &Transform::IDENTITY, Vec2::splat(f32::NAN), 2.).is_none());
        let camera = Camera {
            viewport: Some(Viewport {
                physical_position: UVec2::new(200, 100),
                physical_size: UVec2::new(1000, 800),
                ..default()
            }),
            ..camera
        };
        assert_eq!(
            project(&camera, &Transform::IDENTITY, Vec2::ZERO, 2.)
                .unwrap()
                .0,
            egui::pos2(350., 250.)
        );
    }
    #[test]
    fn cursor_visual_name_color_own_filter_and_no_input_capture() {
        use puzzella_core::PlayerDisplayName;
        use puzzella_game::players::{RosterPlayer, RosterSnapshot};
        let mut roster = PlayerRoster::default();
        roster
            .install_snapshot(
                RosterSnapshot {
                    revision: 2,
                    players: (0..3)
                        .map(|id| RosterPlayer {
                            player: PlayerId(id),
                            display_name: (id != 0)
                                .then(|| PlayerDisplayName::from_user_input("Alice").unwrap()),
                        })
                        .collect(),
                },
                PlayerId(0),
                PlayerId(1),
            )
            .unwrap();
        assert!(cursor_name(&roster, PlayerId(1), PlayerId(1), "Player").is_none());
        assert!(cursor_name(&roster, PlayerId(1), PlayerId(9), "Player").is_none());
        assert_eq!(
            cursor_name(&roster, PlayerId(1), PlayerId(0), "Player"),
            Some("Player")
        );
        assert_eq!(
            cursor_name(&roster, PlayerId(0), PlayerId(1), "Player"),
            Some("Alice")
        );
        assert_eq!(
            cursor_name(&roster, PlayerId(0), PlayerId(2), "Player"),
            Some("Alice")
        );
        assert_ne!(player_color(PlayerId(1)), player_color(PlayerId(2)));
        assert_eq!(player_color(PlayerId(1)), player_color(PlayerId(1)));
        let ctx = egui::Context::default();
        // Match bevy_egui's run_ui pass, which records the unused root area.
        // A bare begin_pass has no root area and conservatively captures input.
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800., 600.),
                )),
                events: vec![egui::Event::PointerMoved(egui::pos2(103., 105.))],
                ..default()
            },
            |ui| {
                paint_cursor(
                    &ui.ctx().layer_painter(egui::LayerId::background()),
                    egui::pos2(100., 100.),
                    PlayerId(2),
                    "Alice",
                );
            },
        );
        assert!(!ctx.egui_wants_pointer_input());
        assert!(!ctx.egui_wants_keyboard_input());
        assert!(output
            .shapes
            .iter()
            .any(|s| matches!(&s.shape,egui::Shape::Text(t) if t.galley.text()=="Alice")));
        let marker = output
            .shapes
            .iter()
            .find_map(|s| {
                if let egui::Shape::Path(p) = &s.shape {
                    Some(p)
                } else {
                    None
                }
            })
            .unwrap();
        let bounds = marker.visual_bounding_rect();
        assert!(bounds.width() < 20. && bounds.height() < 20.);
        output.drop_without_applying_deltas();
    }
}
