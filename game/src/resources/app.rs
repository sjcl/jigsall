use bevy::prelude::*;
use puzzella_core::{PlayerId, LOCAL_PLAYER};
use serde::{Deserialize, Serialize};

/// This process's session identity, independent of puzzle/snapshot state.
/// Snapshot installation may replace PieceDataStore without resetting this resource.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalPlayerId(pub PlayerId);

impl Default for LocalPlayerId {
    fn default() -> Self {
        Self(LOCAL_PLAYER)
    }
}

#[derive(Resource, Default)]
pub struct GameData {
    pub players: Vec<PlayerInfo>,
    pub puzzle_completed: bool,
    pub puzzle_progress: f32,
}

/// メインアプリケーションの状態
#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AppState {
    #[default]
    Menu, // メインメニュー
    GameSetup,    // ゲーム設定・画像読み込み
    InGame,       // ゲーム中
    GameComplete, // ゲーム完了
}

/// ゲーム内のサブ状態（InGame時のみ有効）
#[derive(SubStates, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[source(AppState = AppState::InGame)]
pub enum GameSubState {
    #[default]
    Initializing, // パズル生成中
    Playing, // プレイ中
    Paused,  // ポーズ中（ESCメニュー）
}

/// Completion keeps the existing session alive while showing its result or canvas.
#[derive(SubStates, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[source(AppState = AppState::GameComplete)]
pub enum GameCompleteSubState {
    #[default]
    Summary,
    Viewing,
    Paused,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlayerInfo {
    pub id: PlayerId,
    /// None has no user-supplied name; the UI supplies its localized default.
    pub name: Option<String>,
    pub score: u32,
}
