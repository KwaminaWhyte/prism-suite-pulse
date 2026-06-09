//! Font enumeration + resolution for text layers' **real outline** path.
//!
//! Pulse's text layers default to a self-contained, dependency-free stroke font
//! ([`crate::comp::text`]); selecting a real font family instead lays the string
//! out into TrueType glyph outlines. This module is the bridge between a
//! *font-family name* (what the user picks in the Properties panel and what is
//! persisted per [`TextLayer`](super::text::TextLayer)) and the *raw face bytes*
//! that [`super::text`] parses with [`ttf_parser`].
//!
//! Two pieces of process-wide state, built lazily on first use and shared by
//! every text layer (layout is called from the compositor, the matte path,
//! motion blur, and tests):
//!
//! * a [`fontdb::Database`] of installed system faces (plus the bundled default),
//!   used purely to *enumerate* family names and locate a family's face source;
//! * a small cache of already-resolved face bytes keyed by family name, so a
//!   given family's font file is read from disk at most once — never per frame.
//!
//! Resolution is deliberately forgiving: an unknown / missing family always
//! falls back to the bundled [`Ubuntu Light`](DEFAULT_FONT_FAMILY) face, so a
//! text layer referencing a font absent on this machine still renders rather
//! than vanishing. (A layer with no family — `None`, the default and every
//! legacy `.pulse` file — never reaches here at all: it keeps the built-in stroke
//! font.)

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

/// The bundled default font (Ubuntu Light, Ubuntu Font Licence — see
/// `assets/fonts/UFL.txt`). Embedded so the family dropdown's default outline
/// entry always has a usable face, even on a machine with no fonts installed.
pub static DEFAULT_FONT: &[u8] = include_bytes!("../../assets/fonts/Ubuntu-Light.ttf");

/// The family name of the bundled [`DEFAULT_FONT`]. The guaranteed fallback
/// target for any unknown / missing family. Kept in sync with the embedded face's
/// `name` table (Ubuntu Light's typographic family is "Ubuntu").
pub const DEFAULT_FONT_FAMILY: &str = "Ubuntu";

/// Raw bytes of a single font face plus the face index within its file (non-zero
/// for TrueType collections, `.ttc`). Cheap to clone (the bytes are shared).
#[derive(Clone)]
pub struct FaceBytes {
    pub data: Arc<Vec<u8>>,
    pub index: u32,
}

impl FaceBytes {
    /// The bundled face, wrapped without copying the embedded slice into a new
    /// allocation more than once.
    fn bundled() -> Self {
        FaceBytes {
            data: Arc::new(DEFAULT_FONT.to_vec()),
            index: 0,
        }
    }
}

/// Lazily-built system font database. Enumerating + loading system fonts can take
/// a beat, so it is built once on first access and reused.
fn database() -> &'static fontdb::Database {
    static DB: OnceLock<fontdb::Database> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = fontdb::Database::new();
        // System faces first, then the bundled face so the default family is
        // always queryable even on a machine with no fonts installed.
        db.load_system_fonts();
        db.load_font_data(DEFAULT_FONT.to_vec());
        db
    })
}

/// Per-family resolved-bytes cache, so each family's file is read at most once.
fn cache() -> &'static RwLock<HashMap<String, FaceBytes>> {
    static CACHE: OnceLock<RwLock<HashMap<String, FaceBytes>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Every available font-family name, sorted and de-duplicated, with the bundled
/// [`DEFAULT_FONT_FAMILY`] guaranteed present and first. This is what the
/// Properties panel's Font dropdown lists (after its "Built-in stroke font"
/// entry, which maps to a `None` family).
///
/// Built once and memoised — enumerating the database's faces is cheap but the
/// list is stable for the process lifetime.
pub fn families() -> &'static [String] {
    static FAMILIES: OnceLock<Vec<String>> = OnceLock::new();
    FAMILIES.get_or_init(|| {
        let mut names: Vec<String> = database()
            .faces()
            .flat_map(|f| f.families.iter().map(|(name, _)| name.clone()))
            .collect();
        names.sort_unstable();
        names.dedup();
        // Surface the bundled default first, without duplicating it if the system
        // also happens to have "Ubuntu" installed.
        names.retain(|n| n != DEFAULT_FONT_FAMILY);
        names.insert(0, DEFAULT_FONT_FAMILY.to_string());
        names
    })
}

