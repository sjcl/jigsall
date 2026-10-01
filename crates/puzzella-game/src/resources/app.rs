use bevy::prelude::*;
use puzzella_core::PlayerId;
use serde::{Deserialize, Serialize};

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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlayerInfo {
    pub id: PlayerId,
    pub name: String,
    pub score: u32,
}
