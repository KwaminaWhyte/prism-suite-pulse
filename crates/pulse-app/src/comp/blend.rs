//! Per-layer **blend modes**: the function that decides how a layer's pixels
//! combine with the composite beneath them.
//!
//! Pulse's compositor is a CPU software renderer, so — unlike Pigment, which
//! runs the suite's 18 blend modes as a WGSL pass — the blend math lives here,
//! in pure Rust, working on the same **straight, linear-light** RGBA the
//! [`render`](crate::render) accumulator carries (color un-premultiplied, alpha
//! as coverage). The mode set and its stable numeric ids are reused from
//! [`prism_core::BlendMode`] (the suite's shared enum) so a layer's blend mode
//! round-trips by the same id Pigment writes to disk; only the per-pixel
//! evaluation is Pulse-side.
//!
//! The compositing model is the W3C "blending and compositing" formula used
//! across the suite: the blend function `B(cb, cs)` mixes the *backdrop* and
//! *source* straight colors, the result is interpolated toward the plain source
//! by the backdrop's alpha (so blending only happens where there is a backdrop),
//! and that blended color is finally composited **source-over**. The separable
//! formulas (Multiply … LinearBurn) match Pigment's `composite.wgsl`
//! `blend_fn`; the four non-separable HSL modes (Hue / Saturation / Color /
//! Luminosity) use the same `set_lum` / `set_sat` / `clip_color` constructions.
//!
//! Blending is done in **linear light** — the space the accumulator already
//! holds — matching Pulse's "never bake until output" / linear-light
//! source-over compositing (After Effects' linearized / 32-bpc project mode).

use serde::{Deserialize, Serialize};

pub use prism_core::BlendMode;

/// A straight (non-premultiplied) linear-light RGBA pixel the blend math
/// operates on: `r,g,b` are the color, `a` is coverage in `[0, 1]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlendRgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

/// Composite straight, linear-light `src` over straight `dst` using `mode`.
///
/// Implements the W3C blend+composite model: the source color is first blended
/// with the backdrop by `mode`'s blend function `B`, the blended color is
/// interpolated toward the plain source by the backdrop alpha
/// (`(1 - dst.a)·src + dst.a·B`) so the blend only takes hold where a backdrop
/// exists, and the result is then composited **source-over**. For
/// [`BlendMode::Normal`] the blend function is the identity, so this reduces
/// *exactly* to plain source-over — the renderer's previous behavior — keeping
/// pre-blend-mode projects byte-identical.
pub fn blend_over(mode: BlendMode, src: BlendRgba, dst: BlendRgba) -> BlendRgba {
    let sa = src.a.clamp(0.0, 1.0);
    let da = dst.a.clamp(0.0, 1.0);
    // Fast path: Normal is plain source-over; skip the blend machinery so the
    // common case stays cheap and bit-exact with the old `over`.
    if matches!(mode, BlendMode::Normal) {
        return source_over(src, dst, sa, da);
    }
    let b = blend_fn(mode, [dst.r, dst.g, dst.b], [src.r, src.g, src.b]);
    // W3C: mixed = (1 - da)·Cs + da·B(Cb, Cs). Where there is no backdrop the
    // plain source shows; where it is opaque the full blend shows.
    let mixed = [
        (1.0 - da) * src.r + da * b[0],
        (1.0 - da) * src.g + da * b[1],
        (1.0 - da) * src.b + da * b[2],
    ];
    source_over(
        BlendRgba {
            r: mixed[0],
            g: mixed[1],
            b: mixed[2],
            a: sa,
        },
        dst,
        sa,
        da,
    )
}

