//! Distort (whole-buffer coordinate-remap) effects — the After-Effects *Distort*
//! category.
//!
//! Where a [`SpatialEffect`](super::SpatialEffect) *convolves / blooms / offsets*
//! a layer's rendered buffer, a **distort** effect *re-maps its coordinates*: for
//! every destination pixel it computes a **source position** and bilinearly
//! samples the original buffer there (the classic inverse-warp resampler). They
//! are the After-Effects geometric workhorses that the constant transform stack
//! cannot express:
//! - **Corner Pin** — pin the buffer's four corners to four arbitrary targets
//!   (a perspective/bilinear quad map), the staple screen-replacement / "stick
//!   this on that wall" tool.
//! - **Transform** — an extra position / scale / rotation / anchor / skew /
//!   opacity *inside* the effect stack (AE's *Transform* effect), so a geometric
//!   move can sit between other effects rather than only on the layer.
//! - **Mirror** — reflect the buffer across an arbitrary line (centre + angle),
//!   keeping one side and mirroring it onto the other.
//! - **Polar Coordinates** — remap between rectangular and polar space
//!   (rect→polar and polar→rect), the "tiny-planet" / radial-streak transform.
//!
//! Every pass works on the compositor's **premultiplied, linear-light** RGBA
//! buffer (`color · coverage` in RGB, `coverage` in A) in row-major order — the
//! same representation the spatial passes use — so the resampler interpolates
//! premultiplied values and soft / transparent edges don't bleed. Off-buffer
//! source samples read as **transparent** (the remap can expose the frame border).
//! All passes are pure (no GPU, no time, no IO) so the remap math is
//! unit-testable; they'll migrate to the suite's `prism-fx` host when that lands.
//!
//! Coordinates: parameters that name positions are in **normalized buffer space**
//! `[0, 1]²` (top-left origin, `+y` down) so a distort reads the same regardless
//! of the buffer's pixel size (the preview renders at a capped resolution, the
//! export at native — a fractional corner pins to the same visual spot in both).
//! Pixel-scaled params (Mirror's reflection is geometric; Transform's position is
//! a *fraction* of the buffer) follow the same convention.

use serde::{Deserialize, Serialize};

/// A **distort** (whole-buffer coordinate-remap) effect in a layer's effect
/// stack. Each variant computes, per destination pixel, a source position that is
/// bilinearly sampled from the original (premultiplied linear-light) buffer.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum DistortEffect {
    /// **Corner Pin**: map the buffer's four corners to four target points (each
    /// in normalized buffer space `[0,1]²`). Inside the target quad the source is
    /// found by **inverse bilinear** mapping (the same four-point warp After
    /// Effects' Corner Pin uses), so straight edges of the source stay straight
    /// only when the targets form a parallelogram and bow under a general quad —
    /// the expected bilinear-pin look. Targets are top-left, top-right,
    /// bottom-right, bottom-left (clockwise from the top-left).
    CornerPin {
        top_left: [f32; 2],
        top_right: [f32; 2],
        bottom_right: [f32; 2],
        bottom_left: [f32; 2],
    },
    /// **Transform**: an effect-level extra transform applied within the stack —
    /// AE's *Transform* effect. `anchor`/`position` are normalized buffer-space
    /// points (`[0,1]²`); `scale` is uniform; `rotation` is degrees (clockwise,
    /// `+y` down); `skew` shears in degrees about the anchor; `opacity` fades the
    /// whole buffer. The buffer is remapped so the anchor lands at the position,
    /// scaled / rotated / skewed about that anchor.
    Transform {
        anchor: [f32; 2],
        position: [f32; 2],
        scale: f32,
        rotation: f32,
        skew: f32,
        opacity: f32,
    },
    /// **Mirror**: reflect the buffer across a line through `center` (normalized
    /// buffer space) at `angle` degrees. The half-plane the line's normal points
    /// *away* from is kept; the other half is replaced by its mirror image, so the
    /// kept side reads through and the far side becomes its reflection.
    Mirror {
        center: [f32; 2],
        /// Reflection-line angle in degrees (0° = horizontal line; the normal
        /// points up, so the upper half is mirrored down onto the lower half by
        /// default).
        angle: f32,
    },
    /// **Polar Coordinates**: remap between rectangular and polar space about
    /// `center` (normalized buffer space). `RectToPolar` wraps the source's rows
    /// into rings around the centre (the "tiny planet" look); `PolarToRect`
    /// unwraps rings into rows (radial → linear). `interp` blends the result with
    /// the unaltered buffer (`0` = no effect, `1` = full remap), mirroring AE's
    /// *Interpolation* slider.
    Polar {
        center: [f32; 2],
        kind: PolarKind,
        interp: f32,
    },
}

