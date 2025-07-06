use bevy::prelude::*;
use bevy::render::mesh::Indices;
use bevy::render::render_resource::PrimitiveTopology;
use bevy::render::render_asset::RenderAssetUsages;
use lyon::path::{Path, Builder};
use lyon::math;
use lyon_tessellation::{
    VertexBuffers, FillTessellator, FillOptions, FillRule, FillVertex,
    geometry_builder::BuffersBuilder, VertexId
};
use puzzle_paths::{build_jigsaw_template, JigsawTemplate};
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use svgtypes::{PathParser, PathSegment};

/// シンプルな頂点タイプ
#[derive(Copy, Clone, Debug)]
pub struct SimpleVertex {
    pub position: [f32; 2],
}

/// ジグソーピースの形状データを管理する構造体
pub struct JigsawPieceShape {
    pub mesh: Mesh,
    pub bounds: Rect,
    pub texture_coords: Vec4,
}

/// 共有メッシュマネージャー - 同じ形状のメッシュを共有
pub struct SharedMeshManager {
    /// 形状ハッシュ -> メッシュハンドルのマッピング
    mesh_handles: HashMap<u64, Handle<Mesh>>,
    /// 形状ハッシュ -> バウンディングボックスのマッピング  
    bounds_cache: HashMap<u64, Rect>,
    /// 統計情報
    total_shapes_generated: usize,
    unique_shapes_count: usize,
}

impl SharedMeshManager {
    pub fn new() -> Self {
        Self {
            mesh_handles: HashMap::new(),
            bounds_cache: HashMap::new(),
            total_shapes_generated: 0,
            unique_shapes_count: 0,
        }
    }
    
    /// 形状のハッシュ値を計算（形状パターンのみ、UV座標は除外）
    fn calculate_shape_hash_from_position(x: usize, y: usize, grid_width: usize, grid_height: usize) -> u64 {
        // ジグソーパズルの形状パターンを分類
        // エッジピース、コーナーピース、内部ピースで異なる形状
        let mut hasher = DefaultHasher::new();
        
        // ピースタイプを判定
        let is_top_edge = y == 0;
        let is_bottom_edge = y == grid_height - 1;
        let is_left_edge = x == 0;
        let is_right_edge = x == grid_width - 1;
        
        // 位置タイプをハッシュ化
        if is_top_edge && is_left_edge {
            "corner_top_left".hash(&mut hasher);
        } else if is_top_edge && is_right_edge {
            "corner_top_right".hash(&mut hasher);
        } else if is_bottom_edge && is_left_edge {
            "corner_bottom_left".hash(&mut hasher);
        } else if is_bottom_edge && is_right_edge {
            "corner_bottom_right".hash(&mut hasher);
        } else if is_top_edge {
            "edge_top".hash(&mut hasher);
            // 上端のパターン位置も考慮（タブ/ブランクのパターンが周期的）
            (x % 2).hash(&mut hasher);
        } else if is_bottom_edge {
            "edge_bottom".hash(&mut hasher);
            (x % 2).hash(&mut hasher);
        } else if is_left_edge {
            "edge_left".hash(&mut hasher);
            (y % 2).hash(&mut hasher);
        } else if is_right_edge {
            "edge_right".hash(&mut hasher);
            (y % 2).hash(&mut hasher);
        } else {
            // 内部ピース - 位置のパリティでパターンが決まる
            "internal".hash(&mut hasher);
            ((x + y) % 2).hash(&mut hasher);
        }
        
        hasher.finish()
    }
    
    /// ピースタイプの名前を取得（デバッグ用）
    fn get_piece_type_name(x: usize, y: usize, grid_width: usize, grid_height: usize) -> &'static str {
        let is_top_edge = y == 0;
        let is_bottom_edge = y == grid_height - 1;
        let is_left_edge = x == 0;
        let is_right_edge = x == grid_width - 1;
        
