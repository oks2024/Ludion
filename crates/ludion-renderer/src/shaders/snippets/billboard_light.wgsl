// The light rule of a flat camera-facing billboard: a lit sprite's fragment and a lit particle's
// centre both evaluate it. Needs the `Camera` and `Lights` snippets ahead of it.

// `frag_pos` is the fragment's framebuffer position (`@builtin(position)`.xy) — the tile half of the
// cluster lookup; the shell half is the radial camera distance. The clustered loop walks this
// fragment's own cell's slice in stored order, which is the same order (and therefore the same float
// sum) a flat loop over every light would produce — see the snippet's identity argument.
fn shade_two_sided(world_pos: vec3<f32>, n: vec3<f32>, frag_pos: vec2<f32>) -> vec3<f32> {
    var acc = lights.ambient.rgb;
    let normal = normalize(n);
    let to_camera = camera.cam_pos.xyz - world_pos;
    let slice = cluster_slice(frag_pos, dot(to_camera, to_camera));
    for (var i: u32 = 0u; i < slice.y; i = i + 1u) {
        let light = light_list[cluster_lights[slice.x + i]];
        let to_light = light.position.xyz - world_pos;
        let dist = length(to_light);
        let range = max(light.position.w, 1e-3);
        let atten = clamp(1.0 - dist / range, 0.0, 1.0);
        // Two-sided: a flat sprite lights the same from either face.
        let l = select(vec3<f32>(0.0), normalize(to_light), dist > 0.0);
        let ndl = abs(dot(normal, l));
        acc = acc + light.color.rgb * spot_factor(light, l) * ndl * atten * atten;
    }
    // The sun, two-sided like the point lights and unshadowed, with no IBL. A sunless frame adds +0.0.
    acc = acc + lights.sun_color.rgb * abs(dot(normal, lights.sun_direction.xyz));
    return acc;
}
