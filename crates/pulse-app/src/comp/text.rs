//! Text layers: a string laid out into glyphs, drawn either by a self-contained,
//! dependency-free **stroke vector font** (the default / back-compat path) or by
//! a selected **real outline font** (TrueType faces via `fontdb` + `ttf-parser`).
//!
//! A [`TextLayer`] is a string plus type settings (font size, tracking, leading,
//! alignment), an optional `font_family`, and a [`Fill`](super::shape::Fill) /
//! [`Stroke`](super::shape::Stroke) reused from the shape system.
//!
//! **Stroke font (`font_family == None`).** Each visible character maps to a
//! small set of **polyline strokes** authored on a unit em grid (`glyph` below);
//! the renderer scales those strokes by the font size, lays them out
//! left-to-right with per-line alignment, and rasterizes coverage as a thickened
//! band around the nearest stroke (a "line font"), antialiased the same way
//! shapes are. This is the original, self-contained path: `None` (the default,
//! and every legacy `.pulse` file, which carries no `font_family` key) renders
//! **identically** to before this module gained outline support.
//!
//! **Outline font (`font_family == Some(family)`).** The string is laid out into
//! real glyph **outlines** read from the chosen TrueType face — advances + the
//! glyph contours come from [`ttf_parser`], the family is enumerated / resolved
//! by [`super::fonts`]. Each glyph contour flattens to a closed layer-local
//! polygon; coverage is the **same antialiased even-odd polygon fill the shape
//! layer rasterizes** (`point_in_polygon` + nearest-edge AA), so glyphs read as
//! crisp filled shapes. An unknown / unloadable family falls back to the bundled
//! default face (text never vanishes).
//!
//! Everything here is pure and time-agnostic — the layout produces layer-local
//! geometry that rides the layer's transform (position / scale / rotation /
//! parent) exactly like a shape layer, so text composes with masks, mattes,
//! spatial effects, and motion blur for free, whichever font path is active.
//! Still open: per-character animators, weight / style sub-selection, kerning /
//! full OpenType shaping (outline advances are plain horizontal metrics), and
//! variable-font axes.

use super::mask::{dist_to_polygon, dist_to_segment, point_in_polygon};
use super::shape::{Fill, Stroke};
use serde::{Deserialize, Serialize};

/// One stroke segment of the built-in font: a pair of layer-local (or em-space)
/// `(x, y)` endpoints. The font draws glyphs as a set of these line strokes,
/// thickened into a filled pen band at render time.
pub type Seg = ((f32, f32), (f32, f32));

/// Horizontal alignment of each text line within the block.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

impl TextAlign {
    /// All alignments, in menu order.
    pub const ALL: [TextAlign; 3] = [TextAlign::Left, TextAlign::Center, TextAlign::Right];

    pub fn label(self) -> &'static str {
        match self {
            TextAlign::Left => "Left",
            TextAlign::Center => "Center",
            TextAlign::Right => "Right",
        }
    }
}

/// The advance width of one glyph cell as a fraction of the font size (the
/// built-in font is monospace: every cell is the same width). Glyphs are
/// authored in a `[0, 0.5] × [0, 1]` em box and scaled by the font size.
const ADVANCE_EM: f32 = 0.62;
/// Default line leading (baseline-to-baseline) as a fraction of the font size.
const LEADING_EM: f32 = 1.2;
/// Stroke half-width of the font's pen as a fraction of the font size, used when
/// the layer carries no explicit stroke (the fill renders the pen body).
const PEN_HALF_EM: f32 = 0.055;

/// A text layer: a string drawn with the built-in stroke font, with type
/// settings and a fill / stroke (reused from the shape system).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextLayer {
    /// The source text. Newlines split it into laid-out lines.
    pub text: String,
    /// Font size in comp px (the glyph em height).
    pub size: f32,
    /// Extra spacing between characters, in comp px (After Effects' tracking).
    pub tracking: f32,
    /// Baseline-to-baseline line spacing in comp px. `0` = auto (`size·1.2`).
    pub leading: f32,
    /// Per-line horizontal alignment.
    pub align: TextAlign,
    /// The real font family to render with. `None` (the default, and every legacy
    /// `.pulse` file — `#[serde(default)]` so the absent key deserializes to
    /// `None`) keeps the built-in **stroke font**, so old projects render
    /// identically. `Some(family)` switches to **real glyph outlines** from that
    /// TrueType face (resolved via [`super::fonts`]; an unknown / unloadable
    /// family falls back to the bundled default face, never to nothing).
    #[serde(default)]
    pub font_family: Option<String>,
    /// Fill of the glyph pen body (the strokes' interior). `None` draws only the
    /// outline stroke (if any).
    pub fill: Option<Fill>,
    /// An extra outline drawn around the glyph pen body. `None` = no outline.
    pub stroke: Option<Stroke>,
}

impl Default for TextLayer {
    fn default() -> Self {
        TextLayer {
            text: "TEXT".to_string(),
            size: 120.0,
            tracking: 0.0,
            leading: 0.0,
            align: TextAlign::default(),
            font_family: None,
            fill: Some(Fill::default()),
            stroke: None,
        }
    }
}

