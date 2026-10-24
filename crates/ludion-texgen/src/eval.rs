//! Evaluating a loaded recipe into the map images.

use crate::adjust;
use crate::fill;
use crate::filter;
use crate::hash::mix;
use crate::image::Image;
use crate::noise::Sampler;
use crate::ops::Op;
use crate::picture;
use crate::recipe::OutputKey;
use crate::resolve::Loaded;
use crate::scatter;

/// Receives each top-level step's name and image as it completes.
pub type Observer<'a> = &'a mut dyn FnMut(&str, &Image);

/// The evaluated maps, defaults filled in. Color maps are linear and three-channel; the rest are gray.
#[derive(Clone, Debug, PartialEq)]
pub struct Maps {
    /// Albedo, linear.
    pub base_color: Image,
    /// Roughness, gray.
    pub roughness: Image,
    /// Metallic, gray; zero when the recipe has none.
    pub metallic: Image,
    /// Tangent-space normal in `[0, 1]`.
    pub normal: Image,
    /// Ambient occlusion, gray; one when the recipe has none.
    pub occlusion: Image,
    /// Emissive, linear; present only when the recipe authors it.
    pub emissive: Option<Image>,
    /// Height, gray, linear.
    pub height: Image,
}

/// Evaluates `recipe` at `size`×`size` texels.
///
/// Only steps the outputs reach are evaluated, and an image is dropped after its last consumer.
/// `observe` is called with each top-level step's name and image as it completes.
///
/// # Panics
/// If `size` is zero. [`crate::build`] validates the 8..=8192 range.
pub fn evaluate(recipe: &Loaded, size: u32, observe: &mut dyn FnMut(&str, &Image)) -> Maps {
    assert!(size > 0, "evaluate: size must be at least 1");
    let mut targets: Vec<usize> = recipe.instance.outputs.iter().map(|(_, i)| *i).collect();
    targets.sort_unstable();
    targets.dedup();
    let images = eval_node(recipe, size, &targets, &[], Some(observe));
    let get = |key: OutputKey| {
        let step = recipe.instance.outputs.iter().find(|(k, _)| *k == key)?.1;
        let at = targets.iter().position(|t| *t == step)?;
        Some(&images[at])
    };
    let height = get(OutputKey::Height).expect("height is required").clone();
    Maps {
        base_color: color(get(OutputKey::BaseColor).expect("base_color is required")),
        roughness: get(OutputKey::Roughness)
            .expect("roughness is required")
            .clone(),
        metallic: get(OutputKey::Metallic)
            .cloned()
            .unwrap_or_else(|| Image::filled(size, 1, 0.0)),
        normal: get(OutputKey::Normal)
            .cloned()
            .unwrap_or_else(|| filter::normal_from_height(&height, 0.05)),
        occlusion: get(OutputKey::Occlusion)
            .cloned()
            .unwrap_or_else(|| Image::filled(size, 1, 1.0)),
        emissive: get(OutputKey::Emissive).map(color),
        height,
    }
}

/// Broadcasts a gray image to three channels.
fn color(image: &Image) -> Image {
    if image.channels == 3 {
        return image.clone();
    }
    Image::generate(image.size, 3, |x, y, t| t.fill(image.channel(x, y, 0)))
}

fn eval_node(
    node: &Loaded,
    size: u32,
    targets: &[usize],
    bound: &[(usize, &Image)],
    mut observe: Option<Observer<'_>>,
) -> Vec<Image> {
    let steps = &node.instance.steps;
    let mut needed = vec![false; steps.len()];
    let mut pending: Vec<usize> = targets.to_vec();
    while let Some(i) = pending.pop() {
        if !std::mem::replace(&mut needed[i], true) {
            pending.extend(&steps[i].inputs);
        }
    }
    let mut uses = vec![0usize; steps.len()];
    for (i, step) in steps.iter().enumerate().filter(|(i, _)| needed[*i]) {
        let _ = i;
        for d in &step.inputs {
            uses[*d] += 1;
        }
    }
    let mut slots: Vec<Option<Image>> = vec![None; steps.len()];
    for &i in node.instance.order.iter().filter(|i| needed[**i]) {
        let step = &steps[i];
        let image = {
            let inputs: Vec<&Image> = step
                .inputs
                .iter()
                .map(|d| slots[*d].as_ref().expect("inputs outlive their consumers"))
                .collect();
            compute(node, i, size, &inputs, bound)
        };
        if let Some(observe) = observe.as_mut() {
            observe(&step.name, &image);
        }
        slots[i] = Some(image);
        for d in &step.inputs {
            uses[*d] -= 1;
            if uses[*d] == 0 && !targets.contains(d) {
                slots[*d] = None;
            }
        }
    }
    targets
        .iter()
        .map(|t| slots[*t].clone().expect("targets are kept"))
        .collect()
}

