//! Spatial (whole-buffer) effects: Gaussian Blur, Box Blur, Directional Blur,
//! Radial Blur, Drop Shadow, and Glow.

use serde::{Deserialize, Serialize};

/// A **spatial** (whole-buffer) effect in a layer's effect stack.
///
/// Unlike [`Effect`](super::Effect) (a per-pixel color-correction pass), a spatial effect reads
/// neighbouring pixels — it convolves / offsets / blooms the layer's rendered
/// buffer — so it operates on the layer's *isolated* RGBA buffer rather than one
/// pixel at a time. These are the After-Effects motion-design staples that the
/// constant-color per-pixel path cannot express: **Gaussian Blur**, **Box Blur**,
/// **Directional Blur**, **Radial Blur**, **Drop Shadow**, and **Glow**.
///
/// The buffer is **premultiplied, linear-light** RGBA in row-major order (the
/// representation the software compositor's isolated layer buffer already uses):
/// `color · coverage` in RGB and `coverage` in A. Working premultiplied is what
/// lets a separable Gaussian blur soft, transparent edges without bleeding the
/// quad color across the alpha boundary. All passes are pure (no GPU, no time,
/// no IO) so the convolution math is unit-testable; they'll migrate to the
/// suite's `prism-fx` host when that lands.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum SpatialEffect {
    /// A separable Gaussian blur with a per-axis blurriness (sigma, comp px).
    /// `repeat_edge` clamps the kernel to the edge pixel (After Effects' "Repeat
    /// Edge Pixels") instead of treating off-buffer samples as transparent.
    GaussianBlur {
        sigma_x: f32,
        sigma_y: f32,
        repeat_edge: bool,
    },
    /// A **Box Blur** (After Effects' *Box Blur*): a separable moving-average
    /// box convolution of half-width `radius` (comp px) run `iterations` times.
    /// One box pass is a uniform average; **three** box passes approximate a
    /// Gaussian (central-limit), so `iterations` trades a hard, boxy look (1) for a
    /// smooth Gaussian-like one (3+) at a fraction of a true Gaussian's cost.
    /// `repeat_edge` clamps the kernel to the edge pixel ("Repeat Edge Pixels")
    /// instead of treating off-buffer samples as transparent.
    BoxBlur {
        radius: f32,
        /// Number of box passes (clamped `1..=8`); ~3 reads as a Gaussian.
        iterations: u32,
        repeat_edge: bool,
    },
    /// A **Directional Blur** (After Effects' *Directional Blur*): a 1-D box
    /// average of half-`length` (comp px) taken **along** `angle` (degrees, 0° =
    /// horizontal / +x, clockwise with +y down). The classic motion-streak — it
    /// smears the buffer in one direction only, leaving the perpendicular axis
    /// crisp. Off-buffer samples read as transparent so the streak fades at the
    /// frame border.
    DirectionalBlur {
        /// Streak direction in degrees (0° = horizontal; the smear is symmetric
        /// about each pixel along this axis).
        angle: f32,
        /// Half-length of the smear, comp px (the full streak spans `2·length`).
        length: f32,
    },
    /// A **Radial Blur** (After Effects' *Radial Blur*): blur **about** a centre
    /// (normalized buffer space `[0,1]²`) in one of two modes — `Spin` averages
    /// samples swept around the centre (a rotational motion blur), `Zoom` averages
    /// samples along the ray from the centre (a dolly-zoom streak). `amount` scales
    /// the sweep (degrees for spin, fractional radius for zoom). Samples are taken
    /// symmetrically about each pixel, so the centre itself stays sharp and the
    /// blur grows with distance from it.
    RadialBlur {
        center: [f32; 2],
        kind: RadialKind,
        /// Blur strength: total swept angle in **degrees** for `Spin`; fractional
        /// zoom span (e.g. `0.1` = ±10% radius) for `Zoom`.
        amount: f32,
    },
    /// A drop shadow: a blurred, tinted copy of the layer's alpha, offset by a
    /// distance at an angle (degrees), composited **behind** the layer at
    /// `opacity`. If `shadow_only` the layer itself is dropped, leaving just the
    /// shadow (After Effects' "Shadow Only").
    DropShadow {
        color: [f32; 3],
        opacity: f32,
        /// Shadow direction in degrees (0° = +x / right; clockwise, +y down).
        angle: f32,
        /// Offset distance along `angle`, comp px.
        distance: f32,
        /// Softness (Gaussian sigma, comp px).
        softness: f32,
        shadow_only: bool,
    },
    /// A glow / bloom: the layer's bright areas are blurred and added back on top
    /// of the original, brightening and blooming the highlights. `threshold`
    /// gates which luminance blooms; `radius` is the Gaussian sigma (comp px);
    /// `intensity` scales the bloom that is screened back over the layer.
    Glow {
        threshold: f32,
        radius: f32,
        intensity: f32,
    },
}