/// Which way [`DistortEffect::Polar`] remaps space.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PolarKind {
    /// Rectangular → polar: wrap the source's columns/rows into rings around the
    /// centre (After Effects' "Rect to Polar"). The default.
    #[default]
    RectToPolar,
    /// Polar → rectangular: unwrap rings around the centre into rows (After
    /// Effects' "Polar to Rect").
    PolarToRect,
}

impl PolarKind {
    /// Both kinds, in menu order.
    pub const ALL: [PolarKind; 2] = [PolarKind::RectToPolar, PolarKind::PolarToRect];

    /// A short, stable label for the UI.
    pub fn label(self) -> &'static str {
        match self {
            PolarKind::RectToPolar => "Rect to Polar",
            PolarKind::PolarToRect => "Polar to Rect",
        }
    }
}

impl DistortEffect {
    /// A short, stable label for the UI and the "add effect" menu.
    pub fn label(&self) -> &'static str {
        match self {
            DistortEffect::CornerPin { .. } => "Corner Pin",
            DistortEffect::Transform { .. } => "Transform",
            DistortEffect::Mirror { .. } => "Mirror",
            DistortEffect::Polar { .. } => "Polar Coordinates",
        }
    }

    /// A fresh, sensibly-defaulted instance of each distort effect, for the "add
    /// effect" menu. Defaults give an identity-ish but slightly visible result so
    /// adding one reads immediately without destroying the layer:
    /// - Corner Pin pins the corners where they already are (identity).
    /// - Transform is the identity (anchor == position at centre, unit scale).
    /// - Mirror reflects across the vertical centre line.
    /// - Polar Coordinates does a full Rect→Polar about the centre.
    pub fn defaults() -> [DistortEffect; 4] {
        [
            DistortEffect::CornerPin {
                top_left: [0.0, 0.0],
                top_right: [1.0, 0.0],
                bottom_right: [1.0, 1.0],
                bottom_left: [0.0, 1.0],
            },
            DistortEffect::Transform {
                anchor: [0.5, 0.5],
                position: [0.5, 0.5],
                scale: 1.0,
                rotation: 0.0,
                skew: 0.0,
                opacity: 1.0,
            },
            DistortEffect::Mirror {
                center: [0.5, 0.5],
                angle: 90.0, // vertical line: reflect left↔right
            },
            DistortEffect::Polar {
                center: [0.5, 0.5],
                kind: PolarKind::RectToPolar,
                interp: 1.0,
            },
        ]
    }

    /// Apply this effect to a premultiplied linear-light RGBA buffer in place.
    ///
    /// `buf` is `width × height` row-major premultiplied RGBA. The pass reads the
    /// whole (original) buffer and writes the remapped result back into it.
    pub fn apply(&self, buf: &mut [[f32; 4]], width: usize, height: usize) {
        if width == 0 || height == 0 || buf.len() < width * height {
            return;
        }
        match *self {
            DistortEffect::CornerPin {
                top_left,
                top_right,
                bottom_right,
                bottom_left,
            } => corner_pin(buf, width, height, top_left, top_right, bottom_right, bottom_left),
            DistortEffect::Transform {
                anchor,
                position,
                scale,
                rotation,
                skew,
                opacity,
            } => transform(
                buf,
                width,
                height,
                anchor,
                position,
                scale,
                rotation,
                skew,
                opacity.clamp(0.0, 1.0),
            ),
            DistortEffect::Mirror { center, angle } => mirror(buf, width, height, center, angle),
            DistortEffect::Polar {
                center,
                kind,
                interp,
            } => polar(buf, width, height, center, kind, interp.clamp(0.0, 1.0)),
        }
    }
}