fn compute(
    node: &Loaded,
    index: usize,
    size: u32,
    inputs: &[&Image],
    bound: &[(usize, &Image)],
) -> Image {
    match &node.instance.steps[index].op {
        Op::Constant(value) => adjust::constant(size, value),
        Op::Gradient {
            axis,
            shape,
            repeat,
        } => adjust::gradient(size, *axis, *shape, *repeat),
        Op::Noise(spec) => Sampler::new(spec, mix(node.seed, u64::from(spec.seed))).render(size),
        Op::Blend { mode, opacity, .. } => adjust::blend(
            inputs[0],
            inputs[1],
            inputs.get(2).copied(),
            *mode,
            *opacity,
        ),
        Op::Levels {
            in_low,
            in_high,
            gamma,
            out_low,
            out_high,
            ..
        } => adjust::levels(inputs[0], *in_low, *in_high, *gamma, *out_low, *out_high),
        Op::Curve { points, .. } => adjust::curve(inputs[0], points),
        Op::Clamp { min, max, .. } => adjust::clamp(inputs[0], *min, *max),
        Op::Remap {
            from_low,
            from_high,
            to_low,
            to_high,
            ..
        } => adjust::remap(inputs[0], *from_low, *from_high, *to_low, *to_high),
        Op::Colorize { stops, .. } => adjust::colorize(inputs[0], stops),
        Op::Grayscale { .. } => adjust::grayscale(inputs[0]),
        Op::Posterize { steps, .. } => adjust::posterize(inputs[0], *steps),
        Op::HsvShift {
            hue,
            saturation,
            value,
            ..
        } => adjust::hsv_shift(inputs[0], *hue, *saturation, *value),
        Op::Warp { amount, angle, .. } => filter::warp(inputs[0], inputs[1], *amount, *angle),
        Op::Blur {
            radius,
            angle: None,
            ..
        } => filter::blur(inputs[0], *radius),
        Op::Blur {
            radius,
            angle: Some(angle),
            spread,
            ..
        } => filter::blur_along(inputs[0], *radius, *angle, *spread),
        Op::NormalFromHeight { depth, .. } => filter::normal_from_height(inputs[0], *depth),
        Op::TileSampler(spec) => scatter::render(
            spec,
            inputs.first().copied(),
            size,
            mix(node.seed, u64::from(spec.seed)),
        ),
        Op::FloodFill {
            threshold,
            output,
            angle,
            angle_jitter,
            seed,
            ..
        } => fill::flood_fill(
            inputs[0],
            *threshold,
            *output,
            *angle,
            *angle_jitter,
            mix(node.seed, u64::from(*seed)),
        ),
        Op::Distance { threshold, max, .. } => fill::distance(inputs[0], *threshold, *max),
        Op::Band {
            level,
            width,
            side,
            softness,
            ..
        } => fill::band(inputs[0], *level, *width, *side, *softness),
        Op::AmbientOcclusion {
            radius,
            depth,
            strength,
            ..
        } => filter::ambient_occlusion(inputs[0], *radius, *depth, *strength),
        Op::Curvature {
            radius, strength, ..
        } => filter::curvature(inputs[0], *radius, *strength),
        Op::Image { .. } => picture::resample(&node.pictures[&index], size),
        Op::Input { default, .. } => match bound.iter().find(|(i, _)| *i == index) {
            Some((_, image)) => (*image).clone(),
            None => adjust::constant(size, default),
        },
        Op::Recipe { .. } => {
            let child = &node.children[&index];
            let bound: Vec<(usize, &Image)> = child
                .inputs
                .iter()
                .copied()
                .zip(inputs.iter().copied())
                .collect();
            eval_node(&child.node, size, &[child.output], &bound, None).remove(0)
        }
    }
}
