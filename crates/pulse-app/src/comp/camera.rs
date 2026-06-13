//! The composition **camera** and the pure 3-D → comp-plane **perspective
//! projection** that 3-D layers are placed through.
//!
//! Pulse's compositor is a pure 2-D software rasterizer: every layer is drawn
//! through a comp-space [`Affine2`](super::Affine2) matrix. To add **basic 3-D
//! layers + a camera** without rewriting that pipeline, this module supplies the
//! pure math that turns a 3-D point — a comp-space `(x, y)` plus a **depth** `z`
//! (After-Effects convention: `+z` recedes *into* the screen, away from the
//! viewer) — into the 2-D comp-plane point it projects to through the camera,
//! together with the **perspective scale** at that point and its **camera-space
//! depth** (used to painter's-sort 3-D layers back-to-front).
//!
//! ## Coordinate frame
//!
//! Comp space has its origin at the comp **center**, `+x` right, `+y` **down**
//! (screen convention, matching the rasterizer and preview), and `+z` into the
//! screen. The camera looks down `+z`.
//!
//! ## Default camera = today's flat 2-D look
//!
//! The [`Default`] camera sits on the `-z` axis at distance
//! [`Camera::default_distance`] — the classic After-Effects placement where the
//! comp plane at `z = 0` exactly fills the frame at **unit scale**. With the
//! default camera a point at `z = 0` projects to **itself** with scale `1.0`
//! (`project` is the identity on the comp plane), so a 3-D layer at `Z = 0`
//! lands pixel-for-pixel where its 2-D twin would — the back-compat guarantee.
//! Pushing a layer to `z > 0` shrinks it (`scale < 1`); pulling it to `z < 0`
//! enlarges it.

use serde::{Deserialize, Serialize};

/// A composition camera: a position, a point of interest it looks at, and a
/// lens (field of view). The view it produces is what 3-D layers are projected
/// through onto the comp plane.
///
/// Single-node (free) camera model: orientation comes from looking at the
/// **point of interest**. The lens is described by a **vertical field of view**
/// in degrees (After Effects exposes both *Angle of View* and *Focal Length* /
/// *Zoom*; FOV is the canonical one and the focal length is derived from it via
/// the comp height — see [`Camera::focal_length`] / [`Camera::with_focal`]).
///
/// Every field is `serde`-defaulted (the whole struct is `serde`-defaulted on
/// the comp), so a pre-3-D `.pulse` file with no `camera` key loads the
/// [`Default`] camera — which reproduces today's flat 2-D look exactly.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Camera {
    /// Camera position in comp space (origin at comp center, `+y` down, `+z`
    /// into the screen).
    #[serde(default = "Camera::default_position")]
    pub position: [f32; 3],
    /// The point the camera looks at (comp space). The default sits at the comp
    /// center on the `z = 0` plane, so the default camera looks straight down
    /// `+z`.
    #[serde(default = "Camera::default_poi")]
    pub poi: [f32; 3],
    /// **Vertical field of view**, degrees. Smaller = longer lens (more
    /// telephoto, flatter perspective); larger = wider lens.
    #[serde(default = "Camera::default_fov")]
    pub fov_deg: f32,
    /// **Depth of field** master switch. When `false` (the default) the camera
    /// renders every 3-D layer perfectly sharp — the pre-DoF behavior — so any
    /// existing comp (and a serde-default / `dof`-less `.pulse` file) is
    /// unchanged. When `true` the lens defocuses 3-D layers by how far their
    /// camera-space depth is from [`focus_distance`](Self::focus_distance),
    /// scaled by [`aperture`](Self::aperture).
    #[serde(default)]
    pub dof_enabled: bool,
    /// The camera-space depth (comp px, along the view axis) that is in perfect
    /// focus: a 3-D layer whose [`layer_depth`](super::Comp::layer_depth) equals
    /// this renders sharp; layers nearer or farther blur. Defaults to the
    /// camera-to-point-of-interest distance (`0.0` is a sentinel meaning "use the
    /// focal plane"). Only consulted when [`dof_enabled`](Self::dof_enabled).
    #[serde(default)]
    pub focus_distance: f32,
    /// **Aperture / blur strength**: how aggressively out-of-focus layers blur.
    /// It is the blur radius in comp px produced **per unit of relative depth
    /// error** (see [`coc_blur_radius`](Self::coc_blur_radius)); a wider aperture
    /// (larger value) gives a shallower depth of field. `0.0` (the default) means
    /// no blur even with [`dof_enabled`](Self::dof_enabled) on.
    #[serde(default)]
    pub aperture: f32,
}

