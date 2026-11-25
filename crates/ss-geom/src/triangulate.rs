//! Polygon triangulation for filled path rendering.
//!
//! Uses `i_overlay` to resolve self-intersections and pair outer boundaries with holes according
//! to the active [`FillRule`], bridges holes into their outer boundary, and ear-clips each simple
//! polygon.

use i_overlay::float::simplify::SimplifyShape;
use kurbo::{BezPath, Point};

use crate::FillRule;
use crate::flatten::to_contours;

/// Triangulates `path` into `(vertices, triangle_indices)` in the path's coordinate space.
pub fn triangulate_path(path: &BezPath, fill_rule: FillRule, tolerance: f64) -> (Vec<Point>, Vec<u32>) {
    let contours = to_contours(std::iter::once(path), tolerance);
    let raw: Vec<Vec<[f64; 2]>> =
        contours.into_iter().filter(|c| c.len() >= 3).map(|c| c.into_iter().map(|p| [p.x, p.y]).collect()).collect();
    if raw.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let shapes = raw.simplify_shape(fill_rule.to_overlay());
    let mut verts = Vec::new();
    let mut indices = Vec::new();
    for shape in shapes {
        if let Some((outer, holes)) = shape.split_first() {
            triangulate_shape(outer, holes, &mut verts, &mut indices);
        }
    }
    (verts, indices)
}

fn signed_area(pts: &[[f64; 2]]) -> f64 {
    let n = pts.len();
    if n < 3 {
        return 0.0;
    }
    let mut a = 0.0;
    for i in 0..n {
        let p = pts[i];
        let q = pts[(i + 1) % n];
        a += p[0] * q[1] - q[0] * p[1];
    }
    a * 0.5
}

fn ensure_ccw(pts: &mut [[f64; 2]], ccw: bool) {
    let a = signed_area(pts);
    if (ccw && a < 0.0) || (!ccw && a > 0.0) {
        pts.reverse();
    }
}