/// Which way [`SpatialEffect::RadialBlur`] blurs about its centre.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RadialKind {
    /// **Spin**: average samples swept *around* the centre — a rotational motion
    /// blur (After Effects' "Spin"). The default.
    #[default]
    Spin,
    /// **Zoom**: average samples *along the ray* from the centre — a dolly-zoom
    /// streak (After Effects' "Zoom").
    Zoom,
}

impl RadialKind {
    /// Both kinds, in menu order.
    pub const ALL: [RadialKind; 2] = [RadialKind::Spin, RadialKind::Zoom];

    /// A short, stable label for the UI.
    pub fn label(self) -> &'static str {
        match self {
            RadialKind::Spin => "Spin",
            RadialKind::Zoom => "Zoom",
        }
    }
}

impl SpatialEffect {
    /// A short, stable label for the UI and the "add effect" menu.
    pub fn label(&self) -> &'static str {
        match self {
            SpatialEffect::GaussianBlur { .. } => "Gaussian Blur",
            SpatialEffect::BoxBlur { .. } => "Box Blur",
            SpatialEffect::DirectionalBlur { .. } => "Directional Blur",
            SpatialEffect::RadialBlur { .. } => "Radial Blur",
            SpatialEffect::DropShadow { .. } => "Drop Shadow",
            SpatialEffect::Glow { .. } => "Glow",
        }
    }

    /// A fresh, sensibly-defaulted instance of each spatial effect, for the
    /// "add effect" menu. Defaults give a visible-but-tasteful result so adding
    /// one reads immediately. The Blur variants are kept first (grouped under the
    /// browser's *Blur & Sharpen* folder) and this array's order is the
    /// `default_index` the effect registry addresses, so the two must stay in
    /// sync (the `registry_indices_match_defaults` test guards this).
    pub fn defaults() -> [SpatialEffect; 6] {
        [
            SpatialEffect::GaussianBlur {
                sigma_x: 8.0,
                sigma_y: 8.0,
                repeat_edge: false,
            },
            SpatialEffect::BoxBlur {
                radius: 8.0,
                iterations: 3,
                repeat_edge: false,
            },
            SpatialEffect::DirectionalBlur {
                angle: 0.0,
                length: 16.0,
            },
            SpatialEffect::RadialBlur {
                center: [0.5, 0.5],
                kind: RadialKind::Spin,
                amount: 12.0,
            },
            SpatialEffect::DropShadow {
                color: [0.0, 0.0, 0.0],
                opacity: 0.5,
                angle: 135.0,
                distance: 12.0,
                softness: 6.0,
                shadow_only: false,
            },
            SpatialEffect::Glow {
                threshold: 0.6,
                radius: 12.0,
                intensity: 1.0,
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
            SpatialEffect::GaussianBlur {
                sigma_x,
                sigma_y,
                repeat_edge,
            } => {
                gaussian_blur(buf, width, height, sigma_x, sigma_y, repeat_edge);
            }
            SpatialEffect::BoxBlur {
                radius,
                iterations,
                repeat_edge,
            } => {
                box_blur(buf, width, height, radius, iterations, repeat_edge);
            }
            SpatialEffect::DirectionalBlur { angle, length } => {
                directional_blur(buf, width, height, angle, length);
            }
            SpatialEffect::RadialBlur {
                center,
                kind,
                amount,
            } => {
                radial_blur(buf, width, height, center, kind, amount);
            }
            SpatialEffect::DropShadow {
                color,
                opacity,
                angle,
                distance,
                softness,
                shadow_only,
            } => {
                drop_shadow(
                    buf,
                    width,
                    height,
                    color,
                    opacity.clamp(0.0, 1.0),
                    angle,
                    distance,
                    softness,
                    shadow_only,
                );
            }
            SpatialEffect::Glow {
                threshold,
                radius,
                intensity,
            } => {
                glow(buf, width, height, threshold, radius, intensity.max(0.0));
            }
        }
    }
}

