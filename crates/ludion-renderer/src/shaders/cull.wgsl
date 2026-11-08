// The GPU frustum cull for mesh draws (Plan/gpu-driven-culling.md decisions 11-16).
//
// Five passes over every (lane, instance), in one compute pass, each dispatched `(ceil(x / WG), lanes, 1)`.
// A lane is a (view, level) pair, `lane = view * levels + level` with `levels` = `CullParams.levels`
// (1 + the frame's longest LOD chain; 1 with no chain, which is the cull without levels):
//   cs_test     one thread per visibility word: tests its 32 instances, writes the whole word.
//   cs_reduce   one thread per block of 256 instances: the popcount of its 8 words.
//   cs_scan     one workgroup per view: an exclusive scan of the block counts, the total in the last slot.
//   cs_scatter  one thread per instance: a visible instance is copied to its stable compacted slot.
//   cs_args     one thread per bucket: the indirect-draw args.
//
// No atomics, on purpose: survivors keep input order, so draw order never depends on thread timing
// (`ATOMIC_FREE_UNITS` in the bindgen rejects an `atomic<...>` binding here). Core WebGPU only, except that
// a chain's level runs are drawn with a `first_instance` and so need INDIRECT_FIRST_INSTANCE.
//
// A lane's visibility bit is `admitted(view, i) && select_level(view, i) == level`. Survivors of a bucket
// stay inside its `[first, first + count)` region of the view's slice, ordered level 0, then 1, ..., input
// order within a level; each level's args slot carries its run's start as `first_instance`.
//
// Scratch layout, per lane at `lane * (word_cap + block_cap + 1)`: `word_cap` visibility words (bit
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
    levels: u32,
}

