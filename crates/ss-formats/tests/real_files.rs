//! Integration tests over real Silhouette Studio files.
//!
//! The files are never copied into the repository. Two locations are searched:
//!
//! * the stock samples shipped with the Windows installer, unpacked to
//!   `<workspace>/unpacked/app/Resources` (override with `SS_STOCK_DIR`);
//! * a folder of real user files with reference conversions (override with `SS_FIXTURES_DIR`).
//!
//! A test whose folder is missing prints a note and passes.

use std::path::{Path, PathBuf};

use ss_formats::studio::{self, StudioFile, StudioShape, StudioVersion};
use ss_geom::{BezPath, PathSeg, Point, Rect, Shape};

const DEFAULT_FIXTURES: &str = "/home/vineofthedead/Documents/Studio to SVG files for testing";

fn stock_dir() -> Option<PathBuf> {
    let p = std::env::var_os("SS_STOCK_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../unpacked/app/Resources"));
    p.is_dir().then_some(p)
}

fn fixtures_dir() -> Option<PathBuf> {
    let p = std::env::var_os("SS_FIXTURES_DIR").map_or_else(|| PathBuf::from(DEFAULT_FIXTURES), PathBuf::from);
    p.is_dir().then_some(p)
}

fn skip(what: &str) {
    eprintln!("skipping: {what} not found (set SS_STOCK_DIR / SS_FIXTURES_DIR)");
}

fn studio_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("studio") || e.eq_ignore_ascii_case("studio3"))
            {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn bounds(shapes: &[StudioShape]) -> Option<Rect> {
    shapes.iter().map(|s| s.to_bezpath().bounding_box()).reduce(|a, b| a.union(b))
}

/// Checks that holds for every decoded file, returning a description of the first failure.
fn check_geometry(f: &StudioFile) -> Result<(), String> {
    for s in &f.shapes {
        for sub in &s.outline {
            for p in &sub.points {
                let hs = [p.handle_in, p.handle_out].into_iter().flatten().flat_map(|(a, b)| [a, b]);
                if !(p.x.is_finite() && p.y.is_finite()) || hs.clone().any(|v| !v.is_finite()) {
                    return Err(format!("non-finite point in shape {}", s.serial));
                }
                // Real designs stay within a few metres of the page, even when parked off it.
                if p.x.abs() > 20_000.0 || p.y.abs() > 20_000.0 || hs.into_iter().any(|v| v.abs() > 20_000.0) {
                    return Err(format!("implausible coordinate {},{} in shape {}", p.x, p.y, s.serial));
                }
            }
        }
        if !s.matrix.iter().all(|v| v.is_finite()) {
            return Err(format!("non-finite matrix in shape {}", s.serial));
        }
        let bb = s.to_bezpath().bounding_box();
        if !(bb.width().is_finite() && bb.height().is_finite()) {
            return Err(format!("bad bounds in shape {}", s.serial));
        }
    }
    if let Some((w, h)) = f.page_mm
        && !(10.0..=10_000.0).contains(&w)
        | !(10.0..=10_000.0).contains(&h)
    {
        return Err(format!("implausible page size {w}x{h}"));
    }
    Ok(())
}

/// Least-squares affine fit from the local outline (block A) to the page outline (block P).
/// Returns the largest residual in mm, or `None` when the local outline is not found.
fn local_to_page_residual(doc: &[u8], s: &StudioShape) -> Option<f64> {
    let local = studio::local_outline(doc, s)?;
    let pairs: Vec<(f64, f64, f64, f64)> = local
        .iter()
        .flat_map(|sub| &sub.points)
        .zip(s.outline.iter().flat_map(|sub| &sub.points))
        .map(|(l, p)| (f64::from(l.x), f64::from(l.y), f64::from(p.x), f64::from(p.y)))
        .collect();
    if pairs.len() < 3 {
        return Some(0.0);
    }
    // Normal equations for [a b c] with target = a*x + b*y + c, solved for both page axes.
    let mut m = [[0f64; 3]; 3];
    let (mut rx, mut ry) = ([0f64; 3], [0f64; 3]);
    for &(x, y, px, py) in &pairs {
        let v = [x, y, 1.0];
        for i in 0..3 {
            for j in 0..3 {
                m[i][j] += v[i] * v[j];
            }
            rx[i] += v[i] * px;
            ry[i] += v[i] * py;
        }
    }
    let solve = |r: [f64; 3]| -> Option<[f64; 3]> {
        let det = |a: [[f64; 3]; 3]| {
            a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1]) - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
                + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0])
        };
        let d = det(m);
        if d.abs() < 1e-12 {
            return None;
        }
        let mut out = [0f64; 3];
        for (k, o) in out.iter_mut().enumerate() {
            let mut a = m;
            for i in 0..3 {
                a[i][k] = r[i];
            }
            *o = det(a) / d;
        }
        Some(out)
    };
    // Collinear outlines (single lines) make the system singular; nothing to check there.
    let (Some(ax), Some(ay)) = (solve(rx), solve(ry)) else { return Some(0.0) };
    Some(
        pairs
            .iter()
            .map(|&(x, y, px, py)| {
                let ex = ax[0] * x + ax[1] * y + ax[2] - px;
                let ey = ay[0] * x + ay[1] * y + ay[2] - py;
                ex.hypot(ey)
            })
            .fold(0.0, f64::max),
    )
}

