//! The op set as typed values, and the field reader that turns one step's YAML mapping into one.

use std::collections::BTreeMap;

use ludion_core::codes::code;
use ludion_core::diagnostics::{Diagnostic, pointer_token, value_phrase};
use ludion_core::math::powf;
use ludion_core::suggest::nearest;
use serde_json::{Map, Value};

/// A recipe parameter's value: a number or a color (linear RGB).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParamValue {
    /// A plain number.
    Number(f64),
    /// A `"#rrggbb"` sRGB color decoded to linear.
    Color([f32; 3]),
}

/// A recipe's parameters by name, defaults overlaid with any bindings.
pub type Scope = BTreeMap<String, ParamValue>;

/// Every op name, in documentation order.
pub const OP_NAMES: &[&str] = &[
    "constant",
    "gradient",
    "noise",
    "blend",
    "levels",
    "curve",
    "clamp",
    "remap",
    "colorize",
    "grayscale",
    "warp",
    "blur",
    "normal_from_height",
    "tile_sampler",
    "flood_fill",
    "distance",
    "ambient_occlusion",
    "curvature",
    "image",
    "input",
    "recipe",
];

/// A `constant` op's value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Const {
    /// A one-channel value.
    Gray(f32),
    /// A linear RGB color.
    Color([f32; 3]),
}

/// Which texture axis a `gradient` runs along.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// Across the tile, left to right.
    U,
    /// Down the tile, top to bottom.
    V,
}

/// A `gradient`'s profile along its axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GradientShape {
    /// 0 → 1 each period, discontinuous at the period edge.
    Ramp,
    /// 0 → 1 → 0 each period.
    Triangle,
    /// A raised cosine, 0 at the period edge.
    Sine,
}

/// Which noise a `noise` op generates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoiseKind {
    /// Quintic-interpolated lattice values.
    Value,
    /// Perlin gradient noise over eight fixed gradients.
    Gradient,
    /// Worley noise over the 3×3 wrapped neighbourhood.
    Cellular,
}

/// What a cellular noise returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellReturn {
    /// Distance to the nearest feature point.
    F1,
    /// Distance to the second nearest.
    F2,
    /// `f2 - f1`, bright cell borders.
    F2MinusF1,
}

/// A `noise` op's parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct NoiseSpec {
    /// Which noise.
    pub kind: NoiseKind,
    /// Lattice cells per tile of the first octave.
    pub scale: u32,
    /// Octaves summed (fbm when above 1).
    pub octaves: u32,
    /// Lattice multiplier between octaves.
    pub lacunarity: u32,
    /// Amplitude multiplier between octaves.
    pub gain: f32,
    /// Seed offset mixed into the recipe seed.
    pub seed: u32,
    /// Cellular return value.
    pub ret: CellReturn,
    /// Cellular feature-point jitter in `[0, 1]`.
    pub jitter: f32,
}

/// How a `blend` combines `a` and `b`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlendMode {
    /// `b`.
    Mix,
    /// `a + b`.
    Add,
    /// `a - b`.
    Subtract,
    /// `a * b`.
    Multiply,
    /// `1 - (1 - a)(1 - b)`.
    Screen,
    /// Multiply below 0.5 of `a`, screen above.
    Overlay,
    /// The smaller.
    Min,
    /// The larger.
    Max,
    /// `|a - b|`.
    Difference,
}

/// The shape a `tile_sampler` stamps when it has no `pattern`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StampShape {
    /// A filled square.
    Box,
    /// A filled disc.
    Disc,
    /// A smooth bump, `(1 - r²)²`.
    Bell,
    /// A square pyramid, falling linearly to the edge.
    Pyramid,
}

/// How overlapping `tile_sampler` instances combine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Combine {
    /// The largest contribution.
    Max,
    /// The sum of contributions.
    Add,
}

/// A `tile_sampler` op's parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct TileSpec {
    /// Cells across the tile.
    pub columns: u32,
    /// Cells down the tile.
    pub rows: u32,
    /// Stamp shape when there is no pattern.
    pub shape: StampShape,
    /// Optional gray step used as the stamp.
    pub pattern: Option<String>,
    /// Instance size relative to its cell.
    pub scale: f32,
    /// Random shrink of an instance, in `[0, 1]`.
    pub scale_jitter: f32,
    /// Random shift of an instance within its cell, in `[0, 1]`.
    pub position_jitter: f32,
    /// Random turn of an instance, in `[0, 1]` of a full turn.
    pub rotation_jitter: f32,
    /// Random darkening of an instance, in `[0, 1]`.
    pub value_jitter: f32,
    /// Base rotation in degrees.
    pub rotation: f32,
    /// Shift of odd rows, in cell widths.
    pub offset: f32,
    /// Random extra shift of each row, in cell widths.
    pub offset_jitter: f32,
    /// Tile units shaved off every instance edge-to-edge.
    pub gap: f32,
    /// Random scale of the pattern lookup per instance, in `[1 - j, 1 + j]`.
    pub pattern_scale_jitter: f32,
    /// Random offset of the pattern lookup per instance, in `[0, j)` on each axis.
    pub pattern_offset_jitter: f32,
    /// Probability an instance is kept.
    pub density: f32,
    /// How overlaps combine.
    pub combine: Combine,
    /// Seed offset mixed into the recipe seed.
    pub seed: u32,
}

/// What a `flood_fill` writes into each component.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillOutput {
    /// One random gray per component.
    Random,
    /// A linear ramp across the component's bounding box.
    Gradient,
    /// A color: position in the bounding box, then a random blue.
    Uv,
}

