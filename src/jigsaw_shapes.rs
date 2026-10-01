use bevy::asset::RenderAssetUsages;
use bevy::mesh::Indices;
use bevy::prelude::*;
use bevy::render::render_resource::PrimitiveTopology;
use lyon::math;
use lyon::path::Path;
use lyon_tessellation::{
    geometry_builder::BuffersBuilder, FillOptions, FillRule, FillTessellator, FillVertex,
    StrokeOptions, StrokeTessellator, StrokeVertex, VertexBuffers,
};
use puzzle_paths::{build_jigsaw_template, JigsawTemplate};
use std::collections::HashMap;
use svgtypes::{PathParser, PathSegment};

/// シンプルな頂点タイプ
#[derive(Copy, Clone, Debug)]
pub struct SimpleVertex {
    pub position: [f32; 2],
}

/// ジグソーピースの形状データを管理する構造体
pub struct JigsawPieceShape {
    pub mesh: Mesh,
    pub stroke_mesh: Option<Mesh>, // アウトライン用のストロークメッシュ
    pub bounds: Rect,
    pub shape_hash: String,               // SVGパスベースの形状ハッシュ
    pub boundary_vertices: Vec<[f32; 2]>, // コリジョン検出用の境界頂点
}

/// ジグソーピース形状の生成とキャッシュを管理するシステム
pub struct JigsawShapeGenerator {
    piece_size: (f32, f32),
    seed: u64,
    grid_size: (usize, usize),
    jigsaw_template: Option<JigsawTemplate>,
    shape_cache: HashMap<(usize, usize), JigsawPieceShape>,
}

impl JigsawShapeGenerator {
    /// 新しいジグソー形状ジェネレータを作成
    pub fn new(piece_size: (f32, f32), grid_size: (usize, usize), seed: u64) -> Self {
        Self {
            piece_size,
            seed,
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
            piece_width * grid_width as f32,   // 画像全体の幅
            piece_height * grid_height as f32, // 画像全体の高さ
            grid_width,                        // 行のピース数（列数）
            grid_height,                       // 列のピース数（行数）
            None,                              // デフォルトのタブサイズ
            None,                              // デフォルトのジッター
            Some(((self.seed ^ (self.seed >> 32)) & 0x00ff_ffff) as usize), // stable on 32/64-bit
        );

        // デバッグ: ジグソーテンプレート生成ログ
        let total_pieces = grid_width * grid_height;
        println!(
            "🧩 Generated jigsaw template for {}x{} grid ({} total pieces)",
            grid_width, grid_height, total_pieces
        );

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
        let (grid_width, _) = self.grid_size;
        let piece_index = y * grid_width + x;

        // SVGパスを取得
        let svg_path = if piece_index < template.svg_paths.len() {
            &template.svg_paths[piece_index]
        } else {
            return Err(format!("Piece index {} out of bounds", piece_index).into());
        };

        // SVGパスをlyonのPathに変換してメッシュ生成（フィルとストローク両方）
        let (mesh, stroke_mesh) = self.parse_svg_path_to_meshes(svg_path, x, y)?;

        // 境界頂点を抽出（コリジョン検出用）
        let (piece_width, piece_height) = self.piece_size;
        let svg_piece_width = piece_width;
        let svg_piece_height = piece_height;
        let offset_x = x as f32 * svg_piece_width + svg_piece_width / 2.0;
        let offset_y = y as f32 * svg_piece_height + svg_piece_height / 2.0;

        let boundary_vertices = match self
            .extract_boundary_vertices_from_svg(svg_path, offset_x, offset_y)
        {
            Ok(vertices) => {
                println!(
                    "✅ Successfully extracted {} boundary vertices for piece ({}, {})",
                    vertices.len(),
                    x,
                    y
                );
                vertices
            }
            Err(e) => {
                println!("⚠️ Failed to extract boundary vertices for piece ({}, {}): {}. Using fallback rectangle.", x, y, e);
                // フォールバック: 矩形の境界頂点
                let half_width = piece_width / 2.0;
                let half_height = piece_height / 2.0;
                vec![
                    [-half_width, half_height],  // 左上
                    [half_width, half_height],   // 右上
                    [half_width, -half_height],  // 右下
                    [-half_width, -half_height], // 左下
                ]
            }
        };

        // SVGパスから形状ハッシュを計算
        let shape_hash = self.calculate_shape_hash_from_svg_path(svg_path);

        // バウンディングボックスをメッシュから計算
        let bounds = self.calculate_mesh_bounds(&mesh);

        let shape = JigsawPieceShape {
            mesh,
            stroke_mesh,
            bounds,
            shape_hash,
            boundary_vertices,
        };

        // キャッシュに保存
        self.shape_cache.insert((x, y), shape);

        // ログは削除（バックグラウンド生成で大量になるため）
        Ok(self.shape_cache.get(&(x, y)).unwrap())
    }

