//! Cut order planning.
//!
//! A path must be cut before any path that encloses it: once the outer piece is free it can shift
//! on the mat and spoil the inner cuts. Among the paths that are ready, the nearest one is cut
//! next, entering closed paths at their nearest vertex and open paths at their nearer end.

use ss_geom::flatten::Polyline;
use ss_geom::{Point, Rect};

fn bbox(pts: &[Point]) -> Rect {
    pts.iter().fold(Rect::new(f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY), |r, p| {
        Rect::new(r.x0.min(p.x), r.y0.min(p.y), r.x1.max(p.x), r.y1.max(p.y))
    })
}

fn point_in_polygon(p: Point, poly: &[Point]) -> bool {
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn encloses(outer: &Polyline, outer_box: Rect, inner: &Polyline, inner_box: Rect) -> bool {
    outer.closed
        && outer.points.len() >= 3
        && outer_box.area() > inner_box.area()
        && outer_box.x0 <= inner_box.x0
        && outer_box.y0 <= inner_box.y0
        && outer_box.x1 >= inner_box.x1
        && outer_box.y1 >= inner_box.y1
        && sample(&inner.points).all(|p| point_in_polygon(*p, &outer.points))
}

/// Up to 16 evenly spaced points; enough to tell nesting from overlap on real designs while
/// keeping the containment pass fast on files with thousands of shapes.
fn sample(pts: &[Point]) -> impl Iterator<Item = &Point> {
    let step = pts.len().div_ceil(16).max(1);
    pts.iter().step_by(step)
}

fn box_distance(r: Rect, p: Point) -> f64 {
    let dx = (r.x0 - p.x).max(p.x - r.x1).max(0.0);
    let dy = (r.y0 - p.y).max(p.y - r.y1).max(0.0);
    dx.hypot(dy)
}

/// Orders polylines inside-first with nearest-neighbour travel from `start`.
pub fn order(lines: Vec<Polyline>, start: Point) -> Vec<Polyline> {
    let n = lines.len();
    let boxes: Vec<Rect> = lines.iter().map(|l| bbox(&l.points)).collect();
    // containers[i]: paths that enclose i; pending[j]: uncut paths inside j.
    let mut containers: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut pending = vec![0usize; n];
    for j in 0..n {
        if !lines[j].closed {
            continue;
        }
        for i in 0..n {
            if i != j && encloses(&lines[j], boxes[j], &lines[i], boxes[i]) {
                containers[i].push(j);
                pending[j] += 1;
            }
        }
    }
    let mut done = vec![false; n];
    let mut out = Vec::with_capacity(n);
    let mut pos = start;
    let mut lines: Vec<Option<Polyline>> = lines.into_iter().map(Some).collect();
    for _ in 0..n {
        // (distance, index, entry vertex or reversed flag)
        let mut best: Option<(f64, usize, usize)> = None;
        for i in (0..n).filter(|&i| !done[i] && pending[i] == 0) {
            if best.is_some_and(|(bd, _, _)| box_distance(boxes[i], pos) >= bd) {
                continue;
            }
            let l = lines[i].as_ref().expect("uncut line present");
            let (d, k) = if l.closed {
                l.points
                    .iter()
                    .enumerate()
                    .map(|(k, p)| (p.distance(pos), k))
                    .min_by(|a, b| a.0.total_cmp(&b.0))
                    .unwrap_or((f64::INFINITY, 0))
            } else {
                let (a, b) = (l.points[0].distance(pos), l.points[l.points.len() - 1].distance(pos));
                if b < a { (b, 1) } else { (a, 0) }
            };
            if best.is_none_or(|(bd, _, _)| d < bd) {
                best = Some((d, i, k));
            }
        }
        // Mutually enclosing paths (identical outlines) leave nothing ready; take the first left.
        let (_, i, k) = best.unwrap_or_else(|| (0.0, (0..n).find(|&i| !done[i]).expect("a path is left"), 0));
        let mut l = lines[i].take().expect("uncut line present");
        if l.closed {
            l.points.rotate_left(k);
        } else if k == 1 {
            l.points.reverse();
        }
        pos = if l.closed { l.points[0] } else { *l.points.last().expect("non-empty") };
        done[i] = true;
        for &j in &containers[i] {
            pending[j] -= 1;
        }
        out.push(l);
    }
    out
}