impl TextLayer {
    /// Whether the layer draws anything (non-empty text and at least a fill or
    /// stroke).
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() || (self.fill.is_none() && self.stroke.is_none())
    }

    /// Whether this layer renders with **real outline glyphs** (a family is
    /// selected) rather than the built-in **stroke font** (`font_family` is
    /// `None`). The renderer branches on this to pick the layout + rasterizer.
    pub fn uses_outline(&self) -> bool {
        self.font_family.is_some()
    }

    /// The resolved line leading in comp px (auto = `size·1.2`).
    pub fn line_leading(&self) -> f32 {
        if self.leading > 0.0 {
            self.leading
        } else {
            self.size * LEADING_EM
        }
    }

    /// The advance width of one character cell in comp px (font advance plus
    /// tracking).
    pub fn advance(&self) -> f32 {
        self.size * ADVANCE_EM + self.tracking
    }

    /// The pixel width of `line` (character count × advance, less the trailing
    /// tracking past the last glyph so a line is tightly measured).
    pub fn line_width(&self, line: &str) -> f32 {
        let n = line.chars().count();
        if n == 0 {
            return 0.0;
        }
        n as f32 * self.advance() - self.tracking
    }

    /// The lines of the text (split on `\n`).
    pub fn lines(&self) -> std::str::Lines<'_> {
        self.text.lines()
    }

    /// Lay the whole string out into a flat list of **stroke segments** in
    /// layer-local space (comp px, origin at the layer's geometric center, `+y`
    /// down). Each segment is `((x0, y0), (x1, y1))`. The block is centered
    /// vertically about the origin; each line is aligned horizontally per
    /// [`TextAlign`].
    pub fn segments(&self) -> Vec<Seg> {
        let mut out = Vec::new();
        let size = self.size.max(0.0);
        if size <= 0.0 {
            return out;
        }
        let advance = self.advance();
        let leading = self.line_leading();
        let lines: Vec<&str> = self.lines().collect();
        if lines.is_empty() {
            return out;
        }
        // Vertically center the block: total height is (lines-1)·leading + size.
        // `line_cy` is each line's vertical center; the first line sits at the
        // top of the centered block.
        let block_h = (lines.len().saturating_sub(1)) as f32 * leading;
        let mut line_cy = -block_h * 0.5;
        // The widest line sets the block's horizontal extent; per-line alignment
        // positions each line within `[-widest/2, +widest/2]`.
        let widest = lines
            .iter()
            .map(|l| self.line_width(l))
            .fold(0.0_f32, f32::max);
        for line in lines {
            let width = self.line_width(line);
            // The pen x where this line's first glyph cell starts, given the
            // line's alignment relative to the centered block.
            let mut pen_x = match self.align {
                TextAlign::Left => -widest * 0.5,
                TextAlign::Center => -width * 0.5,
                TextAlign::Right => widest * 0.5 - width,
            };
            for ch in line.chars() {
                let glyph = glyph(ch);
                // Center the glyph ink (em x≈0.25) within the font advance cell
                // (width `size·ADVANCE_EM`), so a glyph sits centered in its cell
                // regardless of the ink box being narrower than the advance.
                let ink_x = pen_x + (size * ADVANCE_EM * 0.5 - 0.25 * size);
                // Map the em-space glyph box to comp px: x from `ink_x`, y with the
                // line vertically centered on `line_cy` (em y=0 is the cap top,
                // y=1 the baseline; `+y` is downward).
                for &((ax, ay), (bx, by)) in glyph {
                    let map =
                        |gx: f32, gy: f32| (ink_x + gx * size, line_cy - size * 0.5 + gy * size);
                    out.push((map(ax, ay), map(bx, by)));
                }
                pen_x += advance;
            }
            line_cy += leading;
        }
        out
    }

    /// The stroke pen half-width in comp px: an explicit stroke widens the body,
    /// otherwise the fill body is a thin pen (`size·PEN_HALF_EM`).
    pub fn pen_half(&self) -> f32 {
        (self.size * PEN_HALF_EM).max(0.5)
    }

    /// The layer-local axis-aligned bounding box of the laid-out text
    /// `(min_x, min_y, max_x, max_y)`, padded by the pen and any stroke
    /// half-width, or `None` when nothing draws. Bounds the rasterizer's pixel
    /// loop instead of scanning the whole frame.
    pub fn local_bounds(&self) -> Option<(f32, f32, f32, f32)> {
        let segs = self.segments();
        if segs.is_empty() {
            return None;
        }
        let pad =
            self.pen_half() + self.stroke.map(|s| s.width * 0.5).unwrap_or(0.0).max(0.0) + 1.0;
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for ((ax, ay), (bx, by)) in segs {
            for (x, y) in [(ax, ay), (bx, by)] {
                min_x = min_x.min(x - pad);
                min_y = min_y.min(y - pad);
                max_x = max_x.max(x + pad);
                max_y = max_y.max(y + pad);
            }
        }
        Some((min_x, min_y, max_x, max_y))
    }

    /// The straight sRGB RGBA the text contributes at layer-local `(px, py)`,
    /// given its pre-laid-out `segs` (from [`segments`](Self::segments)).
    ///
    /// Coverage is a thickened band around the nearest stroke segment: within the
    /// pen half-width the fill is solid, ramping to zero over a ~1 px AA ramp at
    /// the band edge. When the layer has a stroke, an outline band of the
    /// stroke's width straddles the pen-body edge and is composited over the
    /// fill, so glyphs read as filled-then-outlined just like shapes. Returns
    /// `a = 0` where the point contributes nothing.
    pub fn coverage_at(&self, segs: &[Seg], px: f32, py: f32) -> [f32; 4] {
        if segs.is_empty() {
            return [0.0; 4];
        }
        const AA: f32 = 0.75;
        // Distance to the nearest stroke segment (the glyph skeleton).
        let mut dist = f32::INFINITY;
        for &(a, b) in segs {
            dist = dist.min(dist_to_segment((px, py), a, b));
            if dist <= 0.0 {
                break;
            }
        }
        let pen = self.pen_half();

        let mut out = [0.0f32; 4];
        if let Some(fill) = self.fill {
            // Body coverage: solid where dist <= pen, ramping over ±AA.
            let cov =
                ((pen - dist) / (2.0 * AA) + 0.5).clamp(0.0, 1.0) * fill.opacity.clamp(0.0, 1.0);
            if cov > 0.0 {
                out = [fill.color[0], fill.color[1], fill.color[2], cov];
            }
        }
        if let Some(stroke) = self.stroke {
            if stroke.width > 0.0 {
                // An outline band straddling the pen body's edge (radius `pen`):
                // the band covers |dist - pen| <= width/2.
                let half = stroke.width * 0.5;
                let edge_dist = (dist - pen).abs();
                let band = ((half - edge_dist) / (2.0 * AA) + 0.5).clamp(0.0, 1.0)
                    * stroke.opacity.clamp(0.0, 1.0);
                if band > 0.0 {
                    out = over_straight(
                        [stroke.color[0], stroke.color[1], stroke.color[2], band],
                        out,
                    );
                }
            }
        }
        out
    }

    // ---- Real outline-font path (`font_family == Some(..)`) ---------------
    //
    // Mirrors the stroke-font methods above (`segments` / `local_bounds` /
    // `coverage_at`) but lays the string out into **filled glyph outlines** from a
    // TrueType face and rasterizes them with the *shape layer's* even-odd polygon
    // fill, so a family-selected text layer reads as crisp filled glyphs and
    // composes with the whole pipeline exactly like a shape.

    /// Lay the whole string out into a flat list of **closed glyph contours** in
    /// layer-local space (comp px, origin at the layer's geometric center, `+y`
    /// down) using the selected font family's real outlines. Each contour is a
    /// closed polygon (the closing edge is implicit, as the shape system expects).
    ///
    /// The block is vertically centered about the origin and each line is aligned
    /// horizontally per [`TextAlign`], matching the stroke font's layout so the
    /// two paths sit in the same place. Advances and contours come from the face's
    /// horizontal metrics + glyph outlines (plus the layer's `tracking` /
    /// `leading`); quadratic / cubic glyph curves are flattened to line segments.
    /// Returns an empty list for empty text, a missing family that even the
    /// fallback can't parse, or a non-positive size.
    pub fn outline_contours(&self) -> Vec<Vec<(f32, f32)>> {
        self.outline_layout().map(|l| l.contours).unwrap_or_default()
    }

    /// The layer-local AABB `(min_x, min_y, max_x, max_y)` of the laid-out
    /// **outline** glyphs, padded by any stroke half-width, or `None` when nothing
    /// draws. The outline twin of [`local_bounds`](Self::local_bounds).
    pub fn outline_bounds(&self) -> Option<(f32, f32, f32, f32)> {
        let contours = self.outline_contours();
        let pad = self.stroke.map(|s| s.width * 0.5).unwrap_or(0.0).max(0.0) + 1.0;
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        let mut any = false;
        for contour in &contours {
            for &(x, y) in contour {
                any = true;
                min_x = min_x.min(x - pad);
                min_y = min_y.min(y - pad);
                max_x = max_x.max(x + pad);
                max_y = max_y.max(y + pad);
            }
        }
        any.then_some((min_x, min_y, max_x, max_y))
    }

    /// The straight sRGB RGBA the **outline** text contributes at layer-local
    /// `(px, py)`, given its pre-laid-out `contours` (from
    /// [`outline_contours`](Self::outline_contours)).
    ///
    /// The outline twin of [`coverage_at`](Self::coverage_at): the fill is an
    /// antialiased **even-odd** polygon fill (so glyph holes — the inside of `o`,
    /// `A`, `8` — are carved out) reusing the shape system's
    /// [`point_in_polygon`](super::mask::point_in_polygon) test +
    /// nearest-edge-distance AA, and the optional stroke is a band of `width`
    /// straddling the glyph boundary, composited over the fill. Returns `a = 0`
    /// where the point contributes nothing.
    pub fn outline_coverage_at(&self, contours: &[Vec<(f32, f32)>], px: f32, py: f32) -> [f32; 4] {
        if contours.is_empty() {
            return [0.0; 4];
        }
        const AA: f32 = 0.75;
        // Distance to the nearest contour edge, and even-odd inside-ness folded
        // across every contour (an XOR of per-contour ray casts), so holes count.
        let mut dist = f32::INFINITY;
        let mut inside = false;
        for contour in contours {
            if contour.len() < 3 {
                continue;
            }
            dist = dist.min(dist_to_polygon(contour, px, py));
            if point_in_polygon(contour, px, py) {
                inside = !inside;
            }
        }
        if !dist.is_finite() {
            return [0.0; 4];
        }
        // Signed distance to the glyph boundary: positive inside, negative out.
        let signed = if inside { dist } else { -dist };

        let mut out = [0.0f32; 4];
        if let Some(fill) = self.fill {
            // Fill: 1 well inside, ramping to 0 across ±AA around the boundary —
            // the shape fill's exact recipe.
            let cov = ((signed + AA) / (2.0 * AA)).clamp(0.0, 1.0) * fill.opacity.clamp(0.0, 1.0);
            if cov > 0.0 {
                out = [fill.color[0], fill.color[1], fill.color[2], cov];
            }
        }
        if let Some(stroke) = self.stroke {
            if stroke.width > 0.0 {
                // A band of `width` centered on the boundary: covers |signed| <=
                // width/2 (ramped by AA at each edge of the band).
                let half = stroke.width * 0.5;
                let band = ((half - dist) / (2.0 * AA) + 0.5).clamp(0.0, 1.0)
                    * stroke.opacity.clamp(0.0, 1.0);
                if band > 0.0 {
                    out = over_straight(
                        [stroke.color[0], stroke.color[1], stroke.color[2], band],
                        out,
                    );
                }
            }
        }
        out
    }

    /// Lay the string out into glyph contours + measure each line, using the
    /// resolved face. `None` only when the text/size is empty or even the fallback
    /// face fails to parse (so callers degrade to drawing nothing, never panic).
    fn outline_layout(&self) -> Option<OutlineLayout> {
        let family = self.font_family.as_deref()?;
        let size = self.size.max(0.0);
        if size <= 0.0 || self.text.is_empty() {
            return None;
        }
        let face_bytes = super::fonts::resolve(family);
        let face = ttf_parser::Face::parse(&face_bytes.data, face_bytes.index).ok()?;
        let upem = (face.units_per_em() as f32).max(1.0);
        let scale = size / upem;
        let leading = self.line_leading();

        // Measure + build each line independently (pen at x = 0, baseline at y = 0
        // in line-local space), then place lines: vertically center the block and
        // horizontally align each line within the widest one — the same framing
        // the stroke font uses, so the two paths register.
        let lines: Vec<OutlineLine> = self
            .lines()
            .map(|line| layout_outline_line(&face, line, scale, self.tracking))
            .collect();
        if lines.is_empty() {
            return None;
        }
        let widest = lines.iter().map(|l| l.width).fold(0.0_f32, f32::max);
        let block_h = (lines.len().saturating_sub(1)) as f32 * leading;
        // Each line's baseline y so the cap-to-baseline band straddles `line_cy`.
        // The glyph contours are built y-down with the baseline at line-local
        // y = 0; we shift them so a glyph's vertical mass sits on `line_cy`. Using
        // the face ascent centers the visual block like the stroke font's box.
        let ascent = (face.ascender() as f32).max(upem * 0.8) * scale;
        let descent = (-face.descender() as f32).max(0.0) * scale;
        let half_cap = (ascent - descent) * 0.5;

        let mut contours: Vec<Vec<(f32, f32)>> = Vec::new();
        let mut line_cy = -block_h * 0.5;
        for line in &lines {
            let x_off = match self.align {
                TextAlign::Left => -widest * 0.5,
                TextAlign::Center => -line.width * 0.5,
                TextAlign::Right => widest * 0.5 - line.width,
            };
            // Baseline in layer-local y for this line: line center plus half the
            // cap band so cap-height sits above center and the baseline below.
            let baseline_y = line_cy + half_cap;
            for contour in &line.contours {
                contours.push(
                    contour
                        .iter()
                        .map(|&(x, y)| (x + x_off, y + baseline_y))
                        .collect(),
                );
            }
            line_cy += leading;
        }
        Some(OutlineLayout { contours })
    }
}

