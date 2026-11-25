//! Golden-output tests for the GPGL encoder, checked by replaying the stream through the
//! interpreter (the virtual cutter model).

use ss_geom::flatten::flatten;
use ss_geom::kurbo::Circle;
use ss_geom::{BezPath, Point, Rect, Shape};
use ss_gpgl::{EncodeError, EndAction, JobSettings, OffMedia, Perforation, ToolSettings, encode, simulate};

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> BezPath {
    Rect::new(x0, y0, x1, y1).to_path(0.0)
}

/// Commands as text with ETX shown as `|`.
fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.iter().map(|b| if *b == 0x03 { b'|' } else { *b }).collect()).unwrap()
}

#[test]
fn golden_blade_square() {
    let job = encode(&[rect(10.0, 10.0, 20.0, 20.0)], &JobSettings::default()).unwrap();
    assert_eq!(
        text(&job.bytes),
        "FN0|TB50,0|FU6096,6096|\\0,0|Z6096,6096|&100,100,100|FC18|FY0|FX10|!5|\
         M200,200|D200,400|D400,400|D400,200|D200,200|&1,1,1|H|"
    );
    assert_eq!(job.stats.strokes, 1);
    assert!((job.stats.cut_mm - 40.0).abs() < 1e-9);
    assert!((job.stats.travel_mm - 200f64.sqrt()).abs() < 1e-9);
}

#[test]
fn golden_pen_tool_holder_feed_out() {
    let settings = JobSettings {
        media_mm: (210.0, 297.0),
        tool: ToolSettings { force: 7, speed: 3, tool_holder: Some(2), ..ToolSettings::pen() },
        track_enhancing: true,
        end: EndAction::FeedOut { mm: 300.0 },
        ..JobSettings::default()
    };
    let mut line = BezPath::new();
    line.move_to((5.0, 1.0));
    line.line_to((5.0, 2.5));
    let job = encode(&[line], &settings).unwrap();
    assert_eq!(
        text(&job.bytes),
        "J2|FN0|TB50,0|FU5940,4200|\\0,0|Z5940,4200|&100,100,100|FC0,1,2|FY1|FX7,2|!3,2|\
         M20,100|D50,100|&1,1,1|M6000,0|"
    );
}

#[test]
fn interpreter_reads_back_settings() {
    let settings = JobSettings {
        media_mm: (203.2, 304.8),
        tool: ToolSettings { force: 21, speed: 8, ..ToolSettings::blade() },
        ..JobSettings::default()
    };
    let sim = simulate(&encode(&[rect(1.0, 1.0, 2.0, 2.0)], &settings).unwrap().bytes);
    assert!(sim.errors.is_empty() && sim.unknown.is_empty() && sim.trailing.is_empty(), "{sim:?}");
    assert_eq!((sim.force, sim.speed, sim.blade_offset, sim.track_enhancing), (Some(21), Some(8), Some(18), Some(false)));
    let (w, h) = sim.media_mm.unwrap();
    assert!((w - 203.2).abs() < 0.05 && (h - 304.8).abs() < 0.05);
    assert!(sim.homed);
}

#[test]
fn inner_paths_are_cut_before_their_container() {
    // Outer frame listed first, nearer to the origin than the hole and the separate square.
    let paths = [rect(0.5, 0.5, 100.0, 100.0), rect(40.0, 40.0, 60.0, 60.0), rect(150.0, 10.0, 160.0, 20.0)];
    let sim = simulate(&encode(&paths, &JobSettings::default()).unwrap().bytes);
    assert_eq!(sim.strokes.len(), 3);
    let bbox = |s: &Vec<Point>| s.iter().fold(Rect::from_points(s[0], s[0]), |r, p| r.union_pt(*p));
    let order: Vec<Rect> = sim.strokes.iter().map(bbox).collect();
    let frame = order.iter().position(|r| r.width() > 90.0).unwrap();
    let hole = order.iter().position(|r| (r.x0 - 40.0).abs() < 0.1).unwrap();
    assert!(hole < frame, "hole must be cut before the frame: {order:?}");
    // Closed paths are entered at the vertex nearest the previous position.
    let first = sim.strokes[0][0];
    assert!(first.distance(Point::new(40.0, 40.0)) < 0.1 || first.distance(Point::new(150.0, 10.0)) < 0.1);
}

