struct CursorView {
    clip_from_world: mat4x4<f32>,
    viewport_size: vec2<f32>,
    viewport_origin: vec2<f32>,
    scale_factor: f32,
    padding: vec3<f32>,
}
struct Cursor {
    world_position: vec2<f32>,
    logical_size: vec2<f32>,
    color: vec4<f32>,
    label_uv: vec4<f32>,
}
@group(0) @binding(0) var<uniform> view: CursorView;
@group(0) @binding(1) var<storage, read> cursors: array<Cursor>;
@group(1) @binding(0) var atlas: texture_2d<f32>;
@group(1) @binding(1) var atlas_sampler: sampler;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) instance: u32,
}
fn position(world: vec2<f32>, offset: vec2<f32>) -> vec4<f32> {
    let center = view.clip_from_world * vec4(world, 0.0, 1.0);
    // Cull the whole instance by its center, including a label reaching into view.
    if center.w <= 0.0 || any(abs(center.xy) > vec2(center.w)) {
        return vec4(2.0, 2.0, 0.0, 1.0);
    }
    let ndc_offset = offset * view.scale_factor * vec2(2.0, -2.0) / view.viewport_size;
    return vec4(center.xy + ndc_offset * center.w, 0.0, center.w);
}
@vertex fn marker_vertex(@builtin(vertex_index) vertex: u32, @builtin(instance_index) id: u32) -> VertexOut {
    let points = array(vec2(0.0, 0.0), vec2(4.0, 16.0), vec2(15.0, 8.0));
    var out: VertexOut;
    out.position = position(cursors[id].world_position, points[vertex]);
    out.local = points[vertex]; out.instance = id;
    return out;
}
@fragment fn marker_fragment(in: VertexOut) -> @location(0) vec4<f32> {
    return cursors[in.instance].color;
}
@vertex fn label_vertex(@builtin(vertex_index) vertex: u32, @builtin(instance_index) id: u32) -> VertexOut {
    let corners = array(vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
        vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0));
    var out: VertexOut;
    out.local = corners[vertex] * (cursors[id].logical_size + vec2(8.0, 4.0));
    out.position = position(cursors[id].world_position, vec2(13.0, 7.0) + out.local);
    if any(cursors[id].logical_size <= vec2(0.0)) { out.position = vec4(2.0, 2.0, 0.0, 1.0); }
    out.instance = id;
    return out;
}
@fragment fn label_fragment(in: VertexOut) -> @location(0) vec4<f32> {
    let cursor = cursors[in.instance];
    let text_point = in.local - vec2(4.0, 2.0);
    let uv = mix(cursor.label_uv.xy, cursor.label_uv.zw, text_point / cursor.logical_size);
    // Sampling is unconditional so implicit derivatives remain uniform.
    let sample = textureSample(atlas, atlas_sampler, uv).r;
    let coverage = select(0.0, sample, all(text_point >= vec2(0.0)) && all(text_point < cursor.logical_size));
    let alpha = coverage + (1.0 - coverage) * (190.0 / 255.0);
    return vec4(vec3(coverage / alpha), alpha);
}
