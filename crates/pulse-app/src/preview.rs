//! The preview surface: a **render preview**. The composition is rendered at the
//! current playhead time through the *real* offline compositor
//! ([`render`](crate::render)) at a capped preview resolution, uploaded as an
//! egui texture, and drawn as the preview image — so footage frames, precomps,
//! effects, masks, mattes, motion blur, time-remap, and expressions all show
//! **real composited pixels** (the same result an export produces).
//!
//! The render is **cached**: a fingerprint of `(time, comp state, target size)`
//! gates re-rendering, so a static frame is rendered once and only re-rendered
//! when the playhead moves, the comp/layers change, or the viewport resizes. A
//! **persistent** [`FrameCache`](crate::comp::FrameCache) is threaded through the
//! preview renders so footage isn't re-decoded every frame.
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
use std::sync::mpsc::{Receiver, Sender};
use std::thread::JoinHandle;

use crate::comp::Affine2;

/// The capped long-edge resolution (px) of the interactive render preview. Large
/// comps render downscaled to this so scrubbing stays responsive; the cache then
/// avoids re-rendering an unchanged frame.
const PREVIEW_CAP: u32 = 1280;

/// Drives the interactive preview by rendering the comp **off the UI thread**.
///
/// The expensive part — compositing the comp through the real offline renderer at
/// a capped resolution — runs on a background [`worker_loop`] thread that owns a
/// persistent [`FrameCache`](crate::comp::FrameCache). The UI thread only ever
/// *uploads* the latest finished frame and *requests* a new render when the shown
/// frame is stale; it never composites, so playback and scrubbing never block
/// input. When renders can't keep up with playback the worker **coalesces** its
/// queue to the most recent request (drop-frame), so the preview shows the newest
/// frame it can produce instead of building an unbounded backlog.
#[derive(Default)]
pub struct PreviewRenderer {
    /// The uploaded preview texture (the last finished comp render — capped-res,
    /// sRGB pixels).
    tex: Option<TextureHandle>,
    /// The [`PreviewKey`] of the frame currently uploaded to [`tex`](Self::tex).
    shown_key: Option<PreviewKey>,
    /// The playhead time the currently shown frame was rendered for. Overlays
    /// align to *this* (not the live playhead) so they don't lead the pixels when
    /// the off-thread render lags during playback.
    shown_time: Option<f32>,
    /// The most recent key handed to the worker, so the same frame isn't queued
    /// twice while it's still rendering (avoids flooding the request channel).
    last_sent: Option<PreviewKey>,
    /// Request channel to the background render worker (spawned on first use).
    req_tx: Option<Sender<RenderRequest>>,
    /// Result channel from the worker (finished, key-tagged frames).
    res_rx: Option<Receiver<RenderResult>>,
    /// The worker thread handle, kept alive for the app's lifetime. Dropping the
    /// renderer closes [`req_tx`](Self::req_tx), which ends the worker loop.
    #[allow(dead_code)]
    worker: Option<JoinHandle<()>>,
}

/// A render job handed to the background worker: the project comps to render,
/// which comp + time to render, the preview resolution cap, and the
/// [`PreviewKey`] the result is tagged with (so the UI knows which frame returned).
struct RenderRequest {
    comps: Vec<Comp>,
    id: u64,
    t: f32,
    cap: u32,
    key: PreviewKey,
}

/// A finished render from the worker: the tagged [`PreviewKey`], the playhead
/// time it was rendered for (echoed back so overlays can align to the shown
/// frame), plus the rendered (capped-resolution sRGB) [`Frame`].
struct RenderResult {
    key: PreviewKey,
    t: f32,
    frame: Frame,
}

/// The background preview-render loop. Owns a persistent [`FrameCache`] (so a
/// footage source decodes once and is reused across renders) and composites
/// requests off the UI thread. Before each render it **coalesces** any queued
/// requests down to the most recent (dropping stale frames), so a slow render
/// never builds an unbounded backlog. Exits when the request channel closes (the
/// owning [`PreviewRenderer`] is dropped).
fn worker_loop(req_rx: Receiver<RenderRequest>, res_tx: Sender<RenderResult>) {
    let mut cache = crate::comp::FrameCache::new();
    while let Ok(mut req) = req_rx.recv() {
        // Coalesce: skip ahead to the newest pending request (drop stale frames).
        while let Ok(newer) = req_rx.try_recv() {
            req = newer;
        }
        let frame =
            crate::render::render_preview_frame(&req.comps, req.id, req.t, req.cap, &mut cache);
        if res_tx
            .send(RenderResult {
                key: req.key,
                t: req.t,
                frame,
            })
            .is_err()
        {
            break; // UI gone — stop.
        }
    }
}

