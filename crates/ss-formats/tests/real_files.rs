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
    images: usize,
}

fn parse_all(files: &[PathBuf], require_shapes: bool) -> Summary {
    let mut sum = Summary { files: 0, shapes: 0, local_found: 0, local_fit: 0, images: 0 };
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
        for o in &converted.objects {
            let ss_doc::ObjectKind::Image(img) = &o.kind else { continue };
            sum.images += 1;
            // Placed images keep their pixel aspect ratio and are never cut.
            let (b, px) = (o.bounds().unwrap(), f64::from(img.width_px) / f64::from(img.height_px));
            let (ex, ey) = (o.transform * ss_geom::Point::new(1.0, 0.0) - o.transform * ss_geom::Point::ZERO, o.transform * ss_geom::Point::new(0.0, 1.0) - o.transform * ss_geom::Point::ZERO);
            let shown = (ex.hypot() * img.placement().width()) / (ey.hypot() * img.placement().height());
            if (shown / px).ln().abs() > 0.02 || !o.cut_paths().is_empty() || !b.width().is_finite() {
                failures.push(format!("{}: image {} aspect {shown:.3} vs {px:.3}", path.display(), o.name));
            }
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
    eprintln!(
        "fixtures: {} files, {} shapes, local outline found {} fits {}, placed images {}",
        s.files, s.shapes, s.local_found, s.local_fit, s.images
    );
    assert!(s.images > 0, "the fixture corpus contains placed print-and-cut images");
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

#[test]
fn disney_hearts_v4_and_v5_match_svg_colors_and_counts() {
    let Some(dir) = fixtures_dir() else { return skip("fixtures folder") };
    let (Some(v4_path), Some(v5_path), Some(svg_path)) = (
        fixture(&dir, "DISNEY HEARTS.studio"),
        fixture(&dir, "DISNEY HEARTS.studio3"),
        fixture(&dir, "DISNEY HEARTS.svg"),
    ) else {
        return skip("DISNEY HEARTS fixtures");
    };

    let svg_text = std::fs::read_to_string(&svg_path).unwrap();
    for hex in ["#000000", "#800000", "#FFFFFF"] {
        assert!(svg_text.contains(hex), "expected {hex} in DISNEY HEARTS.svg");
    }

    for (path, expected_ver) in [(v4_path, StudioVersion::V4), (v5_path, StudioVersion::V5)] {
        let f = studio::parse(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(f.version, expected_ver);
        assert_eq!(f.shapes.len(), 449);
        let doc = studio::to_document(&f);
        assert_eq!(doc.objects.len(), 449);

        let fills: std::collections::HashSet<String> =
            doc.objects.iter().filter_map(|o| o.style.fill.map(|c| c.to_hex().to_uppercase())).collect();
        assert!(fills.contains("#000000"), "{}: missing #000000 fill in {fills:?}", path.display());
        assert!(fills.contains("#800000"), "{}: missing #800000 fill in {fills:?}", path.display());
        assert!(fills.contains("#FFFFFF"), "{}: missing #FFFFFF fill in {fills:?}", path.display());

        let strokes: std::collections::HashSet<String> =
            doc.objects.iter().filter(|o| o.style.stroke.a > 0).map(|o| o.style.stroke.to_hex().to_uppercase()).collect();
        assert!(strokes.contains("#000000"));
        assert!(strokes.contains("#800000"));
        assert!(!strokes.contains("#E00000"), "unexpected default red stroke fallback");

        // Verify off-mat object positions are preserved.
        let bb = doc.bounds().unwrap();
        assert!(bb.x0 < -150.0 && bb.x1 > 600.0, "unexpected bounds {bb:?}");
    }
}

#[test]
fn japan_v4_and_v5_match_svg_colors_patterns_and_offmat() {
    let Some(dir) = fixtures_dir() else { return skip("fixtures folder") };
    let (Some(v4_path), Some(v5_path), Some(svg_path)) = (
        fixture(&dir, "JAPAN.studio"),
        fixture(&dir, "JAPAN.studio3"),
        fixture(&dir, "JAPAN.svg"),
    ) else {
        return skip("JAPAN fixtures");
    };

    let svg_text = std::fs::read_to_string(&svg_path).unwrap();
    for hex in ["#000000", "#BBBBBB", "#F53727", "#FF0000", "#FFFFFF"] {
        assert!(svg_text.contains(hex), "expected {hex} in JAPAN.svg");
    }

    for (path, expected_ver) in [(v4_path, StudioVersion::V4), (v5_path, StudioVersion::V5)] {
        let f = studio::parse(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(f.version, expected_ver);
        assert_eq!(f.shapes.len(), 684);
        assert_eq!(f.images.len(), 9, "{}: expected 9 embedded images (6 PNG + 3 JPEG)", path.display());

        let doc = studio::to_document(&f);
        assert_eq!(doc.objects.len(), 684);

        let pattern_count = doc.objects.iter().filter(|o| o.style.fill_pattern.is_some()).count();
        assert_eq!(pattern_count, 16, "{}: expected 16 pattern-filled objects", path.display());

        let fills: std::collections::HashSet<String> =
            doc.objects.iter().filter_map(|o| o.style.fill.map(|c| c.to_hex().to_uppercase())).collect();
        for hex in ["#000000", "#BBBBBB", "#F53727", "#FF0000", "#FFFFFF"] {
            assert!(fills.contains(hex), "{}: missing {hex} fill in {fills:?}", path.display());
        }

        // Verify off-mat object positions (JAPAN sits above and left of the 12x12 mat).
        let bb = doc.bounds().unwrap();
        assert!(bb.x0 < -400.0 && bb.y0 < -340.0, "unexpected off-mat bounds {bb:?}");
    }
}

/// One `<path>` definition from a Silhouette Studio SVG export.
struct SvgDef {
    attrs: std::collections::HashMap<String, String>,
    path: BezPath,
    /// `(image id, tile matrix)` when the path is filled with a clipped pattern or picture.
    tile: Option<(String, [f64; 6])>,
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let s = tag.find(&key)? + key.len();
    Some(&tag[s..s + tag[s..].find('"')?])
}

fn matrix(s: &str) -> Option<[f64; 6]> {
    let v: Vec<f64> = s.strip_prefix("matrix(")?.strip_suffix(')')?.split_whitespace().map(|x| x.parse().ok()).collect::<Option<_>>()?;
    v.try_into().ok()
}

/// Definitions keyed by their first anchor rounded to 0.01 mm.
fn svg_defs(svg: &str) -> std::collections::HashMap<(i64, i64), SvgDef> {
    let defs_end = svg.find("</defs>").unwrap();
    let (defs, body) = svg.split_at(defs_end);
    let mut clips = std::collections::HashMap::new();
    for part in defs.split("<clipPath id=\"").skip(1) {
        let clip = &part[..part.find('"').unwrap()];
        let href = attr(&part[..part.find("/>").unwrap()], "xlink:href").unwrap();
        clips.insert(href.trim_start_matches('#').to_string(), clip.to_string());
    }
    let mut tiles = std::collections::HashMap::new();
    for part in body.split("<g clip-path=\"url(#").skip(1) {
        let clip = &part[..part.find(')').unwrap()];
        let inner = &part[part.find("<g transform=\"").unwrap() + 14..];
        let m = matrix(&inner[..inner.find('"').unwrap()]).unwrap();
        let img = attr(&inner[..inner.find("/>").unwrap()], "xlink:href").unwrap();
        tiles.insert(clip.to_string(), (img.trim_start_matches("#c_").to_string(), m));
    }
    let mut out = std::collections::HashMap::new();
    for part in defs.split("<path ").skip(1) {
        let tag = format!(" {}", &part[..part.find("/>").unwrap()]);
        let (Some(id), Some(d)) = (attr(&tag, "id"), attr(&tag, "d")) else {
            continue;
        };
        let mut it = d.split_whitespace().skip(1).map(|v| v.parse::<f64>().unwrap());
        let key = ((it.next().unwrap() * 100.0).round() as i64, (it.next().unwrap() * 100.0).round() as i64);
        let attrs =
            ["fill", "stroke", "stroke-width"].into_iter().filter_map(|n| Some((n.to_string(), attr(&tag, n)?.to_string()))).collect();
        let tile = clips.get(id).and_then(|c| tiles.get(c)).cloned();
        out.insert(key, SvgDef { attrs, path: BezPath::from_svg(d).unwrap(), tile });
    }
    out
}

/// Every shape of JAPAN is compared with the matching path of Silhouette Studio's own SVG export:
/// fill and stroke colours, stroke width, and the exact pattern tile placement.
#[test]
fn japan_shapes_match_silhouette_svg_export() {
    use ss_doc::ObjectKind;
    use ss_geom::Affine;

    let Some(dir) = fixtures_dir() else {
        return skip("fixtures folder");
    };
    let (Some(v4_path), Some(v5_path), Some(svg_path)) =
        (fixture(&dir, "JAPAN.studio"), fixture(&dir, "JAPAN.studio3"), fixture(&dir, "JAPAN.svg"))
    else {
        return skip("JAPAN fixtures");
    };
    let defs = svg_defs(&std::fs::read_to_string(&svg_path).unwrap());
    assert_eq!(defs.len(), 684);

    for path in [v4_path, v5_path] {
        let f = studio::parse(&std::fs::read(&path).unwrap()).unwrap();
        let doc = studio::to_document(&f);
        assert_eq!(doc.objects.len(), f.shapes.len());
        let (mut matched, mut tiles, mut pictures, mut gradients) = (0, 0, 0, 0);
        let mut errors = Vec::new();
        for (s, o) in f.shapes.iter().zip(&doc.objects) {
            let p = s.outline[0].points[0];
            let key = ((f64::from(p.x) * 100.0).round() as i64, (f64::from(p.y) * 100.0).round() as i64);
            // Exports print 4-5 significant digits, so a coordinate can round either way.
            let near = (-1..=1).flat_map(|dx| (-1..=1).map(move |dy| (key.0 + dx, key.1 + dy)));
            let Some(def) = defs.get(&key).or_else(|| near.filter_map(|k| defs.get(&k)).next()) else {
                continue;
            };
            matched += 1;
            let mut err = |what: String| errors.push(format!("shape at ({}, {}): {what}", p.x, p.y));
            // Compare the mesh's fill area with the independent reference geometry. SVG numbers
            // are rounded; allow the area of a 0.01 mm strip along the flattened boundary.
            let contours = ss_geom::flatten::to_contours([&def.path], 0.02);
            let polygons = ss_geom::boolean::simplify(&contours, o.fill_rule());
            let expected_area: f64 = polygons
                .iter()
                .map(|poly| {
                    poly.iter()
                        .map(|ring| {
                            ring.iter().zip(ring.iter().cycle().skip(1)).take(ring.len()).map(|(a, b)| a.x * b.y - b.x * a.y).sum::<f64>()
                                * 0.5
                        })
                        .sum::<f64>()
                        .abs()
                })
                .sum();
            let perimeter: f64 = ss_geom::flatten::flatten(&def.path, 0.02).iter().map(|l| l.len_mm()).sum();
            let (vertices, indices) = ss_geom::triangulate::triangulate_path(&s.to_bezpath(), o.fill_rule(), 0.02);
            let area: f64 = indices
                .as_chunks::<3>()
                .0
                .iter()
                .map(|t| {
                    let (a, b, c) = (vertices[t[0] as usize], vertices[t[1] as usize], vertices[t[2] as usize]);
                    (b - a).cross(c - a).abs() * 0.5
                })
                .sum();
            if (area - expected_area).abs() > perimeter * 0.01 {
                err(format!("mesh area {area:.6}, reference {expected_area:.6}"));
            }
            assert!(!matches!(o.kind, ObjectKind::Image(_)), "pictures are clipped pattern fills");
            let fill = o.style.fill.map(|c| c.to_hex().to_uppercase()).unwrap_or_else(|| "none".into());
            if def.attrs["fill"].starts_with("url(#ss_lineargradient") {
                // Gradients are not modelled yet; one of their colours is used as a solid fill.
                gradients += 1;
                if o.style.fill.is_none() {
                    err("gradient without a fallback fill".into());
                }
            } else if def.attrs["fill"] != fill {
                err(format!("fill {fill}, SVG {}", def.attrs["fill"]));
            }
            let stroke = if o.style.stroke.a > 0 { o.style.stroke.to_hex().to_uppercase() } else { "none".into() };
            if def.attrs["stroke"] != stroke {
                err(format!("stroke {stroke}, SVG {}", def.attrs["stroke"]));
            }
            // The export writes hairlines (thickness 0) as 0.1 mm.
            let width: f64 = def.attrs["stroke-width"].parse().unwrap();
            let ours = if o.style.stroke_width_mm > 0.0 { o.style.stroke_width_mm } else { 0.1 };
            if stroke != "none" && (width - ours).abs() > 1e-3 {
                err(format!("stroke width {ours}, SVG {width}"));
            }
            match (&def.tile, &o.style.fill_pattern) {
                (None, None) => {}
                (Some((id, g)), Some(pat)) => {
                    match s.pattern {
                        Some(m) if m.tiled => tiles += 1,
                        Some(_) => pictures += 1,
                        None => {}
                    }
                    if *id != pat.id {
                        err(format!("pattern {}, SVG {id}", pat.id));
                    }
                    let Some(t) = pat.transform else {
                        err("pattern without placement".into());
                        continue;
                    };
                    let scale = g[0].hypot(g[1]);
                    if t[..4].iter().zip(&g[..4]).any(|(a, b)| (a - b).abs() > 1e-3 * scale) {
                        err(format!("pattern matrix {t:?}, SVG {g:?}"));
                    }
                    // Tiles repeat, so the SVG origin must land on a whole tile in our pixel space.
                    let px = Affine::new(t).inverse() * ss_geom::Point::new(g[4], g[5]);
                    let (tx, ty) = (px.x / f64::from(pat.width_px), px.y / f64::from(pat.height_px));
                    if (tx - tx.round()).abs() > 0.01 || (ty - ty.round()).abs() > 0.01 {
                        err(format!("pattern phase off by ({tx:.3}, {ty:.3}) tiles"));
                    }
                }
                (svg, ours) => err(format!("pattern {:?}, SVG {:?}", ours.as_ref().map(|p| &p.id), svg.as_ref().map(|t| &t.0))),
            }
        }
        let name = path.display();
        assert!(errors.is_empty(), "{name}: {} mismatches:\n{}", errors.len(), errors[..errors.len().min(20)].join("\n"));
        assert!(matched >= 676, "{name}: only {matched} of 684 shapes matched an SVG path");
        assert_eq!((tiles, pictures, gradients), (14, 2, 4), "{name}: pattern, picture and gradient fills");
    }
}

/// Coverage is checked against the filled polygon boundaries, not just triangle bounds.
/// This catches stalled ear clipping, overlapping fills, and bridges crossing holes.
#[test]
fn japan_fill_meshes_cover_paths() {
    use ss_geom::{FillRule, boolean, flatten, triangulate};
    let Some(dir) = fixtures_dir() else {
        return skip("fixtures folder");
    };
    for name in ["JAPAN.studio", "JAPAN.studio3"] {
        let Some(file) = fixture(&dir, name) else {
            return skip(name);
        };
        let doc = studio::to_document(&studio::parse(&std::fs::read(file).unwrap()).unwrap());
        let mut checked = 0;
        for (i, object) in doc.objects.iter().enumerate() {
            for path in object.page_paths() {
                let contours = flatten::to_contours([&path], 0.02);
                let filled = boolean::simplify(&contours, object.fill_rule());
                let expected: Vec<_> = filled.into_iter().flatten().collect();
                let (vertices, indices) = triangulate::triangulate_path(&path, object.fill_rule(), 0.02);
                assert_eq!(indices.len() % 3, 0);
                assert!(vertices.iter().all(|p| p.is_finite()));
                let triangles: Vec<Vec<Point>> =
                    indices.as_chunks::<3>().0.iter().map(|t| t.iter().map(|&j| vertices[j as usize]).collect()).collect();
                let mismatch = boolean::boolean_contours(&expected, &triangles, boolean::BoolOp::Xor, FillRule::NonZero);
                let mismatch_area: f64 = mismatch
                    .iter()
                    .map(|poly| {
                        poly.iter()
                            .map(|ring| {
                                ring.iter()
                                    .zip(ring.iter().cycle().skip(1))
                                    .take(ring.len())
                                    .map(|(a, b)| a.x * b.y - b.x * a.y)
                                    .sum::<f64>()
                                    * 0.5
                            })
                            .sum::<f64>()
                            .abs()
                    })
                    .sum();
                assert!(mismatch_area < 1e-4, "{name} shape {i}: missing or excess fill {mismatch_area} mm²");
                checked += 1;
            }
        }
        assert_eq!(checked, 684, "{name}");
    }
}
