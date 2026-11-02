// The vector to normalize for a view ray. An orthographic camera has one view direction, the lane's forward.
// The selection wraps the input of `normalize`, so a perspective frame normalizes the same value it always did.
fn view_to_eye(to_eye: vec3<f32>, view_forward: vec4<f32>) -> vec3<f32> {
    return select(to_eye, -view_forward.xyz, view_forward.w > 0.5);
}
fn view_from_eye(from_eye: vec3<f32>, view_forward: vec4<f32>) -> vec3<f32> {
    return select(from_eye, view_forward.xyz, view_forward.w > 0.5);
}
