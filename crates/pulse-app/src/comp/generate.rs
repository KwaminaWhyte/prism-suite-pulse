//! Generate (whole-buffer fill) effects — the After-Effects *Generate* category.
//!
//! Unlike [`Effect`](super::Effect) (a per-pixel colour-correction pass that
//! *reads* the layer's pixels) or [`SpatialEffect`](super::SpatialEffect) (a
//! convolve / bloom / offset pass that *filters* them), a **generate** effect
//! *replaces* the layer's pixels: it synthesises content from its parameters and
//! the pixel position, filling the layer's quad. This mirrors After Effects'
//! *Generate* category.
//!
//! The family so far:
//! - **Fractal Noise** — multi-octave gradient noise (the motion-design
//!   workhorse: smoke, clouds, energy, organic textures, mattes, displacement).
//!   Grayscale, authored in linear `[0,1]`.
//! - **Gradient / Ramp** — a linear or radial colour ramp between two colours
//!   (AE's *Ramp*).
//! - **Checkerboard** — a two-colour chequer grid (AE's *Checkerboard*).
//! - **4-Color Gradient** — four corner colours blended across the frame (AE's
//!   *4-Color Gradient*).
//! - **Grid** — a line grid over a (transparent or filled) background (AE's
//!   *Grid*).
//!
//! Every generator is **deterministic**: the same `(params, evolution, seed,
//! pixel)` always produces the same value, so a frame renders identically on
//! every pass (for the RAM-preview cache, multi-frame render, and golden-frame
//! tests). For Fractal Noise the only motion knob is **evolution**; the colour
//! generators are static within a frame (they animate by keyframing their
//! scalar params / scatter via evolution).
//!
//! Each field is evaluated in the layer's **local** frame (comp px, origin at the
//! layer centre) so it rides the layer's transform — `scale` zooms it, the
//! layer's position/rotation move it. Fractal Noise emits a straight grayscale
//! value treated as *linear-light*; the colour generators emit straight **sRGB**
//! colour (decoded to linear by the compositor at the gamma boundary, exactly
//! like a solid's swatch). Pure (no GPU, no IO, no `Track` sampling here — the
//! evolution/scale values are sampled by the caller and passed in), so the math
//! is unit-testable; it'll migrate to the suite's `prism-fx` host when that lands.

use serde::{Deserialize, Serialize};

/// How the per-octave signed noise is shaped into the fractal sum.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FractalType {
    /// **Basic** fractal: sum the *signed* octaves (smooth, cloud-like, with both
    /// bright and dark lobes). The default.
    #[default]
    Basic,
    /// **Turbulent**: sum the *absolute value* of each signed octave (After
    /// Effects' "Turbulent Smooth/Soft" family) — gives the billowy, ridged,
    /// smoke/fire look with sharp valleys and no negative lobes.
    Turbulent,
}

impl FractalType {
    /// All types, in menu order.
    pub const ALL: [FractalType; 2] = [FractalType::Basic, FractalType::Turbulent];

    /// A short, stable label for the UI.
    pub fn label(self) -> &'static str {
        match self {
            FractalType::Basic => "Basic",
            FractalType::Turbulent => "Turbulent",
        }
    }
}

/// How an out-of-range fractal value is brought back into `[0,1]`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Overflow {
    /// **Clip**: hard-clamp to `[0,1]` (After Effects' default). The default.
    #[default]
    Clip,
    /// **Wrap**: take the fractional part (`rem_euclid 1`), so values cycle
    /// through the range — gives banded / contour-like results.
    Wrap,
    /// **Allow HDR**: leave the value un-clamped (it may exceed `[0,1]`), useful
    /// when the result feeds a later grade / glow.
    AllowHdr,
}

impl Overflow {
    /// All modes, in menu order.
    pub const ALL: [Overflow; 3] = [Overflow::Clip, Overflow::Wrap, Overflow::AllowHdr];

    /// A short, stable label for the UI.
    pub fn label(self) -> &'static str {
        match self {
            Overflow::Clip => "Clip",
            Overflow::Wrap => "Wrap",
            Overflow::AllowHdr => "Allow HDR",
        }
    }

    /// Bring a fractal value back into range per this mode.
    pub fn apply(self, v: f32) -> f32 {
        match self {
            Overflow::Clip => v.clamp(0.0, 1.0),
            Overflow::Wrap => v.rem_euclid(1.0),
            Overflow::AllowHdr => v.max(0.0),
        }
    }
}

/// The shape of a [`GenerateEffect::Ramp`] gradient.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RampShape {
    /// **Linear** ramp: the colour interpolates along the start→end axis,
    /// constant on lines perpendicular to it. The default (AE's *Linear Ramp*).
    #[default]
    Linear,
    /// **Radial** ramp: the colour interpolates with distance from the start
    /// point out to the radius (AE's *Radial Ramp*).
    Radial,
}

impl RampShape {
    /// All shapes, in menu order.
    pub const ALL: [RampShape; 2] = [RampShape::Linear, RampShape::Radial];

    /// A short, stable label for the UI.
    pub fn label(self) -> &'static str {
        match self {
            RampShape::Linear => "Linear",
            RampShape::Radial => "Radial",
        }
    }
}

