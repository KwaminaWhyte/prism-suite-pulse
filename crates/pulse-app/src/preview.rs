//! The preview surface: a **render preview**. The composition is rendered at the
//! current playhead time through the *real* offline compositor
//! ([`render`](crate::render)) at a capped preview resolution, uploaded as an
//! egui texture, and drawn as the preview image — so footage frames, precomps,
//! effects, masks, mattes, motion blur, time-remap, and expressions all show
//! **real composited pixels** (the same result an export produces).
//!
//! The preview is a **RAM-preview cache**: each comp frame is rendered (off the
//! UI thread, through the real offline compositor at a capped resolution) *once*
//! and the resulting pixels are cached by frame index, so loop playback runs in
//! real time straight from RAM after the first pass. A **pool** of worker threads
//! fills the cache in parallel (frame-level [`rayon`](https://docs.rs/rayon)-style
//! fan-out, hand-rolled) so the first fill is roughly N× faster than serial. The
//! cache is invalidated (and re-filled) whenever the comp/layers change or the
//! preview resolution changes; a byte budget bounds its memory (frames farthest
//! from the playhead are evicted first).
//!
//! Interactive **overlays** (selection box, mask handles, null pivots, the
//! transform gizmo, onion-skin ghosts) are drawn on top via egui's [`Painter`],
//! pixel-aligned to the displayed image rect through the same aspect-fit mapping
//! (`comp px → screen`) the offline renderer uses, so they track the rendered
//! pixels through letterbox scaling.

use crate::comp::{Comp, LayerKind, MaskMode, PulseLayer};
use crate::gizmo::{GizmoGeom, Handle};
use crate::onion::{Ghost, OnionSkin};
use crate::render::Frame;
use crate::theme;
use egui::{Color32, ColorImage, Painter, Pos2, Rect, Stroke, TextureHandle, Vec2};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;

use crate::comp::Affine2;

/// The capped long-edge resolution (px) of the interactive render preview. Large
/// comps render downscaled to this so scrubbing stays responsive; the cache then
/// holds each rendered frame for real-time loop playback.
const PREVIEW_CAP: u32 = 1280;

/// Memory budget for the RAM-preview frame cache (bytes). When exceeded, cached
/// frames **farthest from the playhead** are evicted first, so the region around
/// the current time stays resident. ~1 GiB ≈ 290 frames at 1280×720 RGBA (i.e. a
/// typical short comp fits whole, so its loop plays back in real time; only long
/// comps evict and keep a window around the playhead cached).
const CACHE_BUDGET_BYTES: u64 = 1024 * 1024 * 1024;

/// How many *additional* (look-ahead) frames to enqueue for rendering per UI
/// frame while the cache is filling. Bounds per-call work; repeated repaints fan
/// the whole work area out to the worker pool over a handful of frames.
const PREFETCH_PER_CALL: usize = 8;

/// Drives the interactive preview as a **RAM-preview cache** rendered by a pool
/// of background worker threads.
///
/// Each comp frame is composited (off the UI thread, at [`PREVIEW_CAP`]) exactly
/// once and its pixels are cached by frame index ([`frames`](Self::frames)); a
/// single reusable [`TextureHandle`] is re-pointed at whichever cached frame the
/// playhead is on. The worker [pool](Self::job_tx) fills the cache in parallel,
/// so loop playback runs in real time straight from RAM after the first pass. A
/// [`sig`](Self::sig) (comp-state hash + render size) gates the whole cache: any
/// edit or resize bumps the [`epoch`](Self::epoch), clears the cache, and lets
/// the pool re-fill — in-flight jobs tagged with the old epoch are skipped, so a
/// rapid scrub/edit never wastes full renders on superseded frames.
#[derive(Default)]
pub struct PreviewRenderer {
    /// The single reusable preview texture, re-pointed at the displayed frame.
    tex: Option<TextureHandle>,
    /// The frame index currently uploaded to [`tex`](Self::tex) (so an unchanged
    /// display frame isn't re-uploaded).
    shown_idx: Option<i64>,
    /// The playhead time of the frame currently displayed. Overlays align to
    /// *this* (not the live playhead) so they don't lead the pixels when the shown
    /// frame is a not-yet-rendered frame's nearest cached neighbour.
    shown_time: Option<f32>,
    /// The RAM frame cache: frame index → rendered (capped-res, sRGB) pixels.
    frames: HashMap<i64, Frame>,
    /// Total bytes held in [`frames`](Self::frames) (for budget eviction).
    bytes: u64,
    /// The signature `(comp-state hash, render dims)` the cache is valid for. A
    /// mismatch invalidates the whole cache.
    sig: Option<(u64, (u32, u32))>,
    /// Monotonic cache generation; bumped on every invalidation and mirrored into
    /// [`epoch_shared`](Self::epoch_shared) so workers can skip stale jobs.
    epoch: u64,
    /// The shared, worker-visible copy of [`epoch`](Self::epoch): a worker drops a
    /// job whose epoch no longer matches (superseded by an edit/resize).
    epoch_shared: Option<Arc<AtomicU64>>,
    /// Frame indices already handed to the pool for the current epoch (so the same
    /// frame isn't enqueued twice while it renders).
    queued: HashSet<i64>,
    /// The comps the current cache renders from, shared cheaply into each job.
    comps_arc: Option<Arc<Vec<Comp>>>,
    /// The active comp id, render dims, fps, and work-area frame count for the
    /// current [`sig`](Self::sig).
    id: u64,
    dims: (u32, u32),
    fps: f32,
    n_frames: i64,
    /// Round-robin cursor for distributing jobs across the worker pool.
    rr: usize,
    /// Per-worker job channels (the pool); a job is sent to `job_tx[rr % len]`.
    job_tx: Vec<Sender<RenderJob>>,
    /// Shared result channel the whole pool sends finished frames back on.
    res_rx: Option<Receiver<RenderDone>>,
    /// Worker thread handles, kept alive for the app's lifetime. Dropping the
    /// renderer closes the job channels, which ends each worker loop.
    #[allow(dead_code)]
    workers: Vec<JoinHandle<()>>,
}

