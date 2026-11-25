//! Cut job encoding: settings header, planned geometry, footer.

use ss_doc::{CutAction, Document};
use ss_geom::flatten::{Polyline, flatten};
use ss_geom::{BezPath, FillRule, Point, Rect};

use crate::{ETX, plan, to_units};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    #[default]
    Blade,
    /// Sketch pen: no blade offset compensation (`FC0`).
    Pen,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolSettings {
    pub tool: Tool,
    /// Blade force, 1..=33.
    pub force: u32,
    /// Speed, 1..=10.
    pub speed: u32,
    /// How many times each path is cut.
    pub passes: u32,
    /// Blade offset in cutter units (`FC`); 18 for the standard blade, ignored for the pen.
    pub blade_offset: u32,
    /// Extra cut past the start point of closed paths, in mm.
    pub overcut_mm: f64,
    /// Carriage index on dual-carriage models. Adds `J<n>` and a tool suffix to `!`, `FX` and `FC`
    /// (*public*, unverified on hardware).
    pub tool_holder: Option<u8>,
}

impl ToolSettings {
    pub fn blade() -> Self {
        Self { tool: Tool::Blade, force: 10, speed: 5, passes: 1, blade_offset: 18, overcut_mm: 0.0, tool_holder: None }
    }

    pub fn pen() -> Self {
        Self { tool: Tool::Pen, blade_offset: 0, ..Self::blade() }
    }
}

