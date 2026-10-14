// Screen-space UI text pass: font glyph quads (bitmap or SDF atlas) generated in pixel space, alpha-blended, unlit.
//
// Vertices are generated on the CPU, one quad per glyph in pixel space with atlas UVs; the quads come
// from `ludion_font::layout` via extract, the same layout `ludion_engine::ui::measure_ui_text`
// measures (+Y down, newlines advance +Y). The vertex stage converts pixel coordinates to NDC (top-left origin, +Y down
// → NDC +Y up). The atlas group and the fragment stage (coverage, bitmap alpha or the thresholded SDF,
// modulated by the text color) come from `shaders/snippets/glyph.wgsl`, shared with `text.wgsl`.

struct Ui {
    // xy = viewport pixel extent; zw = padding to a 16-byte uniform.
    viewport: vec2<f32>,
    pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> ui: Ui;

struct VsIn {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) sdf: f32,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    let ndc = vec2<f32>(
        in.position.x / ui.viewport.x * 2.0 - 1.0,
        1.0 - in.position.y / ui.viewport.y * 2.0,
    );
    var out: VsOut;
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    out.sdf = in.sdf;
    return out;
}
