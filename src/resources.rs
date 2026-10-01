use crate::components::{PieceShape, PuzzlePiece};
pub use crate::gameplay::PieceId;
use crate::gameplay::{PieceState, PlayerId};
use crate::jigsaw_shapes::JigsawShapeGenerator;
use bevy::prelude::*;
use bevy::sprite_render::ColorMaterial;
use crossbeam::channel;
use instant::Instant;
use rstar::{RTree, RTreeObject, AABB};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::collections::{HashMap, HashSet};

/// ピースの当たり判定データ（CPU側で管理）
#[derive(Debug, Clone, PartialEq)]
pub struct PieceCollisionData {
    pub piece_id: PieceId,
    pub position: Vec2,
    pub z_order: f32,
    pub bounding_box: Rect,
    pub vertices: Vec<Vec2>, // Indexed mesh vertices in local coordinates.
    pub indices: Vec<u32>,   // トライアングル頂点インデックス
}

/// rstar R-tree用のトレイト実装
impl RTreeObject for PieceCollisionData {
    type Envelope = AABB<[f32; 2]>;

    fn envelope(&self) -> Self::Envelope {
        AABB::from_corners(
            [self.bounding_box.min.x, self.bounding_box.min.y],
            [self.bounding_box.max.x, self.bounding_box.max.y],
        )
    }
}

/// IDベースの当たり判定システム
#[derive(Resource, Default)]
pub struct PieceCollisionSystem {
    pub pieces: HashMap<PieceId, PieceCollisionData>,
    pub rtree: RTree<PieceCollisionData>,
    pub dragging_pieces: HashSet<PieceId>, // ドラッグ中で当たり判定対象外のピース
    pub need_rebuild: bool,
}

impl PieceCollisionSystem {
    pub fn add_piece(&mut self, collision_data: PieceCollisionData) {
        self.pieces.insert(collision_data.piece_id, collision_data);
        self.need_rebuild = true;
    }

    pub fn remove_piece(&mut self, piece_id: PieceId) {
        if let Some(old) = self.pieces.remove(&piece_id) {
            self.rtree.remove(&old);
        }
        self.dragging_pieces.remove(&piece_id);
    }

    pub fn update_piece_position(
        &mut self,
        piece_id: PieceId,
        new_position: Vec2,
        local_bounds: Rect,
    ) {
        if let Some(piece) = self.pieces.get_mut(&piece_id) {
            if piece.position == new_position {
                return;
            }
            // Use the exact old record: reconstructing it with float subtraction
            // can leave an unequal, stale record in the R-tree.
            let indexed = !self.need_rebuild && !self.dragging_pieces.contains(&piece_id);
            if indexed {
                self.rtree.remove(piece);
            }
            piece.position = new_position;
            // Reproject immutable bounds instead of accumulating rounding
            // error after repeated moves.
            piece.bounding_box.min = new_position + local_bounds.min;
            piece.bounding_box.max = new_position + local_bounds.max;
            if indexed {
                self.rtree.insert(piece.clone());
            }
        }
    }

    pub fn update_piece_z_order(&mut self, piece_id: PieceId, z_order: f32) {
        if let Some(piece) = self.pieces.get_mut(&piece_id) {
            if piece.z_order == z_order {
                return;
            }
            let indexed = !self.need_rebuild && !self.dragging_pieces.contains(&piece_id);
            if indexed {
                self.rtree.remove(piece);
            }
            piece.z_order = z_order;
            if indexed {
                self.rtree.insert(piece.clone());
            }
        }
    }

    pub fn rebuild_rtree(&mut self) {
        if !self.need_rebuild {
            return;
        }

        // R-treeを再構築 - ドラッグ中ピースを除外してbulk_loadを使用
        let pieces_vec: Vec<PieceCollisionData> = self
            .pieces
            .values()
            .filter(|piece_data| !self.dragging_pieces.contains(&piece_data.piece_id))
            .cloned()
            .collect();

        if !pieces_vec.is_empty() {
            self.rtree = RTree::bulk_load(pieces_vec);
        } else {
            self.rtree = RTree::new();
        }

        self.need_rebuild = false;
        // デバッグログは High レベルでのみ表示（頻繁なログを避けるため）
        // println!("✅ R-tree rebuilt with {} pieces ({} dragging excluded)",
        //     pieces_count, self.dragging_pieces.len());
    }

    /// ドラッグ開始: ピースをR-treeから除外
    pub fn start_dragging_piece(&mut self, piece_id: PieceId, debug_level: &PerformanceDebugLevel) {
        if self.dragging_pieces.contains(&piece_id) {
            return;
        }
        if let Some(piece_data) = self.pieces.get(&piece_id) {
            // R-treeから削除
            self.rtree.remove(piece_data);
            // ドラッグ中リストに追加
            self.dragging_pieces.insert(piece_id);

            // デバッグレベルが Medium 以上の場合のみログ出力
            if matches!(
                debug_level,
                PerformanceDebugLevel::Medium | PerformanceDebugLevel::High
            ) {
                println!(
                    "🎯 Piece {} removed from R-tree (dragging started)",
                    piece_id
                );
            }
        }
    }

