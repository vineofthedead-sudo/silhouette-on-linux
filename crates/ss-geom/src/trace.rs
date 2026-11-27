//! Raster Auto-Trace: binary mask → closed cut contours.
//!
//! Pixel boundaries are followed exactly (so holes and islands are preserved), then thinned with
//! Douglas–Peucker and optionally fitted with cubic Béziers. All coordinates are in pixels,
//! x right and y down; pixel `(i, j)` covers `[i, i+1] × [j, j+1]`.

use std::collections::HashMap;

use kurbo::{BezPath, Point};

use crate::boolean::contour_area;
use crate::fit::{FitOptions, fit_contour};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TraceOptions {
    /// Luminance cut-off (0–255) for opaque images, or opacity cut-off for images with transparency.
    pub threshold: u8,
    /// Trace the light areas instead of the dark ones.
    pub invert: bool,
    /// 0 keeps straight polygon edges; 1 fits smooth curves and rounds more corners.
    pub smoothing: f64,
    /// Contours enclosing less than this many pixels are dropped as specks.
    pub min_area_px: f64,
    /// Keep only the outermost contours (no holes, no islands inside holes).
    pub outer_only: bool,
}

impl Default for TraceOptions {
    fn default() -> Self {
        Self { threshold: 128, invert: false, smoothing: 0.5, min_area_px: 4.0, outer_only: false }
    }
}

/// Marks the pixels to trace in an RGBA buffer. Images with real transparency are traced by
/// opacity (the shape of the cut-out); opaque images by darkness.
pub fn mask_from_rgba(rgba: &[u8], width: usize, height: usize, threshold: u8, invert: bool) -> Vec<bool> {
    let px = rgba.as_chunks::<4>().0.iter().take(width * height);
    let has_alpha = px.clone().any(|p| p[3] < 250);
    px.map(|p| {
        let on = if has_alpha {
            p[3] >= threshold.max(1)
        } else {
            let lum = (u32::from(p[0]) * 299 + u32::from(p[1]) * 587 + u32::from(p[2]) * 114) / 1000;
            lum < u32::from(threshold)
        };
        on != invert
    })
    .collect()
}

type Vertex = (u32, u32);
// Edge directions are numbered E=0, S=1, W=2, N=3; a right turn on screen is `dir + 1`.

struct Edge {
    from: Vertex,
    to: Vertex,
    dir: usize,
}

/// Closed pixel-boundary loops. Outer boundaries have positive area, holes negative.
fn boundary_loops(mask: &[bool], w: usize, h: usize) -> Vec<Vec<Point>> {
    let on = |x: i64, y: i64| x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && mask[y as usize * w + x as usize];
    let mut edges: Vec<Edge> = Vec::new();
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            if !on(x, y) {
                continue;
            }
            let (xu, yu) = (x as u32, y as u32);
            if !on(x, y - 1) {
                edges.push(Edge { from: (xu, yu), to: (xu + 1, yu), dir: 0 });
            }
            if !on(x + 1, y) {
                edges.push(Edge { from: (xu + 1, yu), to: (xu + 1, yu + 1), dir: 1 });
            }
            if !on(x, y + 1) {
                edges.push(Edge { from: (xu + 1, yu + 1), to: (xu, yu + 1), dir: 2 });
            }
            if !on(x - 1, y) {
                edges.push(Edge { from: (xu, yu + 1), to: (xu, yu), dir: 3 });
            }
        }
    }
    let mut starts: HashMap<Vertex, Vec<usize>> = HashMap::with_capacity(edges.len());
    for (i, e) in edges.iter().enumerate() {
        starts.entry(e.from).or_default().push(i);
    }
    let mut used = vec![false; edges.len()];
    let mut loops = Vec::new();
    for first in 0..edges.len() {
        if used[first] {
            continue;
        }
        let mut pts = Vec::new();
        let mut cur = first;
        loop {
            used[cur] = true;
            let (to, dir) = (edges[cur].to, edges[cur].dir);
            pts.push((edges[cur].from, dir));
            // At a pinch point two edges leave; turning right keeps diagonal pixels apart.
            let next = starts.get(&to).and_then(|c| {
                let open: Vec<usize> = c.iter().copied().filter(|&i| !used[i] || i == first).collect();
                [(dir + 1) % 4, dir, (dir + 3) % 4]
                    .into_iter()
                    .find_map(|d| open.iter().copied().find(|&i| edges[i].dir == d))
            });
            match next {
                Some(n) if n != first => cur = n,
                _ => break,
            }
        }
        // Keep only the corners.
        let n = pts.len();
        let corners: Vec<Point> = (0..n)
            .filter(|&i| pts[i].1 != pts[(i + n - 1) % n].1)
            .map(|i| Point::new(f64::from(pts[i].0.0), f64::from(pts[i].0.1)))
            .collect();
        if corners.len() >= 3 {
            loops.push(corners);
        }
    }
    loops
}

