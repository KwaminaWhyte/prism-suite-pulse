//! Layer masks: closed Bézier paths (layer-local) carving a layer's coverage.

use serde::{Deserialize, Serialize};

/// How a [`Mask`] combines with the masks above it on the same layer (After
/// Effects' mask-mode dropdown).
///
/// Each mask produces a per-pixel coverage in `[0, 1]` (1 = fully inside the
/// shape, 0 = fully outside, fractional on a feathered edge). The masks on a
/// layer are folded **top-down** into a single coverage that multiplies the
/// layer's own alpha: an [`MaskMode::Add`] unions its shape in, a
/// [`MaskMode::Subtract`] knocks it out, an [`MaskMode::Intersect`] keeps only
/// the overlap, and a [`MaskMode::Difference`] keeps the symmetric difference.
/// [`MaskMode::None`] disables the mask without deleting it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MaskMode {
    /// Disabled — the mask contributes nothing (kept for re-enabling/editing).
    None,
    /// Union: `out = acc + cov·(1 - acc)` (the default for a new mask).
    #[default]
    Add,
    /// Knockout: `out = acc·(1 - cov)`.
    Subtract,
    /// Keep the overlap: `out = acc·cov`.
    Intersect,
    /// Symmetric difference: `out = acc + cov - 2·acc·cov`.
    Difference,
}

impl MaskMode {
    /// All modes, in menu order.
    pub const ALL: [MaskMode; 5] = [
        MaskMode::Add,
        MaskMode::Subtract,
        MaskMode::Intersect,
        MaskMode::Difference,
        MaskMode::None,
    ];

    /// Short label for the mask-mode picker.
    pub fn label(self) -> &'static str {
        match self {
            MaskMode::None => "None",
            MaskMode::Add => "Add",
            MaskMode::Subtract => "Subtract",
            MaskMode::Intersect => "Intersect",
            MaskMode::Difference => "Difference",
        }
    }

    /// Fold this mask's coverage `cov` (already feathered/inverted, in `[0,1]`)
    /// into the running accumulated coverage `acc`, returning the new
    /// accumulator. The very first **enabled** mask on a layer is composited
    /// against a fully-transparent base, so an `Add` reveals exactly its shape
    /// and a `Subtract`/`Intersect` against nothing yields nothing — matching
    /// After Effects, where the topmost mask's mode acts on an empty layer mask.
    pub fn combine(self, acc: f32, cov: f32) -> f32 {
        let cov = cov.clamp(0.0, 1.0);
        let acc = acc.clamp(0.0, 1.0);
        let out = match self {
            MaskMode::None => acc,
            MaskMode::Add => acc + cov * (1.0 - acc),
            MaskMode::Subtract => acc * (1.0 - cov),
            MaskMode::Intersect => acc * cov,
            MaskMode::Difference => acc + cov - 2.0 * acc * cov,
        };
        out.clamp(0.0, 1.0)
    }
}

/// One vertex of a [`Mask`] path: a layer-local anchor point plus its two
/// Bézier tangent handles, stored as **offsets** from the anchor (After
/// Effects' in/out tangents).
///
/// Coordinates are in the layer's local frame — the same `±half_w/±half_h`
/// comp-pixel space the layer's quad lives in (origin at the layer center),
/// before the layer's world transform — so a mask rides the layer's
/// position/scale/rotation/parenting for free. A zero in/out handle makes the
/// adjoining segment a straight line (a corner point); non-zero handles make it
/// a cubic Bézier.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaskVertex {
    /// Anchor position (layer-local comp px).
    pub x: f32,
    pub y: f32,
    /// Tangent handle leaving the *previous* segment / arriving at this anchor
    /// (offset from the anchor).
    pub in_x: f32,
    pub in_y: f32,
    /// Tangent handle leaving this anchor toward the *next* vertex (offset).
    pub out_x: f32,
    pub out_y: f32,
}

impl MaskVertex {
    /// A corner vertex at `(x, y)` with no tangent handles (straight segments).
    pub fn corner(x: f32, y: f32) -> Self {
        MaskVertex {
            x,
            y,
            in_x: 0.0,
            in_y: 0.0,
            out_x: 0.0,
            out_y: 0.0,
        }
    }

    /// The anchor as a tuple.
    pub fn pos(&self) -> (f32, f32) {
        (self.x, self.y)
    }
    /// The absolute (layer-local) position of the outgoing tangent control.
    pub fn out_handle(&self) -> (f32, f32) {
        (self.x + self.out_x, self.y + self.out_y)
    }
    /// The absolute (layer-local) position of the incoming tangent control.
    pub fn in_handle(&self) -> (f32, f32) {
        (self.x + self.in_x, self.y + self.in_y)
    }
}

