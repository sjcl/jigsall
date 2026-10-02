struct PuzzleUniform {
    clip_from_world:mat4x4<f32>,seed:vec2<u32>,grid:vec2<u32>,image_size:vec2<f32>,size:vec2<f32>,
    view_min:vec2<f32>,view_max:vec2<f32>,count:u32,capacity:u32,opaque:u32,reserved:u32,
    selection_min:vec2<f32>,selection_max:vec2<f32>,selection_enabled:vec4<u32>,
    drag_delta:vec2<f32>,drag_active:u32,preview_active:u32,
    viewport_size:vec2<f32>,viewport_origin:vec2<f32>,
    piece_size_px:vec2<f32>,pixel_world_size:vec2<f32>,
    render_clip_scale:vec2<f32>,render_clip_offset:vec2<f32>,
    far_zoom:u32,splat_min_px:f32,splat_padding:vec2<u32>,
};
struct PieceState {position:vec2<f32>,z_order:u32,flags:u32};
struct MainArgs {vertex_count:u32,instance_count:u32,first_vertex:u32,first_instance:u32};
struct PickArgs {vertex_count:u32,instance_count:atomic<u32>,first_vertex:u32,first_instance:u32};
@group(0) @binding(0) var<uniform> config:PuzzleUniform;
@group(0) @binding(1) var<storage,read> states:array<PieceState>;
@group(0) @binding(2) var<storage,read> main_ids:array<u32>;
@group(0) @binding(3) var<storage,read> main_args:MainArgs;
@group(0) @binding(4) var<storage,read_write> pick_ids:array<u32>;
@group(0) @binding(5) var<storage,read_write> pick_args:PickArgs;
@group(0) @binding(6) var<storage,read> drag_members:array<u32>;
@compute @workgroup_size(256) fn cull_pick(@builtin(global_invocation_id) invocation:vec3<u32>) {
    let index=invocation.x;if index>=main_args.instance_count {return;}
    let id=main_ids[index];let state=states[id];
    if (state.flags&9u)!=0u || (state.flags&16u)==0u {return;}
    var half=config.size*0.5+min(config.size.x,config.size.y)*0.22;
    if config.far_zoom!=0u {
        // Retained main-viewport scale, even when view_min/max describe a pick ROI.
        let splat_half=max(config.piece_size_px,vec2(config.splat_min_px))*config.pixel_world_size*0.5;
        half=max(half,splat_half)+config.pixel_world_size*0.5;
    }
    var position=state.position;
    if config.drag_active!=0u && (state.flags&8u)!=0u && (drag_members[id/32u]&(1u<<(id%32u)))!=0u {
        position+=config.drag_delta;
    }
    if all(position+half>=config.view_min) && all(position-half<=config.view_max) {
        let dst=atomicAdd(&pick_args.instance_count,1u);pick_ids[dst]=id;
    }
}