/// One op with every field validated and every `$param` substituted.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// A flat value.
    Constant(Const),
    /// A periodic ramp along an axis.
    Gradient {
        /// Axis it runs along.
        axis: Axis,
        /// Profile.
        shape: GradientShape,
        /// Periods per tile.
        repeat: u32,
    },
    /// Lattice noise.
    Noise(NoiseSpec),
    /// Two images combined, optionally masked.
    Blend {
        /// Base image.
        a: String,
        /// Blended-in image.
        b: String,
        /// Optional gray mask.
        mask: Option<String>,
        /// Combine mode.
        mode: BlendMode,
        /// Overall strength.
        opacity: f32,
    },
    /// Input range, gamma and output range.
    Levels {
        /// Input step.
        input: String,
        /// Input black point.
        in_low: f32,
        /// Input white point.
        in_high: f32,
        /// Gamma.
        gamma: f32,
        /// Output black point.
        out_low: f32,
        /// Output white point.
        out_high: f32,
    },
    /// A piecewise-linear transfer curve.
    Curve {
        /// Input step.
        input: String,
        /// Control points, x strictly increasing.
        points: Vec<(f32, f32)>,
    },
    /// Clamps to a range.
    Clamp {
        /// Input step.
        input: String,
        /// Lower bound.
        min: f32,
        /// Upper bound.
        max: f32,
    },
    /// Linear remap, unclamped.
    Remap {
        /// Input step.
        input: String,
        /// Source low.
        from_low: f32,
        /// Source high.
        from_high: f32,
        /// Target low.
        to_low: f32,
        /// Target high.
        to_high: f32,
    },
    /// Gray to color through a gradient.
    Colorize {
        /// Gray input step.
        input: String,
        /// Stops, `at` non-decreasing, colors linear.
        stops: Vec<(f32, [f32; 3])>,
    },
    /// Color to gray by Rec.709 luminance.
    Grayscale {
        /// Color input step.
        input: String,
    },
    /// Displaces the input by another image.
    Warp {
        /// Input step.
        input: String,
        /// Displacement source.
        by: String,
        /// Displacement in tile units.
        amount: f32,
        /// Direction in degrees for a gray `by`.
        angle: f32,
    },
    /// Wrap-around gaussian-like blur.
    Blur {
        /// Input step.
        input: String,
        /// Radius in tile units.
        radius: f32,
    },
    /// A tangent-space normal map from a height.
    NormalFromHeight {
        /// Gray height input step.
        input: String,
        /// World height of value 1 in tile units.
        depth: f32,
    },
    /// Instances of a stamp scattered on a jittered grid.
    TileSampler(TileSpec),
    /// A value per connected region of a thresholded input.
    FloodFill {
        /// Gray input step.
        input: String,
        /// Texels at or above it are inside.
        threshold: f32,
        /// What each region is filled with.
        output: FillOutput,
        /// Gradient direction in degrees.
        angle: f32,
        /// Random turn of the gradient direction, in `[0, 1]` of a full turn.
        angle_jitter: f32,
        /// Seed offset mixed into the recipe seed.
        seed: u32,
    },
    /// Distance to the nearest inside texel.
    Distance {
        /// Gray input step.
        input: String,
        /// Texels at or above it are inside.
        threshold: f32,
        /// Distance, in tile units, that maps to 1.
        max: f32,
    },
    /// Horizon-based occlusion of a height.
    AmbientOcclusion {
        /// Gray height input step.
        input: String,
        /// Search radius in tile units.
        radius: f32,
        /// World height of value 1 in tile units.
        depth: f32,
        /// Blend toward full occlusion, in `[0, 1]`.
        strength: f32,
    },
    /// Convexity of a height: bright on crests, dark in cavities.
    Curvature {
        /// Gray height input step.
        input: String,
        /// Blur radius in tile units.
        radius: f32,
        /// Gain of the height-minus-blur difference.
        strength: f32,
    },
    /// A PNG file, resampled to the build size.
    Image {
        /// Path relative to the referencing recipe.
        path: String,
        /// Whether a color file is sRGB-encoded.
        srgb: bool,
    },
    /// An image the recipe receives from the step that binds it, or `default` when unbound.
    Input {
        /// Channels of the image: 1 gray, 3 color.
        channels: usize,
        /// The value of an unbound input.
        default: Const,
    },
    /// A step of another recipe, with its parameters and inputs bound.
    Recipe {
        /// Path relative to the referencing file, or `std:<name>`.
        path: String,
        /// Step of the sub-recipe to use.
        output: String,
        /// Bound parameters, in document order.
        params: Vec<(String, ParamValue)>,
        /// `(input step of the sub-recipe, step of this recipe)`, in document order.
        inputs: Vec<(String, String)>,
        /// Seed offset for the sub-recipe.
        seed: u32,
    },
}

