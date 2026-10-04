use super::*;
use crate::localization::{LanguagePreference, Locale};
use bevy::ecs::system::RunSystemOnce;
use jigsall_core::PlayerDisplayName;
use jigsall_game::players::{PresenceMessage, RosterPlayer, RosterSnapshot};

fn font() -> FontRef<'static> {
    FontRef::try_from_slice(EMBEDDED_FALLBACK_FONTS[0].bytes).unwrap()
}

#[test]
fn cursor_atlas_latin_japanese_max_names_bounded_sorted_disjoint_and_deterministic() {
    let names: Vec<_> = (0..65)
        .map(|id| {
            (
                PlayerId(id),
                match id % 5 {
                    0 => "Alice".into(),
                    1 => "山田太郎".into(),
                    2 => "テストユーザー".into(),
                    3 => "漢".repeat(32),
                    _ => "Player �\u{10ffff}".into(),
                },
            )
        })
        .collect();
    for scale in [0.5, 1.0, 2.0, 4.0, 1000.0] {
        let (a, labels) = rasterize(&font(), &names, scale).unwrap();
        let mut reversed = names.clone();
        reversed.reverse();
        let (b, other) = rasterize(&font(), &reversed, scale).unwrap();
        assert_eq!(a.data, b.data);
        assert_eq!(a.texture_descriptor.size, b.texture_descriptor.size);
        assert_eq!(labels.len(), 65);
        assert_eq!(
            labels.keys().copied().collect::<Vec<_>>(),
            names.iter().map(|n| n.0).collect::<Vec<_>>()
        );
        assert_eq!(a.texture_descriptor.format, TextureFormat::R8Unorm);
        let size = Vec2::new(a.width() as f32, a.height() as f32);
        assert!(size.max_element() <= MAX_LABEL_ATLAS_DIMENSION as f32);
        let bitmap = a.data.as_ref().unwrap();
        assert!(bitmap.len() <= MAX_LABEL_ATLAS_BYTES);
        let mut rects = vec![];
        for (id, label) in labels {
            assert_eq!(label.uv, other[&id].uv);
            assert!(label.uv.min_element() >= 0.0 && label.uv.max_element() <= 1.0);
            let rect = Rect::from_corners(
                Vec2::new(label.uv.x, label.uv.y) * size,
                Vec2::new(label.uv.z, label.uv.w) * size,
            );
            for previous in &rects {
                assert!(rect.intersect(*previous).is_empty());
            }
            // Every supported Latin/Japanese/replacement label has actual coverage.
            let covered = (rect.min.y as usize..rect.max.y as usize).any(|y| {
                bitmap[y * a.width() as usize + rect.min.x as usize
                    ..y * a.width() as usize + rect.max.x as usize]
                    .iter()
                    .any(|&v| v != 0)
            });
            assert!(covered, "label {id:?}, scale {scale}");
            rects.push(rect);
        }
    }
    assert!(rasterize(&font(), &[], 1.0).is_none());
    assert!(rasterize(&font(), &[(PlayerId(1), String::new())], 1.0).is_some());
    let longest: Vec<_> = (0..65).map(|id| (PlayerId(id), "漢".repeat(32))).collect();
    for scale in [1.0, 2.0, 4.0] {
        let (image, labels) = rasterize(&font(), &longest, scale).unwrap();
        assert_eq!(labels.len(), 65);
        assert!(
            image.width() <= MAX_LABEL_ATLAS_DIMENSION
                && image.height() <= MAX_LABEL_ATLAS_DIMENSION
        );
        assert!(image.data.as_ref().unwrap().len() <= MAX_LABEL_ATLAS_BYTES);
    }
}

#[test]
fn cursor_atlas_hidpi_doubles_raster_resolution_preserves_logical_size() {
    for name in ["Alice", "山田太郎", "テストユーザー"] {
        let names = [(PlayerId(1), name.into())];
        let (a, first) = rasterize(&font(), &names, 1.0).unwrap();
        let (b, second) = rasterize(&font(), &names, 2.0).unwrap();
        let logical_a = first[&PlayerId(1)].logical_size;
        let logical_b = second[&PlayerId(1)].logical_size;
        assert!(logical_a.abs_diff_eq(logical_b, 1.0));
        assert!(b.height() >= a.height() * 2);
        assert!(layout(&font(), name, 2.0).size.x >= layout(&font(), name, 1.0).size.x * 2 - 1);
    }
}

