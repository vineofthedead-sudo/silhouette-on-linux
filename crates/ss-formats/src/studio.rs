//! Silhouette Studio `.studio` (`silhouette04;`) and `.studio3` (`silhouette05;`) reader.
//!
//! Layout, as verified against the stock samples and real user files:
//!
//! * `.studio3`: 13-byte magic, u64 section sizes from `0x10` up to `0x3e8`, the saving version at
//!   `0x3e8` (decimal packed, `500000079` = 5.0.79), sections back to back from `0x400`.
//!   Section 0 is the document, section 2 the PNG preview. Studio 3.x/4.x writers declare the
//!   last section 4 bytes past the end of the file; sections that do not fit are dropped.
//! * `.studio`: the document section on its own (magic `silhouette04;`) plus a short trailer.
//!
//! The document is a tree of size-prefixed blocks (`0x01`, u32 size counting itself, body).
//! A shape record is `0x00, block A (properties + local outline), 65-byte transform
//! (3×3 row-major f32 matrix + extents), block P (outline in page millimetres), 0x01 u64 serial,
//! block C, [u32 len + UTF-8 name]` (the name is absent in v4). Raw bytes between blocks can
//! look like block headers, so every candidate offset is tried and a record is accepted only
//! when its P block parses as a path that consumes the block exactly.

use std::collections::{HashMap, HashSet};

use ss_geom::{Affine, BezPath, Point, Vec2};

