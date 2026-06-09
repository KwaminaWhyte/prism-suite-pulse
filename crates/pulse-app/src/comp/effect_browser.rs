//! The searchable **effect registry** behind the Effects & Presets browser.
//!
//! Today's "Add effect" UI is two flat menus — one per stack (the per-pixel
//! [`Effect`](super::Effect) colour-correction passes, and the whole-buffer
//! [`SpatialEffect`](super::SpatialEffect) blur/shadow/glow passes). As the
//! effect surface grows toward After Effects parity, a flat list stops scaling:
//! AE's *Effects & Presets* panel is **type-to-filter, categorised**, with a
//! drag-onto-layer affordance. This module is the pure, app-agnostic core of
//! that panel — a single registry of every addable effect across both stacks,
//! each tagged with a **category** and **search keywords**, plus a [`filter`]
//! that ranks the registry against a query string.
//!
//! The UI ([`crate::app`]) renders the filtered, grouped result and, on a click,
//! turns the chosen [`BrowserEntry`] into a concrete effect via
//! [`BrowserEntry::instantiate`] — pushing it onto the right stack of the
//! selected layer. Keeping the registry and the matcher here (not in the UI)
//! means the search/ranking logic is unit-testable without an egui context.

use super::{DistortEffect, Effect, GenerateEffect, KeyEffect, SpatialEffect, StylizeEffect};

/// Which per-layer stack an effect belongs to. The browser adds a
/// [`Category::Color`] / generic per-pixel effect to the layer's
/// [`effects`](super::PulseLayer::effects) vec, a spatial effect to its
/// [`spatial_effects`](super::PulseLayer::spatial_effects) vec, and a generate
/// fill to its [`generate`](super::PulseLayer::generate) slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stack {
    /// A per-pixel colour-correction [`Effect`].
    Color,
    /// A whole-buffer [`SpatialEffect`] (blur / shadow / glow).
    Spatial,
    /// A whole-buffer [`GenerateEffect`] fill (Fractal Noise) — replaces the
    /// layer's content rather than reading it.
    Generate,
    /// A whole-buffer [`DistortEffect`] coordinate-remap (Corner Pin / Transform
    /// / Mirror / Polar) — warps the layer's pixels rather than recoloring them.
    Distort,
    /// A whole-buffer [`StylizeEffect`] look-shaping pass (Find Edges / Mosaic) —
    /// reshapes the layer's look rather than recoloring or warping it.
    Stylize,
    /// A whole-buffer [`KeyEffect`] matte-pull (Color / Luma / Chroma Key, Spill
    /// Suppression, Matte Choke) — carves the layer's alpha (and, for spill,
    /// neutralises RGB) rather than recoloring or warping it.
    Keying,
}

/// The browser's top-level grouping (AE's *Effects & Presets* category folders).
/// Distinct from [`Stack`] because several categories can map to the same stack
/// (e.g. *Blur & Sharpen* and *Perspective* and *Stylize* are all the spatial
/// stack today) and one day a category may straddle stacks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    /// Colour-correction passes (Tint, Levels, Curves, …).
    Color,
    /// Blur & sharpen passes (Gaussian Blur, …).
    Blur,
    /// Perspective passes (Drop Shadow, …).
    Perspective,
    /// Stylize passes (Glow, …).
    Stylize,
    /// Distort passes (Corner Pin, Transform, Mirror, Polar Coordinates, …) —
    /// geometric coordinate-remaps.
    Distort,
    /// Generate passes (Fractal Noise, …) — synthesise content into the layer.
    Generate,
    /// Keying passes (Color / Luma / Chroma Key, Spill Suppression, Matte
    /// Choke, …) — pull a matte from the layer's colour.
    Keying,
}

impl Category {
    /// Every category, in the browser's display order.
    pub const ALL: [Category; 7] = [
        Category::Color,
        Category::Blur,
        Category::Perspective,
        Category::Stylize,
        Category::Distort,
        Category::Generate,
        Category::Keying,
    ];