/// One layer's quad projected through the camera onto the comp plane: the
/// comp-space screen position its anchor/center lands at, the **uniform
/// perspective scale** to draw it at, and its **camera-space depth** for sorting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Projected {
    /// Comp-space `(x, y)` the point projects to (origin at comp center).
    pub screen: (f32, f32),
    /// Uniform perspective scale at that depth: `1.0` on the focal plane,
    /// `< 1.0` farther away, `> 1.0` nearer. `0.0` when the point is at or
    /// behind the camera (degenerate — the caller treats it as not visible).
    pub scale: f32,
    /// Camera-space depth (distance along the camera's view axis). Larger =
    /// farther from the camera; the painter's z-sort draws larger depths first.
    pub depth: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Camera {
            position: Self::default_position(),
            poi: Self::default_poi(),
            fov_deg: Self::default_fov(),
            dof_enabled: false,
            focus_distance: 0.0,
            aperture: 0.0,
        }
    }
}

impl Camera {
    /// The default vertical field of view (degrees). Paired with
    /// [`default_distance`](Self::default_distance) below so the comp plane at
    /// `z = 0` fills the frame at unit scale.
    pub const DEFAULT_FOV_DEG: f32 = 54.0;

    fn default_fov() -> f32 {
        Self::DEFAULT_FOV_DEG
    }

    fn default_position() -> [f32; 3] {
        // Filled in per-comp by `for_comp` / `world` using the comp height; the
        // serde default (no comp context) uses a 720p-height-derived distance so
        // a hand-written camera-less point is still sane. The renderer always
        // passes the comp height to `project_through`, which recomputes the
        // focal distance from the live FOV + height, so this constant only seeds
        // a default struct.
        [0.0, 0.0, -Self::distance_for(720.0, Self::DEFAULT_FOV_DEG)]
    }

    fn default_poi() -> [f32; 3] {
        [0.0, 0.0, 0.0]
    }

    /// The camera-to-comp-plane **distance** that makes a comp of pixel height
    /// `comp_h` exactly fill the frame at unit scale for vertical FOV `fov_deg`:
    /// `d = (comp_h / 2) / tan(fov / 2)`. This is the After-Effects relationship
    /// between *Angle of View*, comp size, and the camera's *Zoom* distance.
    pub fn distance_for(comp_h: f32, fov_deg: f32) -> f32 {
        let half = (fov_deg.clamp(1.0, 179.0) * 0.5).to_radians();
        (comp_h * 0.5) / half.tan().max(1e-6)
    }

    /// The default camera distance for a comp of height `comp_h` at the default
    /// FOV — the `-z` offset of the [`Default`] camera for that comp.
    pub fn default_distance(comp_h: f32) -> f32 {
        Self::distance_for(comp_h, Self::DEFAULT_FOV_DEG)
    }

    /// This camera's **live vertical field of view** (degrees) for a comp of
    /// pixel height `comp_h`, derived from the actual camera-to-poi distance
    /// (the focal length the projection uses): `fov = 2·atan((comp_h/2) / f)`.
    /// This is the authoritative FOV (the stored `fov_deg` is only a seed/cache);
    /// the UI reads and edits the lens through this so it always matches what the
    /// renderer projects.
    pub fn vertical_fov(&self, comp_h: f32) -> f32 {
        let f = self.focal_px(comp_h);
        (2.0 * ((comp_h * 0.5) / f.max(1e-6)).atan()).to_degrees()
    }

    /// **Dolly** the camera to the focal distance that yields vertical FOV
    /// `fov_deg` for a comp of height `comp_h`: moves the camera along its view
    /// axis (keeping its look direction + point of interest) so the projection
    /// matches the requested FOV, and updates the cached `fov_deg`. The UI's FOV
    /// and focal-length sliders drive this.
    pub fn set_vertical_fov(&mut self, fov_deg: f32, comp_h: f32) {
        let target = Self::distance_for(comp_h, fov_deg);
        self.dolly_to(target);
        self.fov_deg = fov_deg.clamp(1.0, 179.0);
    }

    /// Move the camera to be `dist` comp-pixels from its point of interest along
    /// its current view axis (a dolly in/out). Keeps the look direction; falls
    /// back to the `-z` axis if the camera sits on its poi.
    fn dolly_to(&mut self, dist: f32) {
        let dir = normalize(sub(self.position, self.poi)).unwrap_or([0.0, 0.0, -1.0]);
        let d = dist.max(1.0);
        self.position = [
            self.poi[0] + dir[0] * d,
            self.poi[1] + dir[1] * d,
            self.poi[2] + dir[2] * d,
        ];
    }

