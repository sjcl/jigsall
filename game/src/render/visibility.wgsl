struct PuzzleUniform {
    clip_from_world:mat4x4<f32>,seed:vec2<u32>,grid:vec2<u32>,image_size:vec2<f32>,size:vec2<f32>,
    view_min:vec2<f32>,view_max:vec2<f32>,count:u32,capacity:u32,opaque:u32,reserved:u32,
    selection_min:vec2<f32>,selection_max:vec2<f32>,selection_enabled:vec4<u32>,
    drag_delta:vec2<f32>,drag_active:u32,preview_active:u32,
};
struct PieceState {position:vec2<f32>,z_order:u32,flags:u32};
struct DrawArgs {vertex_count:u32,instance_count:atomic<u32>,first_vertex:u32,first_instance:u32};
@group(0) @binding(0) var<uniform> config:PuzzleUniform;
@group(0) @binding(1) var<storage,read> states:array<PieceState>;
@group(0) @binding(2) var<storage,read_write> visible:array<u32>;
@group(0) @binding(3) var<storage,read_write> args:DrawArgs;
@group(0) @binding(4) var<storage,read_write> selectable:array<u32>;
@group(0) @binding(5) var<storage,read> drag_members:array<u32>;
@compute @workgroup_size(256) fn cull(@builtin(global_invocation_id) invocation:vec3<u32>) {
    let id=invocation.x;if id>=config.count {return;}
    if (id%32u)==0u {
        var word=0u;for(var bit=0u;bit<32u && id+bit<config.count;bit++) {
            let flags=states[id+bit].flags;
            if (flags&9u)==0u && (flags&16u)!=0u {word|=1u<<bit;}
        }selectable[id/32u]=word;
    }
    let state=states[id];let half=config.size*0.5+0.22*min(config.size.x,config.size.y);
    var position=state.position;
    if config.drag_active!=0u && (state.flags&8u)!=0u && (drag_members[id/32u]&(1u<<(id%32u)))!=0u {
        position+=config.drag_delta;
    }
    if (state.flags&16u)!=0u && all(position+half>=config.view_min) && all(position-half<=config.view_max) {
        let dst=atomicAdd(&args.instance_count,1u);visible[dst]=id;
    }
}
@compute @workgroup_size(256) fn initialize_visible(@builtin(global_invocation_id) invocation:vec3<u32>) {
    if invocation.x<config.capacity {visible[invocation.x]=0xffffffffu;}
}
struct SortUniform {stride:u32,span:u32,capacity:u32,pad:u32};
@group(1) @binding(0) var<uniform> sort:SortUniform;
fn rank(id:u32)->u32 {if id==0xffffffffu {return 0xffffffffu;}let s=states[id];return select(s.z_order+1u,0u,(s.flags&1u)!=0u);}
// Ascending z for exact alpha blending. Sentinels sort behind all live entries.
@compute @workgroup_size(256) fn sort_visible(@builtin(global_invocation_id) invocation:vec3<u32>) {
    let i=invocation.x;let j=i^sort.stride;if j<=i || j>=sort.capacity {return;}
    let a=visible[i];let b=visible[j];let ra=rank(a);let rb=rank(b);
    let greater=ra>rb || (ra==rb && a>b);
    let ascending=(i&sort.span)==0u;
    if greater==ascending {visible[i]=b;visible[j]=a;}
}
