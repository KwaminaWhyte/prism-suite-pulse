//! Keying (whole-buffer alpha-affecting) effects — the After-Effects *Keying*
//! category.
//!
//! Where a [`SpatialEffect`](super::SpatialEffect) *convolves / blooms / offsets*
//! a layer's rendered buffer and a [`DistortEffect`](super::DistortEffect)
//! *re-maps its coordinates*, a **key** effect *carves the layer's alpha* (its
//! matte): it decides, per pixel, how transparent that pixel becomes, by testing
//! the pixel's colour against a key colour / luminance / chroma. These are the
//! After-Effects compositing workhorses that pull a foreground off a coloured
//! backing:
//! - **Color Key** — key out a target colour within an RGB-distance tolerance,
//!   with an edge-softness band (AE's *Color Key*).
//! - **Luma Key** — key on luminance: drop pixels above or below a threshold,
//!   with a softness band (AE's *Luma Key*).
//! - **Chroma Key** (Keylight-style) — key a chroma colour by **YCbCr** distance
//!   to the key colour, with matte **gain** (how hard the falloff bites) and
//!   **balance** (the green↔blue chroma weighting) and edge **softness** (AE's
//!   *Keylight*-style chroma keyer).
//! - **Spill Suppression** — neutralise the key-colour spill that bleeds onto
//!   retained edges, by pulling the dominant key channel back toward the other
//!   two (AE's *Spill Suppressor* / Keylight's despill).
//! - **Matte Choke** — erode / dilate the alpha (a min/max morphology by a
//!   pixel radius) plus **clip black / clip white** matte levels that crush the
//!   low/high alpha back to fully-transparent / fully-opaque (AE's *Matte
//!   Choker* / *Simple Choker* + Keylight's clip controls).
//!
//! Every pass works on the compositor's **premultiplied, linear-light** RGBA
//! buffer (`colour · coverage` in RGB, `coverage` in A) in row-major order — the
//! same representation the spatial / distort passes use. A keyer needs the
//! pixel's *straight* colour to test it, so each pass **un-premultiplies** before
//! testing and **re-premultiplies** the result (multiplying the existing colour
//! by the new coverage), which keeps soft / transparent edges clean. The key
//! colours are authored straight **sRGB** (the swatch the user picks) and decoded
//! to linear at the pass boundary so the distance test happens in the same
//! linear-light space the buffer lives in. All passes are pure (no GPU, no time,
//! no IO) so the matte math is unit-testable; they'll migrate to the suite's
//! `prism-fx` host when that lands.

use prism_core::color::srgb_to_linear;
use serde::{Deserialize, Serialize};

/// A **key** (whole-buffer alpha-affecting) effect in a layer's effect stack.
/// Each variant rewrites the layer's coverage (alpha) from a per-pixel colour
/// test; [`KeyEffect::SpillSuppression`] additionally neutralises RGB.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum KeyEffect {
    /// **Color Key**: drop pixels whose colour is within `tolerance` (RGB
    /// Euclidean distance, linear-light) of `key` (straight sRGB). `softness`
    /// extends the band beyond the tolerance over which alpha ramps from 0 back
    /// to 1, feathering the matte edge. A pixel exactly on the key colour is
    /// fully keyed (alpha 0); past `tolerance + softness` it is fully kept.
    ColorKey {
        /// Key colour, straight sRGB `0..=1`.
        key: [f32; 3],
        /// RGB-distance tolerance (linear-light); within it, fully keyed.
        tolerance: f32,
        /// Feather band past the tolerance over which alpha ramps back to 1.
        softness: f32,
    },
    /// **Luma Key**: key on the pixel's Rec.709 luminance. When `key_high`,
    /// pixels *brighter* than `threshold` are dropped (key out highlights);
    /// otherwise pixels *darker* than `threshold` are dropped (key out shadows).
    /// `softness` is the luminance band over which alpha ramps, feathering the
    /// edge.
    LumaKey {
        /// Luminance pivot `0..=1`.
        threshold: f32,
        /// Feather band (luminance) over which alpha ramps.
        softness: f32,
        /// Key out the bright side (`true`) or the dark side (`false`).
        key_high: bool,
    },
    /// **Chroma Key** (Keylight-style): key a chroma colour by **YCbCr** distance
    /// (the chroma plane only — luminance is ignored, so shading on the backing
    /// doesn't break the key) to `key` (straight sRGB). `gain` scales how hard the
    /// falloff bites (higher = a harder matte); `balance` weights the Cb↔Cr axes
    /// (`0.5` even, `<0.5` favours blue-screen, `>0.5` green-screen); `softness`
    /// is the chroma-distance band past the core over which alpha ramps back to 1.
    ChromaKey {
        /// Key chroma colour, straight sRGB `0..=1`.
        key: [f32; 3],
        /// Matte gain — falloff steepness (higher bites harder). `1` = neutral.
        gain: f32,
        /// Cb↔Cr weighting, `0..=1` (`0.5` even).
        balance: f32,
        /// Chroma-distance feather band over which alpha ramps back to 1.
        softness: f32,
    },
    /// **Spill Suppression**: neutralise the key colour bleeding onto retained
    /// edges. The dominant key channel (per the `key` colour's max component) is
    /// pulled back toward the average of the other two by `amount` (`0` = off,
    /// `1` = fully neutralised), de-fringing green/blue spill without touching
    /// the alpha.
    SpillSuppression {
        /// Key colour whose dominant channel is the spill to suppress (sRGB).
        key: [f32; 3],
        /// Suppression strength `0..=1`.
        amount: f32,
    },
    /// **Matte Choke**: erode (negative `choke`) or dilate (positive `choke`) the
    /// alpha by a pixel radius — a morphological min/max over a square window —
    /// then crush the matte with **clip black** (alpha ≤ this → 0) and **clip
    /// white** (alpha ≥ this → 1) levels, the rest rescaled between them. Erode
    /// tightens a matte that's too loose; dilate recovers a matte eaten too far;
    /// the clip levels harden a soft matte's tails.
    MatteChoke {
        /// Erode (`<0`) / dilate (`>0`) radius in pixels. `0` = no morphology.
        choke: f32,
        /// Alpha at/below this clips to 0 (`0..=1`).
        clip_black: f32,
        /// Alpha at/above this clips to 1 (`0..=1`).
        clip_white: f32,
    },
}

