//! Footage layers: a still image or a numbered image sequence sampled at comp
//! time `t`, decoded through `prism-io` and rasterized into the compositor's
//! premultiplied-free linear-light buffer exactly like a solid quad.
//!
//! A [`FootageLayer`] points at a [`FootageSource`] on disk — either a single
//! still (constant over time) or a numbered **image sequence** (one file per
//! frame, e.g. `frame_0001.png`). At comp time `t` the sequence's frame index is
//! derived from an optional `fps` override (defaulting to the comp's fps), with
//! **loop** or **hold-last** behaviour past the end (see [`FootageSource::frame_index`]).
//!
//! Decoding is deferred to render time and goes through a [`FrameCache`] so a
//! given (path, frame) is decoded at most once per render pass and reused across
//! the many comp frames that reference the same source frame. Decoded pixels are
//! converted sRGB → linear at the gamma boundary (matching the solid / shape /
//! text paths), so footage enters the compositor in the same space as everything
//! else.
//!
//! Real **video decode (FFmpeg)** is deliberately out of scope this pass; it
//! needs the shared `prism-media` crate and is the natural follow-on — see
//! `PLAN.md` Phase 2.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use prism_core::color::srgb_to_linear;
use serde::{Deserialize, Serialize};

/// How a footage layer's pixels' alpha is interpreted as it enters the
/// compositor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlphaMode {
    /// Use the file's own alpha channel as straight (non-premultiplied)
    /// coverage. The default for files that carry alpha (PNG / TIFF / …).
    #[default]
    Straight,
    /// The file's RGB is premultiplied against its alpha; un-premultiply on load
    /// so the compositor (which carries straight color + coverage) sees straight
    /// color.
    Premultiplied,
    /// Ignore the file's alpha and treat the image as fully opaque (a flattened
    /// still). Useful for JPEGs or to force a footage matte off.
    Ignore,
}

impl AlphaMode {
    pub const ALL: [AlphaMode; 3] = [
        AlphaMode::Straight,
        AlphaMode::Premultiplied,
        AlphaMode::Ignore,
    ];

    pub fn label(self) -> &'static str {
        match self {
            AlphaMode::Straight => "Straight (unmatted)",
            AlphaMode::Premultiplied => "Premultiplied",
            AlphaMode::Ignore => "Ignore (opaque)",
        }
    }
}

/// Where a footage layer gets its pixels.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FootageSource {
    /// A single still image: constant over the whole comp timeline.
    Still { path: PathBuf },
    /// A numbered **image sequence**: one file per frame. `pattern` is a
    /// printf-style template with exactly one `{}` placeholder where the
    /// zero-padded frame number goes (e.g. `"frames/shot_{}.png"`), `pad` is the
    /// number's zero-padded width (4 → `0001`), `start` is the file number of the
    /// sequence's first frame, and `count` is how many frames exist on disk.
    Sequence {
        pattern: String,
        pad: usize,
        start: u32,
        count: u32,
    },
}

impl FootageSource {
    /// A single still from a path.
    pub fn still(path: impl Into<PathBuf>) -> Self {
        FootageSource::Still { path: path.into() }
    }