        if is_top_edge && is_left_edge {
            "corner_top_left"
        } else if is_top_edge && is_right_edge {
            "corner_top_right"
        } else if is_bottom_edge && is_left_edge {
            "corner_bottom_left"
        } else if is_bottom_edge && is_right_edge {
            "corner_bottom_right"
        } else if is_top_edge {
            if x % 2 == 0 { "edge_top_even" } else { "edge_top_odd" }
        } else if is_bottom_edge {
            if x % 2 == 0 { "edge_bottom_even" } else { "edge_bottom_odd" }
        } else if is_left_edge {
            if y % 2 == 0 { "edge_left_even" } else { "edge_left_odd" }
        } else if is_right_edge {
            if y % 2 == 0 { "edge_right_even" } else { "edge_right_odd" }
        } else {
            if (x + y) % 2 == 0 { "internal_even" } else { "internal_odd" }
        }
    }
    
    /// SVGパスを正規化（位置に依存しない形状の特徴を抽出）
    fn normalize_svg_path(svg_path: &str) -> String {
        // より単純なアプローチ: グリッド位置に基づいて形状パターンを推測
        // puzzle-pathsは規則的なパターンでピースを生成するため、
        // グリッド上の位置から形状タイプを推定できる
        
        // SVGパスからピースの境界座標を抽出
        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut coords = Vec::new();
        
        // 数値を抽出
        let parts: Vec<&str> = svg_path.split(|c: char| c.is_alphabetic() || c == ',' || c.is_whitespace())
            .filter(|s| !s.is_empty())
            .collect();
            
        for part in parts {
            if let Ok(val) = part.parse::<f32>() {
                coords.push(val);
                if coords.len() % 2 == 0 {
                    let x = coords[coords.len() - 2];
                    let y = coords[coords.len() - 1];
                    min_x = min_x.min(x);
                    min_y = min_y.min(y);
                }
            }
        }
        
        // 正規化: 最小座標を原点にシフト
        let mut normalized_parts = Vec::new();
        let mut cmd_iter = svg_path.chars();
        let mut current_pos = 0;
        
        while current_pos < svg_path.len() {
            // コマンドを探す
            while let Some(ch) = cmd_iter.next() {
                current_pos += ch.len_utf8();
                if ch.is_alphabetic() {
                    normalized_parts.push(ch.to_string());
                    break;
                }
            }
            
            // 数値を収集
            let mut number_str = String::new();
            let mut coord_count = 0;
            let mut coords_for_cmd = Vec::new();
            
            while let Some(ch) = cmd_iter.next() {
                current_pos += ch.len_utf8();
                if ch.is_alphabetic() {
                    // 次のコマンドに到達
                    if !number_str.is_empty() {
                        if let Ok(val) = number_str.parse::<f32>() {
                            coords_for_cmd.push(val);
                        }
                    }
                    // 座標を正規化して追加
                    for (i, coord) in coords_for_cmd.iter().enumerate() {
                        if i % 2 == 0 {
                            normalized_parts.push(format!("{:.1}", coord - min_x));
                        } else {
                            normalized_parts.push(format!("{:.1}", coord - min_y));
                        }
                    }
                    normalized_parts.push(ch.to_string());
                    break;
                } else if ch == ',' || ch.is_whitespace() {
                    if !number_str.is_empty() {
                        if let Ok(val) = number_str.parse::<f32>() {
                            coords_for_cmd.push(val);
                        }
                        number_str.clear();
                    }
                } else if ch == '-' || ch == '.' || ch.is_numeric() {
                    number_str.push(ch);
                }
            }
        }
        
        normalized_parts.join(" ")
    }
    
    /// メッシュハンドルを取得または作成
    pub fn get_or_create_mesh(
        &mut self, 
        svg_path: &str,
        piece_width: f32,
        piece_height: f32,
        x: usize,
        y: usize,
        meshes: &mut ResMut<Assets<Mesh>>,
        shape_generator: &JigsawShapeGenerator,
    ) -> (Handle<Mesh>, Rect) {
        // グリッドサイズを取得
        let (grid_width, grid_height) = shape_generator.grid_size;
        
        // 位置ベースのハッシュを計算
        let shape_hash = Self::calculate_shape_hash_from_position(x, y, grid_width, grid_height);
        self.total_shapes_generated += 1;
        
        // デバッグ: ピースタイプを表示
        if self.total_shapes_generated <= 20 || self.total_shapes_generated % 10 == 0 {
            let piece_type = Self::get_piece_type_name(x, y, grid_width, grid_height);
            println!("🧩 Piece ({},{}) type: {} (hash: {})", x, y, piece_type, shape_hash);
        }
        
        // 既に存在する場合は再利用
        if let Some(mesh_handle) = self.mesh_handles.get(&shape_hash) {
            println!("♻️  Reusing mesh for piece ({},{}) with hash {}", x, y, shape_hash);
            let bounds = self.bounds_cache.get(&shape_hash).copied()
                .unwrap_or_else(|| Rect::new(-piece_width/2.0, -piece_height/2.0, piece_width/2.0, piece_height/2.0));
            return (mesh_handle.clone(), bounds);
        }
        
        // 新しいメッシュを生成
        if let Ok(mesh) = shape_generator.create_mesh_from_svg(svg_path, x, y) {
            let bounds = shape_generator.calculate_bounds(&mesh);
            let mesh_handle = meshes.add(mesh);
            
            // キャッシュに保存
            self.mesh_handles.insert(shape_hash, mesh_handle.clone());
            self.bounds_cache.insert(shape_hash, bounds);
            self.unique_shapes_count += 1;
            
            println!("✨ Created new shared mesh (hash: {}), unique shapes: {}/{}", 
                shape_hash, self.unique_shapes_count, self.total_shapes_generated);
            
            (mesh_handle, bounds)
        } else {
            // フォールバック: 基本的な矩形メッシュ
            let fallback_mesh = shape_generator.create_fallback_rectangle_mesh(piece_width, piece_height);
            let bounds = Rect::new(-piece_width/2.0, -piece_height/2.0, piece_width/2.0, piece_height/2.0);
            let mesh_handle = meshes.add(fallback_mesh);
            
            self.mesh_handles.insert(shape_hash, mesh_handle.clone());
            self.bounds_cache.insert(shape_hash, bounds);
            self.unique_shapes_count += 1;
            
            (mesh_handle, bounds)
        }
    }
    
    /// 統計情報を出力
    pub fn print_stats(&self) {
        println!("🔧 Mesh Sharing Stats:");
        println!("   Total shapes generated: {}", self.total_shapes_generated);
        println!("   Unique shapes: {}", self.unique_shapes_count);
        println!("   Memory saved: {}% ({} duplicates avoided)", 
            if self.total_shapes_generated > 0 { 
                ((self.total_shapes_generated - self.unique_shapes_count) * 100) / self.total_shapes_generated 
            } else { 0 },
            self.total_shapes_generated - self.unique_shapes_count);
    }
}