    /// SVGパス文字列をlyonで解析してBevyメッシュに変換（フィルとストローク両方）
    fn parse_svg_path_to_meshes(
        &self,
        svg_path: &str,
        x: usize,
        y: usize,
    ) -> Result<(Mesh, Option<Mesh>), Box<dyn std::error::Error>> {
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
        let lyon_path = match self.parse_svg_path_to_lyon_with_offset(svg_path, offset_x, offset_y)
        {
            Ok(path) => path,
            Err(err) => {
                println!(
                    "Failed to parse SVG path for piece ({}, {}): {}. Using fallback rectangle.",
                    x, y, err
                );
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

        // フィル（塗りつぶし）メッシュを生成
        let mut vb: VertexBuffers<SimpleVertex, u16> = VertexBuffers::new();
        let fill_options = FillOptions::tolerance(0.1) // より細かいテッセレーション（デフォルトは0.25）
            .with_fill_rule(FillRule::NonZero);

        FillTessellator::new().tessellate_path(
            &lyon_path,
            &fill_options,
            &mut BuffersBuilder::new(&mut vb, |v: FillVertex| SimpleVertex {
                position: [v.position().x, v.position().y],
            }),
        )?;

        // ストローク（輪郭線）メッシュを生成
        let mut stroke_vb: VertexBuffers<SimpleVertex, u16> = VertexBuffers::new();
        let stroke_options = StrokeOptions::tolerance(0.1).with_line_width(16.0); // 16ピクセル幅の輪郭線

        let stroke_result = StrokeTessellator::new().tessellate_path(
            &lyon_path,
            &stroke_options,
            &mut BuffersBuilder::new(&mut stroke_vb, |v: StrokeVertex| SimpleVertex {
                position: [v.position().x, v.position().y],
            }),
        );

        let stroke_mesh = if stroke_result.is_ok() && !stroke_vb.vertices.is_empty() {
            Some(self.create_stroke_mesh(&stroke_vb)?)
        } else {
            None
        };

        // バウンディングボックスを実際の頂点から計算
        let (min_x, max_x, min_y, max_y) = if vb.vertices.is_empty() {
            // フォールバック: 頂点がない場合は piece_size を使用
            let (piece_width, piece_height) = self.piece_size;
            (
                -piece_width / 2.0,
                piece_width / 2.0,
                -piece_height / 2.0,
                piece_height / 2.0,
            )
        } else {
            vb.vertices.iter().fold(
                (
                    f32::INFINITY,
                    f32::NEG_INFINITY,
                    f32::INFINITY,
                    f32::NEG_INFINITY,
                ),
                |(min_x, max_x, min_y, max_y), v| {
                    let x = v.position[0];
                    let y = v.position[1];
                    (min_x.min(x), max_x.max(x), min_y.min(y), max_y.max(y))
                },
            )
        };
        let _width = max_x - min_x;
        let _height = max_y - min_y;

        // Bevyメッシュを作成
        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::all());

        // 頂点位置を変換
        let positions: Vec<[f32; 3]> = vb
            .vertices
            .iter()
            .map(|v| [v.position[0], v.position[1], 0.0])
            .collect();

        // UV座標を計算（座標変換前の絶対座標を使用してテクスチャマッピング）
        let (grid_width, grid_height) = self.grid_size;
        let (piece_width, piece_height) = self.piece_size;

        // ピースの絶対座標範囲（変換前）
        let piece_abs_min_x = x as f32 * piece_width;
        let _piece_abs_max_x = (x + 1) as f32 * piece_width;
        let piece_abs_min_y = y as f32 * piece_height;
        let _piece_abs_max_y = (y + 1) as f32 * piece_height;

        // テクスチャ座標範囲
        let texture_u_start = x as f32 / grid_width as f32;
        let texture_v_start = y as f32 / grid_height as f32;
        let texture_u_end = (x + 1) as f32 / grid_width as f32;
        let texture_v_end = (y + 1) as f32 / grid_height as f32;

        let uvs: Vec<[f32; 2]> = vb
            .vertices
            .iter()
            .map(|v| {
                // 変換前の絶対座標に戻す（座標変換の逆計算）
                let center_offset_x = x as f32 * piece_width + piece_width / 2.0;
                let center_offset_y = y as f32 * piece_height + piece_height / 2.0;
                let abs_x = v.position[0] + center_offset_x;
                let abs_y = -v.position[1] + center_offset_y; // Y軸の反転を考慮

                // 絶対座標をピース内の相対座標に正規化
                let norm_u = (abs_x - piece_abs_min_x) / piece_width;
                let norm_v = (abs_y - piece_abs_min_y) / piece_height;

                // テクスチャ領域にマッピング
                let u = texture_u_start + norm_u * (texture_u_end - texture_u_start);
                let v = texture_v_start + norm_v * (texture_v_end - texture_v_start);

                [u, v]
            })
            .collect();

        // Debug: Log UV coordinates for first few vertices of first piece
        if x == 0 && y == 0 && !uvs.is_empty() {
            println!(
                "First piece UV mapping: vertex[0]: pos[{:.1},{:.1}] -> uv[{:.3},{:.3}]",
                vb.vertices[0].position[0], vb.vertices[0].position[1], uvs[0][0], uvs[0][1]
            );
            if uvs.len() > 1 {
                println!(
                    "First piece UV mapping: vertex[1]: pos[{:.1},{:.1}] -> uv[{:.3},{:.3}]",
                    vb.vertices[1].position[0], vb.vertices[1].position[1], uvs[1][0], uvs[1][1]
                );
            }
        }

        // インデックスを変換
        let indices: Vec<u32> = vb.indices.iter().map(|&i| i as u32).collect();

        // メッシュにデータを設定
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
        mesh.insert_indices(Indices::U32(indices));

        // ログは削除（バックグラウンド生成で大量になるため）
        Ok((mesh, stroke_mesh))
    }

    /// ストローク用のメッシュを作成
    fn create_stroke_mesh(
        &self,
        stroke_vb: &VertexBuffers<SimpleVertex, u16>,
    ) -> Result<Mesh, Box<dyn std::error::Error>> {
        let mut stroke_mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::all());

        // 頂点位置を変換
        let positions: Vec<[f32; 3]> = stroke_vb
            .vertices
            .iter()
            .map(|v| [v.position[0], v.position[1], 0.0])
            .collect();

        // インデックスを変換
        let indices: Vec<u32> = stroke_vb.indices.iter().map(|&i| i as u32).collect();

        // メッシュにデータを設定（UVは不要）
        stroke_mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        stroke_mesh.insert_indices(Indices::U32(indices));

        Ok(stroke_mesh)
    }