/// A render job for the pool: the cache generation it belongs to, the frame index
/// + time to render, the comp id + resolution cap, and the shared comps.
struct RenderJob {
    epoch: u64,
    idx: i64,
    t: f32,
    id: u64,
    cap: u32,
    comps: Arc<Vec<Comp>>,
}

/// A finished frame from a worker: its cache generation, frame index, and the
/// rendered (capped-resolution sRGB) [`Frame`].
struct RenderDone {
    epoch: u64,
    idx: i64,
    frame: Frame,
}

/// A pool worker loop. Owns a persistent [`FrameCache`] (so a footage source
/// decodes once and is reused across frames it renders) and composites jobs off
/// the UI thread. A job whose `epoch` no longer matches the shared `epoch` — i.e.
/// the comp/resolution changed after the job was queued — is **skipped** (before
/// and after rendering), so superseded work is never produced or sent. Exits when
/// its job channel closes (the owning [`PreviewRenderer`] is dropped).
fn worker_loop(job_rx: Receiver<RenderJob>, res_tx: Sender<RenderDone>, epoch: Arc<AtomicU64>) {
    let mut cache = crate::comp::FrameCache::new();
    while let Ok(job) = job_rx.recv() {
        // Superseded before we even start — skip without rendering.
        if job.epoch != epoch.load(Ordering::Relaxed) {
            continue;
        }
        let frame =
            crate::render::render_preview_frame(&job.comps, job.id, job.t, job.cap, &mut cache);
        // Superseded while rendering — drop the now-stale frame.
        if job.epoch != epoch.load(Ordering::Relaxed) {
            continue;
        }
        if res_tx
            .send(RenderDone {
                epoch: job.epoch,
                idx: job.idx,
                frame,
            })
            .is_err()
        {
            break; // UI gone — stop.
        }
    }
}

/// Hash the project comp state (comp id + serialized comps) into the cache
/// signature's state component. Serializing to JSON and hashing the bytes is
/// robust to any field change without hand-maintaining a hasher; a serialization
/// failure (never expected for these plain structs) degrades to hashing nothing.
fn state_hash(comps: &[Comp], id: u64) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    if let Ok(json) = serde_json::to_vec(comps) {
        json.hash(&mut h);
    }
    h.finish()
}

/// The work-area frame count for a comp of `duration` seconds at `fps` (at least
/// one frame). Frame `i` is presented at `i / fps` seconds.
fn frame_count(duration: f32, fps: f32) -> i64 {
    ((duration.max(0.0) * fps.max(1.0)).ceil() as i64).max(1)
}

impl PreviewRenderer {
    /// Spawn the worker pool on first use (idempotent). One worker per available
    /// core minus two (clamped to 1..=8), each with its own job channel + footage
    /// [`FrameCache`](crate::comp::FrameCache), all sharing one result channel and
    /// the [`epoch_shared`](Self::epoch_shared) generation counter.
    fn ensure_workers(&mut self) {
        if !self.job_tx.is_empty() {
            return;
        }
        let n = std::thread::available_parallelism()
            .map(|c| c.get().saturating_sub(2))
            .unwrap_or(2)
            .clamp(1, 8);
        let (res_tx, res_rx) = std::sync::mpsc::channel::<RenderDone>();
        let epoch = Arc::new(AtomicU64::new(self.epoch));
        for _ in 0..n {
            let (job_tx, job_rx) = std::sync::mpsc::channel::<RenderJob>();
            let res_tx = res_tx.clone();
            let ep = epoch.clone();
            if let Ok(h) = std::thread::Builder::new()
                .name("pulse-preview".into())
                .spawn(move || worker_loop(job_rx, res_tx, ep))
            {
                self.job_tx.push(job_tx);
                self.workers.push(h);
            }
        }
        self.res_rx = Some(res_rx);
        self.epoch_shared = Some(epoch);
        // The original `res_tx` drops here; the per-worker clones keep `res_rx`
        // open for as long as any worker lives.
    }