/// ジグソーピース形状の生成とキャッシュを管理するシステム
pub struct JigsawShapeGenerator {
    piece_size: (f32, f32),
    grid_size: (usize, usize),
    pub jigsaw_template: Option<JigsawTemplate>,
    shape_cache: HashMap<(usize, usize), JigsawPieceShape>,
}

impl JigsawShapeGenerator {
    /// 新しいジグソー形状ジェネレータを作成
    pub fn new(piece_size: (f32, f32), grid_size: (usize, usize)) -> Self {
        Self {
            piece_size,
            grid_size,
            jigsaw_template: None,
            shape_cache: HashMap::new(),
        }
    }

    /// puzzle-pathsを使用してジグソーテンプレートを生成
    pub fn generate_jigsaw_template(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let (piece_width, piece_height) = self.piece_size;
        let (grid_width, grid_height) = self.grid_size;
        
        let template = build_jigsaw_template(
            piece_width * grid_width as f32,  // 画像全体の幅
            piece_height * grid_height as f32, // 画像全体の高さ
            grid_width,                       // 行のピース数（列数）
            grid_height,                      // 列のピース数（行数）
            None,                            // デフォルトのタブサイズ
            None,                            // デフォルトのジッター
            Some(42),                        // 固定シード
        );
        
        // デバッグ: 最初のいくつかのSVGパスを出力
        let total_pieces = grid_width * grid_height;
        println!("Generated jigsaw template for {}x{} grid ({} total pieces)", grid_width, grid_height, total_pieces);
        
        
        self.jigsaw_template = Some(template);
        Ok(())
    }

    /// 指定された位置のピース形状を生成
    pub fn generate_piece_shape(
        &mut self,
        x: usize,
        y: usize,
    ) -> Result<&JigsawPieceShape, Box<dyn std::error::Error>> {
        // キャッシュをチェック
        if self.shape_cache.contains_key(&(x, y)) {
            return Ok(self.shape_cache.get(&(x, y)).unwrap());
        }

        // テンプレートが生成されていない場合は生成
        if self.jigsaw_template.is_none() {
            self.generate_jigsaw_template()?;
        }

        let template = self.jigsaw_template.as_ref().unwrap();
        let (grid_width, grid_height) = self.grid_size;
        let piece_index = y * grid_width + x;

        // SVGパスを取得
        let svg_path = if piece_index < template.svg_paths.len() {
            &template.svg_paths[piece_index]
        } else {
            return Err(format!("Piece index {} out of bounds", piece_index).into());
        };
        

        // SVGパスをlyonのPathに変換してメッシュ生成
        let mesh = self.parse_svg_path_to_mesh(svg_path, x, y)?;
        
        // テクスチャ座標を計算
        let texture_coords = Vec4::new(
            x as f32 / grid_width as f32,
            y as f32 / grid_height as f32,
            (x + 1) as f32 / grid_width as f32,
            (y + 1) as f32 / grid_height as f32,
        );

        // バウンディングボックスをメッシュから計算
        let bounds = self.calculate_mesh_bounds(&mesh);

        let shape = JigsawPieceShape {
            mesh,
            bounds,
            texture_coords,
        };

        // キャッシュに保存
        self.shape_cache.insert((x, y), shape);
        
        println!("Generated shape for piece ({}, {})", x, y);
        Ok(self.shape_cache.get(&(x, y)).unwrap())
    }

