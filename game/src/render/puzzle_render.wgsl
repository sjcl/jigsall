#import jigsall::presentation::{presentation_pose, presentation_position, presentation_rotate, presentation_splat_size, decode_rotation, rotate_quarter, quarter_splat_size, RotationAnimation}
#import jigsall::shape::{piece_profiles, piece_signed_distance, piece_edge_distances, max_edge_distance, inside_piece, piece_uv}
struct PuzzleUniform {
    clip_from_world:mat4x4<f32>,seed:vec2<u32>,grid:vec2<u32>,image_size:vec2<f32>,size:vec2<f32>,
    view_min:vec2<f32>,view_max:vec2<f32>,count:u32,capacity:u32,opaque:u32,reserved:u32,
    selection_min:vec2<f32>,selection_max:vec2<f32>,selection_enabled:vec4<u32>,
    drag_delta:vec2<f32>,drag_active:u32,preview_active:u32,
    viewport_size:vec2<f32>,viewport_origin:vec2<f32>,
    piece_size_px:vec2<f32>,pixel_world_size:vec2<f32>,
    render_clip_scale:vec2<f32>,render_clip_offset:vec2<f32>,
    far_zoom:u32,splat_min_px:f32,splat_padding:vec2<u32>,
    rotation_time:f32,rotation_active:u32,rotation_padding:vec2<u32>,
    pseudo_3d_direction:vec2<f32>,shadow_base_offset_px:f32,shadow_lift_offset_px:f32,
    shadow_opacity:f32,shadow_enabled:u32,shadow_padding:vec2<u32>,
    visual_cull_extent:vec2<f32>,bevel_width_px:f32,bevel_enabled:u32,
    side_color:vec4<f32>,side_thickness_px:f32,side_enabled:u32,
    bevel_highlight_strength:f32,bevel_shadow_strength:f32,
};
struct PieceState {position:vec2<f32>,z_order:u32,flags:u32};
@group(0) @binding(0) var<uniform> config:PuzzleUniform;
@group(0) @binding(1) var<storage,read> states:array<PieceState>;
@group(0) @binding(2) var<storage,read> visible:array<u32>;
@group(0) @binding(3) var<storage,read> drag_members:array<u32>;
@group(0) @binding(4) var<storage,read> preview:array<u32>;
@group(0) @binding(5) var<storage,read> selected:array<u32>;
@group(0) @binding(6) var<storage,read> piece_metadata:array<u32>;
struct RemoteDeltas { entries:array<vec4<f32>,32> };
@group(0) @binding(7) var<uniform> remote_deltas:RemoteDeltas;
@group(0) @binding(8) var<storage,read> rotation_animations:array<RotationAnimation>;
// SoA regions use buffer capacity, not the current visible/piece count.
fn component_root(id:u32)->u32 {return piece_metadata[id];}
fn remote_slot(id:u32)->u32 {return piece_metadata[config.capacity+id];}
fn rotation_slot(root:u32)->u32 {return piece_metadata[2u*config.capacity+root];}
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
fn screen_offset(position:vec4<f32>,offset_px:vec2<f32>)->vec4<f32> {
    return vec4(position.xy+offset_px/config.viewport_size*vec2(2.0,-2.0)*position.w,position.zw);
}
// Literal entry-point arguments remove optional work from top/picking. No new
// varying; only shadow consumes elevation, while side has static thickness.
fn piece_vertex(vi:u32,instance:u32,shadow:bool,side:bool)->VertexOutput {
    let corners=array<vec2<f32>,4>(vec2(-1.0,-1.0),vec2(1.0,-1.0),vec2(-1.0,1.0),vec2(1.0,1.0));
    let id=visible[instance];let state=states[id];let cell=vec2(id%config.grid.x,id/config.grid.x);
    let local_member=config.drag_active!=0u && (drag_members[id/32u]&(1u<<(id%32u)))!=0u;
    let slot=remote_slot(id);
    let packed=remote_deltas.entries[(max(slot,1u)-1u)/2u];
    let remote_delta=select(packed.xy,packed.zw,slot!=0u && ((slot-1u)&1u)!=0u);
    // Uniform gate avoids both metadata lookups on ordinary frames; a zero
    // component slot also keeps unrelated pieces on the quarter-turn path.
    var animation_slot=0u;
    if config.rotation_active!=0u {
        animation_slot=rotation_slot(component_root(id));
    }
    let quarter=decode_rotation(state.flags);
    var position=presentation_position(state.position,state.flags,local_member,config.drag_delta,slot,remote_delta);
    var rotation:vec2<f32>;
    var elevation=0.0;
    if animation_slot!=0u {
        let animation=rotation_animations[animation_slot-1u];
        let pose=presentation_pose(state.position,state.flags,local_member,config.drag_delta,slot,remote_delta,animation,config.rotation_time);
        position=pose.position;rotation=pose.rotation;
        if shadow {elevation=pose.elevation;}
    }
    var out:VertexOutput;
    if config.far_zoom!=0u {
        let center=config.clip_from_world*vec4(position,0.0,1.0);
        let center_px=config.viewport_origin+(center.xy/center.w*vec2(0.5,-0.5)+0.5)*config.viewport_size;
        let snapped_px=floor(center_px)+0.5;
        var world_offset:vec2<f32>;
        if animation_slot==0u {
            let splat_size=quarter_splat_size(config.size,quarter,config.pixel_world_size,config.splat_min_px);
            world_offset=rotate_quarter(corners[vi]*splat_size*0.5,quarter);
        } else {
            let splat_size=presentation_splat_size(config.size,rotation,config.pixel_world_size,config.splat_min_px);
            world_offset=presentation_rotate(corners[vi]*splat_size*0.5,rotation);
        }
        let offset=world_offset/config.pixel_world_size;
        let pixel=snapped_px+offset*vec2(1.0,-1.0);
        let ndc=(pixel-config.viewport_origin)/config.viewport_size*vec2(2.0,-2.0)+vec2(-1.0,1.0);
        out.position=vec4(ndc*center.w,center.z,center.w);
        // Snap first, crop second: a 1x1 point target keeps the main footprint.
        out.position=vec4(out.position.xy*config.render_clip_scale+config.render_clip_offset*out.position.w,out.position.zw);
        // Constant cell-center UV; no edge hash, profile decoding or SDF data.
        out.uv=(vec2<f32>(cell)+0.5)/vec2<f32>(config.grid);
    } else {
        let half=config.size*0.5+0.22*min(config.size.x,config.size.y);
        let local=corners[vi]*half;let edges=piece_profiles(config.seed,config.grid,cell);
        var world_offset:vec2<f32>;
        if animation_slot==0u {world_offset=rotate_quarter(local,quarter);}
        else {world_offset=presentation_rotate(local,rotation);}
        out.position=config.clip_from_world*vec4(position+world_offset,0.0,1.0);
        out.local=local;out.uv=piece_uv(cell,local,config.size,config.image_size);
        out.top=edges[0];out.right=edges[1];out.bottom=edges[2];out.left=edges[3];
    }
    // Reverse-Z: loose ranks are >=2; exact placed pieces keep rank 1.
    // Far placed splats overlap, so unique ranks in [1,2) make the largest ID
    // win independently of opaque culling's unordered atomic append.
    var rank=f32(state.z_order+2u);
    if (state.flags&1u)!=0u {
        rank=1.0;
        if config.far_zoom!=0u {rank+=f32(id)/f32(config.count);}
    }
    out.position.z=rank/16777216.0*out.position.w;
    if shadow {
        // Preserve rank order in temporary shadow depth, leaving headroom for
        // the color pass's 1 ULP even at the maximum canonical loose Z rank.
        out.position.z*=0.5;
        let offset_px=config.pseudo_3d_direction*(config.shadow_base_offset_px+elevation*config.shadow_lift_offset_px);
        out.position=screen_offset(out.position,offset_px);
    }
    if side {
        out.position=screen_offset(out.position,config.pseudo_3d_direction*config.side_thickness_px);
    }
    out.id=id;out.flags=state.flags&~6u;
    // An explicit branch keeps ordinary frames from loading component roots.
    if !shadow && !side && config.preview_active!=0u && (state.flags&9u)==0u {
        let root=component_root(id);
        if (preview[root/32u]&(1u<<(root%32u)))!=0u {out.flags|=4u;}
    }
    return out;
}
@vertex fn vertex(@builtin(vertex_index) vi:u32,@builtin(instance_index) instance:u32)->VertexOutput {
    return piece_vertex(vi,instance,false,false);
}
@vertex fn shadow_vertex(@builtin(vertex_index) vi:u32,@builtin(instance_index) instance:u32)->VertexOutput {
    return piece_vertex(vi,instance,true,false);
}
@vertex fn side_vertex(@builtin(vertex_index) vi:u32,@builtin(instance_index) instance:u32)->VertexOutput {
    return piece_vertex(vi,instance,false,true);
}
fn distance(in:VertexOutput)->f32 {return piece_signed_distance(in.local,config.size,array<vec2<u32>,4>(in.top,in.right,in.bottom,in.left));}
fn outer_boundary_distance(edges:vec4<f32>,flags:u32)->f32 {
    // Bits 5..8: canonical top/right/bottom/left (resources/pieces.rs). An enclosed
    // piece has no outline candidates. Coverage/picking still use every edge.
    let connected=(vec4(flags)&vec4(32u,64u,128u,256u))!=vec4(0u);
    return max_edge_distance(select(edges,vec4(-1e20),connected));
}
// No derivatives or presentation state here. The top entry point supplies a
// screen-space gradient before any silhouette/alpha discard or per-piece branch.
fn bevel_color(color:vec4<f32>,boundary:f32,gradient:vec2<f32>)->vec4<f32> {
    let gradient_len=max(length(gradient),1e-6);
    let inside_px=max(-boundary/gradient_len,0.0);
    let coverage=1.0-smoothstep(0.0,max(config.bevel_width_px,1e-6),inside_px);
    let nl=dot(gradient/gradient_len,-config.pseudo_3d_direction);
    let highlight=max(nl,0.0)*coverage*config.bevel_highlight_strength;
    let shade=max(-nl,0.0)*coverage*config.bevel_shadow_strength;
    let rgb=color.rgb+highlight*(vec3(1.0)-color.rgb)-shade*color.rgb;
    return vec4(clamp(rgb,vec3(0.0),vec3(1.0)),color.a);
}
fn sample_visible(in:VertexOutput,d:f32)->vec4<f32> {
    if d>0.0 {discard;}
    let color=textureSample(image,image_sampler,in.uv);
    if color.a<=0.0 {discard;} return color;
}
fn sample_splat(in:VertexOutput)->vec4<f32> {
    // Keep alpha semantics shared with both pick paths: a fully transparent
    // center sample produces no color, depth or hit. No opaque fallback color.
    let color=textureSampleLevel(image,image_sampler,in.uv,0.0);
    if color.a<=0.0 {discard;} return color;
}
fn pick_visible(in:VertexOutput) {
    if config.far_zoom!=0u {sample_splat(in);} else {sample_visible(in,distance(in));}
}
fn silhouette_source(in:VertexOutput)->vec4<f32> {
    if config.far_zoom!=0u {return sample_splat(in);}
    return sample_visible(in,distance(in));
}
@fragment fn shadow_depth_fragment(in:VertexOutput)->@location(0) vec4<f32> {
    silhouette_source(in);
    return vec4(0.0);
}
struct ShadowOutput { @location(0) color:vec4<f32>, @builtin(frag_depth) depth:f32 };
@fragment fn shadow_fragment(in:VertexOutput)->ShadowOutput {
    let source=silhouette_source(in);
    // Color uses strict Greater against the completed depth prepass. Advance
    // one positive Depth32Float ULP, so only a frontmost silhouette passes and
    // writes a depth that also rejects any equal-rank duplicate. Late depth
    // testing uses this output, after alpha/SDF discard. Top clears depth again.
    return ShadowOutput(vec4(vec3(0.0),config.shadow_opacity*source.a),bitcast<f32>(bitcast<u32>(in.position.z)+1u));
}
@fragment fn side_fragment(in:VertexOutput)->@location(0) vec4<f32> {
    let source=silhouette_source(in);
    return vec4(config.side_color.rgb,config.side_color.a*source.a);
}
@fragment fn fragment(in:VertexOutput)->@location(0) vec4<f32> {
    if config.far_zoom!=0u {
        let color=sample_splat(in);
        var flags=in.flags;
        if (selected[in.id/32u]&(1u<<(in.id%32u)))!=0u {flags|=2u;}
        if (flags&2u)!=0u {return vec4(mix(color.rgb,vec3(1.0,0.8,0.0),0.5),color.a);}
        if (flags&4u)!=0u {return vec4(mix(color.rgb,vec3(0.3,0.6,1.0),0.5),color.a);}
        return color;
    }
    let edges=piece_edge_distances(in.local,config.size,array<vec2<u32>,4>(in.top,in.right,in.bottom,in.left));
    let d=max_edge_distance(edges);
    // Evaluate derivatives before discard and non-uniform per-piece branches.
    let aa=fwidth(d);
    var bevel_boundary=0.0;
    var bevel_gradient=vec2(0.0);
    if config.bevel_enabled!=0u {
        // An enclosed piece supplies a finite constant instead of differentiating
        // the absent-boundary sentinel. It also skips all lighting work below.
        bevel_boundary=select(outer_boundary_distance(edges,in.flags),0.0,(in.flags&480u)==480u);
        bevel_gradient=vec2(dpdx(bevel_boundary),dpdy(bevel_boundary));
    }
    var color=sample_visible(in,d);
    if config.bevel_enabled!=0u {
        if (in.flags&480u)!=480u {color=bevel_color(color,bevel_boundary,bevel_gradient);}
    }
    var flags=in.flags;
    if (selected[in.id/32u]&(1u<<(in.id%32u)))!=0u {flags|=2u;}
    if (flags&6u)==0u {return color;}
    let width=min(16.0,min(config.size.x,config.size.y)*0.16)*0.5;
    var line=vec3(0.3,0.6,1.0);
    var boundary=d;
    if (flags&2u)!=0u {
        line=vec3(1.0,0.8,0.0);
    }
    if (flags&480u)!=0u {boundary=outer_boundary_distance(edges,flags);}
    let coverage=1.0-smoothstep(width-aa,width+aa,abs(boundary));
    return vec4(mix(color.rgb,line,coverage),color.a);
}
fn check_selectable(id:u32) {
    if (selectable[id/32u]&(1u<<(id%32u)))==0u {discard;}
}
@fragment fn point_fragment(in:VertexOutput)->@location(0) u32 {
    pick_visible(in);check_selectable(in.id);return in.id+1u;
}
@fragment fn rectangle_fragment(in:VertexOutput) {
    pick_visible(in);check_selectable(in.id);atomicOr(&selection[in.id/32u],1u<<(in.id%32u));
}

@vertex fn box_vertex(@builtin(vertex_index) vi:u32)->@builtin(position) vec4<f32> {
    let corners=array<vec2<f32>,4>(vec2(0.0,0.0),vec2(1.0,0.0),vec2(0.0,1.0),vec2(1.0,1.0));
    return config.clip_from_world*vec4(mix(config.selection_min,config.selection_max,corners[vi]),0.0,1.0);
}
@fragment fn box_fragment()->@location(0) vec4<f32> {return vec4(0.3,0.6,1.0,0.3);}