    /// ドラッグ終了: ピースをR-treeに再挿入
    pub fn stop_dragging_piece(&mut self, piece_id: PieceId, debug_level: &PerformanceDebugLevel) {
        if self.dragging_pieces.remove(&piece_id) {
            if let Some(piece_data) = self.pieces.get(&piece_id) {
                // R-treeに再挿入
                if !self.need_rebuild {
                    self.rtree.insert(piece_data.clone());
                }

                // デバッグレベルが Medium 以上の場合のみログ出力
                if matches!(
                    debug_level,
                    PerformanceDebugLevel::Medium | PerformanceDebugLevel::High
                ) {
                    println!(
                        "🎯 Piece {} re-inserted to R-tree (dragging stopped)",
                        piece_id
                    );
                }
            }
        }
    }

    /// R-treeによる候補検索
    pub fn query_pieces_in_rect(&mut self, query_rect: Rect) -> Vec<PieceId> {
        // R-tree再構築チェック（必要な場合のみ実行）
        if self.need_rebuild {
            self.rebuild_rtree();
        }

        // R-treeを使用して範囲内のピースを検索
        let envelope = rstar::AABB::from_corners(
            [query_rect.min.x, query_rect.min.y],
            [query_rect.max.x, query_rect.max.y],
        );

        let results: Vec<PieceId> = self
            .rtree
            .locate_in_envelope_intersecting(&envelope)
            .map(|piece_data| piece_data.piece_id)
            .filter(|&piece_id| !self.dragging_pieces.contains(&piece_id)) // 念のため除外
            .collect();
        results
    }

    /// デバッグ用のR-treeクエリ（詳細ログ付き）
    pub fn query_pieces_in_rect_debug(&mut self, query_rect: Rect) -> (Vec<PieceId>, String) {
        // R-tree再構築チェック（必要な場合のみ実行）
        if self.need_rebuild {
            self.rebuild_rtree();
        }

        let mut debug_info = "🌳 R-tree Query Debug:\n".to_string();
        debug_info.push_str(&format!(
            "📍 Query rect: ({:.1}, {:.1}) to ({:.1}, {:.1}) [{}x{}]\n",
            query_rect.min.x,
            query_rect.min.y,
            query_rect.max.x,
            query_rect.max.y,
            query_rect.width() as i32,
            query_rect.height() as i32
        ));

        // R-treeを使用して範囲内のピースを検索
        let envelope = rstar::AABB::from_corners(
            [query_rect.min.x, query_rect.min.y],
            [query_rect.max.x, query_rect.max.y],
        );

        let results: Vec<PieceId> = self
            .rtree
            .locate_in_envelope_intersecting(&envelope)
            .map(|piece_data| piece_data.piece_id)
            .collect();

        debug_info.push_str(&format!("🎯 R-tree found {} pieces\n", results.len()));
        debug_info.push_str(&format!(
            "🌍 Total pieces in system: {}\n",
            self.pieces.len()
        ));

        // 最も近いピースがなぜ見つからないのかをチェック
        if let Some((nearest_id, nearest_distance)) =
            self.find_nearest_piece(query_rect.center(), 500.0)
        {
            if let Some(nearest_data) = self.pieces.get(&nearest_id) {
                debug_info.push_str(&format!("\n🎯 Nearest piece analysis ({})\n", nearest_id));
                debug_info.push_str(&format!(
                    "   📦 Nearest BBox: ({:.1}, {:.1}) to ({:.1}, {:.1})\n",
                    nearest_data.bounding_box.min.x,
                    nearest_data.bounding_box.min.y,
                    nearest_data.bounding_box.max.x,
                    nearest_data.bounding_box.max.y
                ));
                debug_info.push_str(&format!("   📍 Distance: {:.1}px\n", nearest_distance));

                let intersects = !query_rect.intersect(nearest_data.bounding_box).is_empty();
                debug_info.push_str(&format!("   🔄 Intersects with query: {}\n", intersects));

                let found_in_results = results.contains(&nearest_id);
                debug_info.push_str(&format!("   ✅ Found in results: {}\n", found_in_results));

                if intersects && !found_in_results {
                    debug_info.push_str("   ❌ ERROR: Should be found but missing from results!\n");
                    debug_info.push_str("   🚨 This indicates an R-tree query issue!\n");
                }
            }
        }

        (results, debug_info)
    }