/// Whether `family` names a face the database can supply (case-sensitive, matching
/// fontdb's query). The bundled default always counts.
pub fn is_available(family: &str) -> bool {
    family == DEFAULT_FONT_FAMILY || query_source(family).is_some()
}

/// Resolve a family name to concrete face bytes, caching the result. Any family
/// the database can't supply resolves to the bundled [`DEFAULT_FONT`] — resolution
/// never fails, so a selected family always yields a usable face (text never
/// vanishes).
pub fn resolve(family: &str) -> FaceBytes {
    if let Some(hit) = cache().read().ok().and_then(|c| c.get(family).cloned()) {
        return hit;
    }

    let bytes = load_uncached(family);
    if let Ok(mut c) = cache().write() {
        // Return whatever the cache holds — a concurrent resolve may have inserted
        // first; `or_insert_with` keeps that one, so every caller shares a single
        // allocation per family (a font file is read at most once).
        return c
            .entry(family.to_string())
            .or_insert_with(|| bytes.clone())
            .clone();
    }
    bytes
}

/// Look up the fontdb `ID` for a family name (regular weight / normal style),
/// without touching the bytes cache. Used both to test availability and to drive
/// [`load_uncached`].
fn query_source(family: &str) -> Option<fontdb::ID> {
    database().query(&fontdb::Query {
        families: &[fontdb::Family::Name(family)],
        weight: fontdb::Weight::NORMAL,
        stretch: fontdb::Stretch::Normal,
        style: fontdb::Style::Normal,
    })
}

/// Read a family's face bytes from the database (uncached), falling back to the
/// bundled face for an unknown family or any read error.
fn load_uncached(family: &str) -> FaceBytes {
    query_source(family)
        .and_then(read_face_bytes)
        .unwrap_or_else(FaceBytes::bundled)
}

/// Copy a resolved face's bytes out of the database into an owned, shareable
/// buffer (fontdb only lends the data inside a closure). `None` if the source
/// can't be read.
fn read_face_bytes(id: fontdb::ID) -> Option<FaceBytes> {
    database().with_face_data(id, |data, index| FaceBytes {
        data: Arc::new(data.to_vec()),
        index,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The family list is non-empty and always offers the bundled default first.
    #[test]
    fn families_lists_default_first() {
        let fams = families();
        assert!(!fams.is_empty(), "at least the bundled family is listed");
        assert_eq!(
            fams[0], DEFAULT_FONT_FAMILY,
            "bundled default is surfaced first"
        );
        // No duplicate of the default even if the system also has it.
        assert_eq!(
            fams.iter().filter(|f| *f == DEFAULT_FONT_FAMILY).count(),
            1,
            "default family appears exactly once"
        );
    }

    /// An unknown family falls back to the bundled face rather than failing.
    #[test]
    fn unknown_family_falls_back() {
        let face = resolve("This Font Does Not Exist 12345");
        assert!(
            ttf_parser::Face::parse(&face.data, face.index).is_ok(),
            "unknown family still yields a usable face"
        );
        assert!(
            !is_available("This Font Does Not Exist 12345"),
            "the bogus family is reported unavailable"
        );
    }

    /// The bundled default is always reported available and resolves to a face
    /// whose glyph for 'A' exists (proving it is a real, usable face).
    #[test]
    fn default_family_is_available_and_usable() {
        assert!(is_available(DEFAULT_FONT_FAMILY));
        let face = resolve(DEFAULT_FONT_FAMILY);
        let parsed = ttf_parser::Face::parse(&face.data, face.index).expect("default parses");
        assert!(
            parsed.glyph_index('A').is_some(),
            "default face has a glyph for 'A'"
        );
    }

    /// Resolving the same family twice returns byte-identical (shared) data — the
    /// cache hands back the same allocation rather than re-reading the file.
    #[test]
    fn resolution_is_cached() {
        let a = resolve(DEFAULT_FONT_FAMILY);
        let b = resolve(DEFAULT_FONT_FAMILY);
        assert!(
            Arc::ptr_eq(&a.data, &b.data),
            "second resolve returns the cached, shared buffer"
        );
    }
}
