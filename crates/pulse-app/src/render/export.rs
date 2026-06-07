//! The PNG image-sequence exporter: frame counting/timing/paths and the IO
//! shell that drives [`render_frame`] across a comp.

use super::{render_frame_cached, render_frame_in_project, Frame};
use crate::comp::{Comp, FrameCache};
use std::path::{Path, PathBuf};

/// The frame count of a render: every frame on the comp's `[0, duration]`
/// timeline at its fps, inclusive of frame 0. A 5 s comp at 30 fps yields 150
/// frames (0..149), matching After Effects' frame-inclusive duration.
pub fn frame_count(comp: &Comp) -> u32 {
    let fps = comp.fps.max(1.0);
    (comp.duration.max(0.0) * fps).round().max(1.0) as u32
}

/// The presentation time (seconds) of frame `i`.
pub fn frame_time(comp: &Comp, i: u32) -> f32 {
    let fps = comp.fps.max(1.0);
    i as f32 / fps
}

/// Which span of the comp's timeline a render covers.
///
/// After Effects defaults the render range to the **work area** (the in/out
/// sub-range) and lets you switch it to the **full comp**. Pulse mirrors that:
/// [`RenderRange::WorkArea`] renders only the clamped work-area frames (the
/// transport's loop range), [`RenderRange::Full`] renders the whole
/// `[0, duration]` timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderRange {
    /// Render every frame on the comp's `[0, duration]` timeline.
    Full,
    /// Render only the frames inside the comp's clamped work area (in → out).
    WorkArea,
}

impl RenderRange {
    /// The range After Effects defaults to: the **work area** when it is a real
    /// sub-range of the timeline, else the **full comp** (a full or degenerate
    /// work area would otherwise render nothing useful, so fall back to full).
    pub fn default_for(comp: &Comp) -> Self {
        let wa = comp.clamped_work_area();
        // A trimmed, non-empty work area → default to it; a full timeline or a
        // zero-length (degenerate) area → fall back to the whole comp.
        if !wa.is_full(comp.duration) && wa.end - wa.start > 1e-4 {
            RenderRange::WorkArea
        } else {
            RenderRange::Full
        }
    }
}

/// The inclusive comp-frame index span `[first, last]` this range covers, on the
/// comp's full frame grid (so `first` is the work-area start frame and the file
/// numbering / presentation time reflect the chosen range — the first exported
/// frame of a work-area render is the work-area start frame, not 0).
///
/// [`RenderRange::Full`] spans `[0, frame_count − 1]`. [`RenderRange::WorkArea`]
/// rounds the **clamped** work-area in/out to the nearest frames; an empty or
/// degenerate work area (and the full-comp case) falls back to the full span so a
/// render is never empty.
pub fn frame_range(comp: &Comp, range: RenderRange) -> (u32, u32) {
    let total = frame_count(comp);
    let last_idx = total - 1;
    match range {
        RenderRange::Full => (0, last_idx),
        RenderRange::WorkArea => {
            let wa = comp.clamped_work_area();
            // A full / degenerate work area renders the whole comp (never empty).
            if wa.is_full(comp.duration) || wa.end - wa.start <= 1e-4 {
                return (0, last_idx);
            }
            let fps = comp.fps.max(1.0);
            let first = (wa.start * fps).round().clamp(0.0, last_idx as f32) as u32;
            let last = (wa.end * fps).round().clamp(first as f32, last_idx as f32) as u32;
            (first, last)
        }
    }
}

/// The number of frames a render of `range` writes: the inclusive
/// [`frame_range`] span length.
pub fn range_frame_count(comp: &Comp, range: RenderRange) -> u32 {
    let (first, last) = frame_range(comp, range);
    last - first + 1
}

/// Build the output path for frame `i`: `<dir>/<stem>_<0000>.png`, zero-padded
/// to at least 4 digits (more if the sequence needs them).
pub fn frame_path(dir: &Path, stem: &str, i: u32, total: u32) -> PathBuf {
    let pad = total.saturating_sub(1).to_string().len().max(4);
    dir.join(format!("{stem}_{i:0pad$}.png", pad = pad))
}

/// Summary of an [`export_sequence`] run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportSummary {
    pub frames: u32,
    pub dir: PathBuf,
}

/// Render the frames of `comp` in `range` and write the PNG image sequence to
/// `dir`, naming files by their **comp frame index** (so a work-area render's
/// first file is the work-area start frame, e.g. `<stem>_0030.png`, not
/// `_0000`). Creates `dir` if it does not exist. Returns a summary, or the first
/// IO/encode error.
///
/// The single-comp exporter: precomp layers in `comp` resolve against nothing
/// (they render empty). The app exports through [`export_sequence_in_project`]
/// instead; this entry is retained for single-comp callers / tests.
#[cfg_attr(not(test), allow(dead_code))]
pub fn export_sequence(
    comp: &Comp,
    dir: &Path,
    stem: &str,
    range: RenderRange,
) -> std::io::Result<ExportSummary> {
    std::fs::create_dir_all(dir)?;
    let total = frame_count(comp);
    let (first, last) = frame_range(comp, range);
    // One footage cache for the whole export: a sequence's source frames decode
    // at most once each (and a still decodes once) instead of per comp frame.
    let mut cache = FrameCache::new();
    for i in first..=last {
        let t = frame_time(comp, i);
        let frame = render_frame_cached(comp, t, &mut cache);
        let path = frame_path(dir, stem, i, total);
        write_png(&path, &frame)?;
    }
    Ok(ExportSummary {
        frames: last - first + 1,
        dir: dir.to_path_buf(),
    })
}

/// Render the frames of comp `id` (within `comps`) in `range` and write the PNG
/// image sequence to `dir`, resolving any **precomp** layers against the
/// project's sibling comps (and breaking reference cycles). The project-aware
/// twin of [`export_sequence`]; identical framing/timing/naming (files numbered
/// by comp frame index, so a work-area render starts at the in-point frame), but
/// a precomp in the exported comp renders its nested comp recursively instead of
/// nothing.
pub fn export_sequence_in_project(
    comps: &[Comp],
    id: u64,
    dir: &Path,
    stem: &str,
    range: RenderRange,
) -> std::io::Result<ExportSummary> {
    let Some(comp) = comps.iter().find(|c| c.id == id) else {
        return Ok(ExportSummary {
            frames: 0,
            dir: dir.to_path_buf(),
        });
    };
    std::fs::create_dir_all(dir)?;
    let total = frame_count(comp);
    let (first, last) = frame_range(comp, range);
    let mut cache = FrameCache::new();
    for i in first..=last {
        let t = frame_time(comp, i);
        let frame = render_frame_in_project(comps, id, t, &mut cache);
        let path = frame_path(dir, stem, i, total);
        write_png(&path, &frame)?;
    }
    Ok(ExportSummary {
        frames: last - first + 1,
        dir: dir.to_path_buf(),
    })
}

/// Encode a [`Frame`] to a PNG file via the `image` crate, mapping any encode
/// failure into an `io::Error` so callers have a single error type.
fn write_png(path: &Path, frame: &Frame) -> std::io::Result<()> {
    let img = image::RgbaImage::from_raw(frame.width, frame.height, frame.pixels.clone())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "frame buffer size mismatch",
            )
        })?;
    img.save_with_format(path, image::ImageFormat::Png)
        .map_err(std::io::Error::other)
}