    /// The folder label shown in the browser.
    pub fn label(self) -> &'static str {
        match self {
            Category::Color => "Color Correction",
            Category::Blur => "Blur & Sharpen",
            Category::Perspective => "Perspective",
            Category::Stylize => "Stylize",
            Category::Distort => "Distort",
            Category::Generate => "Generate",
            Category::Keying => "Keying",
        }
    }
}

/// One entry in the effect registry: a human name, the [`Category`] folder it
/// lives in, the [`Stack`] it adds to, a stable index *within* that stack's
/// `defaults()` array (so [`instantiate`](Self::instantiate) can build a fresh,
/// sensibly-defaulted instance), and a set of lowercase search **keywords**
/// (synonyms / AE names the user might type that aren't in the display name).
#[derive(Clone, Copy, Debug)]
pub struct BrowserEntry {
    /// Display name, also matched against the query.
    pub name: &'static str,
    /// The category folder this entry belongs to.
    pub category: Category,
    /// Which per-layer stack adding this entry appends to.
    pub stack: Stack,
    /// Index into the stack's `defaults()` array for [`instantiate`](Self::instantiate).
    default_index: usize,
    /// Extra lowercase search terms (synonyms / abbreviations) beyond the name.
    pub keywords: &'static [&'static str],
}

impl BrowserEntry {
    /// Build a fresh, value-neutral (or sensibly-defaulted) effect for this
    /// entry, ready to push onto a layer. Returns a tagged union so the caller
    /// pushes onto the correct stack.
    pub fn instantiate(&self) -> NewEffect {
        match self.stack {
            Stack::Color => NewEffect::Color(Effect::defaults()[self.default_index]),
            Stack::Spatial => NewEffect::Spatial(SpatialEffect::defaults()[self.default_index]),
            Stack::Generate => NewEffect::Generate(GenerateEffect::defaults()[self.default_index]),
            Stack::Distort => NewEffect::Distort(DistortEffect::defaults()[self.default_index]),
            Stack::Stylize => NewEffect::Stylize(StylizeEffect::defaults()[self.default_index]),
            Stack::Keying => NewEffect::Keying(KeyEffect::defaults()[self.default_index]),
        }
    }
}

/// A freshly-instantiated effect, tagged by the stack it belongs on, returned by
/// [`BrowserEntry::instantiate`]. The UI matches on this to push onto the right
/// vec of the selected layer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NewEffect {
    Color(Effect),
    Spatial(SpatialEffect),
    Generate(GenerateEffect),
    Distort(DistortEffect),
    Stylize(StylizeEffect),
    Keying(KeyEffect),
}