/// Apply an ordered **spatial** effect stack to a premultiplied linear-light
/// RGBA buffer in place.
pub fn apply_spatial_effects(
    effects: &[SpatialEffect],
    buf: &mut [[f32; 4]],
    width: usize,
    height: usize,
) {
    for e in effects {
        e.apply(buf, width, height);
    }
}

/// Build a normalized 1-D Gaussian kernel for standard deviation `sigma` (px).
///
/// The kernel half-width is `ceil(3·sigma)` (covering ~99.7% of the mass); the
/// returned weights sum to 1. A non-positive `sigma` yields a single `[1.0]`
/// (identity) kernel.
pub fn gaussian_kernel(sigma: f32) -> Vec<f32> {
    if sigma <= 0.0 {
        return vec![1.0];
    }
    let radius = (sigma * 3.0).ceil() as i32;
    let two_s2 = 2.0 * sigma * sigma;
    let mut k: Vec<f32> = (-radius..=radius)
        .map(|i| {
            let x = i as f32;
            (-(x * x) / two_s2).exp()
        })
        .collect();
    let sum: f32 = k.iter().sum();
    if sum > 0.0 {
        for w in &mut k {
            *w /= sum;
        }
    }
    k
}

/// A separable Gaussian blur over a premultiplied RGBA buffer (horizontal then
/// vertical pass). `repeat_edge` clamps off-buffer samples to the edge pixel;
/// otherwise they read as transparent (zero), so the blur fades at the frame
/// border. A zero/negative sigma on an axis skips that axis's pass.
pub fn gaussian_blur(
    buf: &mut [[f32; 4]],
    width: usize,
    height: usize,
    sigma_x: f32,
    sigma_y: f32,
    repeat_edge: bool,
) {
    if sigma_x > 0.0 {
        let k = gaussian_kernel(sigma_x);
        convolve_axis(buf, width, height, &k, true, repeat_edge);
    }
    if sigma_y > 0.0 {
        let k = gaussian_kernel(sigma_y);
        convolve_axis(buf, width, height, &k, false, repeat_edge);
    }
}

/// Convolve `buf` along one axis with the (odd-length, centered) kernel `k`.
/// `horizontal` selects the x-axis; otherwise the y-axis. Off-buffer samples
/// clamp to the edge when `repeat_edge`, else contribute zero.
fn convolve_axis(
    buf: &mut [[f32; 4]],
    width: usize,
    height: usize,
    k: &[f32],
    horizontal: bool,
    repeat_edge: bool,
) {
    if k.len() <= 1 {
        return;
    }
    let radius = (k.len() / 2) as i32;
    let src = buf.to_vec();
    let (w, h) = (width as i32, height as i32);
    for y in 0..height {
        for x in 0..width {
            let mut acc = [0.0f32; 4];
            for (j, &weight) in k.iter().enumerate() {
                let off = j as i32 - radius;
                let (mut sx, mut sy) = if horizontal {
                    (x as i32 + off, y as i32)
                } else {
                    (x as i32, y as i32 + off)
                };
                let in_bounds = sx >= 0 && sx < w && sy >= 0 && sy < h;
                if !in_bounds {
                    if !repeat_edge {
                        continue; // transparent off-buffer sample (zero)
                    }
                    sx = sx.clamp(0, w - 1);
                    sy = sy.clamp(0, h - 1);
                }
                let s = src[sy as usize * width + sx as usize];
                acc[0] += s[0] * weight;
                acc[1] += s[1] * weight;
                acc[2] += s[2] * weight;
                acc[3] += s[3] * weight;
            }
            buf[y * width + x] = acc;
        }
    }
}

