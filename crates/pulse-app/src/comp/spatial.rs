//! Spatial (whole-buffer) effects: Gaussian Blur, Drop Shadow, and Glow.

use serde::{Deserialize, Serialize};

/// A **spatial** (whole-buffer) effect in a layer's effect stack.
///
/// Unlike [`Effect`](super::Effect) (a per-pixel color-correction pass), a spatial effect reads
/// neighbouring pixels — it convolves / offsets / blooms the layer's rendered
/// buffer — so it operates on the layer's *isolated* RGBA buffer rather than one
/// pixel at a time. These are the After-Effects motion-design staples that the
/// constant-color per-pixel path cannot express: **Gaussian Blur**, **Drop
/// Shadow**, and **Glow**.
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

impl SpatialEffect {
    /// A short, stable label for the UI and the "add effect" menu.
    pub fn label(&self) -> &'static str {
        match self {
            SpatialEffect::GaussianBlur { .. } => "Gaussian Blur",
            SpatialEffect::DropShadow { .. } => "Drop Shadow",
            SpatialEffect::Glow { .. } => "Glow",
        }
    }

    /// A fresh, sensibly-defaulted instance of each spatial effect, for the
    /// "add effect" menu. Defaults give a visible-but-tasteful result so adding
    /// one reads immediately.
    pub fn defaults() -> [SpatialEffect; 3] {
        [
            SpatialEffect::GaussianBlur {
                sigma_x: 8.0,
                sigma_y: 8.0,
                repeat_edge: false,
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