fn point_in_loop(p: Point, poly: &[Point]) -> bool {
    let mut odd = false;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            odd = !odd;
        }
    }
    odd
}

fn bbox(poly: &[Point]) -> (f64, f64, f64, f64) {
    poly.iter().fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |b, p| (b.0.min(p.x), b.1.min(p.y), b.2.max(p.x), b.3.max(p.y)))
}

/// Closed Douglas–Peucker: anchors the loop at its two most distant vertices.
fn simplify_loop(pts: &[Point], eps: f64) -> Vec<Point> {
    if pts.len() <= 4 {
        return pts.to_vec();
    }
    let far = (1..pts.len()).max_by(|&a, &b| pts[0].distance_squared(pts[a]).total_cmp(&pts[0].distance_squared(pts[b]))).unwrap_or(1);
    let mut keep = vec![false; pts.len()];
    keep[0] = true;
    keep[far] = true;
    let mut stack = vec![(0, far), (far, pts.len())];
    while let Some((a, b)) = stack.pop() {
        let (pa, pb) = (pts[a], pts[b % pts.len()]);
        let seg = pb - pa;
        let len = seg.hypot();
        let mut worst = (0.0, None);
        for (i, pt) in pts.iter().enumerate().take(b).skip(a + 1) {
            let d = if len < 1e-12 { pt.distance(pa) } else { ((*pt - pa).cross(seg) / len).abs() };
            if d > worst.0 {
                worst = (d, Some(i));
            }
        }
        if let (d, Some(i)) = worst
            && d > eps
        {
            keep[i] = true;
            stack.push((a, i));
            stack.push((i, b));
        }
    }
    pts.iter().zip(keep).filter_map(|(p, k)| k.then_some(*p)).collect()
}

fn to_path(loop_pts: &[Point], smoothing: f64) -> BezPath {
    let s = smoothing.clamp(0.0, 1.0);
    let pts = simplify_loop(loop_pts, 0.6 + 0.6 * s);
    if s < 0.01 || pts.len() < 4 {
        let mut p = BezPath::new();
        p.move_to(pts[0]);
        for q in &pts[1..] {
            p.line_to(*q);
        }
        p.close_path();
        return p;
    }
    let opts = FitOptions { max_error: 0.3 + 0.8 * s, corner_angle: 0.5 + 0.9 * s, line_tolerance: 0.1 + 0.3 * s };
    fit_contour(&pts, true, &opts)
}

