//! Fitting cubic Beziers to polylines (Schneider, "An Algorithm for Automatically Fitting
//! Digitized Curves", Graphics Gems 1990), with corner detection so straight edges and sharp
//! corners from boolean results stay sharp.

use kurbo::{BezPath, CubicBez, ParamCurve, Point, Vec2};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FitOptions {
    /// Maximum distance (mm) between the fitted curve and the input points.
    pub max_error: f64,
    /// Turning angle (radians) above which a vertex is kept as a corner.
    pub corner_angle: f64,
    /// Points in a run that are this close (mm) to its chord make the run a straight line.
    pub line_tolerance: f64,
}

impl Default for FitOptions {
    fn default() -> Self {
        Self { max_error: 0.02, corner_angle: 0.6, line_tolerance: 0.005 }
    }
}

/// Fits a contour (closed) or polyline (open) and returns a path of lines and cubics.
pub fn fit_contour(points: &[Point], closed: bool, opts: &FitOptions) -> BezPath {
    let mut pts: Vec<Point> = Vec::with_capacity(points.len());
    for &p in points {
        if pts.last().is_none_or(|q: &Point| q.distance_squared(p) > 1e-12) {
            pts.push(p);
        }
    }
    if closed && pts.len() > 1 && pts[0].distance_squared(pts[pts.len() - 1]) <= 1e-12 {
        pts.pop();
    }
    let mut path = BezPath::new();
    if pts.len() < 2 {
        return path;
    }
    let n = pts.len();
    let corners: Vec<usize> = (0..n)
        .filter(|&i| {
            if !closed && (i == 0 || i == n - 1) {
                return true;
            }
            let a = pts[(i + n - 1) % n];
            let b = pts[i];
            let c = pts[(i + 1) % n];
            turn_angle(b - a, c - b) > opts.corner_angle
        })
        .collect();

    // Runs between consecutive corners; for a closed contour without corners, one smooth loop.
    let mut runs: Vec<Vec<Point>> = Vec::new();
    if corners.is_empty() {
        let mut run = pts.clone();
        run.push(pts[0]);
        runs.push(run);
    } else {
        let count = if closed { corners.len() } else { corners.len() - 1 };
        for k in 0..count {
            let s = corners[k];
            let e = corners[(k + 1) % corners.len()];
            let mut run = vec![pts[s]];
            let mut i = s;
            loop {
                i = (i + 1) % n;
                run.push(pts[i]);
                if i == e {
                    break;
                }
            }
            runs.push(run);
        }
    }

    path.move_to(runs[0][0]);
    let smooth_loop = corners.is_empty() && closed;
    for run in &runs {
        if is_straight(run, opts.line_tolerance) {
            path.line_to(run[run.len() - 1]);
            continue;
        }
        let (t1, t2) = if smooth_loop {
            // Tangent continuity across the seam of a corner-free loop.
            let t = (run[1] - run[run.len() - 2]).normalize();
            (t, -t)
        } else {
            (left_tangent(run), right_tangent(run))
        };
        let mut cubics = Vec::new();
        fit_cubic(run, t1, t2, opts.max_error, &mut cubics, 0);
        for c in cubics {
            path.curve_to(c.p1, c.p2, c.p3);
        }
    }
    if closed {
        path.close_path();
    }
    path
}

fn turn_angle(u: Vec2, v: Vec2) -> f64 {
    let (lu, lv) = (u.hypot(), v.hypot());
    if lu < 1e-12 || lv < 1e-12 {
        return 0.0;
    }
    (u.cross(v)).atan2(u.dot(v)).abs()
}

fn is_straight(run: &[Point], tol: f64) -> bool {
    if run.len() <= 2 {
        return true;
    }
    let (a, b) = (run[0], run[run.len() - 1]);
    let d = b - a;
    let len = d.hypot();
    if len < 1e-9 {
        return false;
    }
    run.iter().all(|p| ((*p - a).cross(d) / len).abs() <= tol)
}

fn left_tangent(run: &[Point]) -> Vec2 {
    safe_norm(run[1] - run[0])
}

fn right_tangent(run: &[Point]) -> Vec2 {
    let n = run.len();
    safe_norm(run[n - 2] - run[n - 1])
}

fn safe_norm(v: Vec2) -> Vec2 {
    let l = v.hypot();
    if l < 1e-12 { Vec2::new(1.0, 0.0) } else { v / l }
}

const MAX_DEPTH: u32 = 24;

fn fit_cubic(pts: &[Point], t1: Vec2, t2: Vec2, err: f64, out: &mut Vec<CubicBez>, depth: u32) {
    let n = pts.len();
    if n == 2 {
        let d = pts[0].distance(pts[1]) / 3.0;
        out.push(CubicBez::new(pts[0], pts[0] + t1 * d, pts[1] + t2 * d, pts[1]));
        return;
    }
    let mut u = chord_params(pts);
    let mut bez = generate(pts, &u, t1, t2);
    let (mut max_err, mut split) = max_error(pts, &bez, &u);
    if max_err < err {
        out.push(bez);
        return;
    }
    if max_err < err * 4.0 {
        for _ in 0..8 {
            u = reparameterize(pts, &u, &bez);
            bez = generate(pts, &u, t1, t2);
            (max_err, split) = max_error(pts, &bez, &u);
            if max_err < err {
                out.push(bez);
                return;
            }
        }
    }
    if depth >= MAX_DEPTH || n < 3 {
        out.push(bez);
        return;
    }
    let split = split.clamp(1, n - 2);
    let tc = safe_norm(pts[split - 1] - pts[split + 1]);
    fit_cubic(&pts[..=split], t1, tc, err, out, depth + 1);
    fit_cubic(&pts[split..], -tc, t2, err, out, depth + 1);
}

