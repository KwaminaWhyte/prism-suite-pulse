//! Comp **lights** and the pure **Lambert shading** that lights 3-D layers.
//!
//! Pulse's compositor is a flat 2-D rasterizer; [`camera`](super::camera) lifts
//! layers into 3-D and projects them. This module adds the other half of a 3-D
//! look — **lighting**. A comp carries a list of [`Light`]s (a `serde`-defaulted
//! empty list, so a pre-lighting `.pulse` file loads with none). A 3-D layer that
//! opts in (`accepts_lights`) has its pixels **modulated by an RGB illumination
//! factor**: an **ambient** floor plus, for each **point** light, a Lambert
//! diffuse term `color × intensity × max(0, N·L)`.
//!
//! ## Coordinate frame
//!
//! Light positions live in the same comp 3-D space as the camera and layers
//! (origin at comp center, `+x` right, `+y` **down**, `+z` **into** the screen —
//! see [`camera`](super::camera)). A flat un-oriented 3-D layer at `z = 0` faces
//! the camera (which sits on `-z`), so its surface **normal** is `[0, 0, -1]`
//! (toward the viewer); the layer's X/Y/Z **orientation** rotates that normal the
//! same way it rotates the layer's plane.
//!
//! ## Back-compat
//!
//! The whole feature is **opt-in twice**: a comp with no lights, or a layer with
//! `accepts_lights = false` (the default), is never modulated — the illumination
//! factor is exactly `[1, 1, 1]`, so the layer renders byte-identically to today.
//! Lighting only ever multiplies a layer's *own* pixels in its isolated buffer;
//! it never touches 2-D layers, the accumulator, or anything that doesn't opt in.
//!
//! ## Out of scope (follow-ups)
//!
//! **Shadows** (shadow catcher), **spot** cones, **parallel** (directional) and
//! light **falloff**, plus **specular** material response are noted in `PLAN.md`
//! and not implemented here — only Ambient + Point Lambert diffuse.

use serde::{Deserialize, Serialize};

/// The kind of a comp [`Light`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LightKind {
    /// A uniform, position-independent fill that lights every surface equally —
    /// the diffuse **floor**. (No `N·L` term; ignores [`Light::position`].)
    Ambient,
    /// An omnidirectional point source at [`Light::position`]: shades a surface by
    /// Lambert diffuse `max(0, N·L)` where `L` points from the surface toward the
    /// light. (No distance falloff — a documented simplification; spot/parallel
    /// and falloff are follow-ups.)
    Point,
}

impl LightKind {
    /// A short human label for the UI / pickers.
    pub fn label(self) -> &'static str {
        match self {
            LightKind::Ambient => "Ambient",
            LightKind::Point => "Point",
        }
    }
}

/// One comp light. Lights shade only [`accepts_lights`](super::PulseLayer)-opted
/// **3-D layers**; an empty light list (the comp default) leaves every layer
/// unlit (illumination factor `[1, 1, 1]`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Light {
    /// Ambient vs. point.
    pub kind: LightKind,
    /// Comp-space position (only meaningful for [`LightKind::Point`]).
    #[serde(default)]
    pub position: [f32; 3],
    /// Light color (linear-ish straight RGB, 0..=1) — multiplied into the
    /// diffuse term per channel so a colored light tints the surface.
    #[serde(default = "Light::default_color")]
    pub color: [f32; 3],
    /// Scalar brightness multiplier (1.0 = full). Scales the whole contribution.
    #[serde(default = "Light::default_intensity")]
    pub intensity: f32,
}

impl Light {
    fn default_color() -> [f32; 3] {
        [1.0, 1.0, 1.0]
    }

    fn default_intensity() -> f32 {
        1.0
    }

    /// A new ambient light (a flat diffuse floor) of the given color/intensity.
    pub fn ambient(color: [f32; 3], intensity: f32) -> Self {
        Light {
            kind: LightKind::Ambient,
            position: [0.0, 0.0, 0.0],
            color,
            intensity,
        }
    }

