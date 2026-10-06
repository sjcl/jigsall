// Rectangle-only reduction; direct hits retain their PieceId bits for readback.
@group(0) @binding(0) var<storage,read> direct_hits:array<u32>;
@group(0) @binding(1) var<storage,read> piece_metadata:array<u32>;
@group(0) @binding(2) var<storage,read_write> preview:array<atomic<u32>>;

fn mark_roots(word:u32,bits:u32) {
    var remaining=bits;
    var root_word=0xffffffffu;
    var root_bits=0u;
    while remaining!=0u {
        let bit=firstTrailingBit(remaining);
        remaining&=remaining-1u;
        let id=word*32u+bit;
        // This binding exposes only the root region, regardless of optional
        // presentation regions/tails in the underlying metadata allocation.
        if id>=arrayLength(&piece_metadata) {continue;}
        let root=piece_metadata[id];
        let next_word=root/32u;
        // Combine roots sharing a mask word before the atomic. In particular,
        // a giant component contributes once per input word, not once per hit.
        if next_word!=root_word {
            if root_bits!=0u {atomicOr(&preview[root_word],root_bits);}
            root_word=next_word;root_bits=0u;
        }
        root_bits|=1u<<(root%32u);
    }
    if root_bits!=0u {atomicOr(&preview[root_word],root_bits);}
}
@compute @workgroup_size(256) fn collapse_components(@builtin(global_invocation_id) id:vec3<u32>) {
    if id.x>=arrayLength(&direct_hits) {return;}
    // Picking admits only selectable hits. Authority commands and validated
    // restore keep selectability uniform within each connected component.
    mark_roots(id.x,direct_hits[id.x]);
}
