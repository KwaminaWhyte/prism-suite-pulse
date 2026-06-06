//! Shape layers: parametric vector shapes (rectangle / ellipse / polygon /
//! star) with a fill and a stroke, in layer-local space.
//!
//! A [`ShapeLayer`] is an ordered stack of [`ShapeItem`]s. Each item is one
//! parametric [`ShapePrimitive`], an optional [`Fill`], and an optional
//! [`Stroke`]. The geometry is pure and time-agnostic: each primitive flattens
//! to a closed layer-local polygon; coverage is an even-odd point-in-polygon
//! test for the fill and a signed-distance band for the stroke, both
//! antialiased by the same nearest-edge distance the mask system uses. The
//! renderer rasterizes the stack into the layer's isolated premultiplied
//! linear-light buffer (so a shape layer composes with masks, mattes, spatial
//! effects, and motion blur exactly like a solid).
//!
//! Paths are not yet keyframable (that arrives with the typed-`Property<Path>`
//! rebuild — same as masks); a shape is a fixed look per layer for now, and the
//! pure geometry here is the core a future animated shape will sample into.

use super::mask::{dist_to_polygon, point_in_polygon};
use serde::{Deserialize, Serialize};

/// One parametric shape primitive, centered at its local origin (the item's
/// `offset` then places it within the layer). Sizes are half-extents / radii in
/// layer-local comp px.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum ShapePrimitive {
    /// An axis-aligned rectangle spanning `[-half_w, half_w] × [-half_h,
    /// half_h]`, with an optional corner `radius` (rounded rectangle; `0` =
    /// sharp corners).
    Rectangle {
        half_w: f32,
        half_h: f32,
        radius: f32,
    },
    /// An ellipse inscribed in `[-rx, rx] × [-ry, ry]`.
    Ellipse { rx: f32, ry: f32 },
    /// A regular `points`-gon of circumradius `radius`, with the first vertex
    /// straight up (After Effects' polygon).
    Polygon { points: u32, radius: f32 },
    /// A `points`-pointed star alternating between `outer` and `inner`
    /// circumradii, first point straight up (After Effects' star).
    Star { points: u32, outer: f32, inner: f32 },
}

/// How many chords each quarter-arc (ellipse / rounded corner) is flattened
/// into. Fixed subdivision keeps the point tests cheap and deterministic.
const ARC_STEPS: u32 = 16;

impl ShapePrimitive {
    /// A short, stable label for the UI and the "add shape" menu.
    pub fn label(&self) -> &'static str {
        match self {
            ShapePrimitive::Rectangle { .. } => "Rectangle",
            ShapePrimitive::Ellipse { .. } => "Ellipse",
            ShapePrimitive::Polygon { .. } => "Polygon",
            ShapePrimitive::Star { .. } => "Star",
        }
    }

    /// Flatten this primitive into a closed polygon of layer-local `(x, y)`
    /// points (the closing edge back to the first point is implicit). Degenerate
    /// parameters (non-positive size, <3 polygon/star points) yield fewer than
    /// three points, which the coverage path treats as empty.
    pub fn flatten(&self) -> Vec<(f32, f32)> {
        match *self {
            ShapePrimitive::Rectangle {
                half_w,
                half_h,
                radius,
            } => rectangle_poly(half_w, half_h, radius),
            ShapePrimitive::Ellipse { rx, ry } => ellipse_poly(rx, ry),
            ShapePrimitive::Polygon { points, radius } => regular_poly(points, radius),
            ShapePrimitive::Star {
                points,
                outer,
                inner,
            } => star_poly(points, outer, inner),
        }
    }
}

