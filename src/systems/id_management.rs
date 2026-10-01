use crate::components::*;
use crate::resources::*;
use bevy::prelude::*;

/// 新しく生成されたピースをPieceIdManagerに登録するシステム
pub fn register_new_pieces_to_id_manager(
    mut id_manager: ResMut<PieceIdManager>,
    new_pieces: Query<(Entity, &PuzzlePiece), Added<PuzzlePiece>>,
) {
    let mut count = 0;
    for (entity, puzzle_piece) in new_pieces.iter() {
        // PuzzlePieceコンポーネントのIDを使用してPieceIdManagerに登録
        let piece_id = id_manager.register_piece(entity, Some(puzzle_piece.id));
        count += 1;

        // 最初の数個と最後のピースのみログ出力
        if count <= 5 || new_pieces.iter().count() - count < 5 {
            println!(
                "📝 Registered piece {} (Entity: {:?}) to ID manager",
                piece_id, entity
            );
        }
    }

    if count > 0 {
        println!(
            "📝 Total {} pieces registered to ID manager this frame",
            count
        );
    }
}

/// Entity削除時のクリーンアップシステム
pub fn cleanup_removed_pieces_from_id_manager(
    mut id_manager: ResMut<PieceIdManager>,
    mut removed_pieces: RemovedComponents<PuzzlePiece>,
) {
    for entity in removed_pieces.read() {
        if let Some(piece_id) = id_manager.unregister_entity(entity) {
            println!(
                "🗑️ Unregistered piece {} (Entity: {:?}) from ID manager",
                piece_id, entity
            );
        }
    }
}

/// ID管理システムの統計情報を表示するシステム（デバッグ用）
pub fn debug_id_manager_stats(id_manager: Res<PieceIdManager>, input: Res<ButtonInput<KeyCode>>) {
    if input.just_pressed(KeyCode::F12) {
        let (id_count, entity_count, counter) = id_manager.get_stats();
        println!("🔍 PieceIdManager Stats:");
        println!("  ID mappings: {}", id_count);
        println!("  Entity mappings: {}", entity_count);
        println!("  Next counter: {}", counter);

        let piece_ids = id_manager.get_all_piece_ids();
        println!("  All piece IDs: {:?}", piece_ids.len());
    }
}