    /// デバッグ用の詳細レイキャスト
    pub fn ray_cast_debug(
        &mut self,
        ray_origin: Vec2,
        ray_direction: Vec2,
    ) -> (Option<PieceId>, String) {
        println!(
            "🔍 Ray cast debug: origin={:?}, direction={:?}",
            ray_origin, ray_direction
        );
        println!("🔍 Collision system has {} pieces", self.pieces.len());

        if self.pieces.is_empty() {
            return (None, "No pieces in collision system".to_string());
        }

        // R-treeの状態確認
        if self.rtree.size() == 0 || self.need_rebuild {
            println!("⚠️ R-tree not initialized, rebuilding...");
            self.rebuild_rtree();
        }

        // 1. 詳細なポイント検索（100px範囲）
        let (result, detailed_debug) = self.find_piece_at_position_debug(ray_origin);

        // 2. 最も近いピース検索（1000px範囲）
        let nearest = self.find_nearest_piece(ray_origin, 1000.0);

        // 3. 広範囲検索（500px範囲）
        let large_area_pieces = self.find_pieces_in_large_area(ray_origin, 500.0);

        // 4. 精密形状判定での検索
        let precise_hits = self.find_pieces_with_precise_hit(ray_origin, 100.0);

        // 4. ピース位置の範囲分析
        let mut min_pos = Vec2::new(f32::INFINITY, f32::INFINITY);
        let mut max_pos = Vec2::new(f32::NEG_INFINITY, f32::NEG_INFINITY);

        for piece_data in self.pieces.values() {
            min_pos.x = min_pos.x.min(piece_data.position.x);
            min_pos.y = min_pos.y.min(piece_data.position.y);
            max_pos.x = max_pos.x.max(piece_data.position.x);
            max_pos.y = max_pos.y.max(piece_data.position.y);
        }

        let debug_info = format!(
            "📍 Direct search (100px): {:?}\n📍 Nearest piece: {:?}\n📍 Large area (500px): {} pieces\n📍 Precise hits (100px): {} pieces\n📊 Piece position range: ({:.1}, {:.1}) to ({:.1}, {:.1})\n📏 Cursor distance from center: {:.1}px\n\n🔍 Detailed search:\n{}",
            result,
            nearest,
            large_area_pieces.len(),
            precise_hits.len(),
            min_pos.x, min_pos.y, max_pos.x, max_pos.y,
            ray_origin.distance(Vec2::ZERO),
            detailed_debug
        );

        (result, debug_info)
    }

    /// マウス位置でのピース検索（精密な形状判定付き）
    pub fn find_piece_at_position(&mut self, position: Vec2) -> Option<PieceId> {
        if !position.is_finite() {
            return None;
        }
        self.query_pieces_in_rect(Rect {
            min: position,
            max: position,
        })
        .into_iter()
        .filter(|&id| self.precise_point_in_piece(id, position))
        .max_by(|a, b| {
            self.pieces[a]
                .z_order
                .total_cmp(&self.pieces[b].z_order)
                .then(a.cmp(b))
        })
    }

    /// 矩形と適切に交差するピースを検索（PieceIdベース・Entity不要）
    /// より厳密な矩形交差判定（選択矩形の辺またはピースの境界頂点との交差をチェック）
    pub fn find_pieces_with_detailed_rect_intersection(
        &mut self,
        selection_rect: Rect,
    ) -> Vec<PieceId> {
        let candidate_pieces = self.query_pieces_in_rect(selection_rect);
        let mut intersecting_pieces = Vec::new();

        for piece_id in candidate_pieces {
            if let Some(piece_data) = self.pieces.get(&piece_id) {
                // 1. バウンディングボックスの基本交差チェック
                if !self.rect_intersects_bbox(selection_rect, piece_data.bounding_box) {
                    continue;
                }

                // 2. より詳細な交差判定
                if self.detailed_rect_piece_intersection(selection_rect, piece_id) {
                    intersecting_pieces.push(piece_id);
                }
            }
        }

        intersecting_pieces
    }

    /// 矩形と矩形（バウンディングボックス）の交差判定
    fn rect_intersects_bbox(&self, rect1: Rect, rect2: Rect) -> bool {
        rect1.min.x <= rect2.max.x
            && rect1.max.x >= rect2.min.x
            && rect1.min.y <= rect2.max.y
            && rect1.max.y >= rect2.min.y
    }

    /// 選択矩形とピースの詳細交差判定（複数の判定方法を組み合わせ）
    fn detailed_rect_piece_intersection(&self, selection_rect: Rect, piece_id: PieceId) -> bool {
        self.pieces.get(&piece_id).is_some_and(|piece| {
            let local_rect = Rect {
                min: selection_rect.min - piece.position,
                max: selection_rect.max - piece.position,
            };
            crate::piece_geometry::triangles(&piece.vertices, &piece.indices).any(|triangle| {
                crate::piece_geometry::triangle_intersects_rect(triangle, local_rect)
            })
        })
    }