impl Op {
    /// The step references this op reads, as `(field pointer, step name)` in evaluation-argument
    /// order.
    pub fn inputs(&self) -> Vec<(String, &str)> {
        match self {
            Op::Constant(_)
            | Op::Gradient { .. }
            | Op::Noise(_)
            | Op::Image { .. }
            | Op::Input { .. } => vec![],
            Op::Recipe { inputs, .. } => inputs
                .iter()
                .map(|(key, step)| (format!("inputs/{}", pointer_token(key)), step.as_str()))
                .collect(),
            Op::TileSampler(spec) => spec
                .pattern
                .as_deref()
                .map(|p| ("pattern".to_string(), p))
                .into_iter()
                .collect(),
            Op::Blend { a, b, mask, .. } => {
                let mut v = vec![("a".to_string(), a.as_str()), ("b".to_string(), b.as_str())];
                if let Some(m) = mask {
                    v.push(("mask".to_string(), m.as_str()));
                }
                v
            }
            Op::Warp { input, by, .. } => {
                vec![("input".to_string(), input), ("by".to_string(), by)]
            }
            Op::Levels { input, .. }
            | Op::Curve { input, .. }
            | Op::Clamp { input, .. }
            | Op::Remap { input, .. }
            | Op::Colorize { input, .. }
            | Op::Grayscale { input }
            | Op::Blur { input, .. }
            | Op::NormalFromHeight { input, .. }
            | Op::FloodFill { input, .. }
            | Op::Distance { input, .. }
            | Op::AmbientOcclusion { input, .. }
            | Op::Curvature { input, .. } => vec![("input".to_string(), input)],
        }
    }
}

/// Decodes `#rrggbb` (sRGB) to linear RGB.
pub fn parse_hex(text: &str) -> Option<[f32; 3]> {
    let digits = text.strip_prefix('#')?;
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |i: usize| {
        let byte = u8::from_str_radix(&digits[i * 2..i * 2 + 2], 16).ok()?;
        Some(srgb_to_linear(f32::from(byte) / 255.0))
    };
    Some([channel(0)?, channel(1)?, channel(2)?])
}

pub(crate) fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        powf((c + 0.055) / 1.055, 2.4)
    }
}

/// A field-reading failure; the diagnostic is already recorded.
pub(crate) struct Failed;
pub(crate) type Read<T> = Result<T, Failed>;

enum Scalar {
    Number(f64),
    Color([f32; 3]),
}

