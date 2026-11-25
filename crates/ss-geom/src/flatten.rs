//! Curve flattening (Bezier → polylines), the `AsLinear` step of the original engine.

use kurbo::{BezPath, PathEl, Point};

use crate::Contour;

const DUP_EPS: f64 = 1e-9;

#[derive(Debug, Clone, PartialEq)]
pub struct Polyline {
    pub points: Vec<Point>,
    pub closed: bool,
}

impl Polyline {
    pub fn len_mm(&self) -> f64 {
        let mut l: f64 = self.points.windows(2).map(|w| w[0].distance(w[1])).sum();
        if self.closed && self.points.len() > 1 {
            l += self.points[self.points.len() - 1].distance(self.points[0]);
        }
        l
    }
}

/// Flattens every subpath of `path` into a polyline with maximum deviation `tolerance`.
pub fn flatten(path: &BezPath, tolerance: f64) -> Vec<Polyline> {
    let mut out = Vec::new();
    let mut cur: Option<Polyline> = None;
    let mut last_start = Point::ZERO;
    kurbo::flatten(path.iter(), tolerance.max(1e-6), |el| match el {
        PathEl::MoveTo(p) => {
            if let Some(c) = cur.take() {
                finish(c, &mut out);
            }
            last_start = p;
            cur = Some(Polyline { points: vec![p], closed: false });
        }
        PathEl::LineTo(p) => {
            let c = cur.get_or_insert_with(|| Polyline { points: vec![last_start], closed: false });
            if c.points.last().is_none_or(|q| q.distance_squared(p) > DUP_EPS) {
                c.points.push(p);
            }
        }
        PathEl::ClosePath => {
            if let Some(mut c) = cur.take() {
                c.closed = true;
                finish(c, &mut out);
            }
        }
        // kurbo::flatten only emits MoveTo/LineTo/ClosePath.
        PathEl::QuadTo(..) | PathEl::CurveTo(..) => {}
    });
    if let Some(c) = cur.take() {
        finish(c, &mut out);
    }
    out
}

fn finish(mut c: Polyline, out: &mut Vec<Polyline>) {
    if c.closed && c.points.len() > 1 {
        let (first, last) = (c.points[0], c.points[c.points.len() - 1]);
        if first.distance_squared(last) <= DUP_EPS {
            c.points.pop();
        }
    }
    if c.points.len() >= 2 {
        out.push(c);
    }
}

/// Flattens paths into polygon contours. Open subpaths are closed implicitly, as the cutter
/// boolean tools do; degenerate contours (< 3 points) are dropped.
pub fn to_contours<'a>(paths: impl IntoIterator<Item = &'a BezPath>, tolerance: f64) -> Vec<Contour> {
    paths
        .into_iter()
        .flat_map(|p| flatten(p, tolerance))
        .filter(|pl| pl.points.len() >= 3)
        .map(|pl| pl.points)
        .collect()
}

/// Builds a path of straight segments from polylines.
pub fn polylines_to_path(lines: &[Polyline]) -> BezPath {
    let mut p = BezPath::new();
    for l in lines {
        append_polyline(&mut p, &l.points, l.closed);
    }
    p
}

pub fn contours_to_path<'a>(contours: impl IntoIterator<Item = &'a Contour>) -> BezPath {
    let mut p = BezPath::new();
    for c in contours {
        append_polyline(&mut p, c, true);
    }
    p
}

pub(crate) fn append_polyline(p: &mut BezPath, pts: &[Point], closed: bool) {
    let Some((&first, rest)) = pts.split_first() else { return };
    p.move_to(first);
    for &q in rest {
        p.line_to(q);
    }
    if closed {
        p.close_path();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::{Circle, Shape};

    #[test]
    fn circle_flatten_within_tolerance() {
        let c = Circle::new((10.0, 10.0), 5.0).to_path(1e-9);
        let tol = 0.01;
        let lines = flatten(&c, tol);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].closed);
        for p in &lines[0].points {
            let r = p.distance(Point::new(10.0, 10.0));
            assert!((r - 5.0).abs() <= tol * 1.5, "r = {r}");
        }
        let perim = lines[0].len_mm();
        assert!((perim - std::f64::consts::TAU * 5.0).abs() < 0.05);
    }

    #[test]
    fn open_and_multiple_subpaths() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((10.0, 0.0));
        p.move_to((0.0, 5.0));
        p.line_to((10.0, 5.0));
        p.line_to((10.0, 10.0));
        p.close_path();
        let lines = flatten(&p, 0.01);
        assert_eq!(lines.len(), 2);
        assert!(!lines[0].closed);
        assert!(lines[1].closed);
        assert_eq!(to_contours([&p], 0.01).len(), 1);
    }
}