fn chord_params(pts: &[Point]) -> Vec<f64> {
    let mut u = Vec::with_capacity(pts.len());
    u.push(0.0);
    for i in 1..pts.len() {
        let prev = u[i - 1];
        u.push(prev + pts[i].distance(pts[i - 1]));
    }
    let total = *u.last().unwrap_or(&1.0);
    if total > 0.0 {
        for x in &mut u {
            *x /= total;
        }
    }
    u
}

fn bernstein(t: f64) -> [f64; 4] {
    let mt = 1.0 - t;
    [mt * mt * mt, 3.0 * t * mt * mt, 3.0 * t * t * mt, t * t * t]
}

fn generate(pts: &[Point], u: &[f64], t1: Vec2, t2: Vec2) -> CubicBez {
    let first = pts[0];
    let last = pts[pts.len() - 1];
    let (mut c00, mut c01, mut c11, mut x0, mut x1) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (p, &t) in pts.iter().zip(u) {
        let b = bernstein(t);
        let a0 = t1 * b[1];
        let a1 = t2 * b[2];
        c00 += a0.dot(a0);
        c01 += a0.dot(a1);
        c11 += a1.dot(a1);
        let tmp = *p - (first.to_vec2() * (b[0] + b[1]) + last.to_vec2() * (b[2] + b[3])).to_point();
        x0 += a0.dot(tmp);
        x1 += a1.dot(tmp);
    }
    let det = c00 * c11 - c01 * c01;
    let seg = first.distance(last);
    let eps = 1e-6 * seg;
    let (mut a_l, mut a_r) = if det.abs() > 1e-12 {
        ((x0 * c11 - x1 * c01) / det, (c00 * x1 - c01 * x0) / det)
    } else {
        (seg / 3.0, seg / 3.0)
    };
    if a_l < eps || a_r < eps || a_l > seg * 2.0 || a_r > seg * 2.0 {
        a_l = seg / 3.0;
        a_r = seg / 3.0;
    }
    CubicBez::new(first, first + t1 * a_l, last + t2 * a_r, last)
}

fn max_error(pts: &[Point], bez: &CubicBez, u: &[f64]) -> (f64, usize) {
    let mut max = 0.0;
    let mut split = pts.len() / 2;
    for i in 1..pts.len() - 1 {
        let d = bez.eval(u[i]).distance_squared(pts[i]);
        if d >= max {
            max = d;
            split = i;
        }
    }
    (max.sqrt(), split)
}

fn reparameterize(pts: &[Point], u: &[f64], bez: &CubicBez) -> Vec<f64> {
    use kurbo::ParamCurveDeriv;
    let d1 = bez.deriv();
    let d2 = d1.deriv();
    pts.iter()
        .zip(u)
        .map(|(p, &t)| {
            let diff = bez.eval(t) - *p;
            let q1 = d1.eval(t).to_vec2();
            let q2 = d2.eval(t).to_vec2();
            let num = diff.dot(q1);
            let den = q1.dot(q1) + diff.dot(q2);
            if den.abs() < 1e-12 { t } else { (t - num / den).clamp(0.0, 1.0) }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::{ParamCurveNearest, Shape};

    fn max_dev_to_circle(path: &BezPath, c: Point, r: f64) -> f64 {
        let mut m: f64 = 0.0;
        for seg in path.segments() {
            for i in 0..=20 {
                let p = seg.eval(i as f64 / 20.0);
                m = m.max((p.distance(c) - r).abs());
            }
        }
        m
    }

    #[test]
    fn circle_fit_is_compact_and_accurate() {
        let pts: Vec<Point> = (0..200)
            .map(|i| {
                let a = i as f64 / 200.0 * std::f64::consts::TAU;
                Point::new(5.0 * a.cos(), 5.0 * a.sin())
            })
            .collect();
        let path = fit_contour(&pts, true, &FitOptions::default());
        let curves = path.segments().count();
        assert!(curves <= 12, "segments {curves}");
        assert!(max_dev_to_circle(&path, Point::ZERO, 5.0) < 0.03);
    }

    #[test]
    fn square_keeps_corners() {
        let mut pts = Vec::new();
        for i in 0..10 {
            pts.push(Point::new(i as f64, 0.0));
        }
        for i in 0..10 {
            pts.push(Point::new(10.0, i as f64));
        }
        for i in 0..10 {
            pts.push(Point::new(10.0 - i as f64, 10.0));
        }
        for i in 0..10 {
            pts.push(Point::new(0.0, 10.0 - i as f64));
        }
        let path = fit_contour(&pts, true, &FitOptions::default());
        assert_eq!(path.segments().count(), 4);
        assert!((path.area().abs() - 100.0).abs() < 1e-9);
        let bb = path.bounding_box();
        assert_eq!((bb.x0, bb.y0, bb.x1, bb.y1), (0.0, 0.0, 10.0, 10.0));
        let near = path.segments().next().unwrap().nearest(Point::new(5.0, -1.0), 1e-9);
        assert!((near.distance_sq.sqrt() - 1.0).abs() < 1e-9);
    }
}
