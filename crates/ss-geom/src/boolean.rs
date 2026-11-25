//! Polygon booleans: weld (union), subtract, intersect, exclude (xor), divide.
//!
//! The original application flattens Bezier paths to `glin.LinearPath`, runs the boolean on
//! polygons and refits curves. This module does the same on top of `i_overlay`.

use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::simplify::SimplifyShape;
use i_overlay::float::single::SingleFloatOverlay;
use kurbo::{BezPath, Point};

use crate::fit::{self, FitOptions};
use crate::flatten::{append_polyline, to_contours};
use crate::{Contour, FillRule, Polygon};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BoolOp {
    Union,
    Intersect,
    /// subject minus clip
    Difference,
    Xor,
}

impl BoolOp {
    fn rule(self) -> OverlayRule {
        match self {
            BoolOp::Union => OverlayRule::Union,
            BoolOp::Intersect => OverlayRule::Intersect,
            BoolOp::Difference => OverlayRule::Difference,
            BoolOp::Xor => OverlayRule::Xor,
        }
    }
}

type Raw = Vec<Vec<[f64; 2]>>;

fn to_raw(contours: &[Contour]) -> Raw {
    contours.iter().map(|c| c.iter().map(|p| [p.x, p.y]).collect()).collect()
}

fn from_shapes(shapes: Vec<Vec<Vec<[f64; 2]>>>) -> Vec<Polygon> {
    shapes
        .into_iter()
        .map(|s| s.into_iter().map(|c| c.into_iter().map(|[x, y]| Point::new(x, y)).collect()).collect())
        .collect()
}

/// Boolean between two sets of contours. Each set's own inside is decided by `fill`.
pub fn boolean_contours(subject: &[Contour], clip: &[Contour], op: BoolOp, fill: FillRule) -> Vec<Polygon> {
    let s = to_raw(subject);
    let c = to_raw(clip);
    if s.is_empty() && c.is_empty() {
        return Vec::new();
    }
    from_shapes(s.overlay(&c, op.rule(), fill.to_overlay()))
}

/// Normalises contours into valid polygons (resolves self-intersections, orients holes).
pub fn simplify(contours: &[Contour], fill: FillRule) -> Vec<Polygon> {
    let raw = to_raw(contours);
    if raw.is_empty() {
        return Vec::new();
    }
    from_shapes(raw.simplify_shape(fill.to_overlay()))
}

/// Boolean on Bezier paths; returns polygons.
pub fn boolean(subject: &[BezPath], clip: &[BezPath], op: BoolOp, fill: FillRule, tolerance: f64) -> Vec<Polygon> {
    boolean_contours(&to_contours(subject, tolerance), &to_contours(clip, tolerance), op, fill)
}

/// Weld: union of every shape. Each shape keeps its own even-odd holes; overlaps between
/// shapes merge.
pub fn weld(shapes: &[BezPath], fill: FillRule, tolerance: f64) -> Vec<Polygon> {
    let mut acc: Vec<Contour> = Vec::new();
    for s in shapes {
        let next = to_contours([s], tolerance);
        if next.is_empty() {
            continue;
        }
        let polys = boolean_contours(&acc, &next, BoolOp::Union, fill);
        acc = polys.into_iter().flatten().collect();
    }
    simplify(&acc, FillRule::EvenOdd)
}

/// Subtract every `cutters` shape from `base` (Silhouette "Subtract All" uses the front shapes).
pub fn subtract(base: &BezPath, cutters: &[BezPath], fill: FillRule, tolerance: f64) -> Vec<Polygon> {
    let clip_union: Vec<Contour> = weld(cutters, fill, tolerance).into_iter().flatten().collect();
    boolean_contours(&to_contours([base], tolerance), &clip_union, BoolOp::Difference, fill)
}

/// Divide: returns (a ∩ b, a − b, b − a), the pieces Silhouette's "Divide" produces.
pub fn divide(a: &BezPath, b: &BezPath, fill: FillRule, tolerance: f64) -> [Vec<Polygon>; 3] {
    let ca = to_contours([a], tolerance);
    let cb = to_contours([b], tolerance);
    [
        boolean_contours(&ca, &cb, BoolOp::Intersect, fill),
        boolean_contours(&ca, &cb, BoolOp::Difference, fill),
        boolean_contours(&cb, &ca, BoolOp::Difference, fill),
    ]
}

/// Converts polygons back to a single compound path. With `refit`, straight runs stay lines and
/// curved runs become cubic Beziers (the `CreateFromLinear_adaptive` step).
pub fn polygons_to_path(polys: &[Polygon], refit: Option<FitOptions>) -> BezPath {
    let mut out = BezPath::new();
    for c in polys.iter().flatten() {
        match refit {
            Some(opts) => {
                for el in fit::fit_contour(c, true, &opts).elements() {
                    out.push(*el);
                }
            }
            None => append_polyline(&mut out, c, true),
        }
    }
    out
}