/// Straight-RGBA source-over: `out = src·sa + dst·da·(1 - sa)`, normalized back
/// to straight color by the resulting alpha. Both inputs are straight; `sa`/`da`
/// are their clamped coverages.
fn source_over(src: BlendRgba, dst: BlendRgba, sa: f32, da: f32) -> BlendRgba {
    let out_a = sa + da * (1.0 - sa);
    if out_a <= 0.0 {
        return BlendRgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        };
    }
    // Premultiplied accumulate, then un-premultiply by the output alpha.
    let inv = 1.0 - sa;
    let r = (src.r * sa + dst.r * da * inv) / out_a;
    let g = (src.g * sa + dst.g * da * inv) / out_a;
    let b = (src.b * sa + dst.b * da * inv) / out_a;
    BlendRgba { r, g, b, a: out_a }
}

/// Rec.709 luminance of a linear-light color (matches the WGSL `lum`).
fn lum(c: [f32; 3]) -> f32 {
    0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}

/// Clip a color into `[0, 1]` while preserving its luminance (W3C `ClipColor`).
fn clip_color(c: [f32; 3]) -> [f32; 3] {
    let l = lum(c);
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    let mut col = c;
    if n < 0.0 {
        let d = l - n;
        let s = if d.abs() < 1e-6 { 0.0 } else { l / d };
        col = [
            l + (col[0] - l) * s,
            l + (col[1] - l) * s,
            l + (col[2] - l) * s,
        ];
    }
    if x > 1.0 {
        let d = x - l;
        let s = if d.abs() < 1e-6 { 0.0 } else { (1.0 - l) / d };
        col = [
            l + (col[0] - l) * s,
            l + (col[1] - l) * s,
            l + (col[2] - l) * s,
        ];
    }
    col
}

/// Set a color's luminance to `l`, then clip (W3C `SetLum`).
fn set_lum(c: [f32; 3], l: f32) -> [f32; 3] {
    let d = l - lum(c);
    clip_color([c[0] + d, c[1] + d, c[2] + d])
}

/// Saturation = max channel − min channel (W3C `Sat`).
fn sat(c: [f32; 3]) -> f32 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}

/// Stretch a color to the target saturation `s`, anchoring its min to 0 (W3C
/// `SetSat`). A flat color (max == min) becomes black.
fn set_sat(c: [f32; 3], s: f32) -> [f32; 3] {
    let mn = c[0].min(c[1]).min(c[2]);
    let mx = c[0].max(c[1]).max(c[2]);
    if mx > mn {
        let k = s / (mx - mn);
        [(c[0] - mn) * k, (c[1] - mn) * k, (c[2] - mn) * k]
    } else {
        [0.0, 0.0, 0.0]
    }
}

/// The separable per-channel `hard_light`/`overlay` shape, shared by Overlay
/// (driven by the backdrop) and Hard Light (driven by the source).
fn hard_light_channel(b: f32, s: f32) -> f32 {
    if s <= 0.5 {
        2.0 * b * s
    } else {
        1.0 - 2.0 * (1.0 - b) * (1.0 - s)
    }
}

