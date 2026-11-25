//! SVG import (through `usvg`, which resolves styles, `use`, transforms and shapes) and export.

use std::fmt::Write as _;

use ss_doc::{Document, Object, Page, Rgba};
use ss_geom::{Affine, BezPath, FillRule, PathEl, Point};

/// `id` of the background `<image>` written by [`export`].
const BACKGROUND_ID: &str = "ss-background";

/// usvg works in CSS pixels (96 per inch).
const MM_PER_PX: f64 = 25.4 / 96.0;

#[derive(Debug, thiserror::Error)]
pub enum SvgError {
    #[error("invalid SVG: {0}")]
    Parse(#[from] usvg::Error),
}

#[derive(Debug, Clone, Default)]
pub struct SvgImportOptions {
    /// Load system fonts so `<text>` is converted to outlines.
    pub load_system_fonts: bool,
}

/// Imports an SVG into a document. The page size follows the SVG's physical size.
pub fn import(data: &[u8], opts: &SvgImportOptions) -> Result<Document, SvgError> {
    let mut o = usvg::Options::default();
    if opts.load_system_fonts {
        o.fontdb_mut().load_system_fonts();
    }
    let tree = usvg::Tree::from_data(data, &o)?;
    let mut doc = Document::default();
    let size = tree.size();
    doc.page = Page {
        width_mm: f64::from(size.width()) * MM_PER_PX,
        height_mm: f64::from(size.height()) * MM_PER_PX,
        media: "custom".into(),
        ..Page::default()
    };
    let layer = doc.default_layer();
    collect(tree.root(), layer, &mut doc.objects);
    if let Some(i) = doc.objects.iter().position(|o| o.name == BACKGROUND_ID && matches!(o.kind, ss_doc::ObjectKind::Image(_))) {
        let m = doc.objects[i].transform;
        let c = m.as_coeffs();
        if c[1].abs() < 1e-9 && c[2].abs() < 1e-9 && c[0] > 0.0 && c[3] > 0.0 {
            let ss_doc::ObjectKind::Image(mut img) = doc.objects.remove(i).kind else { unreachable!() };
            let r = m.transform_rect_bbox(img.placement());
            img.rect = [r.x0, r.y0, r.x1, r.y1];
            doc.background = Some(img);
        }
    }
    Ok(doc)
}

fn rgba(p: &usvg::Paint, opacity: f32) -> Option<Rgba> {
    match p {
        usvg::Paint::Color(c) => Some(Rgba { r: c.red, g: c.green, b: c.blue, a: (opacity.clamp(0.0, 1.0) * 255.0) as u8 }),
        _ => None,
    }
}

fn collect(g: &usvg::Group, layer: ss_doc::LayerId, out: &mut Vec<Object>) {
    for node in g.children() {
        match node {
            // Printed marks written by `export`; they must not come back as cut paths.
            usvg::Node::Group(c) if c.id() == "registration-marks" => {}
            usvg::Node::Group(c) => collect(c, layer, out),
            usvg::Node::Text(t) => collect(t.flattened(), layer, out),
            usvg::Node::Path(p) => {
                if !p.is_visible() {
                    continue;
                }
                let path = convert_path(p.data(), p.abs_transform());
                if path.elements().is_empty() {
                    continue;
                }
                let name = if p.id().is_empty() { "Path".to_string() } else { p.id().to_string() };
                let mut obj = Object::path(name, layer, path);
                if let Some(f) = p.fill() {
                    obj.style.fill = rgba(f.paint(), f.opacity().get());
                    if let ss_doc::ObjectKind::Path { fill_rule, .. } = &mut obj.kind {
                        *fill_rule = match f.rule() {
                            usvg::FillRule::EvenOdd => FillRule::EvenOdd,
                            usvg::FillRule::NonZero => FillRule::NonZero,
                        };
                    }
                }
                match p.stroke() {
                    Some(s) => {
                        if let Some(c) = rgba(s.paint(), s.opacity().get()) {
                            obj.style.stroke = c;
                        }
                        let t = p.abs_transform();
                        let scale = f64::from((t.sx * t.sy - t.kx * t.ky).abs()).sqrt();
                        obj.style.stroke_width_mm = f64::from(s.width().get()) * scale * MM_PER_PX;
                    }
                    None => {
                        if let Some(f) = obj.style.fill {
                            obj.style.stroke = f;
                        }
                    }
                }
                out.push(obj);
            }
            usvg::Node::Image(i) => {
                let usvg::ImageKind::PNG(bytes) = i.kind() else { continue };
                if !i.is_visible() {
                    continue;
                }
                let size = i.size();
                let rect = ss_geom::Rect::new(0.0, 0.0, f64::from(size.width()), f64::from(size.height()));
                let Some(img) = ss_doc::ImageData::from_png(bytes.to_vec(), rect) else { continue };
                // usvg moves an image's id onto the group it wraps the image in.
                let id = if i.id().is_empty() { g.id() } else { i.id() };
                let name = if id.is_empty() { "Image".to_string() } else { id.to_string() };
                let mut obj = Object::image(name, layer, img);
                obj.transform = Affine::scale(MM_PER_PX) * to_affine(i.abs_transform());
                out.push(obj);
            }
        }
    }
}

fn to_affine(t: usvg::Transform) -> Affine {
    Affine::new([t.sx, t.ky, t.kx, t.sy, t.tx, t.ty].map(f64::from))
}

fn convert_path(data: &usvg::tiny_skia_path::Path, t: usvg::Transform) -> BezPath {
    let s = Affine::scale(MM_PER_PX) * to_affine(t);
    let pt = |p: usvg::tiny_skia_path::Point| s * Point::new(f64::from(p.x), f64::from(p.y));
    let mut out = BezPath::new();
    for seg in data.segments() {
        use usvg::tiny_skia_path::PathSegment as S;
        match seg {
            S::MoveTo(p) => out.move_to(pt(p)),
            S::LineTo(p) => out.line_to(pt(p)),
            S::QuadTo(a, b) => out.quad_to(pt(a), pt(b)),
            S::CubicTo(a, b, c) => out.curve_to(pt(a), pt(b), pt(c)),
            S::Close => out.close_path(),
        }
    }
    out
}

/// Formats a number with at most 3 decimals and no trailing zeros (0.001 mm resolution).
pub fn fmt_num(v: f64) -> String {
    let r = (v * 1000.0).round() / 1000.0;
    let r = if r == 0.0 { 0.0 } else { r };
    let mut s = format!("{r:.3}");
    while s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    s
}

/// SVG path data in millimetres.
pub fn path_data(p: &BezPath) -> String {
    let mut d = String::new();
    let f = |q: Point| format!("{} {}", fmt_num(q.x), fmt_num(q.y));
    for el in p.elements() {
        if !d.is_empty() {
            d.push(' ');
        }
        match *el {
            PathEl::MoveTo(a) => d += &format!("M{}", f(a)),
            PathEl::LineTo(a) => d += &format!("L{}", f(a)),
            PathEl::QuadTo(a, b) => d += &format!("Q{} {}", f(a), f(b)),
            PathEl::CurveTo(a, b, c) => d += &format!("C{} {} {}", f(a), f(b), f(c)),
            PathEl::ClosePath => d.push('Z'),
        }
    }
    d
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn image_element(img: &ss_doc::ImageData, m: Affine, attr: &str) -> String {
    let r = img.placement();
    let c = m.as_coeffs().map(fmt_num_precise);
    format!(
        r#"<image {} x="{}" y="{}" width="{}" height="{}" preserveAspectRatio="none" transform="matrix({})" href="data:{};base64,{}"/>"#,
        attr,
        fmt_num(r.x0),
        fmt_num(r.y0),
        fmt_num(r.width()),
        fmt_num(r.height()),
        c.join(" "),
        img.mime(),
        ss_doc::b64::encode(&img.encoded)
    )
}

/// Transform coefficients need more precision than coordinates: a 2000 px image scaled into
/// 100 mm has a scale of 0.05.
fn fmt_num_precise(v: f64) -> String {
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

/// Exports visible objects as an SVG sized to the page, in millimetres. Images (and the
/// background) are embedded as `<image>`; registration marks, when enabled, are drawn as filled
/// black shapes so the sheet can be printed elsewhere.
pub fn export(doc: &Document) -> String {
    let (w, h) = (doc.page.width_mm, doc.page.height_mm);
    let mut s = String::new();
    let _ = writeln!(s, r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    let _ = writeln!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}mm" height="{}mm" viewBox="0 0 {} {}">"#,
        fmt_num(w),
        fmt_num(h),
        fmt_num(w),
        fmt_num(h)
    );
    if let Some(bg) = &doc.background {
        let _ = writeln!(s, "{}", image_element(bg, Affine::IDENTITY, &format!(r#"id="{BACKGROUND_ID}""#)));
    }
    for layer in &doc.layers {
        let objs: Vec<&Object> = doc.visible_objects().filter(|o| o.layer == layer.id).collect();
        if objs.is_empty() {
            continue;
        }
        let _ = writeln!(s, r#"<g id="{}">"#, esc(&layer.name));
        for o in objs {
            for (img, m) in o.page_images() {
                let _ = writeln!(s, "{}", image_element(img, m, &format!(r#"data-name="{}""#, esc(&o.name))));
            }
            let fill = o.style.fill.map_or("none".to_string(), Rgba::to_hex);
            let rule = if o.fill_rule() == FillRule::EvenOdd { r#" fill-rule="evenodd""# } else { "" };
            let stroke = if o.style.stroke.a == 0 { "none".to_string() } else { o.style.stroke.to_hex() };
            for p in o.cut_paths() {
                let _ = writeln!(
                    s,
                    r#"<path data-name="{}" d="{}" fill="{}"{} stroke="{}" stroke-width="{}"/>"#,
                    esc(&o.name),
                    path_data(&p),
                    fill,
                    rule,
                    stroke,
                    fmt_num(o.style.stroke_width_mm)
                );
            }
        }
        let _ = writeln!(s, "</g>");
    }
    if let Some(reg) = &doc.page.registration {
        let _ = writeln!(s, r##"<g id="registration-marks" fill="#000000" stroke="none">"##);
        for p in reg.outlines(&doc.page) {
            let _ = writeln!(s, r#"<path d="{}"/>"#, path_data(&p));
        }
        let _ = writeln!(s, "</g>");
    }
    s.push_str("</svg>\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use ss_geom::Shape;

    #[test]
    fn import_units_transforms_and_styles() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="50mm" viewBox="0 0 100 50">
            <g transform="translate(10 5)">
              <rect id="r" x="0" y="0" width="20" height="10" fill="#ff0000" stroke="#0000ff" stroke-width="0.5"/>
            </g>
            <circle cx="70" cy="25" r="10" fill="none" stroke="black"/>
            <path d="M0 0 L10 0 L10 10 Z M2 2 L8 2 L8 8 Z" fill-rule="evenodd"/>
        </svg>"##;
        let doc = import(svg, &SvgImportOptions::default()).unwrap();
        assert!((doc.page.width_mm - 100.0).abs() < 1e-6 && (doc.page.height_mm - 50.0).abs() < 1e-6);
        assert_eq!(doc.objects.len(), 3);
        let r = doc.objects[0].bounds().unwrap();
        assert!((r.x0 - 10.0).abs() < 1e-4 && (r.y0 - 5.0).abs() < 1e-4 && (r.width() - 20.0).abs() < 1e-4);
        assert_eq!(doc.objects[0].name, "r");
        assert_eq!(doc.objects[0].style.fill, Some(Rgba::rgb(255, 0, 0)));
        assert_eq!(doc.objects[0].style.stroke, Rgba::rgb(0, 0, 255));
        assert!((doc.objects[0].style.stroke_width_mm - 0.5).abs() < 1e-4);
        let c = doc.objects[1].bounds().unwrap();
        assert!((c.width() - 20.0).abs() < 1e-3);
        assert_eq!(doc.objects[2].fill_rule(), FillRule::EvenOdd);
    }

    #[test]
    fn export_then_import_roundtrip() {
        let mut doc = Document::default();
        let l = doc.default_layer();
        doc.objects.push(Object::path("Circle", l, ss_geom::kurbo::Circle::new((50.0, 60.0), 12.5).to_path(0.001)));
        let svg = export(&doc);
        assert!(svg.contains(r#"width="304.8mm""#));
        let back = import(svg.as_bytes(), &SvgImportOptions::default()).unwrap();
        assert_eq!(back.objects.len(), 1);
        let (a, b) = (doc.objects[0].bounds().unwrap(), back.objects[0].bounds().unwrap());
        assert!((a.x0 - b.x0).abs() < 0.002 && (a.y1 - b.y1).abs() < 0.002);
    }

    fn tiny_png() -> Vec<u8> {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, 2, 1);
        enc.set_color(png::ColorType::Rgba);
        enc.write_header().unwrap().write_image_data(&[255, 0, 0, 255, 0, 0, 255, 255]).unwrap();
        out
    }

    #[test]
    fn images_background_and_marks_roundtrip() {
        let mut doc = Document::default();
        let l = doc.default_layer();
        let img = ss_doc::ImageData::from_png(tiny_png(), ss_geom::Rect::new(0.0, 0.0, 40.0, 20.0)).unwrap();
        let mut o = Object::image("Print Artwork", l, img.clone());
        o.transform = Affine::translate((30.0, 50.0));
        doc.objects.push(o);
        doc.objects.push(Object::path("Square", l, ss_geom::Rect::new(10.0, 10.0, 20.0, 20.0).to_path(0.0)));
        let mut bg = img;
        bg.rect = [0.0, 0.0, doc.page.width_mm, doc.page.height_mm];
        doc.background = Some(bg);
        doc.page.registration = Some(ss_doc::RegMarks::default());
        let svg = export(&doc);
        // The image is embedded, never written as a rectangle path.
        assert_eq!(svg.matches("<image").count(), 2);
        assert_eq!(svg.matches("data-name=\"Print Artwork\" d=").count(), 0);
        let back = import(svg.as_bytes(), &SvgImportOptions::default()).unwrap();
        // Marks are skipped, the background is restored, the image keeps its placement.
        assert_eq!(back.objects.len(), 2, "{:?}", back.objects.iter().map(|o| &o.name).collect::<Vec<_>>());
        let bg = back.background.as_ref().expect("background");
        assert!((bg.rect[2] - doc.page.width_mm).abs() < 1e-3);
        let im = back.objects.iter().find(|o| matches!(o.kind, ss_doc::ObjectKind::Image(_))).unwrap();
        let b = im.bounds().unwrap();
        assert!((b.x0 - 30.0).abs() < 1e-3 && (b.y0 - 50.0).abs() < 1e-3 && (b.width() - 40.0).abs() < 1e-3, "{b:?}");
        assert!(im.cut_paths().is_empty());
    }

    #[test]
    fn number_format() {
        assert_eq!(fmt_num(1.0), "1");
        assert_eq!(fmt_num(-0.0004), "0");
        assert_eq!(fmt_num(618.5136), "618.514");
        assert_eq!(fmt_num(-33.55), "-33.55");
    }
}
