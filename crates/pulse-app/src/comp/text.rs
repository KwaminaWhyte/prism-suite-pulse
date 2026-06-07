//! Text layers: a string laid out into glyphs drawn by a self-contained,
//! dependency-free **stroke vector font**.
//!
//! A [`TextLayer`] is a string plus type settings (font size, tracking, leading,
//! alignment) and a [`Fill`](super::shape::Fill) / [`Stroke`](super::shape::Stroke)
//! reused from the shape system. Each visible character maps to a small set of
//! **polyline strokes** authored on a unit em grid (`glyph` below); the renderer
//! scales those strokes by the font size, lays them out left-to-right with
//! per-line alignment, and rasterizes coverage as a thickened band around the
//! nearest stroke (a "line font"), antialiased the same way shapes are.
//!
//! Everything here is pure and time-agnostic — the layout produces layer-local
//! geometry that rides the layer's transform (position / scale / rotation /
//! parent) exactly like a shape layer, so text composes with masks, mattes,
//! spatial effects, and motion blur for free. There is no font-shaping
//! dependency: the built-in font is intentionally simple (uppercased, monospace
//! cell) so the feature is self-contained and unit-testable. Per-character
//! animators and real OpenType/variable fonts are a later step.

use super::mask::dist_to_segment;
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

    #[test]
    fn serde_round_trips() {
        let mut t = TextLayer::default();
        t.text = "HELLO\nWORLD!".to_string();
        t.align = TextAlign::Center;
        t.stroke = Some(Stroke::default());
        let json = serde_json::to_string(&t).unwrap();
        let back: TextLayer = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }
}