/// A separable **box blur** over a premultiplied RGBA buffer: a uniform
/// moving-average of half-width `radius` (px) applied along x then y, repeated
/// `iterations` times. One pass is a hard box average; ~three passes approximate a
/// Gaussian (central-limit). `repeat_edge` clamps off-buffer samples to the edge
/// pixel; otherwise they read as transparent (zero). A zero/negative radius or
/// zero iterations is a no-op. Iterations are clamped to `1..=8`.
pub fn box_blur(
    buf: &mut [[f32; 4]],
    width: usize,
    height: usize,
    radius: f32,
    iterations: u32,
    repeat_edge: bool,
) {
    let r = radius.floor() as i32;
    if r <= 0 {
        return;
    }
    let passes = iterations.clamp(1, 8);
    for _ in 0..passes {
        box_axis(buf, width, height, r, true, repeat_edge);
        box_axis(buf, width, height, r, false, repeat_edge);
    }
}

/// Average `buf` along one axis with a `2·radius+1`-wide uniform box window.
/// `horizontal` selects the x-axis; otherwise the y-axis. Off-buffer samples
/// clamp to the edge when `repeat_edge`, else contribute zero (and are excluded
/// from the average's divisor, so the box stays a true average of the in-bounds
/// taps — matching the Gaussian path's transparent-edge behaviour).
fn box_axis(
    buf: &mut [[f32; 4]],
    width: usize,
    height: usize,
    radius: i32,
    horizontal: bool,
    repeat_edge: bool,
) {
    if radius <= 0 {
        return;
    }
    let src = buf.to_vec();
    let (w, h) = (width as i32, height as i32);
    for y in 0..height {
        for x in 0..width {
            let mut acc = [0.0f32; 4];
            let mut count = 0.0f32;
            for off in -radius..=radius {
                let (mut sx, mut sy) = if horizontal {
                    (x as i32 + off, y as i32)
                } else {
                    (x as i32, y as i32 + off)
                };
                let in_bounds = sx >= 0 && sx < w && sy >= 0 && sy < h;
                if !in_bounds {
                    if !repeat_edge {
                        continue; // transparent off-buffer sample, excluded
                    }
                    sx = sx.clamp(0, w - 1);
                    sy = sy.clamp(0, h - 1);
                }
                let s = src[sy as usize * width + sx as usize];
                acc[0] += s[0];
                acc[1] += s[1];
                acc[2] += s[2];
                acc[3] += s[3];
                count += 1.0;
            }
            // `repeat_edge` always fills the full window; the transparent path may
            // drop edge taps but the centre tap always counts, so `count >= 1`.
            let inv = if count > 0.0 { 1.0 / count } else { 0.0 };
            buf[y * width + x] = [acc[0] * inv, acc[1] * inv, acc[2] * inv, acc[3] * inv];
        }
    }
}

/// Bilinearly sample a premultiplied RGBA buffer at **pixel-center** coordinates
/// `(sx, sy)` (a sample at the center of pixel `(x, y)` is at `(x + 0.5, y +
/// 0.5)`). Off-buffer samples read as transparent zero. The directional / radial
/// blurs accumulate sub-pixel taps through this so a streak stays smooth.
fn sample_bilinear(src: &[[f32; 4]], width: usize, height: usize, sx: f32, sy: f32) -> [f32; 4] {
    let fx = sx - 0.5;
    let fy = sy - 0.5;
    let x0f = fx.floor();
    let y0f = fy.floor();
    let tx = fx - x0f;
    let ty = fy - y0f;
    let x0 = x0f as i32;
    let y0 = y0f as i32;
    let fetch = |x: i32, y: i32| -> [f32; 4] {
        if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
            return [0.0; 4];
        }
        src[y as usize * width + x as usize]
    };
    let c00 = fetch(x0, y0);
    let c10 = fetch(x0 + 1, y0);
    let c01 = fetch(x0, y0 + 1);
    let c11 = fetch(x0 + 1, y0 + 1);
    let mut out = [0.0f32; 4];
    for k in 0..4 {
        let top = c00[k] * (1.0 - tx) + c10[k] * tx;
        let bot = c01[k] * (1.0 - tx) + c11[k] * tx;
        out[k] = top * (1.0 - ty) + bot * ty;
    }
    out
}

