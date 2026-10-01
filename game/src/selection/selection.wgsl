struct PickUniform {
    clip_from_model: mat4x4<f32>,
    uv_from_mesh: mat4x4<f32>,
    alpha: f32,
    cutoff: f32,
    alpha_mode: u32,
    unused: u32,
};
@group(0) @binding(0) var<uniform> params: PickUniform;
@group(0) @binding(1) var<storage, read_write> selection: array<atomic<u32>>;
@group(1) @binding(0) var image: texture_2d<f32>;
@group(1) @binding(1) var image_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) piece_id: u32,
};
@vertex
fn vertex(@location(0) position: vec3<f32>, @location(1) uv: vec2<f32>,
          @location(2) piece_id: u32) -> VertexOutput {
    var out: VertexOutput;
    out.position = params.clip_from_model * vec4(position, 1.0);
    out.uv = (params.uv_from_mesh * vec4(uv, 0.0, 1.0)).xy;
    out.piece_id = piece_id;
    return out;
}
fn check_alpha(uv: vec2<f32>) {
    // Opaque ignores texture alpha, Mask matches ColorMaterial's cutoff.
    // Blend has no discard in normal rendering: zero alpha contributes nothing.
    let alpha = textureSample(image, image_sampler, uv).a * params.alpha;
    if params.alpha_mode == 1u && alpha < params.cutoff { discard; }
    if params.alpha_mode == 2u && alpha <= 0.0 { discard; }
}
@fragment
fn rectangle_fragment(in: VertexOutput) {
    check_alpha(in.uv);
    atomicOr(&selection[in.piece_id / 32u], 1u << (in.piece_id % 32u));
}
@fragment
fn point_fragment(in: VertexOutput) -> @location(0) u32 {
    check_alpha(in.uv);
    // Only the integer target reserves zero; bitset ID zero remains valid.
    return in.piece_id + 1u;
}