/// A rectangle (optionally rounded). With `radius <= 0` it is the four corners;
/// otherwise each corner is replaced by a quarter-arc of that radius (clamped to
/// the smaller half-extent so the rounding can't exceed the rect).
fn rectangle_poly(half_w: f32, half_h: f32, radius: f32) -> Vec<(f32, f32)> {
    if half_w <= 0.0 || half_h <= 0.0 {
        return Vec::new();
    }
    let r = radius.clamp(0.0, half_w.min(half_h));
    if r <= 0.0 {
        return vec![
            (-half_w, -half_h),
            (half_w, -half_h),
            (half_w, half_h),
            (-half_w, half_h),
        ];
    }
    // Corner centers, walked clockwise from the top-right; each emits a
    // quarter-arc sweeping into the next side.
    let centers = [
        (half_w - r, -half_h + r, -std::f32::consts::FRAC_PI_2), // top-right, start pointing up
        (half_w - r, half_h - r, 0.0),                           // bottom-right
        (-half_w + r, half_h - r, std::f32::consts::FRAC_PI_2),  // bottom-left
        (-half_w + r, -half_h + r, std::f32::consts::PI),        // top-left
    ];
    let mut out = Vec::with_capacity(4 * (ARC_STEPS as usize + 1));
    for (cx, cy, start) in centers {
        for s in 0..=ARC_STEPS {
            let a = start + (s as f32 / ARC_STEPS as f32) * std::f32::consts::FRAC_PI_2;
            out.push((cx + r * a.cos(), cy + r * a.sin()));
        }
    }
    out
}

/// An ellipse sampled at `4 * ARC_STEPS` points (a closed polygon).
fn ellipse_poly(rx: f32, ry: f32) -> Vec<(f32, f32)> {
    if rx <= 0.0 || ry <= 0.0 {
        return Vec::new();
    }
    let n = 4 * ARC_STEPS;
    (0..n)
        .map(|i| {
            let a = (i as f32 / n as f32) * std::f32::consts::TAU;
            (rx * a.cos(), ry * a.sin())
        })
        .collect()
}

/// A regular `points`-gon of circumradius `radius`, first vertex straight up
/// (`-y`).
fn regular_poly(points: u32, radius: f32) -> Vec<(f32, f32)> {
    if points < 3 || radius <= 0.0 {
        return Vec::new();
    }
    (0..points)
        .map(|i| {
            // Start at the top (-y) and walk clockwise.
            let a =
                -std::f32::consts::FRAC_PI_2 + (i as f32 / points as f32) * std::f32::consts::TAU;
            (radius * a.cos(), radius * a.sin())
        })
        .collect()
}

/// A `points`-pointed star alternating `outer`/`inner` circumradii, first point
/// straight up (`-y`).
fn star_poly(points: u32, outer: f32, inner: f32) -> Vec<(f32, f32)> {
    if points < 2 || outer <= 0.0 || inner <= 0.0 {
        return Vec::new();
    }
    let n = points * 2;
    (0..n)
        .map(|i| {
            let r = if i % 2 == 0 { outer } else { inner };
            let a = -std::f32::consts::FRAC_PI_2 + (i as f32 / n as f32) * std::f32::consts::TAU;
            (r * a.cos(), r * a.sin())
        })
        .collect()
}

/// A solid fill for a shape: a straight sRGB color and an opacity.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fill {
    /// Straight sRGB RGB in `[0, 1]`.
    pub color: [f32; 3],
    /// Fill opacity in `[0, 1]`.
    pub opacity: f32,
}

impl Default for Fill {
    fn default() -> Self {
        Fill {
            color: [1.0, 1.0, 1.0],
            opacity: 1.0,
        }
    }
}

/// A stroke (outline) for a shape: a straight sRGB color, a width centered on
/// the path, and an opacity.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    /// Straight sRGB RGB in `[0, 1]`.
    pub color: [f32; 3],
    /// Stroke width in comp px (centered on the path boundary).
    pub width: f32,
    /// Stroke opacity in `[0, 1]`.
    pub opacity: f32,
}

impl Default for Stroke {
    fn default() -> Self {
        Stroke {
            color: [0.0, 0.0, 0.0],
            width: 4.0,
            opacity: 1.0,
        }
    }
}

/// One drawable item in a shape layer: a parametric primitive plus an optional
/// fill and an optional stroke, offset within the layer.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShapeItem {
    pub primitive: ShapePrimitive,
    /// Local offset of the primitive's center from the layer origin (comp px).
    pub offset_x: f32,
    pub offset_y: f32,
    /// Fill, if any (drawn first / under the stroke).
    pub fill: Option<Fill>,
    /// Stroke, if any (drawn over the fill).
    pub stroke: Option<Stroke>,
}