fn cross(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn same_pt(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
}

fn point_in_tri(p: [f64; 2], a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> bool {
    if same_pt(p, a) || same_pt(p, b) || same_pt(p, c) {
        return false;
    }
    let c1 = cross(a, b, p);
    let c2 = cross(b, c, p);
    let c3 = cross(c, a, p);
    c1 >= -1e-12 && c2 >= -1e-12 && c3 >= -1e-12
}

fn ray_seg_intersect_x(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> Option<f64> {
    if (a[1] > p[1]) == (b[1] > p[1]) {
        return None;
    }
    let t = (p[1] - a[1]) / (b[1] - a[1]);
    let x = a[0] + t * (b[0] - a[0]);
    (x >= p[0] - 1e-12).then_some(x)
}

fn bridge_hole(outer: Vec<[f64; 2]>, hole: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    if hole.len() < 3 || outer.len() < 3 {
        return outer;
    }
    // Pick the rightmost vertex of the hole.
    let (h_idx, &m) = hole.iter().enumerate().max_by(|a, b| a.1[0].total_cmp(&b.1[0])).unwrap();

    // Cast a ray +X from `m` to find the closest edge intersection on `outer`.
    let mut best_x = f64::INFINITY;
    let mut best_edge = 0;
    let mut hit_pt = m;
    for i in 0..outer.len() {
        let a = outer[i];
        let b = outer[(i + 1) % outer.len()];
        if let Some(x) = ray_seg_intersect_x(m, a, b)
            && x < best_x
        {
            best_x = x;
            best_edge = i;
            hit_pt = [x, m[1]];
        }
    }
    if !best_x.is_finite() {
        return outer;
    }

    // Candidate outer vertex: the endpoint of best_edge with larger x.
    let a_idx = best_edge;
    let b_idx = (best_edge + 1) % outer.len();
    let mut p_idx = if outer[a_idx][0] >= outer[b_idx][0] { a_idx } else { b_idx };
    let p = outer[p_idx];

    // Check if any reflex vertex of `outer` lies inside triangle (m, hit_pt, p).
    let (ta, tb, tc) = if cross(m, hit_pt, p) >= 0.0 { (m, hit_pt, p) } else { (m, p, hit_pt) };
    let mut best_dist2 = (p[0] - m[0]).hypot(p[1] - m[1]);
    for (i, &v) in outer.iter().enumerate() {
        if i == p_idx || i == a_idx || i == b_idx {
            continue;
        }
        if point_in_tri(v, ta, tb, tc) {
            let d = (v[0] - m[0]).hypot(v[1] - m[1]);
            if d < best_dist2 {
                best_dist2 = d;
                p_idx = i;
            }
        }
    }

    // Splice `hole` into `outer` at p_idx <-> h_idx.
    let mut merged = Vec::with_capacity(outer.len() + hole.len() + 2);
    merged.extend_from_slice(&outer[..=p_idx]);
    for k in 0..=hole.len() {
        merged.push(hole[(h_idx + k) % hole.len()]);
    }
    merged.push(outer[p_idx]);
    merged.extend_from_slice(&outer[p_idx + 1..]);
    merged
}

fn triangulate_shape(outer_in: &[[f64; 2]], holes_in: &[Vec<[f64; 2]>], verts: &mut Vec<Point>, indices: &mut Vec<u32>) {
    let mut poly = outer_in.to_vec();
    if poly.len() < 3 {
        return;
    }
    ensure_ccw(&mut poly, true);

    let mut holes: Vec<Vec<[f64; 2]>> = holes_in.iter().filter(|h| h.len() >= 3).cloned().collect();
    for h in &mut holes {
        ensure_ccw(h, false);
    }
    // Bridge holes from rightmost to leftmost so earlier bridges don't block later rays.
    holes.sort_by(|a, b| {
        let ax = a.iter().map(|p| p[0]).fold(f64::NEG_INFINITY, f64::max);
        let bx = b.iter().map(|p| p[0]).fold(f64::NEG_INFINITY, f64::max);
        bx.total_cmp(&ax)
    });
    for h in holes {
        poly = bridge_hole(poly, h);
    }

    let base = verts.len() as u32;
    verts.extend(poly.iter().map(|&[x, y]| Point::new(x, y)));
    let mut ring: Vec<usize> = (0..poly.len()).collect();

    // Ear clipping with safety guard against degenerate inputs.
    let mut guard = ring.len() * ring.len() + 16;
    let mut i = 0;
    while ring.len() > 3 && guard > 0 {
        guard -= 1;
        let n = ring.len();
        let prev = ring[(i + n - 1) % n];
        let curr = ring[i % n];
        let next = ring[(i + 1) % n];
        let (a, b, c) = (poly[prev], poly[curr], poly[next]);
        if cross(a, b, c) > 1e-12 {
            let contains_any = ring
                .iter()
                .copied()
                .filter(|&idx| idx != prev && idx != curr && idx != next)
                .any(|idx| point_in_tri(poly[idx], a, b, c));
            if !contains_any {
                indices.extend_from_slice(&[base + prev as u32, base + curr as u32, base + next as u32]);
                ring.remove(i % n);
                i = i.saturating_sub(1);
                continue;
            }
        }
        i = (i + 1) % n;
    }
    if ring.len() == 3 {
        indices.extend_from_slice(&[base + ring[0] as u32, base + ring[1] as u32, base + ring[2] as u32]);
    }
}

#[cfg(test)]
mod tests {
    use kurbo::Rect;
    use kurbo::Shape;

    use super::*;

    fn tri_area(verts: &[Point], indices: &[u32]) -> f64 {
        indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| {
                let (a, b, c) = (verts[t[0] as usize], verts[t[1] as usize], verts[t[2] as usize]);
                0.5 * ((b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)).abs()
            })
            .sum()
    }

    #[test]
    fn triangulates_square_and_hole() {
        let mut p = Rect::new(0.0, 0.0, 100.0, 100.0).to_path(0.01);
        p.extend(Rect::new(25.0, 25.0, 75.0, 75.0).to_path(0.01));
        let (v, idx) = triangulate_path(&p, FillRule::EvenOdd, 0.01);
        assert!(!idx.is_empty());
        let area = tri_area(&v, &idx);
        assert!((area - 7500.0).abs() < 1e-3, "expected 7500, got {area}");
    }

    #[test]
    fn triangulates_concave_l_shape() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((100.0, 0.0));
        p.line_to((100.0, 20.0));
        p.line_to((20.0, 20.0));
        p.line_to((20.0, 100.0));
        p.line_to((0.0, 100.0));
        p.close_path();
        let (v, idx) = triangulate_path(&p, FillRule::EvenOdd, 0.01);
        let area = tri_area(&v, &idx);
        assert!((area - 3600.0).abs() < 1e-3, "expected 3600, got {area}");
    }
}
