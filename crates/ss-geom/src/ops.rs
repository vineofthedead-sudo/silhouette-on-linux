//! Path utilities: bounds, hit testing, compound paths, transforms.

use kurbo::{Affine, BezPath, ParamCurveNearest, PathEl, Point, Rect, Shape};

use crate::FillRule;

/// Exact bounding box over several paths (curve extrema, not control points).
pub fn bounds<'a>(paths: impl IntoIterator<Item = &'a BezPath>) -> Option<Rect> {
    paths
        .into_iter()
        .filter(|p| !p.elements().is_empty())
        .map(|p| p.bounding_box())
        .reduce(|a, b| a.union(b))
}

/// Distance from `pt` to the nearest point on the outline.
pub fn distance_to_outline(path: &BezPath, pt: Point) -> f64 {
    path.segments()
        .map(|s| s.nearest(pt, 1e-6).distance_sq)
        .fold(f64::INFINITY, f64::min)
        .sqrt()
}

/// Inside test honouring the fill rule; open subpaths are treated as closed.
pub fn contains(path: &BezPath, pt: Point, fill: FillRule) -> bool {
    let w = path.winding(pt);
    match fill {
        FillRule::NonZero => w != 0,
        // Winding-number parity equals crossing-count parity.
        FillRule::EvenOdd => w % 2 != 0,
    }
}

/// Hit test used by the canvas: on the outline within `tol`, or inside a filled shape.
pub fn hit_test(path: &BezPath, pt: Point, tol: f64, filled: bool) -> bool {
    if filled && contains(path, pt, FillRule::EvenOdd) {
        return true;
    }
    if let Some(bb) = bounds([path])
        && !bb.inflate(tol, tol).contains(pt)
    {
        return false;
    }
    distance_to_outline(path, pt) <= tol
}

/// Splits a compound path into its subpaths (Silhouette "Release Compound Path").
pub fn subpaths(path: &BezPath) -> Vec<BezPath> {
    let mut out: Vec<BezPath> = Vec::new();
    for el in path.elements() {
        if matches!(el, PathEl::MoveTo(_)) || out.is_empty() {
            out.push(BezPath::new());
        }
        if let Some(last) = out.last_mut() {
            last.push(*el);
        }
    }
    out.retain(|p| p.elements().len() > 1);
    out
}

/// Joins paths into one compound path (Silhouette "Make Compound Path").
pub fn combine<'a>(paths: impl IntoIterator<Item = &'a BezPath>) -> BezPath {
    let mut out = BezPath::new();
    for p in paths {
        for el in p.elements() {
            out.push(*el);
        }
    }
    out
}

pub fn transformed(path: &BezPath, m: Affine) -> BezPath {
    let mut p = path.clone();
    p.apply_affine(m);
    p
}

/// Number of subpaths that are explicitly closed.
pub fn closed_subpaths(path: &BezPath) -> usize {
    path.elements().iter().filter(|e| matches!(e, PathEl::ClosePath)).count()
}

/// True when every coordinate is finite.
pub fn is_finite(path: &BezPath) -> bool {
    path.elements().iter().all(|el| match *el {
        PathEl::MoveTo(p) | PathEl::LineTo(p) => p.is_finite(),
        PathEl::QuadTo(a, b) => a.is_finite() && b.is_finite(),
        PathEl::CurveTo(a, b, c) => a.is_finite() && b.is_finite() && c.is_finite(),
        PathEl::ClosePath => true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Circle;

    #[test]
    fn hit_and_contains() {
        let mut ring = Rect::new(0.0, 0.0, 10.0, 10.0).to_path(0.0);
        for el in Rect::new(3.0, 3.0, 7.0, 7.0).to_path(0.0).elements() {
            ring.push(*el);
        }
        assert!(contains(&ring, Point::new(1.0, 1.0), FillRule::EvenOdd));
        assert!(!contains(&ring, Point::new(5.0, 5.0), FillRule::EvenOdd));
        assert!(hit_test(&ring, Point::new(10.05, 5.0), 0.1, false));
        assert!(!hit_test(&ring, Point::new(5.0, 5.0), 0.1, true));
        assert_eq!(subpaths(&ring).len(), 2);
        assert_eq!(subpaths(&combine(&subpaths(&ring))).len(), 2);
    }

    #[test]
    fn exact_bounds() {
        let c = Circle::new((5.0, 5.0), 2.0).to_path(1e-6);
        let b = bounds([&c]).unwrap();
        assert!((b.x0 - 3.0).abs() < 1e-6 && (b.x1 - 7.0).abs() < 1e-6);
        let t = transformed(&c, Affine::translate((10.0, 0.0)));
        assert!((bounds([&t]).unwrap().x0 - 13.0).abs() < 1e-6);
    }
}
