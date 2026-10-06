#import jigsall::presentation::{presentation_pose, presentation_position, presentation_splat_size, presentation_half_size, decode_rotation, quarter_half_size, quarter_splat_size, RotationAnimation}
struct PuzzleUniform {
    clip_from_world:mat4x4<f32>,seed:vec2<u32>,grid:vec2<u32>,image_size:vec2<f32>,size:vec2<f32>,
    view_min:vec2<f32>,view_max:vec2<f32>,count:u32,capacity:u32,opaque:u32,reserved:u32,
    selection_min:vec2<f32>,selection_max:vec2<f32>,selection_enabled:vec4<u32>,
    drag_delta:vec2<f32>,drag_active:u32,preview_active:u32,
    viewport_size:vec2<f32>,viewport_origin:vec2<f32>,
    piece_size_px:vec2<f32>,pixel_world_size:vec2<f32>,
    render_clip_scale:vec2<f32>,render_clip_offset:vec2<f32>,
    far_zoom:u32,splat_min_px:f32,splat_padding:vec2<u32>,
    rotation_time:f32,rotation_active:u32,drag_elevation_time:f32,drag_elevation_active:u32,
    pseudo_3d_direction:vec2<f32>,shadow_base_offset_px:f32,shadow_lift_offset_px:f32,
    shadow_opacity:f32,shadow_enabled:u32,shadow_padding:vec2<u32>,
    visual_cull_extent:vec2<f32>,bevel_width_px:f32,bevel_enabled:u32,
    side_color:vec4<f32>,side_thickness_px:f32,side_enabled:u32,
    bevel_highlight_strength:f32,bevel_shadow_strength:f32,
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
@group(0) @binding(7) var<storage,read> piece_metadata:array<u32>;
struct RemoteDeltas { entries:array<vec4<f32>,32> };
@group(0) @binding(8) var<uniform> remote_deltas:RemoteDeltas;
@group(0) @binding(9) var<storage,read> rotation_animations:array<RotationAnimation>;
// SoA regions use buffer capacity, not the current visible/piece count.
fn component_root(id:u32)->u32 {return piece_metadata[id];}
fn remote_slot(id:u32)->u32 {return piece_metadata[config.capacity+id];}
fn rotation_slot(root:u32)->u32 {return piece_metadata[2u*config.capacity+root];}
@compute @workgroup_size(256) fn cull_pick(@builtin(global_invocation_id) invocation:vec3<u32>) {
    let index=invocation.x;if index>=main_args.instance_count {return;}
    let id=main_ids[index];let state=states[id];
    if (state.flags&9u)!=0u || (state.flags&16u)==0u {return;}
    let local_member=config.drag_active!=0u && (drag_members[id/32u]&(1u<<(id%32u)))!=0u;
    let slot=remote_slot(id);
    let packed=remote_deltas.entries[(max(slot,1u)-1u)/2u];
    let remote_delta=select(packed.xy,packed.zw,slot!=0u && ((slot-1u)&1u)!=0u);
    var animation_slot=0u;
    if config.rotation_active!=0u {
        animation_slot=rotation_slot(component_root(id));
    }
    let local_half=config.size*0.5+min(config.size.x,config.size.y)*0.22;
    var position:vec2<f32>;var half:vec2<f32>;
    if animation_slot==0u {
        let quarter=decode_rotation(state.flags);
        position=presentation_position(state.position,state.flags,local_member,config.drag_delta,slot,remote_delta);
        half=quarter_half_size(local_half,quarter);
        if config.far_zoom!=0u {
            let splat_size=quarter_splat_size(config.size,quarter,config.pixel_world_size,config.splat_min_px);
            half=max(half,quarter_half_size(splat_size*0.5,quarter))+config.pixel_world_size*0.5;
        }
    } else {
        let animation=rotation_animations[animation_slot-1u];
        let pose=presentation_pose(state.position,state.flags,local_member,config.drag_delta,slot,remote_delta,animation,config.rotation_time);
        position=pose.position;half=presentation_half_size(local_half,pose.rotation);
        if config.far_zoom!=0u {
            // Retained main-viewport scale, even when view_min/max describe a pick ROI.
            let splat_size=presentation_splat_size(config.size,pose.rotation,config.pixel_world_size,config.splat_min_px);
            let splat_half=presentation_half_size(splat_size*0.5,pose.rotation);
            half=max(half,splat_half)+config.pixel_world_size*0.5;
        }
    }

    if all(position+half>=config.view_min) && all(position-half<=config.view_max) {
        let dst=atomicAdd(&pick_args.instance_count,1u);pick_ids[dst]=id;
    }
}