// Six inward-facing normalized planes `(n, d)`.
// `casters_only` (shadow views) rejects items that cast no shadow. `frustum == 0` admits every item
// the casters rule keeps. `lod_eye.xyz` is the LOD eye and `lod_eye.w` is `k`; `lod_ortho` is 1 for an
// orthographic projection and `lod_offset` is the shadow views' level offset.
struct CullView {
    planes: array<vec4<f32>, 6>,
    casters_only: u32,
    frustum: u32,
    lod_ortho: u32,
    lod_offset: u32,
    lod_eye: vec4<f32>,
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

// `elements` is the index count (asset) or the vertex count (primitive), and level 0's of
// `level_elements`, which holds the index counts of levels 0..3. `levels` is the chain length (1 with
// none); `screen[i]` is the screen size under which level `i + 1` takes over.
struct CullBucket {
    first: u32,
    count: u32,
    elements: u32,
    levels: u32,
    screen: vec4<f32>,
    level_elements: vec4<u32>,
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

fn scratch_base(lane: u32) -> u32 {
    return lane * (cull_params.word_cap + cull_params.block_cap + 1u);
}

fn word_count() -> u32 {
    return (cull_params.instance_count + 31u) / 32u;
}

fn block_count() -> u32 {
    return (cull_params.instance_count + 255u) / 256u;
}

struct WorldBox {
    lo: vec3<f32>,
    known: u32,
    hi: vec3<f32>,
    pad0: u32,
}

// The instance's world box, `known == 0` when it cannot be known. Mirrors `Aabb::transformed`, so the
// CPU oracle and this agree except for fused-multiply-add rounding exactly at a plane.
fn world_box(i: u32) -> WorldBox {
    let item = cull_items[i];
    if item.slot == SLOT_ALWAYS {
        return WorldBox(vec3<f32>(0.0), 0u, vec3<f32>(0.0), 0u);
    }
    if item.slot == SLOT_OVERRIDE {
        return WorldBox(item.min, 1u, item.max, 0u);
    }
    if item.slot >= arrayLength(&cull_boxes) {
        return WorldBox(vec3<f32>(0.0), 0u, vec3<f32>(0.0), 0u);
    }
    let local = cull_boxes[item.slot];
    if local.known == 0u {
        return WorldBox(vec3<f32>(0.0), 0u, vec3<f32>(0.0), 0u);
    }
    let model = cull_in[i].model;
    let c = (local.min + local.max) * 0.5;
    let e = (local.max - local.min) * 0.5;
    let center = model[0].xyz * c.x + model[1].xyz * c.y + model[2].xyz * c.z + model[3].xyz;
    let extent = abs(model[0].xyz) * e.x + abs(model[1].xyz) * e.y + abs(model[2].xyz) * e.z;
    return WorldBox(center - extent, 1u, center + extent, 0u);
}

// Mirrors `Frustum::intersects_aabb`.
fn admitted(view: u32, i: u32) -> bool {
    let item = cull_items[i];
    if cull_views[view].casters_only != 0u && item.casts == 0u {
        return false;
    }
    if cull_views[view].frustum == 0u {
        return true;
    }
    let box = world_box(i);
    if box.known == 0u {
        return true;
    }
    for (var p = 0u; p < 6u; p = p + 1u) {
        let plane = cull_views[view].planes[p];
        let n = plane.xyz;
        let far = vec3<f32>(
            select(box.lo.x, box.hi.x, n.x >= 0.0),
            select(box.lo.y, box.hi.y, n.y >= 0.0),
            select(box.lo.z, box.hi.z, n.z >= 0.0),
        );
        if dot(n, far) + plane.w < 0.0 {
            return false;
        }
    }
    return true;
}

// The level `view` draws instance `i` at; mirrors `cull::lod_level` over `cull::lod_size`.
fn select_level(view: u32, i: u32) -> u32 {
    let bucket = cull_buckets[cull_items[i].bucket];
    if bucket.levels <= 1u {
        return 0u;
    }
    let box = world_box(i);
    if box.known == 0u {
        return 0u;
    }
    let v = cull_views[view];
    let center = (box.lo + box.hi) * 0.5;
    let radius = length((box.hi - box.lo) * 0.5);
    var size = radius * v.lod_eye.w;
    if v.lod_ortho == 0u {
        size = size / max(length(center - v.lod_eye.xyz), radius);
    }
    var level = v.lod_offset;
    for (var k = 0u; k + 1u < bucket.levels; k = k + 1u) {
        if size < bucket.screen[k] {
            level = level + 1u;
        }
    }
    return min(level, bucket.levels - 1u);
}

// The visible count in `[0, i)` of lane `lane`, for `i` in `[0, instance_count]`.
fn prefix(lane: u32, i: u32) -> u32 {
    let base = scratch_base(lane);
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
    let view = gid.y / cull_params.levels;
    let level = gid.y % cull_params.levels;
    var bits = 0u;
    for (var k = 0u; k < 32u; k = k + 1u) {
        let i = word * 32u + k;
        if i < cull_params.instance_count && admitted(view, i) && select_level(view, i) == level {
            bits = bits | (1u << k);
        }
    }
    cull_scratch[scratch_base(gid.y) + word] = bits;
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

// The visible count of bucket `b` at `level` in `view`.
fn level_count(view: u32, level: u32, b: u32) -> u32 {
    let lane = view * cull_params.levels + level;
    let bucket = cull_buckets[b];
    return prefix(lane, bucket.first + bucket.count) - prefix(lane, bucket.first);
}

// The visible count of bucket `b` at levels finer than `level` in `view`: where `level`'s run starts.
fn finer_count(view: u32, level: u32, b: u32) -> u32 {
    var total = 0u;
    for (var l = 0u; l < level; l = l + 1u) {
        total = total + level_count(view, l, b);
    }
    return total;
}

@compute @workgroup_size(64)
fn cs_scatter(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if i >= cull_params.instance_count {
        return;
    }
    let lane = gid.y;
    let view = lane / cull_params.levels;
    let level = lane % cull_params.levels;
    let visible = (cull_scratch[scratch_base(lane) + i / 32u] >> (i % 32u)) & 1u;
    if visible == 0u {
        return;
    }
    let b = cull_items[i].bucket;
    let first = cull_buckets[b].first;
    let slot = finer_count(view, level, b) + prefix(lane, i) - prefix(lane, first);
    cull_out[view * cull_params.instance_cap + first + slot] = cull_in[i];
}

@compute @workgroup_size(64)
fn cs_args(@builtin(global_invocation_id) gid: vec3<u32>) {
    let b = gid.x;
    if b >= cull_params.bucket_count {
        return;
    }
    let lane = gid.y;
    let view = lane / cull_params.levels;
    let level = lane % cull_params.levels;
    let bucket = cull_buckets[b];
    let at = (lane * cull_params.bucket_cap + b) * 5u;
    cull_args[at] = bucket.level_elements[level];
    cull_args[at + 1u] = level_count(view, level, b);
    cull_args[at + 2u] = 0u;
    cull_args[at + 3u] = 0u;
    cull_args[at + 4u] = finer_count(view, level, b);
}