    /// Return the preview texture for comp `id` of `comps` at time `t`, served
    /// from the **RAM-preview cache**.
    ///
    /// The comp's frames are rendered once each by the worker pool (off the UI
    /// thread) and cached by frame index; this re-points the single preview
    /// texture at the cached frame for `t` (or, while that frame is still
    /// rendering, its nearest cached neighbour). Any edit/resize invalidates the
    /// cache (a new epoch) so it re-fills. The UI thread never composites, so the
    /// app stays responsive; once the work area is cached, loop playback is
    /// real-time from RAM (see [`is_frame_ready`](Self::is_frame_ready) /
    /// [`fully_cached`](Self::fully_cached), which gate the playback pacing).
    pub fn texture(
        &mut self,
        ctx: &egui::Context,
        comps: Vec<Comp>,
        id: u64,
        t: f32,
        fps: f32,
        duration: f32,
    ) -> Option<TextureHandle> {
        self.ensure_workers();

        // Cache signature: comp-state hash + render dims. A change invalidates the
        // whole cache and bumps the epoch (so in-flight stale jobs are skipped).
        let dims = comps
            .iter()
            .find(|c| c.id == id)
            .map(|c| crate::render::preview_dims(c.width, c.height, PREVIEW_CAP))
            .unwrap_or((1, 1));
        let sig = (state_hash(&comps, id), dims);
        if self.sig != Some(sig) {
            self.sig = Some(sig);
            self.epoch += 1;
            if let Some(ep) = &self.epoch_shared {
                ep.store(self.epoch, Ordering::Relaxed);
            }
            self.frames.clear();
            self.queued.clear();
            self.bytes = 0;
            self.shown_idx = None;
            self.id = id;
            self.dims = dims;
            self.fps = fps.max(1.0);
            self.n_frames = frame_count(duration, fps);
            self.comps_arc = Some(Arc::new(comps));
        }
        // (When `sig` is unchanged the freshly-cloned `comps` is dropped here — the
        // cache already renders from the identical comps in `comps_arc`.)

        // Drain finished frames for the current epoch into the cache.
        let cur_epoch = self.epoch;
        let mut done: Vec<RenderDone> = Vec::new();
        if let Some(rx) = &self.res_rx {
            while let Ok(d) = rx.try_recv() {
                if d.epoch == cur_epoch {
                    done.push(d);
                }
            }
        }
        for d in done {
            self.insert_frame(d.idx, d.frame);
        }

        // The frame the playhead is on, and the work-area enqueue plan.
        let cur = ((t * self.fps).round() as i64).clamp(0, (self.n_frames - 1).max(0));
        self.enqueue(cur);
        // Fan the rest of the work area out to the pool, look-ahead first, a
        // bounded batch per call (repeated repaints fill the whole comp).
        let mut sent = 0;
        let mut i = cur;
        let mut scanned = 0;
        while sent < PREFETCH_PER_CALL && scanned < self.n_frames {
            i += 1;
            if i >= self.n_frames {
                i = 0;
            }
            scanned += 1;
            if self.enqueue(i) {
                sent += 1;
            }
        }
        self.evict_to_budget(cur);

        // Display the current frame, or its nearest cached neighbour while it
        // renders, re-pointing the texture only when the displayed frame changes.
        let display_idx = if self.frames.contains_key(&cur) {
            Some(cur)
        } else {
            self.frames.keys().min_by_key(|&&k| (k - cur).abs()).copied()
        };
        if let Some(di) = display_idx {
            if self.shown_idx != Some(di) {
                if let Some(frame) = self.frames.get(&di) {
                    let image = ColorImage::from_rgba_unmultiplied(
                        [frame.width as usize, frame.height as usize],
                        &frame.pixels,
                    );
                    match &mut self.tex {
                        Some(tex) => tex.set(image, egui::TextureOptions::LINEAR),
                        None => {
                            self.tex = Some(ctx.load_texture(
                                "pulse_preview",
                                image,
                                egui::TextureOptions::LINEAR,
                            ));
                        }
                    }
                    self.shown_idx = Some(di);
                    self.shown_time = Some(di as f32 / self.fps);
                }
            }
        }

        // Keep repainting while the cache is still filling so finished frames get
        // picked up (playback also repaints).
        if !self.fully_cached() {
            ctx.request_repaint();
        }

        self.tex.clone()
    }

    /// Insert a freshly-rendered frame into the cache, updating the byte tally and
    /// clearing its queued mark.
    fn insert_frame(&mut self, idx: i64, frame: Frame) {
        self.queued.remove(&idx);
        let bytes = frame.pixels.len() as u64;
        if let Some(old) = self.frames.insert(idx, frame) {
            self.bytes = self.bytes.saturating_sub(old.pixels.len() as u64);
        }
        self.bytes += bytes;
    }

    /// Enqueue frame `idx` for rendering if it isn't already cached or in flight.
    /// Returns whether a job was sent. Jobs round-robin across the worker pool.
    fn enqueue(&mut self, idx: i64) -> bool {
        if idx < 0 || idx >= self.n_frames {
            return false;
        }
        if self.frames.contains_key(&idx) || self.queued.contains(&idx) {
            return false;
        }
        let Some(comps) = &self.comps_arc else {
            return false;
        };
        if self.job_tx.is_empty() {
            return false;
        }
        let job = RenderJob {
            epoch: self.epoch,
            idx,
            t: idx as f32 / self.fps,
            id: self.id,
            cap: PREVIEW_CAP,
            comps: comps.clone(),
        };
        let w = self.rr % self.job_tx.len();
        self.rr = self.rr.wrapping_add(1);
        if self.job_tx[w].send(job).is_ok() {
            self.queued.insert(idx);
            true
        } else {
            false
        }
    }