impl KeyEffect {
    /// A short, stable label for the UI and the "add effect" menu.
    pub fn label(&self) -> &'static str {
        match self {
            KeyEffect::ColorKey { .. } => "Color Key",
            KeyEffect::LumaKey { .. } => "Luma Key",
            KeyEffect::ChromaKey { .. } => "Chroma Key",
            KeyEffect::SpillSuppression { .. } => "Spill Suppression",
            KeyEffect::MatteChoke { .. } => "Matte Choke",
        }
    }

    /// A fresh, sensibly-defaulted instance of each key effect, for the "add
    /// effect" menu. The colour keyers seed a **green-screen** key colour and a
    /// small tolerance so adding one immediately keys a typical green backing;
    /// Matte Choke seeds an identity (no choke, full clip range) so it is a no-op
    /// until a parameter is touched.
    pub fn defaults() -> [KeyEffect; 5] {
        // A canonical green-screen swatch (straight sRGB), shared by the keyers.
        const GREEN: [f32; 3] = [0.0, 0.6, 0.1];
        [
            KeyEffect::ColorKey {
                key: GREEN,
                tolerance: 0.15,
                softness: 0.1,
            },
            KeyEffect::LumaKey {
                threshold: 0.5,
                softness: 0.1,
                key_high: false,
            },
            KeyEffect::ChromaKey {
                key: GREEN,
                gain: 1.0,
                balance: 0.5,
                softness: 0.1,
            },
            KeyEffect::SpillSuppression {
                key: GREEN,
                amount: 1.0,
            },
            KeyEffect::MatteChoke {
                choke: 0.0,
                clip_black: 0.0,
                clip_white: 1.0,
            },
        ]
    }

    /// Apply this effect to a premultiplied linear-light RGBA buffer in place.
    ///
    /// `buf` is `width × height` row-major premultiplied RGBA. The colour keyers
    /// and spill suppressor are pointwise (each pixel un-premultiplied, tested,
    /// re-premultiplied); Matte Choke reads a neighbourhood, so it snapshots the
    /// alpha before morphing.
    pub fn apply(&self, buf: &mut [[f32; 4]], width: usize, height: usize) {
        if width == 0 || height == 0 || buf.len() < width * height {
            return;
        }
        match *self {
            KeyEffect::ColorKey {
                key,
                tolerance,
                softness,
            } => {
                let key = to_linear(key);
                pointwise_alpha(buf, |straight| {
                    color_key_alpha(straight, key, tolerance.max(0.0), softness.max(0.0))
                });
            }
            KeyEffect::LumaKey {
                threshold,
                softness,
                key_high,
            } => {
                pointwise_alpha(buf, |straight| {
                    luma_key_alpha(straight, threshold, softness.max(0.0), key_high)
                });
            }
            KeyEffect::ChromaKey {
                key,
                gain,
                balance,
                softness,
            } => {
                let key = to_linear(key);
                let (kcb, kcr) = chroma(key);
                pointwise_alpha(buf, |straight| {
                    chroma_key_alpha(
                        straight,
                        kcb,
                        kcr,
                        gain.max(1e-3),
                        balance.clamp(0.0, 1.0),
                        softness.max(0.0),
                    )
                });
            }
            KeyEffect::SpillSuppression { key, amount } => {
                let dom = dominant_channel(key);
                spill_suppress(buf, dom, amount.clamp(0.0, 1.0));
            }
            KeyEffect::MatteChoke {
                choke,
                clip_black,
                clip_white,
            } => {
                matte_choke(buf, width, height, choke, clip_black, clip_white);
            }
        }
    }
}

