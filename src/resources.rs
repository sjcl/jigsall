use bevy::prelude::*;
use bevy::sprite::ColorMaterial;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crossbeam::channel;
use std::collections::{HashMap, HashSet};
use crate::jigsaw_shapes::JigsawShapeGenerator;
use crate::components::{PuzzlePiece, PieceShape};
use instant::Instant;
use rstar::{RTree, RTreeObject, AABB};

/// ピースの一意識別子（将来的にバッチング対応）
pub type PieceId = Uuid;

/// ピースの当たり判定データ（CPU側で管理）
#[derive(Debug, Clone, PartialEq)]
pub struct PieceCollisionData {
    pub piece_id: PieceId,
    pub position: Vec2,
    pub bounding_box: Rect,
    pub vertices: Vec<Vec2>,    // 精密判定用のポリゴン頂点（ローカル座標）
    pub indices: Vec<u32>,     // トライアングル頂点インデックス
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
    pub dragging_pieces: HashSet<PieceId>,  // ドラッグ中で当たり判定対象外のピース
    pub need_rebuild: bool,
}

impl PieceCollisionSystem {
    pub fn new() -> Self {
        Self {
            pieces: HashMap::new(),
            rtree: RTree::new(),
            dragging_pieces: HashSet::new(),
            need_rebuild: true,
        }
    }

    pub fn add_piece(&mut self, collision_data: PieceCollisionData) {
        self.pieces.insert(collision_data.piece_id, collision_data);
        self.need_rebuild = true;
    }

    pub fn remove_piece(&mut self, piece_id: PieceId) {
        self.pieces.remove(&piece_id);
        self.need_rebuild = true;
    }

    pub fn update_piece_position(&mut self, piece_id: PieceId, new_position: Vec2) {
        if let Some(piece_data) = self.pieces.get_mut(&piece_id) {
            // HashMap内の位置データは常に更新（表示用データを維持）
            let offset = new_position - piece_data.position;
            piece_data.position = new_position;
            
            // バウンディングボックスを更新
            piece_data.bounding_box = Rect::new(
                piece_data.bounding_box.min.x + offset.x,
                piece_data.bounding_box.min.y + offset.y,
                piece_data.bounding_box.max.x + offset.x,
                piece_data.bounding_box.max.y + offset.y,
            );
            
            // ドラッグ中のピースはR-tree操作のみスキップ（当たり判定から除外）
            if !self.dragging_pieces.contains(&piece_id) {
                // 効率的な動的更新: 古いデータを削除 → 新しいデータを挿入（O(log n)）
                let old_data_for_rtree = PieceCollisionData {
                    piece_id,
                    position: new_position - offset, // 古い位置
                    bounding_box: Rect::new(
                        piece_data.bounding_box.min.x - offset.x,
                        piece_data.bounding_box.min.y - offset.y,
                        piece_data.bounding_box.max.x - offset.x,
                        piece_data.bounding_box.max.y - offset.y,
                    ),
                    vertices: piece_data.vertices.clone(),
                    indices: piece_data.indices.clone(),
                };
                
                self.rtree.remove(&old_data_for_rtree);
                self.rtree.insert(piece_data.clone());
            }
        }
    }

    pub fn rebuild_rtree(&mut self) {
        if !self.need_rebuild {
            return;
        }

        // R-treeを再構築 - ドラッグ中ピースを除外してbulk_loadを使用
        let pieces_vec: Vec<PieceCollisionData> = self.pieces.values()
            .filter(|piece_data| !self.dragging_pieces.contains(&piece_data.piece_id))
            .cloned()
            .collect();
        
        let pieces_count = pieces_vec.len();
        
        if !pieces_vec.is_empty() {
            self.rtree = RTree::bulk_load(pieces_vec);
        } else {
            self.rtree = RTree::new();
        }

        self.need_rebuild = false;
        // デバッグログは必要時のみ表示
        if pieces_count > 0 {
            println!("✅ R-tree rebuilt with {} pieces ({} dragging excluded)", 
                pieces_count, self.dragging_pieces.len());
        }
    }

    /// ドラッグ開始: ピースをR-treeから除外
    pub fn start_dragging_piece(&mut self, piece_id: PieceId) {
        if let Some(piece_data) = self.pieces.get(&piece_id) {
            // R-treeから削除
            self.rtree.remove(piece_data);
            // ドラッグ中リストに追加
            self.dragging_pieces.insert(piece_id);
            println!("🎯 Piece {} removed from R-tree (dragging started)", piece_id);
        }
    }