    /// SVGパス文字列をlyonで解析してBevyメッシュに変換
    fn parse_svg_path_to_mesh(&self, svg_path: &str, x: usize, y: usize) -> Result<Mesh, Box<dyn std::error::Error>> {
        // puzzle-pathsが生成するSVGパスの座標系を理解する必要がある
        // SVGパス内の座標は、puzzle-pathsが想定する全体画像サイズに基づいている
        let (piece_width, piece_height) = self.piece_size;
        let (grid_width, grid_height) = self.grid_size;
        
        // puzzle-pathsに渡した全体画像サイズ
        let total_width = piece_width * grid_width as f32;
        let total_height = piece_height * grid_height as f32;
        
        // puzzle-pathsのSVGパス内での1ピースあたりのサイズ
        let svg_piece_width = total_width / grid_width as f32;
        let svg_piece_height = total_height / grid_height as f32;
        
        let offset_x = x as f32 * svg_piece_width + svg_piece_width / 2.0;
        let offset_y = y as f32 * svg_piece_height + svg_piece_height / 2.0;
        
        
        // SVGパス文字列をlyonのPathオブジェクトに変換（座標オフセット付き）
        let lyon_path = match self.parse_svg_path_to_lyon_with_offset(svg_path, offset_x, offset_y) {
            Ok(path) => path,
            Err(err) => {
                println!("Failed to parse SVG path for piece ({}, {}): {}. Using fallback rectangle.", x, y, err);
                // フォールバック: 矩形パスを作成
                let (piece_width, piece_height) = self.piece_size;
                let half_width = piece_width / 2.0;
                let half_height = piece_height / 2.0;
                
                {
                    let mut builder = Path::builder();
                    builder.begin(math::point(-half_width, half_height));
                    builder.line_to(math::point(half_width, half_height));
                    builder.line_to(math::point(half_width, -half_height));
                    builder.line_to(math::point(-half_width, -half_height));
                    builder.close();
                    builder.build()
                }
            }
        };
        
        
        // SimpleVertexを使用してテセレーション（より高精度設定）
        let mut vb: VertexBuffers<SimpleVertex, u16> = VertexBuffers::new();
        let fill_options = FillOptions::tolerance(0.1) // より細かいテッセレーション（デフォルトは0.25）
            .with_fill_rule(FillRule::NonZero);
        
        FillTessellator::new()
            .tessellate_path(
                &lyon_path,
                &fill_options,
                &mut BuffersBuilder::new(&mut vb, |v: FillVertex| SimpleVertex {
                    position: [v.position().x, v.position().y],
                }),
            )?;
        
        // バウンディングボックスを実際の頂点から計算
        let (min_x, max_x, min_y, max_y) = if vb.vertices.is_empty() {
            // フォールバック: 頂点がない場合は piece_size を使用
            let (piece_width, piece_height) = self.piece_size;
            (-piece_width / 2.0, piece_width / 2.0, -piece_height / 2.0, piece_height / 2.0)
        } else {
            vb.vertices.iter().fold(
                (f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::NEG_INFINITY),
                |(min_x, max_x, min_y, max_y), v| {
                    let x = v.position[0];
                    let y = v.position[1];
                    (min_x.min(x), max_x.max(x), min_y.min(y), max_y.max(y))
                }
            )
        };
        let width = max_x - min_x;
        let height = max_y - min_y;
        
        
        // Bevyメッシュを作成
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        );
        
        // 頂点位置を変換
        let positions: Vec<[f32; 3]> = vb.vertices
            .iter()
            .map(|v| [v.position[0], v.position[1], 0.0])
            .collect();
        
        // UV座標を正規化（0-1の範囲）で設定
        // 実際のテクスチャ領域はマテリアルのuv_transformで制御
        let uvs: Vec<[f32; 2]> = vb.vertices
            .iter()
            .map(|v| {
                // メッシュの頂点位置を0-1の範囲に正規化
                // min/maxからの相対位置を計算
                let norm_u = (v.position[0] - min_x) / (max_x - min_x);
                // Y座標を反転（ピース内での上下を反転）
                let norm_v = 1.0 - (v.position[1] - min_y) / (max_y - min_y);
                
                [norm_u, norm_v]
            })
            .collect();
            