impl Default for ToolSettings {
    fn default() -> Self {
        Self::blade()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EndAction {
    /// Return the carriage to the origin (`H`).
    Home,
    /// Move to the given distance down the media and stop there.
    FeedOut { mm: f64 },
    /// Leave the carriage where the job ends.
    Stay,
}

/// What to do with geometry outside the media.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OffMedia {
    /// Refuse to encode the job.
    #[default]
    Error,
    /// Drop paths that lie completely outside; refuse paths that cross the edge.
    SkipOutside,
    /// Cut only the parts inside the media.
    Clip,
}

/// Dashed cutting for perforation lines.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Perforation {
    pub cut_mm: f64,
    pub gap_mm: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JobSettings {
    /// Media width (across the carriage) and height (feed direction) in mm.
    pub media_mm: (f64, f64),
    pub tool: ToolSettings,
    /// Grip the media harder while feeding (`FY`).
    pub track_enhancing: bool,
    pub end: EndAction,
    /// Curve flattening tolerance in mm.
    pub tolerance_mm: f64,
    /// Cut inner paths before the paths that enclose them and shorten travel.
    pub optimize_order: bool,
    pub off_media: OffMedia,
    pub perforation: Option<Perforation>,
}

impl Default for JobSettings {
    fn default() -> Self {
        Self {
            media_mm: (304.8, 304.8),
            tool: ToolSettings::blade(),
            track_enhancing: false,
            end: EndAction::Home,
            tolerance_mm: 0.02,
            optimize_order: true,
            off_media: OffMedia::Error,
            perforation: None,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum EncodeError {
    #[error("{count} path(s) extend outside the {w:.1} × {h:.1} mm media")]
    OutOfBounds { count: usize, w: f64, h: f64 },
    #[error("invalid setting: {0}")]
    InvalidSetting(&'static str),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct JobStats {
    /// Polylines cut, counting each pass and each perforation dash.
    pub strokes: usize,
    pub cut_mm: f64,
    pub travel_mm: f64,
    pub commands: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub bytes: Vec<u8>,
    pub stats: JobStats,
}

struct Writer {
    buf: Vec<u8>,
    commands: usize,
}

impl Writer {
    fn cmd(&mut self, s: &str) {
        self.buf.extend_from_slice(s.as_bytes());
        self.buf.push(ETX);
        self.commands += 1;
    }
}

/// Unit-space position written as GPGL `y,x`.
fn yx(p: Point) -> (i64, i64) {
    (to_units(p.y), to_units(p.x))
}

fn validate(s: &JobSettings) -> Result<(), EncodeError> {
    let (w, h) = s.media_mm;
    if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
        return Err(EncodeError::InvalidSetting("media size"));
    }
    if !(1..=33).contains(&s.tool.force) {
        return Err(EncodeError::InvalidSetting("force must be 1..=33"));
    }
    if !(1..=10).contains(&s.tool.speed) {
        return Err(EncodeError::InvalidSetting("speed must be 1..=10"));
    }
    if s.tool.passes == 0 {
        return Err(EncodeError::InvalidSetting("passes must be at least 1"));
    }
    if !(s.tolerance_mm.is_finite() && s.tolerance_mm > 0.0) {
        return Err(EncodeError::InvalidSetting("tolerance must be positive"));
    }
    if !(s.tool.overcut_mm.is_finite() && s.tool.overcut_mm >= 0.0) {
        return Err(EncodeError::InvalidSetting("overcut must not be negative"));
    }
    if let Some(p) = s.perforation
        && !(p.cut_mm > 0.0 && p.gap_mm > 0.0)
    {
        return Err(EncodeError::InvalidSetting("perforation lengths must be positive"));
    }
    Ok(())
}

fn inside(r: Rect, p: Point) -> bool {
    const EPS: f64 = 1e-6;
    p.x >= r.x0 - EPS && p.x <= r.x1 + EPS && p.y >= r.y0 - EPS && p.y <= r.y1 + EPS
}

/// Liang–Barsky: parameter range of segment `a→b` inside `r`.
fn clip_segment(a: Point, b: Point, r: Rect) -> Option<(f64, f64)> {
    let d = b - a;
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (p, q) in [(-d.x, a.x - r.x0), (d.x, r.x1 - a.x), (-d.y, a.y - r.y0), (d.y, r.y1 - a.y)] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                t0 = t0.max(t);
            } else {
                t1 = t1.min(t);
            }
        }
    }
    (t0 < t1).then_some((t0, t1))
}

fn clip_polyline(pl: &Polyline, r: Rect) -> Vec<Polyline> {
    if pl.points.iter().all(|p| inside(r, *p)) {
        return vec![pl.clone()];
    }
    let mut pts = pl.points.clone();
    if pl.closed {
        pts.push(pts[0]);
    }
    let mut out = Vec::new();
    let mut cur: Vec<Point> = Vec::new();
    let flush = |cur: &mut Vec<Point>, out: &mut Vec<Polyline>| {
        if cur.len() >= 2 {
            out.push(Polyline { points: std::mem::take(cur), closed: false });
        }
        cur.clear();
    };
    for w in pts.windows(2) {
        match clip_segment(w[0], w[1], r) {
            Some((t0, t1)) => {
                let a = w[0].lerp(w[1], t0);
                let b = w[0].lerp(w[1], t1);
                if t0 > 0.0 {
                    flush(&mut cur, &mut out);
                }
                if cur.is_empty() {
                    cur.push(a);
                }
                cur.push(b);
                if t1 < 1.0 {
                    flush(&mut cur, &mut out);
                }
            }
            None => flush(&mut cur, &mut out),
        }
    }
    flush(&mut cur, &mut out);
    out
}

/// Points along `pts` (already including the closing point) from the start for `len` mm.
fn prefix(pts: &[Point], len: f64) -> Vec<Point> {
    let mut out = Vec::new();
    let mut left = len;
    for w in pts.windows(2) {
        let d = w[0].distance(w[1]);
        if d <= 0.0 {
            continue;
        }
        if d >= left {
            out.push(w[0].lerp(w[1], left / d));
            return out;
        }
        out.push(w[1]);
        left -= d;
    }
    out
}

/// Splits a polyline into dashes of `cut` mm separated by gaps of `gap` mm.
fn dashes(pts: &[Point], cut: f64, gap: f64) -> Vec<Vec<Point>> {
    let mut out = Vec::new();
    let mut cur: Vec<Point> = Vec::new();
    let mut down = true;
    let mut left = cut;
    if let Some(&p) = pts.first() {
        cur.push(p);
    }
    for w in pts.windows(2) {
        let (mut a, b) = (w[0], w[1]);
        let mut d = a.distance(b);
        while d > 0.0 {
            if d < left {
                if down {
                    cur.push(b);
                }
                left -= d;
                break;
            }
            let m = a.lerp(b, left / d);
            if down {
                cur.push(m);
                out.push(std::mem::take(&mut cur));
            } else {
                cur = vec![m];
            }
            d -= left;
            a = m;
            down = !down;
            left = if down { cut } else { gap };
        }
    }
    if down && cur.len() >= 2 {
        out.push(cur);
    }
    out
}

fn polyline_points(pl: &Polyline, overcut: f64) -> Vec<Point> {
    let mut pts = pl.points.clone();
    if pl.closed && pts.len() > 2 {
        pts.push(pts[0]);
        if overcut > 0.0 {
            let extra = prefix(&pts, overcut);
            pts.extend(extra);
        }
    }
    pts
}

/// Encodes `paths` (page millimetres, y down, origin at the media's top-left corner).
pub fn encode(paths: &[BezPath], s: &JobSettings) -> Result<Job, EncodeError> {
    validate(s)?;
    let media = Rect::new(0.0, 0.0, s.media_mm.0, s.media_mm.1);
    let mut lines: Vec<Polyline> = paths.iter().flat_map(|p| flatten(p, s.tolerance_mm)).collect();
    match s.off_media {
        OffMedia::Error => {
            let count = lines.iter().filter(|l| !l.points.iter().all(|p| inside(media, *p))).count();
            if count > 0 {
                return Err(EncodeError::OutOfBounds { count, w: media.x1, h: media.y1 });
            }
        }
        OffMedia::SkipOutside => {
            lines.retain(|l| l.points.iter().any(|p| inside(media, *p)));
            let count = lines.iter().filter(|l| !l.points.iter().all(|p| inside(media, *p))).count();
            if count > 0 {
                return Err(EncodeError::OutOfBounds { count, w: media.x1, h: media.y1 });
            }
        }
        OffMedia::Clip => lines = lines.iter().flat_map(|l| clip_polyline(l, media)).collect(),
    }
    if s.optimize_order {
        lines = plan::order(lines, Point::ZERO);
    }

    let mut w = Writer { buf: Vec::new(), commands: 0 };
    let t = &s.tool;
    let suffix = t.tool_holder.map(|n| format!(",{n}")).unwrap_or_default();
    let (mh, mw) = yx(Point::new(media.x1, media.y1));
    if let Some(n) = t.tool_holder {
        w.cmd(&format!("J{n}"));
    }
    w.cmd("FN0");
    w.cmd("TB50,0");
    w.cmd(&format!("FU{mh},{mw}"));
    w.cmd("\\0,0");
    w.cmd(&format!("Z{mh},{mw}"));
    w.cmd("&100,100,100");
    let offset = if t.tool == Tool::Pen { 0 } else { t.blade_offset };
    match t.tool_holder {
        Some(n) => w.cmd(&format!("FC{offset},1,{n}")),
        None => w.cmd(&format!("FC{offset}")),
    }
    w.cmd(&format!("FY{}", u8::from(s.track_enhancing)));
    w.cmd(&format!("FX{}{suffix}", t.force));
    w.cmd(&format!("!{}{suffix}", t.speed));

    let mut stats = JobStats::default();
    let mut pos = Point::ZERO;
    let mut emit = |w: &mut Writer, pts: &[Point], stats: &mut JobStats| {
        let Some(&first) = pts.first() else { return };
        stats.travel_mm += pos.distance(first);
        let mut last = yx(first);
        w.cmd(&format!("M{},{}", last.0, last.1));
        for win in pts.windows(2) {
            stats.cut_mm += win[0].distance(win[1]);
            let u = yx(win[1]);
            if u != last {
                w.cmd(&format!("D{},{}", u.0, u.1));
                last = u;
            }
        }
        stats.strokes += 1;
        pos = *pts.last().unwrap_or(&first);
    };
    for line in &lines {
        let pts = polyline_points(line, t.overcut_mm);
        for _ in 0..t.passes {
            match s.perforation {
                Some(p) => {
                    for dash in dashes(&pts, p.cut_mm, p.gap_mm) {
                        emit(&mut w, &dash, &mut stats);
                    }
                }
                None => emit(&mut w, &pts, &mut stats),
            }
        }
    }

    w.cmd("&1,1,1");
    match s.end {
        EndAction::Home => w.cmd("H"),
        EndAction::FeedOut { mm } => w.cmd(&format!("M{},0", to_units(mm.max(0.0)))),
        EndAction::Stay => {}
    }
    stats.commands = w.commands;
    Ok(Job { bytes: w.buf, stats })
}

/// Outlines of the visible objects whose cut action is in `actions`, in page millimetres.
/// `CutEdge` objects contribute only the outer boundary of their welded outline.
pub fn document_paths(doc: &Document, actions: &[CutAction]) -> Vec<BezPath> {
    let mut out = Vec::new();
    for o in doc.visible_objects().filter(|o| actions.contains(&o.cut)) {
        let paths = o.page_paths();
        if o.cut == CutAction::CutEdge {
            let welded = ss_geom::boolean::weld(&paths, FillRule::EvenOdd, ss_geom::DEFAULT_TOLERANCE);
            let outer: Vec<_> = welded.into_iter().filter_map(|poly| poly.into_iter().next()).map(|c| vec![c]).collect();
            out.push(ss_geom::boolean::polygons_to_path(&outer, None));
        } else {
            out.extend(paths);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_splits_crossing_segments() {
        let pl = Polyline { points: vec![Point::new(-5.0, 5.0), Point::new(5.0, 5.0), Point::new(5.0, 15.0)], closed: false };
        let parts = clip_polyline(&pl, Rect::new(0.0, 0.0, 10.0, 10.0));
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].points, vec![Point::new(0.0, 5.0), Point::new(5.0, 5.0), Point::new(5.0, 10.0)]);
    }

    #[test]
    fn dash_lengths() {
        let d = dashes(&[Point::new(0.0, 0.0), Point::new(10.0, 0.0)], 2.0, 1.0);
        let starts: Vec<f64> = d.iter().map(|s| s[0].x).collect();
        assert_eq!(starts, vec![0.0, 3.0, 6.0, 9.0]);
        assert!(d.iter().take(3).all(|s| (s.last().unwrap().x - s[0].x - 2.0).abs() < 1e-9));
        assert!((d[3].last().unwrap().x - 10.0).abs() < 1e-9);
    }

    #[test]
    fn overcut_extends_closed_paths() {
        let pl = Polyline { points: vec![Point::new(0.0, 0.0), Point::new(10.0, 0.0), Point::new(10.0, 10.0)], closed: true };
        let pts = polyline_points(&pl, 3.0);
        assert_eq!(pts.last(), Some(&Point::new(3.0, 0.0)));
    }
}
