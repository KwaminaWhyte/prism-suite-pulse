//! The PNG image-sequence exporter: frame counting/timing/paths and the IO
//! shell that drives [`render_frame`] across a comp.

use super::{render_frame, Frame};
use crate::comp::Comp;
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

/// Render every frame of `comp` and write the PNG image sequence to `dir`,
/// naming files `<stem>_0000.png`, `<stem>_0001.png`, …. Creates `dir` if it
/// does not exist. Returns a summary, or the first IO/encode error.
pub fn export_sequence(comp: &Comp, dir: &Path, stem: &str) -> std::io::Result<ExportSummary> {
    std::fs::create_dir_all(dir)?;
    let total = frame_count(comp);
    for i in 0..total {
        let t = frame_time(comp, i);
        let frame = render_frame(comp, t);
        let path = frame_path(dir, stem, i, total);
        write_png(&path, &frame)?;
    }
    Ok(ExportSummary {
        frames: total,
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
