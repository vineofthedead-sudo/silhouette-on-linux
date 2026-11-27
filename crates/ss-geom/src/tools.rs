//! Active-vector-tool geometry: Knife slicing and Eraser subtraction.
//!
//! Closed shapes are flattened to polygons and processed with `i_overlay`; open paths are split
//! at their intersections with the cutter. Results are polygonal and refitted to cubics by the
//! caller (see `ss_doc::Editor`). Untouched shapes are reported as `None` so callers can leave
//! them alone.

use kurbo::{BezPath, Point, Vec2};

use crate::boolean::{self, BoolOp};
use crate::fit::FitOptions;
use crate::flatten::flatten;
use crate::{Contour, FillRule, Polygon};

/// Width of the gap a Knife cut leaves between the pieces, mm. Far below blade precision.
pub const KNIFE_KERF_MM: f64 = 0.02;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolGeometryError {
    InvalidStroke(&'static str),
}

impl std::fmt::Display for ToolGeometryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidStroke(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ToolGeometryError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BrushShape {
    #[default]
    Round,
    Square,
}

/// Snaps `end` to the nearest 45° direction from `start`, keeping the distance.
pub fn snap_angle(start: Point, end: Point) -> Point {
    let d = end - start;
    let step = std::f64::consts::FRAC_PI_4;
    let a = (d.y.atan2(d.x) / step).round() * step;
    start + Vec2::new(a.cos(), a.sin()) * d.hypot()
}

/// Builds the straight cut line described by the V5 manual. A click without a drag has no geometry.
pub fn knife_preview(start: Point, end: Point) -> Option<BezPath> {
    if !start.is_finite() || !end.is_finite() || start.distance(end) <= 1e-9 {
        return None;
    }
    let mut path = BezPath::new();
    path.move_to(start);
    path.line_to(end);
    Some(path)
}

/// Open stroke through `points`, dropping repeated adjacent samples and non-finite points.
/// Used for freehand Knife slices and the Eraser preview.
pub fn eraser_preview(points: &[Point]) -> Option<BezPath> {
    let pts = clean_points(points);
    (pts.len() >= 2).then(|| {
        let mut path = BezPath::new();
        path.move_to(pts[0]);
        for p in &pts[1..] {
            path.line_to(*p);
        }
        path
    })
}

fn clean_points(points: &[Point]) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::with_capacity(points.len());
    for &p in points.iter().filter(|p| p.is_finite()) {
        if out.last().is_none_or(|l| l.distance(p) > 1e-9) {
            out.push(p);
        }
    }
    out
}

fn ccw(mut c: Contour) -> Contour {
    if boolean::contour_area(&c) < 0.0 {
        c.reverse();
    }
    c
}

/// The cut line widened into a thin sliver polygon set.
fn kerf_polygons(cut: &[Point], half_width: f64) -> Result<Vec<Polygon>, ToolGeometryError> {
    let pts = clean_points(cut);
    if pts.len() < 2 {
        return Err(ToolGeometryError::InvalidStroke("Knife needs a drag with two distinct points"));
    }
    let quads: Vec<Contour> = pts
        .windows(2)
        .map(|w| {
            let d = (w[1] - w[0]).normalize();
            let n = Vec2::new(-d.y, d.x) * half_width;
            let (a, b) = (w[0] - d * half_width, w[1] + d * half_width);
            ccw(vec![a + n, b + n, b - n, a - n])
        })
        .collect();
    Ok(boolean::simplify(&quads, FillRule::NonZero))
}

fn cross(a: Vec2, b: Vec2) -> f64 {
    a.x * b.y - a.y * b.x
}

/// Parameters along `a`→`b` (exclusive of the ends) where it crosses any of `edges`.
fn crossings(a: Point, b: Point, edges: &[(Point, Point)]) -> Vec<f64> {
    let r = b - a;
    let mut ts = Vec::new();
    for &(c, d) in edges {
        let s = d - c;
        let denom = cross(r, s);
        if denom.abs() < 1e-14 {
            continue;
        }
        let t = cross(c - a, s) / denom;
        let u = cross(c - a, r) / denom;
        if t > 1e-9 && t < 1.0 - 1e-9 && (0.0..=1.0).contains(&u) {
            ts.push(t);
        }
    }
    ts.sort_by(f64::total_cmp);
    ts.dedup_by(|x, y| (*x - *y).abs() < 1e-9);
    ts
}

/// Splits an open polyline wherever it crosses `edges`.
fn split_polyline(points: &[Point], edges: &[(Point, Point)]) -> Vec<Vec<Point>> {
    let Some(&first) = points.first() else { return Vec::new() };
    let mut out = Vec::new();
    let mut cur = vec![first];
    for w in points.windows(2) {
        for t in crossings(w[0], w[1], edges) {
            let p = w[0].lerp(w[1], t);
            cur.push(p);
            out.push(std::mem::replace(&mut cur, vec![p]));
        }
        cur.push(w[1]);
    }
    out.push(cur);
    out.retain(|p| p.len() >= 2 && p.windows(2).any(|w| w[0].distance(w[1]) > 1e-9));
    out
}

fn open_path(points: &[Point]) -> BezPath {
    let mut p = BezPath::new();
    p.move_to(points[0]);
    for q in &points[1..] {
        p.line_to(*q);
    }
    p
}

fn polygon_path(poly: &Polygon) -> BezPath {
    boolean::polygons_to_path(std::slice::from_ref(poly), Some(FitOptions::default()))
}

fn edges_of(contours: &[Contour]) -> Vec<(Point, Point)> {
    contours
        .iter()
        .flat_map(|c| (0..c.len()).map(move |i| (c[i], c[(i + 1) % c.len()])))
        .collect()
}

fn inside(contours: &[Contour], p: Point) -> bool {
    let mut odd = false;
    for (a, b) in edges_of(contours) {
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            odd = !odd;
        }
    }
    odd
}

