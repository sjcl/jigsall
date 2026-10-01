use bevy_ecs::prelude::Component;
use bevy_mesh::{Indices, Mesh, VertexAttributeValues};

#[derive(Component, Clone, Debug)]
pub struct PieceShape {
    pub vertices: Vec<[f32; 2]>, // メッシュの頂点（2D）
    pub indices: Vec<u32>,       // 三角形インデックス
    pub shape_hash: String,      // 形状のハッシュ（ストロークメッシュキャッシュのキー）
}

/// メッシュから精密当たり判定用の形状データを抽出（JigsawPieceShapeから）
pub fn extract_shape_data_from_jigsaw_shape(
    jigsaw_shape: &crate::shapes::JigsawPieceShape,
) -> PieceShape {
    // 🔧 FIXED: 描画用には常にメッシュ頂点を使用（boundary_verticesはコリジョン用のみ）
    let vertices = match jigsaw_shape.mesh.attribute(Mesh::ATTRIBUTE_POSITION) {
        Some(VertexAttributeValues::Float32x3(positions)) => {
            println!(
                "✅ Using mesh vertices ({} points) for rendering",
                positions.len()
            );
            positions.iter().map(|pos| [pos[0], pos[1]]).collect()
        }
        _ => {
            // フォールバック: boundary_verticesを使用
            if !jigsaw_shape.boundary_vertices.is_empty() {
                println!(
                    "⚠️ No mesh vertices, falling back to boundary vertices ({} points)",
                    jigsaw_shape.boundary_vertices.len()
                );
                jigsaw_shape.boundary_vertices.clone()
            } else {
                println!("❌ No vertices available, using fallback rectangle");
                vec![[0.0, 0.0], [100.0, 0.0], [0.0, 100.0], [100.0, 100.0]]
            }
        }
    };

    // インデックスを生成（常にメッシュから取得）
    let indices = match jigsaw_shape.mesh.indices() {
        Some(Indices::U32(idx)) => idx.clone(),
        Some(Indices::U16(idx)) => idx.iter().map(|&i| i as u32).collect(),
        None => {
            // フォールバック: 頂点数に基づいてファン三角分割を生成
            println!(
                "⚠️ No mesh indices available, generating fan triangulation for {} vertices",
                vertices.len()
            );
            if vertices.len() >= 3 {
                let mut fan_indices = Vec::new();
                for i in 2..vertices.len() {
                    fan_indices.push(0);
                    fan_indices.push((i - 1) as u32);
                    fan_indices.push(i as u32);
                }
                fan_indices
            } else {
                // 最小限のフォールバック
                vec![0, 1, 2]
            }
        }
    };

    // インデックス値の妥当性をチェックして修正
    let max_index = indices.iter().max().cloned().unwrap_or(0);
    let vertex_count = vertices.len() as u32;

    let final_indices = if max_index >= vertex_count {
        println!(
            "❌ INDEX OUT OF BOUNDS: max_index={}, vertex_count={}",
            max_index, vertex_count
        );
        println!(
            "   Mesh has {} attribute vertices",
            match jigsaw_shape.mesh.attribute(Mesh::ATTRIBUTE_POSITION) {
                Some(bevy_mesh::VertexAttributeValues::Float32x3(pos)) => pos.len(),
                _ => 0,
            }
        );
        println!(
            "   Boundary vertices: {}",
            jigsaw_shape.boundary_vertices.len()
        );
        println!(
            "   Using mesh vertices for rendering: {}",
            jigsaw_shape
                .mesh
                .attribute(Mesh::ATTRIBUTE_POSITION)
                .is_some()
        );
        if indices.len() <= 20 {
            println!("   All indices: {:?}", indices);
        } else {
            println!("   First 10 indices: {:?}", &indices[0..10]);
            println!("   Last 10 indices: {:?}", &indices[indices.len() - 10..]);
        }

        // 重要: インデックスアウトオブバウンズを修正
        println!("🔧 Fixing indices to prevent out-of-bounds access");
        let fixed_indices = indices
            .into_iter()
            .map(|idx| idx.min(vertex_count.saturating_sub(1)))
            .collect::<Vec<u32>>();

        let new_max = fixed_indices.iter().max().cloned().unwrap_or(0);
        println!("✅ Fixed indices: max_index={}", new_max);
        fixed_indices
    } else {
        // 正常ケースでも基本情報をログ出力（最初の数個のピースのみ）

        indices
    };

    let final_max_index = final_indices.iter().max().cloned().unwrap_or(0);
    println!(
        "🔍 Extracted shape data: {} vertices, {} indices (max_index: {})",
        vertices.len(),
        final_indices.len(),
        final_max_index
    );
    if vertices.len() <= 5 {
        println!("   First vertices: {:?}", vertices);
    } else {
        println!("   First 5 vertices: {:?}", &vertices[0..5]);
    }

    PieceShape {
        vertices,
        indices: final_indices,
        shape_hash: jigsaw_shape.shape_hash.clone(), // JigsawPieceShapeから形状ハッシュを取得
    }
}