/// The blend function `B(backdrop, source)` for one mode, on straight
/// linear-light colors. Separable modes apply per channel; the four HSL modes
/// are non-separable. [`BlendMode::Normal`] returns the source unchanged.
///
/// Mirrors Pigment's `composite.wgsl` `blend_fn` (same numeric cases) so the
/// suite shares one definition of each mode.
fn blend_fn(mode: BlendMode, b: [f32; 3], s: [f32; 3]) -> [f32; 3] {
    // Per-channel separable application helper.
    let sep = |f: &dyn Fn(f32, f32) -> f32| [f(b[0], s[0]), f(b[1], s[1]), f(b[2], s[2])];
    match mode {
        BlendMode::Normal => s,
        BlendMode::Multiply => sep(&|b, s| b * s),
        BlendMode::Screen => sep(&|b, s| b + s - b * s),
        BlendMode::Overlay => sep(&|b, s| hard_light_channel(s, b)),
        BlendMode::Darken => sep(&|b, s| b.min(s)),
        BlendMode::Lighten => sep(&|b, s| b.max(s)),
        BlendMode::ColorDodge => sep(&|b, s| {
            if b <= 0.0 {
                0.0
            } else if s >= 1.0 {
                1.0
            } else {
                (b / (1.0 - s)).min(1.0)
            }
        }),
        BlendMode::ColorBurn => sep(&|b, s| {
            if b >= 1.0 {
                1.0
            } else if s <= 0.0 {
                0.0
            } else {
                1.0 - ((1.0 - b) / s).min(1.0)
            }
        }),
        BlendMode::HardLight => sep(&|b, s| hard_light_channel(b, s)),
        BlendMode::SoftLight => sep(&|b, s| {
            let d = if b <= 0.25 {
                ((16.0 * b - 12.0) * b + 4.0) * b
            } else {
                b.sqrt()
            };
            if s <= 0.5 {
                b - (1.0 - 2.0 * s) * b * (1.0 - b)
            } else {
                b + (2.0 * s - 1.0) * (d - b)
            }
        }),
        BlendMode::Difference => sep(&|b, s| (b - s).abs()),
        BlendMode::Exclusion => sep(&|b, s| b + s - 2.0 * b * s),
        BlendMode::LinearDodge => sep(&|b, s| (b + s).min(1.0)),
        BlendMode::LinearBurn => sep(&|b, s| (b + s - 1.0).max(0.0)),
        BlendMode::Hue => set_lum(set_sat(s, sat(b)), lum(b)),
        BlendMode::Saturation => set_lum(set_sat(b, sat(s)), lum(b)),
        BlendMode::Color => set_lum(s, lum(b)),
        BlendMode::Luminosity => set_lum(b, lum(s)),
    }
}

/// A short label for one blend mode, for the picker UI (After-Effects naming,
/// with "Add" noted for Linear Dodge).
pub fn blend_label(mode: BlendMode) -> &'static str {
    match mode {
        BlendMode::Normal => "Normal",
        BlendMode::Multiply => "Multiply",
        BlendMode::Screen => "Screen",
        BlendMode::Overlay => "Overlay",
        BlendMode::Darken => "Darken",
        BlendMode::Lighten => "Lighten",
        BlendMode::ColorDodge => "Color Dodge",
        BlendMode::ColorBurn => "Color Burn",
        BlendMode::HardLight => "Hard Light",
        BlendMode::SoftLight => "Soft Light",
        BlendMode::Difference => "Difference",
        BlendMode::Exclusion => "Exclusion",
        BlendMode::LinearDodge => "Linear Dodge (Add)",
        BlendMode::LinearBurn => "Linear Burn",
        BlendMode::Hue => "Hue",
        BlendMode::Saturation => "Saturation",
        BlendMode::Color => "Color",
        BlendMode::Luminosity => "Luminosity",
    }
}

/// A newtype carrying the blend mode on a layer so `#[serde(default)]` can
/// supply [`BlendMode::Normal`] without an orphan-impl on the foreign enum.
/// Transparent over [`BlendMode`], so it (de)serializes identically.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LayerBlend(pub BlendMode);

