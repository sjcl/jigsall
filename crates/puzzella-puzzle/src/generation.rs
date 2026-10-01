use crate::{
    placement::generate_placement_grid,
    shape_data::{extract_shape_data_from_jigsaw_shape, PieceShape},
    shapes::JigsawShapeGenerator,
};
use bevy_math::{Rect, Vec2};
use bevy_mesh::Mesh;
use puzzella_core::{PieceState, PuzzleDefinition, PuzzlePiece};
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
    #[allow(dead_code)] // Retained shape metadata for debug tools; GPU picking never reads it.
    pub bounds: Rect,
}

// ピース作成の非同期タスク結果
pub struct PieceCreationResult {
    pub pieces: Vec<PieceData>,
}

pub fn generate_shapes(definition: &PuzzleDefinition) -> Result<ShapeGenerationResult, String> {
    definition.validate().map_err(str::to_owned)?;
    let (width, height) = (
        definition.grid_size.x as usize,
        definition.grid_size.y as usize,
    );
    let size = definition.image_size.as_vec2();
    let piece_size = size / definition.grid_size.as_vec2();
    let mut generator = JigsawShapeGenerator::new(
        (piece_size.x, piece_size.y),
        (width, height),
        definition.seed,
    );
    generator
        .generate_jigsaw_template()
        .map_err(|e| e.to_string())?;
    generator.generate_all_shapes().map_err(|e| e.to_string())?;
    let positions = generate_placement_grid(
        width,
        height,
        piece_size.x,
        piece_size.y,
        size.x,
        size.y,
        definition.seed,
    );
    if positions.len() != definition.piece_count() {
        return Err("Could not generate all initial positions".into());
    }
    Ok(ShapeGenerationResult {
        shape_generator: generator,
        placement_positions: positions,
        total_pieces: definition.piece_count(),
    })
}

pub fn create_all_pieces_sync(
    result: ShapeGenerationResult,
    definition: &PuzzleDefinition,
) -> Result<PieceCreationResult, String> {
    let mut pieces = Vec::with_capacity(result.total_pieces);
    for (index, &position) in result.placement_positions.iter().enumerate() {
        let piece = definition.piece(index as u32, position);
        let shape = result
            .shape_generator
            .get_shape(
                piece.grid_position.x as usize,
                piece.grid_position.y as usize,
            )
            .ok_or_else(|| format!("Missing shape for piece {index}"))?;
        pieces.push(PieceData {
            bounds: shape.bounds,
            mesh: crate::shapes::clone_mesh_from_shape(shape),
            stroke_mesh: shape.stroke_mesh.clone(),
            piece_shape: extract_shape_data_from_jigsaw_shape(shape),
            state: PieceState::new(position),
            piece_component: piece,
        });
    }
    Ok(PieceCreationResult { pieces })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_math::UVec2;
    use puzzella_core::{PieceId, GENERATOR_VERSION};
    fn definition(seed: u64) -> PuzzleDefinition {
        PuzzleDefinition {
            generator_version: GENERATOR_VERSION,
            seed,
            grid_size: UVec2::new(4, 3),
            image_size: UVec2::new(800, 600),
            snap_distance: 50.0,
        }
    }
    #[test]
    fn generation_reproduces_ids_shapes_uvs_and_positions() {
        let def = definition(42);
        let a = create_all_pieces_sync(generate_shapes(&def).unwrap(), &def).unwrap();
        let b = create_all_pieces_sync(generate_shapes(&def).unwrap(), &def).unwrap();
        assert_eq!(a.pieces.len(), def.piece_count());
        for (index, (a, b)) in a.pieces.iter().zip(&b.pieces).enumerate() {
            assert_eq!(a.piece_component.id, PieceId(index as u32));
            assert_eq!(a.piece_component, b.piece_component);
            assert_eq!(a.state, b.state);
            assert_eq!(a.piece_shape.vertices, b.piece_shape.vertices);
            assert_eq!(a.piece_shape.indices, b.piece_shape.indices);
            assert_eq!(
                a.mesh.attribute(Mesh::ATTRIBUTE_UV_0),
                b.mesh.attribute(Mesh::ATTRIBUTE_UV_0)
            );
        }
        let other = definition(43);
        let c = create_all_pieces_sync(generate_shapes(&other).unwrap(), &other).unwrap();
        assert_ne!(a.pieces[0].state.position, c.pieces[0].state.position);
        assert!(a
            .pieces
            .iter()
            .zip(&c.pieces)
            .any(|(a, c)| a.piece_shape.vertices != c.piece_shape.vertices));
    }
}