    /// デバッグ用の詳細な位置検索（各段階の結果を表示）
    pub fn find_piece_at_position_debug(&mut self, position: Vec2) -> (Option<PieceId>, String) {
        if !position.is_finite() {
            return (None, "Invalid pointer position".into());
        }
        let query_size = 100.0;
        let query_rect = Rect::new(
            position.x - query_size,
            position.y - query_size,
            position.x + query_size,
            position.y + query_size,
        );

        // R-treeデバッグクエリを実行
        let (mut candidate_pieces, rtree_debug) = self.query_pieces_in_rect_debug(query_rect);
        candidate_pieces.sort_by(|a, b| {
            self.pieces[b]
                .z_order
                .total_cmp(&self.pieces[a].z_order)
                .then(b.cmp(a))
        });

        let mut debug_info = format!("🔍 Cursor at: ({:.1}, {:.1})\n", position.x, position.y);
        debug_info.push_str(&format!(
            "📦 R-tree candidates: {} pieces\n\n",
            candidate_pieces.len()
        ));

        // R-treeの詳細ログを追加
        debug_info.push_str(&rtree_debug);
        debug_info.push_str("\n📋 Candidate piece analysis:\n");

        for (i, piece_id) in candidate_pieces.iter().enumerate() {
            if let Some(piece_data) = self.pieces.get(piece_id) {
                let bbox_contains = piece_data.bounding_box.contains(position);
                let distance = position.distance(piece_data.position);

                debug_info.push_str(&format!(
                    "  {}. {} (distance: {:.1}px)\n",
                    i + 1,
                    piece_id,
                    distance
                ));

                // バウンディングボックスの詳細情報を表示
                debug_info.push_str(&format!(
                    "     📦 BBox: ({:.1}, {:.1}) to ({:.1}, {:.1}) [{}x{}]\n",
                    piece_data.bounding_box.min.x,
                    piece_data.bounding_box.min.y,
                    piece_data.bounding_box.max.x,
                    piece_data.bounding_box.max.y,
                    piece_data.bounding_box.width() as i32,
                    piece_data.bounding_box.height() as i32
                ));

                // ピースの中心位置
                debug_info.push_str(&format!(
                    "     📍 Piece center: ({:.1}, {:.1})\n",
                    piece_data.position.x, piece_data.position.y
                ));

                // カーソルとバウンディングボックスの各辺との距離
                let cursor_to_left = position.x - piece_data.bounding_box.min.x;
                let cursor_to_right = piece_data.bounding_box.max.x - position.x;
                let cursor_to_bottom = position.y - piece_data.bounding_box.min.y;
                let cursor_to_top = piece_data.bounding_box.max.y - position.y;

                debug_info.push_str(&format!(
                    "     📏 Cursor to bbox edges: L:{:.1} R:{:.1} B:{:.1} T:{:.1}\n",
                    cursor_to_left, cursor_to_right, cursor_to_bottom, cursor_to_top
                ));

                debug_info.push_str(&format!(
                    "     ✅ BBox contains cursor: {}\n",
                    bbox_contains
                ));

                if bbox_contains {
                    // 詳細なポリゴン判定を実行
                    let (precise_hit, polygon_debug) =
                        self.precise_point_in_piece_debug(*piece_id, position);
                    debug_info.push_str(&format!("     🎯 Precise polygon hit: {}\n", precise_hit));

                    // 詳細なポリゴンデバッグ情報を追加
                    debug_info.push_str("     🔬 Detailed polygon analysis:\n");
                    for line in polygon_debug.lines() {
                        debug_info.push_str(&format!("       {}\n", line));
                    }

                    if precise_hit {
                        debug_info.push_str("     🎉 FOUND PIECE!\n");
                        return (Some(*piece_id), debug_info);
                    } else {
                        debug_info.push_str(
                            "     ⚠️ Inside bbox but outside polygon - CHECK VERTEX DATA!\n",
                        );
                    }
                } else {
                    debug_info.push_str("     ❌ Outside bounding box - precise test skipped\n");
                }

                debug_info.push('\n');
            }
        }

        // 最も近いピースの情報も表示
        if let Some((nearest_id, nearest_distance)) = self.find_nearest_piece(position, 500.0) {
            debug_info.push_str(&format!(
                "\n🎯 Nearest piece within 500px: {} (distance: {:.1}px)\n",
                nearest_id, nearest_distance
            ));

            if let Some(nearest_data) = self.pieces.get(&nearest_id) {
                debug_info.push_str(&format!(
                    "   📦 Nearest BBox: ({:.1}, {:.1}) to ({:.1}, {:.1})\n",
                    nearest_data.bounding_box.min.x,
                    nearest_data.bounding_box.min.y,
                    nearest_data.bounding_box.max.x,
                    nearest_data.bounding_box.max.y
                ));
                debug_info.push_str(&format!(
                    "   📍 Nearest center: ({:.1}, {:.1})\n",
                    nearest_data.position.x, nearest_data.position.y
                ));
            }
        }

        (None, debug_info)
    }

    /// 最も近いピースを検索（緊急対処用）
    pub fn find_nearest_piece(&self, position: Vec2, max_distance: f32) -> Option<(PieceId, f32)> {
        let mut nearest_piece = None;
        let mut nearest_distance = max_distance;

        for (piece_id, piece_data) in &self.pieces {
            let distance = position.distance(piece_data.position);
            if distance < nearest_distance {
                nearest_distance = distance;
                nearest_piece = Some(*piece_id);
            }
        }

        nearest_piece.map(|id| (id, nearest_distance))
    }

