//! Cut job encoding: settings header, planned geometry, footer.

use ss_doc::{CutAction, Document, Page, RegMarks};
use ss_geom::flatten::{Polyline, flatten};
use ss_geom::{BezPath, FillRule, Point, Rect, Vec2};

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
    /// Carriage index on dual-carriage models. Adds `J<n>` and a tool suffix to `!`, `FX`, `FC`
    /// and `TC` (*public*, unverified on hardware).
    pub tool_holder: Option<u8>,
    /// AutoBlade depth setting (`0..=10`), emitted as `TC<n>` (*public*, unverified on hardware).
    pub blade_depth: Option<u32>,
}

impl ToolSettings {
    pub fn blade() -> Self {
        Self { tool: Tool::Blade, force: 10, speed: 5, passes: 1, blade_offset: 18, overcut_mm: 0.0, tool_holder: None, blade_depth: None }
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

/// Print & Cut registration (*public*, experimental): the cutter's optical sensor finds the
/// printed marks and measures all later coordinates from the top-left mark.
///
/// The command sequence follows inkscape-silhouette (`Graphtec.py`, `plot()`), cross-checked
/// against the `TB5x`/`TB123` literals in the original application. It has not been verified on
/// hardware by this project.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Registration {
    /// Outer corner of the top-left mark on the page, mm (x across, y down). After registration
    /// this point is the cutter origin.
    pub origin_mm: (f64, f64),
    /// Distance between the outer edges of the marks, mm (width across, height down).
    pub size_mm: (f64, f64),
    /// Arm length of the L marks, mm (`TB51`).
    pub mark_length_mm: f64,
    /// Line thickness of the marks, mm (`TB53`).
    pub mark_thickness_mm: f64,
    /// Let the cutter search for the marks (`TB123`); otherwise the user positions the tool over
    /// the first mark with the arrow keys (`TB23`).
    pub auto_search: bool,
}

impl Registration {
    /// How far before the first mark the automatic search starts, mm.
    pub const SEARCH_MARGIN_MM: f64 = 10.0;

    pub fn from_marks(marks: &RegMarks, page: &Page, auto_search: bool) -> Self {
        let a = marks.area(page);
        Self {
            origin_mm: (a.x0, a.y0),
            size_mm: (a.width(), a.height()),
            mark_length_mm: marks.length_mm,
            mark_thickness_mm: marks.thickness_mm,
            auto_search,
        }
    }