    /// Evict cached frames **farthest from `cur`** until the cache is within
    /// [`CACHE_BUDGET_BYTES`] (never evicting `cur` itself). For a work area that
    /// fits the budget this is a no-op; a longer comp keeps the region around the
    /// playhead resident.
    fn evict_to_budget(&mut self, cur: i64) {
        while self.bytes > CACHE_BUDGET_BYTES {
            let Some(&far) = self
                .frames
                .keys()
                .filter(|&&k| k != cur)
                .max_by_key(|&&k| (k - cur).abs())
            else {
                break;
            };
            if let Some(f) = self.frames.remove(&far) {
                self.bytes = self.bytes.saturating_sub(f.pixels.len() as u64);
            }
        }
    }

    /// Whether the frame for time `t` is already rendered and resident — the
    /// signal the render-paced first pass advances on (step only once the current
    /// frame is shown).
    pub fn is_frame_ready(&self, t: f32) -> bool {
        if self.n_frames <= 0 {
            return false;
        }
        let cur = ((t * self.fps).round() as i64).clamp(0, (self.n_frames - 1).max(0));
        self.frames.contains_key(&cur)
    }

    /// Whether the entire work area is cached — once true, loop playback can run
    /// in real time straight from RAM.
    pub fn fully_cached(&self) -> bool {
        self.n_frames > 0 && (self.frames.len() as i64) >= self.n_frames
    }

    /// RAM-cache fill progress as `(cached_frames, total_frames)` for the current
    /// work area. `total` is `0` before the first comp is registered. While the
    /// first pass builds the cache `cached < total`; once `fully_cached`, they're
    /// equal. Drives the transport's "caching…" readout so the first-pass lag the
    /// user sees is legible rather than mysterious.
    pub fn cache_progress(&self) -> (i64, i64) {
        let total = self.n_frames.max(0);
        let cached = (self.frames.len() as i64).min(total);
        (cached, total)
    }

    /// The playhead time of the frame currently displayed. Overlays / the timeline
    /// align to *this* so they track the pixels on screen rather than leading them
    /// (the displayed frame can be the nearest cached neighbour while the exact
    /// frame still renders). `None` until the first frame is uploaded (callers fall
    /// back to the live time).
    pub fn shown_time(&self) -> Option<f32> {
        self.shown_time
    }
}

/// Draw the rendered preview `texture` into the comp's aspect-fit rect inside
/// `avail` (with the comp backdrop + frame outline), and return the `(center,
/// scale)` mapping so overlays land on the same pixels. The texture fills exactly
/// the fitted rect, so comp-px → screen is the same affine the overlays use.
pub fn paint_image(
    painter: &Painter,
    avail: Rect,
    comp: &Comp,
    texture: &TextureHandle,
) -> (Pos2, f32) {
    let (frame, scale) = fit(avail, comp.width, comp.height);
    // Comp backdrop (shows through transparent areas) + frame outline.
    painter.rect_filled(frame, 4.0, Color32::from_rgb(0x10, 0x11, 0x13));
    painter.image(
        texture.id(),
        frame,
        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
        Color32::WHITE,
    );
    painter.rect_stroke(
        frame,
        4.0,
        Stroke::new(1.0, theme::stroke_subtle()),
        egui::StrokeKind::Outside,
    );
    (frame.center(), scale)
}

/// Compute the centered, aspect-fit rect for a comp of `width`x`height` inside
/// `avail`, plus the comp-pixels-to-screen scale factor.
fn fit(avail: Rect, width: u32, height: u32) -> (Rect, f32) {
    let cw = width.max(1) as f32;
    let ch = height.max(1) as f32;
    let margin = 24.0;
    let aw = (avail.width() - margin * 2.0).max(1.0);
    let ah = (avail.height() - margin * 2.0).max(1.0);
    let scale = (aw / cw).min(ah / ch).max(0.01);
    let size = Vec2::new(cw * scale, ch * scale);
    let rect = Rect::from_center_size(avail.center(), size);
    (rect, scale)
}

/// The comp's on-screen center and comp-pixels→screen scale for `avail` — the
/// mapping the preview uses to place layers. Exposed so the panel can convert
/// pointer positions (screen) into comp space for the transform gizmo, using
/// exactly the same fit as the paint pass.
pub fn comp_fit(avail: Rect, width: u32, height: u32) -> (Pos2, f32) {
    let (frame, scale) = fit(avail, width, height);
    (frame.center(), scale)
}

