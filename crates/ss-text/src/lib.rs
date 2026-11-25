//! Text to cut paths: finds installed fonts with `fontdb` and turns glyph outlines from `ab_glyph`
//! into Bézier paths in millimetres, keeping the font's own quadratic and cubic curves.
//!
//! Layout is deliberately simple: left-aligned lines, horizontal advances, legacy `kern` table
//! pairs and extra letter spacing. Complex shaping (ligatures, right-to-left scripts) is out of
//! scope.

use ab_glyph::{Font, FontRef, GlyphId, OutlineCurve};
use fontdb::{Database, Family, Query, Style, Weight};
use ss_doc::TextData;
use ss_geom::{BezPath, Point};

/// Slant used when an italic is requested but the family has no italic face.
const SYNTHETIC_ITALIC_DEG: f64 = 12.0;

const SANS: &[&str] = &["DejaVu Sans", "Liberation Sans", "Noto Sans", "Cantarell", "Adwaita Sans", "Ubuntu", "Arial"];
const SERIF: &[&str] = &["DejaVu Serif", "Liberation Serif", "Noto Serif", "Times New Roman"];
const MONO: &[&str] = &["DejaVu Sans Mono", "Liberation Mono", "Noto Sans Mono", "JetBrains Mono", "Hack", "Courier New"];

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum TextError {
    #[error("no usable fonts are installed")]
    NoFonts,
    #[error("text size must be a positive number of millimetres")]
    InvalidSize,
    #[error("the font file for {0} could not be read")]
    Unreadable(String),
}

/// The face chosen for a request, after family fallback and style matching.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedFace {
    pub id: fontdb::ID,
    pub family: String,
    pub bold: bool,
    pub italic: bool,
    /// Italic was requested but only an upright face exists, so outlines are slanted.
    pub synthetic_italic: bool,
}

#[derive(Debug, Clone)]
pub struct TextLayout {
    /// Glyph outlines in local mm, y down. The first line's baseline sits at the font ascent, so
    /// the origin is the top-left corner of the text block.
    pub outlines: BezPath,
    pub face: ResolvedFace,
    /// Advance width of the longest line, mm.
    pub width_mm: f64,
    /// Height of all lines, mm.
    pub height_mm: f64,
    /// Characters the font has no glyph for (whitespace excluded).
    pub missing_glyphs: usize,
}

pub struct FontLibrary {
    db: Database,
}

impl FontLibrary {
    /// Fonts installed on the system (fontconfig directories, `~/.local/share/fonts`, …).
    pub fn system() -> Self {
        let mut db = Database::new();
        db.load_system_fonts();
        Self { db }
    }

    /// A library holding only the given font files' bytes.
    pub fn from_data(fonts: impl IntoIterator<Item = Vec<u8>>) -> Self {
        let mut db = Database::new();
        fonts.into_iter().for_each(|f| db.load_font_data(f));
        Self { db }
    }

    pub fn is_empty(&self) -> bool {
        self.db.is_empty()
    }

    /// Sorted, de-duplicated family names.
    pub fn families(&self) -> Vec<String> {
        let mut v: Vec<String> = self.db.faces().filter_map(|f| f.families.first().map(|(n, _)| n.clone())).collect();
        v.sort_by_key(|s| s.to_lowercase());
        v.dedup();
        v
    }