    /// A short, stable label for the UI.
    pub fn kind_label(&self) -> &'static str {
        match self {
            FootageSource::Still { .. } => "Still",
            FootageSource::Sequence { .. } => "Image sequence",
        }
    }

    /// A human-readable display path for the UI (the still's path, or the
    /// sequence's pattern).
    pub fn display(&self) -> String {
        match self {
            FootageSource::Still { path } => path.display().to_string(),
            FootageSource::Sequence { pattern, .. } => pattern.clone(),
        }
    }

    /// Resolve the on-disk path for the source-frame at sequence index `seq`
    /// (0-based, already loop/hold-resolved). A still ignores `seq`.
    pub fn path_for(&self, seq: u32) -> PathBuf {
        match self {
            FootageSource::Still { path } => path.clone(),
            FootageSource::Sequence {
                pattern, pad, start, ..
            } => {
                let num = start + seq;
                let num_str = format!("{num:0pad$}", pad = *pad);
                PathBuf::from(pattern.replacen("{}", &num_str, 1))
            }
        }
    }

    /// How many distinct source frames this source has (1 for a still).
    pub fn len(&self) -> u32 {
        match self {
            FootageSource::Still { .. } => 1,
            FootageSource::Sequence { count, .. } => (*count).max(1),
        }
    }

    /// Resolve the 0-based **source-frame index** to display at comp time `t`
    /// (seconds), given the layer's playback options.
    ///
    /// The raw frame number is `floor(t * fps)`, where `fps` is the layer's
    /// override (or the comp fps when unset). For a still this is always `0`.
    /// Past the last frame the sequence either **loops** (modulo wrap) or
    /// **holds** the last frame, per `looping` / `hold_last`; before `t = 0` it
    /// holds the first frame. When neither loop nor hold is set, frames past the
    /// end clamp to the last (a sensible default rather than vanishing).
    pub fn frame_index(&self, t: f32, fps: f32, looping: bool, hold_last: bool) -> u32 {
        let count = self.len();
        if count <= 1 {
            return 0;
        }
        // Frame number at this time, never negative (hold the first frame before 0).
        let raw = (t.max(0.0) * fps.max(0.0)).floor();
        let raw = if raw.is_finite() { raw as i64 } else { 0 };
        let n = count as i64;
        if raw < n {
            return raw.max(0) as u32;
        }
        if looping {
            return (raw.rem_euclid(n)) as u32;
        }
        // Past the end and not looping: hold the last frame (whether or not
        // `hold_last` is explicitly set — clamping is the safe default).
        let _ = hold_last;
        (n - 1) as u32
    }
}

/// The footage-specific fields of a [`PulseLayer`](super::PulseLayer), drawn only
/// when the layer's [`kind`](super::PulseLayer::kind) is
/// [`Footage`](super::LayerKind::Footage). `serde`-defaulted so a layer missing
/// this block (every pre-footage `.pulse` file) loads with an empty default.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FootageLayer {
    /// The on-disk source (still or sequence). `None` until the user picks one;
    /// an unset source renders nothing.
    #[serde(default)]
    pub source: Option<FootageSource>,
    /// How the file's alpha is interpreted at load.
    #[serde(default)]
    pub alpha: AlphaMode,
    /// Optional **fps override** for sequence playback. `None` uses the comp's
    /// fps (so the sequence plays one source frame per comp frame). A value lets
    /// the footage play faster / slower than the comp (e.g. a 12fps sequence in a
    /// 30fps comp).
    #[serde(default)]
    pub fps: Option<f32>,
    /// Loop the sequence past its last frame (modulo wrap). Mutually preferred
    /// over `hold_last`.
    #[serde(default)]
    pub looping: bool,
    /// Hold the last frame past the end of the sequence (the default behaviour
    /// when not looping).
    #[serde(default = "default_true")]
    pub hold_last: bool,
}

fn default_true() -> bool {
    true
}

impl FootageLayer {
    /// Whether this layer has a source to draw.
    pub fn is_set(&self) -> bool {
        self.source.is_some()
    }

    /// Resolve the on-disk path to decode at comp time `t`, given the comp's fps
    /// (used when no fps override is set). `None` when no source is set.
    pub fn path_at(&self, t: f32, comp_fps: f32) -> Option<PathBuf> {
        let src = self.source.as_ref()?;
        let fps = self.fps.unwrap_or(comp_fps);
        let seq = src.frame_index(t, fps, self.looping, self.hold_last);
        Some(src.path_for(seq))
    }
}

/// Detect a numbered **image sequence** from one chosen frame file, or fall back
/// to a single still when the file has no trailing number.
///
/// Splits the file stem's trailing run of ASCII digits into a `{}`-placeholder
/// pattern (absolute, so it resolves regardless of cwd), infers the zero-pad
/// width and the sequence's start number, and counts the contiguous run of
/// existing frames on disk. The picked frame need not be the first — the probe
/// walks down to the run's start and up to its end. A file with no trailing
/// digits returns a [`FootageSource::Still`].
pub fn source_from_path(path: &Path) -> FootageSource {
    detect_sequence(path).unwrap_or_else(|| FootageSource::still(path.to_path_buf()))
}

