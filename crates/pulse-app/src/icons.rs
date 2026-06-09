//! Icon font for Pulse's transport, timeline, and panel UI.
//!
//! Uses [`egui-phosphor`] (Phosphor icons, MIT) which ships a TTF and glyph
//! constants compatible with egui 0.34. We register the font into the
//! Proportional and Monospace families so glyphs render inline with text, then
//! re-export the codepoints under action-oriented names.

use egui_phosphor::regular as ph;

/// Merge the Phosphor icon font into the context's font definitions.
///
/// Call once at startup with `cc.egui_ctx`.
pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    fonts.font_data.insert(
        "phosphor".to_owned(),
        std::sync::Arc::new(egui_phosphor::Variant::Regular.font_data()),
    );

    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("phosphor".to_owned());
    }

    ctx.set_fonts(fonts);
}

// --- Transport --------------------------------------------------------------

/// Play.
pub const PLAY: &str = ph::PLAY;
/// Pause.
pub const PAUSE: &str = ph::PAUSE;
/// Jump to start.
pub const TO_START: &str = ph::SKIP_BACK;
/// Jump to end.
pub const TO_END: &str = ph::SKIP_FORWARD;
/// RAM-preview cache fill (transport "caching…" readout).
pub const RAM_CACHE: &str = ph::DATABASE;

// --- Layers / panels --------------------------------------------------------

/// Add a new layer.
pub const ADD_LAYER: &str = ph::STACK_PLUS;
/// Delete / trash.
pub const TRASH: &str = ph::TRASH;
/// Move item up.
pub const ARROW_UP: &str = ph::ARROW_UP;
/// Move item down.
pub const ARROW_DOWN: &str = ph::ARROW_DOWN;
/// Visibility on (eye).
pub const EYE: &str = ph::EYE;
/// Visibility off (eye slash).
pub const EYE_OFF: &str = ph::EYE_SLASH;

// --- Timeline / properties --------------------------------------------------

/// Keyframe diamond.
pub const KEYFRAME: &str = ph::DIAMOND;
/// Add a keyframe.
pub const ADD_KEY: &str = ph::PLUS;
/// Composition / film badge in the title.
#[allow(dead_code)]
pub const COMP: &str = ph::FILM_STRIP;
/// Export / render to disk.
pub const EXPORT: &str = ph::EXPORT;
/// A layer consumed as a track-matte source (used to define another's alpha).
pub const MATTE: &str = ph::MASK_HAPPY;
/// A layer with a non-Normal blend mode (two overlapping shapes).
pub const BLEND: &str = ph::INTERSECT;
/// A dockable panel / window (Window-menu toggles).
pub const PANEL: &str = ph::SIDEBAR;
/// Search / filter (the Effects & Presets browser search box).
pub const SEARCH: &str = ph::MAGNIFYING_GLASS;
/// Clear the current search query.
pub const CLEAR: &str = ph::X_CIRCLE;
/// An effect (the Effects & Presets browser badge).
pub const EFFECT: &str = ph::SPARKLE;
/// Add a marker at the playhead.
pub const MARKER: &str = ph::MAP_PIN_PLUS;
/// Jump to the previous marker.
pub const MARKER_PREV: &str = ph::CARET_DOUBLE_LEFT;
/// Jump to the next marker.
pub const MARKER_NEXT: &str = ph::CARET_DOUBLE_RIGHT;

// --- Keyframe interpolation -------------------------------------------------

/// Linear interpolation (straight ramp).
pub const INTERP_LINEAR: &str = ph::LINE_SEGMENT;
/// Hold / stepped interpolation.
pub const INTERP_HOLD: &str = ph::STAIRS;
/// Bézier ease (smooth in/out).
pub const INTERP_EASE: &str = ph::CHART_LINE;

// --- Editor mode toggle -----------------------------------------------------

/// Timeline (lane) editor mode.
pub const TIMELINE: &str = ph::ROWS;
/// Graph (value-curve) editor mode.
pub const GRAPH: &str = ph::CHART_LINE_UP;