/// Apply an ordered **distort** effect stack to a premultiplied linear-light RGBA
/// buffer in place.
pub fn apply_distort_effects(
    effects: &[DistortEffect],
    buf: &mut [[f32; 4]],
    width: usize,
    height: usize,
) {
    for e in effects {
        e.apply(buf, width, height);
    }
}

/// Bilinearly sample a premultiplied RGBA buffer at **pixel** coordinates
/// `(sx, sy)` (sample at pixel *centers*, so integer `n` lands on pixel `n`'s
/// center is at `n + 0.5`; we pass center-relative coords). Off-buffer samples
/// read as transparent zero, so a remap that exposes the border fades out.
pub fn sample_bilinear(src: &[[f32; 4]], width: usize, height: usize, sx: f32, sy: f32) -> [f32; 4] {
    // Convert from "pixel center" space: a sample at the center of pixel (x, y)
    // is at (x + 0.5, y + 0.5); shift so integer floor indexes the right texel.
    let fx = sx - 0.5;
    let fy = sy - 0.5;
    let x0 = fx.floor();
    let y0 = fy.floor();
    let tx = fx - x0;
    let ty = fy - y0;
    let x0 = x0 as i32;
    let y0 = y0 as i32;

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

/// Run an inverse-warp resample: for each destination pixel, `src_of` returns the
/// source **pixel** coordinate to sample (or `None` to leave it transparent). The
/// source is the original buffer (cloned once); the destination is written back
/// into `buf`.
fn resample(
    buf: &mut [[f32; 4]],
    width: usize,
    height: usize,
    mut src_of: impl FnMut(usize, usize) -> Option<(f32, f32)>,
) {
    let src = buf[..width * height].to_vec();
    for y in 0..height {
        for x in 0..width {
            let out = match src_of(x, y) {
                Some((sx, sy)) => sample_bilinear(&src, width, height, sx, sy),
                None => [0.0; 4],
            };
            buf[y * width + x] = out;
        }
    }
}

/// Corner-pin pass: the destination buffer's four corners are pinned to the four
/// targets (normalized), and the interior is filled by **inverse bilinear** —
/// for each destination pixel inside the target quad, solve for the `(u, v)` of
/// the source's unit square it came from, then sample the source there. Pixels
/// outside the target quad are transparent.
fn corner_pin(
    buf: &mut [[f32; 4]],
    width: usize,
    height: usize,
    tl: [f32; 2],
    tr: [f32; 2],
    br: [f32; 2],
    bl: [f32; 2],
) {
    let w = width as f32;
    let h = height as f32;
    // Target corners in pixel space.
    let p_tl = (tl[0] * w, tl[1] * h);
    let p_tr = (tr[0] * w, tr[1] * h);
    let p_br = (br[0] * w, br[1] * h);
    let p_bl = (bl[0] * w, bl[1] * h);

    resample(buf, width, height, |x, y| {
        let px = x as f32 + 0.5;
        let py = y as f32 + 0.5;
        // Solve the inverse bilinear: find (u, v) in [0,1]² such that
        //   P(u,v) = (1-u)(1-v)·TL + u(1-v)·TR + uv·BR + (1-u)v·BL = (px, py).
        let (u, v) = inverse_bilinear(p_tl, p_tr, p_br, p_bl, (px, py))?;
        if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
            return None;
        }
        // The source's unit square maps u→x, v→y over the original buffer.
        Some((u * w, v * h))
    });
}