/// Largest distance from any simulated point to the flattened input.
fn max_deviation(paths: &[BezPath], strokes: &[Vec<Point>]) -> f64 {
    let segs: Vec<(Point, Point)> = paths
        .iter()
        .flat_map(|p| flatten(p, 0.001))
        .flat_map(|pl| {
            let mut pts = pl.points.clone();
            if pl.closed {
                pts.push(pts[0]);
            }
            pts.windows(2).map(|w| (w[0], w[1])).collect::<Vec<_>>()
        })
        .collect();
    let dist = |p: Point, (a, b): (Point, Point)| {
        let ab = b - a;
        let t = if ab.hypot2() == 0.0 { 0.0 } else { ((p - a).dot(ab) / ab.hypot2()).clamp(0.0, 1.0) };
        p.distance(a.lerp(b, t))
    };
    strokes
        .iter()
        .flatten()
        .map(|p| segs.iter().map(|s| dist(*p, *s)).fold(f64::INFINITY, f64::min))
        .fold(0.0, f64::max)
}

fn cut_length(strokes: &[Vec<Point>]) -> f64 {
    strokes.iter().flat_map(|s| s.windows(2).map(|w| w[0].distance(w[1]))).sum()
}

#[test]
fn roundtrip_curves_within_quantisation() {
    let mut wave = BezPath::new();
    wave.move_to((20.0, 150.0));
    wave.curve_to((60.0, 100.0), (100.0, 200.0), (140.0, 150.0));
    let paths = [Circle::new((80.0, 60.0), 30.0).to_path(1e-6), Circle::new((80.0, 60.0), 12.5).to_path(1e-6), wave];
    let settings = JobSettings::default();
    let job = encode(&paths, &settings).unwrap();
    let sim = simulate(&job.bytes);
    assert!(sim.errors.is_empty());
    assert_eq!(sim.strokes.len(), 3);
    // Unit rounding (0.025 mm per axis) plus flattening tolerance.
    let dev = max_deviation(&paths, &sim.strokes);
    assert!(dev <= 0.025 * 2f64.sqrt() + settings.tolerance_mm, "deviation {dev}");
    let expected: f64 = paths.iter().flat_map(|p| flatten(p, 0.001)).map(|pl| pl.len_mm()).sum();
    let got = cut_length(&sim.strokes);
    assert!((got - expected).abs() / expected < 0.002, "cut length {got} vs {expected}");
}

#[test]
fn passes_overcut_and_perforation() {
    let square = [rect(10.0, 10.0, 20.0, 20.0)];
    let s = JobSettings { tool: ToolSettings { passes: 3, overcut_mm: 2.0, ..ToolSettings::blade() }, ..JobSettings::default() };
    let sim = simulate(&encode(&square, &s).unwrap().bytes);
    assert_eq!(sim.strokes.len(), 3);
    for st in &sim.strokes {
        assert!((cut_length(std::slice::from_ref(st)) - 42.0).abs() < 1e-9);
    }
    let s = JobSettings { perforation: Some(Perforation { cut_mm: 3.0, gap_mm: 2.0 }), ..JobSettings::default() };
    let sim = simulate(&encode(&square, &s).unwrap().bytes);
    assert_eq!(sim.strokes.len(), 8);
    assert!((cut_length(&sim.strokes) - 24.0).abs() < 1e-9);
}

#[test]
fn off_media_handling() {
    let paths = [rect(-10.0, 10.0, 10.0, 20.0), rect(-50.0, 10.0, -40.0, 20.0), rect(50.0, 50.0, 60.0, 60.0)];
    let err = encode(&paths, &JobSettings::default()).unwrap_err();
    assert_eq!(err, EncodeError::OutOfBounds { count: 2, w: 304.8, h: 304.8 });
    let skip = JobSettings { off_media: OffMedia::SkipOutside, ..JobSettings::default() };
    assert_eq!(encode(&paths, &skip).unwrap_err(), EncodeError::OutOfBounds { count: 1, w: 304.8, h: 304.8 });
    let skip_ok = encode(&paths[1..], &skip).unwrap();
    assert_eq!(simulate(&skip_ok.bytes).strokes.len(), 1);
    let clip = JobSettings { off_media: OffMedia::Clip, ..JobSettings::default() };
    let sim = simulate(&encode(&paths, &clip).unwrap().bytes);
    assert_eq!(sim.strokes.len(), 2);
    assert!(sim.strokes.iter().flatten().all(|p| p.x >= 0.0 && p.y >= 0.0));
    // The clipped rectangle keeps its three on-media sides: 10 + 10 + 10 mm.
    assert!((cut_length(&sim.strokes) - 70.0).abs() < 1e-6);
}

