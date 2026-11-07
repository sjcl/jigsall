use super::*;
use bevy::{
    asset::AssetPlugin,
    input::{
        mouse::{AccumulatedMouseMotion, MouseScrollUnit, MouseWheel},
        InputPlugin,
    },
    state::app::StatesPlugin,
    transform::TransformPlugin,
};

#[test]
fn paused_puzzle_schedule_keeps_camera_controls_but_rejects_piece_input_and_commands() {
    let mut app = App::new();
    app.insert_resource(PersistenceService::with_storage_requests().0)
        .insert_resource(crate::keybindings::KeyBindingsState::load(None))
        .insert_resource(crate::player_settings::PlayerSettingsState::load(None))
        .insert_resource(crate::image_settings::ImageSettingsState::load(None))
        .add_plugins((
            MinimalPlugins,
            StatesPlugin,
            InputPlugin,
            TransformPlugin,
            AssetPlugin::default(),
            crate::asset_reader::DirectFileAssetPlugin,
            GamePlugin,
        ))
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<ColorMaterial>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<bevy_egui::EguiUserTextures>();
    app.update();
    let world = app.world_mut();
    let window = world
        .spawn((Window::default(), PrimaryWindow, CursorOptions::default()))
        .id();
    world
        .get_mut::<Window>(window)
        .unwrap()
        .set_cursor_position(Some(Vec2::splat(200.0)));
    world.insert_resource(State::new(AppState::InGame));
    world.insert_resource(State::new(GameSubState::Playing));
    world.insert_resource(NextState::<GameSubState>::Unchanged);
    world
        .resource_mut::<PieceDataStore>()
        .initialize(vec![Vec2::splat(100.0)]);
    world.resource_mut::<GameplayPause>().request(true);
    world.run_schedule(PostUpdate);
    assert!(world.resource::<GameplayPause>().paused);
    let original = world.resource::<PieceDataStore>().states.clone();
    let camera = world
        .query_filtered::<Entity, With<MainCamera>>()
        .single(world)
        .unwrap();
    let before = *world.get::<Transform>(camera).unwrap();
    world
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    world
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Right);
    let player = world.resource::<LocalPlayerId>().0;
    world
        .resource_mut::<Messages<ClientCommand>>()
        .write(ClientCommand {
            player,
            command: PieceCommand::Grab(PieceId(0)),
        });
    world
        .resource_mut::<Messages<MouseWheel>>()
        .write(MouseWheel {
            unit: MouseScrollUnit::Line,
            phase: bevy::input::touch::TouchPhase::Moved,
            x: 0.0,
            y: 1.0,
            window,
        });
    world.run_schedule(PostUpdate);
    assert_ne!(world.get::<Transform>(camera).unwrap().scale, before.scale);
    assert!(world.resource::<InputState>().is_camera_dragging);
    let before_pan = world.get::<Transform>(camera).unwrap().translation;
    world.resource_mut::<AccumulatedMouseMotion>().delta = Vec2::new(20.0, 10.0);
    world.run_schedule(PostUpdate);
    assert_ne!(
        world.get::<Transform>(camera).unwrap().translation,
        before_pan
    );
    assert_eq!(world.resource::<PieceDataStore>().states, original);
    assert!(world.resource::<PieceDataStore>().held_by.is_empty());
    assert!(world
        .resource::<crate::selection::PuzzleSelection>()
        .latest
        .is_none());
    assert!(!world
        .resource::<crate::interaction::PieceInteraction>()
        .is_dragging());
    world.resource_mut::<GameplayPause>().request(false);
    world.run_schedule(PostUpdate);
    assert!(!world.resource::<GameplayPause>().paused);
    world
        .resource_mut::<Messages<ClientCommand>>()
        .write(ClientCommand {
            player,
            command: PieceCommand::Grab(PieceId(0)),
        });
    world.run_schedule(PostUpdate);
    assert_eq!(
        world.resource::<PieceDataStore>().held_by.get(&PieceId(0)),
        Some(&player)
    );
}