    /// A new point light at `position` with the given color/intensity.
    pub fn point(position: [f32; 3], color: [f32; 3], intensity: f32) -> Self {
        Light {
            kind: LightKind::Point,
            position,
            color,
            intensity,
        }
    }
}

impl Default for Light {
    fn default() -> Self {
        // A neutral point light — what the "add point light" UI seeds before the
        // user places it (the renderer never uses a defaulted light directly).
        Light::point([0.0, 0.0, -500.0], [1.0, 1.0, 1.0], 1.0)
    }
}

/// The **surface normal** of a 3-D layer with X/Y/Z **orientation**
/// `(rx, ry, rz)` degrees, in comp space. A flat un-oriented layer faces the
/// camera (which looks down `+z` from `-z`), so its base normal is `[0, 0, -1]`
/// (toward the viewer); the orientation rotates it exactly as it rotates the
/// layer's plane (the same [`rotate_orientation`](super::rotate_orientation)
/// the layer geometry uses). Always returns a unit vector.
pub fn layer_normal(rx_deg: f32, ry_deg: f32, rz_deg: f32) -> [f32; 3] {
    let (nx, ny, nz) = super::rotate_orientation(0.0, 0.0, -1.0, rx_deg, ry_deg, rz_deg);
    let len = (nx * nx + ny * ny + nz * nz).sqrt().max(1e-9);
    [nx / len, ny / len, nz / len]
}

