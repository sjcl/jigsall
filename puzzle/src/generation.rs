#[cfg(any(test, feature = "cpu-picking-debug"))]
use crate::shape_data::PieceShape;
use crate::{
    placement::generate_placement_grid,
    shapes::{contour_path, piece_edges},
};
use bevy_asset::RenderAssetUsages;
use bevy_log::info_span;
use bevy_math::{Rect, Vec2};
use bevy_mesh::{Indices, Mesh, PrimitiveTopology};
use lyon::path::Path;
use lyon_tessellation::{
    BuffersBuilder, FillOptions, FillRule, FillTessellator, FillVertex, StrokeOptions,
    StrokeTessellator, StrokeVertex, TessellationError, VertexBuffers,
};
use puzzella_core::{PieceId, PieceState, PuzzleDefinition, PuzzlePiece};
use rayon::prelude::*;

pub const REFERENCE_GENERATOR_VERSION: u16 = 2;
fn validate_reference(def: &PuzzleDefinition) -> Result<(), &'static str> {
    if def.generator_version != REFERENCE_GENERATOR_VERSION {
        return Err("CPU reference requires generator version 2");
    }
    let mut current = def.clone();
    current.generator_version = puzzella_core::GENERATOR_VERSION;
    current.validate()
}

#[derive(Debug)]
pub enum GenerationError {
    InvalidDefinition(&'static str),
    InvalidPiece(PieceId),
    PlacementCount,
    Tessellation(TessellationError),
    EmptyGeometry,
}
impl std::fmt::Display for GenerationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidDefinition(reason) => f.write_str(reason),
            Self::InvalidPiece(id) => write!(f, "Piece {id} is outside the puzzle"),
            Self::PlacementCount => f.write_str("Could not generate all initial positions"),
            Self::Tessellation(error) => write!(f, "Tessellation failed: {error:?}"),
            Self::EmptyGeometry => f.write_str("Tessellation produced empty geometry"),
        }
    }
}
impl std::error::Error for GenerationError {}
impl From<TessellationError> for GenerationError {
    fn from(error: TessellationError) -> Self {
        Self::Tessellation(error)
    }
}

/// Owned CPU output. Mesh construction consumes these buffers without cloning.
#[derive(Debug, PartialEq)]
pub struct MeshGeometry {
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u16>,
}
impl MeshGeometry {
    pub fn into_mesh(self) -> Mesh {
        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::all());
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, self.positions);
        if !self.uvs.is_empty() {
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs);
        }
        mesh.insert_indices(Indices::U16(self.indices));
        mesh
    }
}

#[derive(Debug, PartialEq)]
pub struct PieceGeometry {
    pub fill: MeshGeometry,
    pub stroke: MeshGeometry,
    pub bounds: Rect,
    #[cfg(any(test, feature = "cpu-picking-debug"))]
    pub shape: PieceShape,
}

#[derive(Debug, PartialEq)]
pub struct PieceData {
    pub geometry: PieceGeometry,
    pub piece_component: PuzzlePiece,
    pub state: PieceState,
}
pub struct PieceCreationResult {
    pub pieces: Vec<PieceData>,
}

/// Rayon map_init reuses tessellators for each worker's job folder.
/// It may initialize more folders than threads; no per-piece tessellator/lock.
pub struct TessellationWorker {
    fill: FillTessellator,
    stroke: StrokeTessellator,
    fill_buffers: VertexBuffers<[f32; 3], u16>,
    stroke_buffers: VertexBuffers<[f32; 3], u16>,
    tolerance: Option<f32>,
}
impl Default for TessellationWorker {
    fn default() -> Self {
        Self {
            fill: FillTessellator::new(),
            stroke: StrokeTessellator::new(),
            fill_buffers: VertexBuffers {
                vertices: Vec::with_capacity(256),
                indices: Vec::with_capacity(768),
            },
            stroke_buffers: VertexBuffers {
                vertices: Vec::with_capacity(512),
                indices: Vec::with_capacity(1536),
            },
            tolerance: None,
        }
    }
}
impl TessellationWorker {
    /// For explicit quality/performance comparisons; production uses adaptive .25.
    pub fn with_tolerance(tolerance: f32) -> Self {
        assert!(tolerance.is_finite() && tolerance > 0.0);
        Self {
            tolerance: Some(tolerance),
            ..Self::default()
        }
    }

    pub fn generate_piece(
        &mut self,
        definition: &PuzzleDefinition,
        id: PieceId,
        position: Vec2,
    ) -> Result<PieceData, GenerationError> {
        validate_reference(definition).map_err(GenerationError::InvalidDefinition)?;
        if id.0 as usize >= definition.piece_count() {
            return Err(GenerationError::InvalidPiece(id));
        }
        self.generate_validated_piece(definition, id, position)
    }

    fn generate_validated_piece(
        &mut self,
        definition: &PuzzleDefinition,
        id: PieceId,
        position: Vec2,
    ) -> Result<PieceData, GenerationError> {
        let piece = definition.piece(id.0, position);
        let size = definition.image_size.as_vec2() / definition.grid_size.as_vec2();
        let path = contour_path(&piece_edges(
            definition.seed,
            definition.grid_size,
            piece.grid_position,
            size,
        ));
        let geometry = self.tessellate(&path, definition, piece.grid_position.as_vec2(), size)?;
        Ok(PieceData {
            geometry,
            piece_component: piece,
            state: PieceState::new(position),
        })
    }