/// The full effect registry — every addable effect across both stacks. The
/// `default_index` of each entry is the slot in its stack's `defaults()` array,
/// so the registry and `Effect::defaults()` / `SpatialEffect::defaults()` must
/// stay in sync (the `registry_indices_match_defaults` test guards this).
pub const REGISTRY: &[BrowserEntry] = &[
    // --- Color correction (Effect::defaults() order) ------------------------
    BrowserEntry {
        name: "Tint",
        category: Category::Color,
        stack: Stack::Color,
        default_index: 0,
        keywords: &["color", "duotone", "map", "black", "white"],
    },
    BrowserEntry {
        name: "Brightness & Contrast",
        category: Category::Color,
        stack: Stack::Color,
        default_index: 1,
        keywords: &["bright", "contrast", "exposure"],
    },
    BrowserEntry {
        name: "Exposure",
        category: Category::Color,
        stack: Stack::Color,
        default_index: 2,
        keywords: &["stops", "gamma", "brightness"],
    },
    BrowserEntry {
        name: "Levels",
        category: Category::Color,
        stack: Stack::Color,
        default_index: 3,
        keywords: &[
            "gamma",
            "contrast",
            "histogram",
            "black point",
            "white point",
        ],
    },
    BrowserEntry {
        name: "Hue / Saturation",
        category: Category::Color,
        stack: Stack::Color,
        default_index: 4,
        keywords: &["hsl", "saturate", "desaturate", "color", "vibrance"],
    },
    BrowserEntry {
        name: "Curves",
        category: Category::Color,
        stack: Stack::Color,
        default_index: 5,
        keywords: &["tone", "contrast", "s-curve", "spline"],
    },
    BrowserEntry {
        name: "Color Balance",
        category: Category::Color,
        stack: Stack::Color,
        default_index: 6,
        keywords: &["shadows", "midtones", "highlights", "grade", "tint"],
    },
    BrowserEntry {
        name: "Channel Mixer",
        category: Category::Color,
        stack: Stack::Color,
        default_index: 7,
        keywords: &[
            "channel",
            "mixer",
            "swap",
            "rgb",
            "monochrome",
            "grayscale",
            "mix",
        ],
    },
    BrowserEntry {
        name: "Gradient Map",
        category: Category::Color,
        stack: Stack::Color,
        default_index: 8,
        keywords: &[
            "gradient",
            "map",
            "luma",
            "ramp",
            "duotone",
            "grade",
            "color",
        ],
    },
    BrowserEntry {
        name: "Tritone",
        category: Category::Color,
        stack: Stack::Color,
        default_index: 9,
        keywords: &[
            "tritone",
            "tint",
            "duotone",
            "three tone",
            "shadows",
            "midtones",
            "highlights",
            "grade",
        ],
    },
    // --- Spatial (SpatialEffect::defaults() order) --------------------------
    BrowserEntry {
        name: "Gaussian Blur",
        category: Category::Blur,
        stack: Stack::Spatial,
        default_index: 0,
        keywords: &["blur", "soften", "defocus", "smooth"],
    },
    BrowserEntry {
        name: "Box Blur",
        category: Category::Blur,
        stack: Stack::Spatial,
        default_index: 1,
        keywords: &["box", "blur", "average", "soften", "fast", "iterations"],
    },
    BrowserEntry {
        name: "Directional Blur",
        category: Category::Blur,
        stack: Stack::Spatial,
        default_index: 2,
        keywords: &["directional", "motion", "blur", "streak", "smear", "angle"],
    },
    BrowserEntry {
        name: "Radial Blur",
        category: Category::Blur,
        stack: Stack::Spatial,
        default_index: 3,
        keywords: &["radial", "blur", "spin", "zoom", "rotate", "circular", "twirl"],
    },
    BrowserEntry {
        name: "Drop Shadow",
        category: Category::Perspective,
        stack: Stack::Spatial,
        default_index: 4,
        keywords: &["shadow", "cast", "depth"],
    },
    BrowserEntry {
        name: "Glow",
        category: Category::Stylize,
        stack: Stack::Spatial,
        default_index: 5,
        keywords: &["bloom", "bright", "halo", "light"],
    },
    // --- Generate (GenerateEffect::defaults() order) ------------------------
    BrowserEntry {
        name: "Fractal Noise",
        category: Category::Generate,
        stack: Stack::Generate,
        default_index: 0,
        keywords: &[
            "fractal",
            "noise",
            "turbulent",
            "perlin",
            "clouds",
            "smoke",
            "fbm",
            "evolution",
        ],
    },
    BrowserEntry {
        name: "Gradient Ramp",
        category: Category::Generate,
        stack: Stack::Generate,
        default_index: 1,
        keywords: &[
            "ramp",
            "gradient",
            "linear",
            "radial",
            "fade",
            "blend",
            "color",
        ],
    },
    BrowserEntry {
        name: "Checkerboard",
        category: Category::Generate,
        stack: Stack::Generate,
        default_index: 2,
        keywords: &["checker", "checkerboard", "chequer", "grid", "tile", "squares"],
    },
    BrowserEntry {
        name: "4-Color Gradient",
        category: Category::Generate,
        stack: Stack::Generate,
        default_index: 3,
        keywords: &[
            "4 color",
            "four color",
            "gradient",
            "corner",
            "blend",
            "mesh",
        ],
    },
    BrowserEntry {
        name: "Grid",
        category: Category::Generate,
        stack: Stack::Generate,
        default_index: 4,
        keywords: &["grid", "lines", "graph", "guides", "mesh", "checker"],
    },
    // --- Distort (DistortEffect::defaults() order) --------------------------
    BrowserEntry {
        name: "Corner Pin",
        category: Category::Distort,
        stack: Stack::Distort,
        default_index: 0,
        keywords: &[
            "corner",
            "pin",
            "perspective",
            "warp",
            "quad",
            "screen",
            "track",
        ],
    },
    BrowserEntry {
        name: "Transform",
        category: Category::Distort,
        stack: Stack::Distort,
        default_index: 1,
        keywords: &[
            "transform",
            "position",
            "scale",
            "rotate",
            "anchor",
            "skew",
            "move",
        ],
    },
    BrowserEntry {
        name: "Mirror",
        category: Category::Distort,
        stack: Stack::Distort,
        default_index: 2,
        keywords: &["mirror", "reflect", "flip", "symmetry", "kaleidoscope"],
    },
    BrowserEntry {
        name: "Polar Coordinates",
        category: Category::Distort,
        stack: Stack::Distort,
        default_index: 3,
        keywords: &[
            "polar",
            "coordinates",
            "radial",
            "tiny planet",
            "rect",
            "unwrap",
            "twirl",
        ],
    },
    // --- Stylize (StylizeEffect::defaults() order) --------------------------
    BrowserEntry {
        name: "Find Edges",
        category: Category::Stylize,
        stack: Stack::Stylize,
        default_index: 0,
        keywords: &[
            "find",
            "edges",
            "edge",
            "sobel",
            "outline",
            "ink",
            "contour",
            "detect",
        ],
    },
    BrowserEntry {
        name: "Mosaic",
        category: Category::Stylize,
        stack: Stack::Stylize,
        default_index: 1,
        keywords: &[
            "mosaic",
            "pixelate",
            "pixelsize",
            "pixelize",
            "blocks",
            "censor",
            "tile",
        ],
    },
    // --- Keying (KeyEffect::defaults() order) -------------------------------
    BrowserEntry {
        name: "Color Key",
        category: Category::Keying,
        stack: Stack::Keying,
        default_index: 0,
        keywords: &[
            "key",
            "color",
            "chroma",
            "green screen",
            "blue screen",
            "matte",
            "transparency",
            "tolerance",
        ],
    },
    BrowserEntry {
        name: "Luma Key",
        category: Category::Keying,
        stack: Stack::Keying,
        default_index: 1,
        keywords: &[
            "key",
            "luma",
            "luminance",
            "brightness",
            "threshold",
            "matte",
            "shadow",
            "highlight",
        ],
    },
    BrowserEntry {
        name: "Chroma Key",
        category: Category::Keying,
        stack: Stack::Keying,
        default_index: 2,
        keywords: &[
            "key",
            "chroma",
            "keylight",
            "green screen",
            "blue screen",
            "matte",
            "gain",
            "balance",
        ],
    },
    BrowserEntry {
        name: "Spill Suppression",
        category: Category::Keying,
        stack: Stack::Keying,
        default_index: 3,
        keywords: &[
            "spill",
            "suppress",
            "despill",
            "fringe",
            "green",
            "blue",
            "edge",
            "neutralise",
        ],
    },
    BrowserEntry {
        name: "Matte Choke",
        category: Category::Keying,
        stack: Stack::Keying,
        default_index: 4,
        keywords: &[
            "choke",
            "matte",
            "erode",
            "dilate",
            "shrink",
            "grow",
            "clip",
            "refine",
        ],
    },
];

