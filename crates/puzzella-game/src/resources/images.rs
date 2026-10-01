use bevy::prelude::*;

#[derive(Resource)]
pub struct PuzzleImage {
    pub handle: Handle<Image>,
    pub size: Vec2,
}

/// 画像読み込みチャネル（crossbeam-channel）
#[derive(Resource)]
pub struct ImageLoadChannels {
    /// 画像読み込み結果を受信するチャネル
    pub rx_results: crossbeam::channel::Receiver<crate::asset_reader::ImageLoadResult>,
}

/// 画像読み込み送信チャネル（スレッド間通信用）
#[derive(Resource)]
pub struct ImageLoadSender {
    pub tx_results: crossbeam::channel::Sender<crate::asset_reader::ImageLoadResult>,
}