/// A **directional blur**: a 1-D box average of half-`length` (px) taken along
/// `angle` (degrees, 0° = +x). Each destination pixel averages evenly-spaced
/// (~1px apart) bilinear samples along the streak axis, centered on the pixel, so
/// the smear is symmetric and the perpendicular axis stays crisp. Off-buffer
/// samples are transparent. A non-positive length is a no-op.
pub fn directional_blur(
    buf: &mut [[f32; 4]],
    width: usize,
    height: usize,
    angle: f32,
    length: f32,
) {
    if length <= 0.0 {
        return;
    }
    let (sin, cos) = angle.to_radians().sin_cos();
    // One tap per ~pixel of streak length on each side of the centre.
    let steps = length.round().max(1.0) as i32;
    let src = buf.to_vec();
    for y in 0..height {
        for x in 0..width {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let mut acc = [0.0f32; 4];
            let mut count = 0.0f32;
            for s in -steps..=steps {
                // Parameter in [-length, length] along the streak axis.
                let t = (s as f32 / steps as f32) * length;
                let sx = px + cos * t;
                let sy = py + sin * t;
                let sample = sample_bilinear(&src, width, height, sx, sy);
                acc[0] += sample[0];
                acc[1] += sample[1];
                acc[2] += sample[2];
                acc[3] += sample[3];
                count += 1.0;
            }
            let inv = 1.0 / count;
            buf[y * width + x] = [acc[0] * inv, acc[1] * inv, acc[2] * inv, acc[3] * inv];
        }
    }
}

/// A **radial blur** about `center` (normalized buffer space). `Spin` averages
/// samples swept ±`amount/2` **degrees** around the centre (rotational motion
/// blur); `Zoom` averages samples along the ray from the centre over ±`amount`
/// fractional radius (dolly-zoom streak). Samples are symmetric about each pixel,
/// so the centre stays sharp and the blur grows with radius. A non-positive
/// `amount` is a no-op.
pub fn radial_blur(
    buf: &mut [[f32; 4]],
    width: usize,
    height: usize,
    center: [f32; 2],
    kind: RadialKind,
    amount: f32,
) {
    if amount <= 0.0 {
        return;
    }
    let w = width as f32;
    let h = height as f32;
    let cx = center[0] * w;
    let cy = center[1] * h;
    let src = buf.to_vec();
    // A fixed odd tap count keeps the pass deterministic and the centre sharp.
    let half_taps: i32 = 8;
    for y in 0..height {
        for x in 0..width {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let dx = px - cx;
            let dy = py - cy;
            let mut acc = [0.0f32; 4];
            let mut count = 0.0f32;
            for s in -half_taps..=half_taps {
                let frac = s as f32 / half_taps as f32; // [-1, 1]
                let (sx, sy) = match kind {
                    RadialKind::Spin => {
                        // Rotate the pixel's offset about the centre by up to
                        // ±amount/2 degrees.
                        let ang = (amount * 0.5 * frac).to_radians();
                        let (sn, cs) = ang.sin_cos();
                        (cx + dx * cs - dy * sn, cy + dx * sn + dy * cs)
                    }
                    RadialKind::Zoom => {
                        // Scale the radius by up to ±amount along the ray.
                        let scale = 1.0 + amount * frac;
                        (cx + dx * scale, cy + dy * scale)
                    }
                };
                let sample = sample_bilinear(&src, width, height, sx, sy);
                acc[0] += sample[0];
                acc[1] += sample[1];
                acc[2] += sample[2];
                acc[3] += sample[3];
                count += 1.0;
            }
            let inv = 1.0 / count;
            buf[y * width + x] = [acc[0] * inv, acc[1] * inv, acc[2] * inv, acc[3] * inv];
        }
    }
}

