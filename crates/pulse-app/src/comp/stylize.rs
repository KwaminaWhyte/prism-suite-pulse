//! Stylize (whole-buffer look-shaping) effects — the After-Effects *Stylize*
//! category.
//!
//! Where a [`SpatialEffect`](super::SpatialEffect) *convolves / blooms / offsets*
//! a layer's rendered buffer, a [`DistortEffect`](super::DistortEffect) *re-maps
//! its coordinates*, and a [`KeyEffect`](super::KeyEffect) *carves its alpha*, a
//! **stylize** effect *reshapes the layer's look* — it reads the neighbourhood of
//! each pixel and rewrites the colour into a graphic, non-photoreal treatment.
//! These are the After-Effects look-design staples that the per-pixel grade and
//! the blur passes cannot express:
//! - **Find Edges** — Sobel edge-detection: replace the buffer with the gradient
//!   magnitude of its colour, **inverted** like After Effects so flat regions go
//!   white and edges go dark (the "ink outline" look). An `amount` scales the
//!   edge response and an `invert` flag flips back to dark-on-bright edges.
//! - **Mosaic** — pixelate into `horizontal × vertical` blocks, each filled with
//!   the average colour of the pixels it covers (AE's *Mosaic*).
//!
//! Every pass works on the compositor's **premultiplied, linear-light** RGBA
//! buffer (`colour · coverage` in RGB, `coverage` in A) in row-major order — the
//! same representation the spatial / distort / key passes use. Find Edges needs
//! the pixel's *straight* colour to detect edges in the layer's actual colour (not
//! its coverage-faded premultiplied value), so it **un-premultiplies** before the
//! Sobel and **re-premultiplies** the result by the original coverage (so soft /
//! transparent edges stay clean and the alpha matte is preserved). Mosaic averages
//! the **premultiplied** values directly (the correct way to average colours with
//! transparency: a half-covered bright pixel contributes half its colour), so a
//! block over a soft edge blends colour and coverage together. All passes are pure
//! (no GPU, no time, no IO) so the math is unit-testable; they'll migrate to the
//! suite's `prism-fx` host when that lands.

use serde::{Deserialize, Serialize};

/// A **stylize** (whole-buffer look-shaping) effect in a layer's effect stack.
///
/// Unlike [`Effect`](super::Effect) (a per-pixel colour-correction pass), a
/// stylize effect reads neighbouring pixels — it detects edges or pools blocks —
/// so it operates on the layer's *isolated* RGBA buffer rather than one pixel at a
/// time, like the spatial / distort / key families.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum StylizeEffect {
    /// **Find Edges** (After Effects' *Find Edges*): a Sobel gradient-magnitude
    /// edge detector. Each pixel's RGB is replaced by the magnitude of the colour
    /// gradient there, then **inverted** so flat regions read white and edges read
    /// dark (the AE default "ink outline" look). `amount` scales the edge
    /// response (1 = unit Sobel, larger = punchier edges); `invert` flips back to
    /// the un-inverted bright-edges-on-black look. Alpha (the layer's matte) is
    /// preserved.
    FindEdges {
        /// Edge-response gain: scales the Sobel magnitude before inversion
        /// (`1` = unit response, larger = stronger/darker edges).
        amount: f32,
        /// Invert the result: `false` (default) is AE's white-background /
        /// dark-edges look; `true` keeps bright edges on a black background.
        invert: bool,
    },
    /// **Mosaic** (After Effects' *Mosaic*): pixelate the buffer into a grid of
    /// `horizontal × vertical` blocks, each filled with the average colour of the
    /// pixels it covers. Larger counts give finer blocks; `1 × 1` collapses the
    /// whole buffer to its average colour.
    Mosaic {
        /// Number of blocks across the buffer **width** (clamped `>= 1`).
        horizontal: u32,
        /// Number of blocks down the buffer **height** (clamped `>= 1`).
        vertical: u32,
    },
}