/// Slices `path` along `cut` (a straight segment or a freehand polyline).
///
/// Returns `Ok(None)` when the cut does not separate anything, otherwise the independent pieces:
/// every closed polygon (with its holes) and every open run. Cut edges of closed shapes are
/// separated by a [`KNIFE_KERF_MM`] gap.
pub fn knife_split(path: &BezPath, fill: FillRule, cut: &[Point], tolerance: f64) -> Result<Option<Vec<BezPath>>, ToolGeometryError> {
    let kerf = kerf_polygons(cut, KNIFE_KERF_MM / 2.0)?;
    let kerf_contours: Vec<Contour> = kerf.into_iter().flatten().collect();
    let cut_edges: Vec<(Point, Point)> = clean_points(cut).windows(2).map(|w| (w[0], w[1])).collect();

    let lines = flatten(path, tolerance);
    let closed: Vec<Contour> = lines.iter().filter(|l| l.closed && l.points.len() >= 3).map(|l| l.points.clone()).collect();
    let mut pieces = Vec::new();
    let mut split = false;

    if !closed.is_empty() {
        let before = boolean::simplify(&closed, fill);
        let after = boolean::boolean_contours(&closed, &kerf_contours, BoolOp::Difference, fill);
        let chosen = if after.len() > before.len() {
            split = true;
            after
        } else {
            before
        };
        pieces.extend(chosen.iter().map(polygon_path));
    }
    for l in lines.iter().filter(|l| !l.closed) {
        let runs = split_polyline(&l.points, &cut_edges);
        split |= runs.len() > 1;
        pieces.extend(runs.iter().map(|r| open_path(r)));
    }
    Ok(split.then_some(pieces))
}

