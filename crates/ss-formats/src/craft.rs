//! `.craft` projects of the companion app
//! [Studio to SVG](https://github.com/vineofthedead-sudo/Studio_to_SVG): plain JSON, format
//! version 2, lengths in inches, y down.
//!
//! Import:
//! * Rectangle, Ellipse, Line and Freehand shapes become path objects; a shape carrying an
//!   embedded pixmap becomes an image object (never cut).
//! * Text shapes become editable text. The companion sizes text by the font's ascent-to-descent
//!   height, so the em size is derived from the installed font when a [`FontLibrary`] is given.
//! * `group_path` chains become nested groups, the paper image becomes the document background
//!   and `show_reg_marks` becomes registration marks with the companion's mark geometry.
//!
//! Export writes every leaf object as a `Freehand` shape whose `text` is straight-line SVG path
//! data, because the companion draws path data as polylines; curves and text are flattened.
//! Layers, cut actions and image rotation have no `.craft` equivalent and are dropped.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use ss_doc::{Document, ImageData, LayerId, Object, ObjectKind, RegMarks, Rgba, TextData, b64};
use ss_geom::flatten::flatten;
use ss_geom::kurbo::{Ellipse, Shape as _};
use ss_geom::{Affine, BezPath, FillRule, Point, Rect, ops};
use ss_text::FontLibrary;

const MM_PER_IN: f64 = 25.4;
pub const APP_NAME: &str = "Studio to SVG";
pub const VERSION: u32 = 2;
pub const EXTENSION: &str = "craft";
/// The font the companion renders text with (the first of its fallbacks).
pub const COMPANION_FONT: &str = "DejaVu Sans";
/// Flattening tolerance used on export, mm: half a cutter step (1/20 mm).
const FLATTEN_MM: f64 = 0.025;
/// Companion registration mark geometry, inches.
const REG_SIZE_IN: f64 = 0.20;
const REG_THICKNESS_IN: f64 = 0.02;

fn default_true() -> bool {
    true
}

fn default_reg_inset() -> f32 {
    0.25
}