fn detect_sequence(path: &Path) -> Option<FootageSource> {
    let parent = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let stem = path.file_stem().and_then(|s| s.to_str())?;

    // Split off the trailing run of ASCII digits.
    let digits_start = stem
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_ascii_digit())
        .last()
        .map(|(i, _)| i)?;
    let (prefix, digits) = stem.split_at(digits_start);
    if digits.is_empty() {
        return None;
    }
    let pad = digits.len();
    let picked: u32 = digits.parse().ok()?;

    // Build the printf-style pattern (absolute where possible).
    let file_pattern = if ext.is_empty() {
        format!("{prefix}{{}}")
    } else {
        format!("{prefix}{{}}.{ext}")
    };
    let pattern = parent.join(&file_pattern).to_string_lossy().into_owned();

    // Find the contiguous run of existing frames around the picked number.
    let exists = |n: u32| {
        let num = format!("{n:0pad$}");
        Path::new(&pattern.replacen("{}", &num, 1)).exists()
    };
    let mut start = picked;
    while start > 0 && exists(start - 1) {
        start -= 1;
    }
    let mut count = 0u32;
    while exists(start + count) {
        count += 1;
        if count > 100_000 {
            break; // guard against a runaway probe
        }
    }
    Some(FootageSource::Sequence {
        pattern,
        pad,
        start,
        count: count.max(1),
    })
}

/// A decoded footage frame in the compositor's working space: straight
/// (non-premultiplied) **linear-light** RGBA, row-major, `width * height`
/// pixels, top-left origin. Matches the `[f32; 4]` representation the rasterizer
/// samples (sRGB color already converted to linear, alpha as straight coverage).
#[derive(Clone, Debug)]
pub struct DecodedFrame {
    pub width: u32,
    pub height: u32,
    /// `width * height` straight linear-light RGBA pixels.
    pub pixels: Vec<[f32; 4]>,
}

impl DecodedFrame {
    /// Bilinearly sample this frame at normalized UV (`u, v` in `[0, 1]`, top-left
    /// origin). Out-of-range UVs return a fully transparent pixel (so the footage
    /// quad has hard edges). Returns straight linear-light RGBA.
    pub fn sample(&self, u: f32, v: f32) -> [f32; 4] {
        if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
            return [0.0; 4];
        }
        if self.width == 0 || self.height == 0 {
            return [0.0; 4];
        }
        // Map UV to pixel-center space, then bilerp the four neighbours.
        let fx = (u * self.width as f32 - 0.5).max(0.0);
        let fy = (v * self.height as f32 - 0.5).max(0.0);
        let x0 = (fx.floor() as u32).min(self.width - 1);
        let y0 = (fy.floor() as u32).min(self.height - 1);
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);
        let tx = fx - x0 as f32;
        let ty = fy - y0 as f32;
        let at = |x: u32, y: u32| self.pixels[(y * self.width + x) as usize];
        let p00 = at(x0, y0);
        let p10 = at(x1, y0);
        let p01 = at(x0, y1);
        let p11 = at(x1, y1);
        let mut out = [0.0f32; 4];
        for c in 0..4 {
            let top = p00[c] + (p10[c] - p00[c]) * tx;
            let bot = p01[c] + (p11[c] - p01[c]) * tx;
            out[c] = top + (bot - top) * ty;
        }
        out
    }

}

/// A small most-recently-used **decode cache**: keeps the last few decoded
/// footage frames so the compositor doesn't re-decode the same file every comp
/// frame (or for every sub-frame motion-blur snapshot). Keyed by absolute-ish
/// path; a bounded LRU keeps memory in check across a render pass.
///
/// A failed decode is cached as `None` so a missing file doesn't get retried
/// (and spam the log) on every reference within a pass.
#[derive(Default)]
pub struct FrameCache {
    /// path -> (decoded frame or None on failure). `Arc`-free: the renderer reads
    /// frames by reference within a single pass.
    entries: HashMap<PathBuf, Option<DecodedFrame>>,
    /// MRU order of keys (front = most recent); bounds the map size.
    order: Vec<PathBuf>,
    cap: usize,
}