    fn tessellate(
        &mut self,
        path: &Path,
        definition: &PuzzleDefinition,
        cell: Vec2,
        size: Vec2,
    ) -> Result<PieceGeometry, GenerationError> {
        // .25px bounds contour error below a quarter pixel at native scale.
        // Small pieces use <=.25% of their short side to avoid coarse topology.
        let tolerance = self
            .tolerance
            .unwrap_or_else(|| tessellation_tolerance(size));
        self.fill_buffers.vertices.clear();
        self.fill_buffers.indices.clear();
        self.stroke_buffers.vertices.clear();
        self.stroke_buffers.indices.clear();
        self.fill.tessellate_path(
            path,
            &FillOptions::tolerance(tolerance).with_fill_rule(FillRule::NonZero),
            &mut BuffersBuilder::new(&mut self.fill_buffers, |v: FillVertex| {
                [v.position().x, v.position().y, 0.0]
            }),
        )?;
        // Preserve 16px outlines for ordinary pieces; prevent tiny-piece strokes
        // from engulfing the piece. Fill/stroke always share the same Path.
        self.stroke.tessellate_path(
            path,
            &StrokeOptions::tolerance(tolerance).with_line_width(stroke_width(size)),
            &mut BuffersBuilder::new(&mut self.stroke_buffers, |v: StrokeVertex| {
                [v.position().x, v.position().y, 0.0]
            }),
        )?;
        // lyon can emit zero-area triangles at collinear boundary joins.
        // Compact indices in place; do not clamp indices or change the contour.
        remove_zero_area_triangles(&self.fill_buffers.vertices, &mut self.fill_buffers.indices);
        remove_zero_area_triangles(
            &self.stroke_buffers.vertices,
            &mut self.stroke_buffers.indices,
        );
        if self.fill_buffers.vertices.is_empty() || self.stroke_buffers.vertices.is_empty() {
            return Err(GenerationError::EmptyGeometry);
        }
        let mut uvs = Vec::with_capacity(self.fill_buffers.vertices.len());
        let mut min = Vec2::splat(f32::INFINITY);
        let mut max = Vec2::splat(f32::NEG_INFINITY);
        let image_size = definition.image_size.as_vec2();
        let center = (cell + Vec2::splat(0.5)) * size;
        for &vertex in &self.fill_buffers.vertices {
            let local = Vec2::new(vertex[0], vertex[1]);
            min = min.min(local);
            max = max.max(local);
            // Y-up local coordinates -> Y-down source image. Tabs retain their
            // actual source coordinates, including vertices outside the cell.
            let uv = (center + Vec2::new(local.x, -local.y)) / image_size;
            uvs.push(uv.to_array());
        }
        #[cfg(any(test, feature = "cpu-picking-debug"))]
        let shape = PieceShape {
            vertices: self
                .fill_buffers
                .vertices
                .iter()
                .map(|p| [p[0], p[1]])
                .collect(),
            indices: self
                .fill_buffers
                .indices
                .iter()
                .map(|&i| u32::from(i))
                .collect(),
        };
        // Output buffers must be owned by the receiver. Transfer them directly;
        // lyon's tessellator scratch remains reusable across pieces.
        Ok(PieceGeometry {
            fill: MeshGeometry {
                positions: std::mem::take(&mut self.fill_buffers.vertices),
                uvs,
                indices: std::mem::take(&mut self.fill_buffers.indices),
            },
            stroke: MeshGeometry {
                positions: std::mem::take(&mut self.stroke_buffers.vertices),
                uvs: Vec::new(),
                indices: std::mem::take(&mut self.stroke_buffers.indices),
            },
            bounds: Rect { min, max },
            #[cfg(any(test, feature = "cpu-picking-debug"))]
            shape,
        })
    }
}

fn remove_zero_area_triangles<const N: usize>(vertices: &[[f32; N]], indices: &mut Vec<u16>) {
    let mut write = 0;
    for read in (0..indices.len()).step_by(3) {
        let [a, b, c] = [indices[read], indices[read + 1], indices[read + 2]].map(|i| {
            let p = vertices[usize::from(i)];
            Vec2::new(p[0], p[1])
        });
        if (b - a).perp_dot(c - a) != 0.0 {
            indices.copy_within(read..read + 3, write);
            write += 3;
        }
    }
    indices.truncate(write);
}

pub fn tessellation_tolerance(size: Vec2) -> f32 {
    0.25_f32.min(size.min_element() * 0.0025)
}
pub fn stroke_width(size: Vec2) -> f32 {
    16.0_f32.min(size.min_element() * 0.16)
}

/// One background call: placement + independent parallel piece generation.
/// Indexed collection preserves row-major IDs regardless of work stealing.
pub fn generate_pieces(
    definition: &PuzzleDefinition,
) -> Result<PieceCreationResult, GenerationError> {
    validate_reference(definition).map_err(GenerationError::InvalidDefinition)?;
    let _span = info_span!("generate_pieces", count = definition.piece_count()).entered();
    let size = definition.image_size.as_vec2();
    let piece_size = size / definition.grid_size.as_vec2();
    let positions = {
        let _span = info_span!("placement").entered();
        generate_placement_grid(
            definition.grid_size.x as usize,
            definition.grid_size.y as usize,
            piece_size.x,
            piece_size.y,
            size.x,
            size.y,
            definition.seed,
        )
    };
    if positions.len() != definition.piece_count() {
        return Err(GenerationError::PlacementCount);
    }
    let _span = info_span!("parallel_piece_geometry").entered();
    let pieces = positions
        .par_iter()
        .enumerate()
        .with_min_len(16)
        .map_init(TessellationWorker::default, |worker, (index, &position)| {
            worker.generate_validated_piece(definition, PieceId(index as u32), position)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(PieceCreationResult { pieces })
}

#[cfg(test)]
mod tests;