/// Solve the inverse of the bilinear map from the unit square `(u,v)` to the
/// quad `(tl, tr, br, bl)` for the point `p`. Returns `(u, v)`, or `None` if the
/// quadratic in `v` has no real root (degenerate quad). Corners are TL=(0,0),
/// TR=(1,0), BR=(1,1), BL=(0,1).
fn inverse_bilinear(
    tl: (f32, f32),
    tr: (f32, f32),
    br: (f32, f32),
    bl: (f32, f32),
    p: (f32, f32),
) -> Option<(f32, f32)> {
    // P(u,v) = tl + u·(tr-tl) + v·(bl-tl) + uv·(tl - tr + br - bl)
    // Write as A + u·B + v·C + uv·D = p.
    let a = tl;
    let b = (tr.0 - tl.0, tr.1 - tl.1);
    let c = (bl.0 - tl.0, bl.1 - tl.1);
    let d = (
        tl.0 - tr.0 + br.0 - bl.0,
        tl.1 - tr.1 + br.1 - bl.1,
    );
    let q = (p.0 - a.0, p.1 - a.1);

    // Eliminate u. From q = u·B + v·C + uv·D, write q - v·C = u·(B + v·D) and
    // cross both sides with (B + v·D) (whose cross with itself is 0):
    //   (q - v·C) × (B + v·D) = 0
    //   ⇒ (C×D)·v² + (C×B − q×D)·v − (q×B) = 0
    // using the 2D cross a×b = a.x·b.y − a.y·b.x.
    let cross = |m: (f32, f32), n: (f32, f32)| m.0 * n.1 - m.1 * n.0;
    let a2 = cross(c, d);
    let b2 = cross(c, b) - cross(q, d);
    let c2 = -cross(q, b);

    let v = if a2.abs() < 1e-9 {
        // Linear in v (affine / parallelogram quad).
        if b2.abs() < 1e-12 {
            return None;
        }
        -c2 / b2
    } else {
        let disc = b2 * b2 - 4.0 * a2 * c2;
        if disc < 0.0 {
            return None;
        }
        let sq = disc.sqrt();
        let v1 = (-b2 + sq) / (2.0 * a2);
        let v2 = (-b2 - sq) / (2.0 * a2);
        // Prefer the root inside [0,1]; else the closer-to-range one.
        let pick = |v: f32| (0.0..=1.0).contains(&v);
        if pick(v1) {
            v1
        } else if pick(v2) {
            v2
        } else if (v1 - 0.5).abs() <= (v2 - 0.5).abs() {
            v1
        } else {
            v2
        }
    };
    // Back-substitute for u: q = u·(B + v·D) + v·C  ->  u·(B + v·D) = q - v·C.
    let denom = (b.0 + v * d.0, b.1 + v * d.1);
    let rhs = (q.0 - v * c.0, q.1 - v * c.1);
    let u = if denom.0.abs() >= denom.1.abs() {
        if denom.0.abs() < 1e-12 {
            return None;
        }
        rhs.0 / denom.0
    } else {
        if denom.1.abs() < 1e-12 {
            return None;
        }
        rhs.1 / denom.1
    };
    Some((u, v))
}

/// Effect-level Transform pass: build the forward map (anchor → position, scaled /
/// rotated / skewed about the anchor) in pixel space, invert it, and inverse-warp.
/// `opacity` fades the whole result.
#[allow(clippy::too_many_arguments)]
fn transform(
    buf: &mut [[f32; 4]],
    width: usize,
    height: usize,
    anchor: [f32; 2],
    position: [f32; 2],
    scale: f32,
    rotation: f32,
    skew: f32,
    opacity: f32,
) {
    let w = width as f32;
    let h = height as f32;
    let ax = anchor[0] * w;
    let ay = anchor[1] * h;
    let pxp = position[0] * w;
    let pyp = position[1] * h;
    let s = scale.max(0.0);
    let (sin, cos) = rotation.to_radians().sin_cos();
    let shear = skew.to_radians().tan();

    // Forward (dst = M · (src - anchor) + position) is rotate∘skew∘scale; we need
    // the inverse to fetch the source for each dst. Build the linear part L and
    // invert it analytically.
    // L = R · K · S, where S = sI, K = [[1, shear],[0,1]], R = [[cos,-sin],[sin,cos]].
    let k00 = 1.0;
    let k01 = shear;
    let k10 = 0.0;
    let k11 = 1.0;
    // R·K
    let rk00 = cos * k00 - sin * k10;
    let rk01 = cos * k01 - sin * k11;
    let rk10 = sin * k00 + cos * k10;
    let rk11 = sin * k01 + cos * k11;
    // ·S (uniform s)
    let l00 = rk00 * s;
    let l01 = rk01 * s;
    let l10 = rk10 * s;
    let l11 = rk11 * s;
    let det = l00 * l11 - l01 * l10;
    if det.abs() < 1e-12 {
        // Collapsed transform: nothing survives.
        for px in buf[..width * height].iter_mut() {
            *px = [0.0; 4];
        }
        return;
    }
    let inv = 1.0 / det;
    let i00 = l11 * inv;
    let i01 = -l01 * inv;
    let i10 = -l10 * inv;
    let i11 = l00 * inv;

    resample(buf, width, height, |x, y| {
        let dx = x as f32 + 0.5 - pxp;
        let dy = y as f32 + 0.5 - pyp;
        // src = anchor + L⁻¹ · (dst - position)
        let sx = ax + i00 * dx + i01 * dy;
        let sy = ay + i10 * dx + i11 * dy;
        Some((sx, sy))
    });
    if opacity < 1.0 {
        for px in buf[..width * height].iter_mut() {
            for c in px.iter_mut() {
                *c *= opacity;
            }
        }
    }
}