    /// メッシュからバウンディングボックスを計算（ピース中心を原点とした相対座標）
    fn calculate_mesh_bounds(&self, mesh: &Mesh) -> Rect {
        if let Some(positions) = mesh.attribute(Mesh::ATTRIBUTE_POSITION) {
            match positions {
                bevy::mesh::VertexAttributeValues::Float32x3(positions) => {
                    let (min_x, max_x, min_y, max_y) = positions.iter().fold(
                        (
                            f32::INFINITY,
                            f32::NEG_INFINITY,
                            f32::INFINITY,
                            f32::NEG_INFINITY,
                        ),
                        |(min_x, max_x, min_y, max_y), pos| {
                            (
                                min_x.min(pos[0]),
                                max_x.max(pos[0]),
                                min_y.min(pos[1]),
                                max_y.max(pos[1]),
                            )
                        },
                    );

                    // Mesh vertices already use piece-local coordinates.
                    Rect::new(min_x, min_y, max_x, max_y)
                }
                _ => {
                    // フォールバック: 期待される形式でない場合は piece_size を使用
                    let (piece_width, piece_height) = self.piece_size;
                    Rect::new(
                        -piece_width / 2.0,
                        -piece_height / 2.0,
                        piece_width,
                        piece_height,
                    )
                }
            }
        } else {
            // フォールバック: 位置属性がない場合は piece_size を使用
            let (piece_width, piece_height) = self.piece_size;
            Rect::new(
                -piece_width / 2.0,
                -piece_height / 2.0,
                piece_width,
                piece_height,
            )
        }
    }

