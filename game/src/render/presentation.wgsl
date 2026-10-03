#define_import_path puzzella::presentation
// Shared by normal/far draw and main/pick visibility. Local membership wins.
fn presentation_position(canonical:vec2<f32>, flags:u32, local_member:bool, local_delta:vec2<f32>, remote_slot:u32, remote_delta:vec2<f32>)->vec2<f32> {
    if (flags&8u)==0u {return canonical;}
    if local_member {return canonical+local_delta;}
    if remote_slot!=0u {return canonical+remote_delta;}
    return canonical;
}