/// Signed area (positive for y-down clockwise == math counterclockwise) of a contour.
pub fn contour_area(c: &[Point]) -> f64 {
    if c.len() < 3 {
        return 0.0;
    }
    let mut a = 0.0;
    for i in 0..c.len() {
        let p = c[i];
        let q = c[(i + 1) % c.len()];
        a += p.x * q.y - q.x * p.y;
    }
    a * 0.5
}

/// Filled area of polygons (outer minus holes).
pub fn polygons_area(polys: &[Polygon]) -> f64 {
    polys
        .iter()
        .map(|p| {
            let mut it = p.iter();
            let outer = it.next().map_or(0.0, |c| contour_area(c).abs());
            outer - it.map(|c| contour_area(c).abs()).sum::<f64>()
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::{Circle, Rect, Shape};

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> BezPath {
        Rect::new(x0, y0, x1, y1).to_path(0.0)
    }

    #[test]
    fn square_ops_areas() {
        let a = rect(0.0, 0.0, 4.0, 4.0);
        let b = rect(2.0, 2.0, 6.0, 6.0);
        let t = 0.001;
        let ff = FillRule::EvenOdd;
        let area = |op| polygons_area(&boolean(std::slice::from_ref(&a), std::slice::from_ref(&b), op, ff, t));
        assert!((area(BoolOp::Union) - 28.0).abs() < 1e-6);
        assert!((area(BoolOp::Intersect) - 4.0).abs() < 1e-6);
        assert!((area(BoolOp::Difference) - 12.0).abs() < 1e-6);
        assert!((area(BoolOp::Xor) - 24.0).abs() < 1e-6);
    }

    #[test]
    fn weld_keeps_holes_and_merges() {
        // A ring (even-odd hole) welded with a bar crossing it.
        let mut ring = rect(0.0, 0.0, 10.0, 10.0);
        for el in rect(3.0, 3.0, 7.0, 7.0).elements() {
            ring.push(*el);
        }
        let bar = rect(-2.0, 4.0, 12.0, 6.0);
        let polys = weld(&[ring, bar], FillRule::EvenOdd, 0.001);
        // 100 - 16 (hole) + bar parts outside the ring (2 * 2*2) + bar inside the hole (4*2)
        let expect = 100.0 - 16.0 + 8.0 + 8.0;
        assert!((polygons_area(&polys) - expect).abs() < 1e-6, "{}", polygons_area(&polys));
    }

    #[test]
    fn subtract_and_divide() {
        let base = rect(0.0, 0.0, 10.0, 10.0);
        let hole = Circle::new((5.0, 5.0), 2.0).to_path(1e-6);
        let polys = subtract(&base, std::slice::from_ref(&hole), FillRule::EvenOdd, 0.001);
        let expect = 100.0 - std::f64::consts::PI * 4.0;
        assert!((polygons_area(&polys) - expect).abs() < 0.01);
        let [inter, a_only, b_only] = divide(&base, &hole, FillRule::EvenOdd, 0.001);
        assert!((polygons_area(&inter) - std::f64::consts::PI * 4.0).abs() < 0.01);
        assert!((polygons_area(&a_only) - expect).abs() < 0.01);
        assert!(polygons_area(&b_only) < 1e-6);
    }

    #[test]
    fn refit_preserves_shape() {
        let a = Circle::new((0.0, 0.0), 10.0).to_path(1e-6);
        let b = rect(0.0, -20.0, 20.0, 20.0);
        let polys = boolean(&[a], &[b], BoolOp::Difference, FillRule::EvenOdd, 0.005);
        let path = polygons_to_path(&polys, Some(FitOptions::default()));
        let half = std::f64::consts::PI * 100.0 / 2.0;
        assert!((path.area().abs() - half).abs() < 0.5, "area {}", path.area());
        let curves = path.elements().iter().filter(|e| matches!(e, kurbo::PathEl::CurveTo(..))).count();
        assert!(curves > 0 && curves < 20, "curves {curves}");
    }

    mod props {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn union_and_intersection_bounds(
                ax in -50.0f64..50.0, ay in -50.0f64..50.0, aw in 1.0f64..40.0, ah in 1.0f64..40.0,
                bx in -50.0f64..50.0, by in -50.0f64..50.0, bw in 1.0f64..40.0, bh in 1.0f64..40.0,
            ) {
                let a = rect(ax, ay, ax + aw, ay + ah);
                let b = rect(bx, by, bx + bw, by + bh);
                let ff = FillRule::EvenOdd;
                let u = polygons_area(&boolean(std::slice::from_ref(&a), std::slice::from_ref(&b), BoolOp::Union, ff, 0.001));
                let i = polygons_area(&boolean(std::slice::from_ref(&a), std::slice::from_ref(&b), BoolOp::Intersect, ff, 0.001));
                let (aa, ba) = (aw * ah, bw * bh);
                prop_assert!((u + i - aa - ba).abs() < 1e-3 * (aa + ba));
                prop_assert!(u + 1e-6 >= aa.max(ba) * (1.0 - 1e-6));
                prop_assert!(i <= aa.min(ba) * (1.0 + 1e-6) + 1e-6);
            }
        }
    }
}