fn convex_hull(mut pts: Vec<Point>) -> Contour {
    pts.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    pts.dedup_by(|a, b| a.distance(*b) < 1e-12);
    if pts.len() < 3 {
        return pts;
    }
    let turn = |o: Point, a: Point, b: Point| cross(a - o, b - o);
    let mut hull: Vec<Point> = Vec::with_capacity(pts.len() * 2);
    for pass in 0..2 {
        let start = hull.len();
        for &p in &pts {
            while hull.len() >= start + 2 && turn(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0 {
                hull.pop();
            }
            hull.push(p);
        }
        hull.pop();
        if pass == 0 {
            pts.reverse();
        }
    }
    hull
}

fn brush_outline(center: Point, radius: f64, shape: BrushShape) -> Vec<Point> {
    match shape {
        BrushShape::Square => vec![
            center + Vec2::new(-radius, -radius),
            center + Vec2::new(radius, -radius),
            center + Vec2::new(radius, radius),
            center + Vec2::new(-radius, radius),
        ],
        BrushShape::Round => {
            const SIDES: usize = 32;
            (0..SIDES)
                .map(|i| {
                    let a = std::f64::consts::TAU * i as f64 / SIDES as f64;
                    center + Vec2::new(a.cos(), a.sin()) * radius
                })
                .collect()
        }
    }
}

/// Area swept by a circular or square brush of half-size `radius` along `points`, as a union.
pub fn brush_polygons(points: &[Point], radius: f64, shape: BrushShape) -> Result<Vec<Polygon>, ToolGeometryError> {
    if !radius.is_finite() || radius <= 0.0 {
        return Err(ToolGeometryError::InvalidStroke("Eraser radius must be a positive finite measurement"));
    }
    let pts = clean_points(points);
    if pts.is_empty() {
        return Err(ToolGeometryError::InvalidStroke("Eraser needs at least one point"));
    }
    let contours: Vec<Contour> = if pts.len() == 1 {
        vec![ccw(brush_outline(pts[0], radius, shape))]
    } else {
        pts.windows(2)
            .map(|w| {
                let mut outline = brush_outline(w[0], radius, shape);
                outline.extend(brush_outline(w[1], radius, shape));
                convex_hull(outline)
            })
            .map(ccw)
            .collect()
    };
    Ok(boolean::simplify(&contours, FillRule::NonZero))
}

/// Subtracts the swept `brush` from `path`: closed shapes by polygon difference (keeping their
/// holes), open paths by dropping the runs inside the brush.
///
/// Returns `Ok(None)` when the brush touches nothing, otherwise the remaining pieces (possibly none).
pub fn erase(path: &BezPath, fill: FillRule, brush: &[Polygon], tolerance: f64) -> Result<Option<Vec<BezPath>>, ToolGeometryError> {
    let brush_contours: Vec<Contour> = brush.iter().flatten().cloned().collect();
    if brush_contours.is_empty() {
        return Err(ToolGeometryError::InvalidStroke("Eraser needs a drag with a brush area"));
    }
    let edges = edges_of(&brush_contours);
    let lines = flatten(path, tolerance);
    let closed: Vec<Contour> = lines.iter().filter(|l| l.closed && l.points.len() >= 3).map(|l| l.points.clone()).collect();
    let mut pieces = Vec::new();
    let mut touched = false;

    if !closed.is_empty() {
        let hit = boolean::boolean_contours(&closed, &brush_contours, BoolOp::Intersect, fill);
        if boolean::polygons_area(&hit) > 1e-9 {
            touched = true;
            let rest = boolean::boolean_contours(&closed, &brush_contours, BoolOp::Difference, fill);
            pieces.extend(rest.iter().map(polygon_path));
        } else {
            pieces.extend(boolean::simplify(&closed, fill).iter().map(polygon_path));
        }
    }
    for l in lines.iter().filter(|l| !l.closed) {
        let runs = split_polyline(&l.points, &edges);
        let total = runs.len();
        let kept: Vec<&Vec<Point>> = runs.iter().filter(|r| !inside(&brush_contours, r[0].midpoint(r[1]))).collect();
        touched |= kept.len() != total || total > 1;
        pieces.extend(kept.iter().map(|r| open_path(r)));
    }
    Ok(touched.then_some(pieces))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rect;
    use crate::Shape;

    const TOL: f64 = 0.01;

    fn rect_path(x0: f64, y0: f64, x1: f64, y1: f64) -> BezPath {
        Rect::new(x0, y0, x1, y1).to_path(0.0)
    }

    fn area(pieces: &[BezPath]) -> f64 {
        pieces
            .iter()
            .map(|p| boolean::polygons_area(&boolean::simplify(&crate::flatten::to_contours([p], TOL), FillRule::EvenOdd)))
            .sum()
    }

    #[test]
    fn previews_validate_and_normalize_drag_geometry() {
        let a = Point::new(1.0, 2.0);
        let b = Point::new(4.0, 6.0);
        assert!(knife_preview(a, a).is_none());
        assert_eq!(knife_preview(a, b).unwrap().elements().len(), 2);
        assert!(eraser_preview(&[a]).is_none());
        let stroke = eraser_preview(&[a, a, b, b]).unwrap();
        assert_eq!(stroke.elements().len(), 2);
    }

    #[test]
    fn angle_snapping_uses_45_degree_steps() {
        let s = Point::new(10.0, 10.0);
        let e = snap_angle(s, Point::new(20.0, 10.9));
        assert!((e.y - 10.0).abs() < 1e-9 && (e.x - s.x - Point::new(20.0, 10.9).distance(s)).abs() < 1e-9);
        let d = snap_angle(s, Point::new(14.0, 15.0));
        assert!(((d.x - s.x) - (d.y - s.y)).abs() < 1e-9);
    }

    #[test]
    fn knife_splits_a_square_in_two() {
        let sq = rect_path(0.0, 0.0, 10.0, 10.0);
        let pieces = knife_split(&sq, FillRule::EvenOdd, &[Point::new(5.0, -2.0), Point::new(5.0, 12.0)], TOL).unwrap().unwrap();
        assert_eq!(pieces.len(), 2);
        assert!((area(&pieces) - (100.0 - KNIFE_KERF_MM * 10.0)).abs() < 1e-3);
    }

    #[test]
    fn knife_that_stops_inside_or_misses_changes_nothing() {
        let sq = rect_path(0.0, 0.0, 10.0, 10.0);
        assert!(knife_split(&sq, FillRule::EvenOdd, &[Point::new(5.0, -2.0), Point::new(5.0, 5.0)], TOL).unwrap().is_none());
        assert!(knife_split(&sq, FillRule::EvenOdd, &[Point::new(20.0, -2.0), Point::new(20.0, 12.0)], TOL).unwrap().is_none());
        assert!(knife_split(&sq, FillRule::EvenOdd, &[Point::new(1.0, 1.0)], TOL).is_err());
    }

    #[test]
    fn knife_follows_a_freehand_slice() {
        // A zig-zag across an L-shaped slice still separates the square into two parts.
        let sq = rect_path(0.0, 0.0, 10.0, 10.0);
        let cut = [Point::new(5.0, -1.0), Point::new(5.0, 5.0), Point::new(11.0, 5.0)];
        let pieces = knife_split(&sq, FillRule::EvenOdd, &cut, TOL).unwrap().unwrap();
        assert_eq!(pieces.len(), 2);
    }

    #[test]
    fn knife_through_a_ring_yields_two_parts_with_a_hole_kept_whole_otherwise() {
        let mut ring = rect_path(0.0, 0.0, 10.0, 10.0);
        ring.extend(rect_path(3.0, 3.0, 7.0, 7.0));
        // One cut leaves the ring connected on the far side: still one piece.
        let one = knife_split(&ring, FillRule::EvenOdd, &[Point::new(5.0, -1.0), Point::new(5.0, 5.0)], TOL).unwrap();
        assert!(one.is_none());
        let two = knife_split(&ring, FillRule::EvenOdd, &[Point::new(5.0, -1.0), Point::new(5.0, 11.0)], TOL).unwrap().unwrap();
        assert_eq!(two.len(), 2);
        assert!((area(&two) - (100.0 - 16.0 - KNIFE_KERF_MM * 6.0)).abs() < 1e-2);
    }

    #[test]
    fn knife_splits_open_paths_at_the_crossing() {
        let mut line = BezPath::new();
        line.move_to((0.0, 5.0));
        line.line_to((10.0, 5.0));
        let pieces = knife_split(&line, FillRule::EvenOdd, &[Point::new(4.0, 0.0), Point::new(4.0, 10.0)], TOL).unwrap().unwrap();
        assert_eq!(pieces.len(), 2);
        let b = pieces[0].bounding_box();
        assert!((b.x1 - 4.0).abs() < 1e-9);
    }

    #[test]
    fn brush_shapes_cover_the_expected_area() {
        let round = brush_polygons(&[Point::new(0.0, 0.0)], 2.0, BrushShape::Round).unwrap();
        let a = boolean::polygons_area(&round);
        assert!((a - std::f64::consts::PI * 4.0).abs() < 0.2, "{a}");
        let square = brush_polygons(&[Point::new(0.0, 0.0), Point::new(10.0, 0.0)], 1.0, BrushShape::Square).unwrap();
        assert!((boolean::polygons_area(&square) - 24.0).abs() < 1e-6);
        let swept = brush_polygons(&[Point::new(0.0, 0.0), Point::new(10.0, 0.0), Point::new(10.0, 10.0)], 1.0, BrushShape::Round).unwrap();
        assert_eq!(swept.len(), 1);
        assert!(brush_polygons(&[Point::ZERO], 0.0, BrushShape::Round).is_err());
        assert!(brush_polygons(&[], 1.0, BrushShape::Round).is_err());
    }

    #[test]
    fn eraser_subtracts_a_bite() {
        let sq = rect_path(0.0, 0.0, 10.0, 10.0);
        let brush = brush_polygons(&[Point::new(10.0, 5.0)], 2.0, BrushShape::Square).unwrap();
        let out = erase(&sq, FillRule::EvenOdd, &brush, TOL).unwrap().unwrap();
        assert!((area(&out) - (100.0 - 2.0 * 4.0)).abs() < 1e-6);
    }

    #[test]
    fn eraser_keeps_holes_and_can_cut_a_shape_in_two() {
        let mut ring = rect_path(0.0, 0.0, 10.0, 10.0);
        ring.extend(rect_path(3.0, 3.0, 7.0, 7.0));
        // A brush well away from the hole leaves the hole intact.
        let bite = brush_polygons(&[Point::new(0.0, 1.0)], 1.0, BrushShape::Square).unwrap();
        let out = erase(&ring, FillRule::EvenOdd, &bite, TOL).unwrap().unwrap();
        assert!((area(&out) - (100.0 - 16.0 - 1.0 * 2.0)).abs() < 1e-6);
        // A stroke straight through the whole ring (including the hole) severs it.
        let sweep = brush_polygons(&[Point::new(5.0, -2.0), Point::new(5.0, 12.0)], 0.5, BrushShape::Square).unwrap();
        let out = erase(&ring, FillRule::EvenOdd, &sweep, TOL).unwrap().unwrap();
        assert_eq!(out.len(), 2);
        assert!((area(&out) - (100.0 - 16.0 - 1.0 * 6.0)).abs() < 1e-6);
    }

    #[test]
    fn eraser_can_remove_everything_or_nothing() {
        let sq = rect_path(0.0, 0.0, 2.0, 2.0);
        let big = brush_polygons(&[Point::new(1.0, 1.0)], 5.0, BrushShape::Round).unwrap();
        assert!(erase(&sq, FillRule::EvenOdd, &big, TOL).unwrap().unwrap().is_empty());
        let far = brush_polygons(&[Point::new(50.0, 50.0)], 1.0, BrushShape::Round).unwrap();
        assert!(erase(&sq, FillRule::EvenOdd, &far, TOL).unwrap().is_none());
        assert!(erase(&sq, FillRule::EvenOdd, &[], TOL).is_err());
    }

    #[test]
    fn eraser_trims_open_paths() {
        let mut line = BezPath::new();
        line.move_to((0.0, 0.0));
        line.line_to((10.0, 0.0));
        let brush = brush_polygons(&[Point::new(5.0, 0.0)], 1.0, BrushShape::Square).unwrap();
        let out = erase(&line, FillRule::EvenOdd, &brush, TOL).unwrap().unwrap();
        assert_eq!(out.len(), 2);
        assert!((out[0].bounding_box().x1 - 4.0).abs() < 1e-9 && (out[1].bounding_box().x0 - 6.0).abs() < 1e-9);
        let inside_all = brush_polygons(&[Point::new(5.0, 0.0)], 20.0, BrushShape::Square).unwrap();
        assert!(erase(&line, FillRule::EvenOdd, &inside_all, TOL).unwrap().unwrap().is_empty());
    }
}