    /// Picks a face for `family`, falling back to a common family of the same kind (sans, serif or
    /// monospace) and finally to any installed face.
    pub fn resolve(&self, family: &str, bold: bool, italic: bool) -> Option<ResolvedFace> {
        let lower = family.to_lowercase();
        let generic = if lower.contains("mono") || lower.contains("courier") {
            MONO
        } else if lower.contains("serif") && !lower.contains("sans") {
            SERIF
        } else {
            SANS
        };
        let mut families = vec![Family::Name(family)];
        families.extend(generic.iter().map(|n| Family::Name(n)));
        families.extend(SANS.iter().map(|n| Family::Name(n)));
        let query = Query {
            families: &families,
            weight: if bold { Weight::BOLD } else { Weight::NORMAL },
            style: if italic { Style::Italic } else { Style::Normal },
            ..Query::default()
        };
        let id = self.db.query(&query).or_else(|| self.db.faces().next().map(|f| f.id))?;
        let face = self.db.face(id)?;
        let face_italic = face.style != Style::Normal;
        Some(ResolvedFace {
            id,
            family: face.families.first().map(|(n, _)| n.clone()).unwrap_or_default(),
            bold: face.weight.0 >= 600,
            italic: face_italic,
            synthetic_italic: italic && !face_italic,
        })
    }

    /// Em size, mm, of the resolved face when its ascent-to-descent height is `height_mm`. Tools
    /// that size text by that height (such as `ab_glyph`'s `PxScale`) need this to match.
    pub fn em_for_height(&self, family: &str, bold: bool, italic: bool, height_mm: f64) -> Option<f64> {
        let face = self.resolve(family, bold, italic)?;
        self.db
            .with_face_data(face.id, |data, index| {
                let f = FontRef::try_from_slice_and_index(data, index).ok()?;
                let h = f64::from(f.height_unscaled());
                (h > 0.0).then(|| height_mm * f64::from(f.units_per_em().unwrap_or(1000.0)) / h)
            })
            .flatten()
    }

    pub fn layout(&self, t: &TextData) -> Result<TextLayout, TextError> {
        if !(t.size_mm.is_finite() && t.size_mm > 0.0) {
            return Err(TextError::InvalidSize);
        }
        let face = self.resolve(&t.font_family, t.bold, t.italic).ok_or(TextError::NoFonts)?;
        let unreadable = || TextError::Unreadable(face.family.clone());
        let laid = self
            .db
            .with_face_data(face.id, |data, index| {
                FontRef::try_from_slice_and_index(data, index).ok().map(|f| outline_text(&f, t, face.synthetic_italic))
            })
            .flatten()
            .ok_or_else(unreadable)?;
        Ok(TextLayout { outlines: laid.path, face, width_mm: laid.width, height_mm: laid.height, missing_glyphs: laid.missing })
    }
}

struct Laid {
    path: BezPath,
    width: f64,
    height: f64,
    missing: usize,
}

fn outline_text(font: &impl Font, t: &TextData, oblique: bool) -> Laid {
    let upem = f64::from(font.units_per_em().unwrap_or(1000.0));
    let s = t.size_mm / upem;
    let ascent = f64::from(font.ascent_unscaled());
    let line_h = f64::from(font.height_unscaled() + font.line_gap_unscaled()) * s * t.line_spacing.max(0.0);
    let skew = if oblique { SYNTHETIC_ITALIC_DEG.to_radians().tan() } else { 0.0 };
    let mut out = Laid { path: BezPath::new(), width: 0.0, height: 0.0, missing: 0 };
    let lines: Vec<&str> = t.text.split('\n').map(|l| l.trim_end_matches('\r')).collect();
    for (row, line) in lines.iter().enumerate() {
        let baseline = ascent * s + row as f64 * line_h;
        let mut caret = 0.0;
        let mut prev: Option<GlyphId> = None;
        for ch in line.chars() {
            let gid = font.glyph_id(ch);
            if gid.0 == 0 && !ch.is_whitespace() {
                out.missing += 1;
            }
            if let Some(p) = prev {
                caret += f64::from(font.kern_unscaled(p, gid)) * s + t.letter_spacing_mm;
            }
            if let Some(o) = font.outline(gid) {
                let map = |p: ab_glyph::Point| {
                    let (x, y) = (f64::from(p.x), f64::from(p.y));
                    Point::new(caret + (x + skew * y) * s, baseline - y * s)
                };
                append_curves(&mut out.path, &o.curves, map);
            }
            caret += f64::from(font.h_advance_unscaled(gid)) * s;
            prev = Some(gid);
        }
        out.width = out.width.max(caret);
    }
    let descent = -f64::from(font.descent_unscaled()) * s;
    out.height = ascent * s + descent + (lines.len().saturating_sub(1)) as f64 * line_h;
    out
}

