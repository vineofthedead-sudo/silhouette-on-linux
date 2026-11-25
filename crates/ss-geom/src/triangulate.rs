//! Polygon triangulation for filled path rendering.
//!
//! Uses `i_overlay` to resolve self-intersections and pair outer boundaries with holes according
//! to the active [`FillRule`], then `earcutr` to tessellate each polygon with its holes.

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

fn triangulate_shape(outer_in: &[[f64; 2]], holes_in: &[Vec<[f64; 2]>], verts: &mut Vec<Point>, indices: &mut Vec<u32>) {
    if outer_in.len() < 3 {
        return;
    }
    let mut coordinates: Vec<f64> = outer_in.iter().flatten().copied().collect();
    let mut holes = Vec::with_capacity(holes_in.len());
    for hole in holes_in.iter().filter(|h| h.len() >= 3) {
        holes.push(coordinates.len() / 2);
        coordinates.extend(hole.iter().flatten().copied());
    }
    // The input has been simplified into non-self-intersecting polygons. Hole offsets are
    // vertex offsets (not coordinate offsets), and winding is handled by earcut.
    let triangles = earcutr::earcut(&coordinates, &holes, 2).expect("valid simplified 2D polygon");
    let base = verts.len() as u32;
    verts.extend(coordinates.as_chunks::<2>().0.iter().map(|&[x, y]| Point::new(x, y)));
    indices.extend(triangles.into_iter().map(|i| base + i as u32));
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
    fn triangulates_many_aligned_holes() {
        let mut p = Rect::new(-50.0, -50.0, 50.0, 50.0).to_path(0.01);
        for row in 0..8 {
            for col in 0..8 {
                let (x, y) = (-45.0 + f64::from(col) * 12.0, -45.0 + f64::from(row) * 12.0);
                p.extend(Rect::new(x, y, x + 6.0, y + 6.0).to_path(0.01));
            }
        }
        let (v, idx) = triangulate_path(&p, FillRule::EvenOdd, 0.01);
        let area = tri_area(&v, &idx);
        assert!((area - 7696.0).abs() < 1e-3, "expected 7696, got {area}");
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
