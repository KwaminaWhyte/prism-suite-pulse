//! Workspace panel visibility — the Window-menu show/hide model.
//!
//! Pulse's shell is a fixed four-panel layout (Layers · Properties · Timeline ·
//! Preview). A pro motion app lets the user hide panels they don't need (After
//! Effects' *Window* menu, Affinity's *View ▸ Studio*), reclaiming screen for the
//! ones they do. [`PanelVisibility`] is the pure state behind that menu: which of
//! the dockable panels are currently shown.
//!
//! The **Preview** (central) panel has no toggle — it is the comp viewport and is
//! always present (egui's `CentralPanel` fills whatever the side/bottom panels
//! leave, so there is always *something* there; hiding it would be meaningless).
//! Only the surrounding dock panels — Layers, Properties, Timeline — can be
//! hidden.

/// A dockable panel that the Window menu can show or hide. The central Preview
/// viewport is deliberately *not* a member — it is always visible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Panel {
    Layers,
    Properties,
    Timeline,
}

impl Panel {
    /// Every toggleable panel, in Window-menu order.
    pub const ALL: [Panel; 3] = [Panel::Layers, Panel::Properties, Panel::Timeline];

    /// The human-readable label shown in the Window menu.
    pub fn label(self) -> &'static str {
        match self {
            Panel::Layers => "Layers",
            Panel::Properties => "Properties",
            Panel::Timeline => "Timeline",
        }
    }
}

/// Which dockable panels are currently shown. Defaults to all visible (the
/// classic four-panel workspace). Pure state — the app reads each flag to decide
/// whether to render the matching `SidePanel` / `TopBottomPanel` this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PanelVisibility {
    layers: bool,
    properties: bool,
    timeline: bool,
}

impl Default for PanelVisibility {
    fn default() -> Self {
        Self {
            layers: true,
            properties: true,
            timeline: true,
        }
    }
}

impl PanelVisibility {
    /// Whether the given panel is currently shown.
    pub fn is_shown(self, panel: Panel) -> bool {
        match panel {
            Panel::Layers => self.layers,
            Panel::Properties => self.properties,
            Panel::Timeline => self.timeline,
        }
    }

    /// Set a panel's visibility directly.
    pub fn set(&mut self, panel: Panel, shown: bool) {
        match panel {
            Panel::Layers => self.layers = shown,
            Panel::Properties => self.properties = shown,
            Panel::Timeline => self.timeline = shown,
        }
    }

    /// Flip a panel's visibility (the Window-menu checkbox action).
    pub fn toggle(&mut self, panel: Panel) {
        self.set(panel, !self.is_shown(panel));
    }

    /// How many dockable panels are currently shown.
    pub fn shown_count(self) -> usize {
        Panel::ALL.iter().filter(|&&p| self.is_shown(p)).count()
    }

    /// Whether *every* dockable panel is hidden — the workspace is "preview only"
    /// (just the central viewport). Useful for a "Show all" affordance and the
    /// menu's enabled-state hints.
    pub fn all_hidden(self) -> bool {
        self.shown_count() == 0
    }

    /// Show every dockable panel (Window ▸ *Reset / Show all panels*).
    pub fn show_all(&mut self) {
        *self = Self::default();
    }

    /// Hide every dockable panel, leaving only the central Preview viewport
    /// (a quick "maximize the canvas" action).
    pub fn hide_all(&mut self) {
        self.layers = false;
        self.properties = false;
        self.timeline = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_shows_every_panel() {
        let v = PanelVisibility::default();
        for p in Panel::ALL {
            assert!(v.is_shown(p), "{} should default to shown", p.label());
        }
        assert_eq!(v.shown_count(), 3);
        assert!(!v.all_hidden());
    }

    #[test]
    fn toggle_flips_one_panel_only() {
        let mut v = PanelVisibility::default();
        v.toggle(Panel::Properties);
        assert!(!v.is_shown(Panel::Properties));
        // The others are untouched.
        assert!(v.is_shown(Panel::Layers));
        assert!(v.is_shown(Panel::Timeline));
        assert_eq!(v.shown_count(), 2);
        // Toggling back restores it.
        v.toggle(Panel::Properties);
        assert!(v.is_shown(Panel::Properties));
        assert_eq!(v.shown_count(), 3);
    }

    #[test]
    fn set_is_idempotent() {
        let mut v = PanelVisibility::default();
        v.set(Panel::Layers, false);
        v.set(Panel::Layers, false);
        assert!(!v.is_shown(Panel::Layers));
        assert_eq!(v.shown_count(), 2);
    }

    #[test]
    fn hide_all_then_all_hidden() {
        let mut v = PanelVisibility::default();
        v.hide_all();
        assert!(v.all_hidden());
        assert_eq!(v.shown_count(), 0);
        for p in Panel::ALL {
            assert!(!v.is_shown(p));
        }
    }

    #[test]
    fn show_all_restores_default() {
        let mut v = PanelVisibility::default();
        v.toggle(Panel::Layers);
        v.toggle(Panel::Timeline);
        assert_eq!(v.shown_count(), 1);
        v.show_all();
        assert_eq!(v, PanelVisibility::default());
        assert_eq!(v.shown_count(), 3);
    }

    #[test]
    fn all_panels_listed_with_distinct_labels() {
        // Every panel has a unique, non-empty label and ALL has no duplicates.
        let mut seen = Vec::new();
        for p in Panel::ALL {
            assert!(!p.label().is_empty());
            assert!(!seen.contains(&p), "duplicate panel in ALL");
            seen.push(p);
        }
        assert_eq!(seen.len(), 3);
    }
}
