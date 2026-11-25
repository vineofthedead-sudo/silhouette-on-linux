//! DXF import (LINE, LWPOLYLINE, POLYLINE, CIRCLE, ARC, ELLIPSE, SPLINE) and export (LWPOLYLINE).
//!
//! DXF is y-up; documents are y-down. Imports flip y and move the drawing so its top-left corner
//! sits at the page origin, which is how the original places DXF imports.

use std::io::Cursor;

use dxf::entities::{Entity, EntityType, LwPolyline};
use dxf::enums::Units;
use dxf::{Drawing, LwPolylineVertex};
use ss_doc::{Document, Object};
use ss_geom::flatten::flatten;
use ss_geom::kurbo::{Arc, Ellipse, Shape};
use ss_geom::{Affine, BezPath, Point, Vec2, ops};

#[derive(Debug, thiserror::Error)]
pub enum DxfError {
    #[error("invalid DXF: {0}")]
    Parse(String),
}

fn unit_scale(u: Units) -> f64 {
    match u {
        Units::Inches => 25.4,
        Units::Feet => 304.8,
        Units::Centimeters => 10.0,
        Units::Meters => 1000.0,
        _ => 1.0,
    }
}

/// Adds an arc from `a` to `b` with DXF bulge (tan of a quarter of the included angle).
fn bulge_to(p: &mut BezPath, a: Point, b: Point, bulge: f64) {
    if bulge.abs() < 1e-12 {
        p.line_to(b);
        return;
    }
    let chord = b - a;
    let len = chord.hypot();
    if len < 1e-12 {
        return;
    }
    let theta = 4.0 * bulge.atan();
    let r = len / (2.0 * (theta / 2.0).sin());
    let mid = a.midpoint(b);
    let n = Vec2::new(-chord.y, chord.x) / len;
    let h = r * (theta / 2.0).cos();
    let c = mid + n * h;
    let start = (a - c).atan2();
    let arc = Arc::new(c, Vec2::new(r.abs(), r.abs()), start, theta, 0.0);
    arc.to_cubic_beziers(0.001, |p1, p2, p3| p.curve_to(p1, p2, p3));
}

fn bspline_points(degree: usize, ctrl: &[Point], knots: &[f64], samples: usize) -> Vec<Point> {
    let n = ctrl.len();
    if n == 0 || knots.len() < n + degree + 1 {
        return ctrl.to_vec();
    }
    let (t0, t1) = (knots[degree], knots[n]);
    (0..=samples)
        .map(|i| {
            let t = (t0 + (t1 - t0) * i as f64 / samples as f64).min(t1 - 1e-12).max(t0);
            let k = (degree..n).rfind(|&k| knots[k] <= t).unwrap_or(degree);
            // de Boor
            let mut d: Vec<Point> = (0..=degree).map(|j| ctrl[j + k - degree]).collect();
            for r in 1..=degree {
                for j in (r..=degree).rev() {
                    let idx = j + k - degree;
                    let denom = knots[idx + degree + 1 - r] - knots[idx];
                    let alpha = if denom.abs() < 1e-12 { 0.0 } else { (t - knots[idx]) / denom };
                    d[j] = d[j - 1].lerp(d[j], alpha);
                }
            }
            d[degree]
        })
        .collect()
}

fn entity_path(e: &Entity) -> Option<BezPath> {
    let pt = |p: &dxf::Point| Point::new(p.x, p.y);
    let mut out = BezPath::new();
    match &e.specific {
        EntityType::Line(l) => {
            out.move_to(pt(&l.p1));
            out.line_to(pt(&l.p2));
        }
        EntityType::LwPolyline(l) => {
            let v = &l.vertices;
            let first = v.first()?;
            out.move_to(Point::new(first.x, first.y));
            let n = v.len();
            let segs = if l.is_closed() { n } else { n.saturating_sub(1) };
            for i in 0..segs {
                let (a, b) = (&v[i], &v[(i + 1) % n]);
                bulge_to(&mut out, Point::new(a.x, a.y), Point::new(b.x, b.y), a.bulge);
            }
            if l.is_closed() {
                out.close_path();
            }
        }
        EntityType::Polyline(l) => {
            let v: Vec<_> = l.vertices().collect();
            let first = v.first()?;
            out.move_to(pt(&first.location));
            let n = v.len();
            let segs = if l.is_closed() { n } else { n.saturating_sub(1) };
            for i in 0..segs {
                let (a, b) = (v[i], v[(i + 1) % n]);
                bulge_to(&mut out, pt(&a.location), pt(&b.location), a.bulge);
            }
            if l.is_closed() {
                out.close_path();
            }
        }
        EntityType::Circle(c) => {
            out = ss_geom::kurbo::Circle::new(pt(&c.center), c.radius).to_path(0.001);
        }
        EntityType::Arc(a) => {
            let mut sweep = (a.end_angle - a.start_angle).to_radians();
            if sweep <= 0.0 {
                sweep += std::f64::consts::TAU;
            }
            let arc = Arc::new(pt(&a.center), Vec2::new(a.radius, a.radius), a.start_angle.to_radians(), sweep, 0.0);
            out.move_to(arc.center + Vec2::from_angle(arc.start_angle) * a.radius);
            arc.to_cubic_beziers(0.001, |p1, p2, p3| out.curve_to(p1, p2, p3));
        }
        EntityType::Ellipse(el) => {
            let major = Vec2::new(el.major_axis.x, el.major_axis.y);
            let rx = major.hypot();
            let rot = major.atan2();
            let mut sweep = el.end_parameter - el.start_parameter;
            if sweep <= 0.0 {
                sweep += std::f64::consts::TAU;
            }
            let ellipse = Ellipse::new(pt(&el.center), Vec2::new(rx, rx * el.minor_axis_ratio), rot);
            if (sweep - std::f64::consts::TAU).abs() < 1e-9 {
                out = ellipse.to_path(0.001);
            } else {
                let arc = Arc::new(pt(&el.center), ellipse.radii(), el.start_parameter, sweep, rot);
                let start = arc.path_elements(0.001).next();
                if let Some(ss_geom::PathEl::MoveTo(p)) = start {
                    out.move_to(p);
                }
                arc.to_cubic_beziers(0.001, |p1, p2, p3| out.curve_to(p1, p2, p3));
            }
        }
        EntityType::Spline(s) => {
            let ctrl: Vec<Point> = s.control_points.iter().map(pt).collect();
            let pts = if ctrl.len() >= 2 {
                bspline_points(s.degree_of_curve.max(1) as usize, &ctrl, &s.knot_values, ctrl.len() * 16)
            } else {
                s.fit_points.iter().map(pt).collect()
            };
            let (first, rest) = pts.split_first()?;
            out.move_to(*first);
            for q in rest {
                out.line_to(*q);
            }
            if s.is_closed() {
                out.close_path();
            }
        }
        _ => return None,
    }
    (!out.elements().is_empty()).then_some(out)
}