/// Apply an ordered **key** effect stack to a premultiplied linear-light RGBA
/// buffer in place.
pub fn apply_key_effects(effects: &[KeyEffect], buf: &mut [[f32; 4]], width: usize, height: usize) {
    for e in effects {
        e.apply(buf, width, height);
    }
}

/// Decode a straight sRGB colour to linear-light (the key colours are authored
/// in sRGB to match the swatch the user picks).
fn to_linear(c: [f32; 3]) -> [f32; 3] {
    [
        srgb_to_linear(c[0].clamp(0.0, 1.0)),
        srgb_to_linear(c[1].clamp(0.0, 1.0)),
        srgb_to_linear(c[2].clamp(0.0, 1.0)),
    ]
}

/// Run a pointwise alpha-rewriting keyer over a premultiplied buffer.
///
/// For each pixel: un-premultiply to straight colour, call `f` to get the
/// pixel's *new coverage in `[0,1]`*, then re-premultiply by scaling the
/// **existing** premultiplied colour by `new_alpha / old_alpha` and setting the
/// alpha. Fully-transparent pixels are skipped (nothing to key). Multiplying the
/// existing premultiplied RGB by the coverage *ratio* keeps the straight colour
/// unchanged while the coverage changes — exactly a matte edit.
fn pointwise_alpha(buf: &mut [[f32; 4]], f: impl Fn([f32; 3]) -> f32) {
    for px in buf.iter_mut() {
        let a = px[3];
        if a <= 0.0 {
            continue;
        }
        let straight = [px[0] / a, px[1] / a, px[2] / a];
        let new_a = f(straight).clamp(0.0, 1.0);
        let ratio = new_a / a;
        px[0] *= ratio;
        px[1] *= ratio;
        px[2] *= ratio;
        px[3] = new_a;
    }
}

/// Color-key coverage: 0 within `tolerance` of `key` (RGB Euclidean distance),
/// ramping smoothly back to 1 over the `softness` band past it.
fn color_key_alpha(c: [f32; 3], key: [f32; 3], tolerance: f32, softness: f32) -> f32 {
    let dr = c[0] - key[0];
    let dg = c[1] - key[1];
    let db = c[2] - key[2];
    let dist = (dr * dr + dg * dg + db * db).sqrt();
    smoothstep(tolerance, tolerance + softness, dist)
}

/// Luma-key coverage: drop the bright side (`key_high`) above `threshold`, or
/// the dark side below it, ramping over `softness`.
fn luma_key_alpha(c: [f32; 3], threshold: f32, softness: f32, key_high: bool) -> f32 {
    let l = luma(c);
    if key_high {
        // Bright pixels keyed: kept below threshold, ramps to 0 above it.
        1.0 - smoothstep(threshold, threshold + softness, l)
    } else {
        // Dark pixels keyed: 0 below (threshold - softness), kept above threshold.
        smoothstep(threshold - softness, threshold, l)
    }
}

/// Chroma-key coverage from the YCbCr chroma distance to the key chroma.
///
/// The chroma distance is the (balance-weighted) Euclidean distance in the
/// (Cb, Cr) plane; `gain` scales it so a higher gain bites the falloff harder.
/// Pixels at the key chroma read 0 (fully keyed); past the `softness` band they
/// read 1 (fully kept). Luminance is ignored, so shading on the backing colour
/// doesn't pull holes in the matte.
fn chroma_key_alpha(c: [f32; 3], kcb: f32, kcr: f32, gain: f32, balance: f32, softness: f32) -> f32 {
    let (cb, cr) = chroma(c);
    let dcb = (cb - kcb) * (1.0 - balance) * 2.0;
    let dcr = (cr - kcr) * balance * 2.0;
    let dist = (dcb * dcb + dcr * dcr).sqrt() * gain;
    // Core radius scales inversely with gain so gain widens the keyed region;
    // softness feathers the edge. The 0.15 core is a sensible chroma radius for
    // a saturated backing in linear light.
    let core = 0.15;
    smoothstep(core, core + softness.max(1e-4), dist)
}