    /// 広範囲検索（精密判定付き）
    pub fn find_pieces_in_large_area(
        &mut self,
        position: Vec2,
        radius: f32,
    ) -> Vec<(PieceId, f32)> {
        let query_rect = Rect::new(
            position.x - radius,
            position.y - radius,
            position.x + radius,
            position.y + radius,
        );

        let candidate_pieces = self.query_pieces_in_rect(query_rect);
        let mut results = Vec::new();

        for piece_id in candidate_pieces {
            if let Some(piece_data) = self.pieces.get(&piece_id) {
                let distance = position.distance(piece_data.position);
                if distance <= radius {
                    results.push((piece_id, distance));
                }
            }
        }

        // 距離順にソート
        results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        results
    }

    /// 広範囲検索（精密形状判定付き）- 実際にピース形状内にあるもののみ
    pub fn find_pieces_with_precise_hit(
        &mut self,
        position: Vec2,
        radius: f32,
    ) -> Vec<(PieceId, f32)> {
        let query_rect = Rect::new(
            position.x - radius,
            position.y - radius,
            position.x + radius,
            position.y + radius,
        );

        let candidate_pieces = self.query_pieces_in_rect(query_rect);
        let mut results = Vec::new();

        for piece_id in candidate_pieces {
            if let Some(piece_data) = self.pieces.get(&piece_id) {
                let distance = position.distance(piece_data.position);
                if distance <= radius {
                    // バウンディングボックス内かつ精密判定通過のもののみ
                    if piece_data.bounding_box.contains(position)
                        && self.precise_point_in_piece(piece_id, position)
                    {
                        results.push((piece_id, distance));
                    }
                }
            }
        }

        // 距離順にソート
        results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        results
    }

    /// 矩形範囲内のピース検索（範囲選択用API）
    pub fn find_pieces_in_rect(&mut self, rect: Rect) -> Vec<PieceId> {
        let candidate_pieces = self.query_pieces_in_rect(rect);
        let mut result = Vec::new();

        for piece_id in candidate_pieces {
            if let Some(piece_data) = self.pieces.get(&piece_id) {
                // バウンディングボックスが範囲と重なるかチェック
                if !piece_data.bounding_box.intersect(rect).is_empty() {
                    result.push(piece_id);
                }
            }
        }

        result
    }

    /// 精密なポリゴン当たり判定API
    pub fn precise_point_in_piece(&self, piece_id: PieceId, world_position: Vec2) -> bool {
        if let Some(piece_data) = self.pieces.get(&piece_id) {
            // まずバウンディングボックスでチェック
            if !piece_data.bounding_box.contains(world_position) {
                return false;
            }

            // ローカル座標に変換
            let local_position = world_position - piece_data.position;

            crate::piece_geometry::triangles(&piece_data.vertices, &piece_data.indices).any(
                |triangle| crate::piece_geometry::triangle_contains_point(triangle, local_position),
            )
        } else {
            false
        }
    }

    /// 詳細デバッグ付きの精密ポリゴン当たり判定
    pub fn precise_point_in_piece_debug(
        &self,
        piece_id: PieceId,
        world_position: Vec2,
    ) -> (bool, String) {
        if let Some(piece_data) = self.pieces.get(&piece_id) {
            let mut debug_info = format!("🔍 Precise collision debug for piece {}:\n", piece_id);

            // バウンディングボックスチェック
            let bbox_contains = piece_data.bounding_box.contains(world_position);
            debug_info.push_str(&format!("   📦 BBox contains point: {}\n", bbox_contains));
            debug_info.push_str(&format!(
                "   📦 BBox: ({:.1}, {:.1}) to ({:.1}, {:.1})\n",
                piece_data.bounding_box.min.x,
                piece_data.bounding_box.min.y,
                piece_data.bounding_box.max.x,
                piece_data.bounding_box.max.y
            ));
            debug_info.push_str(&format!(
                "   🌍 World position: ({:.1}, {:.1})\n",
                world_position.x, world_position.y
            ));
            debug_info.push_str(&format!(
                "   📍 Piece center: ({:.1}, {:.1})\n",
                piece_data.position.x, piece_data.position.y
            ));

            if !bbox_contains {
                debug_info.push_str("   ❌ Outside bounding box - skipping polygon test\n");
                return (false, debug_info);
            }

            // ローカル座標に変換
            let local_position = world_position - piece_data.position;
            debug_info.push_str(&format!(
                "   📐 Local position: ({:.1}, {:.1})\n",
                local_position.x, local_position.y
            ));

            // ポリゴンデータの詳細情報
            debug_info.push_str(&format!(
                "   🔺 Vertex count: {}\n",
                piece_data.vertices.len()
            ));
            debug_info.push_str(&format!(
                "   🔺 Index count: {}\n",
                piece_data.indices.len()
            ));

            // 最初の数個の頂点を表示
            if !piece_data.vertices.is_empty() {
                debug_info.push_str("   🔺 First 5 vertices (local coords):\n");
                for (i, vertex) in piece_data.vertices.iter().take(5).enumerate() {
                    debug_info.push_str(&format!(
                        "      {}. ({:.1}, {:.1})\n",
                        i, vertex.x, vertex.y
                    ));
                }
                if piece_data.vertices.len() > 5 {
                    debug_info.push_str(&format!(
                        "      ... and {} more\n",
                        piece_data.vertices.len() - 5
                    ));
                }
            }

            // 頂点の範囲をチェック
            if !piece_data.vertices.is_empty() {
                let mut min_v = piece_data.vertices[0];
                let mut max_v = piece_data.vertices[0];
                for vertex in &piece_data.vertices {
                    min_v.x = min_v.x.min(vertex.x);
                    min_v.y = min_v.y.min(vertex.y);
                    max_v.x = max_v.x.max(vertex.x);
                    max_v.y = max_v.y.max(vertex.y);
                }
                debug_info.push_str(&format!(
                    "   📏 Vertex bounds: ({:.1}, {:.1}) to ({:.1}, {:.1})\n",
                    min_v.x, min_v.y, max_v.x, max_v.y
                ));

                // ローカル座標が頂点範囲内にあるかチェック
                let in_vertex_bounds = local_position.x >= min_v.x
                    && local_position.x <= max_v.x
                    && local_position.y >= min_v.y
                    && local_position.y <= max_v.y;
                debug_info.push_str(&format!(
                    "   📏 Local point in vertex bounds: {}\n",
                    in_vertex_bounds
                ));
            }

            // ポリゴン内判定を実行
            let result = self.precise_point_in_piece(piece_id, world_position);
            debug_info.push_str(&format!("   Indexed triangle hit: {result}\n"));

            (result, debug_info)
        } else {
            (
                false,
                format!("❌ Piece {} not found in collision system\n", piece_id),
            )
        }
    }