/// A laid-out outline block: every glyph contour in layer-local space.
struct OutlineLayout {
    contours: Vec<Vec<(f32, f32)>>,
}

/// One measured outline line: its advance width and glyph contours, with the pen
/// at x = 0 and the baseline at line-local y = 0 (y grows downward).
struct OutlineLine {
    width: f32,
    contours: Vec<Vec<(f32, f32)>>,
}

/// Build one baseline run of outline glyphs: walk the characters, extract each
/// glyph's flattened contours at the running pen x, and advance the pen by the
/// glyph's horizontal advance plus the layer's `tracking`. The trailing tracking
/// past the last glyph is dropped so the line width is tight (matching the stroke
/// font's measure). Contours come back y-down (baseline at y = 0).
fn layout_outline_line(
    face: &ttf_parser::Face,
    line: &str,
    scale: f32,
    tracking: f32,
) -> OutlineLine {
    let mut pen_x = 0.0_f32;
    let mut contours: Vec<Vec<(f32, f32)>> = Vec::new();
    for ch in line.chars() {
        let advance = match face.glyph_index(ch) {
            Some(gid) => {
                if !ch.is_whitespace() {
                    let mut builder = OutlineFlattener::new(pen_x, scale);
                    if face.outline_glyph(gid, &mut builder).is_some() {
                        contours.extend(builder.finish());
                    }
                }
                face.glyph_hor_advance(gid)
                    .map(|a| a as f32 * scale)
                    .unwrap_or_else(|| space_advance(face, scale))
            }
            // No glyph for this char: advance by a space so layout stays sane.
            None => space_advance(face, scale),
        };
        pen_x += advance + tracking;
    }
    // Tight width: total pen travel less the trailing tracking after the last
    // glyph (so a single-char line measures its glyph advance, not advance+track).
    let width = (pen_x - tracking).max(0.0);
    OutlineLine { width, contours }
}