        // Debug: Log UV coordinates for first few vertices of first piece
        if x == 0 && y == 0 && uvs.len() > 0 {
            println!("First piece UV mapping: vertex[0]: pos[{:.1},{:.1}] -> uv[{:.3},{:.3}]", 
                vb.vertices[0].position[0], vb.vertices[0].position[1], uvs[0][0], uvs[0][1]);
            if uvs.len() > 1 {
                println!("First piece UV mapping: vertex[1]: pos[{:.1},{:.1}] -> uv[{:.3},{:.3}]", 
                    vb.vertices[1].position[0], vb.vertices[1].position[1], uvs[1][0], uvs[1][1]);
            }
        }
        
        // インデックスを変換
        let indices: Vec<u32> = vb.indices
            .iter()
            .map(|&i| i as u32)
            .collect();
        
        // メッシュにデータを設定
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
        mesh.insert_indices(Indices::U32(indices));
        
        println!("Generated 2D mesh for piece ({}, {}) with {} vertices", x, y, vb.vertices.len());
        Ok(mesh)
    }

    /// メッシュからバウンディングボックスを計算（ピース中心を原点とした相対座標）
    fn calculate_mesh_bounds(&self, mesh: &Mesh) -> Rect {
        if let Some(positions) = mesh.attribute(Mesh::ATTRIBUTE_POSITION) {
            match positions {
                bevy::render::mesh::VertexAttributeValues::Float32x3(positions) => {
                    let (min_x, max_x, min_y, max_y) = positions.iter().fold(
                        (f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::NEG_INFINITY),
                        |(min_x, max_x, min_y, max_y), pos| {
                            (min_x.min(pos[0]), max_x.max(pos[0]), min_y.min(pos[1]), max_y.max(pos[1]))
                        }
                    );
                    
                    // メッシュの中心を計算
                    let center_x = (min_x + max_x) / 2.0;
                    let center_y = (min_y + max_y) / 2.0;
                    
                    // 中心を原点とした相対座標に変換
                    let relative_min_x = min_x - center_x;
                    let relative_min_y = min_y - center_y;
                    let relative_max_x = max_x - center_x;
                    let relative_max_y = max_y - center_y;
                    let width = max_x - min_x;
                    let height = max_y - min_y;
                    
                    // Only log for first few pieces to avoid spam
                    static mut BOUNDS_LOG_COUNT: usize = 0;
                    unsafe {
                        if BOUNDS_LOG_COUNT < 4 {
                            println!("Mesh bounds: abs({:.1},{:.1} to {:.1},{:.1}) center:({:.1},{:.1}) -> relative({:.1},{:.1} to {:.1},{:.1}) size:{}x{}", 
                                min_x, min_y, max_x, max_y, center_x, center_y, 
                                relative_min_x, relative_min_y, relative_max_x, relative_max_y, width, height);
                            BOUNDS_LOG_COUNT += 1;
                        }
                    }
                    
                    // Rect::new は (min_x, min_y, max_x, max_y) の順序
                    Rect::new(relative_min_x, relative_min_y, relative_max_x, relative_max_y)
                }
                _ => {
                    // フォールバック: 期待される形式でない場合は piece_size を使用
                    let (piece_width, piece_height) = self.piece_size;
                    Rect::new(-piece_width / 2.0, -piece_height / 2.0, piece_width, piece_height)
                }
            }
        } else {
            // フォールバック: 位置属性がない場合は piece_size を使用
            let (piece_width, piece_height) = self.piece_size;
            Rect::new(-piece_width / 2.0, -piece_height / 2.0, piece_width, piece_height)
        }
    }

    /// SVGパス文字列をlyonのPathオブジェクトに変換（座標オフセット付き）
    fn parse_svg_path_to_lyon_with_offset(&self, svg_path: &str, offset_x: f32, offset_y: f32) -> Result<Path, Box<dyn std::error::Error>> {
        let mut builder = Path::builder();
        let mut path_started = false;
        let mut coord_count = 0;
        
        // SVG解析を実装
        for segment in PathParser::from(svg_path) {
            match segment {
                Ok(seg) => {
                    match seg {
                        PathSegment::MoveTo { abs, x, y } => {
                            if path_started {
                                builder.end(false);
                            }
                            // 絶対座標から相対座標に変換してからBevyの座標系に合わせる
                            let relative_x = x as f32 - offset_x;
                            let relative_y = y as f32 - offset_y;
                            // Debug: Log first few coordinates
                            if coord_count < 3 {
                                println!("MoveTo: abs({:.1}, {:.1}) - offset({:.1}, {:.1}) = rel({:.1}, {:.1}) -> Bevy: ({:.1}, {:.1})", 
                                    x, y, offset_x, offset_y, relative_x, relative_y, relative_x, -relative_y);
                                coord_count += 1;
                            }
                            builder.begin(math::point(relative_x, -relative_y));
                            path_started = true;
                        }
                        PathSegment::LineTo { abs, x, y } => {
                            if !path_started {
                                builder.begin(math::point(0.0, 0.0));
                                path_started = true;
                            }
                            // 絶対座標から相対座標に変換してからBevyの座標系に合わせる
                            let relative_x = x as f32 - offset_x;
                            let relative_y = y as f32 - offset_y;
                            // Debug: Log first few coordinates
                            if coord_count < 3 {
                                println!("LineTo: abs({:.1}, {:.1}) - offset({:.1}, {:.1}) = rel({:.1}, {:.1}) -> Bevy: ({:.1}, {:.1})", 
                                    x, y, offset_x, offset_y, relative_x, relative_y, relative_x, -relative_y);
                                coord_count += 1;
                            }
                            builder.line_to(math::point(relative_x, -relative_y));
                        }
                        PathSegment::CurveTo { abs, x1, y1, x2, y2, x, y } => {
                            if !path_started {
                                builder.begin(math::point(0.0, 0.0));
                                path_started = true;
                            }
                            // 全ての制御点も相対座標に変換
                            let rel_x1 = x1 as f32 - offset_x;
                            let rel_y1 = y1 as f32 - offset_y;
                            let rel_x2 = x2 as f32 - offset_x;
                            let rel_y2 = y2 as f32 - offset_y;
                            let rel_x = x as f32 - offset_x;
                            let rel_y = y as f32 - offset_y;
                            
                            builder.cubic_bezier_to(
                                math::point(rel_x1, -rel_y1),
                                math::point(rel_x2, -rel_y2),
                                math::point(rel_x, -rel_y)
                            );
                        }
                        PathSegment::ClosePath { abs: _ } => {
                            if path_started {
                                builder.end(true);
                                path_started = false;
                            }
                        }
                        _ => {
                            println!("Unsupported SVG path segment");
                        }
                    }
                }
                Err(err) => {
                    println!("Error parsing SVG segment: {}", err);
                }
            }
        }
        
        if path_started {
            builder.end(true);
        }
        
        Ok(builder.build())
    }

    /// SVGパス文字列をlyonのPathオブジェクトに変換（旧バージョン - 使用しない）
    fn parse_svg_path_to_lyon(&self, svg_path: &str) -> Result<Path, Box<dyn std::error::Error>> {
        // println!("Parsing SVG path: {}", svg_path);
        
        let mut builder = Path::builder();
        let mut path_started = false;
        let mut coord_count = 0;
        
        // SVG解析を実装
        for segment in PathParser::from(svg_path) {
            match segment {
                Ok(seg) => {
                    match seg {
                        PathSegment::MoveTo { abs, x, y } => {
                            if path_started {
                                builder.end(false);
                            }
                            // Debug: Log first few coordinates to understand coordinate system
                            if coord_count < 5 {
                                println!("MoveTo: ({:.1}, {:.1}) -> Bevy: ({:.1}, {:.1})", x, y, x as f32, -y as f32);
                                coord_count += 1;
                            }
                            // Y座標を反転してBevyの座標系に合わせる
                            builder.begin(math::point(x as f32, -y as f32));
                            path_started = true;
                        }
                        PathSegment::LineTo { abs, x, y } => {
                            if !path_started {
                                builder.begin(math::point(0.0, 0.0));
                                path_started = true;
                            }
                            // Debug: Log first few coordinates
                            if coord_count < 5 {
                                println!("LineTo: ({:.1}, {:.1}) -> Bevy: ({:.1}, {:.1})", x, y, x as f32, -y as f32);
                                coord_count += 1;
                            }
                            // Y座標を反転してBevyの座標系に合わせる
                            builder.line_to(math::point(x as f32, -y as f32));
                        }
                        PathSegment::CurveTo { abs, x1, y1, x2, y2, x, y } => {
                            if !path_started {
                                builder.begin(math::point(0.0, 0.0));
                                path_started = true;
                            }
                            // Y座標を反転してBevyの座標系に合わせる
                            builder.cubic_bezier_to(
                                math::point(x1 as f32, -y1 as f32),
                                math::point(x2 as f32, -y2 as f32),
                                math::point(x as f32, -y as f32)
                            );
                        }
                        PathSegment::ClosePath { abs: _ } => {
                            // For ClosePath, we just end with closed=true
                            if path_started {
                                path_started = false;
                                builder.end(true);
                            }
                        }
                        _ => {
                            println!("Unsupported SVG command: {:?}", seg);
                        }
                    }
                }
                Err(e) => {
                    println!("Error parsing SVG: {}, falling back to rectangle", e);
                    return self.create_fallback_rectangle();
                }
            }
        }
        
        // If path is still open, close it
        if path_started {
            builder.end(false);
        }
        
        Ok(builder.build())
    }
    
    /// フォールバック用の矩形パスを作成
    fn create_fallback_rectangle(&self) -> Result<Path, Box<dyn std::error::Error>> {
        let (piece_width, piece_height) = self.piece_size;
        let half_width = piece_width / 2.0;
        let half_height = piece_height / 2.0;
        
        let mut builder = Path::builder();
        builder.begin(math::point(-half_width, half_height));
        builder.line_to(math::point(half_width, half_height));
        builder.line_to(math::point(half_width, -half_height));
        builder.line_to(math::point(-half_width, -half_height));
        builder.end(true); // end(true) for closed path
        
        Ok(builder.build())
    }

    /// 全てのピース形状を事前生成
    pub fn generate_all_shapes(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let (grid_width, grid_height) = self.grid_size;
        
        for y in 0..grid_height {
            for x in 0..grid_width {
                self.generate_piece_shape(x, y)?;
            }
        }
        
        println!("Generated all {} shapes", grid_width * grid_height);
        Ok(())
    }

    /// 指定された位置の形状データを取得
    pub fn get_shape(&self, x: usize, y: usize) -> Option<&JigsawPieceShape> {
        self.shape_cache.get(&(x, y))
    }

    /// 形状内での当たり判定用（実際のメッシュ形状を使用）
    pub fn point_in_shape(&self, x: usize, y: usize, point: Vec2) -> bool {
        if let Some(shape) = self.get_shape(x, y) {
            // まず境界チェックで高速に除外
            if !shape.bounds.contains(point) {
                return false;
            }
            
            // 実際のメッシュ形状での精密判定
            self.point_in_mesh(&shape.mesh, point)
        } else {
            false
        }
    }
    
    /// メッシュの三角形を使った点内判定（Ray-casting アルゴリズム）
    fn point_in_mesh(&self, mesh: &Mesh, point: Vec2) -> bool {
        let positions = match mesh.attribute(Mesh::ATTRIBUTE_POSITION) {
            Some(bevy::render::mesh::VertexAttributeValues::Float32x3(pos)) => pos,
            _ => return false,
        };
        
        let indices = match mesh.indices() {
            Some(Indices::U32(idx)) => idx,
            Some(Indices::U16(idx)) => {
                // U16をU32に変換
                return self.point_in_mesh_u16(mesh, point);
            },
            _ => return false,
        };
        
        // Ray-casting: 点から右方向に水平線を引いて、メッシュの辺との交点数を数える
        let mut intersections = 0;
        let ray_y = point.y;
        
        // 全ての三角形の辺をチェック
        for triangle in indices.chunks(3) {
            let v0 = &positions[triangle[0] as usize];
            let v1 = &positions[triangle[1] as usize];
            let v2 = &positions[triangle[2] as usize];
            
            // 三角形の各辺について交点チェック
            intersections += self.count_ray_edge_intersections(point, ray_y, 
                [v0[0], v0[1]], [v1[0], v1[1]]);
            intersections += self.count_ray_edge_intersections(point, ray_y, 
                [v1[0], v1[1]], [v2[0], v2[1]]);
            intersections += self.count_ray_edge_intersections(point, ray_y, 
                [v2[0], v2[1]], [v0[0], v0[1]]);
        }
        
        // 奇数個の交点 = 点が内部にある
        intersections % 2 == 1
    }
    
    /// U16インデックス用の点内判定
    fn point_in_mesh_u16(&self, mesh: &Mesh, point: Vec2) -> bool {
        let positions = match mesh.attribute(Mesh::ATTRIBUTE_POSITION) {
            Some(bevy::render::mesh::VertexAttributeValues::Float32x3(pos)) => pos,
            _ => return false,
        };
        
        let indices = match mesh.indices() {
            Some(Indices::U16(idx)) => idx,
            _ => return false,
        };
        
        let mut intersections = 0;
        let ray_y = point.y;
        
        for triangle in indices.chunks(3) {
            let v0 = &positions[triangle[0] as usize];
            let v1 = &positions[triangle[1] as usize];
            let v2 = &positions[triangle[2] as usize];
            
            intersections += self.count_ray_edge_intersections(point, ray_y, 
                [v0[0], v0[1]], [v1[0], v1[1]]);
            intersections += self.count_ray_edge_intersections(point, ray_y, 
                [v1[0], v1[1]], [v2[0], v2[1]]);
            intersections += self.count_ray_edge_intersections(point, ray_y, 
                [v2[0], v2[1]], [v0[0], v0[1]]);
        }
        
        intersections % 2 == 1
    }
    
    /// 水平線と線分の交点数を計算
    fn count_ray_edge_intersections(&self, point: Vec2, ray_y: f32, edge_start: [f32; 2], edge_end: [f32; 2]) -> usize {
        let y1 = edge_start[1];
        let y2 = edge_end[1];
        
        // 水平線が線分のY範囲内にない場合は交点なし
        if (y1 > ray_y) == (y2 > ray_y) {
            return 0;
        }
        
        // 水平線と線分の交点のX座標を計算
        let x1 = edge_start[0];
        let x2 = edge_end[0];
        let intersection_x = x1 + (ray_y - y1) * (x2 - x1) / (y2 - y1);
        
        // 交点が点より右側にある場合のみカウント
        if intersection_x > point.x {
            1
        } else {
            0
        }
    }
    
    /// SVGパスからメッシュを作成（共有メッシュマネージャー用）
    pub fn create_mesh_from_svg(&self, svg_path: &str, x: usize, y: usize) -> Result<Mesh, Box<dyn std::error::Error>> {
        self.parse_svg_path_to_mesh(svg_path, x, y)
    }
    
    /// フォールバック用の矩形メッシュを作成
    pub fn create_fallback_rectangle_mesh(&self, piece_width: f32, piece_height: f32) -> Mesh {
        let half_width = piece_width / 2.0;
        let half_height = piece_height / 2.0;
        
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        );
        
        // 矩形の頂点
        let positions = vec![
            [-half_width, half_height, 0.0],   // top-left
            [half_width, half_height, 0.0],    // top-right
            [half_width, -half_height, 0.0],   // bottom-right
            [-half_width, -half_height, 0.0],  // bottom-left
        ];
        
        // UV座標
        let uvs = vec![
            [0.0, 0.0], // top-left
            [1.0, 0.0], // top-right
            [1.0, 1.0], // bottom-right
            [0.0, 1.0], // bottom-left
        ];
        
        // インデックス（2つの三角形）
        let indices = vec![0, 1, 2, 0, 2, 3];
        
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
        mesh.insert_indices(Indices::U32(indices));
        
        mesh
    }
    
    /// メッシュのバウンディングボックスを計算（公開用）
    pub fn calculate_bounds(&self, mesh: &Mesh) -> Rect {
        if let Some(positions) = mesh.attribute(Mesh::ATTRIBUTE_POSITION) {
            match positions {
                bevy::render::mesh::VertexAttributeValues::Float32x3(positions) => {
                    let (min_x, max_x, min_y, max_y) = positions.iter().fold(
                        (f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::NEG_INFINITY),
                        |(min_x, max_x, min_y, max_y), pos| {
                            (min_x.min(pos[0]), max_x.max(pos[0]), min_y.min(pos[1]), max_y.max(pos[1]))
                        }
                    );
                    
                    // 中心を原点とした相対座標に変換
                    let center_x = (min_x + max_x) / 2.0;
                    let center_y = (min_y + max_y) / 2.0;
                    let relative_min_x = min_x - center_x;
                    let relative_min_y = min_y - center_y;
                    let relative_max_x = max_x - center_x;
                    let relative_max_y = max_y - center_y;
                    
                    Rect::new(relative_min_x, relative_min_y, relative_max_x, relative_max_y)
                }
                _ => {
                    let (piece_width, piece_height) = self.piece_size;
                    Rect::new(-piece_width / 2.0, -piece_height / 2.0, piece_width / 2.0, piece_height / 2.0)
                }
            }
        } else {
            let (piece_width, piece_height) = self.piece_size;
            Rect::new(-piece_width / 2.0, -piece_height / 2.0, piece_width / 2.0, piece_height / 2.0)
        }
    }
}

/// メッシュデータをクローンするヘルパー関数
pub fn clone_mesh_from_shape(shape: &JigsawPieceShape) -> Mesh {
    let positions = shape.mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap();
    let uvs = shape.mesh.attribute(Mesh::ATTRIBUTE_UV_0).unwrap();
    let indices = shape.mesh.indices().unwrap();
    
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions.clone());
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs.clone());
    match indices {
        Indices::U16(data) => mesh.insert_indices(Indices::U16(data.clone())),
        Indices::U32(data) => mesh.insert_indices(Indices::U32(data.clone())),
    }
    
    mesh
}

// TODO: 後でカスタムマテリアルを実装してテクスチャマッピングを行う
// 現在はシンプルな色付き形状で動作確認