/// A **generate** (whole-buffer fill) effect in a layer's effect stack.
///
/// A layer carries at most one generate fill (an `Option<GenerateEffect>`): like
/// AE's generate effects each *replaces* the layer's content rather than
/// stacking, so two fills would just override each other.
///
/// All geometry params (start / end / centre / sizes) are in the layer's
/// **local** frame, in comp px with the origin at the layer centre, so the fill
/// rides the layer's transform like the other generators.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum GenerateEffect {
    /// Multi-octave gradient **fractal noise** — the field that drives smoke,
    /// clouds, energy, organic textures, mattes, and displacement. Grayscale
    /// (RGB = value, A = value · `opacity`), evaluated deterministically from the
    /// pixel's layer-local position + `evolution` + `seed`.
    FractalNoise {
        /// How octaves are combined (signed sum vs. abs-sum).
        fractal_type: FractalType,
        /// Output **contrast** about 0.5 (1 = unchanged, >1 punchier, <1 flatter).
        contrast: f32,
        /// Output **brightness** offset added after contrast (`-1..=1` useful range).
        brightness: f32,
        /// Uniform **scale**: the base feature size, in comp px (larger = bigger
        /// blobs). Drives the base sampling frequency `1/scale`.
        scale: f32,
        /// X-scale multiplier (1 = uniform). Stretches features horizontally.
        scale_x: f32,
        /// Y-scale multiplier (1 = uniform). Stretches features vertically.
        scale_y: f32,
        /// **Complexity**: the octave count (1..=10). More octaves add finer detail.
        complexity: u32,
        /// **Sub-influence** (persistence): how much each finer octave contributes
        /// relative to the one before (`0..=1`; AE's "Sub Influence", 0–100%).
        sub_influence: f32,
        /// **Sub-scaling** (lacunarity): the frequency multiplier between octaves
        /// (>1; AE's "Sub Scaling", 2 = each octave doubles frequency).
        sub_scaling: f32,
        /// **Evolution**: the phase/time input that animates the field (the key
        /// motion-design knob). A third noise axis; sweeping it flows the noise.
        evolution: f32,
        /// **Random seed**: salts the gradient hash so different seeds give
        /// independent fields for the same parameters.
        seed: u32,
        /// How out-of-`[0,1]` values are brought back into range.
        overflow: Overflow,
        /// Output **opacity** (the generated value scales the layer's coverage).
        opacity: f32,
    },

    /// **Gradient / Ramp** — a colour ramp between `start_color` and `end_color`,
    /// either [`RampShape::Linear`] (along the start→end axis) or
    /// [`RampShape::Radial`] (with distance from `start` out to `radius`). An
    /// optional **scatter** dithers the ramp parameter (deterministic, seeded by
    /// the pixel) to break up banding.
    Ramp {
        /// Linear vs. radial ramp.
        shape: RampShape,
        /// Ramp start point, layer-local (comp px, origin at centre). The
        /// `start_color` end of the ramp; the radial ramp's centre.
        start: [f32; 2],
        /// Ramp end point, layer-local. The `end_color` end of a linear ramp
        /// (unused for radial, which uses `radius`).
        end: [f32; 2],
        /// Radial ramp radius (comp px): the distance from `start` at which the
        /// ramp reaches `end_color`. Ignored for a linear ramp.
        radius: f32,
        /// Colour at the start of the ramp (straight sRGB).
        start_color: [f32; 3],
        /// Colour at the end of the ramp (straight sRGB).
        end_color: [f32; 3],
        /// **Ramp scatter**: dither amount on the ramp parameter (`0` = clean
        /// bands, larger = more dithered), deterministic per pixel.
        scatter: f32,
        /// Output **opacity** (scales the fill's coverage).
        opacity: f32,
    },

    /// **Checkerboard** — a two-colour chequer grid. Cell `(i + j)` parity picks
    /// `color1` (even) or `color2` (odd); `anchor` shifts the grid origin and
    /// `size` is the cell edge length.
    Checkerboard {
        /// Grid origin offset, layer-local (comp px). Shifts which cells fall
        /// where.
        anchor: [f32; 2],
        /// Cell **width** (comp px).
        size_w: f32,
        /// Cell **height** (comp px).
        size_h: f32,
        /// The **even**-parity cell colour (straight sRGB).
        color1: [f32; 3],
        /// The **odd**-parity cell colour (straight sRGB).
        color2: [f32; 3],
        /// Output **opacity** (scales the fill's coverage).
        opacity: f32,
    },

    /// **4-Color Gradient** — four corner colours bilinearly blended across the
    /// layer's quad (top-left, top-right, bottom-left, bottom-right). An optional
    /// **jitter** dithers the blend (deterministic per pixel) to soften banding;
    /// `blend` biases the bilinear weights toward the corners (sharper) or centre
    /// (softer).
    FourColorGradient {
        /// Top-left corner colour (straight sRGB).
        tl: [f32; 3],
        /// Top-right corner colour (straight sRGB).
        tr: [f32; 3],
        /// Bottom-left corner colour (straight sRGB).
        bl: [f32; 3],
        /// Bottom-right corner colour (straight sRGB).
        br: [f32; 3],
        /// **Blend** sharpness about 0.5: 1 = plain bilinear, >1 pushes the blend
        /// toward the corners (sharper transitions), <1 toward the centre.
        blend: f32,
        /// **Jitter**: dither amount on the blend weights (deterministic per
        /// pixel) to break up banding.
        jitter: f32,
        /// Output **opacity** (scales the fill's coverage).
        opacity: f32,
    },

    /// **Grid** — a line grid over a (transparent or filled) background. Cells of
    /// `size_w × size_h` are outlined with lines of `border` px in `color`;
    /// `anchor` shifts the grid origin. The cell interior is `background` (which
    /// may be transparent).
    Grid {
        /// Grid origin offset, layer-local (comp px).
        anchor: [f32; 2],
        /// Cell **width** (comp px).
        size_w: f32,
        /// Cell **height** (comp px).
        size_h: f32,
        /// Line **width** (comp px).
        border: f32,
        /// Line **colour** (straight sRGB).
        color: [f32; 3],
        /// Cell **background** colour (straight sRGB).
        background: [f32; 3],
        /// Background **opacity** (`0` = transparent cells, so only the lines
        /// show — the common AE grid-over-footage look).
        background_opacity: f32,
        /// Output **opacity** (scales the whole fill's coverage).
        opacity: f32,
    },
}