/// A scored search hit: the matched registry entry and a relevance `score`
/// (higher = better). Used to rank [`filter`] results.
#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub entry: &'static BrowserEntry,
    pub score: i32,
}

/// Score one registry entry against a lowercase, already-trimmed query.
///
/// An empty query matches everything (score 0). Otherwise every whitespace-split
/// token of the query must match *somewhere* on the entry (the name or one of its
/// keywords) for the entry to be a hit at all (AND across tokens — typing more
/// narrows). Per token, a name match outranks a keyword match, a prefix outranks
/// a mid-string substring, and a whole-name exact match is best of all. Returns
/// `None` when any token fails to match.
fn score_entry(entry: &BrowserEntry, query: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let name = entry.name.to_lowercase();
    let mut total = 0;
    for token in query.split_whitespace() {
        // Best score this token earns against the name or any keyword.
        let mut best: Option<i32> = None;
        // Name match.
        if name == token {
            best = Some(100);
        } else if name.starts_with(token) {
            best = best.max(Some(60));
        } else if name.contains(token) {
            best = best.max(Some(40));
        }
        // Keyword match (weaker than the name).
        for kw in entry.keywords {
            if *kw == token {
                best = best.max(Some(30));
            } else if kw.starts_with(token) {
                best = best.max(Some(20));
            } else if kw.contains(token) {
                best = best.max(Some(10));
            }
        }
        match best {
            Some(s) => total += s,
            None => return None, // this token matched nothing → entry is not a hit
        }
    }
    Some(total)
}