pub fn import(data: &[u8]) -> Result<Document, DxfError> {
    let drawing = Drawing::load(&mut Cursor::new(data)).map_err(|e| DxfError::Parse(e.to_string()))?;
    let scale = unit_scale(drawing.header.default_drawing_units);
    let mut paths: Vec<(String, BezPath)> = drawing
        .entities()
        .filter_map(|e| Some((e.common.layer.clone(), entity_path(e)?)))
        .collect();
    let flip = Affine::scale_non_uniform(scale, -scale);
    for (_, p) in &mut paths {
        p.apply_affine(flip);
    }
    let mut doc = Document::default();
    if let Some(b) = ops::bounds(paths.iter().map(|(_, p)| p)) {
        let shift = Affine::translate(Vec2::new(-b.x0, -b.y0));
        for (_, p) in &mut paths {
            p.apply_affine(shift);
        }
    }
    let layer = doc.default_layer();
    for (name, p) in paths {
        doc.objects.push(Object::path(if name.is_empty() { "DXF".into() } else { name }, layer, p));
    }
    Ok(doc)
}

/// Exports visible objects as LWPOLYLINEs in millimetres, y flipped to DXF's y-up convention.
pub fn export(doc: &Document, tolerance: f64) -> Result<Vec<u8>, DxfError> {
    let mut drawing = Drawing::new();
    // LWPOLYLINE needs R14+; R2000 also carries $INSUNITS.
    drawing.header.version = dxf::enums::AcadVersion::R2000;
    drawing.header.default_drawing_units = Units::Millimeters;
    let h = doc.page.height_mm;
    for o in doc.visible_objects() {
        for p in o.cut_paths() {
            for pl in flatten(&p, tolerance) {
                let mut lw = LwPolyline::default();
                lw.set_is_closed(pl.closed);
                lw.vertices = pl
                    .points
                    .iter()
                    .map(|q| LwPolylineVertex { x: q.x, y: h - q.y, ..Default::default() })
                    .collect();
                let mut e = Entity::new(EntityType::LwPolyline(lw));
                e.common.layer = o.name.clone();
                drawing.add_entity(e);
            }
        }
    }
    let mut buf = Vec::new();
    drawing.save(&mut buf).map_err(|e| DxfError::Parse(e.to_string()))?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_import_roundtrip() {
        let mut doc = Document::default();
        let l = doc.default_layer();
        doc.objects.push(Object::path("sq", l, ss_geom::Rect::new(10.0, 20.0, 40.0, 35.0).to_path(0.0)));
        doc.objects.push(Object::path("c", l, ss_geom::kurbo::Circle::new((80.0, 50.0), 10.0).to_path(0.001)));
        let bytes = export(&doc, 0.005).unwrap();
        let back = import(&bytes).unwrap();
        assert_eq!(back.objects.len(), 2);
        let b = back.bounds().unwrap();
        // Imported drawings are moved to the origin; size must be preserved.
        assert!((b.width() - 80.0).abs() < 0.01 && (b.height() - 40.0).abs() < 0.01, "{b:?}");
        let sq = back.objects[0].bounds().unwrap();
        assert!((sq.width() - 30.0).abs() < 1e-6 && (sq.height() - 15.0).abs() < 1e-6);
        // y flip: the square is above the circle in the document and must stay above.
        assert!(sq.y0 < back.objects[1].bounds().unwrap().y0 + 5.0);
    }

    #[test]
    fn bulge_semicircle() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        bulge_to(&mut p, Point::new(0.0, 0.0), Point::new(10.0, 0.0), 1.0);
        let b = p.bounding_box();
        assert!((b.height() - 5.0).abs() < 1e-3 && (b.width() - 10.0).abs() < 1e-3);
    }

    #[test]
    fn bspline_endpoints_clamped() {
        let ctrl = [Point::new(0.0, 0.0), Point::new(5.0, 10.0), Point::new(10.0, 0.0)];
        let knots = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let pts = bspline_points(2, &ctrl, &knots, 10);
        assert!(pts[0].distance(ctrl[0]) < 1e-9);
        assert!(pts[10].distance(ctrl[2]) < 1e-6);
        assert!((pts[5].y - 5.0).abs() < 1e-9);
    }
}
