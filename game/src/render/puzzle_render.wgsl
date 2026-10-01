#import puzzella::shape::{piece_profiles, piece_signed_distance, inside_piece, piece_uv}
struct PuzzleUniform {
    clip_from_world:mat4x4<f32>,seed:vec2<u32>,grid:vec2<u32>,image_size:vec2<f32>,size:vec2<f32>,
    view_min:vec2<f32>,view_max:vec2<f32>,count:u32,capacity:u32,opaque:u32,reserved:u32,
    selection_min:vec2<f32>,selection_max:vec2<f32>,selection_enabled:vec4<u32>,
    drag_delta:vec2<f32>,drag_active:u32,preview_active:u32,
};
struct PieceState {position:vec2<f32>,z_order:u32,flags:u32};
@group(0) @binding(0) var<uniform> config:PuzzleUniform;
@group(0) @binding(1) var<storage,read> states:array<PieceState>;
@group(0) @binding(2) var<storage,read> visible:array<u32>;
@group(0) @binding(3) var<storage,read> drag_members:array<u32>;
@group(0) @binding(4) var<storage,read> preview:array<u32>;
@group(1) @binding(0) var image:texture_2d<f32>;
@group(1) @binding(1) var image_sampler:sampler;
@group(2) @binding(0) var<storage,read_write> selection:array<atomic<u32>>;
@group(2) @binding(1) var<storage,read> selectable:array<u32>;
struct VertexOutput {
    @builtin(position) position:vec4<f32>,@location(0) local:vec2<f32>,@location(1) uv:vec2<f32>,
    @location(2) @interpolate(flat) id:u32,@location(3) @interpolate(flat) flags:u32,
    @location(4) @interpolate(flat) top:vec2<u32>,@location(5) @interpolate(flat) right:vec2<u32>,
    @location(6) @interpolate(flat) bottom:vec2<u32>,@location(7) @interpolate(flat) left:vec2<u32>,
};
@vertex fn vertex(@builtin(vertex_index) vi:u32,@builtin(instance_index) instance:u32)->VertexOutput {
    let corners=array<vec2<f32>,4>(vec2(-1.0,-1.0),vec2(1.0,-1.0),vec2(-1.0,1.0),vec2(1.0,1.0));
    let id=visible[instance];let state=states[id];let cell=vec2(id%config.grid.x,id/config.grid.x);
    let half=config.size*0.5+0.22*min(config.size.x,config.size.y);
    let local=corners[vi]*half;let edges=piece_profiles(config.seed,config.grid,cell);
    var position=state.position;
    if config.drag_active!=0u && (state.flags&8u)!=0u && (drag_members[id/32u]&(1u<<(id%32u)))!=0u {
        position+=config.drag_delta;
    }
    var out:VertexOutput;out.position=config.clip_from_world*vec4(position+local,0.0,1.0);
    // Reverse-Z in the exact 24-bit range; placed pieces have rank zero.
    let rank=select(state.z_order+2u,1u,(state.flags&1u)!=0u);
    out.position.z=f32(rank)/16777216.0*out.position.w;
    out.local=local;out.uv=piece_uv(cell,local,config.size,config.image_size);out.id=id;out.flags=state.flags;
    out.top=edges[0];out.right=edges[1];out.bottom=edges[2];out.left=edges[3];return out;
}
fn distance(in:VertexOutput)->f32 {return piece_signed_distance(in.local,config.size,array<vec2<u32>,4>(in.top,in.right,in.bottom,in.left));}
fn sample_visible(in:VertexOutput,d:f32)->vec4<f32> {
    if d>0.0 {discard;}
    let color=textureSample(image,image_sampler,in.uv);
    if color.a<=0.0 {discard;} return color;
}
@fragment fn fragment(in:VertexOutput)->@location(0) vec4<f32> {
    let d=distance(in);let color=sample_visible(in,d);
    // Evaluate derivatives before the per-piece highlight branch.
    let aa=fwidth(d);
    var flags=in.flags;
    if config.preview_active!=0u && (flags&9u)==0u && (preview[in.id/32u]&(1u<<(in.id%32u)))!=0u {flags|=4u;}
    if (flags&6u)==0u {return color;}
    let width=min(16.0,min(config.size.x,config.size.y)*0.16)*0.5;
    var line=vec3(0.3,0.6,1.0);
    if (flags&2u)!=0u {line=vec3(1.0,0.8,0.0);}
    let coverage=1.0-smoothstep(width-aa,width+aa,abs(d));
    return vec4(mix(color.rgb,line,coverage),color.a);
}
fn check_selectable(id:u32) {
    if (selectable[id/32u]&(1u<<(id%32u)))==0u {discard;}
}
@fragment fn point_fragment(in:VertexOutput)->@location(0) u32 {
    sample_visible(in,distance(in));check_selectable(in.id);return in.id+1u;
}
@fragment fn rectangle_fragment(in:VertexOutput) {
    sample_visible(in,distance(in));check_selectable(in.id);atomicOr(&selection[in.id/32u],1u<<(in.id%32u));
}

@vertex fn box_vertex(@builtin(vertex_index) vi:u32)->@builtin(position) vec4<f32> {
    let corners=array<vec2<f32>,4>(vec2(0.0,0.0),vec2(1.0,0.0),vec2(0.0,1.0),vec2(1.0,1.0));
    return config.clip_from_world*vec4(mix(config.selection_min,config.selection_max,corners[vi]),0.0,1.0);
}
@fragment fn box_fragment()->@location(0) vec4<f32> {return vec4(0.3,0.6,1.0,0.3);}