impl GenerateEffect {
    /// A short, stable label for the UI and the "add effect" menu.
    pub fn label(&self) -> &'static str {
        match self {
            GenerateEffect::FractalNoise { .. } => "Fractal Noise",
            GenerateEffect::Ramp { .. } => "Gradient Ramp",
            GenerateEffect::Checkerboard { .. } => "Checkerboard",
            GenerateEffect::FourColorGradient { .. } => "4-Color Gradient",
            GenerateEffect::Grid { .. } => "Grid",
        }
    }

    /// A fresh, sensibly-defaulted instance of each generate effect, for the
    /// "add effect" menu / browser. The order is the registry/browser order.
    pub fn defaults() -> [GenerateEffect; 5] {
        [
            GenerateEffect::FractalNoise {
                fractal_type: FractalType::Basic,
                contrast: 1.0,
                brightness: 0.0,
                scale: 80.0,
                scale_x: 1.0,
                scale_y: 1.0,
                complexity: 6,
                sub_influence: 0.6,
                sub_scaling: 2.0,
                evolution: 0.0,
                seed: 0,
                overflow: Overflow::Clip,
                opacity: 1.0,
            },
            GenerateEffect::Ramp {
                shape: RampShape::Linear,
                start: [0.0, -120.0],
                end: [0.0, 120.0],
                radius: 160.0,
                start_color: [0.0, 0.0, 0.0],
                end_color: [1.0, 1.0, 1.0],
                scatter: 0.0,
                opacity: 1.0,
            },
            GenerateEffect::Checkerboard {
                anchor: [0.0, 0.0],
                size_w: 64.0,
                size_h: 64.0,
                color1: [0.0, 0.0, 0.0],
                color2: [1.0, 1.0, 1.0],
                opacity: 1.0,
            },
            GenerateEffect::FourColorGradient {
                tl: [0.90, 0.20, 0.25],
                tr: [0.95, 0.80, 0.20],
                bl: [0.20, 0.45, 0.90],
                br: [0.25, 0.80, 0.45],
                blend: 1.0,
                jitter: 0.0,
                opacity: 1.0,
            },
            GenerateEffect::Grid {
                anchor: [0.0, 0.0],
                size_w: 64.0,
                size_h: 64.0,
                border: 2.0,
                color: [1.0, 1.0, 1.0],
                background: [0.0, 0.0, 0.0],
                background_opacity: 0.0,
                opacity: 1.0,
            },
        ]
    }

    /// Whether this generator emits **colour** (straight sRGB, decoded to linear
    /// by the compositor) rather than Fractal Noise's grayscale *linear* value.
    /// The compositor uses this to pick the right colour-space path.
    pub fn produces_color(&self) -> bool {
        !matches!(self, GenerateEffect::FractalNoise { .. })
    }

    /// Sample the generated **straight grayscale value** at a layer-local pixel
    /// position `(lx, ly)` — only meaningful for [`GenerateEffect::FractalNoise`]
    /// (the field). Returns the noise value brought into range by [`Overflow`] in
    /// `[0,1]` (or `≥0` for `AllowHdr`); for a colour generator returns the
    /// luminance of its colour (so callers that only want a scalar still get one).
    /// Pure and deterministic in `(self, lx, ly)`.
    pub fn value_at(&self, lx: f32, ly: f32) -> f32 {
        match *self {
            GenerateEffect::FractalNoise {
                fractal_type,
                contrast,
                brightness,
                scale,
                scale_x,
                scale_y,
                complexity,
                sub_influence,
                sub_scaling,
                evolution,
                seed,
                overflow,
                ..
            } => {
                // Map the local pixel into noise space: divide by the (per-axis)
                // feature size so a larger `scale` zooms the noise (lower
                // frequency). Guard against a zero/negative scale collapsing the
                // domain to a constant.
                let sx = (scale * scale_x).abs().max(1e-3);
                let sy = (scale * scale_y).abs().max(1e-3);
                let nx = lx / sx;
                let ny = ly / sy;
                let raw = fbm(
                    nx,
                    ny,
                    evolution,
                    seed,
                    complexity.clamp(1, MAX_OCTAVES),
                    sub_influence.clamp(0.0, 1.0),
                    sub_scaling.max(1.0),
                    fractal_type,
                );
                // `raw` is ~[-1,1] (basic) or ~[0,1] (turbulent). Remap basic to
                // [0,1] so both types live in the same display range, then apply
                // contrast about mid-grey and the brightness offset.
                let centered = match fractal_type {
                    FractalType::Basic => raw * 0.5 + 0.5,
                    FractalType::Turbulent => raw,
                };
                let contrasted = (centered - 0.5) * contrast.max(0.0) + 0.5 + brightness;
                overflow.apply(contrasted)
            }
            // For a colour generator the "value" is its colour's luminance, with
            // the colour evaluated at a unit half-extent (the scalar callers don't
            // pass geometry). Mostly used by tests / generic callers; the
            // compositor uses `rgba_at`.
            _ => {
                let [r, g, b, _] = self.rgba_at(lx, ly, 100.0, 100.0);
                0.2126 * r + 0.7152 * g + 0.0722 * b
            }
        }
    }

    /// Sample the generated **straight RGBA** at a layer-local pixel position
    /// `(lx, ly)`, given the layer's half-extents `(half_w, half_h)` (so the
    /// 4-color gradient / radial ramp can normalise across the quad). RGB is
    /// straight colour (sRGB for the colour generators, linear-grayscale for
    /// Fractal Noise) and A is the fill coverage **before** the effect/layer
    /// opacity multiply. Pure and deterministic.
    pub fn rgba_at(&self, lx: f32, ly: f32, half_w: f32, half_h: f32) -> [f32; 4] {
        match *self {
            GenerateEffect::FractalNoise { .. } => {
                let v = self.value_at(lx, ly);
                [v, v, v, v]
            }

            GenerateEffect::Ramp {
                shape,
                start,
                end,
                radius,
                start_color,
                end_color,
                scatter,
                ..
            } => {
                let mut t = match shape {
                    RampShape::Linear => {
                        // Project (lx,ly) onto the start→end axis; t = the
                        // normalised position along it, clamped to the endpoints.
                        let dx = end[0] - start[0];
                        let dy = end[1] - start[1];
                        let len2 = dx * dx + dy * dy;
                        if len2 <= 1e-6 {
                            0.0
                        } else {
                            ((lx - start[0]) * dx + (ly - start[1]) * dy) / len2
                        }
                    }
                    RampShape::Radial => {
                        let dx = lx - start[0];
                        let dy = ly - start[1];
                        let r = (dx * dx + dy * dy).sqrt();
                        r / radius.abs().max(1e-3)
                    }
                };
                // Optional scatter: dither the ramp parameter, deterministic per
                // pixel (a hash of the rounded local position), centred on 0.
                if scatter.abs() > 1e-6 {
                    let n = hash_unit(lx, ly, 0) - 0.5;
                    t += n * scatter;
                }
                let t = t.clamp(0.0, 1.0);
                let rgb = lerp3(start_color, end_color, t);
                [rgb[0], rgb[1], rgb[2], 1.0]
            }

            GenerateEffect::Checkerboard {
                anchor,
                size_w,
                size_h,
                color1,
                color2,
                ..
            } => {
                let cw = size_w.abs().max(1e-3);
                let ch = size_h.abs().max(1e-3);
                // Cell indices (floored), shifted by the anchor.
                let i = ((lx - anchor[0]) / cw).floor() as i64;
                let j = ((ly - anchor[1]) / ch).floor() as i64;
                let rgb = if (i + j).rem_euclid(2) == 0 {
                    color1
                } else {
                    color2
                };
                [rgb[0], rgb[1], rgb[2], 1.0]
            }

            GenerateEffect::FourColorGradient {
                tl,
                tr,
                bl,
                br,
                blend,
                jitter,
                ..
            } => {
                // Normalised position in the quad, u/v ∈ [0,1] (origin top-left).
                let mut u = (lx / half_w.max(1e-3)) * 0.5 + 0.5;
                let mut v = (ly / half_h.max(1e-3)) * 0.5 + 0.5;
                if jitter.abs() > 1e-6 {
                    u += (hash_unit(lx, ly, 1) - 0.5) * jitter;
                    v += (hash_unit(lx, ly, 2) - 0.5) * jitter;
                }
                let u = bias(u.clamp(0.0, 1.0), blend);
                let v = bias(v.clamp(0.0, 1.0), blend);
                // Bilinear blend of the four corners.
                let top = lerp3(tl, tr, u);
                let bot = lerp3(bl, br, u);
                let rgb = lerp3(top, bot, v);
                [rgb[0], rgb[1], rgb[2], 1.0]
            }

            GenerateEffect::Grid {
                anchor,
                size_w,
                size_h,
                border,
                color,
                background,
                background_opacity,
                ..
            } => {
                let cw = size_w.abs().max(1e-3);
                let ch = size_h.abs().max(1e-3);
                let bw = border.max(0.0);
                // Distance from the nearest vertical / horizontal grid line. A
                // line sits on each integer multiple of the cell size (offset by
                // the anchor); a pixel is "on a line" when within half the border
                // width of one.
                let px = (lx - anchor[0]).rem_euclid(cw);
                let py = (ly - anchor[1]).rem_euclid(ch);
                let dx = px.min(cw - px);
                let dy = py.min(ch - py);
                let on_line = dx <= bw * 0.5 || dy <= bw * 0.5;
                if on_line {
                    [color[0], color[1], color[2], 1.0]
                } else {
                    [
                        background[0],
                        background[1],
                        background[2],
                        background_opacity.clamp(0.0, 1.0),
                    ]
                }
            }
        }
    }

    /// The output **opacity** this generate fill scales coverage by.
    pub fn opacity(&self) -> f32 {
        match *self {
            GenerateEffect::FractalNoise { opacity, .. }
            | GenerateEffect::Ramp { opacity, .. }
            | GenerateEffect::Checkerboard { opacity, .. }
            | GenerateEffect::FourColorGradient { opacity, .. }
            | GenerateEffect::Grid { opacity, .. } => opacity.clamp(0.0, 1.0),
        }
    }
}