impl Default for LayerBlend {
    fn default() -> Self {
        LayerBlend(BlendMode::Normal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(r: f32, g: f32, b: f32, a: f32) -> BlendRgba {
        BlendRgba { r, g, b, a }
    }

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn normal_is_plain_source_over() {
        // Half-coverage white over opaque black -> straight gray 0.5, full alpha.
        let out = blend_over(
            BlendMode::Normal,
            px(1.0, 1.0, 1.0, 0.5),
            px(0.0, 0.0, 0.0, 1.0),
        );
        assert!(approx(out.a, 1.0));
        assert!(approx(out.r, 0.5), "got {}", out.r);
    }

    #[test]
    fn normal_matches_classic_over() {
        // Independently compute straight source-over and compare.
        let src = px(0.8, 0.2, 0.4, 0.6);
        let dst = px(0.1, 0.5, 0.9, 0.7);
        let out = blend_over(BlendMode::Normal, src, dst);
        let ia = 1.0 - src.a;
        let exp_a = src.a + dst.a * ia;
        let exp_r = (src.r * src.a + dst.r * dst.a * ia) / exp_a;
        assert!(approx(out.a, exp_a));
        assert!(approx(out.r, exp_r), "{} vs {}", out.r, exp_r);
    }

    #[test]
    fn multiply_darkens_over_opaque_backdrop() {
        // Opaque source over opaque backdrop: result is the pure blend B = b*s.
        let out = blend_over(
            BlendMode::Multiply,
            px(0.5, 0.5, 0.5, 1.0),
            px(0.4, 0.4, 0.4, 1.0),
        );
        assert!(approx(out.a, 1.0));
        assert!(approx(out.r, 0.2), "0.5*0.4=0.2, got {}", out.r);
    }

    #[test]
    fn screen_lightens() {
        // Screen: b + s - b*s = 0.5 + 0.5 - 0.25 = 0.75 over an opaque backdrop.
        let out = blend_over(
            BlendMode::Screen,
            px(0.5, 0.5, 0.5, 1.0),
            px(0.5, 0.5, 0.5, 1.0),
        );
        assert!(approx(out.r, 0.75), "got {}", out.r);
    }

    #[test]
    fn add_clamps_to_one() {
        // Linear Dodge (Add): 0.7 + 0.6 clamps to 1.0.
        let out = blend_over(
            BlendMode::LinearDodge,
            px(0.6, 0.6, 0.6, 1.0),
            px(0.7, 0.7, 0.7, 1.0),
        );
        assert!(approx(out.r, 1.0), "got {}", out.r);
    }

    #[test]
    fn difference_is_abs_delta() {
        let out = blend_over(
            BlendMode::Difference,
            px(0.2, 0.2, 0.2, 1.0),
            px(0.9, 0.9, 0.9, 1.0),
        );
        assert!(approx(out.r, 0.7), "|0.9-0.2|=0.7, got {}", out.r);
    }

    #[test]
    fn blend_has_no_effect_where_backdrop_is_empty() {
        // With a transparent backdrop, every mode reduces to the plain source
        // (W3C: blend weighted by backdrop alpha = 0).
        let src = px(0.3, 0.6, 0.9, 1.0);
        let empty = px(0.0, 0.0, 0.0, 0.0);
        for &m in &prism_core::BlendMode::ALL {
            let out = blend_over(m, src, empty);
            assert!(approx(out.a, 1.0));
            assert!(approx(out.r, src.r), "mode {m:?} changed color over empty");
            assert!(approx(out.g, src.g));
            assert!(approx(out.b, src.b));
        }
    }

    #[test]
    fn fully_transparent_source_leaves_backdrop() {
        let dst = px(0.2, 0.4, 0.6, 1.0);
        for &m in &prism_core::BlendMode::ALL {
            let out = blend_over(m, px(1.0, 1.0, 1.0, 0.0), dst);
            assert!(approx(out.a, dst.a), "mode {m:?}");
            assert!(approx(out.r, dst.r), "mode {m:?} changed backdrop r");
        }
    }

    #[test]
    fn luminosity_keeps_backdrop_hue() {
        // Luminosity takes the backdrop's color but the source's brightness:
        // a gray source over a saturated red backdrop stays reddish (r dominant).
        let out = blend_over(
            BlendMode::Luminosity,
            px(0.8, 0.8, 0.8, 1.0),
            px(0.6, 0.1, 0.1, 1.0),
        );
        assert!(out.r > out.g && out.r > out.b, "stays red: {out:?}");
    }

    #[test]
    fn color_takes_source_hue() {
        // Color: source hue/sat, backdrop luma. A blue source over a gray
        // backdrop should read blue-dominant.
        let out = blend_over(
            BlendMode::Color,
            px(0.1, 0.1, 0.8, 1.0),
            px(0.5, 0.5, 0.5, 1.0),
        );
        assert!(out.b > out.r && out.b > out.g, "takes blue hue: {out:?}");
    }

    #[test]
    fn layer_blend_defaults_to_normal() {
        assert_eq!(LayerBlend::default().0, BlendMode::Normal);
    }
}