/// Spill suppression: pull the `dominant` key channel back toward the average of
/// the other two by `amount`, neutralising the colour cast, leaving alpha alone.
fn spill_suppress(buf: &mut [[f32; 4]], dominant: usize, amount: f32) {
    if amount <= 0.0 {
        return;
    }
    for px in buf.iter_mut() {
        let a = px[3];
        if a <= 0.0 {
            continue;
        }
        // Un-premultiply, suppress, re-premultiply (alpha unchanged).
        let mut c = [px[0] / a, px[1] / a, px[2] / a];
        let (o1, o2) = match dominant {
            0 => (1, 2),
            1 => (0, 2),
            _ => (0, 1),
        };
        let others = (c[o1] + c[o2]) * 0.5;
        if c[dominant] > others {
            c[dominant] = c[dominant] + (others - c[dominant]) * amount;
        }
        px[0] = c[0] * a;
        px[1] = c[1] * a;
        px[2] = c[2] * a;
    }
}

/// Matte choke: erode / dilate the alpha by `choke` px (morphological min/max
/// over a square window), then apply clip-black / clip-white levels.
fn matte_choke(
    buf: &mut [[f32; 4]],
    width: usize,
    height: usize,
    choke: f32,
    clip_black: f32,
    clip_white: f32,
) {
    let radius = choke.abs().round() as i32;
    let erode = choke < 0.0;
    // Snapshot the straight alpha; morph reads neighbours from the snapshot.
    let src_a: Vec<f32> = buf.iter().map(|p| p[3]).collect();
    let cb = clip_black.clamp(0.0, 1.0);
    let cw = clip_white.clamp(0.0, 1.0);
    let span = (cw - cb).max(1e-4);
    let (w, h) = (width as i32, height as i32);

    for y in 0..height {
        for x in 0..width {
            let i = y * width + x;
            // Morphology: min (erode) / max (dilate) of the alpha in the window.
            let mut a = if radius > 0 {
                let mut acc = if erode { 1.0f32 } else { 0.0f32 };
                for dy in -radius..=radius {
                    for dx in -radius..=radius {
                        let sx = x as i32 + dx;
                        let sy = y as i32 + dy;
                        // Off-buffer reads as fully transparent (0) so a matte at
                        // the frame edge erodes/dilates against empty space.
                        let s = if sx < 0 || sx >= w || sy < 0 || sy >= h {
                            0.0
                        } else {
                            src_a[sy as usize * width + sx as usize]
                        };
                        acc = if erode { acc.min(s) } else { acc.max(s) };
                    }
                }
                acc
            } else {
                src_a[i]
            };
            // Clip levels: crush the tails, rescale the middle to [0,1].
            if cw > cb {
                a = ((a - cb) / span).clamp(0.0, 1.0);
            } else {
                // Degenerate (clip_white <= clip_black): hard threshold at cb.
                a = if a >= cb { 1.0 } else { 0.0 };
            }
            // Re-premultiply: scale the existing colour by the coverage ratio.
            let old = buf[i][3];
            if old > 0.0 {
                let ratio = a / old;
                buf[i][0] *= ratio;
                buf[i][1] *= ratio;
                buf[i][2] *= ratio;
            }
            // If the pixel was transparent but dilation/clip raised its alpha,
            // there is no colour to recover, so it stays black (premultiplied
            // RGB already 0) — a dilated matte fringe reads as the layer's edge
            // colour only where colour existed, matching AE's choke behaviour.
            buf[i][3] = a;
        }
    }
}

/// Rec.709 luminance of a linear-light colour, clamped to `[0,1]`.
fn luma(c: [f32; 3]) -> f32 {
    (0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]).clamp(0.0, 1.0)
}

/// The (Cb, Cr) chroma of a linear-light colour (Rec.601-style chroma axes,
/// centred at 0 — the luminance term is dropped, only the chroma plane matters).
fn chroma(c: [f32; 3]) -> (f32, f32) {
    let y = 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2];
    let cb = c[2] - y; // blue-difference
    let cr = c[0] - y; // red-difference
    (cb, cr)
}

/// The index of the dominant (max) channel of a colour — the spill channel.
fn dominant_channel(c: [f32; 3]) -> usize {
    if c[1] >= c[0] && c[1] >= c[2] {
        1
    } else if c[2] >= c[0] && c[2] >= c[1] {
        2
    } else {
        0
    }
}

/// The classic `smoothstep` (re-declared locally to keep the keyer self-contained
/// and matching [`super::effect::smoothstep`]): 0 below `e0`, 1 above `e1`, a
/// smooth Hermite ramp between; `e0 >= e1` degenerates to a hard step at `e0`.
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    if e1 <= e0 {
        return if x < e0 { 0.0 } else { 1.0 };
    }
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
