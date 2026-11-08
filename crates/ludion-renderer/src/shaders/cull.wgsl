// The GPU frustum cull for mesh draws (Plan/gpu-driven-culling.md decisions 11-16).
//
// Five passes over every (view, instance), in one compute pass, each dispatched `(ceil(x / WG), views, 1)`:
//   cs_test     one thread per visibility word: tests its 32 instances, writes the whole word.
//   cs_reduce   one thread per block of 256 instances: the popcount of its 8 words.
//   cs_scan     one workgroup per view: an exclusive scan of the block counts, the total in the last slot.
//   cs_scatter  one thread per instance: a visible instance is copied to its stable compacted slot.
//   cs_args     one thread per bucket: the indirect-draw args.
//
// No atomics, on purpose: survivors keep input order, so draw order never depends on thread timing
// (`ATOMIC_FREE_UNITS` in the bindgen rejects an `atomic<...>` binding here). Core WebGPU only.
//
// Scratch layout, per view v at `v * (word_cap + block_cap + 1)`: `word_cap` visibility words (bit
// `i % 32` of word `i / 32`), then `block_cap + 1` block slots.

const SLOT_OVERRIDE: u32 = 0xFFFFFFFEu;
const SLOT_ALWAYS: u32 = 0xFFFFFFFFu;

struct CullParams {
    instance_count: u32,
    bucket_count: u32,
    view_count: u32,
    instance_cap: u32,
    bucket_cap: u32,
    word_cap: u32,
    block_cap: u32,
    pad0: u32,
}

// Six inward-facing normalized planes `(n, d)`.
// `casters_only` (shadow views) rejects items that cast no shadow.
struct CullView {
    planes: array<vec4<f32>, 6>,
    casters_only: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
}

// A `MeshInstanceRaw` (224 bytes), copied whole; only `model` is read.
struct CullInstance {
    model: mat4x4<f32>,
    rest: array<vec4<f32>, 10>,
}

// `slot` indexes `cull_boxes`, or is SLOT_OVERRIDE (min/max is the world box) or SLOT_ALWAYS.
// `casts` is 0 for a mesh with `cast_shadows: false`.
struct CullItem {
    min: vec3<f32>,
    slot: u32,
    max: vec3<f32>,
    bucket: u32,
    casts: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
}

// `known == 0` marks an unknown box: always visible.
struct CullBox {
    min: vec3<f32>,
    known: u32,
    max: vec3<f32>,
    pad0: u32,
}

// `elements` is the index count (asset) or the vertex count (primitive).
struct CullBucket {
    first: u32,
    count: u32,
    elements: u32,
    pad0: u32,
}

@group(0) @binding(0) var<uniform> cull_params: CullParams;
@group(0) @binding(1) var<storage, read> cull_views: array<CullView>;
@group(0) @binding(2) var<storage, read> cull_in: array<CullInstance>;
@group(0) @binding(3) var<storage, read> cull_items: array<CullItem>;
@group(0) @binding(4) var<storage, read> cull_boxes: array<CullBox>;
@group(0) @binding(5) var<storage, read> cull_buckets: array<CullBucket>;

@group(1) @binding(0) var<storage, read_write> cull_scratch: array<u32>;
@group(1) @binding(1) var<storage, read_write> cull_out: array<CullInstance>;
@group(1) @binding(2) var<storage, read_write> cull_args: array<u32>;

var<workgroup> scan_sums: array<u32, 256>;

fn scratch_base(view: u32) -> u32 {
    return view * (cull_params.word_cap + cull_params.block_cap + 1u);
}

fn word_count() -> u32 {
    return (cull_params.instance_count + 31u) / 32u;
}

fn block_count() -> u32 {
    return (cull_params.instance_count + 255u) / 256u;
}

// Mirrors `Aabb::transformed` then `Frustum::intersects_aabb`, so the CPU oracle and this agree
// except for fused-multiply-add rounding exactly at a plane.
fn admitted(view: u32, i: u32) -> bool {
    let item = cull_items[i];
    if cull_views[view].casters_only != 0u && item.casts == 0u {
        return false;
    }
    if item.slot == SLOT_ALWAYS {
        return true;
    }
    var lo = item.min;
    var hi = item.max;
    if item.slot != SLOT_OVERRIDE {
        if item.slot >= arrayLength(&cull_boxes) {
            return true;
        }
        let local = cull_boxes[item.slot];
        if local.known == 0u {
            return true;
        }
        let model = cull_in[i].model;
        let c = (local.min + local.max) * 0.5;
        let e = (local.max - local.min) * 0.5;
        let center = model[0].xyz * c.x + model[1].xyz * c.y + model[2].xyz * c.z + model[3].xyz;
        let extent = abs(model[0].xyz) * e.x + abs(model[1].xyz) * e.y + abs(model[2].xyz) * e.z;
        lo = center - extent;
        hi = center + extent;
    }
    for (var p = 0u; p < 6u; p = p + 1u) {
        let plane = cull_views[view].planes[p];
        let n = plane.xyz;
        let far = vec3<f32>(
            select(lo.x, hi.x, n.x >= 0.0),
            select(lo.y, hi.y, n.y >= 0.0),
            select(lo.z, hi.z, n.z >= 0.0),
        );
        if dot(n, far) + plane.w < 0.0 {
            return false;
        }
    }
    return true;
}