impl FrameCache {
    /// How many decoded frames to keep resident by default. Enough to cover a
    /// motion-blur sample fan plus a few neighbouring comp frames without holding
    /// a whole sequence in RAM.
    pub const DEFAULT_CAP: usize = 8;

    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            order: Vec::new(),
            cap: Self::DEFAULT_CAP,
        }
    }

    /// Clear every cached frame (e.g. when the source changes). Public cache
    /// hygiene the interactive caller will want once it holds a persistent cache.
    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }

    /// Number of resident entries (decoded frames + cached failures).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache holds no entries (pairs with [`len`](Self::len)).
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get the decoded frame for `path`, decoding (and caching) on first use.
    /// Returns `None` if the file is missing or fails to decode (also cached so
    /// it isn't retried within the pass). `alpha` controls how the file's alpha
    /// is interpreted at load.
    pub fn get(&mut self, path: &Path, alpha: AlphaMode) -> Option<&DecodedFrame> {
        if !self.entries.contains_key(path) {
            let decoded = decode_to_linear(path, alpha);
            if decoded.is_none() {
                log::warn!("footage decode failed or missing: {}", path.display());
            }
            self.insert(path.to_path_buf(), decoded);
        } else {
            self.touch(path);
        }
        self.entries.get(path).and_then(|e| e.as_ref())
    }

    fn insert(&mut self, key: PathBuf, val: Option<DecodedFrame>) {
        self.entries.insert(key.clone(), val);
        self.order.retain(|k| k != &key);
        self.order.insert(0, key);
        // Evict beyond capacity (drop the least-recently-used).
        while self.order.len() > self.cap {
            if let Some(old) = self.order.pop() {
                self.entries.remove(&old);
            }
        }
    }

    fn touch(&mut self, key: &Path) {
        if let Some(pos) = self.order.iter().position(|k| k.as_path() == key) {
            let k = self.order.remove(pos);
            self.order.insert(0, k);
        }
    }
}

