use bevy::prelude::*;
use puzzella_core::PieceId;

/// バッチメッシュ管理システム
#[derive(Resource)]
pub struct BatchManager {
    /// Contiguous depth ranges separated by extracted pieces. This preserves
    /// transparent image compositing as well as the visible picking order.
    pub batched_entities: Vec<Entity>,

    /// バッチに含まれているピースのIDリスト
    pub batched_pieces: std::collections::HashSet<PieceId>,

    /// 抽出されているピースのIDリスト（個別エンティティとして存在）
    pub extracted_pieces: std::collections::HashSet<PieceId>,

    /// 正しい位置に配置されたピースのIDリスト（静的バッチ用）
    pub placed_pieces: std::collections::HashSet<PieceId>,

    /// バッチ再構築が必要かどうか
    pub needs_rebuild: bool,

    /// 最後にバッチが更新された時刻
    pub last_update: std::time::Instant,

    /// パフォーマンス統計
    pub rebuild_count: usize,
    pub total_rebuild_time: std::time::Duration,

    /// バッチ再構築中フラグ（重複実行防止）
    pub is_rebuilding: bool,
}

impl Default for BatchManager {
    fn default() -> Self {
        Self {
            batched_entities: Vec::new(),
            batched_pieces: std::collections::HashSet::new(),
            extracted_pieces: std::collections::HashSet::new(),
            placed_pieces: std::collections::HashSet::new(),
            needs_rebuild: false,
            last_update: std::time::Instant::now(),
            rebuild_count: 0,
            total_rebuild_time: std::time::Duration::ZERO,
            is_rebuilding: false,
        }
    }
}

impl BatchManager {
    /// ピースをバッチから抽出（選択時など）
    pub fn extract_piece(&mut self, piece_id: PieceId) -> bool {
        if self.batched_pieces.remove(&piece_id) {
            self.extracted_pieces.insert(piece_id);
            self.needs_rebuild = true;
            true
        } else {
            false
        }
    }

    /// ピースをバッチに戻す（選択解除時など）
    pub fn return_piece(&mut self, piece_id: PieceId) -> bool {
        if self.extracted_pieces.remove(&piece_id) {
            self.batched_pieces.insert(piece_id);
            self.needs_rebuild = true;
            true
        } else {
            false
        }
    }

    /// ピースを配置済みに設定（正しい位置に配置時）
    pub fn place_piece(&mut self, piece_id: PieceId) -> bool {
        let was_extracted = self.extracted_pieces.remove(&piece_id);
        let was_batched = self.batched_pieces.remove(&piece_id);

        if was_extracted || was_batched {
            self.placed_pieces.insert(piece_id);
            self.batched_pieces.insert(piece_id);
            self.needs_rebuild = true;
            true
        } else {
            false
        }
    }

    /// 新しいピースをバッチに追加
    pub fn add_piece(&mut self, piece_id: PieceId) {
        self.batched_pieces.insert(piece_id);
        self.needs_rebuild = true;
    }

    /// バッチ再構築完了を記録
    pub fn record_rebuild(&mut self, rebuild_time: std::time::Duration) {
        self.rebuild_count += 1;
        self.total_rebuild_time += rebuild_time;
        self.last_update = std::time::Instant::now();
        self.needs_rebuild = false;
        self.is_rebuilding = false;
    }

    /// パフォーマンス統計を取得
    pub fn get_stats(&self) -> String {
        let avg_rebuild_time = if self.rebuild_count > 0 {
            self.total_rebuild_time / self.rebuild_count as u32
        } else {
            std::time::Duration::ZERO
        };

        format!(
            "BatchManager Stats:\n  Batched pieces: {}\n  Extracted pieces: {}\n  Placed pieces: {}\n  Rebuild count: {}\n  Average rebuild time: {:.2}ms\n  Needs rebuild: {}",
            self.batched_pieces.len(),
            self.extracted_pieces.len(),
            self.placed_pieces.len(),
            self.rebuild_count,
            avg_rebuild_time.as_secs_f64() * 1000.0,
            self.needs_rebuild
        )
    }

    /// 全ピース数を取得
    pub fn total_pieces(&self) -> usize {
        self.batched_pieces.len() + self.extracted_pieces.len()
    }
}
