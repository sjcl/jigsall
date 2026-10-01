struct PieceState {position:vec2<f32>,z_order:u32,flags:u32};
struct DrawArgs {vertex_count:u32,instance_count:u32,first_vertex:u32,first_instance:u32};
struct SortUniform {shift:u32,count:u32,groups:u32,pad:u32};
@group(0) @binding(0) var<uniform> sort:SortUniform;
@group(0) @binding(1) var<storage,read> states:array<PieceState>;
@group(0) @binding(2) var<storage,read> input_ids:array<u32>;
@group(0) @binding(3) var<storage,read_write> output_ids:array<u32>;
@group(0) @binding(4) var<storage,read> args:DrawArgs;
@group(0) @binding(5) var<storage,read_write> group_counts:array<u32>;
// Bucket-major group counts / exclusive offsets, followed by 256 bucket totals.
@group(0) @binding(6) var<storage,read_write> histogram:array<u32>;
// Two dispatch commands: visible groups, then 256 bucket-scan groups (or zero).
@group(1) @binding(0) var<storage,read_write> dispatch:array<u32>;

var<workgroup> scan:array<u32,256>;
var<workgroup> bins:array<atomic<u32>,256>;
var<workgroup> members:array<atomic<u32>,2048>;

fn inclusive_scan(lane:u32)->u32 {
    workgroupBarrier();
    for(var step=1u;step<256u;step*=2u) {
        var add=0u;if lane>=step {add=scan[lane-step];}
        workgroupBarrier();
        scan[lane]+=add;
        workgroupBarrier();
    }
    return scan[lane];
}
fn digit(id:u32)->u32 {
    let s=states[id];
    // Placed pieces precede loose pieces. z_order + 1 fits in 24 bits.
    let rank=select(s.z_order+1u,0u,(s.flags&1u)!=0u);
    return (rank>>sort.shift)&255u;
}

// Scan culling's group counts in ID order and generate GPU-only dispatch sizes.
@compute @workgroup_size(256) fn prepare_sort(@builtin(local_invocation_index) lane:u32) {
    let chunk=(sort.groups+255u)/256u;
    let begin=lane*chunk;let end=min(begin+chunk,sort.groups);
    var sum=0u;for(var i=begin;i<end;i++) {sum+=group_counts[i];}
    scan[lane]=sum;
    var offset=inclusive_scan(lane)-sum;
    for(var i=begin;i<end;i++) {
        let count=group_counts[i];group_counts[i]=offset;offset+=count;
    }
    if lane==255u {
        group_counts[sort.groups]=scan[255];
        dispatch[0]=(args.instance_count+255u)/256u;dispatch[1]=1u;dispatch[2]=1u;
        dispatch[3]=select(0u,256u,args.instance_count!=0u);dispatch[4]=1u;dispatch[5]=1u;
    }
}
@compute @workgroup_size(256) fn compact_visible(
    @builtin(local_invocation_index) lane:u32,@builtin(workgroup_id) group:vec3<u32>,
) {
    let begin=group_counts[group.x];let count=group_counts[group.x+1u]-begin;
    if lane<count {output_ids[begin+lane]=input_ids[group.x*256u+lane];}
}
@compute @workgroup_size(256) fn radix_histogram(
    @builtin(global_invocation_id) id:vec3<u32>,
    @builtin(local_invocation_index) lane:u32,@builtin(workgroup_id) group:vec3<u32>,
) {
    atomicStore(&bins[lane],0u);workgroupBarrier();
    if id.x<args.instance_count {atomicAdd(&bins[digit(input_ids[id.x])],1u);}
    workgroupBarrier();
    histogram[lane*sort.groups+group.x]=atomicLoad(&bins[lane]);
}
// One group per bucket. Each lane scans a contiguous slice of live groups.
// This keeps the scan parallel even for an entire million-piece view.
@compute @workgroup_size(256) fn radix_scan(
    @builtin(local_invocation_index) lane:u32,@builtin(workgroup_id) bucket:vec3<u32>,
) {
    let live_groups=(args.instance_count+255u)/256u;
    let chunk=(live_groups+255u)/256u;
    let begin=lane*chunk;let end=min(begin+chunk,live_groups);
    let base=bucket.x*sort.groups;
    var sum=0u;for(var i=begin;i<end;i++) {sum+=histogram[base+i];}
    scan[lane]=sum;
    var offset=inclusive_scan(lane)-sum;
    for(var i=begin;i<end;i++) {
        let count=histogram[base+i];histogram[base+i]=offset;offset+=count;
    }
    if lane==255u {histogram[256u*sort.groups+bucket.x]=scan[255];}
}
@compute @workgroup_size(256) fn radix_scatter(
    @builtin(global_invocation_id) id:vec3<u32>,
    @builtin(local_invocation_index) lane:u32,@builtin(workgroup_id) group:vec3<u32>,
) {
    for(var i=lane;i<2048u;i+=256u) {atomicStore(&members[i],0u);}
    let total=histogram[256u*sort.groups+lane];scan[lane]=total;
    let inclusive=inclusive_scan(lane);
    // All lanes have initialized the membership masks before any atomicOr.
    var piece=0u;var bucket=0u;
    if id.x<args.instance_count {
        piece=input_ids[id.x];bucket=digit(piece);
        atomicOr(&members[bucket*8u+lane/32u],1u<<(lane%32u));
    }
    workgroupBarrier();
    if id.x>=args.instance_count {return;}
    var local_rank=0u;
    for(var word=0u;word<lane/32u;word++) {
        local_rank+=countOneBits(atomicLoad(&members[bucket*8u+word]));
    }
    local_rank+=countOneBits(atomicLoad(&members[bucket*8u+lane/32u])&((1u<<(lane%32u))-1u));
    let bucket_begin=scan[bucket]-histogram[256u*sort.groups+bucket];
    output_ids[bucket_begin+histogram[bucket*sort.groups+group.x]+local_rank]=piece;
}
