#define_import_path jigsall::shape

// Mirror puzzle/src/procedural.rs; changing shape constants changes generator compatibility.
const TAB_WIDTH_SCALE:f32=0.70;
const ROOT_WIDTH_FACTOR:f32=0.60;
const ROOT_HEIGHT_FACTOR:f32=0.25;
const ROOT_BLEND_FACTOR:f32=0.04;
struct EdgeProfile { polarity: f32, center: f32, width: f32, depth: f32, neck: f32, head: f32, asymmetry: f32 };
fn mix32(value: u32) -> u32 {
    var x=value; x ^= x >> 16u; x *= 0x7feb352du;
    x ^= x >> 15u; x *= 0x846ca68bu; return x ^ (x >> 16u);
}
fn edge_base(seed: vec2<u32>, orientation: u32, x: u32, y: u32) -> u32 {
    var h=mix32(seed.x ^ 0x9e3779b9u); h=mix32(h ^ seed.y);
    h=mix32(h ^ select(0x484f5249u,0x56455254u,orientation==1u));
    h=mix32(h ^ x); h=mix32(h ^ y); return h;
}
fn edge_hash(seed:vec2<u32>,orientation:u32,x:u32,y:u32,domain:u32)->u32 {return mix32(edge_base(seed,orientation,x,y)^(domain*0x9e3779b9u));}
fn raw_profile(seed: vec2<u32>, orientation: u32, x: u32, y: u32) -> vec2<u32> {
    let base=edge_base(seed,orientation,x,y);
    let h=mix32(base);
    return vec2(((h >> 1u)%6u+1u) | ((h&1u)<<3u)
        | ((mix32(base^(1u*0x9e3779b9u))>>24u)<<4u)
        | ((mix32(base^(2u*0x9e3779b9u))>>24u)<<12u)
        | ((mix32(base^(3u*0x9e3779b9u))>>24u)<<20u),
        (mix32(base^(4u*0x9e3779b9u))>>24u)
        | ((mix32(base^(5u*0x9e3779b9u))>>24u)<<8u)
        | ((mix32(base^(6u*0x9e3779b9u))>>24u)<<16u));
}
fn class_sample(word:u32,shift:u32,count:u32)->vec2<f32> {
    let scaled=((word>>shift)&255u)*count;
    return vec2(f32(scaled>>8u),f32(scaled&255u)/255.0*2.0-1.0);
}
fn dimension(word:u32,shift:u32)->f32 {
    let s=class_sample(word,shift,4u);
    return (0.86+s.x*(0.28/3.0))*(1.0+s.y*0.01);
}
fn edge_profile(raw: vec2<u32>) -> EdgeProfile {
    // Fold style-dependent ratios and depth intervals before per-fragment decoding.
    // Width, depth low, neck/head ratio, head/width ratio; a separate depth step.
    var base=vec4(0.46,0.18*0.85/0.99,0.14/0.28,0.28/0.46);
    var depth_step=(min(0.18*1.15,0.215)/1.01-0.18*0.85/0.99)/3.0;
    switch raw.x&7u {
        case 2u: {base=vec4(0.52,0.17*0.85/0.99,0.18/0.34,0.34/0.52);depth_step=(min(0.17*1.15,0.215)/1.01-0.17*0.85/0.99)/3.0;}
        case 3u: {base=vec4(0.40,0.18*0.85/0.99,0.12/0.23,0.23/0.40);}
        case 4u: {base=vec4(0.46,0.21*0.85/0.99,0.14/0.27,0.27/0.46);depth_step=(min(0.21*1.15,0.215)/1.01-0.21*0.85/0.99)/3.0;}
        case 5u: {base=vec4(0.46,0.14*0.85/0.99,0.16/0.28,0.28/0.46);depth_step=(min(0.14*1.15,0.215)/1.01-0.14*0.85/0.99)/3.0;}
        case 6u: {base=vec4(0.48,0.19*0.85/0.99,0.12/0.32,0.32/0.48);depth_step=(min(0.19*1.15,0.215)/1.01-0.19*0.85/0.99)/3.0;}
        default: {}
    }
    let width=base.x*dimension(raw.x,12u);
    let head=width*base.w*dimension(raw.y,8u);
    let n=class_sample(raw.y,0u,4u);
    let neck=head*base.z*(0.70+n.x*0.16)*(1.0+n.y*0.01);
    let d=class_sample(raw.x,20u,4u);
    let depth=(base.y+depth_step*d.x)*(1.0+d.y*0.01);
    let s=class_sample(raw.y,16u,7u);
    let skew=-0.10+s.x*(0.20/6.0)+s.y*0.005;
    let root=neck*0.5+(width-neck)*0.5*ROOT_WIDTH_FACTOR;
    let envelope=max(root+0.00215,head*(0.5347222+abs(skew)));
    let span=min(0.08,(0.5-0.185-envelope)/1.05);
    let c=class_sample(raw.x,4u,7u);
    let center=0.5+span*(-1.0+c.x/3.0+c.y*0.05);
    // Scale lateral dimensions together, preserving ratios, depth and center classes.
    return EdgeProfile(select(1.0,-1.0,(raw.x&8u)!=0u),center,width*TAB_WIDTH_SCALE,depth,neck*TAB_WIDTH_SCALE,head*TAB_WIDTH_SCALE,skew);
}
fn piece_profiles(seed: vec2<u32>,grid: vec2<u32>,cell: vec2<u32>) -> array<vec2<u32>,4> {
    var edges: array<vec2<u32>,4>;
    if cell.y>0u {edges[0]=raw_profile(seed,0u,cell.x,cell.y);}
    if cell.x+1u<grid.x {edges[1]=raw_profile(seed,1u,cell.x+1u,cell.y);}
    if cell.y+1u<grid.y {edges[2]=raw_profile(seed,0u,cell.x,cell.y+1u);}
    if cell.x>0u {edges[3]=raw_profile(seed,1u,cell.x,cell.y);}
    return edges;
}
fn sd_box(p: vec2<f32>,half: vec2<f32>,r:f32) -> f32 {
    let q=abs(p)-half+r; return length(max(q,vec2(0.0)))+min(max(q.x,q.y),0.0)-r;
}
fn smooth_min(a:f32,b:f32,k:f32) -> f32 {
    let h=clamp(0.5+0.5*(b-a)/k,0.0,1.0); return mix(b,a,h)-k*h*(1.0-h);
}
// Quarter-ellipse fillet: horizontal baseline tangent, vertical neck tangent.
fn sd_root(q:vec2<f32>,neck_half:f32,root_half:f32,height:f32)->f32 {
    let radii=vec2(root_half-neck_half,height);
    let outside_ellipse=(1.0-length(vec2(abs(q.x)-root_half,q.y-height)/radii))*min(radii.x,radii.y);
    return max(max(max(outside_ellipse,abs(q.x)-root_half),q.y-height),-q.y);
}
fn sd_tab(q:vec2<f32>,p:EdgeProfile,len:f32,short:f32) -> f32 {
    let depth=p.depth*short; let x=q.x-p.center*len; let neck=p.neck*len;
    // Lengthen the head toward the neck while keeping the tip at 0.98 * depth.
    let radii=vec2(p.head*len*0.5,depth*0.42);
    let head=(length(vec2(x-p.asymmetry*p.head*len,q.y-depth*0.56)/radii)-1.0)*min(radii.x,radii.y);
    let stem=sd_box(vec2(x,q.y-depth*0.23),vec2(neck*0.5,depth*0.28),min(neck,depth)*0.18);
    let neck_half=neck*0.5;
    let root_half=neck_half+(p.width*len*0.5-neck_half)*ROOT_WIDTH_FACTOR;
    let root=sd_root(vec2(x,q.y),neck_half,root_half,depth*ROOT_HEIGHT_FACTOR);
    return max(smooth_min(smooth_min(head,stem,depth*0.06),root,depth*ROOT_BLEND_FACTOR),-q.y);
}
fn edge_distance(q:vec2<f32>,raw:vec2<u32>,len:f32,short:f32) -> f32 {
    // sd_tab(q)>=-q.y makes the complement exactly q.y on this half-plane.
    if raw.x==0u || ((raw.x&8u)==0u && q.y<=0.0) || ((raw.x&8u)!=0u && q.y>=0.0) {return q.y;}
    let p=edge_profile(raw);
    if p.polarity>0.0 {return min(q.y,sd_tab(q,p,len,short));}
    return max(q.y,-sd_tab(vec2(q.x,-q.y),p,len,short));
}
fn piece_edge_distances(local:vec2<f32>,size:vec2<f32>,edges:array<vec2<u32>,4>) -> vec4<f32> {
    let h=size*0.5; let s=min(size.x,size.y);
    let top=edge_distance(vec2(local.x+h.x,local.y-h.y),edges[0],size.x,s);
    let right=edge_distance(vec2(h.y-local.y,local.x-h.x),edges[1],size.y,s);
    let bottom=-edge_distance(vec2(local.x+h.x,local.y+h.y),edges[2],size.x,s);
    let left=-edge_distance(vec2(h.y-local.y,local.x+h.x),edges[3],size.y,s);
    return vec4(top,right,bottom,left);
}
fn max_edge_distance(edges:vec4<f32>) -> f32 {
    return max(max(edges.x,edges.y),max(edges.z,edges.w));
}
fn piece_signed_distance(local:vec2<f32>,size:vec2<f32>,edges:array<vec2<u32>,4>) -> f32 {
    return max_edge_distance(piece_edge_distances(local,size,edges));
}
fn inside_piece(local:vec2<f32>,size:vec2<f32>,edges:array<vec2<u32>,4>) -> bool {
    return piece_signed_distance(local,size,edges)<=0.0;
}
fn piece_uv(cell:vec2<u32>,local:vec2<f32>,size:vec2<f32>,image_size:vec2<f32>) -> vec2<f32> {
    return ((vec2<f32>(cell)+0.5)*size+vec2(local.x,-local.y))/image_size;
}
