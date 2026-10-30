// The decal instance and the box maths shared by the projector's unlit decal entry points
// (`projector.wgsl`) and the lit decal shader (`decal_lit.wgsl`): the instance struct, the unit-cube
// vertex, the angle fade and the map gradient clamp. One snippet so both shaders see one layout.

// One decal. `ids` = (kind, blend, map flags, 0): bit 0 of the flags = a normal map is bound (lit
// decals only; the unlit path ignores it).
struct ProjectorDecal {
    ids: vec4<u32>,
    // Box space (the unit cube centred on the origin) to world, and back.
    box_to_world: mat4x4<f32>,
    world_to_box: mat4x4<f32>,
    // Linear RGBA tint.
    color: vec4<f32>,
    // The sub-rectangle of the map the box covers: u0, v0, u1, v1.
    uv_rect: vec4<f32>,
    // x = opacity, y = cos(angle end), z = cos(angle start), w = -1 for a mirrored box, else 1.
    look: vec4<f32>,
    // xyz = the box's world +Y, normalized: the axis the angle fade measures a normal against.
    axis: vec4<f32>,
    // x = 0 receives everywhere but the list (exclude) or 1 only the list, y = the list's offset in
    // the receiver-id array, z = its length.
    receivers: vec4<u32>,
    // x = roughness, or -1 for the receiver's own, y = metallic, z = the normal map's scale, w = 0.
    surface: vec4<f32>,
}

// A map gradient's longest length, as a share of the decal's uv-rect span. At a depth edge the
// reconstructed world position jumps across the box, and an unclamped gradient would pick the
// smallest mip of the whole map.
const GRADIENT_CLAMP: f32 = 0.25;

fn clamp_gradient(g: vec2<f32>, longest: f32) -> vec2<f32> {
    return g * min(1.0, longest / max(length(g), 1e-12));
}

// Corner `vi` of a cube in [-1, 1]^3 drawn as 36 vertices: six faces of two triangles, wound
// counter-clockwise seen from outside so the front-face cull keeps the back faces.
fn cube_corner(vi: u32) -> vec3<f32> {
    let face = vi / 6u;
    let axis = face / 2u;
    let sign = select(-1.0, 1.0, face % 2u == 0u);
    var quad = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u);
    var uv = array<vec2<f32>, 4>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let c = uv[quad[vi % 6u]];
    // A face's tangent axes are the next two cyclically, so t1 x t2 = +axis; the negative face
    // mirrors `u` to reverse the winding.
    let u = c.x * sign;
    let v = c.y;
    if (axis == 0u) {
        return vec3<f32>(sign, u, v);
    } else if (axis == 1u) {
        return vec3<f32>(v, sign, u);
    }
    return vec3<f32>(u, v, sign);
}

// The angle fade: 1 within `start` of the box's +Y, 0 beyond `end`, a smoothstep between. The CPU
// bakes the cosines (`look.z` = cos(start) >= `look.y` = cos(end)); equal edges are a step, since
// `smoothstep` with equal edges is undefined. A normal facing away from the box is below both.
fn angle_weight(d: ProjectorDecal, normal: vec3<f32>) -> f32 {
    let facing = dot(normal, d.axis.xyz);
    let ramp = smoothstep(d.look.y, d.look.z, facing);
    let step_weight = select(0.0, 1.0, facing >= d.look.y);
    return select(ramp, step_weight, d.look.z <= d.look.y);
}
