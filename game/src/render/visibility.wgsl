#import jigsall::presentation::presentation_position
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
struct DrawArgs {vertex_count:u32,instance_count:atomic<u32>,first_vertex:u32,first_instance:u32};
@group(0) @binding(0) var<uniform> config:PuzzleUniform;
@group(0) @binding(1) var<storage,read> states:array<PieceState>;
@group(0) @binding(2) var<storage,read_write> visible:array<u32>;
@group(0) @binding(3) var<storage,read_write> args:DrawArgs;
@group(0) @binding(4) var<storage,read_write> selectable:array<u32>;
@group(0) @binding(5) var<storage,read> drag_members:array<u32>;
@group(0) @binding(6) var<storage,read_write> group_counts:array<u32>;
@group(0) @binding(7) var<storage,read> remote_slots:array<u32>;
struct RemoteDeltas { entries:array<vec4<f32>,32> };
@group(0) @binding(8) var<uniform> remote_deltas:RemoteDeltas;
fn is_visible(id:u32)->bool {
    if id>=config.count {return false;}
    if (id%32u)==0u {
        var word=0u;for(var bit=0u;bit<32u && id+bit<config.count;bit++) {
            let flags=states[id+bit].flags;
            if (flags&9u)==0u && (flags&16u)!=0u {word|=1u<<bit;}
        }selectable[id/32u]=word;
    }
    let state=states[id];var half=config.size*0.5+0.22*min(config.size.x,config.size.y);
    let odd_rotation=((state.flags>>9u)&1u)!=0u;
    if odd_rotation {half=half.yx;}
    if config.far_zoom!=0u {
        var piece_size_px=config.piece_size_px;
        if odd_rotation {piece_size_px=config.size.yx/config.pixel_world_size;}
        let splat_half=max(piece_size_px,vec2(config.splat_min_px))*config.pixel_world_size*0.5;
        // Pixel-center snapping can move the splat by another half main pixel.
        half=max(half,splat_half)+config.pixel_world_size*0.5;
    }
    let local_member=config.drag_active!=0u && (drag_members[id/32u]&(1u<<(id%32u)))!=0u;
    let slot=remote_slots[id];
    let packed=remote_deltas.entries[(max(slot,1u)-1u)/2u];
    let remote_delta=select(packed.xy,packed.zw,slot!=0u && ((slot-1u)&1u)!=0u);
    let position=presentation_position(state.position,state.flags,local_member,config.drag_delta,slot,remote_delta);
    return (state.flags&16u)!=0u && all(position+half>=config.view_min) && all(position-half<=config.view_max);
}
@compute @workgroup_size(256) fn cull(@builtin(global_invocation_id) invocation:vec3<u32>) {
    let id=invocation.x;
    if is_visible(id) {
        let dst=atomicAdd(&args.instance_count,1u);visible[dst]=id;
    }
}
// Stable local compaction supplies ID order to the stable 24-bit radix sort.
// Each group initially owns its original 256-ID range; no capacity padding.
var<workgroup> visible_scan:array<u32,256>;
@compute @workgroup_size(256) fn cull_ordered(
    @builtin(global_invocation_id) invocation:vec3<u32>,
    @builtin(local_invocation_index) lane:u32,
    @builtin(workgroup_id) group:vec3<u32>,
) {
    let alive=is_visible(invocation.x);
    visible_scan[lane]=u32(alive);
    workgroupBarrier();
    for(var step=1u;step<256u;step*=2u) {
        var add=0u;if lane>=step {add=visible_scan[lane-step];}
        workgroupBarrier();
        visible_scan[lane]+=add;
        workgroupBarrier();
    }
    if alive {visible[group.x*256u+visible_scan[lane]-1u]=invocation.x;}
    if lane==255u {
        let count=visible_scan[255];group_counts[group.x]=count;
        atomicAdd(&args.instance_count,count);
    }
}