impl ShapeItem {
    /// A new item at the layer origin with a default white fill and no stroke.
    pub fn new(primitive: ShapePrimitive) -> Self {
        ShapeItem {
            primitive,
            offset_x: 0.0,
            offset_y: 0.0,
            fill: Some(Fill::default()),
            stroke: None,
        }
    }

    /// The flattened polygon translated into layer-local space by the item's
    /// offset.
    pub fn polygon(&self) -> Vec<(f32, f32)> {
        self.primitive
            .flatten()
            .into_iter()
            .map(|(x, y)| (x + self.offset_x, y + self.offset_y))
            .collect()
    }
}

/// The straight RGBA the item's fill / stroke contribute at layer-local point
/// `(px, py)`, given its pre-flattened `poly` (from [`ShapeItem::polygon`]).
///
/// Coverage is antialiased over a ~1 px ramp using the nearest-edge distance:
/// the fill ramps the interior in across the boundary, and the stroke is a band
/// of `width` straddling the boundary. The stroke (when present) is composited
/// over the fill, so an item is a filled-then-outlined shape. Returns
/// `[r, g, b, a]` straight sRGB (alpha = coverage·opacity), `a = 0` when the
/// point contributes nothing.
pub fn item_coverage(item: &ShapeItem, poly: &[(f32, f32)], px: f32, py: f32) -> [f32; 4] {
    if poly.len() < 3 {
        return [0.0; 4];
    }
    // Antialiasing ramp half-width (comp px) for both fill edge and stroke band.
    const AA: f32 = 0.75;
    let dist = dist_to_polygon(poly, px, py);
    let inside = point_in_polygon(poly, px, py);
    // Signed distance to the boundary: positive inside, negative outside.
    let signed = if inside { dist } else { -dist };

    // Fill coverage: 1 well inside, ramping to 0 across ±AA around the boundary.
    let mut out = [0.0f32; 4];
    if let Some(fill) = item.fill {
        let cov = ((signed + AA) / (2.0 * AA)).clamp(0.0, 1.0) * fill.opacity.clamp(0.0, 1.0);
        if cov > 0.0 {
            out = [fill.color[0], fill.color[1], fill.color[2], cov];
        }
    }

    // Stroke coverage: a band of `width` centered on the boundary, so the band
    // covers |signed| <= width/2 (ramped by AA at each edge of the band).
    if let Some(stroke) = item.stroke {
        if stroke.width > 0.0 {
            let half = stroke.width * 0.5;
            // Distance from the band's center line is |signed|; inside the band
            // when that is <= half. Ramp the two band edges by AA.
            let band = ((half - dist) / (2.0 * AA) + 0.5).clamp(0.0, 1.0);
            let cov = band * stroke.opacity.clamp(0.0, 1.0);
            if cov > 0.0 {
                out = over_straight(
                    [stroke.color[0], stroke.color[1], stroke.color[2], cov],
                    out,
                );
            }
        }
    }
    out
}

/// Straight (non-premultiplied) source-over of `src` over `dst`, both
/// `[r, g, b, a]` with `a` as coverage. Used to stack stroke over fill.
fn over_straight(src: [f32; 4], dst: [f32; 4]) -> [f32; 4] {
    let sa = src[3].clamp(0.0, 1.0);
    let da = dst[3].clamp(0.0, 1.0);
    let out_a = sa + da * (1.0 - sa);
    if out_a <= 0.0 {
        return [0.0; 4];
    }
    let blend = |s: f32, d: f32| (s * sa + d * da * (1.0 - sa)) / out_a;
    [
        blend(src[0], dst[0]),
        blend(src[1], dst[1]),
        blend(src[2], dst[2]),
        out_a,
    ]
}

/// A **shape layer**: an ordered stack of [`ShapeItem`]s drawn bottom-up
/// (index 0 first / under). The whole stack is rasterized in the layer's local
/// frame and rides the layer transform (position / scale / rotation / parent)
/// like any other layer.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ShapeLayer {
    pub items: Vec<ShapeItem>,
}