/// `ab_glyph` yields contours as a flat list of segments; a segment that does not start where the
/// previous one ended begins a new contour.
fn append_curves(path: &mut BezPath, curves: &[OutlineCurve], map: impl Fn(ab_glyph::Point) -> Point) {
    let mut last: Option<ab_glyph::Point> = None;
    for c in curves {
        let (start, end) = match *c {
            OutlineCurve::Line(a, b) | OutlineCurve::Quad(a, _, b) | OutlineCurve::Cubic(a, _, _, b) => (a, b),
        };
        if last != Some(start) {
            if last.is_some() {
                path.close_path();
            }
            path.move_to(map(start));
        }
        match *c {
            OutlineCurve::Line(_, b) => path.line_to(map(b)),
            OutlineCurve::Quad(_, c1, b) => path.quad_to(map(c1), map(b)),
            OutlineCurve::Cubic(_, c1, c2, b) => path.curve_to(map(c1), map(c2), map(b)),
        }
        last = Some(end);
    }
    if last.is_some() {
        path.close_path();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ss_geom::{Shape, ops};

    fn lib() -> FontLibrary {
        FontLibrary::from_data([epaint_default_fonts::HACK_REGULAR.to_vec(), epaint_default_fonts::UBUNTU_LIGHT.to_vec()])
    }

    fn text(s: &str, family: &str) -> TextData {
        TextData { text: s.into(), font_family: family.into(), ..TextData::default() }
    }

    #[test]
    fn families_and_fallback() {
        let l = lib();
        assert_eq!(l.families(), vec!["Hack".to_string(), "Ubuntu".to_string()]);
        assert_eq!(l.resolve("Hack", false, false).unwrap().family, "Hack");
        // Unknown sans family falls back to the sans list (Ubuntu), unknown mono to Hack.
        assert_eq!(l.resolve("Nonexistent", false, false).unwrap().family, "Ubuntu");
        assert_eq!(l.resolve("Some Mono", false, false).unwrap().family, "Hack");
        let it = l.resolve("Hack", false, true).unwrap();
        assert!(it.synthetic_italic && !it.italic);
        assert!(FontLibrary::from_data([]).layout(&text("A", "Hack")).is_err());
    }

    #[test]
    fn monospace_advances_and_spacing() {
        let l = lib();
        let w = |s: &str, ls: f64| l.layout(&TextData { letter_spacing_mm: ls, ..text(s, "Hack") }).unwrap().width_mm;
        let one = w("M", 0.0);
        assert!(one > 3.0 && one < 10.0, "{one}");
        assert!((w("iiii", 0.0) - 4.0 * one).abs() < 1e-9);
        assert!((w("MMMM", 0.0) - 4.0 * one).abs() < 1e-9);
        // Spacing goes between glyphs only.
        assert!((w("MM", 2.0) - w("MM", 0.0) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn outlines_scale_and_contours() {
        let l = lib();
        let h10 = l.layout(&text("H", "Hack")).unwrap();
        let h20 = l.layout(&TextData { size_mm: 20.0, ..text("H", "Hack") }).unwrap();
        let (b10, b20) = (h10.outlines.bounding_box(), h20.outlines.bounding_box());
        assert!((b20.height() - 2.0 * b10.height()).abs() < 1e-9 && (b20.x0 - 2.0 * b10.x0).abs() < 1e-9);
        // Cap height of a 10 mm em is well under 10 mm and the glyph sits below y = 0.
        assert!(b10.y0 > 0.0 && b10.height() > 5.0 && b10.height() < 10.0, "{b10:?}");
        assert_eq!(ops::subpaths(&h10.outlines).len(), 1);
        assert_eq!(ops::subpaths(&l.layout(&text("O", "Hack")).unwrap().outlines).len(), 2);
        // Curves are kept, not flattened.
        let o = l.layout(&text("O", "Ubuntu")).unwrap().outlines;
        assert!(o.elements().iter().any(|e| matches!(e, ss_geom::kurbo::PathEl::QuadTo(..) | ss_geom::kurbo::PathEl::CurveTo(..))));
    }

    #[test]
    fn lines_italic_and_missing() {
        let l = lib();
        let one = l.layout(&text("A", "Hack")).unwrap();
        let two = l.layout(&text("A\nA", "Hack")).unwrap();
        assert_eq!(ops::subpaths(&two.outlines).len(), 2 * ops::subpaths(&one.outlines).len());
        let gap = two.outlines.bounding_box().height() - one.outlines.bounding_box().height();
        assert!((two.height_mm - one.height_mm - gap).abs() < 1e-9 && gap > 10.0);
        let up = l.layout(&text("I", "Hack")).unwrap().outlines.bounding_box();
        let it = l.layout(&TextData { italic: true, ..text("I", "Hack") }).unwrap().outlines.bounding_box();
        assert!(it.width() > up.width() + 1.0);
        assert_eq!(l.layout(&text("a漢 b", "Hack")).unwrap().missing_glyphs, 1);
        assert_eq!(l.layout(&TextData { size_mm: 0.0, ..text("a", "Hack") }).unwrap_err(), TextError::InvalidSize);
    }

    #[test]
    fn em_for_height_inverts_line_height() {
        let l = lib();
        let em = l.em_for_height("Hack", false, false, 10.0).unwrap();
        let laid = l.layout(&TextData { size_mm: em, ..text("H", "Hack") }).unwrap();
        // Single-line layout height is ascent + descent.
        assert!((laid.height_mm - 10.0).abs() < 1e-9, "{}", laid.height_mm);
    }

    #[test]
    fn system_fonts_smoke() {
        let l = FontLibrary::system();
        if l.is_empty() {
            return;
        }
        let r = l.layout(&text("Silhouette", "Sans")).unwrap();
        assert!(r.width_mm > 20.0 && !r.outlines.elements().is_empty(), "{} via {}", r.width_mm, r.face.family);
    }

    /// Cross-checks outline placement against usvg's independent text-to-path pipeline
    /// (rustybuzz + ttf-parser) for a font without kerning.
    #[test]
    fn matches_usvg_text_outlines() {
        let mut db = fontdb::Database::new();
        db.load_font_data(epaint_default_fonts::HACK_REGULAR.to_vec());
        let opt = usvg::Options { fontdb: std::sync::Arc::new(db), ..usvg::Options::default() };
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="50"><text x="0" y="20" font-family="Hack" font-size="10">Hag Wq</text></svg>"#;
        let tree = usvg::Tree::from_str(svg, &opt).unwrap();
        let Some(usvg::Node::Text(t)) = tree.root().children().first() else { panic!("no text node") };
        // The text node's own box is the layout box; the flattened group holds the glyph paths.
        let layout_box = t.abs_bounding_box();
        let theirs = t.flattened().abs_bounding_box().expect("glyph paths");
        let ours = lib().layout(&text("Hag Wq", "Hack")).unwrap();
        assert!((f64::from(layout_box.width()) - ours.width_mm).abs() < 0.01, "{layout_box:?} vs {}", ours.width_mm);
        let b = ours.outlines.bounding_box();
        assert!((f64::from(theirs.width()) - b.width()).abs() < 0.01, "{theirs:?} vs {b:?}");
        assert!((f64::from(theirs.height()) - b.height()).abs() < 0.01, "{theirs:?} vs {b:?}");
        assert!((f64::from(theirs.left()) - b.x0).abs() < 0.01, "{theirs:?} vs {b:?}");
    }
}