pub const MAGIC_V4: &[u8; 13] = b"silhouette04;";
pub const MAGIC_V5: &[u8; 13] = b"silhouette05;";
const V5_HEADER_LEN: usize = 0x400;
const V5_TABLE_END: usize = 0x3e8;
const TRANSFORM_LEN: usize = 65;
const MAX_NAME_LEN: u32 = 200;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum StudioError {
    #[error("not a Silhouette Studio file")]
    NotStudio,
    #[error("unsupported Silhouette Studio format {0:?} (only silhouette04 and silhouette05 are supported)")]
    Unsupported(String),
    #[error("file is truncated")]
    Truncated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StudioVersion {
    /// `.studio`, `silhouette04;`
    V4,
    /// `.studio3`, `silhouette05;`
    V5,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StudioPoint {
    pub x: f32,
    pub y: f32,
    /// Incoming control point, relative to (x, y).
    pub handle_in: Option<(f32, f32)>,
    /// Outgoing control point, relative to (x, y).
    pub handle_out: Option<(f32, f32)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StudioSubpath {
    pub closed: bool,
    pub points: Vec<StudioPoint>,
}

/// A raster from the image table (section 1 of a `.studio3`). Shapes use these as image or
/// pattern fills.
#[derive(Debug, Clone, PartialEq)]
pub struct StudioImage {
    /// Content id, usually an MD5 hex string, referenced from shape properties.
    pub id: String,
    pub name: String,
    pub width_px: u32,
    pub height_px: u32,
    pub png: ss_doc::SharedBytes,
}

/// A shape whose fill is an image from the table.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageFill {
    pub image_id: String,
    /// The shape's local outline (inside block A). Placed images use the unit square
    /// `(-1,-1)..(1,1)`; mapping it onto the page outline gives the exact placement, including the
    /// transforms of enclosing groups.
    pub local: Option<Vec<StudioSubpath>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StudioFill {
    Solid([f32; 4]),
    Gradient(Vec<(f32, [f32; 4])>),
    Pattern(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct StudioShape {
    pub name: Option<String>,
    pub serial: u64,
    /// Row-major 3×3 matrix mapping the local outline (block A) into the parent's space.
    pub matrix: [f32; 9],
    /// Outline in page millimetres (block P).
    pub outline: Vec<StudioSubpath>,
    /// `key=value;` effect parameters found in block A, e.g. `...;effect=puncher`.
    pub effects: Vec<String>,
    /// Byte range of block A within the document section.
    pub a_range: (usize, usize),
    /// Offset of block P within the document section.
    pub p_offset: usize,
    pub image_fill: Option<ImageFill>,
    pub stroke: Option<[f32; 4]>,
    pub stroke_width_mm: f32,
    pub cut_id: Option<String>,
    pub fill: Option<StudioFill>,
    pub layer_name: Option<String>,
    /// Matrices stored after a pattern-fill reference, see [`PatternMatrices`].
    pub pattern: Option<PatternMatrices>,
}

/// The two row-major 3×3 matrices stored after a pattern reference in block A.
///
/// `uv` stretches the image over the bounding box of the local outline
/// (`[w/width_px, 0, x0; 0, h/height_px, y0]`); `adjust` holds the user's pattern scale and
/// rotation. Checked against Silhouette Studio's own SVG export, the tile drawn is the image scaled
/// uniformly by `max(uv[0], uv[4])`, flipped vertically, transformed by `adjust` and centred on
/// that bounding box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PatternMatrices {
    /// Tiled pattern; otherwise a picture stretched by `uv` alone (clipped to the outline).
    pub tiled: bool,
    pub uv: [f32; 9],
    pub adjust: [f32; 9],
}

impl PatternMatrices {
    /// Maps image pixels to the shape's local coordinates.
    pub fn local_affine(&self, width_px: u32, height_px: u32) -> Affine {
        let u = self.uv.map(f64::from);
        let a = self.adjust.map(f64::from);
        if !self.tiled {
            return Affine::new([u[0], u[3], u[1], u[4], u[2], u[5]]);
        }
        let (w, h) = (f64::from(width_px), f64::from(height_px));
        let centre = Vec2::new(u[2] + u[0] * w / 2.0, u[5] + u[4] * h / 2.0);
        let adjust = Affine::new([a[0], a[3], a[1], a[4], 0.0, 0.0]);
        Affine::translate(centre)
            * Affine::scale(u[0].max(u[4]))
            * Affine::FLIP_Y
            * adjust
            * Affine::translate(Vec2::new(-w / 2.0, -h / 2.0))
    }
}

/// Reads [`PatternMatrices`] after the reference to `id`:
/// `id, 10 bytes, tiled u8, 00 08 00 04, 3 f32, 3 bytes, 01, adjust[9], 01, uv[9]`.
fn parse_pattern_matrices(a: &[u8], id: &str) -> Option<PatternMatrices> {
    let at = a.windows(id.len()).position(|w| w == id.as_bytes())? + id.len();
    let q = at + 10;
    let tiled = match *a.get(q)? {
        0 => false,
        1 => true,
        _ => return None,
    };
    if a.get(q + 1..q + 5)? != [0, 8, 0, 4] || *a.get(q + 20)? != 1 || *a.get(q + 57)? != 1 {
        return None;
    }
    let mats = |o: usize| -> Option<[f32; 9]> {
        let mut c = Cur::new(a, o);
        let mut m = [0f32; 9];
        for v in &mut m {
            *v = c.f32()?;
        }
        (m.iter().all(|v| v.is_finite()) && m[6] == 0.0 && m[7] == 0.0 && m[8] == 1.0).then_some(m)
    };
    let adjust = mats(q + 21)?;
    let uv = mats(q + 58)?;
    (uv[0] > 0.0 && uv[4] > 0.0 && uv[1] == 0.0 && uv[3] == 0.0).then_some(PatternMatrices { tiled, uv, adjust })
}

/// Least-squares affine map taking `local` onto `page` (paired points); `None` when degenerate.
fn fit_affine(local: &[StudioSubpath], page: &[StudioSubpath]) -> Option<Affine> {
    let pts = |s: &[StudioSubpath]| s.iter().flat_map(|p| p.points.iter().map(|q| (f64::from(q.x), f64::from(q.y)))).collect::<Vec<_>>();
    let (l, p) = (pts(local), pts(page));
    if l.len() != p.len() || l.len() < 3 {
        return None;
    }
    // Centre both point sets to keep the normal equations well conditioned.
    let mean = |v: &[(f64, f64)]| {
        let n = v.len() as f64;
        v.iter().fold((0.0, 0.0), |a, q| (a.0 + q.0 / n, a.1 + q.1 / n))
    };
    let (lm, pm) = (mean(&l), mean(&p));
    let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
    let (mut ux, mut vx, mut uy, mut vy) = (0.0, 0.0, 0.0, 0.0);
    for (a, b) in l.iter().zip(&p) {
        let (x, y) = (a.0 - lm.0, a.1 - lm.1);
        let (u, v) = (b.0 - pm.0, b.1 - pm.1);
        sxx += x * x;
        sxy += x * y;
        syy += y * y;
        ux += u * x;
        uy += u * y;
        vx += v * x;
        vy += v * y;
    }
    let det = sxx * syy - sxy * sxy;
    if det.abs() < 1e-12 * (sxx * syy).max(1e-300) {
        return None;
    }
    let a = (ux * syy - uy * sxy) / det;
    let c = (uy * sxx - ux * sxy) / det;
    let b = (vx * syy - vy * sxy) / det;
    let d = (vy * sxx - vx * sxy) / det;
    let lin = Affine::new([a, b, c, d, 0.0, 0.0]);
    let m = Affine::translate(Point::new(pm.0, pm.1) - lin * Point::new(lm.0, lm.1)) * lin;
    let worst = l.iter().zip(&p).map(|(a, b)| (m * Point::new(a.0, a.1)).distance(Point::new(b.0, b.1))).fold(0.0, f64::max);
    (worst < 0.05).then_some(m)
}

/// Exact pattern placement: image pixels to page millimetres, as Silhouette Studio draws it.
pub fn pattern_affine(s: &StudioShape, img: &StudioImage) -> Option<Affine> {
    let mats = s.pattern?;
    let local = s.image_fill.as_ref()?.local.as_deref()?;
    Some(fit_affine(local, &s.outline)? * mats.local_affine(img.width_px, img.height_px))
}

impl StudioShape {
    pub fn is_empty(&self) -> bool {
        self.outline.iter().all(|s| s.points.is_empty())
    }

    pub fn point_count(&self) -> usize {
        self.outline.iter().map(|s| s.points.len()).sum()
    }

    pub fn to_bezpath(&self) -> BezPath {
        subpaths_to_bezpath(&self.outline)
    }

    pub fn affine(&self) -> Affine {
        let m = self.matrix.map(f64::from);
        Affine::new([m[0], m[3], m[1], m[4], m[2], m[5]])
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StudioFile {
    pub version: StudioVersion,
    /// Decimal-packed version of the writer (`major*10^8 + minor*10^6 + build`), v5 only.
    pub saved_by: Option<u64>,
    /// Page width and height in mm.
    pub page_mm: Option<(f32, f32)>,
    /// Leaf shapes with a non-empty page outline, in file order (back to front).
    pub shapes: Vec<StudioShape>,
    /// Records with an empty outline (layers, groups, empty text), counted for diagnostics.
    pub empty_records: usize,
    pub preview_png: Option<Vec<u8>>,
    /// Image table (section 1, v5 only).
    pub images: Vec<StudioImage>,
    /// Section byte ranges in the file (v5).
    pub sections: Vec<(usize, usize)>,
    /// Declared sections dropped because they run past the end of the file.
    pub dropped_sections: usize,
}

impl StudioFile {
    pub fn saved_by_string(&self) -> Option<String> {
        self.saved_by.map(|v| format!("{}.{}.{}", v / 100_000_000, v / 1_000_000 % 100, v % 1_000_000))
    }
}

pub fn subpaths_to_bezpath(subs: &[StudioSubpath]) -> BezPath {
    let mut p = BezPath::new();
    for s in subs {
        let pts = &s.points;
        let Some(first) = pts.first() else { continue };
        let pt = |q: &StudioPoint| Point::new(f64::from(q.x), f64::from(q.y));
        let ctrl = |q: &StudioPoint, h: Option<(f32, f32)>| match h {
            Some((dx, dy)) => Point::new(f64::from(q.x + dx), f64::from(q.y + dy)),
            None => pt(q),
        };
        p.move_to(pt(first));
        let n = pts.len();
        let segs = if s.closed { n } else { n - 1 };
        for i in 0..segs {
            let a = &pts[i];
            let b = &pts[(i + 1) % n];
            if a.handle_out.is_none() && b.handle_in.is_none() {
                p.line_to(pt(b));
            } else {
                p.curve_to(ctrl(a, a.handle_out), ctrl(b, b.handle_in), pt(b));
            }
        }
        if s.closed {
            p.close_path();
        }
    }
    p
}

/// Bounds-checked little-endian reader over a byte slice.
#[derive(Clone, Copy)]
struct Cur<'a> {
    b: &'a [u8],
    o: usize,
}

impl<'a> Cur<'a> {
    fn new(b: &'a [u8], o: usize) -> Self {
        Self { b, o }
    }
    fn u8(&mut self) -> Option<u8> {
        let v = *self.b.get(self.o)?;
        self.o += 1;
        Some(v)
    }
    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let s = self.b.get(self.o..self.o.checked_add(N)?)?;
        self.o += N;
        s.try_into().ok()
    }
    fn u32(&mut self) -> Option<u32> {
        self.take::<4>().map(u32::from_le_bytes)
    }
    fn i32(&mut self) -> Option<i32> {
        self.take::<4>().map(i32::from_le_bytes)
    }
    fn u64(&mut self) -> Option<u64> {
        self.take::<8>().map(u64::from_le_bytes)
    }
    fn f32(&mut self) -> Option<f32> {
        self.take::<4>().map(f32::from_le_bytes)
    }
}

fn read_u64(b: &[u8], o: usize) -> Option<u64> {
    Cur::new(b, o).u64()
}

/// Block at `o`: returns (body_start, end).
fn block_at(b: &[u8], o: usize) -> Option<(usize, usize)> {
    if *b.get(o)? != 1 {
        return None;
    }
    let size = Cur::new(b, o + 1).u32()? as usize;
    let end = o.checked_add(1)?.checked_add(size)?;
    if size < 4 || end > b.len() {
        return None;
    }
    Some((o + 5, end))
}

fn handle(c: &mut Cur) -> Option<Option<(f32, f32)>> {
    match c.u8()? {
        0 => Some(None),
        1 => {
            let dx = c.f32()?;
            let dy = c.f32()?;
            (dx.is_finite() && dy.is_finite()).then_some(Some((dx, dy)))
        }
        _ => None,
    }
}

/// Page-outline path grammar (block P):
/// `i32 lastSub, { u32 size, u8 closed, i32 lastPt, { f32 x, f32 y, handle, handle }* }*`.
/// Returns `None` unless the body is consumed exactly.
fn parse_page_path(body: &[u8]) -> Option<Vec<StudioSubpath>> {
    let mut c = Cur::new(body, 0);
    let last_sub = c.i32()?;
    if last_sub < -1 {
        return None;
    }
    let count = (last_sub + 1) as usize;
    // Each subpath needs at least 9 bytes; reject absurd counts before allocating.
    if count > body.len() / 9 {
        return None;
    }
    let mut subs = Vec::with_capacity(count);
    for _ in 0..count {
        let start = c.o;
        let size = c.u32()? as usize;
        let closed = match c.u8()? {
            0 => false,
            1 => true,
            _ => return None,
        };
        let last_pt = c.i32()?;
        if last_pt < -1 {
            return None;
        }
        let n = (last_pt + 1) as usize;
        if n > body.len() / 10 {
            return None;
        }
        let mut points = Vec::with_capacity(n);
        for _ in 0..n {
            let x = c.f32()?;
            let y = c.f32()?;
            if !x.is_finite() || !y.is_finite() {
                return None;
            }
            let handle_in = handle(&mut c)?;
            let handle_out = handle(&mut c)?;
            points.push(StudioPoint { x, y, handle_in, handle_out });
        }
        if c.o - start != size {
            return None;
        }
        subs.push(StudioSubpath { closed, points });
    }
    (c.o == body.len()).then_some(subs)
}

/// Local-outline grammar inside block A, where subpaths and points are themselves blocks:
/// `i32 lastSub, { block{ u8 closed, i32 lastPt, { block{ f32 x, f32 y, handle, handle, flags } }* } }*`.
fn parse_local_path(b: &[u8], body: usize, end: usize) -> Option<Vec<StudioSubpath>> {
    let mut c = Cur::new(&b[..end], body);
    let last_sub = c.i32()?;
    if last_sub < -1 || (last_sub + 1) as usize > (end - body) / 10 {
        return None;
    }
    let mut subs = Vec::new();
    for _ in 0..=last_sub {
        let (sb, se) = block_at(&b[..end], c.o)?;
        let mut s = Cur::new(&b[..se], sb);
        let closed = match s.u8()? {
            0 => false,
            1 => true,
            _ => return None,
        };
        let last_pt = s.i32()?;
        if last_pt < -1 || (last_pt + 1) as usize > (se - sb) / 10 {
            return None;
        }
        let mut points = Vec::new();
        for _ in 0..=last_pt {
            let (pb, pe) = block_at(&b[..se], s.o)?;
            let mut p = Cur::new(&b[..pe], pb);
            let x = p.f32()?;
            let y = p.f32()?;
            let handle_in = handle(&mut p)?;
            let handle_out = handle(&mut p)?;
            points.push(StudioPoint { x, y, handle_in, handle_out });
            s.o = pe;
        }
        if s.o != se {
            return None;
        }
        subs.push(StudioSubpath { closed, points });
        c.o = se;
    }
    (c.o == end).then_some(subs)
}

/// Finds the shape's own local outline inside its block A (first block that parses as a local
/// path with the same subpath/point structure as the page outline).
pub fn local_outline(doc: &[u8], shape: &StudioShape) -> Option<Vec<StudioSubpath>> {
    let (a0, a1) = shape.a_range;
    let want: Vec<usize> = shape.outline.iter().map(|s| s.points.len()).collect();
    (a0..a1.saturating_sub(5)).find_map(|i| {
        let (body, end) = block_at(&doc[..a1], i)?;
        let subs = parse_local_path(doc, body, end)?;
        (subs.iter().map(|s| s.points.len()).collect::<Vec<_>>() == want).then_some(subs)
    })
}

fn find_effects(a: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < a.len() {
        if a[i].is_ascii_graphic() {
            let s = i;
            while i < a.len() && (a[i].is_ascii_graphic() || a[i] == b' ') {
                i += 1;
            }
            let run = &a[s..i];
            if run.windows(7).any(|w| w == b"effect=")
                && let Ok(t) = std::str::from_utf8(run)
            {
                out.push(t.to_string());
            }
        }
        i += 1;
    }
    out
}

struct ShapeProps {
    stroke: Option<[f32; 4]>,
    stroke_width_mm: f32,
    cut_id: Option<String>,
    fill: Option<StudioFill>,
}

fn read_str(c: &mut Cur, max_len: usize) -> Option<String> {
    let len = c.u32()? as usize;
    if len > max_len {
        return None;
    }
    let bytes = c.b.get(c.o..c.o.checked_add(len)?)?;
    c.o += len;
    Some(String::from_utf8_lossy(bytes).into_owned())
}

fn parse_props_body(p: &[u8]) -> Option<ShapeProps> {
    let mut c = Cur::new(p, 0);
    if c.u8()? != 0 {
        return None;
    }
    let has_stroke = c.u8()?;
    let stroke = match has_stroke {
        0 => None,
        1 => {
            let rgba = [c.f32()?, c.f32()?, c.f32()?, c.f32()?];
            rgba.iter().all(|v| v.is_finite()).then_some(rgba)
        }
        _ => return None,
    };
    let _sw0 = c.f32()?;
    let sw1 = c.f32()?;
    let stroke_width_mm = if sw1.is_finite() && (0.0..=200.0).contains(&sw1) { sw1 } else { 0.0 };
    let (cb, ce) = block_at(p, c.o)?;
    let mut cc = Cur::new(&p[..ce], cb);
    let cut_id = read_str(&mut cc, 64);
    c.o = ce;
    let _ = c.u8()?;
    let _join = read_str(&mut c, 64)?;
    let _ = c.u8()?;
    let _cap = read_str(&mut c, 64)?;
    let fill = match c.u8()? {
        0 => None,
        1 => {
            let rgba = [c.f32()?, c.f32()?, c.f32()?, c.f32()?];
            rgba.iter().all(|v| v.is_finite()).then_some(StudioFill::Solid(rgba))
        }
        2 => {
            let _gsize = c.u32()?;
            let last_stop = c.i32()?;
            let mut stops = Vec::new();
            if (0..=64).contains(&last_stop) {
                for _ in 0..=last_stop {
                    let rgba = [c.f32()?, c.f32()?, c.f32()?, c.f32()?];
                    let pos = c.f32()?;
                    if rgba.iter().all(|v| v.is_finite()) && pos.is_finite() {
                        stops.push((pos, rgba));
                    }
                }
            }
            (!stops.is_empty()).then_some(StudioFill::Gradient(stops))
        }
        // v4 uses 3 and v5 uses 5 for pattern/image fills.
        3 | 5 => read_str(&mut c, 256).filter(|s| !s.is_empty()).map(StudioFill::Pattern),
        _ => None,
    };
    Some(ShapeProps { stroke, stroke_width_mm, cut_id, fill })
}

fn parse_shape_props(a: &[u8]) -> Option<ShapeProps> {
    if a.len() >= 17 && a[0] == 0x06 && a[5] == 0x00 && a[6] == 0x01 && a[11] == 0x02 {
        let size = Cur::new(a, 12).u32()? as usize;
        let end = 12usize.checked_add(size)?.min(a.len());
        if end > 16 {
            return parse_props_body(&a[16..end]);
        }
    }
    if a.len() >= 11 && matches!(a[0], 0x01 | 0x02) && a[5] == 0x00 {
        let (pb, pe) = block_at(a, 6)?;
        return parse_props_body(&a[pb..pe]);
    }
    None
}

fn find_layer_name(a: &[u8]) -> Option<String> {
    for i in 0..a.len().saturating_sub(8) {
        if a[i..i + 4] == [0xff, 0xff, 0xff, 0xff] {
            let len = Cur::new(a, i + 4).u32()? as usize;
            if len <= 128
                && let Some(bytes) = a.get(i + 8..i + 8 + len)
                && let Ok(s) = std::str::from_utf8(bytes)
                && s.chars().all(|ch| !ch.is_control())
            {
                return Some(if s.is_empty() { "Default Layer".to_string() } else { s.to_string() });
            }
        }
    }
    None
}

fn page_size(doc: &[u8]) -> Option<(f32, f32)> {
    let (root_body, _) = block_at(doc, MAGIC_V5.len())?;
    let (_, first_end) = block_at(doc, root_body)?;
    let (second_body, second_end) = block_at(doc, first_end)?;
    let mut c = Cur::new(&doc[..second_end], second_body + 0x29);
    let w = c.f32()?;
    let h = c.f32()?;
    // Calibration templates lay this block out differently; reject what is not a plausible size.
    ((1.0..100_000.0).contains(&w) && (1.0..100_000.0).contains(&h)).then_some((w, h))
}

/// Decodes every shape record of a document section.
pub fn parse_document(doc: &[u8]) -> (Vec<StudioShape>, usize) {
    let mut shapes: Vec<StudioShape> = Vec::new();
    // P offset -> index into `shapes` (None for empty records).
    let mut seen_p: HashMap<usize, Option<usize>> = HashMap::new();
    let has_props = |a: (usize, usize)| a.1 - a.0 < 64 * 1024 && parse_shape_props(&doc[a.0..a.1]).is_some();
    let mut empty = 0;
    for i in 1..doc.len().saturating_sub(5) {
        if doc[i] != 1 || doc[i - 1] != 0 {
            continue;
        }
        let Some((a_body, a_end)) = block_at(doc, i) else { continue };
        let p_off = a_end + TRANSFORM_LEN;
        if let Some(prev) = seen_p.get(&p_off) {
            // An enclosing block can end exactly where the shape's own block A ends; prefer the
            // block that holds shape properties.
            if let Some(s) = prev.map(|k| &mut shapes[k])
                && has_props((a_body, a_end))
                && !has_props(s.a_range)
            {
                s.a_range = (a_body, a_end);
            }
            continue;
        }
        let Some((p_body, p_end)) = block_at(doc, p_off) else { continue };
        let Some(outline) = parse_page_path(&doc[p_body..p_end]) else { continue };
        let mut c = Cur::new(doc, p_end);
        if c.u8() != Some(1) {
            continue;
        }
        let Some(serial) = c.u64() else { continue };
        let Some((_, c_end)) = block_at(doc, c.o) else { continue };
        let mut n = Cur::new(doc, c_end);
        let name = n.u32().filter(|len| *len <= MAX_NAME_LEN).and_then(|len| {
            let bytes = doc.get(n.o..n.o + len as usize)?;
            Some(String::from_utf8_lossy(bytes).into_owned())
        });
        let mut m = Cur::new(doc, a_end);
        let mut matrix = [0f32; 9];
        for v in &mut matrix {
            *v = m.f32().unwrap_or(0.0);
        }
        let shape = StudioShape {
            name,
            serial,
            matrix,
            outline,
            effects: Vec::new(),
            a_range: (a_body, a_end),
            p_offset: p_off,
            image_fill: None,
            stroke: None,
            stroke_width_mm: 0.0,
            cut_id: None,
            fill: None,
            layer_name: None,
            pattern: None,
        };
        if shape.is_empty() {
            empty += 1;
            seen_p.insert(p_off, None);
        } else {
            seen_p.insert(p_off, Some(shapes.len()));
            shapes.push(shape);
        }
    }
    shapes.sort_by_key(|s| s.p_offset);
    for s in &mut shapes {
        // Effects and properties live in the shape's own block A; skip very large A blocks (containers).
        let (a0, a1) = s.a_range;
        if a1 - a0 < 64 * 1024 {
            let a = &doc[a0..a1];
            s.effects = find_effects(a);
            if let Some(props) = parse_shape_props(a) {
                s.stroke = props.stroke;
                s.stroke_width_mm = props.stroke_width_mm;
                s.cut_id = props.cut_id;
                s.fill = props.fill;
            }
            if let Some(StudioFill::Pattern(id)) = &s.fill {
                s.pattern = parse_pattern_matrices(a, id);
            }
            s.layer_name = find_layer_name(a);
        }
    }
    (shapes, empty)
}

/// Image table layout (section 1), verified on 76 real files with 1–14 images each:
/// `0x01, u64 section size, u64 ?`, then from `0x11` records of
/// `u64 record size (from this field), u64 id length, id, u64 header length,
///  header { u64 name length, name, u64 width, u64 height, u64 dpi, …, u64 png length }, PNG`.
pub fn parse_image_table(s: &[u8]) -> Vec<StudioImage> {
    let mut out = Vec::new();
    if s.first() != Some(&1) {
        return out;
    }
    let mut o = 0x11;
    while let Some(size) = read_u64(s, o) {
        let Some(end) = usize::try_from(size).ok().filter(|n| *n >= 24).and_then(|n| o.checked_add(n)).filter(|e| *e <= s.len()) else {
            break;
        };
        if let Some(img) = parse_image_record(&s[o..end]) {
            out.push(img);
        }
        o = end;
    }
    out
}

fn parse_image_record(r: &[u8]) -> Option<StudioImage> {
    let mut c = Cur::new(r, 8);
    let len = |v: u64, max: usize| usize::try_from(v).ok().filter(|n| *n <= max);
    let id_len = len(c.u64()?, 256)?;
    let id = std::str::from_utf8(r.get(c.o..c.o + id_len)?).ok()?.to_string();
    c.o += id_len;
    let header = len(c.u64()?, r.len())?;
    let png_at = c.o.checked_add(header)?;
    let name_len = len(c.u64()?, 4096)?;
    let name = String::from_utf8_lossy(r.get(c.o..c.o + name_len)?).into_owned();
    c.o += name_len;
    let width_px = u32::try_from(c.u64()?).ok()?;
    let height_px = u32::try_from(c.u64()?).ok()?;
    let png_len = len(read_u64(r, png_at.checked_sub(8)?)?, r.len())?;
    let png = r.get(png_at..png_at.checked_add(png_len)?)?;
    (ss_doc::image_size(png) == Some((width_px, height_px)) && !id.is_empty())
        .then(|| StudioImage { id, name, width_px, height_px, png: ss_doc::SharedBytes::from(png) })
}

/// Scans a v4 `.studio` stream for embedded image blocks:
/// `0x01, u32 block_size, u32 id_len, id, u32 w, u32 h, (0x00|0x01) kind, u32 data_len, bytes`.
pub fn parse_v4_image_table(data: &[u8]) -> Vec<StudioImage> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 24 < data.len() {
        if data[i] != 1 {
            i += 1;
            continue;
        }
        let Some((body, end)) = block_at(data, i) else {
            i += 1;
            continue;
        };
        if end - body >= 20
            && let Some(img) = parse_v4_image_record(&data[body..end])
        {
            out.push(img);
            i = end;
            continue;
        }
        i += 1;
    }
    out
}

fn parse_v4_image_record(body: &[u8]) -> Option<StudioImage> {
    let mut c = Cur::new(body, 0);
    let id_len = c.u32()? as usize;
    if !(1..=256).contains(&id_len) {
        return None;
    }
    let id = std::str::from_utf8(body.get(c.o..c.o + id_len)?).ok()?.to_string();
    if id.bytes().any(|b| !(0x20..0x7f).contains(&b)) {
        return None;
    }
    c.o += id_len;
    let width_px = c.u32()?;
    let height_px = c.u32()?;
    if width_px == 0 || height_px == 0 || width_px > 65536 || height_px > 65536 {
        return None;
    }
    let kind = c.u8()?;
    if kind > 1 {
        return None;
    }
    let data_len = c.u32()? as usize;
    let img_start = c.o;
    let mut img_end = img_start.checked_add(data_len)?;
    if img_end > body.len() || body.len() - img_end > 4 {
        return None;
    }
    // v4 PNG blocks often leave the final 4-byte IEND CRC32 outside `data_len` inside `body`.
    if kind == 1 && body.len() - img_end == 4 {
        img_end = body.len();
    }
    let bytes = &body[img_start..img_end];
    (ss_doc::image_size(bytes) == Some((width_px, height_px)))
        .then(|| StudioImage { id: id.clone(), name: id, width_px, height_px, png: ss_doc::SharedBytes::from(bytes) })
}

/// Links shapes to image-table entries. A reference is `0x05` (v5) or `0x03` (v4) followed by a
/// length-prefixed id inside the shape's properties; it belongs to the innermost record whose
/// block A contains it.
fn attach_image_fills(doc: &[u8], shapes: &mut [StudioShape], images: &[StudioImage]) {
    if images.is_empty() {
        return;
    }
    let ids: HashSet<&[u8]> = images.iter().map(|i| i.id.as_bytes()).collect();
    for s in shapes.iter_mut() {
        if let Some(StudioFill::Pattern(id)) = &s.fill
            && ids.contains(id.as_bytes())
        {
            s.image_fill = Some(ImageFill { image_id: id.clone(), local: None });
        }
    }
    for i in 0..doc.len().saturating_sub(5) {
        if !matches!(doc[i], 3 | 5) {
            continue;
        }
        let Some(n) = Cur::new(doc, i + 1).u32().map(|n| n as usize).filter(|n| (1..=256).contains(n)) else { continue };
        let Some(id) = doc.get(i + 5..i + 5 + n).filter(|id| ids.contains(id)) else { continue };
        let owner = shapes
            .iter_mut()
            .filter(|s| s.a_range.0 <= i && i < s.a_range.1)
            .min_by_key(|s| s.a_range.1 - s.a_range.0);
        if let Some(s) = owner
            && s.image_fill.is_none()
        {
            s.image_fill = Some(ImageFill { image_id: String::from_utf8_lossy(id).into_owned(), local: None });
        }
    }
    for s in shapes.iter_mut() {
        if s.image_fill.is_some() {
            let local = local_outline(doc, s);
            if let Some(f) = s.image_fill.as_mut() {
                f.local = local;
            }
        }
    }
}
/// Section byte ranges, number of dropped sections, writer version.
type V5Layout = (Vec<(usize, usize)>, usize, Option<u64>);

/// Splits a `.studio3` container into its section ranges.
fn split_v5(data: &[u8]) -> Result<V5Layout, StudioError> {
    if data.len() < V5_HEADER_LEN {
        return Err(StudioError::Truncated);
    }
    let mut sizes = Vec::new();
    let mut o = 0x10;
    while o < V5_TABLE_END {
        sizes.push(read_u64(data, o).ok_or(StudioError::Truncated)?);
        o += 8;
    }
    while sizes.last() == Some(&0) {
        sizes.pop();
    }
    let saved_by = read_u64(data, V5_TABLE_END).filter(|v| *v > 0);
    let mut ranges = Vec::new();
    let mut dropped = 0;
    let mut start = V5_HEADER_LEN;
    for s in sizes {
        let end = usize::try_from(s).ok().and_then(|s| start.checked_add(s));
        match end {
            Some(end) if end <= data.len() => {
                ranges.push((start, end));
                start = end;
            }
            _ => {
                dropped += 1;
                start = data.len();
            }
        }
    }
    Ok((ranges, dropped, saved_by))
}

pub fn detect(data: &[u8]) -> Result<StudioVersion, StudioError> {
    match data.get(..13) {
        Some(m) if m == MAGIC_V5 => Ok(StudioVersion::V5),
        Some(m) if m == MAGIC_V4 => Ok(StudioVersion::V4),
        Some(m) if m.starts_with(b"silhouette") => Err(StudioError::Unsupported(String::from_utf8_lossy(m).into_owned())),
        _ => Err(StudioError::NotStudio),
    }
}

/// Returns the document section of a studio file (the whole file for v4).
pub fn document_section(data: &[u8]) -> Result<&[u8], StudioError> {
    match detect(data)? {
        StudioVersion::V4 => Ok(data),
        StudioVersion::V5 => {
            let (ranges, _, _) = split_v5(data)?;
            let (s, e) = *ranges.first().ok_or(StudioError::Truncated)?;
            let doc = &data[s..e];
            if doc.get(..13) != Some(MAGIC_V5) {
                return Err(StudioError::NotStudio);
            }
            Ok(doc)
        }
    }
}

pub fn parse(data: &[u8]) -> Result<StudioFile, StudioError> {
    let version = detect(data)?;
    let (sections, dropped, saved_by) = match version {
        StudioVersion::V4 => (vec![(0, data.len())], 0, None),
        StudioVersion::V5 => split_v5(data)?,
    };
    let doc = document_section(data)?;
    let (mut shapes, empty_records) = parse_document(doc);
    let images = match version {
        StudioVersion::V5 => sections.get(1).map(|&(s, e)| parse_image_table(&data[s..e])).unwrap_or_default(),
        StudioVersion::V4 => parse_v4_image_table(data),
    };
    attach_image_fills(doc, &mut shapes, &images);
    let preview_png = sections
        .get(2)
        .map(|&(s, e)| &data[s..e])
        .filter(|b| b.starts_with(b"\x89PNG\r\n\x1a\n"))
        .map(<[u8]>::to_vec);
    Ok(StudioFile { version, saved_by, page_mm: page_size(doc), shapes, empty_records, preview_png, images, sections, dropped_sections: dropped })
}

/// Page preset matching `w`×`h` mm, or a custom page of that size.
pub fn page_for_size(w: f64, h: f64) -> ss_doc::Page {
    use ss_doc::Page;
    ["letter", "a4", "a3", "legal", "8x12", "12x12", "12x24", "15x15", "24x24", "6x8"]
        .into_iter()
        .filter_map(Page::preset)
        .find(|p| (p.width_mm - w).abs() < 0.5 && (p.height_mm - h).abs() < 0.5)
        .unwrap_or(Page { width_mm: w, height_mm: h, media: "custom".into(), ..Page::default() })
}

fn straight_points(subs: &[StudioSubpath]) -> Option<Vec<Point>> {
    let [s] = subs else { return None };
    let curved = s.points.iter().any(|p| p.handle_in.is_some() || p.handle_out.is_some());
    (s.points.len() == 4 && !curved).then(|| s.points.iter().map(|p| Point::new(f64::from(p.x), f64::from(p.y))).collect())
}

/// Placement of a shape that shows a whole image stretched over a rectangle: the image's pixel
/// grid spans the local rectangle and `Affine` maps it to the page. Tiled pattern fills, whose
/// outline is any shape or whose rectangle does not match the image's aspect ratio, return `None`.
pub fn placed_image(file: &StudioFile, s: &StudioShape) -> Option<(ss_doc::ImageData, Affine)> {
    let fill = s.image_fill.as_ref()?;
    let img = file.images.iter().find(|i| i.id == fill.image_id)?;
    let page = straight_points(&s.outline)?;
    let unit = [(-1.0, -1.0), (-1.0, 1.0), (1.0, 1.0), (1.0, -1.0)].map(|(x, y)| Point::new(x, y)).to_vec();
    let local = fill.local.as_deref().and_then(straight_points).unwrap_or(unit);
    let (dl1, dl3) = (local[1] - local[0], local[3] - local[0]);
    let (dp1, dp3) = (page[1] - page[0], page[3] - page[0]);
    let det = dl1.x * dl3.y - dl3.x * dl1.y;
    if det.abs() < 1e-12 {
        return None;
    }
    // Linear part M solves M·dl1 = dp1 and M·dl3 = dp3.
    let a = (dp1.x * dl3.y - dp3.x * dl1.y) / det;
    let c = (dp3.x * dl1.x - dp1.x * dl3.x) / det;
    let b = (dp1.y * dl3.y - dp3.y * dl1.y) / det;
    let d = (dp3.y * dl1.x - dp1.y * dl3.x) / det;
    let lin = Affine::new([a, b, c, d, 0.0, 0.0]);
    let m = Affine::translate(page[0] - lin * local[0]) * lin;
    if (m * local[2]).distance(page[2]) > 0.05 {
        return None;
    }
    let lb = local.iter().fold(ss_geom::Rect::from_points(local[0], local[0]), |r, p| r.union_pt(*p));
    let (sx, sy) = (m.as_coeffs()[0].hypot(m.as_coeffs()[1]), m.as_coeffs()[2].hypot(m.as_coeffs()[3]));
    let shown = (lb.width() * sx) / (lb.height() * sy);
    let native = f64::from(img.width_px) / f64::from(img.height_px);
    if (shown / native - 1.0).abs() > 0.01 {
        return None;
    }
    let data = ss_doc::ImageData::from_encoded(img.png.clone(), lb)?;
    Some((data, m))
}

fn pattern_fill(s: &StudioShape, img: &StudioImage) -> ss_doc::PatternFill {
    let transform = pattern_affine(s, img);
    let (tw, th) = match transform {
        Some(m) => {
            let c = m.as_coeffs();
            (c[0].hypot(c[1]) * f64::from(img.width_px), c[2].hypot(c[3]) * f64::from(img.height_px))
        }
        // Without stored matrices, assume 300 DPI clamped to a sensible range.
        None => ((f64::from(img.width_px) * 25.4 / 300.0).clamp(15.0, 150.0), (f64::from(img.height_px) * 25.4 / 300.0).clamp(15.0, 150.0)),
    };
    ss_doc::PatternFill {
        id: img.id.clone(),
        encoded: img.png.clone(),
        width_px: img.width_px,
        height_px: img.height_px,
        tile_width_mm: tw,
        tile_height_mm: th,
        transform: transform.map(|m| m.as_coeffs()),
    }
}

fn map_cut_action(cut_id: Option<&str>) -> ss_doc::CutAction {
    use ss_doc::CutAction;
    match cut_id.map(str::to_ascii_lowercase).as_deref() {
        Some("nocut" | "none") => CutAction::NoCut,
        Some("sketch" | "pen") => CutAction::Sketch,
        Some("cutedge") => CutAction::CutEdge,
        Some("perforate") => CutAction::Perforate,
        _ => CutAction::Cut,
    }
}

/// Converts a decoded file into an editable document: one path object per shape in page
/// coordinates, and an image object (never cut) for each placed raster image.
pub fn to_document(file: &StudioFile) -> ss_doc::Document {
    use std::collections::HashMap;

    use ss_doc::{Document, Layer, Object, Rgba};
    let mut doc = Document::default();
    if let Some((w, h)) = file.page_mm {
        doc.page = page_for_size(f64::from(w), f64::from(h));
    }
    let default_layer = doc.default_layer();
    let mut layer_ids: HashMap<String, ss_doc::LayerId> = HashMap::new();
    layer_ids.insert("Default Layer".to_string(), default_layer);

    let images_by_id: HashMap<&str, &StudioImage> = file.images.iter().map(|i| (i.id.as_str(), i)).collect();

    for s in &file.shapes {
        let layer_name = s.layer_name.as_deref().unwrap_or("Default Layer");
        let layer = *layer_ids.entry(layer_name.to_string()).or_insert_with(|| {
            let l = Layer::new(layer_name);
            let id = l.id;
            doc.layers.push(l);
            id
        });
        let name = s.name.clone().filter(|n| !n.trim().is_empty());
        if s.pattern.is_none()
            && let Some((img, m)) = placed_image(file, s)
        {
            let mut o = Object::image(name.unwrap_or_else(|| "Image".into()), layer, img);
            o.transform = m;
            doc.objects.push(o);
            continue;
        }
        let mut o = Object::path(name.unwrap_or_else(|| "Shape".into()), layer, s.to_bezpath());
        o.cut = map_cut_action(s.cut_id.as_deref());
        o.style.stroke = match s.stroke {
            Some(c) => Rgba::from_f32(c),
            None => {
                if s.fill.is_some() || s.image_fill.is_some() {
                    Rgba::TRANSPARENT
                } else {
                    Rgba::RED
                }
            }
        };
        o.style.stroke_width_mm = f64::from(s.stroke_width_mm);
        match &s.fill {
            Some(StudioFill::Solid(c)) => {
                let rgba = Rgba::from_f32(*c);
                if rgba.a > 0 {
                    o.style.fill = Some(rgba);
                }
            }
            Some(StudioFill::Gradient(stops)) => {
                if let Some((_, c)) = stops.get(stops.len() / 2) {
                    let rgba = Rgba::from_f32(*c);
                    if rgba.a > 0 {
                        o.style.fill = Some(rgba);
                    }
                }
                let mut gstops: Vec<(f64, Rgba)> = stops
                    .iter()
                    .map(|(pos, c)| (f64::from(*pos).clamp(0.0, 1.0), Rgba::from_f32(*c)))
                    .collect();
                gstops.sort_by(|a, b| a.0.total_cmp(&b.0));
                if !gstops.is_empty() {
                    o.style.fill_gradient = Some(ss_doc::GradientFill { stops: gstops, angle_deg: 0.0, radial: false });
                }
            }
            Some(StudioFill::Pattern(pid)) => {
                if let Some(img) = images_by_id.get(pid.as_str()) {
                    o.style.fill_pattern = Some(Box::new(pattern_fill(s, img)));
                }
            }
            None => {
                if let Some(img_fill) = &s.image_fill
                    && let Some(img) = images_by_id.get(img_fill.image_id.as_str())
                {
                    o.style.fill_pattern = Some(Box::new(pattern_fill(s, img)));
                }
            }
        }
        if s.pattern.is_some_and(|p| !p.tiled) {
            // Placed pictures are artwork, not cut lines.
            o.cut = ss_doc::CutAction::NoCut;
        }
        if o.cut == ss_doc::CutAction::Perforate && o.style.dash_mm.is_empty() {
            o.style.dash_mm = vec![2.0, 1.0];
        }
        doc.objects.push(o);
    }
    doc
}
#[cfg(test)]
mod tests {
    use super::*;

    fn block(body: &[u8]) -> Vec<u8> {
        let mut v = vec![1];
        v.extend(((body.len() + 4) as u32).to_le_bytes());
        v.extend(body);
        v
    }

    fn page_path(subs: &[(bool, &[(f32, f32)])]) -> Vec<u8> {
        let mut b = ((subs.len() as i32) - 1).to_le_bytes().to_vec();
        for (closed, pts) in subs {
            let mut s = Vec::new();
            s.push(*closed as u8);
            s.extend(((pts.len() as i32) - 1).to_le_bytes());
            for (x, y) in pts.iter() {
                s.extend(x.to_le_bytes());
                s.extend(y.to_le_bytes());
                s.extend([0, 0]);
            }
            b.extend(((s.len() + 4) as u32).to_le_bytes());
            b.extend(s);
        }
        b
    }

    /// A minimal synthetic v4 document with one rectangle record.
    fn synthetic_v4() -> Vec<u8> {
        let mut doc = MAGIC_V4.to_vec();
        let mut root = Vec::new();
        root.extend(block(&[0; 8]));
        let mut page = vec![0u8; 0x29];
        page.extend(304.8f32.to_le_bytes());
        page.extend(304.8f32.to_le_bytes());
        root.extend(block(&page));
        root.push(0);
        root.extend(block(b"props"));
        let mut t = Vec::new();
        for v in [1f32, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] {
            t.extend(v.to_le_bytes());
        }
        t.resize(TRANSFORM_LEN, 0);
        root.extend(t);
        let pts: &[(f32, f32)] = &[(10.0, 10.0), (50.0, 10.0), (50.0, 30.0), (10.0, 30.0)];
        root.extend(block(&page_path(&[(true, pts)])));
        root.push(1);
        root.extend(7u64.to_le_bytes());
        root.extend(block(&[0; 8]));
        doc.extend(block(&root));
        doc
    }

    #[test]
    fn synthetic_record_roundtrip() {
        let f = parse(&synthetic_v4()).unwrap();
        assert_eq!(f.version, StudioVersion::V4);
        assert_eq!(f.page_mm, Some((304.8, 304.8)));
        assert_eq!(f.shapes.len(), 1);
        let s = &f.shapes[0];
        assert_eq!(s.serial, 7);
        assert_eq!(s.point_count(), 4);
        let bb = ss_geom::Shape::bounding_box(&s.to_bezpath());
        assert_eq!((bb.x0, bb.y0, bb.x1, bb.y1), (10.0, 10.0, 50.0, 30.0));
        let doc = to_document(&f);
        assert_eq!(doc.page.media, "12x12");
        assert_eq!(doc.objects.len(), 1);
    }

    #[test]
    fn truncation_and_corruption_never_panic() {
        let data = synthetic_v4();
        for len in 0..data.len() {
            let _ = parse(&data[..len]);
        }
        let mut seed = 0x1234_5678u32;
        for _ in 0..2000 {
            let mut d = data.clone();
            for _ in 0..4 {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let i = seed as usize % d.len();
                d[i] = (seed >> 8) as u8;
            }
            let _ = parse(&d);
        }
    }

    #[test]
    fn rejects_other_files() {
        assert_eq!(parse(b"<svg/>"), Err(StudioError::NotStudio));
        assert!(matches!(parse(b"silhouette03;xxxxxxxx"), Err(StudioError::Unsupported(_))));
    }

    #[test]
    fn version_string() {
        let f = StudioFile {
            version: StudioVersion::V5,
            saved_by: Some(500_000_079),
            page_mm: None,
            shapes: vec![],
            empty_records: 0,
            preview_png: None,
            images: vec![],
            sections: vec![],
            dropped_sections: 0,
        };
        assert_eq!(f.saved_by_string().as_deref(), Some("5.0.79"));
    }

    #[test]
    fn decodes_solid_gradient_and_perforate_styles() {
        let str_blk = |s: &str| {
            let mut b = vec![1u8];
            let mut body = (s.len() as u32).to_le_bytes().to_vec();
            body.extend_from_slice(s.as_bytes());
            b.extend(((body.len() + 4) as u32).to_le_bytes());
            b.extend(body);
            b
        };
        let mut p = vec![0u8, 1];
        for c in [0.0f32, 0.5, 1.0, 1.0] {
            p.extend(c.to_le_bytes());
        }
        p.extend(0.0f32.to_le_bytes());
        p.extend(1.25f32.to_le_bytes());
        p.extend(str_blk("perforate"));
        p.push(0);
        p.extend((5u32).to_le_bytes());
        p.extend_from_slice(b"miter");
        p.push(0);
        p.extend((4u32).to_le_bytes());
        p.extend_from_slice(b"flat");
        // Gradient fill (type 2) with 2 stops (last_stop = 1).
        p.push(2);
        p.extend(44u32.to_le_bytes());
        p.extend(1i32.to_le_bytes());
        for (pos, rgba) in [(0.0f32, [1.0f32, 0.0, 0.0, 1.0]), (1.0f32, [0.0f32, 0.0, 1.0, 1.0])] {
            for c in rgba {
                p.extend(c.to_le_bytes());
            }
            p.extend(pos.to_le_bytes());
        }

        let props = parse_props_body(&p).unwrap();
        assert_eq!(props.stroke, Some([0.0, 0.5, 1.0, 1.0]));
        assert!((props.stroke_width_mm - 1.25).abs() < 1e-6);
        assert_eq!(props.cut_id.as_deref(), Some("perforate"));
        let Some(StudioFill::Gradient(stops)) = props.fill else { panic!("expected gradient") };
        assert_eq!(stops.len(), 2);

        let file = StudioFile {
            version: StudioVersion::V5,
            saved_by: None,
            page_mm: None,
            shapes: vec![StudioShape {
                name: Some("Grad".into()),
                serial: 1,
                matrix: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
                outline: vec![StudioSubpath {
                    closed: true,
                    points: vec![
                        StudioPoint { x: 0.0, y: 0.0, handle_in: None, handle_out: None },
                        StudioPoint { x: 10.0, y: 0.0, handle_in: None, handle_out: None },
                        StudioPoint { x: 10.0, y: 10.0, handle_in: None, handle_out: None },
                    ],
                }],
                effects: vec![],
                a_range: (0, 0),
                p_offset: 0,
                image_fill: None,
                stroke: props.stroke,
                stroke_width_mm: props.stroke_width_mm,
                cut_id: props.cut_id,
                fill: Some(StudioFill::Gradient(stops)),
                layer_name: None,
                pattern: None,
            }],
            empty_records: 0,
            preview_png: None,
            images: vec![],
            sections: vec![],
            dropped_sections: 0,
        };
        let doc = to_document(&file);
        let obj = &doc.objects[0];
        assert_eq!(obj.cut, ss_doc::CutAction::Perforate);
        assert_eq!(obj.style.dash_mm, vec![2.0, 1.0]);
        assert!(obj.style.fill_gradient.is_some());
        let svg = crate::svg::export(&doc);
        assert!(svg.contains("<linearGradient"));
        assert!(svg.contains("stroke-dasharray=\"2 1\""));
    }
}