impl ShapeLayer {
    /// Whether the layer draws anything (at least one item with three or more
    /// flattened points).
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The straight sRGB RGBA the whole shape stack contributes at layer-local
    /// `(px, py)`: each item's [`item_coverage`] composited bottom-up. `polys`
    /// must be the pre-flattened polygon for each item (same order), so the hot
    /// per-pixel loop doesn't re-flatten.
    pub fn coverage_at(&self, polys: &[Vec<(f32, f32)>], px: f32, py: f32) -> [f32; 4] {
        let mut acc = [0.0f32; 4];
        for (item, poly) in self.items.iter().zip(polys.iter()) {
            let c = item_coverage(item, poly, px, py);
            if c[3] > 0.0 {
                acc = over_straight(c, acc);
            }
        }
        acc
    }

    /// The layer-local axis-aligned bounding box of the whole stack
    /// `(min_x, min_y, max_x, max_y)`, padded by half each item's stroke width,
    /// or `None` when nothing draws. The renderer uses it to bound the pixel
    /// loop instead of scanning the whole frame.
    pub fn local_bounds(&self) -> Option<(f32, f32, f32, f32)> {
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        let mut any = false;
        for item in &self.items {
            let poly = item.polygon();
            if poly.len() < 3 {
                continue;
            }
            // Pad by the stroke half-width so the stroke band isn't clipped.
            let pad = item.stroke.map(|s| s.width * 0.5).unwrap_or(0.0).max(0.0) + 1.0;
            for (x, y) in poly {
                any = true;
                min_x = min_x.min(x - pad);
                min_y = min_y.min(y - pad);
                max_x = max_x.max(x + pad);
                max_y = max_y.max(y + pad);
            }
        }
        any.then_some((min_x, min_y, max_x, max_y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangle_flattens_to_four_corners() {
        let p = ShapePrimitive::Rectangle {
            half_w: 10.0,
            half_h: 5.0,
            radius: 0.0,
        };
        let poly = p.flatten();
        assert_eq!(poly.len(), 4);
        // Center is inside; a far point is outside.
        assert!(point_in_polygon(&poly, 0.0, 0.0));
        assert!(!point_in_polygon(&poly, 20.0, 0.0));
        // The corner extents are exactly the half-extents.
        let max_x = poly.iter().map(|p| p.0).fold(f32::MIN, f32::max);
        let max_y = poly.iter().map(|p| p.1).fold(f32::MIN, f32::max);
        assert!((max_x - 10.0).abs() < 1e-4);
        assert!((max_y - 5.0).abs() < 1e-4);
    }

    #[test]
    fn rounded_rectangle_clips_the_corner() {
        // A square with a large corner radius rounds: the extreme corner point is
        // no longer covered, but the center still is.
        let sharp = ShapePrimitive::Rectangle {
            half_w: 10.0,
            half_h: 10.0,
            radius: 0.0,
        }
        .flatten();
        let round = ShapePrimitive::Rectangle {
            half_w: 10.0,
            half_h: 10.0,
            radius: 6.0,
        }
        .flatten();
        // The very corner (9.9, 9.9) is inside the sharp rect, outside the round.
        assert!(point_in_polygon(&sharp, 9.9, 9.9));
        assert!(!point_in_polygon(&round, 9.9, 9.9));
        assert!(point_in_polygon(&round, 0.0, 0.0));
        // The rounded path has many more vertices (arc subdivision).
        assert!(round.len() > sharp.len());
    }

    #[test]
    fn ellipse_inside_and_outside() {
        let poly = ShapePrimitive::Ellipse { rx: 10.0, ry: 6.0 }.flatten();
        assert!(point_in_polygon(&poly, 0.0, 0.0));
        // Just inside the major axis vs. clearly outside the minor axis.
        assert!(point_in_polygon(&poly, 9.0, 0.0));
        assert!(!point_in_polygon(&poly, 0.0, 9.0));
        // Corner of the bounding box is outside the ellipse.
        assert!(!point_in_polygon(&poly, 9.5, 5.5));
    }

    #[test]
    fn polygon_has_point_count_vertices() {
        let poly = ShapePrimitive::Polygon {
            points: 6,
            radius: 10.0,
        }
        .flatten();
        assert_eq!(poly.len(), 6);
        assert!(point_in_polygon(&poly, 0.0, 0.0));
        // First vertex points straight up (-y).
        let top = poly[0];
        assert!(top.0.abs() < 1e-4 && (top.1 + 10.0).abs() < 1e-4);
    }

    #[test]
    fn star_alternates_radii() {
        let poly = ShapePrimitive::Star {
            points: 5,
            outer: 10.0,
            inner: 4.0,
        }
        .flatten();
        assert_eq!(poly.len(), 10, "5-point star = 10 vertices");
        // Even indices sit at the outer radius, odd at the inner.
        let r = |i: usize| (poly[i].0 * poly[i].0 + poly[i].1 * poly[i].1).sqrt();
        assert!((r(0) - 10.0).abs() < 1e-3);
        assert!((r(1) - 4.0).abs() < 1e-3);
        assert!(point_in_polygon(&poly, 0.0, 0.0));
    }

    #[test]
    fn degenerate_primitives_are_empty() {
        assert!(
            ShapePrimitive::Rectangle {
                half_w: 0.0,
                half_h: 5.0,
                radius: 0.0
            }
            .flatten()
            .len()
                < 3
        );
        assert!(ShapePrimitive::Polygon {
            points: 2,
            radius: 5.0
        }
        .flatten()
        .is_empty());
        assert!(ShapePrimitive::Ellipse { rx: -1.0, ry: 5.0 }
            .flatten()
            .is_empty());
    }

    #[test]
    fn fill_coverage_inside_is_color_outside_is_clear() {
        let item = ShapeItem::new(ShapePrimitive::Rectangle {
            half_w: 10.0,
            half_h: 10.0,
            radius: 0.0,
        });
        let poly = item.polygon();
        let inside = item_coverage(&item, &poly, 0.0, 0.0);
        assert!((inside[3] - 1.0).abs() < 1e-3, "fully inside fill");
        assert_eq!(inside[0], 1.0); // default white fill
        let outside = item_coverage(&item, &poly, 50.0, 50.0);
        assert_eq!(outside[3], 0.0, "outside contributes nothing");
    }

    #[test]
    fn fill_opacity_scales_coverage() {
        let mut item = ShapeItem::new(ShapePrimitive::Ellipse { rx: 10.0, ry: 10.0 });
        item.fill = Some(Fill {
            color: [1.0, 0.0, 0.0],
            opacity: 0.5,
        });
        let poly = item.polygon();
        let c = item_coverage(&item, &poly, 0.0, 0.0);
        assert!(
            (c[3] - 0.5).abs() < 1e-3,
            "alpha = opacity inside, got {}",
            c[3]
        );
    }

    #[test]
    fn fill_edge_is_antialiased() {
        // A point straddling the boundary gets partial fill coverage between the
        // fully-inside 1.0 and fully-outside 0.0.
        let item = ShapeItem::new(ShapePrimitive::Rectangle {
            half_w: 10.0,
            half_h: 10.0,
            radius: 0.0,
        });
        let poly = item.polygon();
        let edge = item_coverage(&item, &poly, 10.0, 0.0); // exactly on the edge
        assert!(edge[3] > 0.0 && edge[3] < 1.0, "edge AA, got {}", edge[3]);
    }

    #[test]
    fn stroke_only_covers_the_boundary_band() {
        // A stroked, unfilled shape: the boundary is covered, the deep interior
        // and the far exterior are not.
        let mut item = ShapeItem::new(ShapePrimitive::Rectangle {
            half_w: 20.0,
            half_h: 20.0,
            radius: 0.0,
        });
        item.fill = None;
        item.stroke = Some(Stroke {
            color: [0.0, 0.0, 1.0],
            width: 4.0,
            opacity: 1.0,
        });
        let poly = item.polygon();
        // On the boundary (x = 20): covered, blue.
        let on = item_coverage(&item, &poly, 20.0, 0.0);
        assert!(on[3] > 0.5, "stroke covers the boundary, got {}", on[3]);
        assert!(on[2] > on[0], "stroke is blue");
        // Deep interior: not covered (no fill, far from the band).
        assert_eq!(item_coverage(&item, &poly, 0.0, 0.0)[3], 0.0);
        // Far outside the band.
        assert_eq!(item_coverage(&item, &poly, 40.0, 0.0)[3], 0.0);
    }

    #[test]
    fn stroke_composites_over_fill() {
        // A filled + stroked shape reads the stroke color on the boundary and the
        // fill color in the interior.
        let mut item = ShapeItem::new(ShapePrimitive::Rectangle {
            half_w: 20.0,
            half_h: 20.0,
            radius: 0.0,
        });
        item.fill = Some(Fill {
            color: [1.0, 0.0, 0.0],
            opacity: 1.0,
        });
        item.stroke = Some(Stroke {
            color: [0.0, 0.0, 1.0],
            width: 4.0,
            opacity: 1.0,
        });
        let poly = item.polygon();
        let interior = item_coverage(&item, &poly, 0.0, 0.0);
        assert!(interior[0] > interior[2], "interior is the red fill");
        let boundary = item_coverage(&item, &poly, 20.0, 0.0);
        assert!(boundary[2] > boundary[0], "boundary is the blue stroke");
    }

    #[test]
    fn stack_composites_items_bottom_up() {
        // Two overlapping rects: the later (top) item's fill wins where they
        // overlap.
        let mut layer = ShapeLayer::default();
        let mut a = ShapeItem::new(ShapePrimitive::Rectangle {
            half_w: 10.0,
            half_h: 10.0,
            radius: 0.0,
        });
        a.fill = Some(Fill {
            color: [1.0, 0.0, 0.0],
            opacity: 1.0,
        });
        let mut b = ShapeItem::new(ShapePrimitive::Rectangle {
            half_w: 10.0,
            half_h: 10.0,
            radius: 0.0,
        });
        b.fill = Some(Fill {
            color: [0.0, 1.0, 0.0],
            opacity: 1.0,
        });
        layer.items.push(a); // bottom (red)
        layer.items.push(b); // top (green)
        let polys: Vec<_> = layer.items.iter().map(|it| it.polygon()).collect();
        let c = layer.coverage_at(&polys, 0.0, 0.0);
        assert!(c[1] > c[0], "top green item wins, got {c:?}");
    }

    #[test]
    fn local_bounds_pads_for_stroke() {
        let mut item = ShapeItem::new(ShapePrimitive::Rectangle {
            half_w: 10.0,
            half_h: 10.0,
            radius: 0.0,
        });
        item.stroke = Some(Stroke {
            color: [0.0; 3],
            width: 8.0,
            opacity: 1.0,
        });
        let layer = ShapeLayer { items: vec![item] };
        let (min_x, _, max_x, _) = layer.local_bounds().unwrap();
        // 10 + 4 (half stroke) + 1 (margin) = 15.
        assert!((14.9..=15.1).contains(&max_x), "padded max_x, got {max_x}");
        assert!((-15.1..=-14.9).contains(&min_x));
    }

    #[test]
    fn empty_layer_has_no_bounds() {
        assert!(ShapeLayer::default().local_bounds().is_none());
        // An item with no fill and no stroke still bounds its geometry.
        let mut item = ShapeItem::new(ShapePrimitive::Ellipse { rx: 5.0, ry: 5.0 });
        item.fill = None;
        let layer = ShapeLayer { items: vec![item] };
        assert!(layer.local_bounds().is_some());
    }

    #[test]
    fn serde_round_trips() {
        let mut layer = ShapeLayer::default();
        layer.items.push(ShapeItem::new(ShapePrimitive::Star {
            points: 6,
            outer: 30.0,
            inner: 12.0,
        }));
        let json = serde_json::to_string(&layer).unwrap();
        let back: ShapeLayer = serde_json::from_str(&json).unwrap();
        assert_eq!(layer, back);
    }
}