    /// The area the cutter can reach after registration, in page mm.
    pub fn area(&self) -> Rect {
        let (x, y) = self.origin_mm;
        Rect::new(x, y, x + self.size_mm.0, y + self.size_mm.1)
    }
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
    /// Print & Cut registration before cutting. Geometry must then lie inside the marks.
    pub registration: Option<Registration>,
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
            registration: None,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum EncodeError {
    #[error("{count} path(s) extend outside the {w:.1} × {h:.1} mm media")]
    OutOfBounds { count: usize, w: f64, h: f64 },
    #[error("invalid setting: {0}")]
    InvalidSetting(&'static str),
    #[error("{count} path(s) extend outside the registration marks")]
    OutsideRegistration { count: usize },
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
    /// With registration: length of the prefix ending in the mark search command. The sender
    /// must wait for the cutter's reply (see [`crate::parse_registration`]) before sending the
    /// rest.
    pub registration_end: Option<usize>,
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

fn validate_tool(t: &ToolSettings) -> Result<(), EncodeError> {
    if !(1..=33).contains(&t.force) {
        return Err(EncodeError::InvalidSetting("force must be 1..=33"));
    }
    if !(1..=10).contains(&t.speed) {
        return Err(EncodeError::InvalidSetting("speed must be 1..=10"));
    }
    if t.passes == 0 {
        return Err(EncodeError::InvalidSetting("passes must be at least 1"));
    }
    if !(t.overcut_mm.is_finite() && t.overcut_mm >= 0.0) {
        return Err(EncodeError::InvalidSetting("overcut must not be negative"));
    }
    if t.blade_depth.is_some_and(|d| d > 10) {
        return Err(EncodeError::InvalidSetting("blade depth must be 0..=10"));
    }
    Ok(())
}

fn validate_perforation(p: Option<Perforation>) -> Result<(), EncodeError> {
    if let Some(p) = p
        && !(p.cut_mm > 0.0 && p.gap_mm > 0.0)
    {
        return Err(EncodeError::InvalidSetting("perforation lengths must be positive"));
    }
    Ok(())
}

fn validate(s: &JobSettings) -> Result<(), EncodeError> {
    let (w, h) = s.media_mm;
    if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
        return Err(EncodeError::InvalidSetting("media size"));
    }
    validate_tool(&s.tool)?;
    if !(s.tolerance_mm.is_finite() && s.tolerance_mm > 0.0) {
        return Err(EncodeError::InvalidSetting("tolerance must be positive"));
    }
    validate_perforation(s.perforation)?;
    if let Some(r) = s.registration {
        let vals = [r.origin_mm.0, r.origin_mm.1, r.size_mm.0, r.size_mm.1, r.mark_length_mm, r.mark_thickness_mm];
        if !vals.iter().all(|v| v.is_finite()) || r.origin_mm.0 < 0.0 || r.origin_mm.1 < 0.0 {
            return Err(EncodeError::InvalidSetting("registration origin must be on the media"));
        }
        if r.size_mm.0 <= 0.0 || r.size_mm.1 <= 0.0 || r.mark_length_mm <= 0.0 || r.mark_thickness_mm <= 0.0 {
            return Err(EncodeError::InvalidSetting("registration mark sizes must be positive"));
        }
        let a = r.area();
        if a.x1 > w + 1e-6 || a.y1 > h + 1e-6 {
            return Err(EncodeError::InvalidSetting("registration marks extend past the media"));
        }
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

/// One ordered stage in a multi-group job (e.g. a Line Color, Fill Color, or Layer row).
#[derive(Debug, Clone, PartialEq)]
pub struct JobStage {
    pub paths: Vec<BezPath>,
    pub tool: ToolSettings,
    pub perforation: Option<Perforation>,
    /// Pause after this stage by returning the carriage home before the next stage.
    pub pause_after: bool,
}

fn prepare_lines(paths: &[BezPath], s: &JobSettings, area: Rect, media: Rect) -> Result<Vec<Polyline>, EncodeError> {
    let out_of_bounds = |count: usize| match s.registration {
        Some(_) => EncodeError::OutsideRegistration { count },
        None => EncodeError::OutOfBounds { count, w: media.x1, h: media.y1 },
    };
    let mut lines: Vec<Polyline> = paths.iter().flat_map(|p| flatten(p, s.tolerance_mm)).collect();
    match s.off_media {
        OffMedia::Error => {
            let count = lines.iter().filter(|l| !l.points.iter().all(|p| inside(area, *p))).count();
            if count > 0 {
                return Err(out_of_bounds(count));
            }
        }
        OffMedia::SkipOutside => {
            lines.retain(|l| l.points.iter().any(|p| inside(area, *p)));
            let count = lines.iter().filter(|l| !l.points.iter().all(|p| inside(area, *p))).count();
            if count > 0 {
                return Err(out_of_bounds(count));
            }
        }
        OffMedia::Clip => lines = lines.iter().flat_map(|l| clip_polyline(l, area)).collect(),
    }
    if s.optimize_order {
        lines = plan::order(lines, area.origin());
    }
    if s.registration.is_some() {
        let shift = Vec2::new(-area.x0, -area.y0);
        for l in &mut lines {
            l.points.iter_mut().for_each(|p| *p += shift);
        }
    }
    Ok(lines)
}

fn emit_tool_settings(w: &mut Writer, t: &ToolSettings) {
    let suffix = t.tool_holder.map(|n| format!(",{n}")).unwrap_or_default();
    if let Some(n) = t.tool_holder {
        w.cmd(&format!("J{n}"));
    }
    let offset = if t.tool == Tool::Pen { 0 } else { t.blade_offset };
    match t.tool_holder {
        Some(n) => w.cmd(&format!("FC{offset},1,{n}")),
        None => w.cmd(&format!("FC{offset}")),
    }
    if let Some(d) = t.blade_depth {
        w.cmd(&format!("TC{d}{suffix}"));
    }
    w.cmd(&format!("FX{}{suffix}", t.force));
    w.cmd(&format!("!{}{suffix}", t.speed));
}

fn emit_header(w: &mut Writer, media: Rect, s: &JobSettings, initial_tool: &ToolSettings) -> Option<usize> {
    let (mh, mw) = yx(Point::new(media.x1, media.y1));
    if let Some(n) = initial_tool.tool_holder {
        w.cmd(&format!("J{n}"));
    }
    w.cmd("FN0");
    w.cmd("TB50,0");
    w.cmd(&format!("FU{mh},{mw}"));
    w.cmd("\\0,0");
    w.cmd(&format!("Z{mh},{mw}"));
    w.cmd("&100,100,100");
    let offset = if initial_tool.tool == Tool::Pen { 0 } else { initial_tool.blade_offset };
    match initial_tool.tool_holder {
        Some(n) => w.cmd(&format!("FC{offset},1,{n}")),
        None => w.cmd(&format!("FC{offset}")),
    }
    w.cmd(&format!("FY{}", u8::from(s.track_enhancing)));
    let suffix = initial_tool.tool_holder.map(|n| format!(",{n}")).unwrap_or_default();
    if let Some(d) = initial_tool.blade_depth {
        w.cmd(&format!("TC{d}{suffix}"));
    }
    w.cmd(&format!("FX{}{suffix}", initial_tool.force));
    w.cmd(&format!("!{}{suffix}", initial_tool.speed));
    let mut registration_end = None;
    if let Some(r) = s.registration {
        w.cmd("TB50,0");
        w.cmd("TB99");
        // Mark type 2: Cameo / Portrait style (square plus two L brackets).
        w.cmd("TB52,2");
        w.cmd(&format!("TB51,{}", to_units(r.mark_length_mm)));
        w.cmd(&format!("TB53,{}", to_units(r.mark_thickness_mm)));
        w.cmd("TB55,1");
        let (h, wd) = (to_units(r.size_mm.1), to_units(r.size_mm.0));
        if r.auto_search {
            let top = to_units((r.origin_mm.1 - Registration::SEARCH_MARGIN_MM).max(0.0));
            let left = to_units((r.origin_mm.0 - Registration::SEARCH_MARGIN_MM).max(0.0));
            w.cmd(&format!("TB123,{h},{wd},{top},{left}"));
        } else {
            w.cmd(&format!("TB23,{h},{wd}"));
        }
        registration_end = Some(w.buf.len());
    }
    registration_end
}

fn emit_lines(w: &mut Writer, lines: &[Polyline], t: &ToolSettings, perf: Option<Perforation>, pos: &mut Point, stats: &mut JobStats) {
    let mut emit = |w: &mut Writer, pts: &[Point]| {
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
        *pos = *pts.last().unwrap_or(&first);
    };
    for line in lines {
        let pts = polyline_points(line, t.overcut_mm);
        for _ in 0..t.passes {
            match perf {
                Some(p) => {
                    for dash in dashes(&pts, p.cut_mm, p.gap_mm) {
                        emit(w, &dash);
                    }
                }
                None => emit(w, &pts),
            }
        }
    }
}

fn emit_footer(w: &mut Writer, s: &JobSettings) {
    w.cmd("&1,1,1");
    match s.end {
        EndAction::Home => w.cmd("H"),
        EndAction::FeedOut { mm } => w.cmd(&format!("M{},0", to_units(mm.max(0.0)))),
        EndAction::Stay => {}
    }
}

/// Encodes `paths` (page millimetres, y down, origin at the media's top-left corner).
///
/// With [`JobSettings::registration`], geometry must lie inside the marks (the off-media policy
/// applies to the mark area) and is written relative to the top-left mark.
pub fn encode(paths: &[BezPath], s: &JobSettings) -> Result<Job, EncodeError> {
    validate(s)?;
    let media = Rect::new(0.0, 0.0, s.media_mm.0, s.media_mm.1);
    let area = s.registration.map_or(media, |r| r.area());
    let lines = prepare_lines(paths, s, area, media)?;
    let mut w = Writer { buf: Vec::new(), commands: 0 };
    let registration_end = emit_header(&mut w, media, s, &s.tool);
    let mut stats = JobStats::default();
    let mut pos = Point::ZERO;
    emit_lines(&mut w, &lines, &s.tool, s.perforation, &mut pos, &mut stats);
    emit_footer(&mut w, s);
    stats.commands = w.commands;
    Ok(Job { bytes: w.buf, stats, registration_end })
}

/// Encodes multiple ordered [`JobStage`]s into a single GPGL job, switching tool settings (and
/// optionally pausing at home) between stages.
pub fn encode_stages(stages: &[JobStage], s: &JobSettings) -> Result<Job, EncodeError> {
    validate(s)?;
    for st in stages {
        validate_tool(&st.tool)?;
        validate_perforation(st.perforation)?;
    }
    let media = Rect::new(0.0, 0.0, s.media_mm.0, s.media_mm.1);
    let area = s.registration.map_or(media, |r| r.area());
    let mut prepared = Vec::with_capacity(stages.len());
    for st in stages {
        let lines = prepare_lines(&st.paths, s, area, media)?;
        if !lines.is_empty() {
            prepared.push((st, lines));
        }
    }
    let initial_tool = prepared.first().map_or(&s.tool, |(st, _)| &st.tool);
    let mut w = Writer { buf: Vec::new(), commands: 0 };
    let registration_end = emit_header(&mut w, media, s, initial_tool);
    let mut stats = JobStats::default();
    let mut pos = Point::ZERO;
    for (idx, (st, lines)) in prepared.iter().enumerate() {
        if idx > 0 {
            emit_tool_settings(&mut w, &st.tool);
        }
        emit_lines(&mut w, lines, &st.tool, st.perforation, &mut pos, &mut stats);
        if st.pause_after && idx + 1 < prepared.len() {
            w.cmd("H");
            pos = Point::ZERO;
        }
    }
    emit_footer(&mut w, s);
    stats.commands = w.commands;
    Ok(Job { bytes: w.buf, stats, registration_end })
}

/// Welds `paths` and keeps only the outer boundaries (the Silhouette "Cut Edge" rule).
pub fn cut_edge_paths(paths: &[BezPath]) -> Vec<BezPath> {
    if paths.is_empty() {
        return Vec::new();
    }
    let welded = ss_geom::boolean::weld(paths, FillRule::EvenOdd, ss_geom::DEFAULT_TOLERANCE);
    let outer: Vec<_> = welded.into_iter().filter_map(|poly| poly.into_iter().next()).map(|c| vec![c]).collect();
    if outer.is_empty() { Vec::new() } else { vec![ss_geom::boolean::polygons_to_path(&outer, None)] }
}

/// Outlines of the visible objects whose cut action is in `actions`, in page millimetres.
/// `CutEdge` objects contribute only the outer boundary of their welded outline. Images and the
/// background never produce cut paths.
pub fn document_paths(doc: &Document, actions: &[CutAction]) -> Vec<BezPath> {
    let mut out = Vec::new();
    for o in doc.visible_objects().filter(|o| actions.contains(&o.cut)) {
        let paths = o.cut_paths();
        if paths.is_empty() {
            continue;
        }
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
    fn images_never_produce_cut_paths() {
        let mut doc = Document::default();
        let l = doc.default_layer();
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR\0\0\0\x02\0\0\0\x02".to_vec();
        let img = ss_doc::ImageData::from_png(png, Rect::new(0.0, 0.0, 10.0, 10.0)).unwrap();
        let mut o = ss_doc::Object::image("Art", l, img);
        // Even when someone sets the image to cut, it has no outline to cut.
        o.cut = CutAction::Cut;
        doc.objects.push(o);
        doc.objects.push(ss_doc::Object::path("sq", l, ss_geom::Shape::to_path(&Rect::new(1.0, 1.0, 2.0, 2.0), 0.0)));
        assert_eq!(document_paths(&doc, &[CutAction::Cut, CutAction::CutEdge]).len(), 1);
    }

    #[test]
    fn overcut_extends_closed_paths() {
        let pl = Polyline { points: vec![Point::new(0.0, 0.0), Point::new(10.0, 0.0), Point::new(10.0, 10.0)], closed: true };
        let pts = polyline_points(&pl, 3.0);
        assert_eq!(pts.last(), Some(&Point::new(3.0, 0.0)));
    }

    #[test]
    fn multi_stage_job_switches_tool_settings_blade_depth_and_pauses() {
        let mut p1 = BezPath::new();
        p1.move_to((10.0, 10.0));
        p1.line_to((30.0, 10.0));
        let mut p2 = BezPath::new();
        p2.move_to((10.0, 20.0));
        p2.line_to((30.0, 20.0));

        let stages = vec![
            JobStage {
                paths: vec![p1],
                tool: ToolSettings {
                    tool: Tool::Blade,
                    force: 22,
                    speed: 4,
                    passes: 2,
                    blade_offset: 18,
                    overcut_mm: 0.0,
                    tool_holder: Some(1),
                    blade_depth: Some(5),
                },
                perforation: None,
                pause_after: true,
            },
            JobStage {
                paths: vec![p2],
                tool: ToolSettings {
                    tool: Tool::Pen,
                    force: 8,
                    speed: 7,
                    passes: 1,
                    blade_offset: 0,
                    overcut_mm: 0.0,
                    tool_holder: Some(2),
                    blade_depth: None,
                },
                perforation: None,
                pause_after: false,
            },
        ];
        let job = encode_stages(&stages, &JobSettings::default()).unwrap();
        let sim = crate::simulate(&job.bytes);
        assert!(sim.errors.is_empty());
        assert!(sim.unknown.is_empty());
        assert_eq!(sim.strokes.len(), 3); // 2 passes for stage 1 + 1 pass for stage 2
        assert_eq!(sim.force, Some(8));
        assert_eq!(sim.speed, Some(7));
        assert_eq!(sim.blade_depth, Some(5));
        assert_eq!(sim.tool_holder, Some(2));
        assert_eq!(sim.homes, 2); // 1 pause between stages + 1 EndAction::Home
    }

    #[test]
    fn perforated_stage_emits_dashed_strokes() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((20.0, 0.0));
        let stages = vec![JobStage {
            paths: vec![p],
            tool: ToolSettings::blade(),
            perforation: Some(Perforation { cut_mm: 2.0, gap_mm: 2.0 }),
            pause_after: false,
        }];
        let job = encode_stages(&stages, &JobSettings::default()).unwrap();
        let sim = crate::simulate(&job.bytes);
        assert!(sim.errors.is_empty());
        assert_eq!(sim.strokes.len(), 5);
    }
}