/// Reads one mapping's fields, substituting `$param` and recording a diagnostic per problem.
pub(crate) struct Reader<'a> {
    base: String,
    label: String,
    body: &'a Map<String, Value>,
    scope: &'a Scope,
    diags: &'a mut Vec<Diagnostic>,
    known: Vec<&'static str>,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(
        base: String,
        label: String,
        body: &'a Map<String, Value>,
        scope: &'a Scope,
        diags: &'a mut Vec<Diagnostic>,
    ) -> Self {
        Reader {
            base,
            label,
            body,
            scope,
            diags,
            known: Vec::new(),
        }
    }

    fn ptr(&self, field: &str) -> String {
        format!("{}/{field}", self.base)
    }

    fn structure(&mut self, field: &str, message: String, help: String) -> Failed {
        let d = Diagnostic::new(code::TEXTURE_STRUCTURE, message)
            .with_path(self.ptr(field))
            .with_help(help);
        self.diags.push(d);
        Failed
    }

    pub(crate) fn data(&mut self, field: &str, message: String, help: String) -> Failed {
        let d = Diagnostic::new(code::TEXTURE_DATA, message)
            .with_path(self.ptr(field))
            .with_help(help);
        self.diags.push(d);
        Failed
    }

    fn missing(&mut self, field: &str, expected: &str) -> Failed {
        let message = format!("{} needs a `{field}` field", self.label);
        self.structure(field, message, format!("add `{field}: {expected}`"))
    }

    fn get(&mut self, field: &'static str) -> Option<&'a Value> {
        self.known.push(field);
        self.body.get(field)
    }

    fn scalar(&mut self, field: &'static str, expected: &str) -> Read<Option<Scalar>> {
        let Some(value) = self.get(field) else {
            return Ok(None);
        };
        match value {
            Value::Number(n) => Ok(Some(Scalar::Number(n.as_f64().unwrap_or(f64::NAN)))),
            Value::String(s) if s.starts_with('$') => match self.scope.get(&s[1..]) {
                Some(ParamValue::Number(x)) => Ok(Some(Scalar::Number(*x))),
                Some(ParamValue::Color(c)) => Ok(Some(Scalar::Color(*c))),
                None => {
                    let known: Vec<&str> = self.scope.keys().map(String::as_str).collect();
                    let help = match nearest(&s[1..], known) {
                        Some(near) => format!("did you mean `${near}`?"),
                        None => format!("declare `{}` under `params:`", &s[1..]),
                    };
                    Err(self.data(
                        field,
                        format!("`{s}` is not a parameter of this recipe"),
                        help,
                    ))
                }
            },
            Value::String(s) => match parse_hex(s) {
                Some(c) => Ok(Some(Scalar::Color(c))),
                None => Err(self.data(
                    field,
                    format!("`{s}` is not a number or a `#rrggbb` color"),
                    format!("write `{field}: {expected}`"),
                )),
            },
            other => {
                let message = format!(
                    "`{field}` must be {expected}, found {}",
                    value_phrase(other)
                );
                Err(self.structure(field, message, format!("write `{field}: {expected}`")))
            }
        }
    }

    pub(crate) fn number(&mut self, field: &'static str, default: Option<f64>) -> Read<f64> {
        match self.scalar(field, "a number")? {
            Some(Scalar::Number(x)) if x.is_finite() => Ok(x),
            Some(Scalar::Number(_)) => Err(self.data(
                field,
                format!("`{field}` must be finite"),
                "use a finite number".into(),
            )),
            Some(Scalar::Color(_)) => Err(self.data(
                field,
                format!("`{field}` needs a number, found a color"),
                "use a numeric value or a numeric `$param`".into(),
            )),
            None => default.ok_or_else(|| self.missing(field, "<number>")),
        }
    }

    /// A number in `[lo, hi]`.
    pub(crate) fn ranged(
        &mut self,
        field: &'static str,
        default: Option<f64>,
        lo: f64,
        hi: f64,
    ) -> Read<f32> {
        let x = self.number(field, default)?;
        if x < lo || x > hi {
            return Err(self.data(
                field,
                format!("`{field}` must be in [{lo}, {hi}], found {x}"),
                format!("use a value between {lo} and {hi}"),
            ));
        }
        Ok(x as f32)
    }

    /// A number above zero.
    pub(crate) fn positive(&mut self, field: &'static str, default: Option<f64>) -> Read<f32> {
        let x = self.number(field, default)?;
        if x <= 0.0 {
            return Err(self.data(
                field,
                format!("`{field}` must be above 0, found {x}"),
                "use a positive value".into(),
            ));
        }
        Ok(x as f32)
    }

    /// Any finite number as `f32`.
    pub(crate) fn float(&mut self, field: &'static str, default: Option<f64>) -> Read<f32> {
        self.number(field, default).map(|x| x as f32)
    }

    /// An integer in `[lo, hi]`.
    pub(crate) fn int(
        &mut self,
        field: &'static str,
        default: Option<i64>,
        lo: i64,
        hi: i64,
    ) -> Read<u32> {
        let x = self.number(field, default.map(|d| d as f64))?;
        if x.fract() != 0.0 {
            let message = format!("`{field}` must be an integer, found {x}");
            return Err(self.structure(field, message, format!("write `{field}: {}`", x.round())));
        }
        if x < lo as f64 || x > hi as f64 {
            return Err(self.data(
                field,
                format!("`{field}` must be in [{lo}, {hi}], found {x}"),
                format!("use an integer between {lo} and {hi}"),
            ));
        }
        Ok(x as u32)
    }

    /// One of `options`, by index.
    pub(crate) fn word(
        &mut self,
        field: &'static str,
        default: Option<&str>,
        options: &[&str],
    ) -> Read<usize> {
        let found = match self.get(field) {
            Some(Value::String(s)) => s.as_str(),
            Some(other) => {
                let message = format!("`{field}` must be a string, found {}", value_phrase(other));
                return Err(self.structure(
                    field,
                    message,
                    format!("use one of: {}", options.join(", ")),
                ));
            }
            None => match default {
                Some(d) => d,
                None => return Err(self.missing(field, &options.join("|"))),
            },
        };
        if let Some(i) = options.iter().position(|o| *o == found) {
            return Ok(i);
        }
        let help = match nearest(found, options.iter().copied()) {
            Some(near) => format!("did you mean `{near}`?"),
            None => format!("use one of: {}", options.join(", ")),
        };
        Err(self.structure(
            field,
            format!("`{found}` is not a `{field}` of {}", self.label),
            help,
        ))
    }

    /// A string field.
    pub(crate) fn string(&mut self, field: &'static str) -> Read<String> {
        match self.get(field) {
            Some(Value::String(s)) => Ok(s.clone()),
            Some(other) => {
                let message = format!("`{field}` must be a string, found {}", value_phrase(other));
                Err(self.structure(field, message, format!("write `{field}: <text>`")))
            }
            None => Err(self.missing(field, "<text>")),
        }
    }

    /// A step reference; whether the step exists is checked with the graph.
    pub(crate) fn step(&mut self, field: &'static str) -> Read<String> {
        self.string(field)
    }

    /// An optional step reference.
    pub(crate) fn opt_step(&mut self, field: &'static str) -> Read<Option<String>> {
        if self.body.contains_key(field) {
            self.step(field).map(Some)
        } else {
            self.known.push(field);
            Ok(None)
        }
    }

    /// A color: `#rrggbb` or a color `$param`, linear.
    pub(crate) fn color(&mut self, field: &'static str) -> Read<[f32; 3]> {
        match self.scalar(field, "\"#rrggbb\"")? {
            Some(Scalar::Color(c)) => Ok(c),
            Some(Scalar::Number(_)) => Err(self.data(
                field,
                format!("`{field}` needs a `#rrggbb` color, found a number"),
                format!("write `{field}: \"#808080\"`"),
            )),
            None => Err(self.missing(field, "\"#rrggbb\"")),
        }
    }

    /// A number or a color, as a `constant` takes.
    pub(crate) fn constant(&mut self, field: &'static str) -> Read<Const> {
        match self.scalar(field, "a number or \"#rrggbb\"")? {
            Some(Scalar::Number(x)) if x.is_finite() => Ok(Const::Gray(x as f32)),
            Some(Scalar::Number(_)) => Err(self.data(
                field,
                format!("`{field}` must be finite"),
                "use a finite number".into(),
            )),
            Some(Scalar::Color(c)) => Ok(Const::Color(c)),
            None => Err(self.missing(field, "<number or \"#rrggbb\">")),
        }
    }

    /// A non-empty block list of mappings; entries come back with their index.
    pub(crate) fn mappings(
        &mut self,
        field: &'static str,
        min: usize,
        example: &str,
    ) -> Read<Vec<(usize, &'a Map<String, Value>)>> {
        let Some(value) = self.get(field) else {
            return Err(self.missing(field, example));
        };
        let Value::Array(items) = value else {
            let message = format!("`{field}` must be a list, found {}", value_phrase(value));
            return Err(self.structure(field, message, format!("write `{field}` as a block list")));
        };
        if items.len() < min {
            return Err(self.data(
                field,
                format!(
                    "`{field}` needs at least {min} entries, found {}",
                    items.len()
                ),
                format!("add entries, e.g. {example}"),
            ));
        }
        let mut out = Vec::new();
        let mut failed = false;
        for (i, item) in items.iter().enumerate() {
            match item {
                Value::Object(map) => out.push((i, map)),
                other => {
                    failed = true;
                    let message = format!(
                        "`{field}` entry {i} must be a mapping, found {}",
                        value_phrase(other)
                    );
                    self.structure(
                        &format!("{field}/{i}"),
                        message,
                        format!("write it as {example}"),
                    );
                }
            }
        }
        if failed { Err(Failed) } else { Ok(out) }
    }

    /// A sub-reader over one list entry, with the field path under this reader's base.
    pub(crate) fn entry<R>(
        &mut self,
        field: &str,
        index: usize,
        body: &Map<String, Value>,
        f: impl FnOnce(&mut Reader<'_>) -> R,
    ) -> R {
        let base = format!("{}/{field}/{index}", self.base);
        let label = format!("`{field}` entry {index} of {}", self.label);
        let mut r = Reader::new(base, label, body, self.scope, &mut *self.diags);
        let out = f(&mut r);
        r.finish();
        out
    }

    /// Reports every field no read asked for, with the nearest known one.
    pub(crate) fn finish(&mut self) {
        let known = self.known.clone();
        for key in self.body.keys() {
            if known.iter().any(|k| k == key) {
                continue;
            }
            let help = match nearest(key, known.iter().copied()) {
                Some(near) => format!("did you mean `{near}`?"),
                None => format!("remove `{key}`; the fields here are: {}", known.join(", ")),
            };
            let d = Diagnostic::new(
                code::TEXTURE_STRUCTURE,
                format!("{} has no field `{key}`", self.label),
            )
            .with_path(self.ptr(&pointer_token(key)))
            .with_help(help);
            self.diags.push(d);
        }
    }
}

const STEP_SEED_MAX: i64 = u32::MAX as i64;

/// Reads one step's body as an op; `None` when a diagnostic was recorded.
pub(crate) fn parse_op(
    name: &str,
    body: &Map<String, Value>,
    scope: &Scope,
    diags: &mut Vec<Diagnostic>,
) -> Option<Op> {
    let before = diags.len();
    let base = format!("/steps/{}", pointer_token(name));
    let op_name = match body.get("op") {
        Some(Value::String(s)) => s.as_str(),
        Some(other) => {
            diags.push(
                Diagnostic::new(
                    code::TEXTURE_STRUCTURE,
                    format!(
                        "step `{name}`: `op` must be a string, found {}",
                        value_phrase(other)
                    ),
                )
                .with_path(format!("{base}/op"))
                .with_help(format!("use one of: {}", OP_NAMES.join(", "))),
            );
            return None;
        }
        None => {
            diags.push(
                Diagnostic::new(
                    code::TEXTURE_STRUCTURE,
                    format!("step `{name}` has no `op`"),
                )
                .with_path(base)
                .with_help(format!("add `op:` — one of: {}", OP_NAMES.join(", "))),
            );
            return None;
        }
    };
    if !OP_NAMES.contains(&op_name) {
        let help = match nearest(op_name, OP_NAMES.iter().copied()) {
            Some(near) => format!("did you mean `{near}`?"),
            None => format!("use one of: {}", OP_NAMES.join(", ")),
        };
        diags.push(
            Diagnostic::new(
                code::TEXTURE_STRUCTURE,
                format!("step `{name}` has an unknown op `{op_name}`"),
            )
            .with_path(format!("{base}/op"))
            .with_help(help),
        );
        return None;
    }
    let label = format!("step `{name}` (op `{op_name}`)");
    let mut r = Reader::new(base, label, body, scope, diags);
    r.known.push("op");
    let op = build(&mut r, op_name);
    r.finish();
    if diags.len() > before { None } else { op.ok() }
}

fn build(r: &mut Reader<'_>, op: &str) -> Read<Op> {
    match op {
        "constant" => {
            let value = r.constant("value");
            Ok(Op::Constant(value?))
        }
        "gradient" => {
            let axis = r.word("axis", None, &["u", "v"]);
            let shape = r.word("shape", Some("ramp"), &["ramp", "triangle", "sine"]);
            let repeat = r.int("repeat", Some(1), 1, 4096);
            Ok(Op::Gradient {
                axis: [Axis::U, Axis::V][axis?],
                shape: [
                    GradientShape::Ramp,
                    GradientShape::Triangle,
                    GradientShape::Sine,
                ][shape?],
                repeat: repeat?,
            })
        }
        "noise" => noise(r),
        "blend" => {
            let a = r.step("a");
            let b = r.step("b");
            let mask = r.opt_step("mask");
            let mode = r.word(
                "mode",
                Some("mix"),
                &[
                    "mix",
                    "add",
                    "subtract",
                    "multiply",
                    "screen",
                    "overlay",
                    "min",
                    "max",
                    "difference",
                ],
            );
            let opacity = r.ranged("opacity", Some(1.0), 0.0, 1.0);
            Ok(Op::Blend {
                a: a?,
                b: b?,
                mask: mask?,
                mode: [
                    BlendMode::Mix,
                    BlendMode::Add,
                    BlendMode::Subtract,
                    BlendMode::Multiply,
                    BlendMode::Screen,
                    BlendMode::Overlay,
                    BlendMode::Min,
                    BlendMode::Max,
                    BlendMode::Difference,
                ][mode?],
                opacity: opacity?,
            })
        }
        "levels" => {
            let input = r.step("input");
            let in_low = r.float("in_low", Some(0.0));
            let in_high = r.float("in_high", Some(1.0));
            let gamma = r.positive("gamma", Some(1.0));
            let out_low = r.float("out_low", Some(0.0));
            let out_high = r.float("out_high", Some(1.0));
            let (in_low, in_high) = (in_low?, in_high?);
            if in_low == in_high {
                return Err(r.data(
                    "in_high",
                    "`in_high` equals `in_low`, so the input range is empty".into(),
                    "make `in_high` differ from `in_low`".into(),
                ));
            }
            Ok(Op::Levels {
                input: input?,
                in_low,
                in_high,
                gamma: gamma?,
                out_low: out_low?,
                out_high: out_high?,
            })
        }
        "curve" => {
            let input = r.step("input");
            let entries = r.mappings("points", 2, "- x: 0.0\n  y: 0.0");
            let mut points: Vec<(f32, f32)> = Vec::new();
            let mut ok = true;
            for (i, body) in entries? {
                let point = r.entry("points", i, body, |e| {
                    let x = e.float("x", None);
                    let y = e.float("y", None);
                    Ok::<_, Failed>((x?, y?))
                });
                match point {
                    Ok(p) => {
                        if points.last().is_some_and(|last| p.0 <= last.0) {
                            ok = false;
                            r.data(
                                &format!("points/{i}/x"),
                                format!(
                                    "curve point {i} has `x` {} not above the previous point's",
                                    p.0
                                ),
                                "list points with strictly increasing `x`".into(),
                            );
                        }
                        points.push(p);
                    }
                    Err(Failed) => ok = false,
                }
            }
            if !ok {
                return Err(Failed);
            }
            Ok(Op::Curve {
                input: input?,
                points,
            })
        }
        "clamp" => {
            let input = r.step("input");
            let min = r.float("min", Some(0.0));
            let max = r.float("max", Some(1.0));
            let (min, max) = (min?, max?);
            if min > max {
                return Err(r.data(
                    "max",
                    format!("`max` {max} is below `min` {min}"),
                    "swap them or raise `max`".into(),
                ));
            }
            Ok(Op::Clamp {
                input: input?,
                min,
                max,
            })
        }
        "remap" => {
            let input = r.step("input");
            let from_low = r.float("from_low", None);
            let from_high = r.float("from_high", None);
            let to_low = r.float("to_low", None);
            let to_high = r.float("to_high", None);
            let (from_low, from_high) = (from_low?, from_high?);
            if from_low == from_high {
                return Err(r.data(
                    "from_high",
                    "`from_high` equals `from_low`, so the source range is empty".into(),
                    "make `from_high` differ from `from_low`".into(),
                ));
            }
            Ok(Op::Remap {
                input: input?,
                from_low,
                from_high,
                to_low: to_low?,
                to_high: to_high?,
            })
        }
        "colorize" => {
            let input = r.step("input");
            let entries = r.mappings("stops", 2, "- at: 0.0\n  color: \"#000000\"");
            let mut stops: Vec<(f32, [f32; 3])> = Vec::new();
            let mut ok = true;
            for (i, body) in entries? {
                let stop = r.entry("stops", i, body, |e| {
                    let at = e.ranged("at", None, 0.0, 1.0);
                    let color = e.color("color");
                    Ok::<_, Failed>((at?, color?))
                });
                match stop {
                    Ok(s) => {
                        if stops.last().is_some_and(|last| s.0 < last.0) {
                            ok = false;
                            r.data(
                                &format!("stops/{i}/at"),
                                format!("stop {i} has `at` {} below the previous stop's", s.0),
                                "order stops by non-decreasing `at`".into(),
                            );
                        }
                        stops.push(s);
                    }
                    Err(Failed) => ok = false,
                }
            }
            if !ok {
                return Err(Failed);
            }
            Ok(Op::Colorize {
                input: input?,
                stops,
            })
        }
        "grayscale" => {
            let input = r.step("input");
            Ok(Op::Grayscale { input: input? })
        }
        "warp" => {
            let input = r.step("input");
            let by = r.step("by");
            let amount = r.float("amount", None);
            let angle = r.float("angle", Some(0.0));
            Ok(Op::Warp {
                input: input?,
                by: by?,
                amount: amount?,
                angle: angle?,
            })
        }
        "blur" => {
            let input = r.step("input");
            let radius = r.ranged("radius", None, 0.0, 1.0);
            Ok(Op::Blur {
                input: input?,
                radius: radius?,
            })
        }
        "normal_from_height" => {
            let input = r.step("input");
            let depth = r.positive("depth", Some(0.05));
            Ok(Op::NormalFromHeight {
                input: input?,
                depth: depth?,
            })
        }
        "tile_sampler" => tile_sampler(r),
        "flood_fill" => {
            let input = r.step("input");
            let threshold = r.ranged("threshold", Some(0.5), 0.0, 1.0);
            let output = r.word("output", Some("random"), &["random", "gradient", "uv"]);
            let angle = r.float("angle", Some(0.0));
            let angle_jitter = r.ranged("angle_jitter", Some(0.0), 0.0, 1.0);
            let seed = r.int("seed", Some(0), 0, STEP_SEED_MAX);
            Ok(Op::FloodFill {
                input: input?,
                threshold: threshold?,
                output: [FillOutput::Random, FillOutput::Gradient, FillOutput::Uv][output?],
                angle: angle?,
                angle_jitter: angle_jitter?,
                seed: seed?,
            })
        }
        "distance" => {
            let input = r.step("input");
            let threshold = r.ranged("threshold", Some(0.5), 0.0, 1.0);
            let max = r.positive("max", Some(0.1));
            Ok(Op::Distance {
                input: input?,
                threshold: threshold?,
                max: max?,
            })
        }
        "ambient_occlusion" => {
            let input = r.step("input");
            let radius = r.number("radius", Some(0.05));
            let depth = r.positive("depth", Some(0.05));
            let strength = r.ranged("strength", Some(1.0), 0.0, 1.0);
            let radius = open_below(r, "radius", radius?, 0.5)?;
            Ok(Op::AmbientOcclusion {
                input: input?,
                radius,
                depth: depth?,
                strength: strength?,
            })
        }
        "curvature" => {
            let input = r.step("input");
            let radius = r.number("radius", Some(0.01));
            let strength = r.positive("strength", Some(4.0));
            let radius = open_below(r, "radius", radius?, 0.25)?;
            Ok(Op::Curvature {
                input: input?,
                radius,
                strength: strength?,
            })
        }
        "image" => {
            let path = r.string("path");
            let color = r.word("color", Some("srgb"), &["srgb", "linear"]);
            Ok(Op::Image {
                path: path?,
                srgb: color? == 0,
            })
        }
        "input" => input_op(r),
        "recipe" => recipe_op(r),
        _ => unreachable!("op name checked against OP_NAMES"),
    }
}

fn noise(r: &mut Reader<'_>) -> Read<Op> {
    let kind = r.word("type", None, &["value", "gradient", "cellular"]);
    let scale = r.int("scale", None, 1, 65536);
    let octaves = r.int("octaves", Some(1), 1, 12);
    let lacunarity = r.int("lacunarity", Some(2), 2, 65536);
    let gain = r.float("gain", Some(0.5));
    let seed = r.int("seed", Some(0), 0, STEP_SEED_MAX);
    let kind = [NoiseKind::Value, NoiseKind::Gradient, NoiseKind::Cellular][kind?];
    let (mut ret, mut jitter) = (Ok(CellReturn::F1), Ok(1.0));
    if kind == NoiseKind::Cellular {
        ret = r
            .word("return", Some("f1"), &["f1", "f2", "f2_minus_f1"])
            .map(|i| [CellReturn::F1, CellReturn::F2, CellReturn::F2MinusF1][i]);
        jitter = r.ranged("jitter", Some(1.0), 0.0, 1.0);
    }
    let (scale, octaves, lacunarity) = (scale?, octaves?, lacunarity?);
    let mut finest = u64::from(scale);
    for _ in 1..octaves {
        finest = finest.saturating_mul(u64::from(lacunarity));
    }
    if finest > 65536 {
        return Err(r.data(
            "octaves",
            format!(
                "the finest octave would have {finest} lattice cells per tile; the limit is 65536"
            ),
            "lower `scale`, `lacunarity` or `octaves`".into(),
        ));
    }
    Ok(Op::Noise(NoiseSpec {
        kind,
        scale,
        octaves,
        lacunarity,
        gain: gain?,
        seed: seed?,
        ret: ret?,
        jitter: jitter?,
    }))
}

/// `x` in `(0, hi]`, as `f32`.
fn open_below(r: &mut Reader<'_>, field: &str, x: f64, hi: f64) -> Read<f32> {
    if x > 0.0 && x <= hi {
        return Ok(x as f32);
    }
    Err(r.data(
        field,
        format!("`{field}` must be above 0 and at most {hi}, found {x}"),
        format!("use a value in (0, {hi}]"),
    ))
}

fn tile_sampler(r: &mut Reader<'_>) -> Read<Op> {
    let columns = r.int("columns", None, 1, 1024);
    let rows = r.int("rows", None, 1, 1024);
    let shape = r.word("shape", Some("box"), &["box", "disc", "bell", "pyramid"]);
    let pattern = r.opt_step("pattern");
    let scale = r.ranged("scale", Some(1.0), 0.0, 4.0);
    let scale_jitter = r.ranged("scale_jitter", Some(0.0), 0.0, 1.0);
    let position_jitter = r.ranged("position_jitter", Some(0.0), 0.0, 1.0);
    let rotation_jitter = r.ranged("rotation_jitter", Some(0.0), 0.0, 1.0);
    let value_jitter = r.ranged("value_jitter", Some(0.0), 0.0, 1.0);
    let rotation = r.float("rotation", Some(0.0));
    let offset = r.ranged("offset", Some(0.0), 0.0, 1.0);
    let offset_jitter = r.ranged("offset_jitter", Some(0.0), 0.0, 1.0);
    let gap = r.ranged("gap", Some(0.0), 0.0, 0.5);
    let pattern_scale_jitter = r.ranged("pattern_scale_jitter", Some(0.0), 0.0, 1.0);
    let pattern_offset_jitter = r.ranged("pattern_offset_jitter", Some(0.0), 0.0, 1.0);
    let density = r.ranged("density", Some(1.0), 0.0, 1.0);
    let combine = r.word("combine", Some("max"), &["max", "add"]);
    let seed = r.int("seed", Some(0), 0, STEP_SEED_MAX);
    let (columns, rows, offset) = (columns?, rows?, offset?);
    if offset > 0.0 && rows % 2 != 0 {
        return Err(r.data(
            "offset",
            format!("`offset` shifts odd rows, which only tiles with an even `rows`; found {rows}"),
            "make `rows` even, or set `offset: 0`".into(),
        ));
    }
    Ok(Op::TileSampler(TileSpec {
        columns,
        rows,
        shape: [
            StampShape::Box,
            StampShape::Disc,
            StampShape::Bell,
            StampShape::Pyramid,
        ][shape?],
        pattern: pattern?,
        scale: scale?,
        scale_jitter: scale_jitter?,
        position_jitter: position_jitter?,
        rotation_jitter: rotation_jitter?,
        value_jitter: value_jitter?,
        rotation: rotation?,
        offset,
        offset_jitter: offset_jitter?,
        gap: gap?,
        pattern_scale_jitter: pattern_scale_jitter?,
        pattern_offset_jitter: pattern_offset_jitter?,
        density: density?,
        combine: [Combine::Max, Combine::Add][combine?],
        seed: seed?,
    }))
}

fn input_op(r: &mut Reader<'_>) -> Read<Op> {
    let kind = r.word("type", None, &["gray", "color"]);
    let channels = if kind? == 0 { 1 } else { 3 };
    let default = if r.body.contains_key("default") {
        let value = r.constant("default")?;
        let found = match value {
            Const::Gray(_) => 1,
            Const::Color(_) => 3,
        };
        if found != channels {
            let (want, help) = if channels == 1 {
                ("a number", "write `default: 0.5`, or `type: color`")
            } else {
                (
                    "a `#rrggbb` color",
                    "write `default: \"#808080\"`, or `type: gray`",
                )
            };
            return Err(r.data(
                "default",
                format!(
                    "a {} input needs {want} as its `default`",
                    if channels == 1 { "gray" } else { "color" }
                ),
                help.into(),
            ));
        }
        value
    } else {
        r.known.push("default");
        if channels == 1 {
            Const::Gray(0.0)
        } else {
            Const::Color([0.0; 3])
        }
    };
    Ok(Op::Input { channels, default })
}

fn recipe_op(r: &mut Reader<'_>) -> Read<Op> {
    let path = r.string("path");
    let output = r.string("output");
    let seed = r.int("seed", Some(0), 0, STEP_SEED_MAX);
    let params = recipe_params(r);
    let inputs = recipe_inputs(r);
    Ok(Op::Recipe {
        path: path?,
        output: output?,
        params: params?,
        inputs: inputs?,
        seed: seed?,
    })
}

fn recipe_inputs(r: &mut Reader<'_>) -> Read<Vec<(String, String)>> {
    let Some(value) = r.get("inputs") else {
        return Ok(Vec::new());
    };
    let Value::Object(map) = value else {
        let message = format!("`inputs` must be a mapping, found {}", value_phrase(value));
        return Err(r.structure(
            "inputs",
            message,
            "write `inputs:` as `<input step of the recipe>: <step of this recipe>` pairs".into(),
        ));
    };
    let mut out = Vec::new();
    let mut failed = false;
    for (key, v) in map {
        match v {
            Value::String(step) => out.push((key.clone(), step.clone())),
            other => {
                failed = true;
                let message = format!(
                    "input `{key}` must name a step, found {}",
                    value_phrase(other)
                );
                r.structure(
                    &format!("inputs/{}", pointer_token(key)),
                    message,
                    format!("write `{key}: <step name>`"),
                );
            }
        }
    }
    if failed { Err(Failed) } else { Ok(out) }
}

fn recipe_params(r: &mut Reader<'_>) -> Read<Vec<(String, ParamValue)>> {
    let Some(value) = r.get("params") else {
        return Ok(Vec::new());
    };
    let Value::Object(map) = value else {
        let message = format!("`params` must be a mapping, found {}", value_phrase(value));
        return Err(r.structure(
            "params",
            message,
            "write `params:` as name: value pairs".into(),
        ));
    };
    let mut out = Vec::new();
    let mut failed = false;
    for (name, v) in map {
        let resolved = match v {
            Value::Number(n) => Some(ParamValue::Number(n.as_f64().unwrap_or(f64::NAN))),
            Value::String(s) if s.starts_with('$') => r.scope.get(&s[1..]).copied(),
            Value::String(s) => parse_hex(s).map(ParamValue::Color),
            _ => None,
        };
        match resolved {
            Some(p) => out.push((name.clone(), p)),
            None => {
                failed = true;
                let message = match v {
                    Value::String(s) if s.starts_with('$') => {
                        format!("`{s}` is not a parameter of this recipe")
                    }
                    _ => format!("parameter `{name}` must be a number or a `#rrggbb` color"),
                };
                r.data(
                    &format!("params/{}", pointer_token(name)),
                    message,
                    "bind a number, a `\"#rrggbb\"` color, or a `$param` of this recipe".into(),
                );
            }
        }
    }
    if failed { Err(Failed) } else { Ok(out) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_decodes_to_linear() {
        assert_eq!(parse_hex("#000000"), Some([0.0, 0.0, 0.0]));
        assert_eq!(parse_hex("#ffffff"), Some([1.0, 1.0, 1.0]));
        assert!(parse_hex("#fff").is_none());
        assert!(parse_hex("ffffff").is_none());
        assert!(parse_hex("#gggggg").is_none());
        let mid = parse_hex("#808080").unwrap()[0];
        assert!((mid - 0.2158605).abs() < 1e-5);
    }
}
