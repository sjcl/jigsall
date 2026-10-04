//! egui screens and their state-specific schedule registration.
mod completion;
mod fonts;
mod game_play;
mod game_setup;
mod grid;
mod key_config;
pub mod localization;
mod menu;
mod messages;
mod multiplayer;
mod overlays;
mod performance;
mod persistence;
mod preferences;
mod remote_cursor;
mod settings;
mod theme;
mod window_close;
use bevy::{ecs::schedule::common_conditions::not, prelude::*};
use bevy_egui::{EguiPlugin, EguiPrimaryContextPass};
use puzzella_game::resources::{AppState, GameCompleteSubState, GameSubState};

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct GameplayUi;

pub struct GameUiPlugin;
impl Plugin for GameUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<localization::Localization>()
            .init_resource::<preferences::UiPreferences>()
            .add_systems(Startup, preferences::initialize)
            .add_systems(Update, preferences::poll_save)
            .init_resource::<persistence::SaveDialogs>()
            .init_resource::<persistence::thumbnails::SaveThumbnails>()
            .init_resource::<settings::SettingsDialog>()
            .init_resource::<multiplayer::MultiplayerUi>()
            .init_resource::<puzzella_game::resources::LocalGameplayBlocked>()
            .add_systems(PreUpdate, window_close::handle_close_requests)
            .add_systems(First, multiplayer::start_prepared_host)
            .add_systems(
                First,
                multiplayer::sync_local_gameplay_block.after(multiplayer::start_prepared_host),
            )
            .add_systems(OnEnter(AppState::Menu), multiplayer::reset_on_menu)
            .init_resource::<game_setup::image_picker::ImagePicker>()
            .add_systems(OnEnter(AppState::GameSetup), game_setup::randomize_seed)
            .add_systems(Update, game_setup::image_picker::finish_image_selection)
            .add_systems(
                OnExit(AppState::GameSetup),
                game_setup::image_picker::discard_image_selection,
            )
            .add_systems(OnEnter(AppState::Menu), persistence::reset_dialogs)
            .add_systems(OnEnter(AppState::Menu), settings::reset_dialog)
            .add_systems(OnExit(AppState::Menu), settings::reset_dialog)
            .add_plugins(EguiPlugin::default());
        register_screens(app);
    }
}

fn register_screens(app: &mut App) {
    app.configure_sets(
        EguiPrimaryContextPass,
        GameplayUi.after(multiplayer::draw_connection_ui).run_if(
            multiplayer::connection_screen_hidden.or_else(window_close::exit_dialog_pending),
        ),
    )
    .add_systems(
        EguiPrimaryContextPass,
        (
            persistence::draw_save_dialogs
                .in_set(GameplayUi)
                .after(menu::draw_menu_ui)
                .after(overlays::draw_in_game_menu_ui)
                .after(completion::draw_completion_ui),
            persistence::process_departure
                .in_set(GameplayUi)
                .after(persistence::draw_save_dialogs),
            menu::draw_menu_ui.run_if(in_state(AppState::Menu)),
            settings::draw_settings_ui
                .in_set(GameplayUi)
                .after(menu::draw_menu_ui)
                .after(game_setup::draw_game_setup_ui)
                .run_if(in_state(AppState::Menu).or_else(in_state(AppState::GameSetup))),
            game_setup::draw_game_setup_ui.run_if(in_state(AppState::GameSetup)),
            multiplayer::draw_connection_ui
                .run_if(not(window_close::exit_dialog_pending))
                .after(menu::draw_menu_ui)
                .after(game_setup::draw_game_setup_ui),
            multiplayer::process_actions
                .after(GameplayUi)
                .after(menu::draw_menu_ui)
                .after(game_setup::draw_game_setup_ui)
                .after(persistence::draw_save_dialogs)
                .after(overlays::draw_in_game_menu_ui)
                .after(completion::draw_completion_ui)
                .after(multiplayer::draw_connection_ui),
            multiplayer::sync_local_gameplay_block.after(multiplayer::process_actions),
            remote_cursor::draw_remote_cursors
                .in_set(GameplayUi)
                .before(game_play::draw_game_ui)
                .before(performance::draw_performance_overlay)
                .before(game_play::draw_players_overlay)
                .run_if(in_state(GameSubState::Playing)),
            game_play::draw_game_ui
                .in_set(GameplayUi)
                .run_if(in_state(AppState::InGame)),
            performance::draw_performance_overlay
                .in_set(GameplayUi)
                .run_if(in_state(AppState::InGame)),
            game_play::draw_players_overlay
                .in_set(GameplayUi)
                .run_if(in_state(AppState::InGame).and_then(players_key_pressed)),
            overlays::draw_in_game_menu_ui.in_set(GameplayUi).run_if(
                in_state(GameSubState::Paused).or_else(in_state(GameCompleteSubState::Paused)),
            ),
            overlays::draw_generation_progress_ui
                .in_set(GameplayUi)
                .run_if(in_state(AppState::InGame)),
            completion::draw_completion_ui
                .in_set(GameplayUi)
                .run_if(in_state(GameCompleteSubState::Summary)),
            completion::draw_completed_puzzle_ui
                .in_set(GameplayUi)
                .run_if(
                    in_state(GameCompleteSubState::Viewing)
                        .or_else(in_state(GameCompleteSubState::Paused)),
                ),
        ),
    );
}
fn players_key_pressed(
    keys: Res<ButtonInput<KeyCode>>,
    bindings: Res<puzzella_game::keybindings::KeyBindingsState>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    egui_input: Option<Res<bevy_egui::input::EguiWantsInput>>,
) -> bool {
    windows.iter().all(|window| window.focused)
        && egui_input.is_none_or(|input| !input.wants_any_keyboard_input())
        && bindings
            .current
            .pressed(puzzella_game::keybindings::KeyAction::ShowPlayers, &keys)
}