fn roster(name: Option<&str>) -> PlayerRoster {
    PlayerRoster::validated_snapshot(
        RosterSnapshot {
            revision: 1,
            players: vec![
                RosterPlayer {
                    player: PlayerId(0),
                    display_name: None,
                },
                RosterPlayer {
                    player: PlayerId(1),
                    display_name: name.map(|n| PlayerDisplayName::from_user_input(n).unwrap()),
                },
            ],
        },
        PlayerId(0),
        PlayerId(0),
    )
    .unwrap()
}

#[test]
fn cursor_atlas_rebuilds_on_presence_names_locale_dpi_reset_only() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .init_resource::<Assets<Image>>()
        .init_resource::<LabelAtlasCache>()
        .init_resource::<RemoteCursorLabels>()
        .init_resource::<LocalPlayerId>()
        .init_resource::<Localization>()
        .insert_resource(roster(None))
        .add_systems(Last, prepare_label_atlas);
    let window = app
        .world_mut()
        .spawn((Window::default(), bevy::window::PrimaryWindow))
        .id();
    app.update();
    assert_eq!(
        app.world()
            .resource::<LabelAtlasCache>()
            .key
            .as_ref()
            .unwrap()
            .names[0]
            .1,
        app.world()
            .resource::<Localization>()
            .text("game-default-player")
    );
    let revision = |app: &App| app.world().resource::<LabelAtlasCache>().revision;
    let mut last = revision(&app);
    let image = app
        .world()
        .resource::<RemoteCursorLabels>()
        .atlas
        .as_ref()
        .unwrap()
        .image
        .id();
    for _ in 0..8 {
        app.update();
        assert_eq!(revision(&app), last);
    }
    // Camera motion is deliberately outside the atlas builder's system inputs.
    let camera = app
        .world_mut()
        .spawn((jigsall_game::MainCamera, Transform::IDENTITY))
        .id();
    for frame in 0..8 {
        *app.world_mut().get_mut::<Transform>(camera).unwrap() =
            Transform::from_xyz(frame as f32, 0.0, 0.0).with_scale(Vec3::splat(1.0 + frame as f32));
        app.update();
        assert_eq!(revision(&app), last);
    }
    assert_eq!(
        app.world()
            .resource::<RemoteCursorLabels>()
            .atlas
            .as_ref()
            .unwrap()
            .image
            .id(),
        image
    );
    app.world_mut()
        .resource_mut::<PlayerRoster>()
        .apply_presence(PresenceMessage::PlayerJoined {
            revision: 2,
            player: RosterPlayer {
                player: PlayerId(2),
                display_name: None,
            },
        })
        .unwrap();
    app.update();
    assert_eq!(revision(&app), last + 1);
    last += 1;
    app.world_mut()
        .resource_mut::<PlayerRoster>()
        .apply_presence(PresenceMessage::PlayerLeft {
            revision: 3,
            player: PlayerId(2),
        })
        .unwrap();
    app.update();
    assert_eq!(revision(&app), last + 1);
    last += 1;
    assert!(!app
        .world()
        .resource::<RemoteCursorLabels>()
        .members
        .contains(&PlayerId(2)));
    assert!(!app
        .world()
        .resource::<RemoteCursorLabels>()
        .atlas
        .as_ref()
        .unwrap()
        .labels
        .contains_key(&PlayerId(2)));
    let mut renamed = roster(Some("山田太郎")).snapshot();
    renamed.revision = 3; // Keep membership/revision fixed: only the name changes.
    app.world_mut()
        .resource_mut::<PlayerRoster>()
        .install_snapshot(renamed, PlayerId(0), PlayerId(0))
        .unwrap();
    app.update();
    assert_eq!(revision(&app), last + 1);
    last += 1;
    let current = app.world().resource::<Localization>().locale();
    app.world_mut()
        .resource_mut::<Localization>()
        .set_preference(LanguagePreference::Locale(if current == Locale::JA {
            Locale::EN_US
        } else {
            Locale::JA
        }));
    app.update();
    assert_eq!(revision(&app), last + 1);
    last += 1;
    app.world_mut()
        .get_mut::<Window>(window)
        .unwrap()
        .resolution
        .set_scale_factor_override(Some(2.0));
    app.update();
    assert_eq!(revision(&app), last + 1);
    last += 1;
    app.world_mut().run_system_once(reset_label_atlas).unwrap();
    app.update();
    assert_eq!(revision(&app), last + 1);
    let output = app.world().resource::<RemoteCursorLabels>();
    assert!(!output.members.contains(&PlayerId(0)));
    let atlas = output.atlas.as_ref().unwrap();
    assert!(atlas.labels.contains_key(&PlayerId(1)));
    assert_eq!(atlas.revision, revision(&app));
}