struct Summary {
    files: usize,
    shapes: usize,
    local_found: usize,
    local_fit: usize,
}

fn parse_all(files: &[PathBuf], require_shapes: bool) -> Summary {
    let mut sum = Summary { files: 0, shapes: 0, local_found: 0, local_fit: 0 };
    let mut failures = Vec::new();
    for path in files {
        let data = std::fs::read(path).unwrap();
        let f = match studio::parse(&data) {
            Ok(f) => f,
            Err(e) => {
                failures.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        let ext = path.extension().unwrap().to_ascii_lowercase();
        let expected = if ext == "studio3" { StudioVersion::V5 } else { StudioVersion::V4 };
        if f.version != expected {
            failures.push(format!("{}: version {:?}", path.display(), f.version));
        }
        if require_shapes && f.shapes.is_empty() {
            failures.push(format!("{}: no shapes", path.display()));
        }
        if let Err(e) = check_geometry(&f) {
            failures.push(format!("{}: {e}", path.display()));
        }
        if f.version == StudioVersion::V5 {
            if f.sections.len() < 3 || f.preview_png.is_none() {
                failures.push(format!("{}: sections {:?}, preview {}", path.display(), f.sections, f.preview_png.is_some()));
            }
            if f.saved_by.is_none() {
                failures.push(format!("{}: no writer version", path.display()));
            }
        }
        let doc = studio::document_section(&data).unwrap();
        for s in &f.shapes {
            if let Some(r) = local_to_page_residual(doc, s) {
                sum.local_found += 1;
                if r < 0.01 {
                    sum.local_fit += 1;
                }
            }
        }
        let converted = studio::to_document(&f);
        if converted.objects.len() != f.shapes.len() {
            failures.push(format!("{}: to_document dropped shapes", path.display()));
        }
        sum.files += 1;
        sum.shapes += f.shapes.len();
    }
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
    sum
}

#[test]
fn stock_samples_parse() {
    let Some(dir) = stock_dir() else { return skip("stock Resources folder") };
    let files = studio_files(&dir);
    assert_eq!(files.len(), 39, "expected the 39 stock samples in {}", dir.display());
    let s = parse_all(&files, false);
    eprintln!("stock: {} files, {} shapes, local outline found {} fits {}", s.files, s.shapes, s.local_found, s.local_fit);
    assert!(s.shapes > 0);
    // The page outline is authoritative. The local outline is a cross-check: where it is found,
    // an affine map must carry it onto the page outline.
    assert!(s.local_found * 10 >= s.shapes * 9, "local outline found for {}/{}", s.local_found, s.shapes);
    assert!(s.local_fit * 100 >= s.local_found * 99, "local outlines must map onto page outlines");
}

#[test]
fn user_fixtures_parse() {
    let Some(dir) = fixtures_dir() else { return skip("fixtures folder") };
    let files = studio_files(&dir);
    assert!(!files.is_empty());
    let s = parse_all(&files, true);
    eprintln!("fixtures: {} files, {} shapes, local outline found {} fits {}", s.files, s.shapes, s.local_found, s.local_fit);
    // The page outline is authoritative. The local outline is a cross-check: where it is found,
    // an affine map must carry it onto the page outline.
    assert!(s.local_found * 10 >= s.shapes * 9, "local outline found for {}/{}", s.local_found, s.shapes);
    assert!(s.local_fit * 100 >= s.local_found * 99, "local outlines must map onto page outlines");
}

/// Reads `data-name` and `d` of each `<path>` in an SVG written by studio3svg.
fn reference_paths(svg: &str) -> Vec<(String, BezPath)> {
    let attr = |tag: &str, name: &str| -> Option<String> {
        let key = format!("{name}=\"");
        let start = tag.find(&key)? + key.len();
        let len = tag[start..].find('"')?;
        Some(tag[start..start + len].to_string())
    };
    svg.split("<path")
        .skip(1)
        .map(|chunk| {
            let tag = &chunk[..chunk.find('>').unwrap()];
            let d = attr(tag, "d").expect("path without d");
            (attr(tag, "data-name").unwrap_or_default(), BezPath::from_svg(&d).expect("bad path data"))
        })
        .collect()
}

/// Segments as the reference writer emits them: coordinates rounded to 3 decimals and segments
/// that collapse to a point after rounding dropped (this also absorbs the closing line, which
/// studio3svg leaves to `Z`).
fn segments(p: &BezPath) -> Vec<PathSeg> {
    let round = |p: Point| Point::new((p.x * 1000.0).round() / 1000.0, (p.y * 1000.0).round() / 1000.0);
    p.segments()
        .filter(|s| {
            let pts: Vec<Point> = seg_points(s).into_iter().map(round).collect();
            match s {
                PathSeg::Line(_) => pts[0] != pts[1],
                _ => pts.iter().any(|q| *q != pts[0]),
            }
        })
        .collect()
}

fn seg_points(s: &PathSeg) -> Vec<Point> {
    match *s {
        PathSeg::Line(l) => vec![l.p0, l.p1],
        PathSeg::Quad(q) => vec![q.p0, q.p1, q.p2],
        PathSeg::Cubic(c) => vec![c.p0, c.p1, c.p2, c.p3],
    }
}

/// Largest point deviation between two paths, or `None` when their segment structure differs.
fn deviation(a: &BezPath, b: &BezPath) -> Option<f64> {
    let (sa, sb) = (segments(a), segments(b));
    if sa.len() != sb.len() {
        return None;
    }
    let mut worst = 0f64;
    for (x, y) in sa.iter().zip(&sb) {
        let (px, py) = (seg_points(x), seg_points(y));
        if px.len() != py.len() {
            return None;
        }
        for (p, q) in px.iter().zip(&py) {
            worst = worst.max(p.distance(*q));
        }
    }
    Some(worst)
}

/// Matches every reference path to a decoded shape (each used once) and returns the worst
/// deviation in mm plus the number of reference paths left unmatched.
fn compare_with_reference(studio_path: &Path, svg_path: &Path, tol: f64) -> (usize, usize, f64) {
    let f = studio::parse(&std::fs::read(studio_path).unwrap()).unwrap();
    let refs = reference_paths(&std::fs::read_to_string(svg_path).unwrap());
    let ours: Vec<BezPath> = f.shapes.iter().map(StudioShape::to_bezpath).collect();
    let mut used = vec![false; ours.len()];
    let mut worst = 0f64;
    let mut unmatched = Vec::new();
    for (i, (name, rp)) in refs.iter().enumerate() {
        // The file order normally lines up; fall back to a search otherwise.
        let try_idx = |j: usize| (!used[j]).then(|| deviation(&ours[j], rp)).flatten().filter(|d| *d <= tol);
        let hit = (i < ours.len()).then(|| try_idx(i).map(|d| (i, d))).flatten().or_else(|| {
            (0..ours.len()).filter_map(|j| try_idx(j).map(|d| (j, d))).min_by(|a, b| a.1.total_cmp(&b.1))
        });
        match hit {
            Some((j, d)) => {
                used[j] = true;
                worst = worst.max(d);
                if let Some(n) = &f.shapes[j].name {
                    assert_eq!(n, name, "shape name differs for reference path {i}");
                }
            }
            None => unmatched.push(i),
        }
    }
    eprintln!(
        "{}: {} shapes, {} reference paths, {} unmatched {:?}, worst deviation {worst:.5} mm",
        svg_path.file_name().unwrap().to_string_lossy(),
        ours.len(),
        refs.len(),
        unmatched.len(),
        &unmatched[..unmatched.len().min(10)]
    );
    (ours.len(), unmatched.len(), worst)
}

fn fixture(dir: &Path, name: &str) -> Option<PathBuf> {
    [dir.join("Chip and Dale").join(name), dir.join(name)].into_iter().find(|p| p.is_file())
}

#[test]
fn matches_studio3svg_reference_chip_dale_2() {
    let Some(dir) = fixtures_dir() else { return skip("fixtures folder") };
    let (Some(studio), Some(svg)) = (
        fixture(&dir, "CHIP & DALE #2.studio3"),
        fixture(&dir, "CHIP & DALE #2(Shapes only converted via studio3svg).svg"),
    ) else {
        return skip("CHIP & DALE #2 fixtures");
    };
    // The reference rounds to 3 decimals, so 0.0005 mm per coordinate is the expected noise.
    let (shapes, unmatched, worst) = compare_with_reference(&studio, &svg, 0.002);
    assert_eq!(shapes, 133);
    assert_eq!(unmatched, 0);
    assert!(worst < 0.001, "worst deviation {worst}");
}

#[test]
fn matches_reference_chip_dale_1() {
    let Some(dir) = fixtures_dir() else { return skip("fixtures folder") };
    let (Some(studio), Some(svg)) = (fixture(&dir, "CHIP & DALE #1.studio3"), fixture(&dir, "CHIP & DALE #1.svg")) else {
        return skip("CHIP & DALE #1 fixtures");
    };
    let (_, unmatched, worst) = compare_with_reference(&studio, &svg, 0.002);
    assert_eq!(unmatched, 0);
    assert!(worst < 0.001, "worst deviation {worst}");
}

/// The embedded preview frames the design: its inked area should have the same aspect ratio as
/// the decoded shapes' bounding box.
#[test]
fn preview_matches_shape_bounds() {
    let Some(dir) = fixtures_dir() else { return skip("fixtures folder") };
    let (mut checked, mut agree) = (0, 0);
    let mut worst = Vec::new();
    for path in studio_files(&dir) {
        let f = studio::parse(&std::fs::read(&path).unwrap()).unwrap();
        let (Some(png), Some(bb)) = (f.preview_png.as_ref(), bounds(&f.shapes)) else { continue };
        let Some(ink) = ink_box(png) else { continue };
        if bb.width() < 1.0 || bb.height() < 1.0 || ink.width() < 8.0 || ink.height() < 8.0 {
            continue;
        }
        checked += 1;
        let ratio = (bb.width() / bb.height()) / (ink.width() / ink.height());
        if (ratio.ln()).abs() < 0.1f64.ln_1p() {
            agree += 1;
        } else {
            worst.push(format!("{} ratio {ratio:.3}", path.file_name().unwrap().to_string_lossy()));
        }
    }
    eprintln!("preview aspect: {agree}/{checked} agree; mismatches: {worst:?}");
    assert!(checked > 0);
    assert!(agree * 10 >= checked * 9, "{agree}/{checked}");
}

/// Bounding box of non-background pixels in a PNG.
fn ink_box(png_bytes: &[u8]) -> Option<Rect> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(png_bytes));
    dec.set_transformations(png::Transformations::normalize_to_color8() | png::Transformations::ALPHA);
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let ch = info.color_type.samples();
    let (w, h) = (info.width as usize, info.height as usize);
    let mut r: Option<Rect> = None;
    for y in 0..h {
        for x in 0..w {
            let px = &buf[(y * w + x) * ch..][..ch];
            let (rgb, a) = match ch {
                4 => (&px[..3], px[3]),
                2 => (&px[..1], px[1]),
                _ => (&px[..ch.min(3)], 255),
            };
            if a > 16 && rgb.iter().any(|&c| c < 240) {
                let p = Rect::new(x as f64, y as f64, x as f64 + 1.0, y as f64 + 1.0);
                r = Some(r.map_or(p, |q| q.union(p)));
            }
        }
    }
    r
}