    /// SVGパスから形状ハッシュを計算（座標正規化版）
    fn calculate_shape_hash_from_svg_path(&self, svg_path: &str) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        // SVGパスから形状パターンを抽出（座標を正規化）
        let shape_pattern = self.extract_shape_pattern_from_svg(svg_path);

        // 正規化されたパターンからハッシュを計算
        let mut hasher = DefaultHasher::new();
        shape_pattern.hash(&mut hasher);
        let hash_value = hasher.finish();

        let hash = format!("shape_{:08x}", hash_value % 0xFFFFFFFF);

        // デバッグログ（最初の数ピースのみ）

        hash
    }

    /// SVGパスから形状パターンを抽出（正規化版）
    fn extract_shape_pattern_from_svg(&self, svg_path: &str) -> String {
        let mut coords = Vec::new();
        let mut commands = Vec::new();

        // SVGパスから座標と命令を分離
        for segment in PathParser::from(svg_path) {
            match segment {
                Ok(seg) => {
                    match seg {
                        PathSegment::MoveTo { x, y, .. } => {
                            commands.push("M");
                            coords.push(x);
                            coords.push(y);
                        }
                        PathSegment::LineTo { x, y, .. } => {
                            commands.push("L");
                            coords.push(x);
                            coords.push(y);
                        }
                        PathSegment::CurveTo {
                            x1,
                            y1,
                            x2,
                            y2,
                            x,
                            y,
                            ..
                        } => {
                            commands.push("C");
                            coords.extend_from_slice(&[x1, y1, x2, y2, x, y]);
                        }
                        PathSegment::ClosePath { .. } => {
                            commands.push("Z");
                        }
                        _ => {
                            // 他のセグメントタイプは無視
                        }
                    }
                }
                Err(_) => {
                    // パースエラーは無視
                }
            }
        }

        // 座標を正規化（バウンディングボックスで0-1に正規化）
        if coords.len() >= 2 {
            let min_x = coords
                .iter()
                .step_by(2)
                .cloned()
                .fold(f64::INFINITY, f64::min);
            let max_x = coords
                .iter()
                .step_by(2)
                .cloned()
                .fold(f64::NEG_INFINITY, f64::max);
            let min_y = coords
                .iter()
                .skip(1)
                .step_by(2)
                .cloned()
                .fold(f64::INFINITY, f64::min);
            let max_y = coords
                .iter()
                .skip(1)
                .step_by(2)
                .cloned()
                .fold(f64::NEG_INFINITY, f64::max);

            let width = max_x - min_x;
            let height = max_y - min_y;

            if width > 0.0 && height > 0.0 {
                // 座標を0-1000の範囲に正規化（精度のため）
                for (i, coordinate) in coords.iter_mut().enumerate() {
                    if i % 2 == 0 {
                        // X座標
                        *coordinate = ((*coordinate - min_x) / width * 1000.0).round();
                    } else {
                        // Y座標
                        *coordinate = ((*coordinate - min_y) / height * 1000.0).round();
                    }
                }
            }
        }

        // 正規化された座標と命令から形状パターンを生成
        let mut pattern = String::new();
        let mut coord_idx = 0;

        for cmd in commands {
            pattern.push_str(cmd);
            match cmd {
                "M" | "L" => {
                    if coord_idx + 1 < coords.len() {
                        pattern.push_str(&format!(
                            "{},{}",
                            coords[coord_idx] as i32,
                            coords[coord_idx + 1] as i32
                        ));
                        coord_idx += 2;
                    }
                }
                "C" => {
                    if coord_idx + 5 < coords.len() {
                        pattern.push_str(&format!(
                            "{},{},{},{},{},{}",
                            coords[coord_idx] as i32,
                            coords[coord_idx + 1] as i32,
                            coords[coord_idx + 2] as i32,
                            coords[coord_idx + 3] as i32,
                            coords[coord_idx + 4] as i32,
                            coords[coord_idx + 5] as i32
                        ));
                        coord_idx += 6;
                    }
                }
                "Z" => {
                    // ClosePath - 座標なし
                }
                _ => {}
            }
        }

        pattern
    }

    /// SVGパス文字列をlyonのPathオブジェクトに変換（座標オフセット付き）
    fn parse_svg_path_to_lyon_with_offset(
        &self,
        svg_path: &str,
        offset_x: f32,
        offset_y: f32,
    ) -> Result<Path, Box<dyn std::error::Error>> {
        let mut builder = Path::builder();
        let mut path_started = false;
        let mut coord_count = 0;

        // デバッグログを最初の数ピースのみに制限
        let should_debug = offset_x < 200.0 && offset_y < 200.0;

        // SVG解析を実装
        for segment in PathParser::from(svg_path) {
            match segment {
                Ok(seg) => {
                    match seg {
                        PathSegment::MoveTo { abs: _, x, y } => {
                            if path_started {
                                builder.end(false);
                            }
                            // 絶対座標から相対座標に変換してからBevyの座標系に合わせる
                            let relative_x = x as f32 - offset_x;
                            let relative_y = y as f32 - offset_y;
                            // Debug: Log first few coordinates for first few pieces only
                            if should_debug && coord_count < 3 {
                                println!("MoveTo: abs({:.1}, {:.1}) - offset({:.1}, {:.1}) = rel({:.1}, {:.1}) -> Bevy: ({:.1}, {:.1})",
                                    x, y, offset_x, offset_y, relative_x, relative_y, relative_x, -relative_y);
                                coord_count += 1;
                            }
                            builder.begin(math::point(relative_x, -relative_y));
                            path_started = true;
                        }
                        PathSegment::LineTo { abs: _, x, y } => {
                            if !path_started {
                                builder.begin(math::point(0.0, 0.0));
                                path_started = true;
                            }
                            // 絶対座標から相対座標に変換してからBevyの座標系に合わせる
                            let relative_x = x as f32 - offset_x;
                            let relative_y = y as f32 - offset_y;
                            // Debug: Log first few coordinates for first few pieces only
                            if should_debug && coord_count < 3 {
                                println!("LineTo: abs({:.1}, {:.1}) - offset({:.1}, {:.1}) = rel({:.1}, {:.1}) -> Bevy: ({:.1}, {:.1})",
                                    x, y, offset_x, offset_y, relative_x, relative_y, relative_x, -relative_y);
                                coord_count += 1;
                            }
                            builder.line_to(math::point(relative_x, -relative_y));
                        }
                        PathSegment::CurveTo {
                            abs: _,
                            x1,
                            y1,
                            x2,
                            y2,
                            x,
                            y,
                        } => {
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
                                math::point(rel_x, -rel_y),
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

    /// SVGパスから境界頂点を抽出（コリジョン検出用）
    pub fn extract_boundary_vertices_from_svg(
        &self,
        svg_path: &str,
        offset_x: f32,
        offset_y: f32,
    ) -> Result<Vec<[f32; 2]>, Box<dyn std::error::Error>> {
        let mut vertices = Vec::new();
        let mut current_x = 0.0f64;
        let mut current_y = 0.0f64;

        // SVGパスを解析して境界頂点のみを抽出
        for segment in PathParser::from(svg_path) {
            match segment {
                Ok(seg) => {
                    match seg {
                        PathSegment::MoveTo { abs: _, x, y } => {
                            // 座標系変換: SVG座標 → Bevy座標 (Y反転 + オフセット)
                            current_x = x;
                            current_y = y;
                            let bevy_x = x as f32 - offset_x;
                            let bevy_y = -(y as f32 - offset_y);
                            vertices.push([bevy_x, bevy_y]);
                        }
                        PathSegment::LineTo { abs: _, x, y } => {
                            current_x = x;
                            current_y = y;
                            let bevy_x = x as f32 - offset_x;
                            let bevy_y = -(y as f32 - offset_y);
                            vertices.push([bevy_x, bevy_y]);
                        }
                        PathSegment::CurveTo {
                            abs: _,
                            x1,
                            y1,
                            x2,
                            y2,
                            x,
                            y,
                        } => {
                            // ベジェ曲線を複数セグメントに分割して境界の凸部分をキャプチャ
                            let start_x = current_x;
                            let start_y = current_y;

                            // ベジェ曲線を8セグメントに分割（凸部分を正確にキャプチャするため）
                            let segments = 8;
                            for i in 1..=segments {
                                let t = i as f32 / segments as f32;

                                // 3次ベジェ曲線の計算: B(t) = (1-t)³P₀ + 3(1-t)²tP₁ + 3(1-t)t²P₂ + t³P₃
                                let one_minus_t = 1.0 - t;
                                let one_minus_t_sq = one_minus_t * one_minus_t;
                                let one_minus_t_cube = one_minus_t_sq * one_minus_t;
                                let t_sq = t * t;
                                let t_cube = t_sq * t;

                                let bezier_x = one_minus_t_cube * (start_x as f32)
                                    + 3.0 * one_minus_t_sq * t * (x1 as f32)
                                    + 3.0 * one_minus_t * t_sq * (x2 as f32)
                                    + t_cube * (x as f32);

                                let bezier_y = one_minus_t_cube * (start_y as f32)
                                    + 3.0 * one_minus_t_sq * t * (y1 as f32)
                                    + 3.0 * one_minus_t * t_sq * (y2 as f32)
                                    + t_cube * (y as f32);

                                // 座標系変換: SVG座標 → Bevy座標 (Y反転 + オフセット)
                                let bevy_x = bezier_x - offset_x;
                                let bevy_y = -(bezier_y - offset_y);
                                vertices.push([bevy_x, bevy_y]);
                            }

                            // 現在位置を更新
                            current_x = x;
                            current_y = y;
                        }
                        PathSegment::ClosePath { abs: _ } => {
                            // ClosePath: 最初の頂点に戻る（すでに追加されているのでスキップ）
                        }
                        _ => {
                            // その他のコマンド（楕円弧など）は無視
                        }
                    }
                }
                Err(e) => {
                    return Err(format!("SVG parsing error: {}", e).into());
                }
            }
        }

        // 最低限の頂点数をチェック
        if vertices.len() < 3 {
            return Err("Not enough vertices for collision polygon".into());
        }

        // 境界ボックスを計算してタブの凸部分がキャプチャされているか確認
        let mut min_x = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_y = f32::NEG_INFINITY;

        for vertex in &vertices {
            min_x = min_x.min(vertex[0]);
            max_x = max_x.max(vertex[0]);
            min_y = min_y.min(vertex[1]);
            max_y = max_y.max(vertex[1]);
        }

        println!(
            "🔍 Extracted {} boundary vertices from SVG path",
            vertices.len()
        );
        println!(
            "   📏 Boundary box: ({:.1}, {:.1}) to ({:.1}, {:.1}) [size: {:.1}x{:.1}]",
            min_x,
            min_y,
            max_x,
            max_y,
            max_x - min_x,
            max_y - min_y
        );

        if vertices.len() <= 5 {
            println!("   First vertices: {:?}", vertices);
        } else {
            println!("   First 5 vertices: {:?}", &vertices[0..5]);
            println!("   Last 5 vertices: {:?}", &vertices[vertices.len() - 5..]);
        }

        Ok(vertices)
    }

    pub fn generate_all_shapes(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let (grid_width, grid_height) = self.grid_size;

        for y in 0..grid_height {
            for x in 0..grid_width {
                self.generate_piece_shape(x, y)?;
            }
        }

        println!("✅ Generated all {} shapes", grid_width * grid_height);
        Ok(())
    }

    /// 指定された位置の形状データを取得
    pub fn get_shape(&self, x: usize, y: usize) -> Option<&JigsawPieceShape> {
        self.shape_cache.get(&(x, y))
    }
}

pub fn clone_mesh_from_shape(shape: &JigsawPieceShape) -> Mesh {
    let positions = shape.mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap();
    let uvs = shape.mesh.attribute(Mesh::ATTRIBUTE_UV_0).unwrap();
    let indices = shape.mesh.indices().unwrap();

    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::all());

    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions.clone());
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs.clone());
    match indices {
        Indices::U16(data) => mesh.insert_indices(Indices::U16(data.clone())),
        Indices::U32(data) => mesh.insert_indices(Indices::U32(data.clone())),
    }

    mesh
}