/// One design element, field for field as the companion writes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CraftShape {
    /// `Rectangle`, `Ellipse`, `Line`, `Freehand` or `Text`; anything else is read as `Freehand`.
    pub kind: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub points: Vec<[f32; 2]>,
    /// Text content, or SVG path data when `is_svg_path` is set.
    pub text: String,
    pub fill_color: [u8; 4],
    #[serde(default)]
    pub stroke_color: Option<[u8; 4]>,
    #[serde(default)]
    pub stroke_width: f32,
    #[serde(default)]
    pub is_closed: bool,
    #[serde(default)]
    pub is_svg_path: bool,
    #[serde(default)]
    pub group_id: Option<u64>,
    /// Group ids from outermost to innermost.
    #[serde(default)]
    pub group_path: Vec<u64>,
    #[serde(default = "default_true")]
    pub is_visible: bool,
    #[serde(default)]
    pub is_locked: bool,
    #[serde(default)]
    pub name: Option<String>,
    /// Base64 PNG.
    #[serde(default)]
    pub pixmap_b64: Option<String>,
    #[serde(default)]
    pub pixmap_width: u32,
    #[serde(default)]
    pub pixmap_height: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CraftProject {
    pub version: u32,
    pub app_name: String,
    pub mat_width_in: f32,
    pub mat_height_in: f32,
    /// Back to front.
    pub shapes: Vec<CraftShape>,
    pub group_names: BTreeMap<u64, String>,
    pub next_group_id: u64,
    /// Base64 PNG of the paper background.
    #[serde(default)]
    pub paper_image_b64: Option<String>,
    #[serde(default)]
    pub show_reg_marks: bool,
    #[serde(default = "default_reg_inset")]
    pub reg_mark_inset_in: f32,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub source_path: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum CraftError {
    #[error("invalid .craft file: {0}")]
    Json(#[from] serde_json::Error),
}

/// Whether the start of a (lower-cased) JSON file looks like a `.craft` project.
pub fn detect(head: &str) -> bool {
    head.contains("\"app_name\"") && head.contains("\"mat_width_in\"")
}

pub fn parse(json: &str) -> Result<CraftProject, CraftError> {
    Ok(serde_json::from_str(json)?)
}

/// Parses and converts a `.craft` file. Text is laid out with `fonts` when given; otherwise text
/// objects carry no outlines until the caller lays them out.
pub fn import(json: &str, fonts: Option<&FontLibrary>) -> Result<Document, CraftError> {
    Ok(project_to_document(&parse(json)?, fonts))
}

pub fn has_text(p: &CraftProject) -> bool {
    p.shapes.iter().any(|s| s.kind == "Text" && s.pixmap_b64.is_none())
}

enum Entry {
    Leaf(Box<Object>),
    Group(u64, Vec<Entry>),
}

fn insert(entries: &mut Vec<Entry>, chain: &[u64], obj: Object) {
    let Some((&g, rest)) = chain.split_first() else {
        entries.push(Entry::Leaf(Box::new(obj)));
        return;
    };
    let pos = entries.iter().position(|e| matches!(e, Entry::Group(id, _) if *id == g));
    let idx = pos.unwrap_or_else(|| {
        entries.push(Entry::Group(g, Vec::new()));
        entries.len() - 1
    });
    if let Entry::Group(_, children) = &mut entries[idx] {
        insert(children, rest, obj);
    }
}

fn build(e: Entry, layer: LayerId, names: &BTreeMap<u64, String>) -> Object {
    match e {
        Entry::Leaf(o) => *o,
        Entry::Group(id, children) => {
            let name = names.get(&id).cloned().unwrap_or_else(|| "Group".into());
            let mut g = Object::path(name, layer, BezPath::new());
            g.kind = ObjectKind::Group { children: children.into_iter().map(|c| build(c, layer, names)).collect() };
            g
        }
    }
}

pub fn project_to_document(p: &CraftProject, fonts: Option<&FontLibrary>) -> Document {
    let mut doc = Document::default();
    let (w, h) = (f64::from(p.mat_width_in) * MM_PER_IN, f64::from(p.mat_height_in) * MM_PER_IN);
    if w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0 {
        doc.page = crate::studio::page_for_size(w, h);
    }
    if p.show_reg_marks {
        doc.page.registration = Some(RegMarks {
            inset_mm: f64::from(p.reg_mark_inset_in) * MM_PER_IN,
            length_mm: REG_SIZE_IN * MM_PER_IN,
            thickness_mm: REG_THICKNESS_IN * MM_PER_IN,
            square_mm: REG_SIZE_IN * MM_PER_IN,
        });
    }
    doc.background = p.paper_image_b64.as_deref().and_then(b64::decode).and_then(|b| ImageData::from_png(b, doc.page.rect()));
    let layer = doc.default_layer();
    let mut entries = Vec::new();
    for s in &p.shapes {
        let Some(obj) = shape_to_object(s, layer, fonts) else { continue };
        let chain = if s.group_path.is_empty() { s.group_id.into_iter().collect() } else { s.group_path.clone() };
        insert(&mut entries, &chain, obj);
    }
    doc.objects = entries.into_iter().map(|e| build(e, layer, &p.group_names)).collect();
    doc
}

fn mm(v: f32) -> f64 {
    f64::from(v) * MM_PER_IN
}

fn pt([x, y]: [f32; 2]) -> Point {
    Point::new(mm(x), mm(y))
}

fn polyline(points: &[[f32; 2]], closed: bool) -> BezPath {
    let mut p = BezPath::new();
    if let Some((first, rest)) = points.split_first() {
        p.move_to(pt(*first));
        rest.iter().for_each(|q| p.line_to(pt(*q)));
        if closed {
            p.close_path();
        }
    }
    p
}

/// Path data in inches. Falls back to the companion's own reading (numbers taken in pairs as
/// polyline vertices, one closed polygon per `M`) when the data is not valid SVG.
fn parse_path_data(d: &str) -> BezPath {
    let mut p = BezPath::from_svg(d).unwrap_or_else(|_| {
        let mut p = BezPath::new();
        for sub in d.split(['M', 'm']).filter(|s| !s.trim().is_empty()) {
            let nums: Vec<f64> = sub
                .split(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
                .filter_map(|n| n.parse().ok())
                .collect();
            let (pairs, _) = nums.as_chunks::<2>();
            if let Some((first, rest)) = pairs.split_first() {
                p.move_to((first[0], first[1]));
                rest.iter().for_each(|q| p.line_to((q[0], q[1])));
                p.close_path();
            }
        }
        p
    });
    p.apply_affine(Affine::scale(MM_PER_IN));
    p
}

fn shape_to_object(s: &CraftShape, layer: LayerId, fonts: Option<&FontLibrary>) -> Option<Object> {
    let (x0, y0) = (mm(s.x), mm(s.y));
    let r = Rect::new(x0, y0, x0 + mm(s.width), y0 + mm(s.height)).abs();
    let name = s.name.clone().filter(|n| !n.trim().is_empty());
    if let Some(img) = s.pixmap_b64.as_deref().and_then(b64::decode).and_then(|b| ImageData::from_png(b, r)) {
        let mut o = Object::image(name.unwrap_or_else(|| "Image".into()), layer, img);
        o.visible = s.is_visible;
        o.locked = s.is_locked;
        return Some(o);
    }
    let mut o = match s.kind.as_str() {
        "Text" => text_object(s, r, layer, fonts),
        kind => {
            let (path, rule) = match kind {
                "Rectangle" => (r.to_path(0.0), FillRule::NonZero),
                "Ellipse" => (Ellipse::from_rect(r).to_path(0.001), FillRule::NonZero),
                "Line" => match s.points.as_slice() {
                    [a, b, ..] => (polyline(&[*a, *b], false), FillRule::NonZero),
                    _ => {
                        let mut p = BezPath::new();
                        p.move_to((x0, y0));
                        p.line_to((x0 + mm(s.width), y0 + mm(s.height)));
                        (p, FillRule::NonZero)
                    }
                },
                _ if s.is_svg_path && !s.text.trim().is_empty() => {
                    let p = parse_path_data(&s.text);
                    // The companion fills multi-contour paths even-odd and single ones non-zero.
                    let rule = if ops::subpaths(&p).len() > 1 { FillRule::EvenOdd } else { FillRule::NonZero };
                    (p, rule)
                }
                _ => (polyline(&s.points, s.is_closed), FillRule::NonZero),
            };
            if path.elements().is_empty() {
                return None;
            }
            let mut o = Object::path(name.clone().unwrap_or_else(|| kind_name(kind).into()), layer, path);
            if let ObjectKind::Path { fill_rule, .. } = &mut o.kind {
                *fill_rule = rule;
            }
            o
        }
    };
    if let Some(n) = name {
        o.name = n;
    }
    let [r_, g, b, a] = s.fill_color;
    o.style.fill = (a > 0).then_some(Rgba { r: r_, g, b, a });
    match s.stroke_color {
        Some([r, g, b, a]) => o.style.stroke = Rgba { r, g, b, a },
        None => {
            if let Some(f) = o.style.fill {
                o.style.stroke = f;
            }
        }
    }
    if s.stroke_width > 0.0 && s.stroke_width.is_finite() {
        o.style.stroke_width_mm = mm(s.stroke_width);
    }
    o.visible = s.is_visible;
    o.locked = s.is_locked;
    Some(o)
}

fn kind_name(kind: &str) -> &'static str {
    match kind {
        "Rectangle" => "Rectangle",
        "Ellipse" => "Ellipse",
        "Line" => "Line",
        _ => "Path",
    }
}

fn text_object(s: &CraftShape, r: Rect, layer: LayerId, fonts: Option<&FontLibrary>) -> Object {
    let height = if r.height() > 0.0 { r.height() } else { 10.0 };
    let size_mm = fonts.and_then(|f| f.em_for_height(COMPANION_FONT, false, false, height)).unwrap_or(height);
    let data = TextData { text: s.text.clone(), font_family: COMPANION_FONT.into(), size_mm, ..TextData::default() };
    let outlines = fonts.and_then(|f| f.layout(&data).ok()).map(|l| l.outlines).unwrap_or_default();
    let mut o = Object::text(layer, data, outlines);
    o.transform = Affine::translate((r.x0, r.y0));
    o
}

/// Compact JSON: pretty-printing puts every coordinate on its own line, which makes detailed
/// designs several times larger. The companion reads both.
pub fn export(doc: &Document, title: Option<&str>) -> Result<String, CraftError> {
    Ok(serde_json::to_string(&document_to_project(doc, title))?)
}

pub fn document_to_project(doc: &Document, title: Option<&str>) -> CraftProject {
    let mut p = CraftProject {
        version: VERSION,
        app_name: APP_NAME.into(),
        mat_width_in: (doc.page.width_mm / MM_PER_IN) as f32,
        mat_height_in: (doc.page.height_mm / MM_PER_IN) as f32,
        shapes: Vec::new(),
        group_names: BTreeMap::new(),
        next_group_id: 1,
        paper_image_b64: doc.background.as_ref().map(|b| b64::encode(&b.encoded)),
        show_reg_marks: doc.page.registration.is_some(),
        reg_mark_inset_in: doc.page.registration.map_or(default_reg_inset(), |r| (r.inset_mm / MM_PER_IN) as f32),
        title: title.map(str::to_string),
        source_path: None,
    };
    for o in &doc.objects {
        let layer = doc.layer(o.layer);
        let visible = layer.is_none_or(|l| l.visible);
        let locked = layer.is_some_and(|l| l.locked);
        emit(o, Affine::IDENTITY, &mut Vec::new(), (visible, locked), &mut p);
    }
    p
}

fn inches(v: f64) -> f32 {
    (v / MM_PER_IN) as f32
}

fn rgba(c: Rgba) -> [u8; 4] {
    [c.r, c.g, c.b, c.a]
}

fn base_shape(kind: &str, o: &Object, bb: Rect, chain: &[u64], (visible, locked): (bool, bool)) -> CraftShape {
    CraftShape {
        kind: kind.into(),
        x: inches(bb.x0),
        y: inches(bb.y0),
        width: inches(bb.width()),
        height: inches(bb.height()),
        points: Vec::new(),
        text: String::new(),
        fill_color: o.style.fill.map_or([0; 4], rgba),
        stroke_color: Some(rgba(o.style.stroke)),
        stroke_width: inches(o.style.stroke_width_mm),
        is_closed: false,
        is_svg_path: false,
        group_id: chain.last().copied(),
        group_path: chain.to_vec(),
        is_visible: visible,
        is_locked: locked,
        name: Some(o.name.clone()),
        pixmap_b64: None,
        pixmap_width: 0,
        pixmap_height: 0,
    }
}

/// Inches with 4 decimals (2.5 µm), no exponent, no trailing zeros.
fn num(v: f64) -> String {
    let s = format!("{:.4}", v / MM_PER_IN);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

fn emit(o: &Object, parent: Affine, chain: &mut Vec<u64>, flags: (bool, bool), p: &mut CraftProject) {
    let m = parent * o.transform;
    let flags = (flags.0 && o.visible, flags.1 || o.locked);
    match &o.kind {
        ObjectKind::Group { children } => {
            let id = p.next_group_id;
            p.next_group_id += 1;
            p.group_names.insert(id, o.name.clone());
            chain.push(id);
            children.iter().for_each(|c| emit(c, m, chain, flags, p));
            chain.pop();
        }
        ObjectKind::Image(img) => {
            let mut s = base_shape("Rectangle", o, m.transform_rect_bbox(img.placement()), chain, flags);
            s.fill_color = [0; 4];
            s.stroke_color = None;
            s.stroke_width = 0.0;
            s.is_closed = true;
            s.pixmap_b64 = Some(b64::encode(&img.encoded));
            (s.pixmap_width, s.pixmap_height) = (img.width_px, img.height_px);
            p.shapes.push(s);
        }
        ObjectKind::Path { path, .. } | ObjectKind::Text { outlines: path, .. } => {
            let page = ops::transformed(path, m);
            let lines = flatten(&page, FLATTEN_MM);
            if lines.iter().all(|l| l.points.is_empty()) {
                return;
            }
            let mut s = base_shape("Freehand", o, page.bounding_box(), chain, flags);
            for l in lines.iter().filter(|l| !l.points.is_empty()) {
                for (i, q) in l.points.iter().enumerate() {
                    let sep = if s.text.is_empty() { "" } else { " " };
                    let _ = write!(s.text, "{sep}{} {} {}", if i == 0 { 'M' } else { 'L' }, num(q.x), num(q.y));
                    s.points.push([inches(q.x), inches(q.y)]);
                }
                if l.closed {
                    s.text.push_str(" Z");
                }
            }
            s.is_closed = lines.iter().all(|l| l.closed);
            s.is_svg_path = true;
            p.shapes.push(s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ss_geom::Shape;

    fn tiny_png() -> Vec<u8> {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, 3, 2);
        enc.set_color(png::ColorType::Rgba);
        enc.write_header().unwrap().write_image_data(&[200; 24]).unwrap();
        out
    }

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    /// A file as the companion writes it (its own test fixture plus every shape kind).
    fn companion_json() -> String {
        let png = b64::encode(&tiny_png());
        format!(
            r#"{{
  "version": 2,
  "app_name": "Studio to SVG",
  "mat_width_in": 8.5,
  "mat_height_in": 11.0,
  "shapes": [
    {{ "kind": "Rectangle", "x": 1.0, "y": 2.0, "width": 3.0, "height": 4.0, "points": [[0.0, 0.0], [1.0, 1.0]],
      "text": "", "fill_color": [255, 0, 128, 255], "stroke_color": [0, 0, 0, 255], "stroke_width": 0.01,
      "is_closed": true, "is_svg_path": false, "group_id": 1, "group_path": [1], "is_visible": true,
      "is_locked": false, "name": "Base Silhouette", "pixmap_b64": null, "pixmap_width": 0, "pixmap_height": 0 }},
    {{ "kind": "Ellipse", "x": 5.0, "y": 1.0, "width": 2.0, "height": 1.0, "points": [], "text": "",
      "fill_color": [0, 0, 0, 0], "group_id": 2, "group_path": [1, 2] }},
    {{ "kind": "Line", "x": 0.0, "y": 0.0, "width": 0.0, "height": 0.0, "points": [[0.5, 9.0], [2.5, 10.0]],
      "text": "", "fill_color": [0, 0, 0, 0], "stroke_color": [0, 0, 255, 255], "is_visible": false }},
    {{ "kind": "Freehand", "x": 4.0, "y": 6.0, "width": 2.0, "height": 2.0, "points": [], "fill_color": [0, 255, 0, 255],
      "text": "M4 6 L6 6 L6 8 L4 8 Z M4.5 6.5 L5.5 6.5 L5.5 7.5 L4.5 7.5 Z", "is_closed": true, "is_svg_path": true }},
    {{ "kind": "Freehand", "x": 0, "y": 0, "width": 1, "height": 1, "points": [[1, 8], [2, 8], [2, 9]],
      "text": "", "fill_color": [0, 0, 0, 0], "is_closed": false, "is_locked": true }},
    {{ "kind": "Rectangle", "x": 6.0, "y": 9.0, "width": 1.5, "height": 1.0, "points": [], "text": "",
      "fill_color": [0, 0, 0, 0], "name": "Print Artwork", "pixmap_b64": "{png}", "pixmap_width": 3, "pixmap_height": 2 }},
    {{ "kind": "Text", "x": 0.5, "y": 0.5, "width": 2.0, "height": 0.5, "points": [], "text": "Hello",
      "fill_color": [0, 0, 0, 255], "stroke_color": null }}
  ],
  "group_names": {{ "1": "Layer 1", "2": "Inner" }},
  "next_group_id": 5,
  "paper_image_b64": "{png}",
  "show_reg_marks": true,
  "title": "Roundtrip Design",
  "source_path": "/tmp/sample.studio"
}}"#
        )
    }

    #[test]
    fn imports_companion_file() {
        let doc = import(&companion_json(), None).unwrap();
        assert_eq!(doc.page.media, "letter");
        let reg = doc.page.registration.unwrap();
        assert!(close(reg.inset_mm, 6.35, 1e-6) && close(reg.length_mm, 5.08, 1e-9) && close(reg.thickness_mm, 0.508, 1e-9));
        let bg = doc.background.as_ref().unwrap();
        assert_eq!((bg.width_px, bg.height_px), (3, 2));
        assert!(close(bg.rect[2], 8.5 * 25.4, 1e-9));
        // Group 1 holds the rectangle and nested group 2 (the ellipse); then line, two freehands,
        // the image and the text.
        assert_eq!(doc.objects.len(), 6);
        let ObjectKind::Group { children } = &doc.objects[0].kind else { panic!("expected group") };
        assert_eq!(doc.objects[0].name, "Layer 1");
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].name, "Base Silhouette");
        let rb = children[0].bounds().unwrap();
        assert!(close(rb.x0, 25.4, 1e-4) && close(rb.y0, 50.8, 1e-4) && close(rb.width(), 76.2, 1e-4) && close(rb.height(), 101.6, 1e-4));
        assert_eq!(children[0].style.fill, Some(Rgba::rgb(255, 0, 128)));
        assert!(close(children[0].style.stroke_width_mm, 0.254, 1e-6));
        let ObjectKind::Group { children: inner } = &children[1].kind else { panic!("expected nested group") };
        assert_eq!(children[1].name, "Inner");
        let eb = inner[0].bounds().unwrap();
        assert!(close(eb.width(), 50.8, 1e-3) && close(eb.height(), 25.4, 1e-3));
        assert_eq!(inner[0].style.fill, None);

        let line = &doc.objects[1];
        assert!(!line.visible);
        assert_eq!(line.style.stroke, Rgba::rgb(0, 0, 255));
        let lb = line.bounds().unwrap();
        assert!(close(lb.x0, 12.7, 1e-4) && close(lb.x1, 63.5, 1e-4) && close(lb.y1, 254.0, 1e-4));

        let compound = &doc.objects[2];
        assert_eq!(compound.fill_rule(), FillRule::EvenOdd);
        assert_eq!(compound.cut_paths().iter().map(ops::subpaths).map(|s| s.len()).sum::<usize>(), 2);
        let open = &doc.objects[3];
        assert!(open.locked);
        assert!(!matches!(open.cut_paths()[0].elements().last(), Some(ss_geom::PathEl::ClosePath)));

        let img = &doc.objects[4];
        assert_eq!(img.name, "Print Artwork");
        assert!(img.cut_paths().is_empty() && img.cut == ss_doc::CutAction::NoCut);
        let ib = img.bounds().unwrap();
        assert!(close(ib.x0, 152.4, 1e-4) && close(ib.width(), 38.1, 1e-4));

        let ObjectKind::Text { data, .. } = &doc.objects[5].kind else { panic!("expected text") };
        assert_eq!(data.text, "Hello");
        assert_eq!(data.font_family, COMPANION_FONT);
    }

    #[test]
    fn text_is_laid_out_to_the_companion_height() {
        let fonts = FontLibrary::from_data([epaint_default_fonts::UBUNTU_LIGHT.to_vec()]);
        let doc = import(&companion_json(), Some(&fonts)).unwrap();
        let t = &doc.objects[5];
        let ObjectKind::Text { data, outlines } = &t.kind else { panic!("expected text") };
        assert!(!outlines.elements().is_empty());
        let laid = fonts.layout(data).unwrap();
        // The companion's text box is 0.5 in tall from ascent to descent.
        assert!(close(laid.height_mm, 12.7, 1e-6), "{}", laid.height_mm);
        let b = t.bounds().unwrap();
        assert!(b.x0 >= 12.7 - 1.0 && b.y0 >= 12.7 && b.y1 <= 12.7 + 12.7 + 1e-6, "{b:?}");
    }

    #[test]
    fn export_import_roundtrip() {
        let mut doc = Document::default();
        let l = doc.default_layer();
        let circle = ss_geom::kurbo::Circle::new((50.0, 60.0), 12.5).to_path(0.001);
        let mut c = Object::path("Circle", l, circle);
        c.style.fill = Some(Rgba::rgb(10, 20, 30));
        doc.objects.push(c);
        let mut g = Object::path("Pair", l, BezPath::new());
        let mut a = Object::path("A", l, Rect::new(0.0, 0.0, 10.0, 10.0).to_path(0.0));
        a.transform = Affine::translate((100.0, 100.0));
        g.kind = ObjectKind::Group { children: vec![a, Object::path("B", l, Rect::new(120.0, 100.0, 130.0, 110.0).to_path(0.0))] };
        g.transform = Affine::translate((5.0, 0.0));
        doc.objects.push(g);
        let mut img = Object::image("Art", l, ImageData::from_png(tiny_png(), Rect::new(0.0, 0.0, 30.0, 20.0)).unwrap());
        img.transform = Affine::translate((150.0, 30.0));
        doc.objects.push(img);
        doc.background = ImageData::from_png(tiny_png(), doc.page.rect());
        doc.page.registration = Some(RegMarks { inset_mm: 12.7, ..RegMarks::default() });

        let json = export(&doc, Some("t")).unwrap();
        // What the companion itself reads.
        let p: CraftProject = serde_json::from_str(&json).unwrap();
        assert_eq!((p.version, p.app_name.as_str(), p.title.as_deref()), (2, APP_NAME, Some("t")));
        assert!(close(f64::from(p.mat_width_in), 12.0, 1e-6) && p.show_reg_marks && close(f64::from(p.reg_mark_inset_in), 0.5, 1e-6));
        assert_eq!(p.shapes.len(), 4);
        assert_eq!(p.shapes[1].group_path, vec![1]);
        assert_eq!(p.group_names.get(&1).map(String::as_str), Some("Pair"));
        assert!(p.shapes[0].is_svg_path && p.shapes[0].is_closed && p.shapes[0].text.starts_with("M "));
        assert!(!p.shapes[0].text.contains(['C', 'Q', 'e']));
        assert_eq!(p.shapes[3].pixmap_width, 3);

        let back = import(&json, None).unwrap();
        assert_eq!(back.objects.len(), 3);
        let (a, b) = (doc.objects[0].bounds().unwrap(), back.objects[0].bounds().unwrap());
        assert!(a.x0.max(a.y0).max(a.x1).max(a.y1) > 0.0);
        // Flattening tolerance plus 4-decimal inch rounding.
        for (u, v) in [(a.x0, b.x0), (a.y0, b.y0), (a.x1, b.x1), (a.y1, b.y1)] {
            assert!(close(u, v, FLATTEN_MM + 0.003), "{a:?} vs {b:?}");
        }
        assert_eq!(back.objects[0].style.fill, Some(Rgba::rgb(10, 20, 30)));
        let ObjectKind::Group { children } = &back.objects[1].kind else { panic!("group lost") };
        assert_eq!(children.len(), 2);
        let ab = children[0].bounds().unwrap();
        assert!(close(ab.x0, 105.0, 2e-3) && close(ab.y0, 100.0, 2e-3), "{ab:?}");
        let ib = back.objects[2].bounds().unwrap();
        assert!(close(ib.x0, 150.0, 2e-3) && close(ib.width(), 30.0, 2e-3));
        assert!(back.background.is_some());
        assert!(close(back.page.registration.unwrap().inset_mm, 12.7, 1e-4));
    }

    #[test]
    fn minimal_and_unknown_kinds() {
        let json = r#"{"version":2,"app_name":"Craft Anything","mat_width_in":12.0,"mat_height_in":12.0,
            "shapes":[{"kind":"Star","x":0,"y":0,"width":1,"height":1,"points":[[0,0],[1,0],[1,1]],"text":"","fill_color":[0,0,0,0],"is_closed":true}],
            "group_names":{},"next_group_id":1}"#;
        let doc = import(json, None).unwrap();
        assert_eq!(doc.page.media, "12x12");
        assert!(doc.page.registration.is_none() && doc.background.is_none());
        assert_eq!(doc.objects.len(), 1);
        assert!(close(doc.objects[0].bounds().unwrap().width(), 25.4, 1e-9));
        assert!(import("{}", None).is_err());
    }

    #[test]
    fn invalid_path_data_uses_companion_reading() {
        let p = parse_path_data("M 1,1 2,1 2,2 bogus");
        assert_eq!(ops::subpaths(&p).len(), 1);
        assert!(close(p.bounding_box().width(), 25.4, 1e-9));
    }
}