/// Source-over `src` onto `dst`, both **premultiplied** linear-light RGBA.
/// `out = src + dst·(1 - src.a)`.
fn over_premul(src: [f32; 4], dst: [f32; 4]) -> [f32; 4] {
    let ia = 1.0 - src[3];
    [
        src[0] + dst[0] * ia,
        src[1] + dst[1] * ia,
        src[2] + dst[2] * ia,
        src[3] + dst[3] * ia,
    ]
}

/// Drop-shadow pass: build a blurred, tinted, offset copy of the layer's alpha
/// and composite it **behind** the layer (premultiplied linear-light).
#[allow(clippy::too_many_arguments)]
fn drop_shadow(
    buf: &mut [[f32; 4]],
    width: usize,
    height: usize,
    color: [f32; 3],
    opacity: f32,
    angle: f32,
    distance: f32,
    softness: f32,
    shadow_only: bool,
) {
    let (sin, cos) = angle.to_radians().sin_cos();
    let dx = (cos * distance).round() as i32;
    let dy = (sin * distance).round() as i32; // +y down (screen convention)
    let (w, h) = (width as i32, height as i32);

    // Build the shadow layer: the source's coverage, offset, tinted, faded.
    // Premultiplied, so RGB = tint · coverage.
    let mut shadow = vec![[0.0f32; 4]; width * height];
    for y in 0..height {
        for x in 0..width {
            let sx = x as i32 - dx;
            let sy = y as i32 - dy;
            if sx < 0 || sx >= w || sy < 0 || sy >= h {
                continue;
            }
            let a = buf[sy as usize * width + sx as usize][3] * opacity;
            shadow[y * width + x] = [color[0] * a, color[1] * a, color[2] * a, a];
        }
    }
    if softness > 0.0 {
        gaussian_blur(&mut shadow, width, height, softness, softness, false);
    }

    // Composite: layer over shadow (shadow sits behind). Shadow-only drops the
    // layer entirely, leaving just the shadow.
    for (px, sh) in buf.iter_mut().zip(shadow.iter()) {
        *px = if shadow_only {
            *sh
        } else {
            over_premul(*px, *sh)
        };
    }
}

/// Glow / bloom pass: extract the layer's bright areas (above `threshold`),
/// blur them by `radius`, and **screen** the result back over the layer so the
/// highlights bloom. Premultiplied linear-light.
fn glow(
    buf: &mut [[f32; 4]],
    width: usize,
    height: usize,
    threshold: f32,
    radius: f32,
    intensity: f32,
) {
    // Extract bright mass. Work in straight color (un-premultiply) to threshold
    // on the layer's actual luminance, then re-premultiply the bloom seed.
    let mut bloom = vec![[0.0f32; 4]; width * height];
    for (i, px) in buf.iter().enumerate() {
        let a = px[3];
        if a <= 0.0 {
            continue;
        }
        let (r, g, b) = (px[0] / a, px[1] / a, px[2] / a);
        let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        if luma <= threshold {
            continue;
        }
        // How far above threshold the pixel is drives the bloom seed strength.
        let excess = (luma - threshold) / (1.0 - threshold).max(1e-3);
        let seed = (excess * intensity).clamp(0.0, 4.0) * a;
        // Premultiplied bloom seed in the pixel's own (bright) color.
        bloom[i] = [r * seed, g * seed, b * seed, seed];
    }
    if radius > 0.0 {
        gaussian_blur(&mut bloom, width, height, radius, radius, false);
    }
    // Screen the bloom over the layer (additive-ish highlight, premultiplied):
    // out = src + bloom·(1 - src) keeps it from blowing past white too hard while
    // still brightening. Alpha grows toward the bloom's coverage where the layer
    // was transparent so the glow extends past the layer edge.
    for (px, bl) in buf.iter_mut().zip(bloom.iter()) {
        let s = *px;
        px[0] = s[0] + bl[0] * (1.0 - s[0]).max(0.0);
        px[1] = s[1] + bl[1] * (1.0 - s[1]).max(0.0);
        px[2] = s[2] + bl[2] * (1.0 - s[2]).max(0.0);
        px[3] = (s[3] + bl[3] * (1.0 - s[3])).clamp(0.0, 1.0);
    }
}