/// A cache key fingerprinting everything the rendered preview frame depends on:
/// the playhead time (quantized to avoid float-noise churn), the target render
/// dimensions, and a hash of the project comp state at that time.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PreviewKey {
    /// Playhead time quantized to milliticks (1e-4 s) so equal times compare
    /// equal across frames but a real scrub/playback step changes the key.
    time_q: i64,
    /// Target render width/height (px) — a viewport resize re-renders.
    dims: (u32, u32),
    /// Hash of the serialized project comps (id, size, layers, keyframes, …); any
    /// edit to the comp / its layers changes it, a no-op leaves it stable.
    state: u64,
}

impl PreviewKey {
    /// Build the fingerprint for rendering comp `id` of `comps` at time `t` into a
    /// `dims`-sized target. The state hash serializes the comps to JSON and hashes
    /// the bytes — robust to any field change without hand-maintaining a hasher.
    pub fn new(comps: &[Comp], id: u64, t: f32, dims: (u32, u32)) -> Self {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        id.hash(&mut h);
        // Serialize the comps; serialization failure (never expected for these
        // plain structs) degrades to hashing nothing — still a valid, stable key.
        if let Ok(json) = serde_json::to_vec(comps) {
            json.hash(&mut h);
        }
        Self {
            time_q: (t as f64 * 10_000.0).round() as i64,
            dims,
            state: h.finish(),
        }
    }
}

impl PreviewRenderer {
    /// Spawn the background render worker on first use (idempotent). The worker
    /// owns the persistent [`FrameCache`](crate::comp::FrameCache) and renders
    /// off the UI thread.
    fn ensure_worker(&mut self) {
        if self.req_tx.is_some() {
            return;
        }
        let (req_tx, req_rx) = std::sync::mpsc::channel::<RenderRequest>();
        let (res_tx, res_rx) = std::sync::mpsc::channel::<RenderResult>();
        let worker = std::thread::Builder::new()
            .name("pulse-preview".into())
            .spawn(move || worker_loop(req_rx, res_tx))
            .ok();
        self.req_tx = Some(req_tx);
        self.res_rx = Some(res_rx);
        self.worker = worker;
    }

    /// Return the latest rendered preview texture for comp `id` of `comps` at time
    /// `t`, rendering **off the UI thread**.
    ///
    /// Finished frames from the worker are picked up and uploaded here; a fresh
    /// render is requested (at most once per distinct frame) whenever the displayed
    /// frame is stale. The render size is the comp's aspect capped to
    /// [`PREVIEW_CAP`] px on the long edge, so a viewport resize only changes how
    /// the texture is *drawn*, not what is rendered. Because the UI thread never
    /// composites, playback and interaction stay responsive even when a frame is
    /// expensive — the preview simply drops frames it can't keep up with (the
    /// worker coalesces a backlog to its most recent request).
    ///
    /// `comps` is taken by value: it is moved into the render request when one is
    /// issued (no extra clone), and dropped otherwise.
    pub fn texture(
        &mut self,
        ctx: &egui::Context,
        comps: Vec<Comp>,
        id: u64,
        t: f32,
    ) -> Option<TextureHandle> {
        self.ensure_worker();

        // 1. Drain finished renders and upload the newest (older ones are stale).
        let mut newest: Option<RenderResult> = None;
        if let Some(rx) = &self.res_rx {
            while let Ok(r) = rx.try_recv() {
                newest = Some(r);
            }
        }
        if let Some(r) = newest {
            let image = ColorImage::from_rgba_unmultiplied(
                [r.frame.width as usize, r.frame.height as usize],
                &r.frame.pixels,
            );
            match &mut self.tex {
                // Re-use the existing GPU texture slot, just swap its pixels.
                Some(tex) => tex.set(image, egui::TextureOptions::LINEAR),
                None => {
                    self.tex =
                        Some(ctx.load_texture("pulse_preview", image, egui::TextureOptions::LINEAR));
                }
            }
            self.shown_key = Some(r.key);
            self.shown_time = Some(r.t);
        }

        // 2. Request a render when the displayed frame is stale and this exact
        //    frame isn't already queued (avoid flooding the worker's channel).
        let dims = comps
            .iter()
            .find(|c| c.id == id)
            .map(|c| crate::render::preview_dims(c.width, c.height, PREVIEW_CAP))
            .unwrap_or((1, 1));
        let desired = PreviewKey::new(&comps, id, t, dims);
        let up_to_date = self.shown_key == Some(desired);
        let already_queued = self.last_sent == Some(desired);
        if !up_to_date && !already_queued {
            if let Some(tx) = &self.req_tx {
                let req = RenderRequest {
                    comps,
                    id,
                    t,
                    cap: PREVIEW_CAP,
                    key: desired,
                };
                if tx.send(req).is_ok() {
                    self.last_sent = Some(desired);
                }
            }
        }

        // 3. Keep repainting until the shown frame matches the request, so the
        //    finished render is picked up promptly (playback already repaints).
        if !up_to_date {
            ctx.request_repaint();
        }

        self.tex.clone()
    }

