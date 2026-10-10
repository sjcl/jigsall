// Shared piece presentation for rendering and GPU picking.
struct RotationAnimation {
    pivot:vec2<f32>,offset:vec2<f32>,residual:f32,start:f32,duration:f32,start_elevation:f32,
};
struct PresentationPose { position:vec2<f32>,rotation:vec2<f32>,elevation:f32 };
// Same counterclockwise quarter turns and bits 9..10 as jigsall_core::rotation.
fn decode_rotation(flags:u32)->u32 {return (flags>>9u)&3u;}
fn rotate_quarter(v:vec2<f32>,rotation:u32)->vec2<f32> {
    switch rotation&3u {
        case 1u: {return vec2(-v.y,v.x);}
        case 2u: {return -v;}
        case 3u: {return vec2(v.y,-v.x);}
        default: {return v;}
    }
}
fn quarter_half_size(half:vec2<f32>,quarter:u32)->vec2<f32> {
    return select(half,half.yx,(quarter&1u)!=0u);
}
// The local axes align with world X/Y, so no length/sqrt is needed even with
// anisotropic viewport scales. Return local dimensions for rotate_quarter.
fn quarter_splat_size(size:vec2<f32>,quarter:u32,pixel_world_size:vec2<f32>,minimum_px:f32)->vec2<f32> {
    let local_pixel_size=quarter_half_size(pixel_world_size,quarter);
    return max(size,local_pixel_size*minimum_px);
}
// Shared by normal/far draw and main/pick visibility. Local membership wins.
fn presentation_position(canonical:vec2<f32>, flags:u32, local_member:bool, local_delta:vec2<f32>, remote_slot:u32, remote_delta:vec2<f32>)->vec2<f32> {
    if (flags&8u)==0u {return canonical;}
    if local_member {return canonical+local_delta;}
    if remote_slot!=0u {return canonical+remote_delta;}
    return canonical;
}
fn presentation_rotate(p:vec2<f32>,rotation:vec2<f32>)->vec2<f32> {
    return vec2(rotation.x*p.x-rotation.y*p.y,rotation.y*p.x+rotation.x*p.y);
}
fn presentation_half_size(half:vec2<f32>,rotation:vec2<f32>)->vec2<f32> {
    return vec2(abs(rotation.x)*half.x+abs(rotation.y)*half.y,abs(rotation.y)*half.x+abs(rotation.x)*half.y);
}
// Inflate in each local axis before rotation. Far splats keep the continuous
// orientation, including elongated pieces, and share this footprint with picks.
fn presentation_splat_size(size:vec2<f32>,rotation:vec2<f32>,pixel_world_size:vec2<f32>,minimum_px:f32)->vec2<f32> {
    let x_pixels_per_world=length(rotation/pixel_world_size);
    let y_pixels_per_world=length(vec2(-rotation.y,rotation.x)/pixel_world_size);
    return max(size,vec2(minimum_px/x_pixels_per_world,minimum_px/y_pixels_per_world));
}
// A single progress sample drives the rigid residual and visual lift envelope.
fn rotation_progress(animation:RotationAnimation,time:f32)->f32 {
    if animation.duration<=0.0 {return 1.0;}
    return clamp((time-animation.start)/animation.duration,0.0,1.0);
}
// Same cheap polynomial as RotationAnimation::elevation on the CPU. This
// normalized scalar is reserved for future effects; never alter geometry here.
fn rotation_elevation(animation:RotationAnimation,progress:f32)->f32 {
    if progress>=1.0 {return 0.0;}
    let quarter_turn=1.5707963267948966;
    let peak=max(animation.start_elevation,clamp(abs(animation.residual)/quarter_turn,0.0,1.0));
    if progress<0.5 {
        let t=progress*2.0;
        let eased=t*t*(3.0-2.0*t);
        return animation.start_elevation+(peak-animation.start_elevation)*eased;
    }
    let t=progress*2.0-1.0;
    return peak*(1.0-t*t*(3.0-2.0*t));
}
fn presentation_pose(canonical:vec2<f32>,flags:u32,local_member:bool,local_delta:vec2<f32>,remote_slot:u32,remote_delta:vec2<f32>,animation:RotationAnimation,time:f32)->PresentationPose {
    let quarter=decode_rotation(flags);
    let rotations=array<vec2<f32>,4>(vec2(1.0,0.0),vec2(0.0,1.0),vec2(-1.0,0.0),vec2(0.0,-1.0));
    var rotation=rotations[quarter];
    var base=canonical;
    let progress=rotation_progress(animation,time);
    if progress<1.0 {
        let remaining=1.0-progress*progress*(3.0-2.0*progress);
        let angle=animation.residual*remaining;
        let residual=vec2(cos(angle),sin(angle));
        base=animation.pivot+presentation_rotate(canonical-animation.pivot,residual)+animation.offset*remaining;
        rotation=presentation_rotate(rotation,residual);
    }
    return PresentationPose(presentation_position(base,flags,local_member,local_delta,remote_slot,remote_delta),rotation,rotation_elevation(animation,progress));
}
