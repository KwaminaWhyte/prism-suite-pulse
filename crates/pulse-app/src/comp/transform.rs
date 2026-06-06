//! The sampled layer [`Transform`] and the 2-D [`Affine2`] matrix it builds.

/// A sampled layer transform at one instant.
#[derive(Clone, Copy, Debug)]
pub struct Transform {
    /// Anchor-point offset from the layer center (comp px), the pivot.
    pub anchor_x: f32,
    pub anchor_y: f32,
    pub x: f32,
    pub y: f32,
    pub scale: f32,
    pub rotation_deg: f32,
    pub opacity: f32,
}

impl Transform {
    /// The layer's **local** affine matrix (comp space, origin at comp center),
    /// mapping layer-local points into the layer's own comp-space frame —
    /// *before* any parent transform.
    ///
    /// Built as `Translate(position) · Rotate · Scale · Translate(-anchor)`:
    /// the anchor point maps to `position`, and scale/rotation pivot about the
    /// anchor — the standard After-Effects transform order.
    pub fn local_matrix(self) -> Affine2 {
        let s = self.scale.max(0.0);
        Affine2::translate(self.x, self.y)
            .then(Affine2::rotate_deg(self.rotation_deg))
            .then(Affine2::scale(s))
            .then(Affine2::translate(-self.anchor_x, -self.anchor_y))
    }
}

/// A 2-D affine transform `[[a, c, tx], [b, d, ty]]` mapping a point
/// `(x, y)` to `(a·x + c·y + tx, b·x + d·y + ty)`. Comp space; origin at the
/// comp center with `+y` downward (screen convention), matching the preview.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine2 {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub tx: f32,
    pub ty: f32,
}

impl Affine2 {
    /// The identity transform.
    pub const IDENTITY: Affine2 = Affine2 {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        tx: 0.0,
        ty: 0.0,
    };

    /// A pure translation.
    pub fn translate(tx: f32, ty: f32) -> Self {
        Affine2 {
            tx,
            ty,
            ..Affine2::IDENTITY
        }
    }

    /// A uniform scale about the origin.
    pub fn scale(s: f32) -> Self {
        Affine2 {
            a: s,
            d: s,
            ..Affine2::IDENTITY
        }
    }

    /// A rotation (degrees) about the origin. `+y` is downward, so a positive
    /// angle rotates clockwise on screen, matching the preview.
    pub fn rotate_deg(deg: f32) -> Self {
        let (sin, cos) = deg.to_radians().sin_cos();
        Affine2 {
            a: cos,
            b: sin,
            c: -sin,
            d: cos,
            tx: 0.0,
            ty: 0.0,
        }
    }

    /// Compose: `self.then(rhs)` applies `rhs` first, then `self` — i.e. the
    /// matrix product `self * rhs`. Reads left-to-right as outermost-first.
    #[must_use]
    pub fn then(self, rhs: Affine2) -> Self {
        Affine2 {
            a: self.a * rhs.a + self.c * rhs.b,
            b: self.b * rhs.a + self.d * rhs.b,
            c: self.a * rhs.c + self.c * rhs.d,
            d: self.b * rhs.c + self.d * rhs.d,
            tx: self.a * rhs.tx + self.c * rhs.ty + self.tx,
            ty: self.b * rhs.tx + self.d * rhs.ty + self.ty,
        }
    }

    /// Apply the transform to a point.
    pub fn apply(self, x: f32, y: f32) -> (f32, f32) {
        (
            self.a * x + self.c * y + self.tx,
            self.b * x + self.d * y + self.ty,
        )
    }

    /// The inverse transform, or `None` if the matrix is singular (e.g. a
    /// zero-scale collapse). Used by the rasterizer to map a comp-space pixel
    /// back into the layer's local frame for coverage testing.
    pub fn inverse(self) -> Option<Affine2> {
        let det = self.a * self.d - self.b * self.c;
        if det.abs() < 1e-12 {
            return None;
        }
        let inv = 1.0 / det;
        let a = self.d * inv;
        let b = -self.b * inv;
        let c = -self.c * inv;
        let d = self.a * inv;
        Some(Affine2 {
            a,
            b,
            c,
            d,
            tx: -(a * self.tx + c * self.ty),
            ty: -(b * self.tx + d * self.ty),
        })
    }
}
