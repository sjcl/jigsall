//! Optional CPU triangle data, copied from tessellation before Mesh creation.
#[derive(Clone, Debug, PartialEq)]
pub struct PieceShape {
    pub vertices: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}