/// A reasonable advance for a missing / whitespace glyph: the face's space
/// advance if it has one, else half the em.
fn space_advance(face: &ttf_parser::Face, scale: f32) -> f32 {
    face.glyph_index(' ')
        .and_then(|g| face.glyph_hor_advance(g))
        .map(|a| a as f32 * scale)
        .unwrap_or(face.units_per_em() as f32 * 0.5 * scale)
}

/// Number of straight chords each glyph curve (quadratic / cubic) is flattened
/// into. Fixed subdivision keeps the polygon fill cheap and deterministic, and is
/// plenty smooth at motion-graphics sizes (mirrors the shape system's `ARC_STEPS`
/// approach).
const CURVE_STEPS: u32 = 8;

/// A [`ttf_parser::OutlineBuilder`] that flattens a glyph's outline into closed
/// layer-local polygons (one per glyph contour). TrueType outlines are y-up;
/// layer space is y-down with the baseline at y = 0, so every y is negated. The
/// pen-x offset and em→px `scale` are folded in as points arrive. Quadratic and
/// cubic segments are tessellated into [`CURVE_STEPS`] line chords each, so the
/// shape system's polygon fill consumes the result unchanged.
struct OutlineFlattener {
    pen_x: f32,
    scale: f32,
    /// Completed contours.
    done: Vec<Vec<(f32, f32)>>,
    /// Current contour's points (layer-local).
    cur: Vec<(f32, f32)>,
    /// Last emitted point (the current pen position), for curve starts.
    last: (f32, f32),
}

impl OutlineFlattener {
    fn new(pen_x: f32, scale: f32) -> Self {
        Self {
            pen_x,
            scale,
            done: Vec::new(),
            cur: Vec::new(),
            last: (0.0, 0.0),
        }
    }

    /// Map a font-space point to layer space (apply pen offset + scale, flip y).
    fn map(&self, x: f32, y: f32) -> (f32, f32) {
        (self.pen_x + x * self.scale, -y * self.scale)
    }

    /// Finish the in-progress contour (if any) and return every contour built.
    fn finish(mut self) -> Vec<Vec<(f32, f32)>> {
        self.flush();
        self.done
    }

    /// Close out the current contour into `done` (glyph contours are closed
    /// regions; the polygon fill closes the implicit last edge). Drops degenerate
    /// (< 3-point) contours.
    fn flush(&mut self) {
        if self.cur.len() >= 3 {
            self.done.push(std::mem::take(&mut self.cur));
        } else {
            self.cur.clear();
        }
    }
}

impl ttf_parser::OutlineBuilder for OutlineFlattener {
    fn move_to(&mut self, x: f32, y: f32) {
        // Starting a new contour: flush any previous one.
        self.flush();
        let p = self.map(x, y);
        self.cur.push(p);
        self.last = p;
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.map(x, y);
        self.cur.push(p);
        self.last = p;
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let a = self.last;
        let ctrl = self.map(x1, y1);
        let end = self.map(x, y);
        // Tessellate the quadratic Bézier into line chords.
        for s in 1..=CURVE_STEPS {
            let t = s as f32 / CURVE_STEPS as f32;
            let mt = 1.0 - t;
            let bx = mt * mt * a.0 + 2.0 * mt * t * ctrl.0 + t * t * end.0;
            let by = mt * mt * a.1 + 2.0 * mt * t * ctrl.1 + t * t * end.1;
            self.cur.push((bx, by));
        }
        self.last = end;
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let a = self.last;
        let c1 = self.map(x1, y1);
        let c2 = self.map(x2, y2);
        let end = self.map(x, y);
        // Tessellate the cubic Bézier into line chords.
        for s in 1..=CURVE_STEPS {
            let t = s as f32 / CURVE_STEPS as f32;
            let mt = 1.0 - t;
            let bx = mt * mt * mt * a.0
                + 3.0 * mt * mt * t * c1.0
                + 3.0 * mt * t * t * c2.0
                + t * t * t * end.0;
            let by = mt * mt * mt * a.1
                + 3.0 * mt * mt * t * c1.1
                + 3.0 * mt * t * t * c2.1
                + t * t * t * end.1;
            self.cur.push((bx, by));
        }
        self.last = end;
    }

    fn close(&mut self) {
        self.flush();
    }
}

/// Straight (non-premultiplied) source-over of `src` over `dst`, both
/// `[r, g, b, a]` with `a` as coverage. (Mirrors the shape system's blend.)
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