/// Paint the on-canvas transform gizmo for the selected layer: the bounding
/// box, corner scale handles, the rotation knob (with its connector), and the
/// anchor-point cross. `hot` highlights the handle the pointer is over (or is
/// dragging). Drawn on top of the preview so the layer stays visible.
pub fn paint_gizmo(
    painter: &Painter,
    geom: &GizmoGeom,
    center: Pos2,
    scale: f32,
    hot: Option<Handle>,
) {
    let to_screen = |(cx, cy): (f32, f32)| center + Vec2::new(cx * scale, cy * scale);
    let corners: Vec<Pos2> = geom.corners.iter().map(|&c| to_screen(c)).collect();
    let knob = to_screen(geom.rotate_knob);
    let anchor = to_screen(geom.anchor);
    let accent = theme::accent();

    // Bounding box outline.
    let mut box_pts = corners.clone();
    box_pts.push(corners[0]);
    painter.add(egui::Shape::line(box_pts, Stroke::new(1.5, accent)));

    // Connector from the top-edge midpoint to the rotation knob.
    let top_mid = corners[0].lerp(corners[1], 0.5);
    painter.line_segment(
        [top_mid, knob],
        Stroke::new(1.0, accent.gamma_multiply(0.8)),
    );

    // Rotation knob (hollow circle, filled when hot).
    let knob_hot = hot == Some(Handle::Rotate);
    painter.circle(
        knob,
        5.0,
        if knob_hot {
            accent
        } else {
            Color32::from_rgb(0x16, 0x18, 0x1c)
        },
        Stroke::new(1.5, accent),
    );

    // Corner scale handles (small squares).
    for (i, &c) in corners.iter().enumerate() {
        let h = hot == Some(Handle::Scale(i as u8));
        let r = Rect::from_center_size(c, Vec2::splat(7.0));
        painter.rect(
            r,
            1.0,
            if h {
                accent
            } else {
                Color32::from_rgb(0x16, 0x18, 0x1c)
            },
            Stroke::new(1.5, accent),
            egui::StrokeKind::Middle,
        );
    }

    // Anchor cross (the scale/rotation pivot).
    let anchor_hot = hot == Some(Handle::Anchor);
    let ac = if anchor_hot {
        accent
    } else {
        accent.gamma_multiply(0.9)
    };
    let s = 7.0;
    painter.line_segment(
        [anchor - Vec2::new(s, 0.0), anchor + Vec2::new(s, 0.0)],
        Stroke::new(1.5, ac),
    );
    painter.line_segment(
        [anchor - Vec2::new(0.0, s), anchor + Vec2::new(0.0, s)],
        Stroke::new(1.5, ac),
    );
    painter.circle_stroke(anchor, 4.0, Stroke::new(1.5, ac));
}

/// Paint the interactive **overlays** on top of the rendered preview image: for
/// each visible layer, locatability markers that aren't in the rendered pixels —
/// null pivots, adjustment-layer bounds — and, for the selected layer, its
/// bounding box and editable mask paths. `center`/`scale` are the same comp-px →
/// screen mapping [`paint_image`] returned, so every overlay lands exactly on the
/// rendered pixels (through any letterbox scaling).
///
/// The rendered texture already shows real composited pixels for solids, shapes,
/// text, footage, precomps, effects, masks, mattes, and motion blur, so the
/// overlays only add editor chrome — no placeholder fills.
pub fn paint_overlays(
    painter: &Painter,
    comp: &Comp,
    t: f32,
    selected: Option<usize>,
    center: Pos2,
    scale: f32,
) {
    let half_w = comp.width as f32 * crate::render::LAYER_HALF_FRAC;
    let half_h = comp.height as f32 * crate::render::LAYER_HALF_FRAC;
    let local = [
        (-half_w, -half_h),
        (half_w, -half_h),
        (half_w, half_h),
        (-half_w, half_h),
    ];

    for (i, layer) in comp.layers.iter().enumerate() {
        if !layer.visible || comp.is_matte_source(i) {
            continue;
        }
        let world = comp.world_matrix(i, t);
        let is_selected = selected == Some(i);

        match layer.kind {
            // Nulls are invisible reference handles: draw a small pivot marker at
            // the layer origin so the rig is locatable (not in the rendered frame).
            LayerKind::Null => {
                let (ox, oy) = world.apply(0.0, 0.0);
                let o = center + Vec2::new(ox * scale, oy * scale);
                let s = 8.0;
                painter.line_segment(
                    [o - Vec2::new(s, 0.0), o + Vec2::new(s, 0.0)],
                    Stroke::new(1.0, theme::muted()),
                );
                painter.line_segment(
                    [o - Vec2::new(0.0, s), o + Vec2::new(0.0, s)],
                    Stroke::new(1.0, theme::muted()),
                );
            }
            // Adjustment layers don't draw pixels (the regrade is render-only); a
            // dashed-ish bounds outline keeps them visible & selectable.
            LayerKind::Adjustment => {
                let corners = screen_corners(&local, world, center, scale);
                let mut outline = corners.clone();
                outline.push(corners[0]);
                painter.add(egui::Shape::line(
                    outline,
                    Stroke::new(1.0, theme::muted().gamma_multiply(0.8)),
                ));
            }
            _ => {}
        }

        if is_selected {
            let corners = screen_corners(&local, world, center, scale);
            let mut outline = corners.clone();
            outline.push(corners[0]);
            painter.add(egui::Shape::line(
                outline,
                Stroke::new(1.5, theme::accent()),
            ));
            // Draw the layer's mask paths (layer-local space) so the editable
            // region is visible while the layer is selected.
            paint_masks(painter, layer, center, scale, world);
        }
    }
}

