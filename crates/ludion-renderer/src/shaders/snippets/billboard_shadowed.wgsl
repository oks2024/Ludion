// The light rule of a flat camera-facing billboard with one shadow tap per light and a forward-scatter
// lobe: `shade_two_sided`'s loop, each term multiplied by a single tap of the sun's or a granted local
// light's map at the shade point, and by a Henyey-Greenstein lobe of the angle between the light's travel
// and the direction to the eye. Needs the `Camera`, `Lights` and `ShadowBindings` snippets ahead of it.

// The sun's shadow at `p`, one compare tap, 1 = lit. The anchored map when the sun has no cascades,
// otherwise the first cascade whose far depth exceeds the view depth (no seam band); a point past the last
// cascade, outside the box or behind the light is lit. A frame with no shadow map has a zeroed matrix and
// takes the lit early-out.
fn sun_tap(p: vec3<f32>) -> f32 {
    if (sun.cascade_count == 0u) {
        let clip = sun.light_space * vec4<f32>(p, 1.0);
        if (clip.w <= 0.0) {
            return 1.0;
        }
        let ndc = clip.xyz / clip.w;
        let uv = vec2<f32>(ndc.x * 0.5 + 0.5, ndc.y * -0.5 + 0.5);
        if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || ndc.z < 0.0 || ndc.z > 1.0) {
            return 1.0;
        }
        let texel = 1.0 / vec2<f32>(textureDimensions(shadow_tex));
        let compare = ndc.z - sun.direction.w * 2048.0 * texel.x;
        return textureSampleCompareLevel(shadow_tex, shadow_samp, uv, compare);
    }
    let depth = dot(sun.view_depth.xyz, p) + sun.view_depth.w;
    var i = 0u;
    for (; i < sun.cascade_count; i = i + 1u) {
        if (sun.cascade_params[i].y > depth) {
            break;
        }
    }
    if (i >= sun.cascade_count) {
        return 1.0;
    }
    let clip = sun.cascade_space[i] * vec4<f32>(p, 1.0);
    if (clip.w <= 0.0) {
        return 1.0;
    }
    let ndc = clip.xyz / clip.w;
    if (abs(ndc.x) > 1.0 || abs(ndc.y) > 1.0 || ndc.z < 0.0 || ndc.z > 1.0) {
        return 1.0;
    }
    let rect = sun.cascade_rect[i];
    let local = vec2<f32>(ndc.x * 0.5 + 0.5, ndc.y * -0.5 + 0.5);
    let texel = 1.0 / vec2<f32>(textureDimensions(shadow_tex));
    let tap = clamp(rect.xy + local * rect.zw, rect.xy + 0.5 * texel, rect.xy + rect.zw - 0.5 * texel);
    return textureSampleCompareLevel(shadow_tex, shadow_samp, tap, ndc.z - sun.cascade_params[i].z);
}

// The shadow of the light at list index `index`, one tap: `local_shadow`'s view pick and projection with the
// normal offset pushing toward the light (`n = l`). An unshadowed light returns an exact 1.0 first.
fn local_tap(index: u32, p: vec3<f32>, l: vec3<f32>, dist: f32) -> f32 {
    let first = light_shadow[index];
    if (first == UNSHADOWED) {
        return 1.0;
    }
    var view_index = first;
    let cube = local_views[first].params.z > 1.5;
    if (cube) {
        let a = abs(-l);
        if (a.x >= a.y && a.x >= a.z) {
            view_index = first + select(1u, 0u, -l.x > 0.0);
        } else if (a.y >= a.z) {
            view_index = first + select(3u, 2u, -l.y > 0.0);
        } else {
            view_index = first + select(5u, 4u, -l.z > 0.0);
        }
    }
    let view = local_views[view_index];
    let texel_world = dist * view.params.y;
    let q = p + l * view.params.x + l * (NORMAL_OFFSET_TEXELS * texel_world);
    let clip = view.light_space * vec4<f32>(q, 1.0);
    if (clip.w <= 0.0) {
        return 1.0;
    }
    let ndc = clip.xyz / clip.w;
    if (ndc.z < 0.0 || ndc.z > 1.0) {
        return 1.0;
    }
    if (!cube && (abs(ndc.x) > 1.0 || abs(ndc.y) > 1.0)) {
        return 1.0;
    }
    let local = clamp(vec2<f32>(ndc.x * 0.5 + 0.5, ndc.y * -0.5 + 0.5), vec2<f32>(0.0), vec2<f32>(1.0));
    let uv = view.rect.xy + local * view.rect.z;
    let margin = vec2<f32>(0.5 * view.rect.w);
    let tap = clamp(uv, view.rect.xy + margin, view.rect.xy + vec2<f32>(view.rect.z) - margin);
    let shadow = textureSampleCompareLevel(local_shadow_tex, shadow_samp, tap, ndc.z);
    return select(mix(1.0, shadow, view.params.w), shadow, view.params.w >= 1.0);
}

// The Henyey-Greenstein phase lobe times 4 pi: (1 - g^2) / (1 + g^2 - 2 g c)^1.5, 1 at g = 0. `c` is the
// cosine between the light's travel direction and the direction to the eye.
fn hg4pi(c: f32, g: f32) -> f32 {
    return (1.0 - g * g) / pow(max(1.0 + g * g - 2.0 * g * c, 1e-4), 1.5);
}

fn shade_billboard_shadowed(
    p: vec3<f32>,
    n: vec3<f32>,
    frag: vec2<f32>,
    shadowed: bool,
    backlight: f32,
    g: f32,
) -> vec3<f32> {
    var acc = lights.ambient.rgb;
    let normal = normalize(n);
    let to_camera = camera.cam_pos.xyz - p;
    let v = normalize(to_camera);
    let slice = cluster_slice(frag, dot(to_camera, to_camera));
    for (var i: u32 = 0u; i < slice.y; i = i + 1u) {
        let index = cluster_lights[slice.x + i];
        let light = light_list[index];
        let to_light = light.position.xyz - p;
        let dist = length(to_light);
        let range = max(light.position.w, 1e-3);
        let atten = clamp(1.0 - dist / range, 0.0, 1.0);
        let l = select(vec3<f32>(0.0), normalize(to_light), dist > 0.0);
        let ndl = abs(dot(normal, l));
        let tap = select(1.0, local_tap(index, p, l, dist), shadowed);
        let lobe = mix(1.0, hg4pi(dot(-l, v), g), backlight);
        acc = acc + light.color.rgb * spot_factor(light, l) * ndl * atten * atten * tap * lobe;
    }
    let tap = select(1.0, sun_tap(p), shadowed);
    let lobe = mix(1.0, hg4pi(dot(sun.direction.xyz, v), g), backlight);
    acc = acc + lights.sun_color.rgb * abs(dot(normal, lights.sun_direction.xyz)) * tap * lobe;
    return acc;
}
