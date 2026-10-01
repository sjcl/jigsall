use super::performance::PerformanceDebugLevel;
use bevy::prelude::*;
use puzzella_core::PieceId;
use rstar::{RTree, RTreeObject, AABB};
use std::collections::{HashMap, HashSet};

/// ピースの当たり判定データ（CPU側で管理）
#[cfg(any(test, feature = "cpu-picking-debug"))]
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
#[cfg(any(test, feature = "cpu-picking-debug"))]
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
#[cfg(any(test, feature = "cpu-picking-debug"))]
#[derive(Resource, Default)]
pub struct PieceCollisionSystem {
    pub pieces: HashMap<PieceId, PieceCollisionData>,
    pub rtree: RTree<PieceCollisionData>,
    pub dragging_pieces: HashSet<PieceId>, // ドラッグ中で当たり判定対象外のピース
    pub need_rebuild: bool,
}

#[cfg(any(test, feature = "cpu-picking-debug"))]
#[allow(dead_code)]
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
