#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! `ludion-render-graph` — the engine's one rendering abstraction: a wgpu frame graph.
//!
//! A frame is described *declaratively* — virtual resources (transient textures + the final target)
//! and passes that declare what they read and write — and the graph derives everything else: a
//! deterministic execution order from the dependency edges, and transient aliasing (reusing one
//! physical texture for virtual resources whose lifetimes do not overlap). Only then does the
//! [`execute`] layer touch wgpu, resolving the plan onto a device that always draws into an
//! offscreen texture (`engine-architecture.md` §3). The offscreen target stays the single source of
//! truth: an optional [`PresentTarget`] blits it into a window after the fact, and is never a render
//! target of its own.
//!
//! **Backend boundary (the ash-migration seam).** The declarative half ([`graph`], [`schedule`](mod@schedule),
//! [`resource`], [`dump`]) names no wgpu type — resource formats/sizes/usages are this crate's own
//! small enums, and passes reference [`ResourceId`] handles. wgpu appears only in [`execute`],
//! [`readback`], [`present`], and [`timings`]. Per `engine-architecture.md` §2 this is a deliberate seam, not
//! backend abstraction: there are no generic resource types and no backend traits; if wgpu is ever
//! replaced with ash, the execution layer is swapped wholesale and the graph survives untouched.
//!
//! # What this crate does *not* do (yet)
//!
//! Scene content is not rendered here — that is build-order step 6 (`ludion-renderer`). The passes
//! this crate ships draw a fixed, procedural test image ([`build_graph`]): a gradient followed by a
//! chain of vignette passes. Their purpose is to prove the graph end to end — a real dependency
//! edge, real transient lifetimes to alias, real WGSL/pipeline plumbing, and deterministic PNG
//! readback.
//!
//! # Example — inspect a graph with no GPU
//!
//! ```
//! // Build the fixed proving graph, schedule it, and dump it — all pure CPU.
//! let (graph, _output) = ludion_render_graph::build_graph(1920, 1080);
//! let schedule = ludion_render_graph::schedule(&graph).expect("the graph is acyclic");
//!
//! // Four passes, in dependency order:
//! assert_eq!(
//!     schedule.order_names(),
//!     ["gradient", "vignette", "bleed", "present"],
//! );
//! // Three transients share two physical textures; the output gets its own — four resources,
//! // three physical textures. Aliasing is observable without ever creating a device.
//! assert_eq!(schedule.physical_texture_count(), 3);
//! assert!(schedule.dump_graph_json().contains("\"aliasing\""));
//! ```
//!
//! # Example — render to PNG (requires a GPU adapter)
//!
//! ```no_run
//! let (graph, output) = ludion_render_graph::build_graph(1920, 1080);
//! let schedule = ludion_render_graph::schedule(&graph).unwrap();
//! let gpu = ludion_render_graph::Gpu::request().unwrap();
//! let image = gpu.render_to_image(&schedule, output).unwrap();
//! image.write_png("frame.png").unwrap();
//! ```

pub mod blit;
pub mod dump;
pub mod execute;
pub mod graph;
pub mod pass_timing;
mod png_io;
pub mod present;
pub mod readback;
pub mod resource;
pub mod schedule;
pub mod timings;

pub use blit::{Blit, SurfaceSetup};
pub use execute::{
    ColorTarget, Gpu, MAX_TEXTURE_DIMENSION_2D, PassRecorder, RecordedPass, RenderedImage,
    compressed_format_supported, engine_limits,
};
pub use graph::{PassDecl, PassKind, RenderGraph, ResourceDecl, ResourceKind};
pub use pass_timing::PassTimingRing;
pub use present::{PresentOutcome, PresentSize, PresentTarget};
pub use readback::{FrameSinks, StreamReadback};
pub use resource::{Extent2d, ResourceId, TextureDesc, TextureFormat, TextureUsage};
pub use schedule::{Aliasing, PhysicalTexture, Schedule, schedule};
pub use timings::PassTiming;

/// Builds the fixed proving graph at `width` × `height` and returns it with the handle of its final
/// output target.
///
/// The graph is: a **gradient** source pass → a chain of **vignette** passes (`vignette`, `bleed`)
/// → a final **present** pass into the output target. This yields a real dependency chain, three
/// transient textures with staggered lifetimes (two of which alias onto one physical texture), and
/// a final target that is read back. The content is procedural and deterministic — no scene data is
/// consulted (that arrives in build-order step 6).
///
/// # Example
/// ```
/// let (graph, output) = ludion_render_graph::build_graph(64, 64);
/// assert_eq!(graph.passes().len(), 4);
/// assert_eq!(graph.resource(output).kind, ludion_render_graph::ResourceKind::Output);
/// ```
pub fn build_graph(width: u32, height: u32) -> (RenderGraph, ResourceId) {
    let mut graph = RenderGraph::new();

    let scene = graph.transient("scene", TextureDesc::color(width, height));
    let veiled = graph.transient("veiled", TextureDesc::color(width, height));
    let bloom = graph.transient("bloom", TextureDesc::color(width, height));
    let output = graph.output("final", TextureDesc::present_target(width, height));

    graph.add_pass("gradient", PassKind::Gradient, scene);
    graph.add_pass("vignette", PassKind::Vignette { source: scene }, veiled);
    graph.add_pass("bleed", PassKind::Vignette { source: veiled }, bloom);
    graph.add_pass("present", PassKind::Vignette { source: bloom }, output);

    (graph, output)
}