/// Map a layer-local quad's corners through `world` into screen space.
fn screen_corners(
    local: &[(f32, f32); 4],
    world: Affine2,
    center: Pos2,
    scale: f32,
) -> Vec<Pos2> {
    local
        .iter()
        .map(|&(lx, ly)| {
            let (wx, wy) = world.apply(lx, ly);
            center + Vec2::new(wx * scale, wy * scale)
        })
        .collect()
}

/// Paint **onion-skin ghost frames** over the rendered preview: for each [`Ghost`]
/// (the comp sampled at a neighbouring frame), draw every visible layer as a
/// flat, tinted, faded quad/shape — a legible silhouette of where the motion was
/// (cool tint) or is going (warm tint). Drawn *before* the live frame in
/// [`paint_comp`], so the real frame composites on top.
///
/// Ghosts are intentionally cheap: each layer is a single tinted quad (or shape
/// outline) at the ghost's sampled world transform, with no effects / masks /
/// mattes — onion skinning is a *timing* aid, not a render preview. The list is
/// pre-ordered farthest → nearest, so nearer (more opaque) ghosts paint last.
pub fn paint_onion(painter: &Painter, avail: Rect, comp: &Comp, onion: &OnionSkin, t: f32) {
    let ghosts = onion.ghosts(t, comp.fps, comp.duration);
    if ghosts.is_empty() {
        return;
    }
    let (frame, scale) = fit(avail, comp.width, comp.height);
    let center = frame.center();
    for ghost in ghosts {
        paint_ghost_frame(painter, comp, &ghost, center, scale);
    }
}

/// Paint one ghost frame: every visible, pixel-drawing layer of `comp` sampled at
/// `ghost.time`, as a flat quad/outline tinted toward `ghost.tint` and faded by
/// `ghost.opacity`.
fn paint_ghost_frame(painter: &Painter, comp: &Comp, ghost: &Ghost, center: Pos2, scale: f32) {
    let half_w = comp.width as f32 * 0.22;
    let half_h = comp.height as f32 * 0.22;
    let local = [
        (-half_w, -half_h),
        (half_w, -half_h),
        (half_w, half_h),
        (-half_w, half_h),
    ];
    for (i, layer) in comp.layers.iter().enumerate() {
        if !layer.visible || comp.is_matte_source(i) {
            continue;
        }
        // Nulls / adjustments draw nothing of their own — skip them in ghosts
        // (their effect on the frame is render-only and not part of a timing aid).
        if !layer.kind.draws_own_pixels() {
            continue;
        }
        let world = comp.world_matrix(i, ghost.time);
        let op = (comp.layer_opacity(i, ghost.time) * ghost.opacity).clamp(0.0, 1.0);
        if op <= 0.0 {
            continue;
        }
        let corners: Vec<Pos2> = local
            .iter()
            .map(|&(lx, ly)| {
                let (wx, wy) = world.apply(lx, ly);
                center + Vec2::new(wx * scale, wy * scale)
            })
            .collect();
        let fill = Color32::from_rgba_unmultiplied(
            (ghost.tint[0] * 255.0).round() as u8,
            (ghost.tint[1] * 255.0).round() as u8,
            (ghost.tint[2] * 255.0).round() as u8,
            (op * 255.0).round() as u8,
        );
        painter.add(egui::Shape::convex_polygon(corners, fill, Stroke::NONE));
    }
}