/// Total **illumination factor** (a per-channel RGB multiplier) at a surface
/// point `surface` with unit normal `normal`, from a set of `lights`.
///
/// `factor = Σ_ambient(color·intensity) + Σ_point(color·intensity·max(0, N·L))`
/// where `L = normalize(light.position − surface)`. Each channel is clamped to be
/// non-negative (so a zero-intensity / fully-back-facing surface stays at the
/// ambient floor, never negative).
///
/// **Crucially, an empty light list returns `[1, 1, 1]`** (the identity
/// multiplier) — so a comp with no lights, or a layer the caller never calls this
/// for, renders unchanged. This is the back-compat contract.
pub fn illumination(lights: &[Light], surface: [f32; 3], normal: [f32; 3]) -> [f32; 3] {
    if lights.is_empty() {
        return [1.0, 1.0, 1.0];
    }
    let mut acc = [0.0f32; 3];
    for light in lights {
        let scale = match light.kind {
            LightKind::Ambient => light.intensity,
            LightKind::Point => {
                let l = [
                    light.position[0] - surface[0],
                    light.position[1] - surface[1],
                    light.position[2] - surface[2],
                ];
                let len = (l[0] * l[0] + l[1] * l[1] + l[2] * l[2]).sqrt();
                if len < 1e-9 {
                    // Light sitting on the surface — treat as fully lit.
                    light.intensity
                } else {
                    let ndotl = (normal[0] * l[0] + normal[1] * l[1] + normal[2] * l[2]) / len;
                    light.intensity * ndotl.max(0.0)
                }
            }
        };
        acc[0] += light.color[0] * scale;
        acc[1] += light.color[1] * scale;
        acc[2] += light.color[2] * scale;
    }
    [acc[0].max(0.0), acc[1].max(0.0), acc[2].max(0.0)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn no_lights_is_identity() {
        let f = illumination(&[], [0.0, 0.0, 0.0], [0.0, 0.0, -1.0]);
        assert_eq!(f, [1.0, 1.0, 1.0]);
    }

    #[test]
    fn unoriented_layer_faces_the_camera() {
        // Flat layer → normal toward the viewer (−z).
        let n = layer_normal(0.0, 0.0, 0.0);
        assert!(approx(n[0], 0.0) && approx(n[1], 0.0) && approx(n[2], -1.0));
    }

    #[test]
    fn facing_light_is_brighter_than_back_facing() {
        // A point light in front of the comp (toward the camera, −z).
        let light = Light::point([0.0, 0.0, -500.0], [1.0, 1.0, 1.0], 1.0);
        let facing = layer_normal(0.0, 0.0, 0.0); // toward −z, toward the light
        let away = layer_normal(180.0, 0.0, 0.0); // flipped → +z, away
        let lit = illumination(&[light], [0.0, 0.0, 0.0], facing)[0];
        let dark = illumination(&[light], [0.0, 0.0, 0.0], away)[0];
        assert!(lit > dark, "facing {lit} should beat back-facing {dark}");
        assert!(approx(lit, 1.0), "head-on N·L = 1 → full: {lit}");
        assert!(approx(dark, 0.0), "back-facing clamps to 0: {dark}");
    }

    #[test]
    fn ambient_is_a_flat_floor_regardless_of_normal() {
        let amb = Light::ambient([1.0, 1.0, 1.0], 0.3);
        let a = illumination(&[amb], [0.0, 0.0, 0.0], layer_normal(0.0, 0.0, 0.0));
        let b = illumination(&[amb], [10.0, 5.0, 7.0], layer_normal(90.0, 45.0, 0.0));
        assert!(approx(a[0], 0.3) && approx(b[0], 0.3));
        assert_eq!(a, b);
    }

    #[test]
    fn intensity_and_color_scale_the_contribution() {
        let n = layer_normal(0.0, 0.0, 0.0);
        let p = [0.0, 0.0, 0.0];
        // Intensity 2 doubles a head-on contribution.
        let f = illumination(&[Light::point([0.0, 0.0, -500.0], [1.0, 1.0, 1.0], 2.0)], p, n);
        assert!(approx(f[0], 2.0));
        // A red light only tints red.
        let r = illumination(&[Light::point([0.0, 0.0, -500.0], [1.0, 0.0, 0.0], 1.0)], p, n);
        assert!(approx(r[0], 1.0) && approx(r[1], 0.0) && approx(r[2], 0.0));
    }

    #[test]
    fn ambient_plus_point_adds() {
        let n = layer_normal(0.0, 0.0, 0.0);
        let lights = [
            Light::ambient([1.0, 1.0, 1.0], 0.2),
            Light::point([0.0, 0.0, -500.0], [1.0, 1.0, 1.0], 1.0),
        ];
        let f = illumination(&lights, [0.0, 0.0, 0.0], n);
        assert!(approx(f[0], 1.2), "ambient 0.2 + head-on point 1.0: {}", f[0]);
    }

    #[test]
    fn serde_round_trip() {
        let lights = vec![
            Light::ambient([0.1, 0.2, 0.3], 0.4),
            Light::point([1.0, 2.0, 3.0], [0.5, 0.6, 0.7], 1.5),
        ];
        let json = serde_json::to_string(&lights).unwrap();
        let back: Vec<Light> = serde_json::from_str(&json).unwrap();
        assert_eq!(lights, back);
    }

    #[test]
    fn serde_legacy_point_defaults_color_and_intensity() {
        // A hand-written / legacy point light with only a kind+position fills the
        // serde-defaulted color (white) + intensity (1.0).
        let l: Light = serde_json::from_str(r#"{"kind":"Point","position":[1.0,2.0,3.0]}"#).unwrap();
        assert_eq!(l.color, [1.0, 1.0, 1.0]);
        assert_eq!(l.intensity, 1.0);
        assert_eq!(l.position, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn determinism() {
        let lights = [
            Light::ambient([0.3, 0.3, 0.3], 0.5),
            Light::point([100.0, -50.0, -300.0], [0.9, 0.8, 1.0], 1.2),
        ];
        let n = layer_normal(20.0, -35.0, 10.0);
        let p = [12.0, -7.0, 40.0];
        let a = illumination(&lights, p, n);
        let b = illumination(&lights, p, n);
        assert_eq!(a, b);
    }
}
