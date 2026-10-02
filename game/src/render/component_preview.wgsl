// Rectangle-only reduction; direct hits retain their PieceId bits for readback.
@group(0) @binding(0) var<storage,read> direct_hits:array<u32>;
@group(0) @binding(1) var<storage,read> component_roots:array<u32>;
@group(0) @binding(2) var<storage,read> selectable:array<u32>;
@group(0) @binding(3) var<storage,read_write> preview:array<atomic<u32>>;
@group(0) @binding(4) var<storage,read_write> rejected:array<atomic<u32>>;

fn write_roots(word:u32,bits:u32,reject:bool) {
    if reject {atomicOr(&rejected[word],bits);}
    else {atomicOr(&preview[word],bits);}
}
fn mark_roots(word:u32,bits:u32,reject:bool) {
    var remaining=bits;
    var root_word=0xffffffffu;
    var root_bits=0u;
    while remaining!=0u {
        let bit=firstTrailingBit(remaining);
        remaining&=remaining-1u;
        let id=word*32u+bit;
        if id>=arrayLength(&component_roots) {continue;}
        let root=component_roots[id];
        let next_word=root/32u;
        // Combine roots sharing a mask word before the atomic. In particular,
        // a giant component contributes once per input word, not once per hit.
        if next_word!=root_word {
            if root_bits!=0u {write_roots(root_word,root_bits,reject);}
            root_word=next_word;root_bits=0u;
        }
        root_bits|=1u<<(root%32u);
    }
    if root_bits!=0u {write_roots(root_word,root_bits,reject);}
}
@compute @workgroup_size(256) fn collapse_components(@builtin(global_invocation_id) id:vec3<u32>) {
    if id.x>=arrayLength(&direct_hits) {return;}
    mark_roots(id.x,direct_hits[id.x],false);
    // Whole-component rejection matches CPU canonical_members even with mixed
    // HELD / PLACED / ENABLED members. All-selectable words cost no root lookups.
    mark_roots(id.x,~selectable[id.x],true);
}
@compute @workgroup_size(256) fn filter_components(@builtin(global_invocation_id) id:vec3<u32>) {
    if id.x>=arrayLength(&preview) {return;}
    atomicAnd(&preview[id.x],~atomicLoad(&rejected[id.x]));
}