    /// The playhead time the currently displayed preview frame was rendered for.
    ///
    /// Because the render runs off the UI thread, during playback the shown frame
    /// **lags** the live playhead. Editor overlays (selection box, motion path,
    /// mask outlines, the transform gizmo) must be drawn at *this* time so they
    /// land on the pixels actually on screen rather than leading them. `None`
    /// until the first frame is uploaded (the caller falls back to the live time).
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

    // --- Preview cache key fingerprint -------------------------------------

    #[test]
    fn preview_key_stable_when_nothing_changes() {
        let comps = vec![comp_of(1, 640, 360, vec![])];
        let a = PreviewKey::new(&comps, 1, 1.0, (640, 360));
        let b = PreviewKey::new(&comps, 1, 1.0, (640, 360));
        assert_eq!(a, b, "same time + state + dims must produce the same key");
    }

    #[test]
    fn preview_key_changes_with_time() {
        let comps = vec![comp_of(1, 640, 360, vec![])];
        let a = PreviewKey::new(&comps, 1, 1.0, (640, 360));
        let b = PreviewKey::new(&comps, 1, 1.5, (640, 360));
        assert_ne!(a, b, "advancing the playhead must invalidate the cache");
    }

    #[test]
    fn preview_key_quantizes_time_noise() {
        // Sub-quantum jitter (< 1e-4 s) collapses to the same key, so float noise
        // doesn't churn the cache while parked on a frame.
        let comps = vec![comp_of(1, 640, 360, vec![])];
        let a = PreviewKey::new(&comps, 1, 1.0, (640, 360));
        let b = PreviewKey::new(&comps, 1, 1.0 + 1e-6, (640, 360));
        assert_eq!(a, b);
    }

    #[test]
    fn preview_key_changes_with_comp_state_edit() {
        let base = vec![comp_of(1, 640, 360, vec![])];
        let a = PreviewKey::new(&base, 1, 1.0, (640, 360));

        // Add a layer — the serialized state, hence the key, must change.
        let edited = vec![comp_of(
            1,
            640,
            360,
            vec![PulseLayer::new("Solid", [0.5, 0.5, 0.5, 1.0])],
        )];
        let b = PreviewKey::new(&edited, 1, 1.0, (640, 360));
        assert_ne!(a, b, "a layer edit must invalidate the cache");

        // Move a keyframe on that layer — also a state change.
        let mut moved = edited.clone();
        moved[0].layers[0].x.set_key(0.0, 123.0);
        let c = PreviewKey::new(&moved, 1, 1.0, (640, 360));
        assert_ne!(b, c, "a keyframe edit must invalidate the cache");
    }

    #[test]
    fn preview_key_changes_with_target_size() {
        let comps = vec![comp_of(1, 640, 360, vec![])];
        let a = PreviewKey::new(&comps, 1, 1.0, (640, 360));
        let b = PreviewKey::new(&comps, 1, 1.0, (320, 180));
        assert_ne!(a, b, "a viewport/resolution change must re-render");
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

    // --- off-thread render worker -----------------------------------------

    #[test]
    fn worker_renders_and_returns_tagged_frame() {
        // The background worker composites a request off the UI thread and sends
        // back a frame tagged with the request's key — the property the preview
        // relies on to match a finished render to the frame it asked for.
        let comps = vec![comp_of(
            1,
            64,
            36,
            vec![PulseLayer::new("Solid", [1.0, 0.0, 0.0, 1.0])],
        )];
        let key = PreviewKey::new(&comps, 1, 0.0, (64, 36));
        let (req_tx, req_rx) = std::sync::mpsc::channel();
        let (res_tx, res_rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || worker_loop(req_rx, res_tx));
        req_tx
            .send(RenderRequest {
                comps,
                id: 1,
                t: 0.0,
                cap: 1280,
                key,
            })
            .unwrap();
        let result = res_rx.recv().expect("worker returns a rendered frame");
        assert_eq!(result.key, key, "the worker tags the frame with the request key");
        assert_eq!(result.t, 0.0, "the worker echoes the request's playhead time");
        assert!(result.frame.width >= 1 && result.frame.height >= 1);
        // Closing the request channel ends the worker loop cleanly.
        drop(req_tx);
        handle.join().expect("worker thread exits when its channel closes");
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

