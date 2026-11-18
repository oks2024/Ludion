//! Secondary views: each [`RenderScene::views`] entry is the main scene seen through another camera,
//! drawn through the ordinary frame graph into a texture the UI pass samples in the same frame.
//!
//! A view is a derived frame, as a probe face is ([`crate::probe_capture::face_scene`]); unlike a face
//! it keeps the sprites, markers, particles, text, decals and blended meshes, and draws the ordinary
//! frame graph with its cull. It drops what belongs to the screen (the UI, the viewmodel, silhouettes,
//! the debug view) and the screen-space effects that need history or the main camera.

use super::{CacheEviction, Renderer};
use crate::graph::{AntiAliasing, FrameSize, PostChain, build_frame_graph};
use crate::scene::{RenderScene, Silhouettes};
use ludion_core::diagnostics::Diagnostic;
use ludion_render_graph::{RenderedImage, schedule};

/// The scene `scene.views[index]` renders: `scene` seen through that view's camera, without the UI,
/// other views, viewmodel, silhouettes, debug view, SSAO, SSR, depth of field, light shafts or light
/// scatter. Everything else, lights with their shadows, the sun with the main camera's cascade fit,
/// the environment, GI and the probes, stays.
///
/// # Panics
/// If `index` is not a view of `scene`.
///
/// # Example
/// ```
/// use ludion_renderer::{RenderScene, RenderViewData, view_scene};
///
/// let mut scene = RenderScene::empty((1280, 720));
/// scene.views.push(RenderViewData {
///     name: "radar".into(),
///     size: (256, 256),
///     camera: scene.camera,
/// });
/// let derived = view_scene(&scene, 0);
/// assert!(derived.views.is_empty() && derived.ui.is_empty());
/// ```
pub fn view_scene(scene: &RenderScene, index: usize) -> RenderScene {
    let mut out = scene.clone();
    out.camera = scene.views[index].camera;
    out.cull_camera = None;
    // The gun belongs to the screen, not the world. A static prefix never holds a viewmodel, so
    // dropping them leaves `statics` valid.
    out.meshes.retain(|mesh| !mesh.viewmodel);
    out.particles.retain(|particle| !particle.viewmodel);
    out.viewmodel = None;
    // `retain` above shifts the mesh indices the table names, and a view has no mask to draw.
    out.silhouettes = Silhouettes::default();
    out.debug_view = None;
    out.ui.clear();
    out.views.clear();
    for light in &mut out.lights {
        light.scatter = 0.0;
    }
    for light in &mut out.spot_lights {
        light.scatter = 0.0;
    }
    let mut post = out.post.unwrap_or_default();
    post.ssao = 0.0;
    post.ssr = 0.0;
    post.shafts = 0.0;
    post.dof = 0.0;
    out.post = Some(post);
    out
}

/// A view's frame size: its own texels, at native scale.
fn view_size(scene: &RenderScene, index: usize) -> FrameSize {
    let (w, h) = scene.views[index].size;
    FrameSize::native(w, h)
}

/// Renders every view of `scene` into a fresh `Rgba8UnormSrgb` texture each, in `views` order, and
/// returns their views for the UI pass to sample. Called after the main scene's probe resolve, so the
/// views' own resolves are cache hits (a view keeps `probes` and `probe_world`).
pub(super) fn render_views(
    r: &Renderer,
    scene: &RenderScene,
) -> Result<Vec<wgpu::TextureView>, Diagnostic> {
    let mut targets = Vec::with_capacity(scene.views.len());
    for index in 0..scene.views.len() {
        let size = view_size(scene, index).validate()?;
        let derived = view_scene(scene, index);
        let chain = PostChain {
            aa: AntiAliasing::Off,
            ..r.frame_chain(&derived)
        };
        let (graph, output) = build_frame_graph(size, chain);
        let scheduled = schedule(&graph)?;
        let recorder = r.prepare_frame_with(&derived, size, None, CacheEviction::Keep, chain)?;
        let (width, height) = size.output;
        let texture = r.gpu().device().create_texture(&wgpu::TextureDescriptor {
            label: Some("ludion secondary view"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        r.gpu()
            .execute_into(&scheduled, &recorder, &[(output, texture.as_image_copy())])?;
        targets.push(texture.create_view(&wgpu::TextureViewDescriptor::default()));
    }
    Ok(targets)
}

impl Renderer {
    /// Renders `scene.views[index]` alone, as the image its `UiRect` would show: the same derived scene
    /// and graph as the in-frame view, read back. For tests and tools.
    ///
    /// # Errors
    /// `L2010` / `L2011` if the GPU rejected the work or the device was lost; `L2012` for an
    /// unusable size.
    ///
    /// # Panics
    /// If `index` is not a view of `scene`.
    ///
    /// # Example
    /// ```no_run
    /// use ludion_renderer::{RenderScene, RenderViewData, Renderer};
    ///
    /// let renderer = Renderer::new()?;
    /// let mut scene = RenderScene::empty((1280, 720));
    /// scene.views.push(RenderViewData {
    ///     name: "radar".into(),
    ///     size: (256, 256),
    ///     camera: scene.camera,
    /// });
    /// let image = renderer.render_view(&scene, 0)?;
    /// assert_eq!((image.width, image.height), (256, 256));
    /// # Ok::<(), ludion_core::diagnostics::Diagnostic>(())
    /// ```
    pub fn render_view(
        &self,
        scene: &RenderScene,
        index: usize,
    ) -> Result<RenderedImage, Diagnostic> {
        let derived = view_scene(scene, index);
        let size = view_size(scene, index).validate()?;
        let chain = PostChain {
            aa: AntiAliasing::Off,
            ..self.frame_chain(&derived)
        };
        let (graph, output) = build_frame_graph(size, chain);
        let scheduled = schedule(&graph)?;
        let recorder = self.prepare_frame_with(&derived, size, None, CacheEviction::Keep, chain)?;
        self.gpu().execute(&scheduled, output, &recorder)
    }
}