/// The maximum octave count (complexity) the fractal sum honours — keeps the
/// per-pixel cost bounded.
pub const MAX_OCTAVES: u32 = 10;

/// Linear interpolation of two RGB triples.
fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// A blend-sharpness bias about 0.5: `amount == 1` is the identity, `> 1` pushes
/// `t` toward the ends (sharper corners), `< 1` toward 0.5 (softer). Symmetric so
/// 0 and 1 are fixed points.
fn bias(t: f32, amount: f32) -> f32 {
    let k = amount.max(0.0);
    if (k - 1.0).abs() < 1e-6 {
        return t;
    }
    // Raise the half-range to the `amount` power: `> 1` steepens the centre
    // transition (pushing values toward 0 / 1 — sharper corners), `< 1` flattens
    // it (toward 0.5 — softer).
    if t < 0.5 {
        0.5 * (2.0 * t).powf(k)
    } else {
        1.0 - 0.5 * (2.0 * (1.0 - t)).powf(k)
    }
}

/// A deterministic pseudo-random value in `[0,1)` for a local pixel position +
/// channel salt — used for ramp scatter / gradient jitter so the dither is
/// stable per (pixel, frame) (never `rand` / `Math.random`).
fn hash_unit(x: f32, y: f32, salt: u32) -> f32 {
    let xi = (x * 16.0).floor() as i64 as u64;
    let yi = (y * 16.0).floor() as i64 as u64;
    let mut h = xi.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= yi.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= (salt as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
    let m = splitmix64(h);
    (m >> 11) as f32 / (1u64 << 53) as f32
}

/// Fractional Brownian motion: sum `octaves` of gradient noise, each at a higher
/// frequency (`lacunarity`) and lower amplitude (`persistence`) than the last.
///
/// `(x, y)` are the noise-space coordinates; `z` is the **evolution** axis (a
/// third noise dimension that animates the field). `seed` salts the gradient
/// hash. For [`FractalType::Turbulent`] the absolute value of each octave is
/// summed (ridged / billowy); otherwise the signed octaves are summed (smooth).
/// The result is normalized by the total amplitude so it stays in a stable range
/// (~`[-1,1]` basic, ~`[0,1]` turbulent) regardless of octave count / persistence.
#[allow(clippy::too_many_arguments)]
fn fbm(
    x: f32,
    y: f32,
    z: f32,
    seed: u32,
    octaves: u32,
    persistence: f32,
    lacunarity: f32,
    fractal_type: FractalType,
) -> f32 {
    let mut freq = 1.0f32;
    let mut amp = 1.0f32;
    let mut sum = 0.0f32;
    let mut norm = 0.0f32;
    for o in 0..octaves {
        // Salt each octave's hash so octaves are independent fields (not just a
        // scaled copy of octave 0).
        let oseed = seed.wrapping_add(o.wrapping_mul(0x9E37_79B9));
        let n = gradient_noise_3d(x * freq, y * freq, z * freq, oseed);
        let shaped = match fractal_type {
            FractalType::Basic => n,
            FractalType::Turbulent => n.abs(),
        };
        sum += shaped * amp;
        norm += amp;
        freq *= lacunarity;
        amp *= persistence;
    }
    if norm <= 0.0 {
        return 0.0;
    }
    sum / norm
}

/// 3-D value-gradient noise at `(x, y, z)`, seeded by `seed`, in roughly
/// `[-1, 1]`.
///
/// This is Perlin-style **gradient** noise: at each integer lattice corner a
/// pseudo-random gradient vector (derived by hashing the corner + seed) is dotted
/// with the offset to the sample point, and the eight corner contributions are
/// smoothly (quintic-fade) interpolated. Because the gradients come from a stable
/// integer hash of `(corner, seed)`, the field is **fully deterministic** — the
/// same `(x, y, z, seed)` always yields the same value — and continuous, so it
/// flows smoothly as `z` (evolution) sweeps.
pub fn gradient_noise_3d(x: f32, y: f32, z: f32, seed: u32) -> f32 {
    let xi = x.floor();
    let yi = y.floor();
    let zi = z.floor();
    let xf = x - xi;
    let yf = y - yi;
    let zf = z - zi;
    let (ix, iy, iz) = (xi as i32, yi as i32, zi as i32);

    let u = fade(xf);
    let v = fade(yf);
    let w = fade(zf);

    // Corner gradient · offset for each of the 8 lattice corners.
    let g = |cx: i32, cy: i32, cz: i32, fx: f32, fy: f32, fz: f32| {
        grad(hash3(ix + cx, iy + cy, iz + cz, seed), fx, fy, fz)
    };
    let n000 = g(0, 0, 0, xf, yf, zf);
    let n100 = g(1, 0, 0, xf - 1.0, yf, zf);
    let n010 = g(0, 1, 0, xf, yf - 1.0, zf);
    let n110 = g(1, 1, 0, xf - 1.0, yf - 1.0, zf);
    let n001 = g(0, 0, 1, xf, yf, zf - 1.0);
    let n101 = g(1, 0, 1, xf - 1.0, yf, zf - 1.0);
    let n011 = g(0, 1, 1, xf, yf - 1.0, zf - 1.0);
    let n111 = g(1, 1, 1, xf - 1.0, yf - 1.0, zf - 1.0);

    // Trilinear interpolation with the faded weights.
    let nx00 = lerp(n000, n100, u);
    let nx10 = lerp(n010, n110, u);
    let nx01 = lerp(n001, n101, u);
    let nx11 = lerp(n011, n111, u);
    let nxy0 = lerp(nx00, nx10, v);
    let nxy1 = lerp(nx01, nx11, v);
    lerp(nxy0, nxy1, w)
}

/// Quintic fade curve `6t⁵ − 15t⁴ + 10t³` (Perlin's improved-noise smoothstep):
/// zero first/second derivative at the ends, so octaves tile without creases.
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Linear interpolation.
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Pick one of 16 evenly-spread gradient directions from a hash and dot it with
/// the offset `(x, y, z)` — Perlin's improved-noise gradient selection.
fn grad(hash: u32, x: f32, y: f32, z: f32) -> f32 {
    // Ken Perlin's improved-noise gradient set (12 edge vectors of a cube,
    // reused to fill 16 hash buckets).
    match hash & 15 {
        0 => x + y,
        1 => -x + y,
        2 => x - y,
        3 => -x - y,
        4 => x + z,
        5 => -x + z,
        6 => x - z,
        7 => -x - z,
        8 => y + z,
        9 => -y + z,
        10 => y - z,
        11 => -y - z,
        12 => y + x,
        13 => -y + z,
        14 => y - x,
        _ => -y - z,
    }
}

/// A stable, well-mixed integer hash of an integer lattice corder `(x, y, z)` +
/// `seed`, via SplitMix64 (the same hash family `wiggle` seeds from). Pure — the
/// same inputs always give the same hash, so the noise field is deterministic.
fn hash3(x: i32, y: i32, z: i32, seed: u32) -> u32 {
    let mut h = (x as u32 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= (y as u32 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= (z as u32 as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
    h ^= (seed as u64).wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    (splitmix64(h) & 0xFFFF_FFFF) as u32
}

/// A fast, well-mixed 64-bit integer hash (SplitMix64) — turns the packed lattice
/// corner + seed into a well-distributed gradient bucket.
fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A default Fractal Noise for tweaking in tests.
    fn fractal() -> GenerateEffect {
        GenerateEffect::defaults()[0]
    }

    /// Replace the named fields of a generate effect (terse test helper).
    fn with(mut e: GenerateEffect, f: impl FnOnce(&mut GenerateEffect)) -> GenerateEffect {
        f(&mut e);
        e
    }

    fn approx(a: [f32; 4], b: [f32; 4], eps: f32) -> bool {
        a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() <= eps)
    }

    #[test]
    fn labels_and_defaults() {
        let d = GenerateEffect::defaults();
        assert_eq!(d.len(), 5);
        assert_eq!(d[0].label(), "Fractal Noise");
        assert_eq!(d[1].label(), "Gradient Ramp");
        assert_eq!(d[2].label(), "Checkerboard");
        assert_eq!(d[3].label(), "4-Color Gradient");
        assert_eq!(d[4].label(), "Grid");
    }

    #[test]
    fn produces_color_only_for_color_generators() {
        let d = GenerateEffect::defaults();
        assert!(!d[0].produces_color(), "fractal noise is grayscale-linear");
        for e in &d[1..] {
            assert!(e.produces_color(), "{} is a colour generator", e.label());
        }
    }

    // --- Fractal Noise ------------------------------------------------------

    #[test]
    fn noise_is_deterministic_across_calls() {
        // Same (params, pixel) → same value, every call. This is the whole point:
        // a frame must render identically for the cache / multi-frame render.
        let e = fractal();
        for &(x, y) in &[(0.0, 0.0), (13.0, -7.0), (200.0, 130.0), (-50.5, 88.25)] {
            let a = e.value_at(x, y);
            let b = e.value_at(x, y);
            assert_eq!(a, b, "noise must be deterministic at ({x},{y})");
        }
    }

    #[test]
    fn gradient_noise_is_deterministic_and_in_range() {
        for &(x, y, z) in &[(0.3, 0.7, 0.0), (10.1, -3.4, 2.2), (-100.0, 50.0, 9.9)] {
            let a = gradient_noise_3d(x, y, z, 0);
            let b = gradient_noise_3d(x, y, z, 0);
            assert_eq!(a, b, "gradient noise must be deterministic");
            assert!(a.abs() <= 1.5, "gradient noise roughly bounded, got {a}");
        }
    }

    #[test]
    fn value_is_in_unit_range_when_clipped() {
        let e = fractal();
        for i in 0..200 {
            let x = (i as f32) * 3.7 - 100.0;
            let y = (i as f32) * -2.1 + 40.0;
            let v = e.value_at(x, y);
            assert!((0.0..=1.0).contains(&v), "clipped value out of range: {v}");
        }
    }

    #[test]
    fn evolution_changes_the_field() {
        // Sweeping evolution must move the field — at least one sampled pixel
        // changes meaningfully (the key motion-design knob).
        let a = fractal();
        let b = with(fractal(), |e| {
            if let GenerateEffect::FractalNoise { evolution, .. } = e {
                *evolution = 5.0;
            }
        });
        let mut max_diff = 0.0f32;
        for i in 0..64 {
            let x = i as f32 * 5.0;
            let y = i as f32 * 3.0;
            max_diff = max_diff.max((a.value_at(x, y) - b.value_at(x, y)).abs());
        }
        assert!(max_diff > 0.05, "evolution should change the field, max diff {max_diff}");
    }

    #[test]
    fn seed_changes_the_field() {
        let a = fractal();
        let b = with(fractal(), |e| {
            if let GenerateEffect::FractalNoise { seed, .. } = e {
                *seed = 12345;
            }
        });
        let mut max_diff = 0.0f32;
        for i in 0..64 {
            let x = i as f32 * 5.0;
            let y = i as f32 * 3.0;
            max_diff = max_diff.max((a.value_at(x, y) - b.value_at(x, y)).abs());
        }
        assert!(max_diff > 0.05, "seed should change the field, max diff {max_diff}");
    }

    #[test]
    fn turbulent_differs_from_basic() {
        // Same seed/scale/evolution, just the fractal type flipped, must give a
        // visibly different field (abs-sum vs signed-sum).
        let basic = with(fractal(), |e| {
            if let GenerateEffect::FractalNoise { fractal_type, .. } = e {
                *fractal_type = FractalType::Basic;
            }
        });
        let turb = with(fractal(), |e| {
            if let GenerateEffect::FractalNoise { fractal_type, .. } = e {
                *fractal_type = FractalType::Turbulent;
            }
        });
        let mut max_diff = 0.0f32;
        for i in 0..64 {
            let x = i as f32 * 4.0 + 1.0;
            let y = i as f32 * 2.0 - 3.0;
            max_diff = max_diff.max((basic.value_at(x, y) - turb.value_at(x, y)).abs());
        }
        assert!(max_diff > 0.05, "turbulent should differ from basic, max diff {max_diff}");
    }

    #[test]
    fn turbulent_is_nonnegative_before_contrast() {
        // The raw turbulent fbm is an abs-sum, so it is ≥ 0. Sample fbm directly
        // (value_at adds contrast/brightness which could push it negative).
        for i in 0..50 {
            let x = i as f32 * 0.37;
            let y = i as f32 * -0.21;
            let n = fbm(x, y, 0.0, 0, 6, 0.6, 2.0, FractalType::Turbulent);
            assert!(n >= 0.0, "turbulent fbm must be non-negative, got {n}");
        }
    }

    #[test]
    fn complexity_adds_detail() {
        // More octaves should change the field (finer detail), not be a no-op.
        let low = with(fractal(), |e| {
            if let GenerateEffect::FractalNoise { complexity, .. } = e {
                *complexity = 1;
            }
        });
        let high = with(fractal(), |e| {
            if let GenerateEffect::FractalNoise { complexity, .. } = e {
                *complexity = 8;
            }
        });
        let mut max_diff = 0.0f32;
        for i in 0..64 {
            let x = i as f32 * 2.5;
            let y = i as f32 * 1.5;
            max_diff = max_diff.max((low.value_at(x, y) - high.value_at(x, y)).abs());
        }
        assert!(max_diff > 0.02, "complexity should add detail, max diff {max_diff}");
    }

    #[test]
    fn single_octave_ignores_persistence_and_scaling() {
        // With one octave there is nothing for persistence/lacunarity to act on,
        // so they must not change the result.
        let base = with(fractal(), |e| {
            if let GenerateEffect::FractalNoise { complexity, .. } = e {
                *complexity = 1;
            }
        });
        let tweaked = with(base, |e| {
            if let GenerateEffect::FractalNoise {
                sub_influence,
                sub_scaling,
                ..
            } = e
            {
                *sub_influence = 0.1;
                *sub_scaling = 4.0;
            }
        });
        for i in 0..32 {
            let x = i as f32 * 6.0;
            let y = i as f32 * 4.0;
            assert!(
                (base.value_at(x, y) - tweaked.value_at(x, y)).abs() < 1e-5,
                "one octave should ignore sub-influence/scaling"
            );
        }
    }

    #[test]
    fn contrast_pushes_away_from_mid_grey() {
        // High contrast pushes values away from 0.5; sample a pixel that isn't
        // exactly mid-grey and confirm the deviation grows.
        let flat = with(fractal(), |e| {
            if let GenerateEffect::FractalNoise {
                contrast,
                overflow,
                ..
            } = e
            {
                *contrast = 1.0;
                *overflow = Overflow::AllowHdr; // don't clip so we can see the push
            }
        });
        let punchy = with(flat, |e| {
            if let GenerateEffect::FractalNoise { contrast, .. } = e {
                *contrast = 3.0;
            }
        });
        // Find a pixel whose flat value is clearly off mid-grey.
        let (mut fx, mut fy) = (0.0f32, 0.0f32);
        let mut found = false;
        for i in 0..200 {
            let x = i as f32 * 3.3;
            let y = i as f32 * 1.7;
            if (flat.value_at(x, y) - 0.5).abs() > 0.05 {
                fx = x;
                fy = y;
                found = true;
                break;
            }
        }
        assert!(found, "expected an off-mid-grey pixel");
        let flat_dev = (flat.value_at(fx, fy) - 0.5).abs();
        let punchy_dev = (punchy.value_at(fx, fy) - 0.5).abs();
        assert!(
            punchy_dev > flat_dev,
            "higher contrast should push further from mid-grey ({punchy_dev} vs {flat_dev})"
        );
    }

    #[test]
    fn brightness_lifts_the_field() {
        let dark = with(fractal(), |e| {
            if let GenerateEffect::FractalNoise {
                brightness,
                overflow,
                ..
            } = e
            {
                *brightness = 0.0;
                *overflow = Overflow::AllowHdr;
            }
        });
        let bright = with(dark, |e| {
            if let GenerateEffect::FractalNoise { brightness, .. } = e {
                *brightness = 0.3;
            }
        });
        for i in 0..32 {
            let x = i as f32 * 5.0;
            let y = i as f32 * 3.0;
            assert!(
                (bright.value_at(x, y) - dark.value_at(x, y) - 0.3).abs() < 1e-4,
                "brightness should lift the field by its offset"
            );
        }
    }

    #[test]
    fn overflow_modes_bring_value_into_range() {
        assert_eq!(Overflow::Clip.apply(1.5), 1.0);
        assert_eq!(Overflow::Clip.apply(-0.3), 0.0);
        assert_eq!(Overflow::Clip.apply(0.4), 0.4);
        // Wrap takes the fractional part.
        assert!((Overflow::Wrap.apply(1.25) - 0.25).abs() < 1e-6);
        assert!((Overflow::Wrap.apply(-0.25) - 0.75).abs() < 1e-6);
        // AllowHdr keeps values above 1 but floors at 0.
        assert_eq!(Overflow::AllowHdr.apply(2.0), 2.0);
        assert_eq!(Overflow::AllowHdr.apply(-1.0), 0.0);
    }

    #[test]
    fn scale_changes_feature_size() {
        // A different scale samples the field at a different frequency, so the
        // value at a fixed pixel changes.
        let small = with(fractal(), |e| {
            if let GenerateEffect::FractalNoise { scale, .. } = e {
                *scale = 20.0;
            }
        });
        let large = with(fractal(), |e| {
            if let GenerateEffect::FractalNoise { scale, .. } = e {
                *scale = 200.0;
            }
        });
        let mut max_diff = 0.0f32;
        for i in 0..64 {
            let x = i as f32 * 4.0;
            let y = i as f32 * 4.0;
            max_diff = max_diff.max((small.value_at(x, y) - large.value_at(x, y)).abs());
        }
        assert!(max_diff > 0.05, "scale should change feature size, max diff {max_diff}");
    }

    #[test]
    fn zero_scale_does_not_panic() {
        // A degenerate zero scale must be guarded (no div-by-zero / NaN).
        let e = with(fractal(), |e| {
            if let GenerateEffect::FractalNoise {
                scale,
                scale_x,
                scale_y,
                ..
            } = e
            {
                *scale = 0.0;
                *scale_x = 0.0;
                *scale_y = 0.0;
            }
        });
        let v = e.value_at(10.0, 20.0);
        assert!(v.is_finite(), "zero scale must not produce NaN/inf");
    }

    #[test]
    fn opacity_is_clamped() {
        for d in GenerateEffect::defaults() {
            let e = with(d, |x| match x {
                GenerateEffect::FractalNoise { opacity, .. }
                | GenerateEffect::Ramp { opacity, .. }
                | GenerateEffect::Checkerboard { opacity, .. }
                | GenerateEffect::FourColorGradient { opacity, .. }
                | GenerateEffect::Grid { opacity, .. } => *opacity = 2.0,
            });
            assert_eq!(e.opacity(), 1.0, "{} opacity clamps", e.label());
        }
    }

    #[test]
    fn serde_round_trips_every_generator() {
        for e in GenerateEffect::defaults() {
            let json = serde_json::to_string(&e).unwrap();
            let back: GenerateEffect = serde_json::from_str(&json).unwrap();
            assert_eq!(e, back, "{} serde round-trip", e.label());
        }
    }

    // --- Gradient / Ramp ----------------------------------------------------

    /// A linear ramp from black at y=-100 to white at y=+100 (vertical).
    fn linear_ramp() -> GenerateEffect {
        GenerateEffect::Ramp {
            shape: RampShape::Linear,
            start: [0.0, -100.0],
            end: [0.0, 100.0],
            radius: 100.0,
            start_color: [0.0, 0.0, 0.0],
            end_color: [1.0, 1.0, 1.0],
            scatter: 0.0,
            opacity: 1.0,
        }
    }

    #[test]
    fn linear_ramp_endpoints_and_midpoint() {
        let r = linear_ramp();
        // At the start point: start_color (black).
        assert!(approx(r.rgba_at(0.0, -100.0, 200.0, 200.0), [0.0, 0.0, 0.0, 1.0], 1e-4));
        // At the end point: end_color (white).
        assert!(approx(r.rgba_at(0.0, 100.0, 200.0, 200.0), [1.0, 1.0, 1.0, 1.0], 1e-4));
        // Midpoint: mid-grey.
        let mid = r.rgba_at(0.0, 0.0, 200.0, 200.0);
        assert!(approx(mid, [0.5, 0.5, 0.5, 1.0], 1e-4), "midpoint grey, got {mid:?}");
    }

    #[test]
    fn linear_ramp_clamps_past_the_endpoints() {
        let r = linear_ramp();
        // Past the white end stays white (clamped, not extrapolated).
        assert!(approx(r.rgba_at(0.0, 500.0, 200.0, 200.0), [1.0, 1.0, 1.0, 1.0], 1e-4));
        // Before the black end stays black.
        assert!(approx(r.rgba_at(0.0, -500.0, 200.0, 200.0), [0.0, 0.0, 0.0, 1.0], 1e-4));
    }

    #[test]
    fn linear_ramp_is_constant_perpendicular_to_axis() {
        // A vertical ramp is constant along x.
        let r = linear_ramp();
        let a = r.rgba_at(-80.0, 0.0, 200.0, 200.0);
        let b = r.rgba_at(80.0, 0.0, 200.0, 200.0);
        assert!(approx(a, b, 1e-5), "constant across the perpendicular axis");
    }

    #[test]
    fn radial_ramp_centre_and_edge() {
        let r = GenerateEffect::Ramp {
            shape: RampShape::Radial,
            start: [0.0, 0.0],
            end: [0.0, 0.0],
            radius: 100.0,
            start_color: [0.0, 0.0, 0.0],
            end_color: [1.0, 1.0, 1.0],
            scatter: 0.0,
            opacity: 1.0,
        };
        // Centre = start_color.
        assert!(approx(r.rgba_at(0.0, 0.0, 200.0, 200.0), [0.0, 0.0, 0.0, 1.0], 1e-4));
        // At the radius (along +x) = end_color.
        assert!(approx(r.rgba_at(100.0, 0.0, 200.0, 200.0), [1.0, 1.0, 1.0, 1.0], 1e-4));
        // Halfway out = mid-grey, and isotropic (same in any direction).
        let half_x = r.rgba_at(50.0, 0.0, 200.0, 200.0);
        let half_y = r.rgba_at(0.0, 50.0, 200.0, 200.0);
        assert!(approx(half_x, [0.5, 0.5, 0.5, 1.0], 1e-4), "radial midpoint grey");
        assert!(approx(half_x, half_y, 1e-5), "radial ramp is isotropic");
    }

    #[test]
    fn degenerate_linear_ramp_does_not_nan() {
        // start == end → zero-length axis, must not divide by zero.
        let r = GenerateEffect::Ramp {
            shape: RampShape::Linear,
            start: [10.0, 10.0],
            end: [10.0, 10.0],
            radius: 100.0,
            start_color: [0.2, 0.4, 0.6],
            end_color: [0.8, 0.6, 0.4],
            scatter: 0.0,
            opacity: 1.0,
        };
        let v = r.rgba_at(50.0, 50.0, 200.0, 200.0);
        assert!(v.iter().all(|c| c.is_finite()), "degenerate ramp finite, got {v:?}");
    }

    #[test]
    fn ramp_scatter_dithers_deterministically() {
        let mut r = linear_ramp();
        if let GenerateEffect::Ramp { scatter, .. } = &mut r {
            *scatter = 0.4;
        }
        // Deterministic: the same pixel always gives the same dithered value.
        let a = r.rgba_at(13.0, 7.0, 200.0, 200.0);
        let b = r.rgba_at(13.0, 7.0, 200.0, 200.0);
        assert_eq!(a, b, "scatter must be deterministic per pixel");
        // And it actually perturbs vs the clean ramp at some pixels.
        let clean = linear_ramp();
        let mut diff = 0.0f32;
        for i in 0..64 {
            let x = i as f32 * 3.0;
            let y = i as f32 * 2.0 - 50.0;
            diff = diff.max((r.rgba_at(x, y, 200.0, 200.0)[1] - clean.rgba_at(x, y, 200.0, 200.0)[1]).abs());
        }
        assert!(diff > 0.01, "scatter should perturb the ramp, max diff {diff}");
    }

    // --- Checkerboard -------------------------------------------------------

    fn checker() -> GenerateEffect {
        GenerateEffect::Checkerboard {
            anchor: [0.0, 0.0],
            size_w: 50.0,
            size_h: 50.0,
            color1: [0.0, 0.0, 0.0],
            color2: [1.0, 1.0, 1.0],
            opacity: 1.0,
        }
    }

    #[test]
    fn checkerboard_cell_parity() {
        let c = checker();
        // Cell (0,0): even parity → color1 (black). Sample its interior.
        assert!(approx(c.rgba_at(25.0, 25.0, 200.0, 200.0), [0.0, 0.0, 0.0, 1.0], 1e-5));
        // Cell (1,0): odd parity → color2 (white).
        assert!(approx(c.rgba_at(75.0, 25.0, 200.0, 200.0), [1.0, 1.0, 1.0, 1.0], 1e-5));
        // Cell (0,1): odd parity → color2 (white).
        assert!(approx(c.rgba_at(25.0, 75.0, 200.0, 200.0), [1.0, 1.0, 1.0, 1.0], 1e-5));
        // Cell (1,1): even parity → color1 (black).
        assert!(approx(c.rgba_at(75.0, 75.0, 200.0, 200.0), [0.0, 0.0, 0.0, 1.0], 1e-5));
    }

    #[test]
    fn checkerboard_negative_cells_keep_parity() {
        // rem_euclid keeps the chequer continuous across the origin.
        let c = checker();
        // Cell (-1,0): odd → white.
        assert!(approx(c.rgba_at(-25.0, 25.0, 200.0, 200.0), [1.0, 1.0, 1.0, 1.0], 1e-5));
        // Cell (-1,-1): even → black.
        assert!(approx(c.rgba_at(-25.0, -25.0, 200.0, 200.0), [0.0, 0.0, 0.0, 1.0], 1e-5));
    }

    #[test]
    fn checkerboard_anchor_shifts_the_grid() {
        let mut c = checker();
        if let GenerateEffect::Checkerboard { anchor, .. } = &mut c {
            *anchor = [50.0, 0.0]; // shift one cell right
        }
        // The pixel that was cell (0,0) black is now cell (-1,0) odd → white.
        assert!(approx(c.rgba_at(25.0, 25.0, 200.0, 200.0), [1.0, 1.0, 1.0, 1.0], 1e-5));
    }

    #[test]
    fn checkerboard_zero_size_does_not_panic() {
        let c = GenerateEffect::Checkerboard {
            anchor: [0.0, 0.0],
            size_w: 0.0,
            size_h: 0.0,
            color1: [0.2, 0.2, 0.2],
            color2: [0.8, 0.8, 0.8],
            opacity: 1.0,
        };
        let v = c.rgba_at(10.0, 20.0, 200.0, 200.0);
        assert!(v.iter().all(|x| x.is_finite()));
    }

    // --- 4-Color Gradient ---------------------------------------------------

    fn four_color() -> GenerateEffect {
        GenerateEffect::FourColorGradient {
            tl: [1.0, 0.0, 0.0],
            tr: [0.0, 1.0, 0.0],
            bl: [0.0, 0.0, 1.0],
            br: [1.0, 1.0, 0.0],
            blend: 1.0,
            jitter: 0.0,
            opacity: 1.0,
        }
    }

    #[test]
    fn four_color_corner_values() {
        let g = four_color();
        let (hw, hh) = (100.0, 100.0);
        // Top-left corner (lx=-hw, ly=-hh) → tl (red).
        assert!(approx(g.rgba_at(-hw, -hh, hw, hh), [1.0, 0.0, 0.0, 1.0], 1e-4));
        // Top-right (lx=+hw, ly=-hh) → tr (green).
        assert!(approx(g.rgba_at(hw, -hh, hw, hh), [0.0, 1.0, 0.0, 1.0], 1e-4));
        // Bottom-left (lx=-hw, ly=+hh) → bl (blue).
        assert!(approx(g.rgba_at(-hw, hh, hw, hh), [0.0, 0.0, 1.0, 1.0], 1e-4));
        // Bottom-right (lx=+hw, ly=+hh) → br (yellow).
        assert!(approx(g.rgba_at(hw, hh, hw, hh), [1.0, 1.0, 0.0, 1.0], 1e-4));
    }

    #[test]
    fn four_color_interior_blend() {
        let g = four_color();
        let (hw, hh) = (100.0, 100.0);
        // Centre = average of the four corners.
        let c = g.rgba_at(0.0, 0.0, hw, hh);
        let avg = [
            (1.0 + 0.0 + 0.0 + 1.0) / 4.0,
            (0.0 + 1.0 + 0.0 + 1.0) / 4.0,
            (0.0 + 0.0 + 1.0 + 0.0) / 4.0,
        ];
        assert!(approx(c, [avg[0], avg[1], avg[2], 1.0], 1e-4), "centre is the average, got {c:?}");
        // Top edge midpoint = average of tl & tr.
        let top = g.rgba_at(0.0, -hh, hw, hh);
        assert!(approx(top, [0.5, 0.5, 0.0, 1.0], 1e-4), "top edge blends tl/tr, got {top:?}");
    }

    #[test]
    fn four_color_jitter_is_deterministic_and_perturbs() {
        let mut g = four_color();
        if let GenerateEffect::FourColorGradient { jitter, .. } = &mut g {
            *jitter = 0.3;
        }
        let a = g.rgba_at(11.0, 23.0, 100.0, 100.0);
        let b = g.rgba_at(11.0, 23.0, 100.0, 100.0);
        assert_eq!(a, b, "jitter deterministic per pixel");
        let clean = four_color();
        let mut diff = 0.0f32;
        for i in 0..64 {
            let x = i as f32 * 2.0 - 60.0;
            let y = i as f32 * 1.5 - 40.0;
            diff = diff.max((g.rgba_at(x, y, 100.0, 100.0)[0] - clean.rgba_at(x, y, 100.0, 100.0)[0]).abs());
        }
        assert!(diff > 0.005, "jitter should perturb the blend, max diff {diff}");
    }

    // --- Grid ---------------------------------------------------------------

    fn grid() -> GenerateEffect {
        GenerateEffect::Grid {
            anchor: [0.0, 0.0],
            size_w: 50.0,
            size_h: 50.0,
            border: 4.0,
            color: [1.0, 1.0, 1.0],
            background: [0.0, 0.0, 0.0],
            background_opacity: 0.0,
            opacity: 1.0,
        }
    }

    #[test]
    fn grid_line_vs_cell_pixels() {
        let g = grid();
        // On a vertical line (x near a multiple of 50): opaque white line.
        let on = g.rgba_at(0.0, 25.0, 200.0, 200.0);
        assert!(approx(on, [1.0, 1.0, 1.0, 1.0], 1e-5), "on a grid line, got {on:?}");
        // Cell interior (far from any line): transparent background.
        let off = g.rgba_at(25.0, 25.0, 200.0, 200.0);
        assert_eq!(off[3], 0.0, "cell interior is transparent");
    }

    #[test]
    fn grid_horizontal_and_corner_lines() {
        let g = grid();
        // On a horizontal line.
        assert!(approx(g.rgba_at(25.0, 50.0, 200.0, 200.0), [1.0, 1.0, 1.0, 1.0], 1e-5));
        // On a grid intersection (both lines).
        assert!(approx(g.rgba_at(0.0, 0.0, 200.0, 200.0), [1.0, 1.0, 1.0, 1.0], 1e-5));
    }

    #[test]
    fn grid_filled_background_is_opaque() {
        let mut g = grid();
        if let GenerateEffect::Grid {
            background_opacity, ..
        } = &mut g
        {
            *background_opacity = 1.0;
        }
        let off = g.rgba_at(25.0, 25.0, 200.0, 200.0);
        assert_eq!(off[3], 1.0, "filled background is opaque");
        assert!(approx(off, [0.0, 0.0, 0.0, 1.0], 1e-5));
    }

    #[test]
    fn grid_thicker_border_covers_more() {
        let thin = grid();
        let thick = with(grid(), |e| {
            if let GenerateEffect::Grid { border, .. } = e {
                *border = 20.0;
            }
        });
        // A pixel 8 px from a line: off for the thin border, on for the thick.
        assert_eq!(thin.rgba_at(8.0, 25.0, 200.0, 200.0)[3], 0.0);
        assert_eq!(thick.rgba_at(8.0, 25.0, 200.0, 200.0)[3], 1.0);
    }

    #[test]
    fn grid_zero_size_does_not_panic() {
        let g = GenerateEffect::Grid {
            anchor: [0.0, 0.0],
            size_w: 0.0,
            size_h: 0.0,
            border: 2.0,
            color: [1.0, 1.0, 1.0],
            background: [0.0, 0.0, 0.0],
            background_opacity: 0.0,
            opacity: 1.0,
        };
        let v = g.rgba_at(10.0, 20.0, 200.0, 200.0);
        assert!(v.iter().all(|x| x.is_finite()));
    }

    #[test]
    fn bias_is_identity_at_one_and_fixes_ends() {
        assert!((bias(0.3, 1.0) - 0.3).abs() < 1e-6);
        assert!((bias(0.0, 2.0) - 0.0).abs() < 1e-6);
        assert!((bias(1.0, 2.0) - 1.0).abs() < 1e-6);
        assert!((bias(0.5, 2.0) - 0.5).abs() < 1e-6, "0.5 is a fixed point");
        // Sharper (>1) pushes a below-mid value lower.
        assert!(bias(0.3, 2.0) < 0.3);
    }

    #[test]
    fn color_generators_are_deterministic() {
        for e in &GenerateEffect::defaults()[1..] {
            for &(x, y) in &[(0.0, 0.0), (33.0, -17.0), (-90.0, 120.0)] {
                let a = e.rgba_at(x, y, 100.0, 100.0);
                let b = e.rgba_at(x, y, 100.0, 100.0);
                assert_eq!(a, b, "{} must be deterministic", e.label());
            }
        }
    }
}