    /// ドラッグ終了: ピースをR-treeに再挿入
    pub fn stop_dragging_piece(&mut self, piece_id: PieceId) {
        if self.dragging_pieces.remove(&piece_id) {
            if let Some(piece_data) = self.pieces.get(&piece_id) {
                // R-treeに再挿入
                self.rtree.insert(piece_data.clone());
                println!("🎯 Piece {} re-inserted to R-tree (dragging stopped)", piece_id);
            }
        }
    }

    /// 複数ピースのドラッグ開始
    pub fn start_dragging_pieces(&mut self, piece_ids: &[PieceId]) {
        for &piece_id in piece_ids {
            self.start_dragging_piece(piece_id);
        }
    }

    /// 複数ピースのドラッグ終了
    pub fn stop_dragging_pieces(&mut self, piece_ids: &[PieceId]) {
        for &piece_id in piece_ids {
            self.stop_dragging_piece(piece_id);
        }
    }

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
        
        let results: Vec<PieceId> = self.rtree.locate_in_envelope_intersecting(&envelope)
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

        let mut debug_info = format!("🌳 R-tree Query Debug:\n");
        debug_info.push_str(&format!("📍 Query rect: ({:.1}, {:.1}) to ({:.1}, {:.1}) [{}x{}]\n", 
            query_rect.min.x, query_rect.min.y, query_rect.max.x, query_rect.max.y,
            query_rect.width() as i32, query_rect.height() as i32));
        
        // R-treeを使用して範囲内のピースを検索
        let envelope = rstar::AABB::from_corners(
            [query_rect.min.x, query_rect.min.y],
            [query_rect.max.x, query_rect.max.y],
        );
        
        let results: Vec<PieceId> = self.rtree.locate_in_envelope_intersecting(&envelope)
            .map(|piece_data| piece_data.piece_id)
            .collect();
        
        debug_info.push_str(&format!("🎯 R-tree found {} pieces\n", results.len()));
        debug_info.push_str(&format!("🌍 Total pieces in system: {}\n", self.pieces.len()));
        