impl StylizeEffect {
    /// A short, stable label for the UI and the "add effect" menu.
    pub fn label(&self) -> &'static str {
        match self {
            StylizeEffect::FindEdges { .. } => "Find Edges",
            StylizeEffect::Mosaic { .. } => "Mosaic",
        }
    }

    /// A fresh, sensibly-defaulted instance of each stylize effect, for the "add
    /// effect" menu / browser. Defaults give a visible-but-tasteful result so
    /// adding one reads immediately. This array's order is the `default_index` the
    /// effect registry addresses, so the two must stay in sync (the
    /// `registry_indices_match_defaults` test guards this).
    pub fn defaults() -> [StylizeEffect; 2] {
        [
            StylizeEffect::FindEdges {
                amount: 1.0,
                invert: false,
            },
            StylizeEffect::Mosaic {
                horizontal: 24,
                vertical: 24,
            },
        ]
    }

    /// Apply this effect to a premultiplied linear-light RGBA buffer in place.
    ///
    /// `buf` is `width × height` row-major premultiplied RGBA. The pass reads the
    /// whole buffer and writes the result back into it.
    pub fn apply(&self, buf: &mut [[f32; 4]], width: usize, height: usize) {
        if width == 0 || height == 0 || buf.len() < width * height {
            return;
        }
        match *self {
            StylizeEffect::FindEdges { amount, invert } => {
                find_edges(buf, width, height, amount.max(0.0), invert);
            }
            StylizeEffect::Mosaic {
                horizontal,
                vertical,
            } => {
                mosaic(buf, width, height, horizontal.max(1), vertical.max(1));
            }
        }
    }
}

/// Apply an ordered **stylize** effect stack to a premultiplied linear-light RGBA
/// buffer in place.
pub fn apply_stylize_effects(
    effects: &[StylizeEffect],
    buf: &mut [[f32; 4]],
    width: usize,
    height: usize,
) {
    for e in effects {
        e.apply(buf, width, height);
    }
}

/// **Find Edges** pass: Sobel gradient magnitude on the buffer's straight colour,
/// inverted (white background, dark edges) like After Effects.
///
/// Each pixel is un-premultiplied to its straight RGB; the 3×3 Sobel kernels are
/// run per channel (edges off-buffer read as the clamped edge pixel, so the frame
/// border isn't itself a giant edge); the per-channel magnitudes are scaled by
/// `amount` and clamped to `[0,1]`; then (unless `invert`) `1 - magnitude` is
/// taken so flat regions are white and edges are dark. The result is
/// re-premultiplied by the **original** coverage, so the layer's alpha matte is
/// untouched and soft / transparent edges stay clean.
fn find_edges(buf: &mut [[f32; 4]], width: usize, height: usize, amount: f32, invert: bool) {
    let (w, h) = (width as i32, height as i32);
    // Straight-colour source, so the Sobel sees the layer's actual colour rather
    // than its coverage-faded premultiplied value (a half-covered edge would
    // otherwise read as a colour gradient that isn't really there).
    let src = buf[..width * height].to_vec();
    let unpremul = |p: [f32; 4]| -> [f32; 3] {
        let a = p[3];
        if a > 0.0 {
            [p[0] / a, p[1] / a, p[2] / a]
        } else {
            [0.0, 0.0, 0.0]
        }
    };
    // Clamp off-buffer samples to the edge pixel so the frame border doesn't read
    // as a hard edge (matching the blur passes' "repeat edge" convention).
    let fetch = |x: i32, y: i32| -> [f32; 3] {
        let cx = x.clamp(0, w - 1) as usize;
        let cy = y.clamp(0, h - 1) as usize;
        unpremul(src[cy * width + cx])
    };

    for y in 0..height {
        for x in 0..width {
            let (xi, yi) = (x as i32, y as i32);
            // 3×3 neighbourhood, straight colour per corner / edge.
            let tl = fetch(xi - 1, yi - 1);
            let tc = fetch(xi, yi - 1);
            let tr = fetch(xi + 1, yi - 1);
            let ml = fetch(xi - 1, yi);
            let mr = fetch(xi + 1, yi);
            let bl = fetch(xi - 1, yi + 1);
            let bc = fetch(xi, yi + 1);
            let br = fetch(xi + 1, yi + 1);
            let mut out = [0.0f32; 3];
            for k in 0..3 {
                // Standard Sobel: gx weights the horizontal neighbours, gy the
                // vertical; magnitude = hypot(gx, gy).
                let gx = (tr[k] + 2.0 * mr[k] + br[k]) - (tl[k] + 2.0 * ml[k] + bl[k]);
                let gy = (bl[k] + 2.0 * bc[k] + br[k]) - (tl[k] + 2.0 * tc[k] + tr[k]);
                let mag = (gx * gx + gy * gy).sqrt() * amount;
                let edge = mag.clamp(0.0, 1.0);
                out[k] = if invert { edge } else { 1.0 - edge };
            }
            // Re-premultiply by the original coverage (alpha is preserved).
            let a = src[y * width + x][3];
            buf[y * width + x] = [out[0] * a, out[1] * a, out[2] * a, a];
        }
    }
}