/// Filter and rank the registry against a free-text `query`.
///
/// Case-insensitive and whitespace-tolerant. Returns the matching entries as
/// scored [`Hit`]s sorted **best score first**, ties broken alphabetically by
/// name so the order is stable. An empty/whitespace-only query returns the whole
/// registry in its declared order (score 0, name-tiebroken → alphabetical).
pub fn filter(query: &str) -> Vec<Hit> {
    let q = query.trim().to_lowercase();
    let mut hits: Vec<Hit> = REGISTRY
        .iter()
        .filter_map(|entry| score_entry(entry, &q).map(|score| Hit { entry, score }))
        .collect();
    // Best score first; ties alphabetical for determinism.
    hits.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.entry.name.cmp(b.entry.name))
    });
    hits
}

/// Filter the registry and group the hits **by [`Category`]**, preserving the
/// score ranking within each group. Returns one `(Category, Vec<Hit>)` pair per
/// category that has at least one hit, in [`Category::ALL`] order — the shape the
/// browser's collapsing folders consume.
pub fn filter_grouped(query: &str) -> Vec<(Category, Vec<Hit>)> {
    let hits = filter(query);
    Category::ALL
        .into_iter()
        .filter_map(|cat| {
            let group: Vec<Hit> = hits
                .iter()
                .copied()
                .filter(|h| h.entry.category == cat)
                .collect();
            (!group.is_empty()).then_some((cat, group))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_indices_match_defaults() {
        // Every entry's (stack, default_index) must address a real slot in that
        // stack's defaults() array — and instantiate must return that slot.
        let colors = Effect::defaults();
        let spatials = SpatialEffect::defaults();
        let generates = GenerateEffect::defaults();
        let distorts = DistortEffect::defaults();
        let stylizes = StylizeEffect::defaults();
        let keys = KeyEffect::defaults();
        for entry in REGISTRY {
            match entry.instantiate() {
                NewEffect::Color(e) => {
                    assert!(entry.default_index < colors.len());
                    assert_eq!(e, colors[entry.default_index]);
                    assert_eq!(entry.stack, Stack::Color);
                }
                NewEffect::Spatial(e) => {
                    assert!(entry.default_index < spatials.len());
                    assert_eq!(e, spatials[entry.default_index]);
                    assert_eq!(entry.stack, Stack::Spatial);
                }
                NewEffect::Generate(e) => {
                    assert!(entry.default_index < generates.len());
                    assert_eq!(e, generates[entry.default_index]);
                    assert_eq!(entry.stack, Stack::Generate);
                }
                NewEffect::Distort(e) => {
                    assert!(entry.default_index < distorts.len());
                    assert_eq!(e, distorts[entry.default_index]);
                    assert_eq!(entry.stack, Stack::Distort);
                }
                NewEffect::Stylize(e) => {
                    assert!(entry.default_index < stylizes.len());
                    assert_eq!(e, stylizes[entry.default_index]);
                    assert_eq!(entry.stack, Stack::Stylize);
                }
                NewEffect::Keying(e) => {
                    assert!(entry.default_index < keys.len());
                    assert_eq!(e, keys[entry.default_index]);
                    assert_eq!(entry.stack, Stack::Keying);
                }
            }
        }
    }

    #[test]
    fn registry_names_match_effect_labels() {
        // An entry's display name must equal the label of the effect it builds,
        // so the browser and the per-stack editor name things identically.
        for entry in REGISTRY {
            let label = match entry.instantiate() {
                NewEffect::Color(e) => e.label(),
                NewEffect::Spatial(e) => e.label(),
                NewEffect::Generate(e) => e.label(),
                NewEffect::Distort(e) => e.label(),
                NewEffect::Stylize(e) => e.label(),
                NewEffect::Keying(e) => e.label(),
            };
            assert_eq!(entry.name, label, "name/label mismatch for {}", entry.name);
        }
    }

    #[test]
    fn registry_covers_every_effect() {
        // Every effect in all defaults() arrays is reachable from the registry,
        // so nothing is missing from the browser.
        for (i, _) in Effect::defaults().iter().enumerate() {
            assert!(
                REGISTRY
                    .iter()
                    .any(|e| e.stack == Stack::Color && e.default_index == i),
                "color effect {i} missing from registry"
            );
        }
        for (i, _) in SpatialEffect::defaults().iter().enumerate() {
            assert!(
                REGISTRY
                    .iter()
                    .any(|e| e.stack == Stack::Spatial && e.default_index == i),
                "spatial effect {i} missing from registry"
            );
        }
        for (i, _) in GenerateEffect::defaults().iter().enumerate() {
            assert!(
                REGISTRY
                    .iter()
                    .any(|e| e.stack == Stack::Generate && e.default_index == i),
                "generate effect {i} missing from registry"
            );
        }
        for (i, _) in DistortEffect::defaults().iter().enumerate() {
            assert!(
                REGISTRY
                    .iter()
                    .any(|e| e.stack == Stack::Distort && e.default_index == i),
                "distort effect {i} missing from registry"
            );
        }
        for (i, _) in StylizeEffect::defaults().iter().enumerate() {
            assert!(
                REGISTRY
                    .iter()
                    .any(|e| e.stack == Stack::Stylize && e.default_index == i),
                "stylize effect {i} missing from registry"
            );
        }
        for (i, _) in KeyEffect::defaults().iter().enumerate() {
            assert!(
                REGISTRY
                    .iter()
                    .any(|e| e.stack == Stack::Keying && e.default_index == i),
                "key effect {i} missing from registry"
            );
        }
    }

    #[test]
    fn fractal_noise_is_findable() {
        // The generate workhorse is reachable by name and synonyms.
        for q in ["fractal", "noise", "perlin", "turbulent", "clouds"] {
            let hits = filter(q);
            assert!(
                hits.iter().any(|h| h.entry.name == "Fractal Noise"),
                "querying {q:?} should find Fractal Noise"
            );
        }
        // And it groups under the Generate category.
        let groups = filter_grouped("fractal");
        assert!(groups.iter().any(|(c, _)| *c == Category::Generate));
    }

    #[test]
    fn generate_family_is_findable() {
        // Each new generate effect is reachable by name and a synonym, and groups
        // under the Generate category.
        for (name, queries) in [
            ("Gradient Ramp", ["ramp", "gradient"]),
            ("Checkerboard", ["checker", "checkerboard"]),
            ("4-Color Gradient", ["4 color", "corner"]),
            ("Grid", ["grid", "lines"]),
        ] {
            for q in queries {
                let hits = filter(q);
                assert!(
                    hits.iter().any(|h| h.entry.name == name),
                    "querying {q:?} should find {name}"
                );
            }
        }
        // The Generate folder holds all five generators on an empty query.
        let groups = filter_grouped("");
        let gen = groups
            .iter()
            .find(|(c, _)| *c == Category::Generate)
            .expect("Generate folder present");
        assert_eq!(gen.1.len(), 5, "five generators in the Generate folder");
    }

    #[test]
    fn distort_family_is_findable() {
        // Each distort effect is reachable by name and a synonym, and groups under
        // the Distort category.
        for (name, queries) in [
            ("Corner Pin", ["corner", "perspective"]),
            ("Transform", ["transform", "skew"]),
            ("Mirror", ["mirror", "reflect"]),
            ("Polar Coordinates", ["polar", "radial"]),
        ] {
            for q in queries {
                let hits = filter(q);
                assert!(
                    hits.iter().any(|h| h.entry.name == name),
                    "querying {q:?} should find {name}"
                );
            }
        }
        // The Distort folder holds all four distorts on an empty query.
        let groups = filter_grouped("");
        let dist = groups
            .iter()
            .find(|(c, _)| *c == Category::Distort)
            .expect("Distort folder present");
        assert_eq!(dist.1.len(), 4, "four distorts in the Distort folder");
    }

    #[test]
    fn stylize_family_is_findable() {
        // Each stylize effect is reachable by name and a synonym, and groups under
        // the Stylize category.
        for (name, queries) in [
            ("Find Edges", ["find edges", "sobel"]),
            ("Mosaic", ["mosaic", "pixelate"]),
        ] {
            for q in queries {
                let hits = filter(q);
                assert!(
                    hits.iter().any(|h| h.entry.name == name),
                    "querying {q:?} should find {name}"
                );
            }
        }
        // The Stylize folder holds Glow (spatial stack) plus the two stylize-stack
        // effects on an empty query.
        let groups = filter_grouped("");
        let stylize = groups
            .iter()
            .find(|(c, _)| *c == Category::Stylize)
            .expect("Stylize folder present");
        assert_eq!(stylize.1.len(), 3, "three effects in the Stylize folder");
    }

    #[test]
    fn blur_family_is_findable() {
        // Each blur is reachable by name and a synonym, and groups under the Blur
        // & Sharpen category.
        for (name, queries) in [
            ("Gaussian Blur", ["gaussian", "soften"]),
            ("Box Blur", ["box", "average"]),
            ("Directional Blur", ["directional", "motion"]),
            ("Radial Blur", ["radial", "spin"]),
        ] {
            for q in queries {
                let hits = filter(q);
                assert!(
                    hits.iter().any(|h| h.entry.name == name),
                    "querying {q:?} should find {name}"
                );
            }
        }
        // The Blur folder holds all four blurs on an empty query.
        let groups = filter_grouped("");
        let blur = groups
            .iter()
            .find(|(c, _)| *c == Category::Blur)
            .expect("Blur folder present");
        assert_eq!(blur.1.len(), 4, "four blurs in the Blur & Sharpen folder");
    }

    #[test]
    fn keying_family_is_findable() {
        // Each keyer is reachable by name and a synonym, and groups under the
        // Keying category.
        for (name, queries) in [
            ("Color Key", ["color key", "green screen"]),
            ("Luma Key", ["luma", "luminance"]),
            ("Chroma Key", ["chroma", "keylight"]),
            ("Spill Suppression", ["spill", "despill"]),
            ("Matte Choke", ["choke", "erode"]),
        ] {
            for q in queries {
                let hits = filter(q);
                assert!(
                    hits.iter().any(|h| h.entry.name == name),
                    "querying {q:?} should find {name}"
                );
            }
        }
        // The Keying folder holds all five keyers on an empty query.
        let groups = filter_grouped("");
        let key = groups
            .iter()
            .find(|(c, _)| *c == Category::Keying)
            .expect("Keying folder present");
        assert_eq!(key.1.len(), 5, "five keyers in the Keying folder");
    }

    #[test]
    fn color_correction_family_is_findable() {
        // The newest color-correction effects are reachable by name and a
        // synonym, and group under the Color Correction category.
        for (name, queries) in [
            ("Channel Mixer", ["channel", "monochrome"]),
            ("Gradient Map", ["gradient map", "luma"]),
            ("Tritone", ["tritone", "three tone"]),
        ] {
            for q in queries {
                let hits = filter(q);
                assert!(
                    hits.iter().any(|h| h.entry.name == name),
                    "querying {q:?} should find {name}"
                );
            }
        }
        // The Color Correction folder holds every Effect on an empty query.
        let groups = filter_grouped("");
        let color = groups
            .iter()
            .find(|(c, _)| *c == Category::Color)
            .expect("Color Correction folder present");
        assert_eq!(
            color.1.len(),
            Effect::defaults().len(),
            "every color effect in the Color Correction folder"
        );
    }

    #[test]
    fn empty_query_returns_whole_registry() {
        let hits = filter("");
        assert_eq!(hits.len(), REGISTRY.len());
        // All score 0, so the order is alphabetical by name.
        assert!(hits.iter().all(|h| h.score == 0));
        let names: Vec<&str> = hits.iter().map(|h| h.entry.name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted, "empty query should be alphabetical");
    }

    #[test]
    fn whitespace_query_is_treated_as_empty() {
        assert_eq!(filter("   ").len(), REGISTRY.len());
    }

    #[test]
    fn name_substring_filters() {
        // "blur" hits the whole Blur & Sharpen family (Gaussian / Box /
        // Directional / Radial all carry "blur" in name/keywords).
        let hits = filter("blur");
        let names: Vec<&str> = hits.iter().map(|h| h.entry.name).collect();
        assert!(names.contains(&"Gaussian Blur"));
        assert!(names.contains(&"Box Blur"));
        assert!(names.contains(&"Directional Blur"));
        assert!(names.contains(&"Radial Blur"));
        // Every hit is in the Blur category.
        assert!(hits.iter().all(|h| h.entry.category == Category::Blur));
    }

    #[test]
    fn case_insensitive() {
        let lower = filter("levels");
        let upper = filter("LEVELS");
        assert_eq!(lower.len(), 1);
        assert_eq!(upper.len(), 1);
        assert_eq!(lower[0].entry.name, upper[0].entry.name);
    }

    #[test]
    fn exact_name_outscores_substring() {
        // "glow" exactly names Glow but is also nothing else's substring here.
        let hits = filter("glow");
        assert_eq!(hits[0].entry.name, "Glow");
        assert!(hits[0].score >= 100, "exact name match should score high");
    }

    #[test]
    fn prefix_outranks_mid_substring() {
        // "col" is a prefix of "Color Balance" (name) and only mid-string in
        // some keywords; the name-prefix entry should rank above keyword-only.
        let hits = filter("col");
        assert!(!hits.is_empty());
        assert_eq!(
            hits[0].entry.name, "Color Balance",
            "name-prefix should top keyword-only hits"
        );
    }

    #[test]
    fn keyword_match_finds_effect_not_named_for_it() {
        // "bloom" is only a keyword of Glow, not in any display name.
        let hits = filter("bloom");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entry.name, "Glow");
    }

    #[test]
    fn multi_token_query_is_and() {
        // "color balance" must match both tokens — Color Balance qualifies; a
        // bare "Levels" (no "balance") does not.
        let hits = filter("color balance");
        assert!(hits.iter().any(|h| h.entry.name == "Color Balance"));
        assert!(!hits.iter().any(|h| h.entry.name == "Levels"));
    }

    #[test]
    fn unmatched_token_drops_the_entry() {
        // "color zzz" — no entry has "zzz", so nothing matches.
        assert!(filter("color zzz").is_empty());
    }

    #[test]
    fn no_match_is_empty() {
        assert!(filter("xylophone").is_empty());
    }

    #[test]
    fn results_sorted_best_first() {
        let hits = filter("color");
        for w in hits.windows(2) {
            assert!(
                w[0].score >= w[1].score,
                "filter results must be sorted by descending score"
            );
        }
    }

    #[test]
    fn grouped_preserves_category_order_and_ranking() {
        let groups = filter_grouped("");
        // Empty query: every category that has entries appears, in ALL order.
        let cats: Vec<Category> = groups.iter().map(|(c, _)| *c).collect();
        let expected: Vec<Category> = Category::ALL
            .into_iter()
            .filter(|c| REGISTRY.iter().any(|e| e.category == *c))
            .collect();
        assert_eq!(cats, expected);
        // Total hits across groups equals the flat filter count.
        let total: usize = groups.iter().map(|(_, h)| h.len()).sum();
        assert_eq!(total, filter("").len());
        // Within each group, ranking is preserved (descending score).
        for (_, hits) in &groups {
            for w in hits.windows(2) {
                assert!(w[0].score >= w[1].score);
            }
        }
    }

    #[test]
    fn grouped_drops_empty_categories() {
        // "blur" only hits the Blur category → exactly one group, holding the
        // whole Gaussian / Box / Directional / Radial family.
        let groups = filter_grouped("blur");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].0, Category::Blur);
        assert_eq!(groups[0].1.len(), 4);
    }

    #[test]
    fn every_category_has_a_distinct_nonempty_label() {
        let mut seen = Vec::new();
        for c in Category::ALL {
            assert!(!c.label().is_empty());
            assert!(!seen.contains(&c.label()), "duplicate category label");
            seen.push(c.label());
        }
    }
}