/// The stroke skeleton of one character in the built-in font, as a slice of
/// segments on the unit em grid: x in `[0, GLYPH_W]`, y in `[0, 1]` with `y = 0`
/// the cap line (top) and `y = 1` the baseline (bottom). Letters are uppercased;
/// unknown printable characters fall back to a small box, and space/control
/// characters draw nothing.
///
/// The font is a simple humanist sans skeleton — enough strokes to read clearly
/// at motion-graphics sizes, authored once here so text needs no font
/// dependency.
fn glyph(ch: char) -> &'static [Seg] {
    // Convenience corners on the em grid.
    let up = ch.to_ascii_uppercase();
    match up {
        ' ' => &[],
        'A' => A,
        'B' => B,
        'C' => C,
        'D' => D,
        'E' => E,
        'F' => F,
        'G' => G,
        'H' => H,
        'I' => I,
        'J' => J,
        'K' => K,
        'L' => L,
        'M' => M,
        'N' => N,
        'O' => O,
        'P' => P,
        'Q' => Q,
        'R' => R,
        'S' => S,
        'T' => T,
        'U' => U,
        'V' => V,
        'W' => W,
        'X' => X,
        'Y' => Y,
        'Z' => Z,
        '0' => O, // share the O ring
        '1' => ONE,
        '2' => TWO,
        '3' => THREE,
        '4' => FOUR,
        '5' => S, // 5 reads like S
        '6' => SIX,
        '7' => SEVEN,
        '8' => EIGHT,
        '9' => NINE,
        '.' => DOT,
        ',' => DOT,
        '!' => EXCLAIM,
        '?' => QUESTION,
        '-' => DASH,
        '_' => UNDERSCORE,
        ':' => COLON,
        '/' => SLASH,
        '+' => PLUS,
        '=' => EQUALS,
        '\t' => &[],
        c if c.is_control() => &[],
        _ => FALLBACK,
    }
}

// Glyph geometry. Each is a slice of segments in the `[0, 0.5] × [0, 1]` em box.
// y grows downward (0 = top / cap, 1 = bottom / baseline).
const L0: f32 = 0.04; // left margin
const R0: f32 = 0.46; // right margin
const MX: f32 = 0.25; // horizontal center
const TOP: f32 = 0.05;
const MID: f32 = 0.5;
const BOT: f32 = 0.95;