/// Decode an image file to straight **linear-light** RGBA, applying `alpha`.
///
/// Goes through `prism_io::load_image` (8-bit sRGB RGBA, top-left origin), then
/// converts each channel sRGB → linear at the gamma boundary and resolves the
/// alpha interpretation so the result is straight color + straight coverage —
/// the exact representation the solid / shape / text rasterizers feed the
/// compositor. `None` on any IO / decode error.
fn decode_to_linear(path: &Path, alpha: AlphaMode) -> Option<DecodedFrame> {
    let loaded = prism_io::load_image(path).ok()?;
    let (w, h) = (loaded.size.width, loaded.size.height);
    let mut pixels = Vec::with_capacity((w * h) as usize);
    for chunk in loaded.rgba8.chunks_exact(4) {
        let r = srgb_to_linear(chunk[0] as f32 / 255.0);
        let g = srgb_to_linear(chunk[1] as f32 / 255.0);
        let b = srgb_to_linear(chunk[2] as f32 / 255.0);
        let mut a = chunk[3] as f32 / 255.0;
        let (mut r, mut g, mut b) = (r, g, b);
        match alpha {
            AlphaMode::Straight => {}
            AlphaMode::Ignore => a = 1.0,
            AlphaMode::Premultiplied => {
                // The file's RGB is premultiplied; un-premultiply to straight.
                if a > 0.0 {
                    r /= a;
                    g /= a;
                    b /= a;
                } else {
                    r = 0.0;
                    g = 0.0;
                    b = 0.0;
                }
            }
        }
        pixels.push([r, g, b, a]);
    }
    Some(DecodedFrame {
        width: w,
        height: h,
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seq(count: u32) -> FootageSource {
        FootageSource::Sequence {
            pattern: "frame_{}.png".to_string(),
            pad: 4,
            start: 1,
            count,
        }
    }

    #[test]
    fn still_is_constant_frame_zero() {
        let s = FootageSource::still("a.png");
        assert_eq!(s.len(), 1);
        for &t in &[0.0, 1.0, 5.0, 100.0] {
            assert_eq!(s.frame_index(t, 30.0, false, false), 0);
            assert_eq!(s.frame_index(t, 30.0, true, true), 0);
        }
    }

    #[test]
    fn sequence_frame_from_time_and_fps() {
        let s = seq(10);
        // At 10 fps, t=0 -> 0, t=0.5 -> 5, t=0.9 -> 9.
        assert_eq!(s.frame_index(0.0, 10.0, false, true), 0);
        assert_eq!(s.frame_index(0.5, 10.0, false, true), 5);
        assert_eq!(s.frame_index(0.95, 10.0, false, true), 9);
        // fps override changes the mapping: 5 fps -> half the index at same time.
        assert_eq!(s.frame_index(0.5, 5.0, false, true), 2);
    }

    #[test]
    fn hold_last_clamps_past_end() {
        let s = seq(5); // frames 0..=4
        // t=1.0 @ 10fps -> raw 10, past the 5-frame end -> hold last (4).
        assert_eq!(s.frame_index(1.0, 10.0, false, true), 4);
        // Default (no loop, no hold flag) still clamps to last.
        assert_eq!(s.frame_index(1.0, 10.0, false, false), 4);
    }

    #[test]
    fn loop_wraps_past_end() {
        let s = seq(5); // frames 0..=4
        // raw 5 -> wrap to 0, raw 6 -> 1, raw 12 -> 2.
        assert_eq!(s.frame_index(0.5, 10.0, true, false), 0);
        assert_eq!(s.frame_index(0.6, 10.0, true, false), 1);
        assert_eq!(s.frame_index(1.2, 10.0, true, false), 2);
    }

    #[test]
    fn negative_time_holds_first_frame() {
        let s = seq(5);
        assert_eq!(s.frame_index(-1.0, 10.0, false, true), 0);
        assert_eq!(s.frame_index(-1.0, 10.0, true, false), 0);
    }

    #[test]
    fn path_for_zero_pads_and_offsets_by_start() {
        let s = FootageSource::Sequence {
            pattern: "shot/img_{}.png".to_string(),
            pad: 4,
            start: 10,
            count: 100,
        };
        assert_eq!(s.path_for(0), PathBuf::from("shot/img_0010.png"));
        assert_eq!(s.path_for(5), PathBuf::from("shot/img_0015.png"));
        assert_eq!(s.path_for(90), PathBuf::from("shot/img_0100.png"));
    }

    #[test]
    fn footage_layer_path_at_uses_override_then_comp_fps() {
        let mut fl = FootageLayer {
            source: Some(seq(10)),
            ..Default::default()
        };
        // No override: comp fps drives it. comp_fps=10, t=0.3 -> frame 3 -> _0004.
        assert_eq!(fl.path_at(0.3, 10.0), Some(PathBuf::from("frame_0004.png")));
        // Override at 5 fps: t=0.3 -> frame 1 -> _0002, ignoring comp fps.
        fl.fps = Some(5.0);
        assert_eq!(fl.path_at(0.3, 30.0), Some(PathBuf::from("frame_0002.png")));
    }

    #[test]
    fn unset_source_has_no_path() {
        let fl = FootageLayer::default();
        assert!(!fl.is_set());
        assert_eq!(fl.path_at(0.0, 30.0), None);
    }

    #[test]
    fn cache_decodes_once_and_caches_failure() {
        // A missing path decodes to None and is cached (no panic, no retry churn).
        let mut cache = FrameCache::new();
        let p = Path::new("definitely_missing_xyz.png");
        assert!(cache.get(p, AlphaMode::Straight).is_none());
        assert_eq!(cache.len(), 1);
        // Second get hits the cache (still None).
        assert!(cache.get(p, AlphaMode::Straight).is_none());
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn cache_evicts_beyond_capacity() {
        let mut cache = FrameCache::new();
        for i in 0..(FrameCache::DEFAULT_CAP + 4) {
            let p = PathBuf::from(format!("missing_{i}.png"));
            cache.get(&p, AlphaMode::Straight);
        }
        assert!(cache.len() <= FrameCache::DEFAULT_CAP);
    }

    #[test]
    fn decoded_frame_sample_bilerp_and_oob() {
        let frame = DecodedFrame {
            width: 2,
            height: 1,
            pixels: vec![[0.0, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0, 1.0]],
        };
        // Center samples land near the two texels; OOB returns transparent.
        let mid = frame.sample(0.5, 0.5);
        assert!(mid[3] > 0.0);
        assert_eq!(frame.sample(-0.1, 0.5), [0.0; 4]);
        assert_eq!(frame.sample(0.5, 1.1), [0.0; 4]);
    }
}