    /// This camera's **focal length** in the After-Effects sense — millimetres on
    /// a 36 mm-wide-equivalent horizontal film back, derived from the live
    /// vertical FOV and the comp aspect. Exposed for the UI so the lens reads in
    /// familiar mm. `focal_mm = (film_back/2) / tan(h_fov/2)`.
    pub fn focal_length(&self, comp_w: f32, comp_h: f32) -> f32 {
        const FILM_BACK_MM: f32 = 36.0;
        let aspect = (comp_w / comp_h.max(1.0)).max(1e-3);
        let v_half = (self.vertical_fov(comp_h).clamp(1.0, 179.0) * 0.5).to_radians();
        let h_half = (v_half.tan() * aspect).atan();
        (FILM_BACK_MM * 0.5) / h_half.tan().max(1e-6)
    }

    /// Dolly the camera to a desired **focal length** (mm, 36 mm horizontal film
    /// back) for the given comp size — the inverse of [`focal_length`]. Drives the
    /// same lens (camera distance) as [`set_vertical_fov`].
    pub fn set_focal_length(&mut self, focal_mm: f32, comp_w: f32, comp_h: f32) {
        const FILM_BACK_MM: f32 = 36.0;
        let aspect = (comp_w / comp_h.max(1.0)).max(1e-3);
        let h_half = ((FILM_BACK_MM * 0.5) / focal_mm.max(1e-3)).atan();
        let v_half = (h_half.tan() / aspect).atan();
        self.set_vertical_fov((v_half.to_degrees() * 2.0).clamp(1.0, 179.0), comp_h);
    }

    /// The orthonormal camera basis `(right, up, forward)` in comp space.
    /// `forward` points from the camera toward its point of interest; `right`
    /// and `up` complete the frame with world-up `(0, 1, 0)` (comp `+y` is down,
    /// the screen convention). Chosen so the **default camera** (on `-z` looking
    /// down `+z`) yields the standard axes `right = +x`, `up = +y`, so a `z = 0`
    /// point projects to itself (the back-compat identity). Falls back to an
    /// axis-aligned basis when the camera sits on its point of interest or looks
    /// straight along world-up (degenerate cross products).
    fn basis(&self) -> ([f32; 3], [f32; 3], [f32; 3]) {
        let fwd = normalize(sub(self.poi, self.position)).unwrap_or([0.0, 0.0, 1.0]);
        let world_up = [0.0, 1.0, 0.0];
        // right = world_up × forward (so forward=+z, up=+y ⇒ right=+x); up
        // completes the frame as forward × right.
        let right = normalize(cross(world_up, fwd)).unwrap_or([1.0, 0.0, 0.0]);
        let up = normalize(cross(fwd, right)).unwrap_or([0.0, 1.0, 0.0]);
        (right, up, fwd)
    }