/// A **mask** on a layer: a closed Bézier path defining a region of the layer
/// to keep or remove, in layer-local space (After Effects' layer masks).
///
/// The path is flattened to a polygon (sampling each cubic Bézier segment) and
/// rasterized by an even-odd point-in-polygon test, yielding a per-pixel
/// coverage that is then **expanded/contracted** (offset), **feathered**
/// (softened) and optionally **inverted**, scaled by `opacity`, and finally
/// folded into the layer's coverage by the mask's [`MaskMode`]. Mask shapes are
/// not yet keyframable (that arrives with the typed-`Property<Path>` rebuild),
/// so a mask is a fixed shape per layer for now; the geometry below is the pure,
/// time-agnostic core a future animated mask will sample into.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mask {
    /// Display name (for the masks list).
    pub name: String,
    /// Boolean combination with the masks above it (and the layer).
    pub mode: MaskMode,
    /// Invert the coverage (`1 - cov`) before combining — show the layer
    /// *outside* the shape.
    pub inverted: bool,
    /// Mask opacity in `[0, 1]`: scales the coverage the shape contributes.
    pub opacity: f32,
    /// Edge softness in comp px (per side). `0` is a hard edge; larger values
    /// ramp the coverage linearly across `feather` px straddling the boundary.
    pub feather: f32,
    /// Signed offset in comp px: positive **expands** the shape outward,
    /// negative **contracts** it (After Effects' mask expansion).
    pub expansion: f32,
    /// The closed path's vertices (layer-local comp px), in order.
    pub vertices: Vec<MaskVertex>,
}

impl Default for Mask {
    fn default() -> Self {
        Mask {
            name: "Mask".to_owned(),
            mode: MaskMode::Add,
            inverted: false,
            opacity: 1.0,
            feather: 0.0,
            expansion: 0.0,
            vertices: Vec::new(),
        }
    }
}

/// How finely each cubic Bézier mask segment is flattened into line segments.
/// A fixed subdivision is plenty for the small mask paths Pulse edits and keeps
/// the point-in-polygon test cheap and deterministic.
const MASK_BEZIER_STEPS: u32 = 16;

impl Mask {
    /// A rectangular mask covering `[-hw, hw] × [-hh, hh]` (layer-local px),
    /// four corner vertices — the default "new mask" shape, sized to the layer.
    pub fn rect(hw: f32, hh: f32) -> Self {
        Mask {
            vertices: vec![
                MaskVertex::corner(-hw, -hh),
                MaskVertex::corner(hw, -hh),
                MaskVertex::corner(hw, hh),
                MaskVertex::corner(-hw, hh),
            ],
            ..Mask::default()
        }
    }

    /// An elliptical mask inscribed in `[-hw, hw] × [-hh, hh]`, built from four
    /// Bézier vertices with the standard `k ≈ 0.5523` circle-approximation
    /// handles (a smooth oval — AE's elliptical mask tool).
    pub fn ellipse(hw: f32, hh: f32) -> Self {
        // Kappa: handle length as a fraction of the radius for a 90° arc.
        const K: f32 = 0.552_284_8;
        let (kx, ky) = (hw * K, hh * K);
        // Right, bottom, left, top anchors with tangents along the perimeter.
        let verts = vec![
            MaskVertex {
                x: hw,
                y: 0.0,
                in_x: 0.0,
                in_y: -ky,
                out_x: 0.0,
                out_y: ky,
            },
            MaskVertex {
                x: 0.0,
                y: hh,
                in_x: kx,
                in_y: 0.0,
                out_x: -kx,
                out_y: 0.0,
            },
            MaskVertex {
                x: -hw,
                y: 0.0,
                in_x: 0.0,
                in_y: ky,
                out_x: 0.0,
                out_y: -ky,
            },
            MaskVertex {
                x: 0.0,
                y: -hh,
                in_x: -kx,
                in_y: 0.0,
                out_x: kx,
                out_y: 0.0,
            },
        ];
        Mask {
            vertices: verts,
            ..Mask::default()
        }
    }

    /// Whether the mask actually contributes (mode isn't [`MaskMode::None`] and
    /// it has enough vertices to enclose an area).
    pub fn is_active(&self) -> bool {
        self.mode != MaskMode::None && self.vertices.len() >= 3
    }

    /// Flatten the closed Bézier path into a polygon of `(x, y)` points in
    /// layer-local space, subdividing each cubic segment into
    /// [`MASK_BEZIER_STEPS`] chords. The polygon is implicitly closed (the last
    /// point connects back to the first). Straight segments (zero handles)
    /// collapse to a single chord cheaply since their interior points are
    /// colinear.
    pub fn flatten(&self) -> Vec<(f32, f32)> {
        let n = self.vertices.len();
        if n < 2 {
            return self.vertices.iter().map(|v| v.pos()).collect();
        }
        let mut out = Vec::with_capacity(n * MASK_BEZIER_STEPS as usize);
        for i in 0..n {
            let a = &self.vertices[i];
            let b = &self.vertices[(i + 1) % n];
            let (p0x, p0y) = a.pos();
            let (p1x, p1y) = a.out_handle();
            let (p2x, p2y) = b.in_handle();
            let (p3x, p3y) = b.pos();
            // A straight segment (no handles either side) needs only its start.
            let straight = a.out_x == 0.0 && a.out_y == 0.0 && b.in_x == 0.0 && b.in_y == 0.0;
            if straight {
                out.push((p0x, p0y));
                continue;
            }
            let steps = MASK_BEZIER_STEPS;
            for s in 0..steps {
                let u = s as f32 / steps as f32;
                let mt = 1.0 - u;
                let w0 = mt * mt * mt;
                let w1 = 3.0 * mt * mt * u;
                let w2 = 3.0 * mt * u * u;
                let w3 = u * u * u;
                out.push((
                    w0 * p0x + w1 * p1x + w2 * p2x + w3 * p3x,
                    w0 * p0y + w1 * p1y + w2 * p2y + w3 * p3y,
                ));
            }
        }
        out
    }