/// Traces `mask` (`width × height`, row-major) into closed paths in pixel coordinates.
/// Combine the paths in one compound object with even-odd fill to keep holes and islands.
pub fn trace_mask(mask: &[bool], width: usize, height: usize, opts: &TraceOptions) -> Vec<BezPath> {
    if width == 0 || height == 0 || mask.len() < width * height {
        return Vec::new();
    }
    let mut loops = boundary_loops(mask, width, height);
    loops.retain(|l| contour_area(l).abs() >= opts.min_area_px);
    if opts.outer_only {
        let boxes: Vec<_> = loops.iter().map(|l| bbox(l)).collect();
        let keep: Vec<bool> = (0..loops.len())
            .map(|i| {
                // Probe with the middle of the first edge, which no other loop can share.
                let probe = loops[i][0].midpoint(loops[i][1]);
                !(0..loops.len()).any(|j| {
                    let b = boxes[j];
                    j != i && probe.x >= b.0 && probe.x <= b.2 && probe.y >= b.1 && probe.y <= b.3 && point_in_loop(probe, &loops[j])
                })
            })
            .collect();
        let mut it = keep.into_iter();
        loops.retain(|_| it.next().unwrap_or(false));
    }
    loops.iter().map(|l| to_path(l, opts.smoothing)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flatten::to_contours;

    fn bitmap(w: usize, h: usize, f: impl Fn(usize, usize) -> bool) -> Vec<bool> {
        (0..w * h).map(|i| f(i % w, i / w)).collect()
    }

    fn signed_area(paths: &[BezPath]) -> f64 {
        to_contours(paths, 0.01).iter().map(|c| contour_area(c)).sum()
    }

    /// 24×24: a ring (outer 2..14, hole 6..10) and a separate 4×4 island at 18..22.
    fn ring_and_island() -> Vec<bool> {
        bitmap(24, 24, |x, y| {
            let ring = (2..14).contains(&x) && (2..14).contains(&y) && !((6..10).contains(&x) && (6..10).contains(&y));
            ring || ((18..22).contains(&x) && (18..22).contains(&y))
        })
    }

    #[test]
    fn traces_outer_edges_and_holes_exactly_without_smoothing() {
        let m = ring_and_island();
        let paths = trace_mask(&m, 24, 24, &TraceOptions { smoothing: 0.0, ..TraceOptions::default() });
        assert_eq!(paths.len(), 3);
        // outer 144 + island 16 − hole 16
        assert!((signed_area(&paths) - 144.0).abs() < 1e-6, "{}", signed_area(&paths));
        let b = crate::ops::bounds(&paths).unwrap();
        assert_eq!((b.x0, b.y0, b.x1, b.y1), (2.0, 2.0, 22.0, 22.0));
    }

    #[test]
    fn outer_only_drops_the_hole_but_keeps_separate_islands() {
        let m = ring_and_island();
        let paths = trace_mask(&m, 24, 24, &TraceOptions { smoothing: 0.0, outer_only: true, ..TraceOptions::default() });
        assert_eq!(paths.len(), 2);
        assert!((signed_area(&paths) - (144.0 + 16.0)).abs() < 1e-6);
    }

    #[test]
    fn an_island_inside_a_hole_is_not_outermost() {
        let m = bitmap(20, 20, |x, y| {
            let outer = (1..19).contains(&x) && (1..19).contains(&y);
            let hole = (5..15).contains(&x) && (5..15).contains(&y);
            let island = (8..12).contains(&x) && (8..12).contains(&y);
            (outer && !hole) || island
        });
        let all = trace_mask(&m, 20, 20, &TraceOptions { smoothing: 0.0, ..TraceOptions::default() });
        assert_eq!(all.len(), 3);
        let outer = trace_mask(&m, 20, 20, &TraceOptions { smoothing: 0.0, outer_only: true, ..TraceOptions::default() });
        assert_eq!(outer.len(), 1);
    }

    #[test]
    fn specks_are_ignored_and_diagonal_pixels_stay_separate() {
        let m = bitmap(10, 10, |x, y| (x == 1 && y == 1) || (x == 2 && y == 2) || ((4..9).contains(&x) && (4..9).contains(&y)));
        let paths = trace_mask(&m, 10, 10, &TraceOptions { smoothing: 0.0, min_area_px: 4.0, ..TraceOptions::default() });
        assert_eq!(paths.len(), 1);
        let both = trace_mask(&m, 10, 10, &TraceOptions { smoothing: 0.0, min_area_px: 0.0, ..TraceOptions::default() });
        assert_eq!(both.len(), 3, "touching corners must not merge into one loop");
    }

    #[test]
    fn smoothing_turns_a_disc_into_few_curves_close_to_the_pixels() {
        let m = bitmap(60, 60, |x, y| {
            let (dx, dy) = (x as f64 + 0.5 - 30.0, y as f64 + 0.5 - 30.0);
            dx * dx + dy * dy <= 20.0 * 20.0
        });
        let sharp = trace_mask(&m, 60, 60, &TraceOptions { smoothing: 0.0, ..TraceOptions::default() });
        let smooth = trace_mask(&m, 60, 60, &TraceOptions { smoothing: 0.5, ..TraceOptions::default() });
        assert_eq!((sharp.len(), smooth.len()), (1, 1));
        assert!(smooth[0].elements().len() < sharp[0].elements().len());
        let expected = std::f64::consts::PI * 400.0;
        assert!((signed_area(&smooth) - expected).abs() / expected < 0.03, "{}", signed_area(&smooth));
    }

    #[test]
    fn mask_uses_darkness_for_opaque_images_and_opacity_for_transparent_ones() {
        let opaque = [0, 0, 0, 255, 255, 255, 255, 255];
        assert_eq!(mask_from_rgba(&opaque, 2, 1, 128, false), [true, false]);
        assert_eq!(mask_from_rgba(&opaque, 2, 1, 128, true), [false, true]);
        let cutout = [255, 255, 255, 255, 255, 255, 255, 0];
        assert_eq!(mask_from_rgba(&cutout, 2, 1, 128, false), [true, false]);
    }

    #[test]
    fn empty_or_blank_input_gives_nothing() {
        assert!(trace_mask(&[], 0, 0, &TraceOptions::default()).is_empty());
        assert!(trace_mask(&[false; 16], 4, 4, &TraceOptions::default()).is_empty());
    }
}