    /// パフォーマンス統計取得
    pub fn get_performance_stats(&self) -> String {
        let rtree_size = self.rtree.size();

        format!(
            "Collision System Stats:\n  Pieces: {}\n  R-tree size: {}\n  Needs rebuild: {}",
            self.pieces.len(),
            rtree_size,
            self.need_rebuild
        )
    }
}

/// ストロークメッシュのキャッシュリソース
#[derive(Resource, Default)]
pub struct StrokeMeshCache {
    pub stroke_meshes: HashMap<String, Handle<Mesh>>, // shape_hash -> stroke mesh handle
}

/// ハイライト表示用の共有マテリアルリソース
#[derive(Resource)]
pub struct HighlightMaterials {
    pub preview_material: Handle<ColorMaterial>, // プレビュー用（薄い青色）
    pub selected_material: Handle<ColorMaterial>, // 選択用（黄色）
}

/// ハイライト状態変更検出リソース
#[derive(Resource, Default)]
pub struct HighlightState {
    pub last_selected_pieces: HashSet<Entity>, // 前フレームの選択ピース
    pub last_preview_pieces: HashSet<Entity>,  // 前フレームのプレビューピース
}

/// Local presentation mapping, never part of a network snapshot.
#[derive(Resource, Default)]
pub struct PieceIdManager {
    id_to_entity: HashMap<PieceId, Entity>,
    entity_to_id: HashMap<Entity, PieceId>,
}
impl PieceIdManager {
    pub fn len(&self) -> usize {
        self.id_to_entity.len()
    }
    pub fn register_piece(&mut self, entity: Entity, id: PieceId) {
        self.id_to_entity.insert(id, entity);
        self.entity_to_id.insert(entity, id);
    }
    pub fn unregister_entity(&mut self, entity: Entity) {
        if let Some(id) = self.entity_to_id.remove(&entity) {
            self.id_to_entity.remove(&id);
        }
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlayerInfo {
    pub id: PlayerId,
    pub name: String,
    pub score: u32,
}

#[derive(Clone, Copy, PartialEq, Default)]
pub enum PieceMode {
    #[default]
    TargetCount, // 目標ピース数から計算
    ManualGrid,   // 手動でグリッドサイズを指定
    SquarePieces, // 正方形ピースサイズから計算
}

#[derive(Resource)]
pub struct PuzzleConfig {
    pub grid_size: (usize, usize),
    pub snap_distance: f32,
    pub image_path: String,
    pub target_piece_count: usize,
    pub seed: u64,
    pub piece_mode: PieceMode,  // 新しいモード選択
    pub target_piece_size: f32, // 正方形ピースの目標サイズ（ピクセル）
}

impl Default for PuzzleConfig {
    fn default() -> Self {
        Self {
            grid_size: (4, 4),
            snap_distance: 50.0,       // Reduced to prevent immediate snapping
            image_path: String::new(), // 空の文字列から開始
            target_piece_count: 16,    // デフォルト16ピース
            seed: 42,
            piece_mode: PieceMode::SquarePieces, // アスペクト比モードをデフォルトに
            target_piece_size: 4.0,              // 4x4グリッド相当（16ピース）
        }
    }
}

#[derive(Resource)]
pub struct PuzzleImage {
    pub handle: Handle<Image>,
    pub size: Vec2,
}

/// Sampled pointer coordinates and local camera state.
#[derive(Resource, Default)]
pub struct InputState {
    pub mouse_position: Option<Vec2>,
    pub window_focused: bool,
    pub is_camera_dragging: bool,
    pub last_cursor_position: Option<Vec2>,
    pub cursor_screen_position: Option<Vec2>,
}

/// The HUD is built on a separate background Ui, so its rectangle must be
/// captured explicitly as well as egui's normal window/widget capture.
#[derive(Resource, Default)]
pub struct GameUiPointerCapture {
    pub over_hud: bool,
}

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum GenerationPhase {
    #[default]
    NotStarted,
    PreparingShapes,  // ジグソー形状を生成中（非同期）
    CreatingPieces,   // ピースデータを作成中（非同期）
    SpawningEntities, // メインスレッドでデータストアに保存中（エンティティは作成しない）
    Completed,
    Failed,
}

// 非同期タスクの結果を格納する構造体
pub struct ShapeGenerationResult {
    pub shape_generator: JigsawShapeGenerator,
    pub placement_positions: Vec<Vec2>,
    pub total_pieces: usize,
}

// メッシュとピースデータを含む構造体
pub struct PieceData {
    pub mesh: Mesh,
    pub stroke_mesh: Option<Mesh>, // ストロークメッシュ
    pub piece_component: PuzzlePiece,
    pub state: PieceState,
    pub piece_shape: PieceShape,
    pub bounds: Rect,
}

// ピース作成の非同期タスク結果
pub struct PieceCreationResult {
    pub pieces: Vec<PieceData>,
}

#[derive(Resource)]
pub struct PieceGenerationProgress {
    // バックグラウンドスレッド版 - フィールドテスト
    pub is_generating: bool,
    pub total_pieces: usize,
    pub generation_phase: GenerationPhase,
    pub grid_size: (usize, usize),
    pub shapes_generated: usize,
    pub pieces_created: usize,
    pub pending_pieces: VecDeque<PieceData>, // 非同期で作成されたピースデータの待機列
    pub pieces_spawned_this_frame: usize,    // 今フレームでスポーンしたピース数

    // 新しい標準スレッド用フィールド（crossbeam channelを使用）
    pub bg_thread_receiver: Option<channel::Receiver<Result<ShapeGenerationResult, String>>>,
    pub piece_thread_receiver: Option<channel::Receiver<Result<PieceCreationResult, String>>>,
    pub error: Option<String>,
}

impl Default for PieceGenerationProgress {
    fn default() -> Self {
        Self {
            is_generating: false,
            total_pieces: 0,
            generation_phase: GenerationPhase::NotStarted,
            grid_size: (0, 0),
            shapes_generated: 0,
            pieces_created: 0,
            pending_pieces: VecDeque::new(),
            pieces_spawned_this_frame: 0,
            bg_thread_receiver: None,
            piece_thread_receiver: None,
            error: None,
        }
    }
}

/// パフォーマンス計測のデバッグレベル
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum PerformanceDebugLevel {
    #[default]
    Off,
    Low,    // 基本的な統計のみ
    Medium, // 個別システムの時間
    High,   // 詳細な内部計測
}

/// 個別システムの計測データ
#[derive(Debug, Clone)]
pub struct SystemTiming {
    pub last_duration: std::time::Duration,
    pub total_duration: std::time::Duration,
    pub call_count: u64,
    pub min_duration: std::time::Duration,
    pub max_duration: std::time::Duration,
}

impl SystemTiming {
    pub fn new() -> Self {
        Self {
            last_duration: std::time::Duration::ZERO,
            total_duration: std::time::Duration::ZERO,
            call_count: 0,
            min_duration: std::time::Duration::MAX,
            max_duration: std::time::Duration::ZERO,
        }
    }

    pub fn record_timing(&mut self, duration: std::time::Duration) {
        self.last_duration = duration;
        self.total_duration += duration;
        self.call_count += 1;
        self.min_duration = self.min_duration.min(duration);
        self.max_duration = self.max_duration.max(duration);
    }

    pub fn average_duration(&self) -> std::time::Duration {
        if self.call_count > 0 {
            self.total_duration / self.call_count as u32
        } else {
            std::time::Duration::ZERO
        }
    }
}

/// パフォーマンス監視リソース
#[derive(Resource)]
pub struct PerformanceMonitor {
    pub debug_level: PerformanceDebugLevel,
    pub enabled: bool,
    pub frame_start: Option<Instant>,
    pub frame_times: Vec<std::time::Duration>,
    pub system_timings: HashMap<String, SystemTiming>,
    pub last_report_time: Instant,
    pub report_interval: std::time::Duration,
    pub frame_count: u64,
    pub max_stored_frames: usize,
}

impl Default for PerformanceMonitor {
    fn default() -> Self {
        Self {
            debug_level: PerformanceDebugLevel::Off,
            enabled: false,
            frame_start: None,
            frame_times: Vec::new(),
            system_timings: HashMap::new(),
            last_report_time: Instant::now(),
            report_interval: std::time::Duration::from_secs(5), // 5秒間隔でレポート
            frame_count: 0,
            max_stored_frames: 300, // 5秒分のフレーム（60FPS想定）
        }
    }
}

impl PerformanceMonitor {
    pub fn start_frame(&mut self) {
        if self.enabled {
            self.frame_start = Some(Instant::now());
        }
    }

    pub fn start_system_timing(&mut self, _system_name: &str) -> Option<Instant> {
        if self.enabled {
            Some(Instant::now())
        } else {
            None
        }
    }

    pub fn end_system_timing(&mut self, system_name: &str, start_time: Option<Instant>) {
        if self.enabled {
            if let Some(start) = start_time {
                let duration = start.elapsed();
                let timing = self
                    .system_timings
                    .entry(system_name.to_string())
                    .or_insert_with(SystemTiming::new);
                timing.record_timing(duration);
            }
        }
    }

    pub fn should_report(&self) -> bool {
        self.enabled && self.last_report_time.elapsed() >= self.report_interval
    }

    pub fn reset_report_timer(&mut self) {
        self.last_report_time = Instant::now();
    }

    pub fn get_fps(&self) -> f32 {
        if self.frame_times.is_empty() {
            return 0.0;
        }

        let total_time: std::time::Duration = self.frame_times.iter().sum();
        let avg_frame_time_nanos = total_time.as_nanos() / self.frame_times.len() as u128;

        if avg_frame_time_nanos > 0 {
            1_000_000_000.0 / avg_frame_time_nanos as f32
        } else {
            0.0
        }
    }

    pub fn get_frame_time_ms(&self) -> f32 {
        if self.frame_times.is_empty() {
            return 0.0;
        }

        let total_time: std::time::Duration = self.frame_times.iter().sum();
        let avg_frame_time_nanos = total_time.as_nanos() / self.frame_times.len() as u128;
        avg_frame_time_nanos as f32 / 1_000_000.0
    }

    pub fn toggle_debug_level(&mut self) {
        self.debug_level = match self.debug_level {
            PerformanceDebugLevel::Off => PerformanceDebugLevel::Low,
            PerformanceDebugLevel::Low => PerformanceDebugLevel::Medium,
            PerformanceDebugLevel::Medium => PerformanceDebugLevel::High,
            PerformanceDebugLevel::High => PerformanceDebugLevel::Off,
        };
        self.enabled = self.debug_level != PerformanceDebugLevel::Off;

        if self.enabled {
            println!("🔍 Performance monitoring enabled: {:?}", self.debug_level);
        } else {
            println!("🔍 Performance monitoring disabled");
        }
    }
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

// ==========================================
// Pure Batch System - Centralized Piece Data
// ==========================================

/// Canonical gameplay records plus separate, local presentation caches.
#[derive(Resource, Default)]
pub struct PieceDataStore {
    pub pieces: HashMap<PieceId, StoredPieceData>,
    pub transforms: HashMap<PieceId, Transform>,
    pub selected_pieces: HashSet<PieceId>,
    pub preview_pieces: HashSet<PieceId>,
    pub temporary_entities: HashMap<PieceId, Entity>,
    pub dirty_pieces: HashSet<PieceId>,
    pub held_pieces: HashSet<PieceId>,
    pub placed_pieces: HashSet<PieceId>,
    pub next_z_order: f32,
}

#[derive(Clone, Debug)]
pub struct StoredPieceData {
    pub definition: PuzzlePiece,
    pub state: PieceState,
    pub render: PieceRenderData,
}

/// Mesh, bounds, shape and handles are never included in gameplay snapshots.
#[derive(Clone, Debug)]
pub struct PieceRenderData {
    pub bounds: Rect,
    pub shape: PieceShapeData,
    pub mesh: Handle<Mesh>,
    pub material: Handle<ColorMaterial>,
}
#[derive(Clone, Debug)]
pub struct PieceShapeData {
    pub vertices: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub shape_hash: String,
}
impl PieceDataStore {
    pub fn add_piece(&mut self, piece: StoredPieceData) {
        let id = piece.definition.id;
        let z = id.0 as f32 * 0.001;
        self.next_z_order = self.next_z_order.max(z);
        self.transforms.insert(
            id,
            Transform::from_translation(piece.state.position.extend(z)),
        );
        self.pieces.insert(id, piece);
    }

    /// Keep stacking below overlays and inside the camera's depth range even
    /// after thousands of grabs. Returns whether existing batches need rebuilding.
    pub fn bring_piece_to_front(&mut self, id: PieceId) -> bool {
        let compacted = self.next_z_order >= 50.0;
        if compacted {
            let mut ids: Vec<_> = self
                .pieces
                .iter()
                .filter_map(|(&id, piece)| (!piece.state.placed).then_some(id))
                .collect();
            ids.sort_by(|a, b| {
                self.transforms[a]
                    .translation
                    .z
                    .total_cmp(&self.transforms[b].translation.z)
                    .then(a.cmp(b))
            });
            let step = 10.0 / (ids.len() + 1) as f32;
            for (index, id) in ids.into_iter().enumerate() {
                if let Some(transform) = self.transforms.get_mut(&id) {
                    transform.translation.z = (index + 1) as f32 * step;
                    self.dirty_pieces.insert(id);
                }
            }
            self.next_z_order = 10.0;
        }
        self.next_z_order += 0.1;
        if let Some(transform) = self.transforms.get_mut(&id) {
            transform.translation.z = self.next_z_order;
        }
        compacted
    }
}

// ==========================================
// Mesh Batching System Resources
// ==========================================

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
