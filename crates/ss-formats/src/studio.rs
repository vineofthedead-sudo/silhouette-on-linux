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

use std::collections::HashSet;

use ss_geom::{Affine, BezPath, Point};

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
    let mut shapes = Vec::new();
    let mut seen_p: HashSet<usize> = HashSet::new();
    let mut empty = 0;
    for i in 1..doc.len().saturating_sub(5) {
        if doc[i] != 1 || doc[i - 1] != 0 {
            continue;
        }
        let Some((a_body, a_end)) = block_at(doc, i) else { continue };
        let p_off = a_end + TRANSFORM_LEN;
        if seen_p.contains(&p_off) {
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
        seen_p.insert(p_off);
        let shape = StudioShape {
            name,
            serial,
            matrix,
            outline,
            effects: Vec::new(),
            a_range: (a_body, a_end),
            p_offset: p_off,
        };
        if shape.is_empty() {
            empty += 1;
        } else {
            shapes.push(shape);
        }
    }
    shapes.sort_by_key(|s| s.p_offset);
    for s in &mut shapes {
        // Effects live in the shape's own properties; skip very large A blocks (containers).
        let (a0, a1) = s.a_range;
        if a1 - a0 < 64 * 1024 {
            s.effects = find_effects(&doc[a0..a1]);
        }
    }
    (shapes, empty)
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
    let (shapes, empty_records) = parse_document(doc);
    let preview_png = sections
        .get(2)
        .map(|&(s, e)| &data[s..e])
        .filter(|b| b.starts_with(b"\x89PNG\r\n\x1a\n"))
        .map(<[u8]>::to_vec);
    Ok(StudioFile { version, saved_by, page_mm: page_size(doc), shapes, empty_records, preview_png, sections, dropped_sections: dropped })
}

/// Converts a decoded file into an editable document (one path object per shape, page coords).
pub fn to_document(file: &StudioFile) -> ss_doc::Document {
    use ss_doc::{Document, Object, Page};
    let mut doc = Document::default();
    if let Some((w, h)) = file.page_mm {
        let (w, h) = (f64::from(w), f64::from(h));
        let preset = ["letter", "a4", "a3", "legal", "8x12", "12x12", "12x24", "15x15", "24x24", "6x8"]
            .into_iter()
            .filter_map(Page::preset)
            .find(|p| (p.width_mm - w).abs() < 0.5 && (p.height_mm - h).abs() < 0.5);
        doc.page = preset.unwrap_or(Page { width_mm: w, height_mm: h, media: "custom".into(), ..Page::default() });
    }
    let layer = doc.default_layer();
    for s in &file.shapes {
        let name = s.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "Shape".into());
        doc.objects.push(Object::path(name, layer, s.to_bezpath()));
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
            sections: vec![],
            dropped_sections: 0,
        };
        assert_eq!(f.saved_by_string().as_deref(), Some("5.0.79"));
    }
}