#[rustfmt::skip]
const A: &[Seg] = &[
    ((L0, BOT), (MX, TOP)), ((MX, TOP), (R0, BOT)), ((0.13, MID + 0.12), (0.37, MID + 0.12)),
];
#[rustfmt::skip]
const B: &[Seg] = &[
    ((L0, TOP), (L0, BOT)), ((L0, TOP), (0.34, TOP)), ((0.34, TOP), (R0, 0.22)),
    ((R0, 0.22), (0.34, MID)), ((L0, MID), (0.34, MID)), ((0.34, MID), (R0, 0.72)),
    ((R0, 0.72), (0.34, BOT)), ((L0, BOT), (0.34, BOT)),
];
#[rustfmt::skip]
const C: &[Seg] = &[
    ((R0, 0.2), (MX, TOP)), ((MX, TOP), (L0, 0.3)), ((L0, 0.3), (L0, 0.7)),
    ((L0, 0.7), (MX, BOT)), ((MX, BOT), (R0, 0.8)),
];
#[rustfmt::skip]
const D: &[Seg] = &[
    ((L0, TOP), (L0, BOT)), ((L0, TOP), (0.28, TOP)), ((0.28, TOP), (R0, 0.3)),
    ((R0, 0.3), (R0, 0.7)), ((R0, 0.7), (0.28, BOT)), ((L0, BOT), (0.28, BOT)),
];
#[rustfmt::skip]
const E: &[Seg] = &[
    ((R0, TOP), (L0, TOP)), ((L0, TOP), (L0, BOT)), ((L0, MID), (0.36, MID)),
    ((L0, BOT), (R0, BOT)),
];
#[rustfmt::skip]
const F: &[Seg] = &[
    ((R0, TOP), (L0, TOP)), ((L0, TOP), (L0, BOT)), ((L0, MID), (0.36, MID)),
];
#[rustfmt::skip]
const G: &[Seg] = &[
    ((R0, 0.2), (MX, TOP)), ((MX, TOP), (L0, 0.3)), ((L0, 0.3), (L0, 0.7)),
    ((L0, 0.7), (MX, BOT)), ((MX, BOT), (R0, 0.7)), ((R0, 0.7), (R0, MID)),
    ((R0, MID), (0.3, MID)),
];
#[rustfmt::skip]
const H: &[Seg] = &[
    ((L0, TOP), (L0, BOT)), ((R0, TOP), (R0, BOT)), ((L0, MID), (R0, MID)),
];
#[rustfmt::skip]
const I: &[Seg] = &[
    ((MX, TOP), (MX, BOT)), ((0.13, TOP), (0.37, TOP)), ((0.13, BOT), (0.37, BOT)),
];
#[rustfmt::skip]
const J: &[Seg] = &[
    ((R0, TOP), (R0, 0.78)), ((R0, 0.78), (MX, BOT)), ((MX, BOT), (L0, 0.78)),
];
#[rustfmt::skip]
const K: &[Seg] = &[
    ((L0, TOP), (L0, BOT)), ((R0, TOP), (L0, MID)), ((L0, MID), (R0, BOT)),
];
#[rustfmt::skip]
const L: &[Seg] = &[
    ((L0, TOP), (L0, BOT)), ((L0, BOT), (R0, BOT)),
];
#[rustfmt::skip]
const M: &[Seg] = &[
    ((L0, BOT), (L0, TOP)), ((L0, TOP), (MX, MID)), ((MX, MID), (R0, TOP)),
    ((R0, TOP), (R0, BOT)),
];
#[rustfmt::skip]
const N: &[Seg] = &[
    ((L0, BOT), (L0, TOP)), ((L0, TOP), (R0, BOT)), ((R0, BOT), (R0, TOP)),
];
#[rustfmt::skip]
const O: &[Seg] = &[
    ((MX, TOP), (R0, 0.3)), ((R0, 0.3), (R0, 0.7)), ((R0, 0.7), (MX, BOT)),
    ((MX, BOT), (L0, 0.7)), ((L0, 0.7), (L0, 0.3)), ((L0, 0.3), (MX, TOP)),
];
#[rustfmt::skip]
const P: &[Seg] = &[
    ((L0, BOT), (L0, TOP)), ((L0, TOP), (0.34, TOP)), ((0.34, TOP), (R0, 0.2)),
    ((R0, 0.2), (0.34, MID)), ((0.34, MID), (L0, MID)),
];
#[rustfmt::skip]
const Q: &[Seg] = &[
    ((MX, TOP), (R0, 0.3)), ((R0, 0.3), (R0, 0.7)), ((R0, 0.7), (MX, BOT)),
    ((MX, BOT), (L0, 0.7)), ((L0, 0.7), (L0, 0.3)), ((L0, 0.3), (MX, TOP)),
    ((0.3, 0.7), (R0, BOT)),
];
#[rustfmt::skip]
const R: &[Seg] = &[
    ((L0, BOT), (L0, TOP)), ((L0, TOP), (0.34, TOP)), ((0.34, TOP), (R0, 0.2)),
    ((R0, 0.2), (0.34, MID)), ((0.34, MID), (L0, MID)), ((0.28, MID), (R0, BOT)),
];
#[rustfmt::skip]
const S: &[Seg] = &[
    ((R0, 0.2), (MX, TOP)), ((MX, TOP), (L0, 0.25)), ((L0, 0.25), (R0, MID)),
    ((R0, MID), (R0, 0.7)), ((R0, 0.7), (MX, BOT)), ((MX, BOT), (L0, 0.8)),
];
#[rustfmt::skip]
const T: &[Seg] = &[
    ((L0, TOP), (R0, TOP)), ((MX, TOP), (MX, BOT)),
];
#[rustfmt::skip]
const U: &[Seg] = &[
    ((L0, TOP), (L0, 0.72)), ((L0, 0.72), (0.16, BOT)), ((0.16, BOT), (0.34, BOT)),
    ((0.34, BOT), (R0, 0.72)), ((R0, 0.72), (R0, TOP)),
];
#[rustfmt::skip]
const V: &[Seg] = &[
    ((L0, TOP), (MX, BOT)), ((MX, BOT), (R0, TOP)),
];
#[rustfmt::skip]
const W: &[Seg] = &[
    ((L0, TOP), (0.16, BOT)), ((0.16, BOT), (MX, MID)), ((MX, MID), (0.34, BOT)),
    ((0.34, BOT), (R0, TOP)),
];
#[rustfmt::skip]
const X: &[Seg] = &[
    ((L0, TOP), (R0, BOT)), ((R0, TOP), (L0, BOT)),
];
#[rustfmt::skip]
const Y: &[Seg] = &[
    ((L0, TOP), (MX, MID)), ((R0, TOP), (MX, MID)), ((MX, MID), (MX, BOT)),
];
#[rustfmt::skip]
const Z: &[Seg] = &[
    ((L0, TOP), (R0, TOP)), ((R0, TOP), (L0, BOT)), ((L0, BOT), (R0, BOT)),
];
#[rustfmt::skip]
const ONE: &[Seg] = &[
    ((0.16, 0.2), (MX, TOP)), ((MX, TOP), (MX, BOT)), ((0.13, BOT), (0.37, BOT)),
];
#[rustfmt::skip]
const TWO: &[Seg] = &[
    ((L0, 0.22), (MX, TOP)), ((MX, TOP), (R0, 0.25)), ((R0, 0.25), (L0, BOT)),
    ((L0, BOT), (R0, BOT)),
];
#[rustfmt::skip]
const THREE: &[Seg] = &[
    ((L0, TOP), (R0, TOP)), ((R0, TOP), (0.28, MID)), ((0.28, MID), (R0, 0.7)),
    ((R0, 0.7), (MX, BOT)), ((MX, BOT), (L0, 0.8)),
];
#[rustfmt::skip]
const FOUR: &[Seg] = &[
    ((0.34, BOT), (0.34, TOP)), ((0.34, TOP), (L0, 0.6)), ((L0, 0.6), (R0, 0.6)),
];
#[rustfmt::skip]
const SIX: &[Seg] = &[
    ((R0, 0.2), (MX, TOP)), ((MX, TOP), (L0, MID)), ((L0, MID), (L0, 0.8)),
    ((L0, 0.8), (MX, BOT)), ((MX, BOT), (R0, 0.75)), ((R0, 0.75), (0.3, MID)),
    ((0.3, MID), (L0, MID)),
];
#[rustfmt::skip]
const SEVEN: &[Seg] = &[
    ((L0, TOP), (R0, TOP)), ((R0, TOP), (0.2, BOT)),
];
#[rustfmt::skip]
const EIGHT: &[Seg] = &[
    ((MX, TOP), (R0, 0.2)), ((R0, 0.2), (MX, MID)), ((MX, MID), (L0, 0.2)),
    ((L0, 0.2), (MX, TOP)), ((MX, MID), (R0, 0.72)), ((R0, 0.72), (MX, BOT)),
    ((MX, BOT), (L0, 0.72)), ((L0, 0.72), (MX, MID)),
];
#[rustfmt::skip]
const NINE: &[Seg] = &[
    ((0.3, MID), (L0, 0.25)), ((L0, 0.25), (MX, TOP)), ((MX, TOP), (R0, MID)),
    ((R0, MID), (R0, 0.5)), ((MX, TOP), (R0, MID)), ((R0, MID), (MX, BOT)),
    ((MX, BOT), (L0, 0.8)),
];
#[rustfmt::skip]
const DOT: &[Seg] = &[
    ((MX, 0.9), (MX, BOT)),
];
#[rustfmt::skip]
const EXCLAIM: &[Seg] = &[
    ((MX, TOP), (MX, 0.65)), ((MX, 0.88), (MX, BOT)),
];
#[rustfmt::skip]
const QUESTION: &[Seg] = &[
    ((L0, 0.22), (MX, TOP)), ((MX, TOP), (R0, 0.25)), ((R0, 0.25), (MX, MID)),
    ((MX, MID), (MX, 0.65)), ((MX, 0.88), (MX, BOT)),
];
#[rustfmt::skip]
const DASH: &[Seg] = &[
    ((0.12, MID), (0.38, MID)),
];
#[rustfmt::skip]
const UNDERSCORE: &[Seg] = &[
    ((L0, BOT), (R0, BOT)),
];
#[rustfmt::skip]
const COLON: &[Seg] = &[
    ((MX, 0.35), (MX, 0.45)), ((MX, 0.78), (MX, 0.88)),
];
#[rustfmt::skip]
const SLASH: &[Seg] = &[
    ((L0, BOT), (R0, TOP)),
];
#[rustfmt::skip]
const PLUS: &[Seg] = &[
    ((MX, 0.3), (MX, 0.7)), ((0.13, MID), (0.37, MID)),
];
#[rustfmt::skip]
const EQUALS: &[Seg] = &[
    ((0.12, 0.4), (0.38, 0.4)), ((0.12, 0.6), (0.38, 0.6)),
];
#[rustfmt::skip]
const FALLBACK: &[Seg] = &[
    ((L0, TOP), (R0, TOP)), ((R0, TOP), (R0, BOT)), ((R0, BOT), (L0, BOT)),
    ((L0, BOT), (L0, TOP)),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_text_draws_something() {
        let t = TextLayer::default();
        assert!(!t.is_empty());
        assert!(!t.segments().is_empty());
        assert!(t.local_bounds().is_some());
    }

    #[test]
    fn empty_or_blank_text_is_empty() {
        let mut t = TextLayer::default();
        t.text = "   \n  ".to_string();
        assert!(t.is_empty());
        assert!(t.segments().is_empty());
        assert!(t.local_bounds().is_none());
    }

    #[test]
    fn no_fill_no_stroke_is_empty() {
        let mut t = TextLayer::default();
        t.fill = None;
        t.stroke = None;
        assert!(t.is_empty());
    }

    #[test]
    fn space_draws_no_strokes_but_advances() {
        // "A A" lays out wider than "AA" by one advance, and the space adds no
        // segments of its own.
        let mut a_space_a = TextLayer::default();
        a_space_a.text = "A A".to_string();
        let mut aa = TextLayer::default();
        aa.text = "AA".to_string();
        assert!(a_space_a.line_width("A A") > aa.line_width("AA"));
        // Same number of A glyphs → same segment count (space contributes none).
        assert_eq!(a_space_a.segments().len(), aa.segments().len());
    }

    #[test]
    fn unknown_char_falls_back_to_box() {
        // A char with no glyph entry draws the 4-segment fallback box.
        assert_eq!(glyph('~').len(), 4);
        assert_eq!(glyph('@').len(), 4);
        // Control chars draw nothing.
        assert!(glyph('\n').is_empty());
    }

    #[test]
    fn letters_are_case_insensitive() {
        assert_eq!(glyph('a').len(), glyph('A').len());
        assert_eq!(glyph('z').len(), glyph('Z').len());
    }

    #[test]
    fn tracking_widens_the_line() {
        let mut t = TextLayer::default();
        t.text = "ABC".to_string();
        let base = t.line_width("ABC");
        t.tracking = 20.0;
        assert!(t.line_width("ABC") > base);
        // Two extra gaps between three glyphs = 2·tracking added.
        assert!((t.line_width("ABC") - base - 40.0).abs() < 1e-3);
    }

    #[test]
    fn multiline_block_is_vertically_centered() {
        let mut t = TextLayer::default();
        t.text = "AB\nCD".to_string();
        let segs = t.segments();
        assert!(!segs.is_empty());
        // The block straddles y=0: there is geometry above and below the origin.
        let min_y = segs
            .iter()
            .flat_map(|&(a, b)| [a.1, b.1])
            .fold(f32::INFINITY, f32::min);
        let max_y = segs
            .iter()
            .flat_map(|&(a, b)| [a.1, b.1])
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(
            min_y < 0.0 && max_y > 0.0,
            "block centered: {min_y}..{max_y}"
        );
    }

    #[test]
    fn alignment_shifts_lines() {
        // A short line over a long line: left-align starts both at the same x,
        // right-align ends both at the same x.
        let mut left = TextLayer::default();
        left.text = "I\nWWWW".to_string();
        left.align = TextAlign::Left;
        let mut right = left.clone();
        right.align = TextAlign::Right;

        let min_x = |t: &TextLayer| {
            t.segments()
                .iter()
                .flat_map(|&(a, b)| [a.0, b.0])
                .fold(f32::INFINITY, f32::min)
        };
        let max_x = |t: &TextLayer| {
            t.segments()
                .iter()
                .flat_map(|&(a, b)| [a.0, b.0])
                .fold(f32::NEG_INFINITY, f32::max)
        };
        // Left-aligned block hugs the left; right-aligned hugs the right.
        assert!(max_x(&right) > max_x(&left) - 1.0);
        assert!(min_x(&left) < min_x(&right) + 1.0);
    }

    #[test]
    fn coverage_is_solid_on_a_stroke_and_clear_far_away() {
        let mut t = TextLayer::default();
        t.text = "I".to_string(); // a vertical bar through the center
        let segs = t.segments();
        // A point on the central vertical stroke is covered.
        let on = t.coverage_at(&segs, 0.0, 0.0);
        assert!(on[3] > 0.5, "on-stroke covered, got {}", on[3]);
        // A point far to the side is clear.
        let off = t.coverage_at(&segs, 10_000.0, 0.0);
        assert_eq!(off[3], 0.0);
    }

    #[test]
    fn fill_opacity_scales_coverage() {
        let mut t = TextLayer::default();
        t.text = "I".to_string();
        t.fill = Some(Fill {
            color: [1.0, 0.0, 0.0],
            opacity: 0.5,
        });
        let segs = t.segments();
        let c = t.coverage_at(&segs, 0.0, 0.0);
        assert!(
            (c[3] - 0.5).abs() < 1e-2,
            "alpha tracks opacity, got {}",
            c[3]
        );
        assert_eq!(c[0], 1.0);
    }

    #[test]
    fn stroke_outlines_the_body() {
        // A stroked glyph reads the stroke color in a band just outside the pen
        // body edge.
        let mut t = TextLayer::default();
        t.text = "I".to_string();
        t.fill = Some(Fill {
            color: [1.0, 0.0, 0.0],
            opacity: 1.0,
        });
        t.stroke = Some(Stroke {
            color: [0.0, 0.0, 1.0],
            width: 8.0,
            opacity: 1.0,
        });
        let segs = t.segments();
        // Interior of the pen body: the red fill.
        let center = t.coverage_at(&segs, 0.0, 0.0);
        assert!(center[0] > center[2], "body is red, got {center:?}");
        // Just outside the body edge (pen_half + a couple px): the blue stroke.
        let edge_x = t.pen_half() + 2.0;
        let edge = t.coverage_at(&segs, edge_x, 0.0);
        assert!(
            edge[3] > 0.0 && edge[2] > edge[0],
            "outline is blue, got {edge:?}"
        );
    }

    #[test]
    fn local_bounds_grows_with_size() {
        let mut small = TextLayer::default();
        small.text = "WIDE".to_string();
        small.size = 40.0;
        let mut big = small.clone();
        big.size = 200.0;
        let (sx0, _, sx1, _) = small.local_bounds().unwrap();
        let (bx0, _, bx1, _) = big.local_bounds().unwrap();
        assert!((bx1 - bx0) > (sx1 - sx0), "bigger size → wider bounds");
    }

    #[test]
    fn leading_auto_and_explicit() {
        let mut t = TextLayer::default();
        t.size = 100.0;
        assert!((t.line_leading() - 120.0).abs() < 1e-3, "auto = size·1.2");
        t.leading = 80.0;
        assert!((t.line_leading() - 80.0).abs() < 1e-3, "explicit honored");
    }

    // ---- Real outline-font path -----------------------------------------

    /// A new layer (and every legacy file) defaults `font_family` to `None`, which
    /// selects the built-in **stroke** path, not the outline path.
    #[test]
    fn default_is_stroke_font_not_outline() {
        let t = TextLayer::default();
        assert_eq!(t.font_family, None);
        assert!(!t.uses_outline(), "None → built-in stroke font");
        // The stroke path lays out; the outline path is empty (no family).
        assert!(!t.segments().is_empty());
        assert!(t.outline_contours().is_empty());
    }

    /// Selecting a family flips the layer to the outline path and produces real
    /// glyph contours (closed polygons), while the stroke layout is irrelevant.
    #[test]
    fn some_family_selects_outline_path() {
        let mut t = TextLayer::default();
        t.text = "A".to_string();
        t.font_family = Some("Ubuntu".to_string());
        assert!(t.uses_outline(), "Some(family) → outline path");
        let contours = t.outline_contours();
        assert!(!contours.is_empty(), "letter A produces outline contours");
        assert!(
            contours.iter().any(|c| c.len() >= 3),
            "at least one contour has real geometry"
        );
    }

    /// An unknown family falls back to the bundled face and still renders glyphs —
    /// text never vanishes.
    #[test]
    fn unknown_family_falls_back_to_bundled_glyphs() {
        let mut t = TextLayer::default();
        t.text = "A".to_string();
        t.font_family = Some("No Such Font 99999".to_string());
        assert!(t.uses_outline());
        assert!(
            !t.outline_contours().is_empty(),
            "unknown family still renders via the fallback face"
        );
    }

    /// Outline layout width is **monotonic** (more / wider text is wider) and
    /// **deterministic** (the same input lays out identically every call).
    #[test]
    fn outline_layout_advance_is_monotonic_and_deterministic() {
        let mk = |s: &str| {
            let mut t = TextLayer::default();
            t.text = s.to_string();
            t.size = 100.0;
            t.font_family = Some("Ubuntu".to_string());
            t.outline_layout().expect("layout").contours
        };
        let span_x = |contours: &[Vec<(f32, f32)>]| {
            let min = contours
                .iter()
                .flat_map(|c| c.iter())
                .map(|p| p.0)
                .fold(f32::INFINITY, f32::min);
            let max = contours
                .iter()
                .flat_map(|c| c.iter())
                .map(|p| p.0)
                .fold(f32::NEG_INFINITY, f32::max);
            max - min
        };
        let one = mk("W");
        let many = mk("WWW");
        assert!(
            span_x(&many) > span_x(&one),
            "more glyphs lay out wider: {} vs {}",
            span_x(&many),
            span_x(&one)
        );
        // Determinism: identical input → identical geometry.
        let a = mk("Ag");
        let b = mk("Ag");
        assert_eq!(a, b, "outline layout is deterministic");
    }

    /// Doubling the font size roughly doubles the outline advance (metrics scale).
    #[test]
    fn outline_advance_scales_with_size() {
        let span = |size: f32| {
            let mut t = TextLayer::default();
            t.text = "Ag".to_string();
            t.size = size;
            t.font_family = Some("Ubuntu".to_string());
            let contours = t.outline_contours();
            let min = contours
                .iter()
                .flat_map(|c| c.iter())
                .map(|p| p.0)
                .fold(f32::INFINITY, f32::min);
            let max = contours
                .iter()
                .flat_map(|c| c.iter())
                .map(|p| p.0)
                .fold(f32::NEG_INFINITY, f32::max);
            max - min
        };
        let small = span(40.0);
        let big = span(80.0);
        assert!(
            (big - small * 2.0).abs() < small * 0.1,
            "doubling size ~doubles the outline width: {small} -> {big}"
        );
    }

    /// A glyph with a hole ("o") yields an interior point that is *outside* the
    /// even-odd fill — proving the counter is carved out, not filled solid.
    #[test]
    fn outline_fill_carves_glyph_holes() {
        let mut t = TextLayer::default();
        t.text = "o".to_string();
        t.size = 200.0;
        t.font_family = Some("Ubuntu".to_string());
        let contours = t.outline_contours();
        assert!(contours.len() >= 2, "o has an outer + inner contour");
        // The glyph ink sits roughly at the layer center; the very center of "o"
        // falls inside the hole, so coverage there is clear.
        let center = t.outline_coverage_at(&contours, 0.0, 0.0);
        assert_eq!(center[3], 0.0, "center of 'o' is the carved hole");
    }

    /// The outline fill is solid on glyph ink and clear far outside.
    #[test]
    fn outline_coverage_solid_on_ink_clear_outside() {
        let mut t = TextLayer::default();
        t.text = "I".to_string();
        t.size = 200.0;
        t.font_family = Some("Ubuntu".to_string());
        let contours = t.outline_contours();
        // The vertical bar of "I" passes through the layer center: solid fill.
        let on = t.outline_coverage_at(&contours, 0.0, 0.0);
        assert!(on[3] > 0.5, "on-ink covered, got {}", on[3]);
        // Far to the side: clear.
        let off = t.outline_coverage_at(&contours, 10_000.0, 0.0);
        assert_eq!(off[3], 0.0);
    }

    /// `outline_bounds` grows with the font size, like the stroke path's bounds.
    #[test]
    fn outline_bounds_grows_with_size() {
        let mut small = TextLayer::default();
        small.text = "WIDE".to_string();
        small.size = 40.0;
        small.font_family = Some("Ubuntu".to_string());
        let mut big = small.clone();
        big.size = 200.0;
        let (sx0, _, sx1, _) = small.outline_bounds().unwrap();
        let (bx0, _, bx1, _) = big.outline_bounds().unwrap();
        assert!((bx1 - bx0) > (sx1 - sx0), "bigger size → wider outline bounds");
    }

    #[test]
    fn serde_round_trips() {
        let mut t = TextLayer::default();
        t.text = "HELLO\nWORLD!".to_string();
        t.align = TextAlign::Center;
        t.font_family = Some("Helvetica".to_string());
        t.stroke = Some(Stroke::default());
        let json = serde_json::to_string(&t).unwrap();
        let back: TextLayer = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }

    /// A legacy `.pulse` text layer — written before font selection existed, so it
    /// has no `font_family` key — deserializes with `font_family = None`, i.e. the
    /// built-in stroke font, so old projects render identically.
    #[test]
    fn legacy_text_without_font_family_defaults_to_stroke_font() {
        let legacy = r#"{
            "text": "OLD",
            "size": 120.0,
            "tracking": 0.0,
            "leading": 0.0,
            "align": "Left",
            "fill": { "color": [1.0, 1.0, 1.0], "opacity": 1.0 },
            "stroke": null
        }"#;
        let t: TextLayer = serde_json::from_str(legacy).unwrap();
        assert_eq!(t.font_family, None, "absent key → None");
        assert!(!t.uses_outline(), "legacy file keeps the stroke font");
        // It lays out via the stroke path exactly as it always did.
        assert!(!t.segments().is_empty());
        assert!(t.outline_contours().is_empty());
    }
}