// The visible count in `[0, i)` for `i` in `[0, instance_count]`.
fn prefix(view: u32, i: u32) -> u32 {
    let base = scratch_base(view);
    let blocks = base + cull_params.word_cap;
    var count = cull_scratch[blocks + i / 256u];
    for (var w = (i / 256u) * 8u; w < i / 32u; w = w + 1u) {
        count = count + countOneBits(cull_scratch[base + w]);
    }
    if i % 32u != 0u {
        count = count + countOneBits(cull_scratch[base + i / 32u] & ((1u << (i % 32u)) - 1u));
    }
    return count;
}

@compute @workgroup_size(64)
fn cs_test(@builtin(global_invocation_id) gid: vec3<u32>) {
    let word = gid.x;
    if word >= word_count() {
        return;
    }
    let view = gid.y;
    var bits = 0u;
    for (var k = 0u; k < 32u; k = k + 1u) {
        let i = word * 32u + k;
        if i < cull_params.instance_count && admitted(view, i) {
            bits = bits | (1u << k);
        }
    }
    cull_scratch[scratch_base(view) + word] = bits;
}

@compute @workgroup_size(64)
fn cs_reduce(@builtin(global_invocation_id) gid: vec3<u32>) {
    let block = gid.x;
    if block >= block_count() {
        return;
    }
    let base = scratch_base(gid.y);
    var count = 0u;
    for (var k = 0u; k < 8u; k = k + 1u) {
        let w = block * 8u + k;
        if w < word_count() {
            count = count + countOneBits(cull_scratch[base + w]);
        }
    }
    cull_scratch[base + cull_params.word_cap + block] = count;
}

@compute @workgroup_size(256)
fn cs_scan(@builtin(local_invocation_id) lid: vec3<u32>, @builtin(workgroup_id) wid: vec3<u32>) {
    let t = lid.x;
    let blocks = scratch_base(wid.y) + cull_params.word_cap;
    let k = block_count();
    let chunk = (k + 255u) / 256u;
    let start = min(t * chunk, k);
    let end = min(start + chunk, k);
    var mine = 0u;
    for (var b = start; b < end; b = b + 1u) {
        mine = mine + cull_scratch[blocks + b];
    }
    scan_sums[t] = mine;
    workgroupBarrier();
    // Hillis-Steele inclusive scan of the 256 chunk sums: eight rounds, all in uniform control flow.
    for (var offset = 1u; offset < 256u; offset = offset * 2u) {
        var s = scan_sums[t];
        if t >= offset {
            s = s + scan_sums[t - offset];
        }
        workgroupBarrier();
        scan_sums[t] = s;
        workgroupBarrier();
    }
    var running = scan_sums[t] - mine;
    for (var b = start; b < end; b = b + 1u) {
        let count = cull_scratch[blocks + b];
        cull_scratch[blocks + b] = running;
        running = running + count;
    }
    if t == 255u {
        cull_scratch[blocks + k] = scan_sums[255];
    }
}

@compute @workgroup_size(64)
fn cs_scatter(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if i >= cull_params.instance_count {
        return;
    }
    let view = gid.y;
    let visible = (cull_scratch[scratch_base(view) + i / 32u] >> (i % 32u)) & 1u;
    if visible == 0u {
        return;
    }
    let first = cull_buckets[cull_items[i].bucket].first;
    let slot = prefix(view, i) - prefix(view, first);
    cull_out[view * cull_params.instance_cap + first + slot] = cull_in[i];
}

@compute @workgroup_size(64)
fn cs_args(@builtin(global_invocation_id) gid: vec3<u32>) {
    let b = gid.x;
    if b >= cull_params.bucket_count {
        return;
    }
    let view = gid.y;
    let bucket = cull_buckets[b];
    let at = (view * cull_params.bucket_cap + b) * 5u;
    cull_args[at] = bucket.elements;
    cull_args[at + 1u] = prefix(view, bucket.first + bucket.count) - prefix(view, bucket.first);
    cull_args[at + 2u] = 0u;
    cull_args[at + 3u] = 0u;
    cull_args[at + 4u] = 0u;
}
