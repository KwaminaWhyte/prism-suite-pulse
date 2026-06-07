//! Generate (whole-buffer fill) effects: **Fractal Noise** — the motion-design
//! workhorse.
//!
//! Unlike [`Effect`](super::Effect) (a per-pixel colour-correction pass that
//! *reads* the layer's pixels) or [`SpatialEffect`](super::SpatialEffect) (a
//! convolve / bloom / offset pass that *filters* them), a **generate** effect
//! *replaces* the layer's pixels: it synthesises content from its parameters and
//! the pixel position, filling the layer's quad. This mirrors After Effects'
//! *Generate* category, whose flagship is **Fractal Noise** — multi-octave
//! gradient noise driving smoke, clouds, energy, organic textures, mattes, and
//! displacement maps.
//!
//! The noise is **deterministic**: the same `(params, evolution, seed, pixel)`
//! always produces the same value — it is hash-seeded gradient noise (the same
//! SplitMix64-hash philosophy the `wiggle` expression uses), never `rand` / system
//! entropy / `Math.random`. That determinism is non-negotiable: a frame must
//! render identically on every pass (for the RAM-preview cache, multi-frame
//! render, and golden-frame tests), and **evolution** (a phase/time input) is the
//! only thing that moves the field — so animating evolution gives the signature
//! flowing-noise motion while a still frame stays bit-stable.
//!
//! The field is evaluated in the layer's **local** frame (comp px, origin at the
//! layer centre) so it rides the layer's transform — `scale` zooms the noise,
//! the layer's position/rotation move it — and is written into the compositor's
//! **premultiplied, linear-light** isolated buffer (`color · coverage` in RGB,
//! `coverage` in A). Pure (no GPU, no IO, no `Track` sampling here — the
//! evolution/scale values are sampled by the caller and passed in), so the noise
//! math is unit-testable; it'll migrate to the suite's `prism-fx` host when that
//! lands.

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

/// A **generate** (whole-buffer fill) effect in a layer's effect stack.
///
/// Currently the one member is [`GenerateEffect::FractalNoise`] — After Effects'
/// motion-design workhorse. A layer carries at most one generate fill (an
/// `Option<GenerateEffect>`): like AE's Fractal Noise it *replaces* the layer's
/// content rather than stacking, so two fills would just override each other.
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
}

impl GenerateEffect {
    /// A short, stable label for the UI and the "add effect" menu.
    pub fn label(&self) -> &'static str {
        match self {
            GenerateEffect::FractalNoise { .. } => "Fractal Noise",
        }
    }

    /// A fresh, sensibly-defaulted instance of each generate effect, for the
    /// "add effect" menu. The default gives a recognizable, mid-scale cloud field
    /// so adding it reads immediately.
    pub fn defaults() -> [GenerateEffect; 1] {
        [GenerateEffect::FractalNoise {
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
        }]
    }

    /// Sample the generated **straight grayscale value** at a layer-local pixel
    /// position `(lx, ly)` (comp px, origin at the layer centre). Returns the
    /// noise value brought into range by [`Overflow`] in `[0,1]` (or `≥0` for
    /// `AllowHdr`). Pure and deterministic in `(self, lx, ly)`.
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
        }
    }

    /// The output **opacity** this generate fill scales coverage by.
    pub fn opacity(&self) -> f32 {
        match *self {
            GenerateEffect::FractalNoise { opacity, .. } => opacity.clamp(0.0, 1.0),
        }
    }
}

/// The maximum octave count (complexity) the fractal sum honours — keeps the
/// per-pixel cost bounded.
pub const MAX_OCTAVES: u32 = 10;

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
    // `GenerateEffect` has a single variant today, so destructuring it with
    // `if let` is irrefutable; the helpers keep the `if let` form so they stay
    // correct when more generate variants are added.
    #![allow(irrefutable_let_patterns)]
    use super::*;

    /// A default Fractal Noise for tweaking in tests.
    fn fractal() -> GenerateEffect {
        GenerateEffect::defaults()[0]
    }

    /// Replace the named fields of a Fractal Noise (terse test helper).
    fn with(mut e: GenerateEffect, f: impl FnOnce(&mut GenerateEffect)) -> GenerateEffect {
        f(&mut e);
        e
    }

    #[test]
    fn label_and_defaults() {
        let d = GenerateEffect::defaults();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].label(), "Fractal Noise");
    }

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
        let e = with(fractal(), |e| {
            if let GenerateEffect::FractalNoise { opacity, .. } = e {
                *opacity = 2.0;
            }
        });
        assert_eq!(e.opacity(), 1.0);
    }

    #[test]
    fn serde_round_trips() {
        let e = fractal();
        let json = serde_json::to_string(&e).unwrap();
        let back: GenerateEffect = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }
}
