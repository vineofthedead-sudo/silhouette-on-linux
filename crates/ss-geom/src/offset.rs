//! Offset / inset (Silhouette "Offset" panel) and stroking of open paths.

use i_overlay::mesh::float::outline::offset::OutlineOffset;
use i_overlay::mesh::float::stroke::offset::StrokeOffset;
use i_overlay::mesh::float::style::{LineCap, LineJoin, OutlineStyle, StrokeStyle};
use kurbo::{BezPath, Point};

use crate::boolean::simplify;
use crate::flatten::{flatten, to_contours};
use crate::{FillRule, Polygon};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Join {
    /// Silhouette "Round" corners.
    #[default]
    Round,
    /// Silhouette "Corner" (sharp) corners; very sharp spikes are clipped.
    Miter,
    Bevel,
}

fn arc_step(distance: f64, tolerance: f64) -> f64 {
    // Max segment length over radius for a chord sagitta of `tolerance`.
    (8.0 * tolerance / distance.abs().max(1e-6)).sqrt().clamp(0.02, 0.5)
}

fn join_for(join: Join, distance: f64, tolerance: f64) -> LineJoin<f64> {
    match join {
        Join::Round => LineJoin::Round(arc_step(distance, tolerance)),
        Join::Miter => LineJoin::Miter(std::f64::consts::PI / 18.0),
        Join::Bevel => LineJoin::Bevel,
    }
}

fn to_polys(shapes: Vec<Vec<Vec<[f64; 2]>>>) -> Vec<Polygon> {
    shapes
        .into_iter()
        .map(|s| s.into_iter().map(|c| c.into_iter().map(|[x, y]| Point::new(x, y)).collect()).collect())
        .collect()
}

/// Offsets closed shapes by `distance` mm: positive grows (offset), negative shrinks (inset).
/// Open subpaths are closed implicitly.
pub fn offset(paths: &[BezPath], distance: f64, join: Join, fill: FillRule, tolerance: f64) -> Vec<Polygon> {
    let polys = simplify(&to_contours(paths, tolerance), fill);
    if distance == 0.0 || polys.is_empty() {
        return polys;
    }
    let raw: Vec<Vec<Vec<[f64; 2]>>> =
        polys.iter().map(|p| p.iter().map(|c| c.iter().map(|q| [q.x, q.y]).collect()).collect()).collect();
    let style = OutlineStyle::new(distance).line_join(join_for(join, distance, tolerance));
    to_polys(raw.outline(&style))
}

/// Outlines open strokes with total width `2 * distance` and round caps ("line offset" of an
/// open path).
pub fn stroke_open(paths: &[BezPath], distance: f64, join: Join, tolerance: f64) -> Vec<Polygon> {
    let d = distance.abs();
    if d == 0.0 {
        return Vec::new();
    }
    let step = arc_step(d, tolerance);
    let mut out = Vec::new();
    for p in paths {
        for pl in flatten(p, tolerance) {
            let pts: Vec<[f64; 2]> = pl.points.iter().map(|q| [q.x, q.y]).collect();
            let style = StrokeStyle::new(2.0 * d)
                .line_join(join_for(join, d, tolerance))
                .start_cap(LineCap::Round(step))
                .end_cap(LineCap::Round(step));
            out.extend(to_polys(pts.stroke(style, pl.closed)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boolean::polygons_area;
    use kurbo::{Rect, Shape};

    fn sq() -> BezPath {
        Rect::new(0.0, 0.0, 10.0, 10.0).to_path(0.0)
    }

    #[test]
    fn outward_offsets() {
        let miter = polygons_area(&offset(&[sq()], 1.0, Join::Miter, FillRule::EvenOdd, 0.001));
        assert!((miter - 144.0).abs() < 0.05, "miter {miter}");
        let round = polygons_area(&offset(&[sq()], 1.0, Join::Round, FillRule::EvenOdd, 0.001));
        let expect = 100.0 + 40.0 + std::f64::consts::PI;
        assert!((round - expect).abs() < 0.05, "round {round}");
    }

    #[test]
    fn inset() {
        let a = polygons_area(&offset(&[sq()], -1.0, Join::Miter, FillRule::EvenOdd, 0.001));
        assert!((a - 64.0).abs() < 0.05, "inset {a}");
    }

    #[test]
    fn open_line_stroke() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((10.0, 0.0));
        let a = polygons_area(&stroke_open(&[p], 1.0, Join::Round, 0.001));
        let expect = 20.0 + std::f64::consts::PI;
        assert!((a - expect).abs() < 0.05, "stroke {a}");
    }
}