#[test]
fn rejects_bad_settings() {
    let sq = [rect(1.0, 1.0, 2.0, 2.0)];
    for tool in [
        ToolSettings { force: 0, ..ToolSettings::blade() },
        ToolSettings { force: 34, ..ToolSettings::blade() },
        ToolSettings { speed: 11, ..ToolSettings::blade() },
        ToolSettings { passes: 0, ..ToolSettings::blade() },
    ] {
        assert!(matches!(encode(&sq, &JobSettings { tool, ..JobSettings::default() }), Err(EncodeError::InvalidSetting(_))));
    }
}

#[test]
fn interpreter_handles_batched_draws_and_beziers() {
    let sim = simulate(b"M0,0\x03D20,0,20,20\x03BZ0,100,100,100,200,200,200,200,100\x03\x1b\x05trail");
    assert!(sim.errors.is_empty(), "{:?}", sim.errors);
    assert_eq!(sim.strokes.len(), 2);
    assert_eq!(sim.strokes[0], vec![Point::new(0.0, 0.0), Point::new(0.0, 1.0), Point::new(1.0, 1.0)]);
    let end = *sim.strokes[1].last().unwrap();
    assert!(end.distance(Point::new(5.0, 10.0)) < 1e-9);
    assert_eq!(sim.commands.last().unwrap(), "ESC+05");
    assert_eq!(sim.trailing, b"trail");
    assert_eq!(simulate(b"D1\x03Mx,y\x03").errors.len(), 2);
}

/// Encodes a stock Silhouette sample that fits a 12×12 mat and checks the replayed cut.
#[test]
fn stock_sample_roundtrip() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../unpacked/app/Resources/Resources/SPEC_S/vector");
    let Ok(data) = std::fs::read(dir.join("Gift_Tag.studio3")) else {
        eprintln!("skipping: stock samples not found");
        return;
    };
    let file = ss_formats::studio::parse(&data).unwrap();
    let paths: Vec<BezPath> = file.shapes.iter().map(|s| s.to_bezpath()).collect();
    assert!(!paths.is_empty());
    let settings = JobSettings { off_media: OffMedia::Clip, ..JobSettings::default() };
    let job = encode(&paths, &settings).unwrap();
    let sim = simulate(&job.bytes);
    assert!(sim.errors.is_empty() && sim.unknown.is_empty());
    assert_eq!(sim.strokes.len(), job.stats.strokes);
    let dev = max_deviation(&paths, &sim.strokes);
    assert!(dev <= 0.025 * 2f64.sqrt() + settings.tolerance_mm, "deviation {dev}");
    eprintln!("Gift_Tag: {} shapes, {} strokes, {} bytes, cut {:.0} mm, travel {:.0} mm", paths.len(), sim.strokes.len(), job.bytes.len(), job.stats.cut_mm, job.stats.travel_mm);
}

/// Timing on a large real file: `SS_BENCH_FILE=<file.studio3> cargo test -p ss-gpgl -- --ignored`.
#[test]
#[ignore]
fn bench_large_file() {
    let Ok(path) = std::env::var("SS_BENCH_FILE") else { return };
    let file = ss_formats::studio::parse(&std::fs::read(path).unwrap()).unwrap();
    let paths: Vec<BezPath> = file.shapes.iter().map(|s| s.to_bezpath()).collect();
    let settings = JobSettings { off_media: OffMedia::Clip, media_mm: (3000.0, 3000.0), ..JobSettings::default() };
    let t = std::time::Instant::now();
    let job = encode(&paths, &settings).unwrap();
    eprintln!("BENCH {} shapes -> {} strokes, {} bytes in {:?}", paths.len(), job.stats.strokes, job.bytes.len(), t.elapsed());
}