/// Mirror pass: reflect the buffer across the line through `center` at `angle`.
/// The half-plane the normal points toward is mirrored onto the other side; the
/// far half reads its reflection, the near half passes through.
fn mirror(buf: &mut [[f32; 4]], width: usize, height: usize, center: [f32; 2], angle: f32) {
    let w = width as f32;
    let h = height as f32;
    let cx = center[0] * w;
    let cy = center[1] * h;
    // Line direction; its normal is (−sin, cos) rotated from the direction.
    let (sin, cos) = angle.to_radians().sin_cos();
    // Normal to the line (unit).
    let nx = -sin;
    let ny = cos;

    resample(buf, width, height, |x, y| {
        let px = x as f32 + 0.5;
        let py = y as f32 + 0.5;
        // Signed distance from the line.
        let signed = (px - cx) * nx + (py - cy) * ny;
        if signed <= 0.0 {
            // Near side: pass through.
            Some((px, py))
        } else {
            // Far side: reflect across the line.
            Some((px - 2.0 * signed * nx, py - 2.0 * signed * ny))
        }
    });
}

/// Polar-coordinates pass about `center`, blended with the original by `interp`.
///
/// `RectToPolar`: the destination is read in polar terms — angle θ around the
/// centre maps to a source *column* (θ over the full turn → x over the width) and
/// radius r maps to a source *row* (r over the max radius → y over the height) —
/// so the source's rows wrap into rings (the "tiny planet" look).
/// `PolarToRect`: the inverse — the destination's `(x, y)` are read as (θ, r) and
/// the source is sampled at the corresponding cartesian point around the centre,
/// unwrapping rings into rows.
fn polar(
    buf: &mut [[f32; 4]],
    width: usize,
    height: usize,
    center: [f32; 2],
    kind: PolarKind,
    interp: f32,
) {
    if interp <= 0.0 {
        return; // identity
    }
    let w = width as f32;
    let h = height as f32;
    let cx = center[0] * w;
    let cy = center[1] * h;
    let max_r = (cx.max(w - cx)).hypot(cy.max(h - cy));
    let two_pi = std::f32::consts::TAU;
    let src = buf[..width * height].to_vec();

    for y in 0..height {
        for x in 0..width {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let mapped = match kind {
                PolarKind::RectToPolar => {
                    // Destination read as cartesian around centre → (θ, r) →
                    // source column/row.
                    let dx = px - cx;
                    let dy = py - cy;
                    let mut theta = dy.atan2(dx); // (-π, π]
                    if theta < 0.0 {
                        theta += two_pi;
                    }
                    let r = dx.hypot(dy);
                    let su = (theta / two_pi) * w;
                    let sv = if max_r > 0.0 { (r / max_r) * h } else { 0.0 };
                    sample_bilinear(&src, width, height, su, sv)
                }
                PolarKind::PolarToRect => {
                    // Destination (x, y) read as (θ, r) → cartesian source point.
                    let theta = (px / w) * two_pi;
                    let r = (py / h) * max_r;
                    let su = cx + r * theta.cos();
                    let sv = cy + r * theta.sin();
                    sample_bilinear(&src, width, height, su, sv)
                }
            };
            let idx = y * width + x;
            if interp >= 1.0 {
                buf[idx] = mapped;
            } else {
                let orig = src[idx];
                for k in 0..4 {
                    buf[idx][k] = orig[k] * (1.0 - interp) + mapped[k] * interp;
                }
            }
        }
    }
}