/// Paint the selected layer's **mask** paths as outlines, transformed through
/// the layer's `world` matrix into screen space.
///
/// Each active mask's flattened polygon is stroked closed; the stroke color
/// hints at the mode (subtractive/inverted masks are dimmed) so the carve reads
/// at a glance. This is an editor overlay only — coverage itself is computed in
/// the renderer.
fn paint_masks(painter: &Painter, layer: &PulseLayer, center: Pos2, scale: f32, world: Affine2) {
    for mask in &layer.masks {
        if !mask.is_active() {
            continue;
        }
        let poly = mask.flatten();
        if poly.len() < 2 {
            continue;
        }
        let mut pts: Vec<Pos2> = poly
            .iter()
            .map(|&(lx, ly)| {
                let (wx, wy) = world.apply(lx, ly);
                center + Vec2::new(wx * scale, wy * scale)
            })
            .collect();
        pts.push(pts[0]); // close the path
                          // Subtract / inverted masks read as "removing" — dim them; Add/Intersect
                          // are the "keep" ops — draw them in the accent color.
        let removes = mask.inverted || matches!(mask.mode, MaskMode::Subtract);
        let color = if removes {
            theme::muted().gamma_multiply(0.9)
        } else {
            theme::accent().gamma_multiply(0.9)
        };
        painter.add(egui::Shape::line(pts, Stroke::new(1.0, color)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comp::{FootageSource, FrameCache, LayerKind, PulseLayer};
    use crate::gizmo::screen_to_comp;

    /// A minimal one-comp project: a comp with `id`, given size, and the supplied
    /// layers.
    fn comp_of(id: u64, w: u32, h: u32, layers: Vec<PulseLayer>) -> Comp {
        let mut c = Comp::new();
        c.id = id;
        c.width = w;
        c.height = h;
        c.layers = layers;
        c
    }

    // --- cache signature (state hash) + work-area frame count -------------

    #[test]
    fn state_hash_stable_when_nothing_changes() {
        let comps = vec![comp_of(1, 640, 360, vec![])];
        assert_eq!(
            state_hash(&comps, 1),
            state_hash(&comps, 1),
            "identical comps must hash equal (the cache stays valid)"
        );
    }

    #[test]
    fn state_hash_changes_with_comp_state_edit() {
        let base = vec![comp_of(1, 640, 360, vec![])];
        let a = state_hash(&base, 1);

        // Add a layer — the serialized state, hence the hash, must change.
        let edited = vec![comp_of(
            1,
            640,
            360,
            vec![PulseLayer::new("Solid", [0.5, 0.5, 0.5, 1.0])],
        )];
        let b = state_hash(&edited, 1);
        assert_ne!(a, b, "a layer edit must invalidate the cache");

        // Move a keyframe on that layer — also a state change.
        let mut moved = edited.clone();
        moved[0].layers[0].x.set_key(0.0, 123.0);
        let c = state_hash(&moved, 1);
        assert_ne!(b, c, "a keyframe edit must invalidate the cache");
    }

    #[test]
    fn state_hash_independent_of_time() {
        // The cache signature has no time component (frames are keyed by index),
        // so the same comps hash the same regardless of playhead — only edits /
        // resize (a separate dims component) invalidate the cache.
        let comps = vec![comp_of(1, 640, 360, vec![])];
        assert_eq!(state_hash(&comps, 1), state_hash(&comps, 1));
    }

    #[test]
    fn frame_count_covers_the_work_area() {
        // 5 s @ 30 fps → 150 frames (indices 0..=149); always at least one frame.
        assert_eq!(frame_count(5.0, 30.0), 150);
        assert_eq!(frame_count(1.0, 24.0), 24);
        assert_eq!(frame_count(0.0, 30.0), 1, "a degenerate comp still has one frame");
        assert_eq!(frame_count(2.0, 0.0), 2, "fps floors at 1 so the count is finite");
    }

    // --- comp-space <-> display-rect mapping round-trip --------------------

    /// Round-trip a comp-space point through `comp_fit` (comp → screen) and
    /// `screen_to_comp` (screen → comp) and assert it returns to the origin, for a
    /// letterboxed (non-matching aspect) viewport.
    fn assert_roundtrip(avail: Rect, w: u32, h: u32, cx: f32, cy: f32) {
        let (center, scale) = comp_fit(avail, w, h);
        // comp → screen (the exact mapping paint_image / overlays use).
        let screen = center + Vec2::new(cx * scale, cy * scale);
        // screen → comp.
        let (rx, ry) = screen_to_comp(screen.x, screen.y, center.x, center.y, scale);
        assert!((rx - cx).abs() < 1e-3, "x round-trip {cx} -> {rx}");
        assert!((ry - cy).abs() < 1e-3, "y round-trip {cy} -> {ry}");
    }

    #[test]
    fn mapping_roundtrips_with_letterboxing() {
        // A wide comp in a tall viewport letterboxes top/bottom; a tall comp in a
        // wide viewport pillarboxes — both must round-trip.
        let wide_view = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(800.0, 600.0));
        // 16:9 comp inside a 4:3 viewport (letterbox), several comp-space points.
        for (cx, cy) in [(0.0, 0.0), (320.0, -180.0), (-640.0, 360.0)] {
            assert_roundtrip(wide_view, 1920, 1080, cx, cy);
        }
        // A tall comp inside the same viewport (pillarbox).
        for (cx, cy) in [(0.0, 0.0), (100.0, 200.0), (-150.0, -300.0)] {
            assert_roundtrip(wide_view, 720, 1280, cx, cy);
        }
    }

    #[test]
    fn comp_center_maps_to_fitted_rect_center() {
        let avail = Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 400.0));
        let (center, _scale) = comp_fit(avail, 1920, 1080);
        // The comp origin (0,0) is the center of the fitted rect, which is the
        // center of a symmetric available area.
        assert!((center.x - avail.center().x).abs() < 1e-3);
        assert!((center.y - avail.center().y).abs() < 1e-3);
    }

    // --- preview resolution cap -------------------------------------------

    #[test]
    fn preview_dims_caps_long_edge_keeping_aspect() {
        // 1920x1080 capped to 1280 -> 1280x720 (16:9 preserved).
        let (w, h) = crate::render::preview_dims(1920, 1080, 1280);
        assert_eq!((w, h), (1280, 720));
        // Small comps are not upscaled.
        assert_eq!(crate::render::preview_dims(320, 180, 1280), (320, 180));
        // Portrait long edge is the height.
        let (w, h) = crate::render::preview_dims(1080, 1920, 1280);
        assert_eq!((w, h), (720, 1280));
    }

    // --- pool worker render + stale-epoch skip ----------------------------

    #[test]
    fn worker_renders_job_and_skips_stale_epoch() {
        // A pool worker renders a job tagged with the current epoch and returns it
        // by frame index; a job whose epoch was superseded (an edit/resize bumped
        // the shared epoch) is skipped without producing a frame — so the first
        // result the UI sees is the *current*-epoch job, not the stale one.
        let comps = Arc::new(vec![comp_of(
            1,
            64,
            36,
            vec![PulseLayer::new("Solid", [1.0, 0.0, 0.0, 1.0])],
        )]);
        let (job_tx, job_rx) = std::sync::mpsc::channel();
        let (res_tx, res_rx) = std::sync::mpsc::channel();
        let epoch = Arc::new(AtomicU64::new(5));
        let handle = {
            let ep = epoch.clone();
            std::thread::spawn(move || worker_loop(job_rx, res_tx, ep))
        };
        // Stale job (epoch 1) — must be skipped.
        job_tx
            .send(RenderJob {
                epoch: 1,
                idx: 0,
                t: 0.0,
                id: 1,
                cap: 1280,
                comps: comps.clone(),
            })
            .unwrap();
        // Current job (epoch 5) — renders and returns.
        job_tx
            .send(RenderJob {
                epoch: 5,
                idx: 7,
                t: 0.0,
                id: 1,
                cap: 1280,
                comps: comps.clone(),
            })
            .unwrap();
        let done = res_rx.recv().expect("the current-epoch job returns a frame");
        assert_eq!(done.epoch, 5);
        assert_eq!(done.idx, 7, "the stale (epoch-1) job was skipped, not returned");
        assert!(done.frame.width >= 1 && done.frame.height >= 1);
        drop(job_tx);
        handle.join().expect("worker exits when its channel closes");
    }

    #[test]
    fn cache_readiness_and_full_predicate() {
        // The pacing gates (is_frame_ready / fully_cached) read straight off the
        // RAM cache: not ready / not full until every work-area frame is resident.
        let mut p = PreviewRenderer::default();
        p.fps = 30.0;
        p.n_frames = 3; // frames 0, 1, 2
        assert!(!p.fully_cached());
        assert!(!p.is_frame_ready(0.0));

        for idx in 0..3 {
            p.insert_frame(
                idx,
                Frame {
                    width: 2,
                    height: 2,
                    pixels: vec![0u8; 16],
                },
            );
        }
        assert!(p.fully_cached(), "every work-area frame is cached");
        assert!(p.is_frame_ready(0.0)); // frame 0
        assert!(p.is_frame_ready(2.0 / 30.0)); // frame 2
        assert!(
            p.is_frame_ready(10.0),
            "a time past the work area clamps to the cached last frame"
        );
        assert_eq!(p.bytes, 16 * 3, "byte tally tracks the cached frames");
    }

    #[test]
    fn cache_progress_tracks_first_pass_fill() {
        // The transport "caching…" readout reads `cache_progress()`: 0 frames
        // until a comp is registered, then `cached` climbs to `total` as the
        // first pass fills the RAM cache, reaching equality exactly when
        // `fully_cached` is true.
        let mut p = PreviewRenderer::default();
        assert_eq!(p.cache_progress(), (0, 0), "no comp yet → nothing to cache");

        p.fps = 30.0;
        p.n_frames = 3; // frames 0, 1, 2
        assert_eq!(p.cache_progress(), (0, 3), "first pass not started");

        p.insert_frame(
            0,
            Frame {
                width: 2,
                height: 2,
                pixels: vec![0u8; 16],
            },
        );
        assert_eq!(p.cache_progress(), (1, 3), "one frame cached mid-fill");
        assert!(!p.fully_cached());

        for idx in 1..3 {
            p.insert_frame(
                idx,
                Frame {
                    width: 2,
                    height: 2,
                    pixels: vec![0u8; 16],
                },
            );
        }
        assert_eq!(
            p.cache_progress(),
            (3, 3),
            "cached == total once the work area is full"
        );
        assert!(p.fully_cached());
    }

    // --- persistent FrameCache reuse (no re-decode for the same frame) -----

    #[test]
    fn persistent_cache_decodes_a_frame_once_across_preview_renders() {
        // A footage layer pointing at a (missing) still: the decode is attempted
        // once and the result cached (a missing file caches as a failure), so a
        // second preview render at the same source frame does NOT re-decode — the
        // cache still holds exactly one entry. This is the property the persistent
        // preview cache relies on (the offline export keeps its own per-run cache).
        let mut footage = PulseLayer::of_kind(LayerKind::Footage, "F", [0.5, 0.5, 0.5, 1.0]);
        footage.footage.source = Some(FootageSource::still("preview_cache_probe.png"));
        let comps = vec![comp_of(1, 64, 36, vec![footage])];

        let mut cache = FrameCache::new();
        let _ = crate::render::render_preview_frame(&comps, 1, 0.0, 1280, &mut cache);
        assert_eq!(cache.len(), 1, "first render decodes the source once");
        let before = cache.len();
        // Re-render the same time: the source frame is already resident, so no new
        // decode happens and the cache size is unchanged.
        let _ = crate::render::render_preview_frame(&comps, 1, 0.0, 1280, &mut cache);
        assert_eq!(cache.len(), before, "second render must reuse the cache");
    }
}