/// **Mosaic** pass: pixelate the buffer into `horizontal × vertical` blocks, each
/// filled with the average colour of the pixels it covers.
///
/// The buffer is partitioned into a grid of (at least 1×1) blocks; each block's
/// **premultiplied** RGBA average is computed (the correct way to pool colours with
/// transparency) and written back to every pixel in the block. Block boundaries
/// are computed in floating point and floored, so an uneven division spreads the
/// remainder pixels across blocks rather than dropping them.
fn mosaic(buf: &mut [[f32; 4]], width: usize, height: usize, horizontal: u32, vertical: u32) {
    let cols = (horizontal as usize).min(width.max(1));
    let rows = (vertical as usize).min(height.max(1));
    let src = buf[..width * height].to_vec();
    for by in 0..rows {
        // This block's destination row span (floor of the even division, so the
        // last block soaks up any remainder).
        let y0 = by * height / rows;
        let y1 = ((by + 1) * height / rows).max(y0 + 1).min(height);
        for bx in 0..cols {
            let x0 = bx * width / cols;
            let x1 = ((bx + 1) * width / cols).max(x0 + 1).min(width);
            // Average the premultiplied values over the block.
            let mut acc = [0.0f64; 4];
            let mut count = 0.0f64;
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = src[y * width + x];
                    acc[0] += p[0] as f64;
                    acc[1] += p[1] as f64;
                    acc[2] += p[2] as f64;
                    acc[3] += p[3] as f64;
                    count += 1.0;
                }
            }
            let inv = if count > 0.0 { 1.0 / count } else { 0.0 };
            let avg = [
                (acc[0] * inv) as f32,
                (acc[1] * inv) as f32,
                (acc[2] * inv) as f32,
                (acc[3] * inv) as f32,
            ];
            // Fill the block with its average.
            for y in y0..y1 {
                for x in x0..x1 {
                    buf[y * width + x] = avg;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A default Find Edges for tweaking in tests.
    fn find() -> StylizeEffect {
        StylizeEffect::defaults()[0]
    }

    /// A default Mosaic for tweaking in tests.
    fn mos() -> StylizeEffect {
        StylizeEffect::defaults()[1]
    }

    fn approx(a: [f32; 4], b: [f32; 4], eps: f32) -> bool {
        a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() <= eps)
    }

    /// Build an opaque grayscale buffer from a row-major value grid (RGB = value,
    /// A = 1), already premultiplied (A = 1 so premul == straight). The caller
    /// passes the `width`/`height` separately to `apply`.
    fn gray(_width: usize, _height: usize, vals: &[f32]) -> Vec<[f32; 4]> {
        vals.iter().map(|&v| [v, v, v, 1.0]).collect()
    }

    // --- labels / defaults --------------------------------------------------

    #[test]
    fn labels_and_defaults() {
        let d = StylizeEffect::defaults();
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].label(), "Find Edges");
        assert_eq!(d[1].label(), "Mosaic");
    }

    #[test]
    fn serde_round_trips_every_stylize() {
        for e in StylizeEffect::defaults() {
            let json = serde_json::to_string(&e).unwrap();
            let back: StylizeEffect = serde_json::from_str(&json).unwrap();
            assert_eq!(e, back, "{} serde round-trip", e.label());
        }
    }

    #[test]
    fn empty_or_degenerate_buffer_is_a_noop() {
        // Zero dimensions must not panic / index out of bounds.
        for e in StylizeEffect::defaults() {
            let mut empty: Vec<[f32; 4]> = Vec::new();
            e.apply(&mut empty, 0, 0);
            assert!(empty.is_empty());
        }
    }

    // --- Find Edges ---------------------------------------------------------

    #[test]
    fn find_edges_flat_region_stays_white() {
        // A perfectly flat colour has zero gradient everywhere, so (inverted)
        // every pixel reads white. Alpha is preserved.
        let mut buf = gray(5, 5, &[0.5; 25]);
        find().apply(&mut buf, 5, 5);
        for p in &buf {
            assert!(approx(*p, [1.0, 1.0, 1.0, 1.0], 1e-5), "flat → white, got {p:?}");
        }
    }

    #[test]
    fn find_edges_responds_at_an_edge() {
        // A vertical step (left black, right white) produces a strong edge at the
        // boundary column: those pixels darken well below the flat-white interior.
        let vals: Vec<f32> = (0..36)
            .map(|i| if (i % 6) < 3 { 0.0 } else { 1.0 })
            .collect();
        let mut buf = gray(6, 6, &vals);
        find().apply(&mut buf, 6, 6);
        // A boundary pixel (column 2 or 3, mid row) sits on the step → darkened.
        let boundary = buf[2 * 6 + 2][0];
        // A flat-interior pixel (column 0, mid row) has no gradient → white.
        let flat = buf[2 * 6][0];
        assert!(boundary < 0.5, "edge pixel should darken, got {boundary}");
        assert!(flat > 0.95, "flat interior stays white, got {flat}");
    }

    #[test]
    fn find_edges_invert_flips_the_result() {
        // With invert on, the flat region is black (no edge) instead of white.
        let mut buf = gray(5, 5, &[0.5; 25]);
        let inv = StylizeEffect::FindEdges {
            amount: 1.0,
            invert: true,
        };
        inv.apply(&mut buf, 5, 5);
        for p in &buf {
            assert!(approx(*p, [0.0, 0.0, 0.0, 1.0], 1e-5), "flat inverted → black, got {p:?}");
        }
    }

    #[test]
    fn find_edges_amount_strengthens_edges() {
        // A higher amount drives a partial edge response darker (further from
        // white) — sample a soft ramp so the magnitude isn't already clipped.
        let vals: Vec<f32> = (0..36).map(|i| (i % 6) as f32 * 0.1).collect();
        let mut low = gray(6, 6, &vals);
        let mut high = gray(6, 6, &vals);
        StylizeEffect::FindEdges {
            amount: 0.5,
            invert: false,
        }
        .apply(&mut low, 6, 6);
        StylizeEffect::FindEdges {
            amount: 2.0,
            invert: false,
        }
        .apply(&mut high, 6, 6);
        // An interior gradient pixel: the stronger amount reads darker (lower).
        let idx = 2 * 6 + 3;
        assert!(
            high[idx][0] < low[idx][0],
            "higher amount should darken edges more ({} vs {})",
            high[idx][0],
            low[idx][0]
        );
    }

    #[test]
    fn find_edges_preserves_alpha() {
        // A semi-transparent flat region keeps its coverage; only RGB is reshaped.
        let mut buf: Vec<[f32; 4]> = (0..16).map(|_| [0.3 * 0.4, 0.3 * 0.4, 0.3 * 0.4, 0.4]).collect();
        find().apply(&mut buf, 4, 4);
        for p in &buf {
            assert!((p[3] - 0.4).abs() < 1e-5, "alpha preserved, got {}", p[3]);
        }
    }

    #[test]
    fn find_edges_is_deterministic() {
        let vals: Vec<f32> = (0..64).map(|i| ((i * 37) % 11) as f32 / 11.0).collect();
        let mut a = gray(8, 8, &vals);
        let mut b = gray(8, 8, &vals);
        find().apply(&mut a, 8, 8);
        find().apply(&mut b, 8, 8);
        assert_eq!(a, b, "find edges must be deterministic");
    }

    // --- Mosaic -------------------------------------------------------------

    #[test]
    fn mosaic_block_is_constant() {
        // A 2×2 mosaic over a 4×4 buffer: each 2×2 block becomes one constant
        // colour (the block's average).
        let vals: Vec<f32> = (0..16).map(|i| i as f32 / 16.0).collect();
        let mut buf = gray(4, 4, &vals);
        StylizeEffect::Mosaic {
            horizontal: 2,
            vertical: 2,
        }
        .apply(&mut buf, 4, 4);
        // Top-left block covers pixels (0,0),(1,0),(0,1),(1,1) — all now equal.
        let tl = buf[0];
        assert_eq!(buf[1], tl, "block pixel 1 matches");
        assert_eq!(buf[4], tl, "block pixel (0,1) matches");
        assert_eq!(buf[5], tl, "block pixel (1,1) matches");
    }

    #[test]
    fn mosaic_block_is_the_average() {
        // The top-left 2×2 block's value is the mean of its four source values.
        let vals: Vec<f32> = (0..16).map(|i| i as f32 / 16.0).collect();
        let expected = (vals[0] + vals[1] + vals[4] + vals[5]) / 4.0;
        let mut buf = gray(4, 4, &vals);
        StylizeEffect::Mosaic {
            horizontal: 2,
            vertical: 2,
        }
        .apply(&mut buf, 4, 4);
        assert!(
            (buf[0][0] - expected).abs() < 1e-5,
            "block average {} vs expected {expected}",
            buf[0][0]
        );
    }

    #[test]
    fn mosaic_one_by_one_is_the_whole_average() {
        // A 1×1 mosaic collapses the whole buffer to its single average colour.
        let vals: Vec<f32> = (0..16).map(|i| i as f32 / 16.0).collect();
        let mean = vals.iter().sum::<f32>() / vals.len() as f32;
        let mut buf = gray(4, 4, &vals);
        StylizeEffect::Mosaic {
            horizontal: 1,
            vertical: 1,
        }
        .apply(&mut buf, 4, 4);
        for p in &buf {
            assert!((p[0] - mean).abs() < 1e-5, "1×1 → whole average, got {}", p[0]);
        }
    }

    #[test]
    fn mosaic_full_resolution_is_identity() {
        // One block per pixel leaves the buffer unchanged (each block averages a
        // single pixel — itself).
        let vals: Vec<f32> = (0..16).map(|i| i as f32 / 16.0).collect();
        let orig = gray(4, 4, &vals);
        let mut buf = orig.clone();
        StylizeEffect::Mosaic {
            horizontal: 4,
            vertical: 4,
        }
        .apply(&mut buf, 4, 4);
        assert_eq!(buf, orig, "per-pixel mosaic is identity");
    }

    #[test]
    fn mosaic_zero_counts_are_clamped_to_one() {
        // A degenerate 0×0 request clamps to 1×1 (whole average), never panics.
        let vals: Vec<f32> = (0..16).map(|i| i as f32 / 16.0).collect();
        let mean = vals.iter().sum::<f32>() / vals.len() as f32;
        let mut buf = gray(4, 4, &vals);
        StylizeEffect::Mosaic {
            horizontal: 0,
            vertical: 0,
        }
        .apply(&mut buf, 4, 4);
        assert!((buf[0][0] - mean).abs() < 1e-5, "0 counts clamp to 1×1");
    }

    #[test]
    fn mosaic_more_blocks_than_pixels_is_per_pixel() {
        // Requesting more blocks than there are pixels can't sub-divide a pixel —
        // it clamps to per-pixel (identity), not a panic / empty block.
        let vals: Vec<f32> = (0..4).map(|i| i as f32).collect();
        let orig = gray(2, 2, &vals);
        let mut buf = orig.clone();
        StylizeEffect::Mosaic {
            horizontal: 100,
            vertical: 100,
        }
        .apply(&mut buf, 2, 2);
        assert_eq!(buf, orig, "over-subdivided mosaic clamps to per-pixel");
    }

    #[test]
    fn mosaic_averages_premultiplied_with_transparency() {
        // A block half opaque-white, half transparent should average to a
        // half-coverage, half-bright premultiplied value (premultiplied averaging
        // is the correct pooling for transparency).
        let buf_in = [
            [1.0, 1.0, 1.0, 1.0],
            [0.0, 0.0, 0.0, 0.0],
            [1.0, 1.0, 1.0, 1.0],
            [0.0, 0.0, 0.0, 0.0],
        ];
        let mut buf = buf_in.to_vec();
        StylizeEffect::Mosaic {
            horizontal: 1,
            vertical: 1,
        }
        .apply(&mut buf, 2, 2);
        assert!(
            approx(buf[0], [0.5, 0.5, 0.5, 0.5], 1e-5),
            "premultiplied average, got {:?}",
            buf[0]
        );
    }

    #[test]
    fn mosaic_is_deterministic() {
        let vals: Vec<f32> = (0..64).map(|i| ((i * 13) % 7) as f32 / 7.0).collect();
        let mut a = gray(8, 8, &vals);
        let mut b = gray(8, 8, &vals);
        mos().apply(&mut a, 8, 8);
        mos().apply(&mut b, 8, 8);
        assert_eq!(a, b, "mosaic must be deterministic");
    }

    // --- stack --------------------------------------------------------------

    #[test]
    fn apply_stylize_effects_stacks_in_order() {
        // Find Edges then Mosaic: the order matters (edges first, then pooled).
        let vals: Vec<f32> = (0..16).map(|i| (i % 4) as f32 / 4.0).collect();
        let mut stacked = gray(4, 4, &vals);
        apply_stylize_effects(&StylizeEffect::defaults(), &mut stacked, 4, 4);
        // Equivalent to applying them one at a time in order.
        let mut manual = gray(4, 4, &vals);
        StylizeEffect::defaults()[0].apply(&mut manual, 4, 4);
        StylizeEffect::defaults()[1].apply(&mut manual, 4, 4);
        assert_eq!(stacked, manual, "stack applies effects in order");
    }
}