    /// The signed distance-ish **coverage** of layer-local point `(px, py)`
    /// against this mask, in `[0, 1]`, *before* opacity scaling and mode
    /// folding.
    ///
    /// Computed from the flattened polygon: the point's signed distance to the
    /// nearest edge (negative = outside, positive = inside, via an even-odd
    /// inside test) is shifted by `expansion` and ramped across the `feather`
    /// width to a soft `[0,1]` coverage, then inverted if requested and scaled
    /// by `opacity`. A hard-edged mask (`feather == 0`) returns a crisp 0/1
    /// (then ×opacity).
    pub fn coverage_at(&self, poly: &[(f32, f32)], px: f32, py: f32) -> f32 {
        if poly.len() < 3 {
            return 0.0;
        }
        let inside = point_in_polygon(poly, px, py);
        let dist = dist_to_polygon(poly, px, py); // ≥ 0, distance to boundary
                                                  // Signed distance: positive inside, negative outside.
        let signed = if inside { dist } else { -dist };
        // Expansion shifts the boundary outward (+) / inward (−).
        let signed = signed + self.expansion;
        // Feather ramps coverage from 0 to 1 across ±feather/2 around the edge.
        let cov = if self.feather <= 0.0 {
            if signed >= 0.0 {
                1.0
            } else {
                0.0
            }
        } else {
            let half = self.feather * 0.5;
            ((signed + half) / self.feather).clamp(0.0, 1.0)
        };
        let cov = if self.inverted { 1.0 - cov } else { cov };
        (cov * self.opacity).clamp(0.0, 1.0)
    }
}

/// Even-odd point-in-polygon test (ray casting) for a closed polygon given as
/// an ordered list of `(x, y)` vertices (the closing edge is implicit).
pub fn point_in_polygon(poly: &[(f32, f32)], px: f32, py: f32) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        // Does a horizontal ray from (px, py) cross edge j→i?
        let crosses = (yi > py) != (yj > py)
            && px < (xj - xi) * (py - yi) / (yj - yi + f32::EPSILON.copysign(yj - yi)) + xi;
        if crosses {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// The shortest Euclidean distance from `(px, py)` to the boundary of a closed
/// polygon (the minimum distance to any of its edges). Always `≥ 0`.
pub fn dist_to_polygon(poly: &[(f32, f32)], px: f32, py: f32) -> f32 {
    let n = poly.len();
    if n == 0 {
        return f32::INFINITY;
    }
    let mut best = f32::INFINITY;
    let mut j = n - 1;
    for i in 0..n {
        best = best.min(dist_to_segment((px, py), poly[j], poly[i]));
        j = i;
    }
    best
}

/// Euclidean distance from point `p` to the segment `a→b`.
fn dist_to_segment(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (px, py) = p;
    let (ax, ay) = a;
    let (bx, by) = b;
    let (dx, dy) = (bx - ax, by - ay);
    let len2 = dx * dx + dy * dy;
    if len2 <= f32::EPSILON {
        return ((px - ax).powi(2) + (py - ay).powi(2)).sqrt();
    }
    let t = (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0);
    let (cx, cy) = (ax + t * dx, ay + t * dy);
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

/// Fold a layer's whole mask stack into a single coverage multiplier in
/// `[0, 1]` for the layer-local point `(px, py)`.
///
/// The masks are combined **top-down** (list order) via each mask's
/// [`MaskMode::combine`], each contributing its [`Mask::coverage_at`]. When the
/// layer has **no active masks** the layer is unmasked, so this returns `1.0`
/// (full coverage) — callers should special-case "no masks" rather than
/// multiplying by this. `polys` must be the pre-flattened polygon for each mask
/// in `masks` (same order), so the hot per-pixel loop doesn't re-flatten.
pub fn mask_stack_coverage(masks: &[Mask], polys: &[Vec<(f32, f32)>], px: f32, py: f32) -> f32 {
    let mut acc = 0.0;
    let mut any = false;
    for (mask, poly) in masks.iter().zip(polys.iter()) {
        if !mask.is_active() {
            continue;
        }
        any = true;
        let cov = mask.coverage_at(poly, px, py);
        acc = mask.mode.combine(acc, cov);
    }
    if any {
        acc
    } else {
        1.0
    }
}
