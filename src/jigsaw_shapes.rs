use bevy::prelude::*;
use bevy_prototype_lyon::prelude::*;
use puzzle_paths::{build_jigsaw_template, JigsawTemplate};
use std::collections::HashMap;

/// ジグソーピースの形状データを管理する構造体
pub struct JigsawPieceShape {
    pub path: Path,
    pub bounds: Rect,
    pub texture_coords: Vec4,
}

/// ジグソーピース形状の生成とキャッシュを管理するシステム
pub struct JigsawShapeGenerator {
    piece_size: (f32, f32),
    grid_size: (usize, usize),
    jigsaw_template: Option<JigsawTemplate>,
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
            grid_height,                      // 列のピース数
            grid_width,                       // 行のピース数
            None,                            // デフォルトのタブサイズ
            None,                            // デフォルトのジッター
            Some(42),                        // 固定シード
        );
        
        self.jigsaw_template = Some(template);
        println!("Generated jigsaw template for {}x{} grid", grid_width, grid_height);
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

        // SVGパスをbevy_prototype_lyonのPathに変換
        let lyon_path = self.parse_svg_path_to_lyon(svg_path)?;
        
        // テクスチャ座標を計算
        let texture_coords = Vec4::new(
            x as f32 / grid_width as f32,
            y as f32 / grid_height as f32,
            (x + 1) as f32 / grid_width as f32,
            (y + 1) as f32 / grid_height as f32,
        );

        // バウンディングボックスを計算
        let (piece_width, piece_height) = self.piece_size;
        let bounds = Rect::new(
            -piece_width / 2.0,
            -piece_height / 2.0,
            piece_width,
            piece_height,
        );

        let shape = JigsawPieceShape {
            path: lyon_path,
            bounds,
            texture_coords,
        };

        // キャッシュに保存
        self.shape_cache.insert((x, y), shape);
        
        println!("Generated shape for piece ({}, {})", x, y);
        Ok(self.shape_cache.get(&(x, y)).unwrap())
    }

    /// SVGパス文字列をbevy_prototype_lyonのPathに変換
    fn parse_svg_path_to_lyon(&self, svg_path: &str) -> Result<Path, Box<dyn std::error::Error>> {
        // TODO: 実際のSVGパス解析を実装
        // 現在は簡単な矩形パスを作成
        let (piece_width, piece_height) = self.piece_size;
        let half_width = piece_width / 2.0;
        let half_height = piece_height / 2.0;

        let mut path_builder = PathBuilder::new();
        
        // 基本的な矩形パス（後でジグソー形状に変更）
        path_builder.move_to(Vec2::new(-half_width, -half_height));
        path_builder.line_to(Vec2::new(half_width, -half_height));
        path_builder.line_to(Vec2::new(half_width, half_height));
        path_builder.line_to(Vec2::new(-half_width, half_height));
        path_builder.close();
        
        Ok(path_builder.build())
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

    /// 形状内での当たり判定用
    pub fn point_in_shape(&self, x: usize, y: usize, point: Vec2) -> bool {
        if let Some(shape) = self.get_shape(x, y) {
            shape.bounds.contains(point)
        } else {
            false
        }
    }
}

// TODO: 後でカスタムマテリアルを実装してテクスチャマッピングを行う
// 現在はシンプルな色付き形状で動作確認