    /// The **focal length** in comp pixels used by the perspective divide: the
    /// camera-to-**point-of-interest** distance, so a point lying on the focal
    /// (point-of-interest) plane projects at unit scale. This makes the identity
    /// **position-driven** — for the [`Default`] camera (poi at the origin on the
    /// `z = 0` plane) any layer at `Z = 0` projects to itself, *independent of
    /// comp size*. Falls back to the comp-height/FOV formula when the camera sits
    /// on its point of interest (degenerate zero distance).
    fn focal_px(&self, comp_h: f32) -> f32 {
        let d = {
            let v = sub(self.poi, self.position);
            (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
        };
        if d > 1e-4 {
            d
        } else {
            Self::distance_for(comp_h, self.fov_deg)
        }
    }

    /// The camera-space depth (comp px) that is in **perfect focus**: the stored
    /// [`focus_distance`](Self::focus_distance) when it is set (positive), else
    /// the camera-to-point-of-interest distance — the focal plane that projects
    /// at unit scale (see [`focal_px`](Self::focal_px)). Falling back to the focal
    /// plane makes "DoF on, focus untouched" focus on whatever the camera is
    /// already aimed at.
    pub fn effective_focus(&self, comp_h: f32) -> f32 {
        if self.focus_distance > 0.0 {
            self.focus_distance
        } else {
            self.focal_px(comp_h)
        }
    }

    /// The **circle-of-confusion blur radius** (comp px) for a 3-D layer at
    /// camera-space `depth`, under this camera's depth-of-field settings.
    ///
    /// Pure function of `(depth, focus, aperture)` — the testable heart of DoF.
    /// A real thin lens blurs a point by a circle whose diameter grows with the
    /// *relative* depth error `|depth − focus| / focus`; we model the blur radius
    /// as
    ///
    /// ```text
    /// r = aperture · |depth − focus| / focus
    /// ```
    ///
    /// so a layer **at** the focus distance is perfectly sharp (`r = 0`), the
    /// radius grows linearly with both how far it is from focus and the aperture,
    /// and it is scale-invariant (doubling the whole scene's depth and focus
    /// leaves the look unchanged). Returns `0.0` when DoF is off, the aperture is
    /// zero, or `focus`/`depth` is degenerate. The result is clamped to a sane
    /// `MAX_DOF_RADIUS` so a pathological aperture can't blow up the kernel.
    pub fn coc_blur_radius(&self, depth: f32, comp_h: f32) -> f32 {
        if !self.dof_enabled || self.aperture <= 0.0 {
            return 0.0;
        }
        let focus = self.effective_focus(comp_h);
        if !focus.is_finite() || focus <= 0.0 || !depth.is_finite() {
            return 0.0;
        }
        let r = self.aperture * (depth - focus).abs() / focus;
        r.clamp(0.0, Self::MAX_DOF_RADIUS)
    }

    /// Upper bound on the DoF blur radius (comp px) so an extreme aperture can't
    /// produce a runaway convolution kernel.
    pub const MAX_DOF_RADIUS: f32 = 256.0;

    /// Project a comp-space 3-D point `(x, y, z)` through this camera onto the
    /// comp plane, for a comp of pixel height `comp_h`.
    ///
    /// Transforms the point into camera space (translate by `-position`, rotate
    /// into the camera basis), then applies the pinhole perspective divide using
    /// the focal length [`focal_px`](Self::focal_px) (the camera-to-poi distance):
    /// a point at camera-space depth `zc` projects with `scale = f / zc` and
    /// `screen = (xc, yc) · scale`.
    ///
    /// With the [`Default`] camera (on the `-z` axis looking down `+z` at the
    /// origin) a point at `z = 0` has camera depth `zc = f`, so `scale = 1` and
    /// `screen = (x, y)` — the **identity on the comp plane** (today's 2-D look),
    /// for any comp size.
    pub fn project(&self, x: f32, y: f32, z: f32, comp_h: f32) -> Projected {
        let d = self.focal_px(comp_h);
        let (right, up, fwd) = self.basis();
        let rel = [x - self.position[0], y - self.position[1], z - self.position[2]];
        let xc = dot(rel, right);
        let yc = dot(rel, up);
        let zc = dot(rel, fwd);
        if zc <= 1e-4 {
            // At or behind the camera — no valid projection.
            return Projected {
                screen: (x, y),
                scale: 0.0,
                depth: zc,
            };
        }
        let scale = d / zc;
        Projected {
            screen: (xc * scale, yc * scale),
            scale,
            depth: zc,
        }
    }
}

/// `a - b` for 3-vectors.
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// Dot product of two 3-vectors.
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Cross product `a × b`.
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Normalize a 3-vector, or `None` if it's (near) zero length.
fn normalize(v: [f32; 3]) -> Option<[f32; 3]> {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len < 1e-9 {
        None
    } else {
        Some([v[0] / len, v[1] / len, v[2] / len])
    }
}

/// Rotate a layer-local point by the 3-D **orientation** `(rx, ry, rz)` in
/// degrees, returning the rotated `(x, y, z)`. Intrinsic Z→Y→X order (roll, then
/// pan, then tilt) about the layer's anchor — the After-Effects orientation
/// order. A layer's in-plane `rotation` (the 2-D Rotation property) is the Z
/// component and is handled by the existing affine path, so callers pass only
/// the X/Y/Z **orientation** here (with Z orientation, if any, folded in).
///
/// With all-zero orientation this is the identity (`z` stays `0` for a flat
/// layer), so a 3-D layer with no orientation is coplanar with the comp plane.
pub fn rotate_orientation(
    x: f32,
    y: f32,
    z: f32,
    rx_deg: f32,
    ry_deg: f32,
    rz_deg: f32,
) -> (f32, f32, f32) {
    let mut p = [x, y, z];
    // Z (roll): screen-convention clockwise for +angle, matching Affine2::rotate_deg.
    let (s, c) = rz_deg.to_radians().sin_cos();
    p = [c * p[0] - s * p[1], s * p[0] + c * p[1], p[2]];
    // Y (pan): rotate in the x–z plane.
    let (s, c) = ry_deg.to_radians().sin_cos();
    p = [c * p[0] + s * p[2], p[1], -s * p[0] + c * p[2]];
    // X (tilt): rotate in the y–z plane.
    let (s, c) = rx_deg.to_radians().sin_cos();
    p = [p[0], c * p[1] - s * p[2], s * p[1] + c * p[2]];
    (p[0], p[1], p[2])
}