        // 最も近いピースがなぜ見つからないのかをチェック
        if let Some((nearest_id, nearest_distance)) = self.find_nearest_piece(query_rect.center(), 500.0) {
            if let Some(nearest_data) = self.pieces.get(&nearest_id) {
                debug_info.push_str(&format!("\n🎯 Nearest piece analysis ({})\n", nearest_id));
                debug_info.push_str(&format!("   📦 Nearest BBox: ({:.1}, {:.1}) to ({:.1}, {:.1})\n",
                    nearest_data.bounding_box.min.x, nearest_data.bounding_box.min.y,
                    nearest_data.bounding_box.max.x, nearest_data.bounding_box.max.y));
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

    pub fn ray_cast(&mut self, ray_origin: Vec2, ray_direction: Vec2) -> Option<PieceId> {
        // より効率的なポイント検索を使用
        // レイキャストよりもマウス位置での直接検索の方が適している
        self.find_piece_at_position(ray_origin)
    }

    /// デバッグ用の詳細レイキャスト
    pub fn ray_cast_debug(&mut self, ray_origin: Vec2, ray_direction: Vec2) -> (Option<PieceId>, String) {
        println!("🔍 Ray cast debug: origin={:?}, direction={:?}", ray_origin, ray_direction);
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

    pub fn get_piece_data(&self, piece_id: PieceId) -> Option<&PieceCollisionData> {
        self.pieces.get(&piece_id)
    }

    /// マウス位置でのピース検索（精密な形状判定付き）
    pub fn find_piece_at_position(&mut self, position: Vec2) -> Option<PieceId> {
        // 大幅に拡大された範囲でクエリ（座標範囲問題の対処）
        let query_size = 100.0;
        let query_rect = Rect::new(
            position.x - query_size,
            position.y - query_size,
            position.x + query_size,
            position.y + query_size,
        );

        let candidate_pieces = self.query_pieces_in_rect(query_rect);

        // 2段階判定: バウンディングボックス → 精密ポリゴン判定
        for piece_id in candidate_pieces {
            if let Some(piece_data) = self.pieces.get(&piece_id) {
                // 1段階目: バウンディングボックスでの高速フィルタリング
                if piece_data.bounding_box.contains(position) {
                    // 2段階目: 精密なポリゴン内判定
                    if self.precise_point_in_piece(piece_id, position) {
                        return Some(piece_id);
                    }
                }
            }
        }

        None
    }

    /// 矩形と適切に交差するピースを検索（UUIDベース・Entity不要）
    pub fn find_pieces_intersecting_rect(&mut self, selection_rect: Rect) -> Vec<PieceId> {
        let candidate_pieces = self.query_pieces_in_rect(selection_rect);
        let mut intersecting_pieces = Vec::new();
        
        for piece_id in candidate_pieces {
            if let Some(piece_data) = self.pieces.get(&piece_id) {
                // バウンディングボックスが矩形と交差するかチェック
                if self.rect_intersects_bbox(selection_rect, piece_data.bounding_box) {
                    intersecting_pieces.push(piece_id);
                }
            }
        }
        
        intersecting_pieces
    }
    
    /// より厳密な矩形交差判定（選択矩形の辺またはピースの境界頂点との交差をチェック）
    pub fn find_pieces_with_detailed_rect_intersection(&mut self, selection_rect: Rect) -> Vec<PieceId> {
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
        rect1.min.x <= rect2.max.x && 
        rect1.max.x >= rect2.min.x &&
        rect1.min.y <= rect2.max.y && 
        rect1.max.y >= rect2.min.y
    }
    
    /// 選択矩形とピースの詳細交差判定（複数の判定方法を組み合わせ）
    fn detailed_rect_piece_intersection(&self, selection_rect: Rect, piece_id: PieceId) -> bool {
        if let Some(piece_data) = self.pieces.get(&piece_id) {
            // 方法1: 選択矩形の角がピース内にあるかチェック
            let corners = [
                Vec2::new(selection_rect.min.x, selection_rect.min.y),
                Vec2::new(selection_rect.max.x, selection_rect.min.y),
                Vec2::new(selection_rect.min.x, selection_rect.max.y),
                Vec2::new(selection_rect.max.x, selection_rect.max.y),
            ];
            
            for &corner in &corners {
                if self.precise_point_in_piece(piece_id, corner) {
                    return true;
                }
            }
            
            // 方法2: ピースの頂点が選択矩形内にあるかチェック
            for vertex in &piece_data.vertices {
                let world_vertex = Vec2::new(vertex.x, vertex.y) + piece_data.position;
                if selection_rect.contains(world_vertex) {
                    return true;
                }
            }
            
            // 方法3: ピースの中心が選択矩形内にあるかチェック
            if selection_rect.contains(piece_data.position) {
                return true;
            }
            
            // 方法4: バウンディングボックスの中心が選択矩形内にあるかチェック
            let bbox_center = Vec2::new(
                (piece_data.bounding_box.min.x + piece_data.bounding_box.max.x) / 2.0,
                (piece_data.bounding_box.min.y + piece_data.bounding_box.max.y) / 2.0,
            );
            if selection_rect.contains(bbox_center) {
                return true;
            }
        }
        
        false
    }

    /// デバッグ用の詳細な位置検索（各段階の結果を表示）
    pub fn find_piece_at_position_debug(&mut self, position: Vec2) -> (Option<PieceId>, String) {
        let query_size = 100.0;
        let query_rect = Rect::new(
            position.x - query_size,
            position.y - query_size,
            position.x + query_size,
            position.y + query_size,
        );

        // R-treeデバッグクエリを実行
        let (candidate_pieces, rtree_debug) = self.query_pieces_in_rect_debug(query_rect);
        
        let mut debug_info = format!("🔍 Cursor at: ({:.1}, {:.1})\n", position.x, position.y);
        debug_info.push_str(&format!("📦 R-tree candidates: {} pieces\n\n", candidate_pieces.len()));
        
        // R-treeの詳細ログを追加
        debug_info.push_str(&rtree_debug);
        debug_info.push_str("\n📋 Candidate piece analysis:\n");

        for (i, piece_id) in candidate_pieces.iter().enumerate() {
            if let Some(piece_data) = self.pieces.get(piece_id) {
                let bbox_contains = piece_data.bounding_box.contains(position);
                let distance = position.distance(piece_data.position);
                
                debug_info.push_str(&format!(
                    "  {}. {} (distance: {:.1}px)\n",
                    i + 1, piece_id, distance
                ));
                
                // バウンディングボックスの詳細情報を表示
                debug_info.push_str(&format!(
                    "     📦 BBox: ({:.1}, {:.1}) to ({:.1}, {:.1}) [{}x{}]\n",
                    piece_data.bounding_box.min.x, piece_data.bounding_box.min.y,
                    piece_data.bounding_box.max.x, piece_data.bounding_box.max.y,
                    piece_data.bounding_box.width() as i32, piece_data.bounding_box.height() as i32
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
                
                debug_info.push_str(&format!("     ✅ BBox contains cursor: {}\n", bbox_contains));
                
                if bbox_contains {
                    // 詳細なポリゴン判定を実行
                    let (precise_hit, polygon_debug) = self.precise_point_in_piece_debug(*piece_id, position);
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
                        debug_info.push_str("     ⚠️ Inside bbox but outside polygon - CHECK VERTEX DATA!\n");
                    }
                } else {
                    debug_info.push_str("     ❌ Outside bounding box - precise test skipped\n");
                }
                
                debug_info.push_str("\n");
            }
        }
        
        // 最も近いピースの情報も表示
        if let Some((nearest_id, nearest_distance)) = self.find_nearest_piece(position, 500.0) {
            debug_info.push_str(&format!("\n🎯 Nearest piece within 500px: {} (distance: {:.1}px)\n", nearest_id, nearest_distance));
            
            if let Some(nearest_data) = self.pieces.get(&nearest_id) {
                debug_info.push_str(&format!(
                    "   📦 Nearest BBox: ({:.1}, {:.1}) to ({:.1}, {:.1})\n",
                    nearest_data.bounding_box.min.x, nearest_data.bounding_box.min.y,
                    nearest_data.bounding_box.max.x, nearest_data.bounding_box.max.y
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
    pub fn find_pieces_in_large_area(&mut self, position: Vec2, radius: f32) -> Vec<(PieceId, f32)> {
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
    pub fn find_pieces_with_precise_hit(&mut self, position: Vec2, radius: f32) -> Vec<(PieceId, f32)> {
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
                    if piece_data.bounding_box.contains(position) {
                        if self.precise_point_in_piece(piece_id, position) {
                            results.push((piece_id, distance));
                        }
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

            // 簡単なポリゴン内判定（レイキャスト法）
            self.point_in_polygon(local_position, &piece_data.vertices)
        } else {
            false
        }
    }

    /// 詳細デバッグ付きの精密ポリゴン当たり判定
    pub fn precise_point_in_piece_debug(&self, piece_id: PieceId, world_position: Vec2) -> (bool, String) {
        if let Some(piece_data) = self.pieces.get(&piece_id) {
            let mut debug_info = format!("🔍 Precise collision debug for piece {}:\n", piece_id);
            
            // バウンディングボックスチェック
            let bbox_contains = piece_data.bounding_box.contains(world_position);
            debug_info.push_str(&format!("   📦 BBox contains point: {}\n", bbox_contains));
            debug_info.push_str(&format!("   📦 BBox: ({:.1}, {:.1}) to ({:.1}, {:.1})\n", 
                piece_data.bounding_box.min.x, piece_data.bounding_box.min.y,
                piece_data.bounding_box.max.x, piece_data.bounding_box.max.y));
            debug_info.push_str(&format!("   🌍 World position: ({:.1}, {:.1})\n", world_position.x, world_position.y));
            debug_info.push_str(&format!("   📍 Piece center: ({:.1}, {:.1})\n", piece_data.position.x, piece_data.position.y));
            
            if !bbox_contains {
                debug_info.push_str("   ❌ Outside bounding box - skipping polygon test\n");
                return (false, debug_info);
            }

            // ローカル座標に変換
            let local_position = world_position - piece_data.position;
            debug_info.push_str(&format!("   📐 Local position: ({:.1}, {:.1})\n", local_position.x, local_position.y));
            
            // ポリゴンデータの詳細情報
            debug_info.push_str(&format!("   🔺 Vertex count: {}\n", piece_data.vertices.len()));
            debug_info.push_str(&format!("   🔺 Index count: {}\n", piece_data.indices.len()));
            
            // 最初の数個の頂点を表示
            if !piece_data.vertices.is_empty() {
                debug_info.push_str("   🔺 First 5 vertices (local coords):\n");
                for (i, vertex) in piece_data.vertices.iter().take(5).enumerate() {
                    debug_info.push_str(&format!("      {}. ({:.1}, {:.1})\n", i, vertex.x, vertex.y));
                }
                if piece_data.vertices.len() > 5 {
                    debug_info.push_str(&format!("      ... and {} more\n", piece_data.vertices.len() - 5));
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
                debug_info.push_str(&format!("   📏 Vertex bounds: ({:.1}, {:.1}) to ({:.1}, {:.1})\n", 
                    min_v.x, min_v.y, max_v.x, max_v.y));
                
                // ローカル座標が頂点範囲内にあるかチェック
                let in_vertex_bounds = local_position.x >= min_v.x && local_position.x <= max_v.x &&
                                     local_position.y >= min_v.y && local_position.y <= max_v.y;
                debug_info.push_str(&format!("   📏 Local point in vertex bounds: {}\n", in_vertex_bounds));
            }

            // ポリゴン内判定を実行
            let (result, polygon_debug) = self.point_in_polygon_debug(local_position, &piece_data.vertices);
            debug_info.push_str(&polygon_debug);
            
            (result, debug_info)
        } else {
            (false, format!("❌ Piece {} not found in collision system\n", piece_id))
        }
    }

    /// ポリゴン内判定（レイキャスト法）
    fn point_in_polygon(&self, point: Vec2, vertices: &[Vec2]) -> bool {
        if vertices.len() < 3 {
            return false;
        }

        let mut intersections = 0;
        let ray_y = point.y;

        for i in 0..vertices.len() {
            let j = (i + 1) % vertices.len();
            let v1 = vertices[i];
            let v2 = vertices[j];

            // 水平レイが線分と交差するかチェック
            if ((v1.y > ray_y) != (v2.y > ray_y)) &&
               (point.x < (v2.x - v1.x) * (ray_y - v1.y) / (v2.y - v1.y) + v1.x) {
                intersections += 1;
            }
        }

        intersections % 2 == 1
    }

    /// デバッグ付きポリゴン内判定
    fn point_in_polygon_debug(&self, point: Vec2, vertices: &[Vec2]) -> (bool, String) {
        let mut debug_info = String::new();
        
        if vertices.len() < 3 {
            debug_info.push_str(&format!("   ❌ Too few vertices: {} (need at least 3)\n", vertices.len()));
            return (false, debug_info);
        }

        let mut intersections = 0;
        let ray_y = point.y;
        
        debug_info.push_str(&format!("   🎯 Ray casting from ({:.1}, {:.1}) horizontally (y={:.1})\n", 
            point.x, point.y, ray_y));
        
        let mut edge_details = Vec::new();
        
        for i in 0..vertices.len() {
            let j = (i + 1) % vertices.len();
            let v1 = vertices[i];
            let v2 = vertices[j];
            
            // エッジの詳細情報を収集
            let y_cross = (v1.y > ray_y) != (v2.y > ray_y);
            let intersection_x = if (v2.y - v1.y).abs() > f32::EPSILON {
                (v2.x - v1.x) * (ray_y - v1.y) / (v2.y - v1.y) + v1.x
            } else {
                f32::NAN  // 水平線
            };
            let x_cross = point.x < intersection_x;
            
            // 水平レイが線分と交差するかチェック
            if y_cross && x_cross && !intersection_x.is_nan() {
                intersections += 1;
                edge_details.push(format!("      Edge {}->{}: ({:.1},{:.1}) to ({:.1},{:.1}) → intersection at x={:.1} ✅", 
                    i, j, v1.x, v1.y, v2.x, v2.y, intersection_x));
            } else {
                if i < 5 || !edge_details.is_empty() {  // 最初の5個または交差がある場合のみ表示
                    let reason = if !y_cross {
                        "no Y crossing"
                    } else if intersection_x.is_nan() {
                        "horizontal edge"
                    } else if !x_cross {
                        "intersection behind point"
                    } else {
                        "unknown"
                    };
                    edge_details.push(format!("      Edge {}->{}: ({:.1},{:.1}) to ({:.1},{:.1}) → {} ❌", 
                        i, j, v1.x, v1.y, v2.x, v2.y, reason));
                }
            }
        }
        
        debug_info.push_str(&format!("   🔍 Testing {} edges:\n", vertices.len()));
        for detail in edge_details.iter().take(10) {  // 最大10個まで表示
            debug_info.push_str(&format!("{}\n", detail));
        }
        if edge_details.len() > 10 {
            debug_info.push_str(&format!("      ... and {} more edges\n", edge_details.len() - 10));
        }
        
        let result = intersections % 2 == 1;
        debug_info.push_str(&format!("   🎯 Total intersections: {} → Point is {} polygon\n", 
            intersections, if result { "INSIDE" } else { "OUTSIDE" }));
        
        (result, debug_info)
    }

    /// デバッグ用：統計情報取得
    pub fn get_debug_stats(&self) -> (usize, bool, bool) {
        (
            self.pieces.len(),
            self.rtree.size() > 0,
            self.need_rebuild
        )
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
    pub preview_material: Handle<ColorMaterial>,  // プレビュー用（薄い青色）
    pub selected_material: Handle<ColorMaterial>, // 選択用（黄色）
}

/// ハイライト状態変更検出リソース
#[derive(Resource, Default)]
pub struct HighlightState {
    pub last_selected_pieces: HashSet<Entity>,    // 前フレームの選択ピース
    pub last_preview_pieces: HashSet<Entity>,     // 前フレームのプレビューピース
    pub selection_changed: bool,                  // 選択状態が変わったか
    pub preview_changed: bool,                    // プレビュー状態が変わったか
    pub frame_count: u64,                         // フレーム数（デバッグ用）
}

/// パフォーマンス最適化用のピース検索キャッシュ
#[derive(Resource, Default)]
pub struct PieceSelectionCache {
    pub all_pieces: Vec<Entity>,  // 全ピースのキャッシュリスト
    pub piece_positions: HashMap<Entity, Vec2>,  // ピース位置のキャッシュ
    pub piece_bounds: HashMap<Entity, (Vec2, Vec2)>,  // ピース境界ボックスのキャッシュ
    pub need_refresh: bool,  // キャッシュ更新が必要か
}

/// ID管理とEntity関連付けシステム（将来的なバッチング対応）
#[derive(Resource, Default)]
pub struct PieceIdManager {
    /// ID → Entity の関連付け
    id_to_entity: HashMap<PieceId, Entity>,
    /// Entity → ID の関連付け（逆引き用）
    entity_to_id: HashMap<Entity, PieceId>,
    /// 次に使用可能なID（Uuidの代替案として、デバッグ用）
    next_id_counter: u32,
}

impl PieceIdManager {
    /// 新しいピースIDを生成してEntityと関連付け
    pub fn register_piece(&mut self, entity: Entity, existing_id: Option<PieceId>) -> PieceId {
        let piece_id = existing_id.unwrap_or_else(|| Uuid::new_v4());
        
        // 既存の関連付けを削除
        if let Some(old_id) = self.entity_to_id.remove(&entity) {
            self.id_to_entity.remove(&old_id);
        }
        
        // 新しい関連付けを登録
        self.id_to_entity.insert(piece_id, entity);
        self.entity_to_id.insert(entity, piece_id);
        
        piece_id
    }
    
    /// EntityからピースIDを取得
    pub fn get_piece_id(&self, entity: Entity) -> Option<PieceId> {
        self.entity_to_id.get(&entity).copied()
    }
    
    /// ピースIDからEntityを取得
    pub fn get_entity(&self, piece_id: PieceId) -> Option<Entity> {
        self.id_to_entity.get(&piece_id).copied()
    }
    
    /// Entity削除時のクリーンアップ
    pub fn unregister_entity(&mut self, entity: Entity) -> Option<PieceId> {
        if let Some(piece_id) = self.entity_to_id.remove(&entity) {
            self.id_to_entity.remove(&piece_id);
            Some(piece_id)
        } else {
            None
        }
    }
    
    /// 将来的なバッチング対応：複数ピースを1つのEntityに関連付け
    pub fn register_batch(&mut self, entity: Entity, piece_ids: Vec<PieceId>) {
        for piece_id in piece_ids {
            self.id_to_entity.insert(piece_id, entity);
            // 注意：entity_to_id は1対1のため、バッチング時は別のマップが必要
        }
    }
    
    /// 統計情報取得（デバッグ用）
    pub fn get_stats(&self) -> (usize, usize, u32) {
        (
            self.id_to_entity.len(),
            self.entity_to_id.len(),
            self.next_id_counter
        )
    }
    
    /// 全てのピースIDを取得
    pub fn get_all_piece_ids(&self) -> Vec<PieceId> {
        self.id_to_entity.keys().copied().collect()
    }
}

#[derive(Resource, Default)]
pub struct GameData {
    pub current_screen: GameScreen,  // 一時的に残す
    pub is_host: bool,
    pub players: Vec<PlayerInfo>,
    pub puzzle_completed: bool,
    pub puzzle_progress: f32,
    pub needs_reset: bool, // パズルをリセットする必要があるかのフラグ
}

/// メインアプリケーションの状態
#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AppState {
    #[default]
    Loading,      // 起動時の初期化
    Menu,         // メインメニュー
    GameSetup,    // ゲーム設定・画像読み込み
    InGame,       // ゲーム中
    GameComplete, // ゲーム完了
}

/// ゲーム内のサブ状態（InGame時のみ有効）
#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum GameSubState {
    #[default]
    Initializing, // パズル生成中
    Playing,      // プレイ中
    Paused,       // ポーズ中（ESCメニュー）
    // Complete is removed as it's unused
}

/// 従来のGameScreen（後で削除予定）
#[derive(Default, PartialEq, Clone, Debug)]
pub enum GameScreen {
    #[default]
    Menu,
    HostSetup,
    JoinGame,
    InGame,
    InGameMenu,  // ESCキーで表示されるゲーム内メニュー
    GameComplete,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlayerInfo {
    pub id: Uuid,
    pub name: String,
    pub score: u32,
}

#[derive(Clone, Copy, PartialEq)]
pub enum PieceMode {
    TargetCount,  // 目標ピース数から計算
    ManualGrid,   // 手動でグリッドサイズを指定
    SquarePieces, // 正方形ピースサイズから計算
}

impl Default for PieceMode {
    fn default() -> Self {
        PieceMode::TargetCount
    }
}

#[derive(Resource)]
pub struct PuzzleConfig {
    pub grid_size: (usize, usize),
    pub snap_distance: f32,
    pub image_path: String,
    pub target_piece_count: usize,
    pub use_target_mode: bool, // 下位互換性のため残す
    pub piece_mode: PieceMode, // 新しいモード選択
    pub target_piece_size: f32, // 正方形ピースの目標サイズ（ピクセル）
}

impl Default for PuzzleConfig {
    fn default() -> Self {
        Self {
            grid_size: (4, 4),
            snap_distance: 50.0, // Reduced to prevent immediate snapping
            image_path: String::new(), // 空の文字列から開始
            target_piece_count: 16, // デフォルト16ピース
            use_target_mode: false, // アスペクト比モードがデフォルト
            piece_mode: PieceMode::SquarePieces, // アスペクト比モードをデフォルトに
            target_piece_size: 4.0, // 4x4グリッド相当（16ピース）
        }
    }
}

#[derive(Resource)]
pub struct PuzzleImage {
    pub handle: Handle<Image>,
    pub size: Vec2,
}


#[derive(Resource, Default)]
pub struct NetworkInfo {
    pub server_address: String,
    pub port: u16,
}

#[derive(Default, Clone, Debug)]
pub enum SelectionMode {
    #[default]
    Single,        // 単一ピース選択モード
    BoxSelection,  // 範囲選択モード  
    MultiDrag,     // 複数ピース同時移動モード
}

#[derive(Resource)]
pub struct InputState {
    pub mouse_position: Vec2,
    pub is_mouse_pressed: bool,
    pub selected_piece: Option<Entity>,
    pub next_z_order: f32,
    pub is_camera_dragging: bool,
    pub last_mouse_position: Vec2,
    pub last_cursor_position: Option<Vec2>,
    
    // 新しいマルチ選択関連フィールド
    pub selection_mode: SelectionMode,
    pub selection_start: Option<Vec2>,
    pub selection_current: Option<Vec2>,
    pub selected_pieces: Vec<Entity>,
    pub multi_drag_offset: HashMap<Entity, Vec2>,
    
    // パフォーマンス最適化用のキャッシュ
    pub selected_pieces_set: HashSet<Entity>,  // 高速な選択状態チェック用
    pub last_selection_rect: Option<(Vec2, Vec2)>,  // 前回の選択範囲
    pub cached_drag_entity: Option<Entity>,  // ドラッグ中のエンティティキャッシュ
    
    // エッジスクロール用
    pub cursor_screen_position: Option<Vec2>,  // スクリーン座標でのカーソル位置
    pub is_dragging_piece: bool,  // ピースをドラッグ中かどうか
    
    // コリジョンシステム制御用
    pub is_any_piece_dragging: bool,  // 任意のピースがドラッグ中（コリジョンシステム自動更新を停止）
}

impl Default for InputState {
    fn default() -> Self {
        Self {
            mouse_position: Vec2::ZERO,
            is_mouse_pressed: false,
            selected_piece: None,
            next_z_order: 1.0, // 1.0から開始
            is_camera_dragging: false,
            last_mouse_position: Vec2::ZERO,
            last_cursor_position: None,
            
            // 新しいマルチ選択関連フィールドの初期化
            selection_mode: SelectionMode::Single,
            selection_start: None,
            selection_current: None,
            selected_pieces: Vec::new(),
            multi_drag_offset: HashMap::new(),
            
            // パフォーマンス最適化用のキャッシュの初期化
            selected_pieces_set: HashSet::new(),
            last_selection_rect: None,
            cached_drag_entity: None,
            
            // エッジスクロール用の初期化
            cursor_screen_position: None,
            is_dragging_piece: false,
            
            // コリジョンシステム制御用の初期化
            is_any_piece_dragging: false,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum GenerationPhase {
    NotStarted,
    PreparingShapes,    // ジグソー形状を生成中（非同期）
    CreatingPieces,     // ピースエンティティを作成中（非同期）
    SpawningEntities,   // メインスレッドでエンティティをスポーン中
    Completed,
}

// 非同期タスクの結果を格納する構造体
pub struct ShapeGenerationResult {
    pub shape_generator: JigsawShapeGenerator,
    pub placement_positions: Vec<Vec2>,
    pub grid_size: (usize, usize),
    pub total_pieces: usize,
}

// メッシュとピースデータを含む構造体
pub struct PieceData {
    pub mesh: Mesh,
    pub stroke_mesh: Option<Mesh>, // ストロークメッシュ
    pub piece_component: PuzzlePiece,
    pub piece_shape: PieceShape,
    pub transform: Transform,
}

// ピース作成の非同期タスク結果
pub struct PieceCreationResult {
    pub pieces: Vec<PieceData>,
}


impl Default for GenerationPhase {
    fn default() -> Self {
        GenerationPhase::NotStarted
    }
}

#[derive(Resource)]
pub struct PieceGenerationProgress {
    // バックグラウンドスレッド版 - フィールドテスト
    pub is_generating: bool,
    pub current_piece: usize,
    pub total_pieces: usize,
    pub generation_phase: GenerationPhase,
    pub grid_size: (usize, usize),
    pub shapes_generated: usize,
    pub pieces_created: usize,
    pub placement_positions: Vec<Vec2>,
    pub pending_pieces: Vec<PieceData>, // 非同期で作成されたピースデータの待機列
    pub pieces_spawned_this_frame: usize, // 今フレームでスポーンしたピース数
    
    // 新しい標準スレッド用フィールド（crossbeam channelを使用）
    pub bg_thread_receiver: Option<channel::Receiver<ShapeGenerationResult>>,
    pub piece_thread_receiver: Option<channel::Receiver<PieceCreationResult>>,
    pub progress_receiver: Option<channel::Receiver<()>>,
    
}

impl Default for PieceGenerationProgress {
    fn default() -> Self {
        Self {
            is_generating: false,
            current_piece: 0,
            total_pieces: 0,
            generation_phase: GenerationPhase::NotStarted,
            grid_size: (0, 0),
            shapes_generated: 0,
            pieces_created: 0,
            placement_positions: Vec::new(),
            pending_pieces: Vec::new(),
            pieces_spawned_this_frame: 0,
            bg_thread_receiver: None,
            piece_thread_receiver: None,
            progress_receiver: None,
        }
    }
}

/// パフォーマンス計測のデバッグレベル
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PerformanceDebugLevel {
    Off,
    Low,     // 基本的な統計のみ
    Medium,  // 個別システムの時間
    High,    // 詳細な内部計測
}

impl Default for PerformanceDebugLevel {
    fn default() -> Self {
        PerformanceDebugLevel::Off
    }
}

/// 個別システムの計測データ
#[derive(Debug, Clone)]
pub struct SystemTiming {
    pub name: String,
    pub last_duration: std::time::Duration,
    pub total_duration: std::time::Duration,
    pub call_count: u64,
    pub min_duration: std::time::Duration,
    pub max_duration: std::time::Duration,
}

impl SystemTiming {
    pub fn new(name: String) -> Self {
        Self {
            name,
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

    pub fn reset(&mut self) {
        self.total_duration = std::time::Duration::ZERO;
        self.call_count = 0;
        self.min_duration = std::time::Duration::MAX;
        self.max_duration = std::time::Duration::ZERO;
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
    pub fn new(debug_level: PerformanceDebugLevel) -> Self {
        Self {
            debug_level,
            enabled: debug_level != PerformanceDebugLevel::Off,
            ..Default::default()
        }
    }

    pub fn start_frame(&mut self) {
        if self.enabled {
            self.frame_start = Some(Instant::now());
        }
    }

    pub fn end_frame(&mut self) {
        if self.enabled {
            if let Some(start) = self.frame_start {
                let frame_duration = start.elapsed();
                self.frame_times.push(frame_duration);
                
                // 古いフレームデータを削除
                if self.frame_times.len() > self.max_stored_frames {
                    self.frame_times.remove(0);
                }
                
                self.frame_count += 1;
                self.frame_start = None;
            }
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
                let timing = self.system_timings
                    .entry(system_name.to_string())
                    .or_insert_with(|| SystemTiming::new(system_name.to_string()));
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

    // reset_statistics method removed - never used
}

// ImageLoadingStatus removed - unused with new thread-based implementation

// ImageLoadingTask removed - unused with new thread-based implementation

// ImageLoadingTasks removed - unused with new thread-based implementation


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

// バックグラウンドスレッド版の完了