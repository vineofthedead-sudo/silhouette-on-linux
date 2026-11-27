use std::sync::Arc;

use serde::{Deserialize, Serialize};
use ss_geom::{Affine, BezPath, FillRule, Rect, ops};
use uuid::Uuid;

pub type ObjectId = Uuid;
pub type LayerId = Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const TRANSPARENT: Rgba = Rgba { r: 0, g: 0, b: 0, a: 0 };
    pub const BLACK: Rgba = Rgba::rgb(0, 0, 0);
    pub const RED: Rgba = Rgba::rgb(0xe0, 0x00, 0x00);
    pub const BLUE: Rgba = Rgba::rgb(0x00, 0x60, 0xe0);

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// From float components in 0..=1, as stored in Silhouette files.
    pub fn from_f32(c: [f32; 4]) -> Self {
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        Self { r: q(c[0]), g: q(c[1]), b: q(c[2]), a: q(c[3]) }
    }

    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.strip_prefix('#')?;
        let v = |i: usize| u8::from_str_radix(s.get(i..i + 2)?, 16).ok();
        match s.len() {
            6 => Some(Self::rgb(v(0)?, v(2)?, v(4)?)),
            3 => {
                let d = |i: usize| u8::from_str_radix(s.get(i..=i)?, 16).ok().map(|x| x * 17);
                Some(Self::rgb(d(0)?, d(1)?, d(2)?))
            }
            _ => None,
        }
    }
}

/// What the cutter does with an object's outline (Silhouette "Send" panel actions).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CutAction {
    #[default]
    Cut,
    NoCut,
    /// Pen / sketch tool.
    Sketch,
    /// Cut only the outer edge (print and cut).
    CutEdge,
    Perforate,
}

/// Encoded raster bytes shared across shapes and undo snapshots so cloning a document is cheap.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SharedBytes(pub Arc<[u8]>);

impl SharedBytes {
    pub fn new(bytes: impl Into<Arc<[u8]>>) -> Self {
        Self(bytes.into())
    }
}

impl From<Vec<u8>> for SharedBytes {
    fn from(v: Vec<u8>) -> Self {
        Self(Arc::from(v))
    }
}

impl From<&[u8]> for SharedBytes {
    fn from(s: &[u8]) -> Self {
        Self(Arc::from(s))
    }
}

impl std::ops::Deref for SharedBytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl Serialize for SharedBytes {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&b64::encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for SharedBytes {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        b64::decode(&String::deserialize(d)?)
            .map(Self::from)
            .ok_or_else(|| serde::de::Error::custom("bad base64"))
    }
}

/// A tiled raster pattern fill (imported from `.studio` / `.studio3` pattern fills).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PatternFill {
    pub id: String,
    pub encoded: SharedBytes,
    pub width_px: u32,
    pub height_px: u32,
    pub tile_width_mm: f64,
    pub tile_height_mm: f64,
    /// Affine coefficients `[a, b, c, d, e, f]` mapping image pixels to the object's untransformed
    /// coordinates (the tile repeats every `width_px` × `height_px`). When absent, tiles are
    /// axis-aligned `tile_width_mm` × `tile_height_mm` from the origin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<[f64; 6]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Style {
    pub stroke: Rgba,
    pub stroke_width_mm: f64,
    pub fill: Option<Rgba>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill_pattern: Option<Box<PatternFill>>,
}

impl Default for Style {
    fn default() -> Self {
        Self { stroke: Rgba::RED, stroke_width_mm: 0.25, fill: None, fill_pattern: None }
    }
}

/// Paths are shared between undo snapshots, so cloning a document is cheap.
#[derive(Debug, Clone, PartialEq)]
pub struct SharedPath(pub Arc<BezPath>);

impl SharedPath {
    pub fn new(p: BezPath) -> Self {
        Self(Arc::new(p))
    }
}

impl std::ops::Deref for SharedPath {
    type Target = BezPath;
    fn deref(&self) -> &BezPath {
        &self.0
    }
}

impl Serialize for SharedPath {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0.to_svg())
    }
}

impl<'de> Deserialize<'de> for SharedPath {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        if s.trim().is_empty() {
            return Ok(Self::new(BezPath::new()));
        }
        BezPath::from_svg(&s).map(Self::new).map_err(serde::de::Error::custom)
    }
}

mod affine_serde {
    use serde::{Deserialize, Deserializer, Serializer};
    use ss_geom::Affine;

    pub fn serialize<S: Serializer>(a: &Affine, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(a.as_coeffs())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Affine, D::Error> {
        Ok(Affine::new(<[f64; 6]>::deserialize(d)?))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextData {
    pub text: String,
    pub font_family: String,
    /// Em size (font units scaled so that one em is this many mm).
    pub size_mm: f64,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    /// Extra space added after every glyph, mm.
    #[serde(default)]
    pub letter_spacing_mm: f64,
    /// Line advance as a multiple of the font's natural line height.
    #[serde(default = "one")]
    pub line_spacing: f64,
}

fn one() -> f64 {
    1.0
}

impl Default for TextData {
    fn default() -> Self {
        Self {
            text: String::new(),
            font_family: "Sans".into(),
            size_mm: 10.0,
            bold: false,
            italic: false,
            letter_spacing_mm: 0.0,
            line_spacing: 1.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageData {
    /// Encoded PNG/JPEG bytes.
    pub encoded: SharedBytes,
    pub width_px: u32,
    pub height_px: u32,
    /// Placement rectangle in local coordinates (mm).
    pub rect: [f64; 4],
}

impl ImageData {
    /// Wraps PNG bytes, reading the pixel size from the IHDR chunk.
    pub fn from_png(encoded: impl Into<SharedBytes>, rect: Rect) -> Option<Self> {
        let encoded = encoded.into();
        let (width_px, height_px) = png_size(&encoded)?;
        Some(Self { encoded, width_px, height_px, rect: [rect.x0, rect.y0, rect.x1, rect.y1] })
    }

    /// Wraps PNG or JPEG bytes, reading the pixel size from the image header.
    pub fn from_encoded(encoded: impl Into<SharedBytes>, rect: Rect) -> Option<Self> {
        let encoded = encoded.into();
        let (width_px, height_px) = image_size(&encoded)?;
        Some(Self { encoded, width_px, height_px, rect: [rect.x0, rect.y0, rect.x1, rect.y1] })
    }

    pub fn placement(&self) -> Rect {
        let [x0, y0, x1, y1] = self.rect;
        Rect::new(x0, y0, x1, y1)
    }

    /// MIME type from the encoded bytes' signature.
    pub fn mime(&self) -> &'static str {
        if self.encoded.starts_with(b"\xFF\xD8") { "image/jpeg" } else { "image/png" }
    }
}

/// Pixel size of a PNG from its IHDR chunk, or `None` if `b` is not a PNG.
pub fn png_size(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 24 || !b.starts_with(b"\x89PNG\r\n\x1a\n") || &b[12..16] != b"IHDR" {
        return None;
    }
    let be = |i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    let (w, h) = (be(16), be(20));
    (w > 0 && h > 0).then_some((w, h))
}

/// Pixel size of a JPEG from its SOF marker, or `None` if `b` is not a valid JPEG.
pub fn jpeg_size(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 4 || !b.starts_with(b"\xFF\xD8") {
        return None;
    }
    let mut i = 2;
    while i + 3 < b.len() {
        if b[i] != 0xFF {
            i += 1;
            continue;
        }
        while i < b.len() && b[i] == 0xFF {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        let marker = b[i];
        i += 1;
        if marker == 0xD8 || marker == 0xD9 || (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            continue;
        }
        let len = u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]) as usize;
        if len < 2 || i + len > b.len() {
            return None;
        }
        // SOF0..SOF3, SOF5..SOF7, SOF9..SOF11, SOF13..SOF15
        if matches!(marker, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) && len >= 7 {
            let h = u32::from(u16::from_be_bytes([b[i + 3], b[i + 4]]));
            let w = u32::from(u16::from_be_bytes([b[i + 5], b[i + 6]]));
            return (w > 0 && h > 0).then_some((w, h));
        }
        i += len;
    }
    None
}

/// Pixel size of a PNG or JPEG image.
pub fn image_size(b: &[u8]) -> Option<(u32, u32)> {
    png_size(b).or_else(|| jpeg_size(b))
}
/// Standard padded base64, used for embedded images in project files.
pub mod b64 {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn encode(b: &[u8]) -> String {
        let mut out = String::with_capacity(b.len().div_ceil(3) * 4);
        for c in b.chunks(3) {
            let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            for i in 0..4 {
                if i <= c.len() {
                    out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    /// Decodes, ignoring padding and whitespace. Returns `None` on any other character.
    pub fn decode(s: &str) -> Option<Vec<u8>> {
        let mut out = Vec::with_capacity(s.len() / 4 * 3);
        let mut acc = 0u32;
        let mut bits = 0;
        for ch in s.bytes().filter(|c| *c != b'=' && !c.is_ascii_whitespace()) {
            let v = T.iter().position(|t| *t == ch)?;
            acc = acc << 6 | v as u32;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((acc >> bits) as u8);
                acc &= (1 << bits) - 1;
            }
        }
        Some(out)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ObjectKind {
    Path { path: SharedPath, fill_rule: FillRule },
    Group { children: Vec<Object> },
    Text { data: TextData, outlines: SharedPath },
    Image(ImageData),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Object {
    pub id: ObjectId,
    pub name: String,
    pub layer: LayerId,
    #[serde(with = "affine_serde")]
    pub transform: Affine,
    pub style: Style,
    pub cut: CutAction,
    pub visible: bool,
    pub locked: bool,
    pub kind: ObjectKind,
}

impl Object {
    pub fn path(name: impl Into<String>, layer: LayerId, path: BezPath) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            layer,
            transform: Affine::IDENTITY,
            style: Style::default(),
            cut: CutAction::Cut,
            visible: true,
            locked: false,
            kind: ObjectKind::Path { path: SharedPath::new(path), fill_rule: FillRule::EvenOdd },
        }
    }

    /// A text object; `outlines` are the glyph outlines in local mm, as produced by `ss-text`.
    pub fn text(layer: LayerId, data: TextData, outlines: BezPath) -> Self {
        let name = data.text.lines().next().unwrap_or_default().chars().take(24).collect::<String>();
        let mut o = Self::path(if name.is_empty() { "Text".to_string() } else { name }, layer, BezPath::new());
        o.style.fill = Some(Rgba::BLACK);
        o.kind = ObjectKind::Text { data, outlines: SharedPath::new(outlines) };
        o
    }

    /// A placed raster image. Images are printed and displayed, never cut.
    pub fn image(name: impl Into<String>, layer: LayerId, img: ImageData) -> Self {
        let mut o = Self::path(name, layer, BezPath::new());
        o.cut = CutAction::NoCut;
        o.kind = ObjectKind::Image(img);
        o
    }

    /// Outlines in page coordinates, flattened through groups. Images contribute their placement
    /// rectangle, which is what selection and hit testing need.
    pub fn page_paths(&self) -> Vec<BezPath> {
        let mut out = Vec::new();
        self.collect_paths(Affine::IDENTITY, true, &mut out);
        out
    }

    /// Outlines the cutter or a vector exporter should see: like [`Self::page_paths`] but without
    /// image rectangles.
    pub fn cut_paths(&self) -> Vec<BezPath> {
        let mut out = Vec::new();
        self.collect_paths(Affine::IDENTITY, false, &mut out);
        out
    }

    /// Images in this object (itself or inside groups) with their local-to-page transform.
    pub fn page_images(&self) -> Vec<(&ImageData, Affine)> {
        let mut out = Vec::new();
        self.collect_images(Affine::IDENTITY, &mut out);
        out
    }

    fn collect_images<'a>(&'a self, parent: Affine, out: &mut Vec<(&'a ImageData, Affine)>) {
        let m = parent * self.transform;
        match &self.kind {
            ObjectKind::Image(img) => out.push((img, m)),
            ObjectKind::Group { children } => children.iter().for_each(|c| c.collect_images(m, out)),
            _ => {}
        }
    }

    fn collect_paths(&self, parent: Affine, images: bool, out: &mut Vec<BezPath>) {
        let m = parent * self.transform;
        match &self.kind {
            ObjectKind::Path { path, .. } | ObjectKind::Text { outlines: path, .. } => {
                if !path.elements().is_empty() {
                    out.push(ops::transformed(path, m));
                }
            }
            ObjectKind::Group { children } => {
                for c in children {
                    c.collect_paths(m, images, out);
                }
            }
            ObjectKind::Image(img) if images => {
                out.push(ops::transformed(&ss_geom::Shape::to_path(&img.placement(), 0.0), m));
            }
            ObjectKind::Image(_) => {}
        }
    }

    pub fn bounds(&self) -> Option<Rect> {
        ops::bounds(&self.page_paths())
    }

    pub fn fill_rule(&self) -> FillRule {
        match &self.kind {
            ObjectKind::Path { fill_rule, .. } => *fill_rule,
            // TrueType and CFF outlines are defined with the non-zero rule.
            ObjectKind::Text { .. } => FillRule::NonZero,
            _ => FillRule::EvenOdd,
        }
    }

    /// Fresh ids for this object and all its children (used by duplicate / paste).
    pub fn reassign_ids(&mut self) {
        self.id = Uuid::new_v4();
        if let ObjectKind::Group { children } = &mut self.kind {
            children.iter_mut().for_each(Object::reassign_ids);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    pub visible: bool,
    pub locked: bool,
    pub color: Rgba,
}

impl Layer {
    pub fn new(name: impl Into<String>) -> Self {
        Self { id: Uuid::new_v4(), name: name.into(), visible: true, locked: false, color: Rgba::BLUE }
    }
}

/// Print & Cut registration marks in the Silhouette "type 1" layout: a filled square at the
/// top-left corner and L-shaped brackets at the top-right and bottom-left corners.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RegMarks {
    /// Distance from the page edge to the outer edge of each mark, mm.
    pub inset_mm: f64,
    /// Arm length of the L-brackets, mm.
    pub length_mm: f64,
    /// Line thickness of the L-brackets, mm.
    pub thickness_mm: f64,
    /// Side of the top-left square, mm.
    pub square_mm: f64,
}

impl Default for RegMarks {
    fn default() -> Self {
        Self { inset_mm: 10.0, length_mm: 20.0, thickness_mm: 0.5, square_mm: 5.0 }
    }
}

impl RegMarks {
    pub fn validate(&self, page: &Page) -> Result<(), &'static str> {
        let finite = [self.inset_mm, self.length_mm, self.thickness_mm, self.square_mm].iter().all(|v| v.is_finite());
        if !finite || self.inset_mm < 0.0 {
            return Err("registration mark inset must be a non-negative number");
        }
        if self.length_mm <= 0.0 || self.thickness_mm <= 0.0 || self.square_mm <= 0.0 {
            return Err("registration mark sizes must be positive");
        }
        if self.thickness_mm >= self.length_mm {
            return Err("registration mark thickness must be smaller than its length");
        }
        let span = 2.0 * (self.inset_mm + self.length_mm.max(self.square_mm));
        if span > page.width_mm || span > page.height_mm {
            return Err("registration marks do not fit on the page");
        }
        Ok(())
    }

    /// Rectangle spanned by the outer edges of the marks, in page mm. After registration the
    /// cutter measures coordinates from this rectangle's top-left corner.
    pub fn area(&self, page: &Page) -> Rect {
        let i = self.inset_mm;
        Rect::new(i, i, page.width_mm - i, page.height_mm - i)
    }

    /// Filled outlines of the three marks in page mm. They are printed, never cut.
    pub fn outlines(&self, page: &Page) -> Vec<BezPath> {
        let a = self.area(page);
        let (l, t, s) = (self.length_mm, self.thickness_mm, self.square_mm);
        let poly = |pts: &[(f64, f64)]| {
            let mut p = BezPath::new();
            p.move_to(pts[0]);
            pts[1..].iter().for_each(|q| p.line_to(*q));
            p.close_path();
            p
        };
        vec![
            poly(&[(a.x0, a.y0), (a.x0 + s, a.y0), (a.x0 + s, a.y0 + s), (a.x0, a.y0 + s)]),
            poly(&[(a.x1 - l, a.y0), (a.x1, a.y0), (a.x1, a.y0 + l), (a.x1 - t, a.y0 + l), (a.x1 - t, a.y0 + t), (a.x1 - l, a.y0 + t)]),
            poly(&[(a.x0, a.y1 - l), (a.x0 + t, a.y1 - l), (a.x0 + t, a.y1 - t), (a.x0 + l, a.y1 - t), (a.x0 + l, a.y1), (a.x0, a.y1)]),
        ]
    }

    /// Zones around each mark that must stay free of cuts and artwork so the optical sensor can
    /// read the marks reliably (mark bounds grown by `clearance_mm`).
    pub fn keep_out(&self, page: &Page, clearance_mm: f64) -> Vec<Rect> {
        self.outlines(page).iter().map(|p| ss_geom::Shape::bounding_box(p).inflate(clearance_mm, clearance_mm)).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Page {
    pub width_mm: f64,
    pub height_mm: f64,
    /// Media preset name such as `12x12`, `letter`, `a4`.
    pub media: String,
    /// Cutter "no cut" border, mm.
    pub margin_mm: f64,
    pub show_cut_border: bool,
    /// Print & Cut registration marks, if enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registration: Option<RegMarks>,
}

impl Default for Page {
    fn default() -> Self {
        Self::preset("12x12").unwrap_or(Self {
            width_mm: 304.8,
            height_mm: 304.8,
            media: "12x12".into(),
            margin_mm: 6.35,
            show_cut_border: true,
            registration: None,
        })
    }
}
impl Page {
    /// Media presets seen in the original (`studio_plot_data_header` mat identifiers).
    pub fn preset(name: &str) -> Option<Self> {
        const IN: f64 = 25.4;
        let (w, h) = match name.to_ascii_lowercase().as_str() {
            "letter" => (8.5 * IN, 11.0 * IN),
            "a4" => (210.0, 297.0),
            "a3" => (297.0, 420.0),
            "legal" => (8.5 * IN, 14.0 * IN),
            "8x12" => (8.0 * IN, 12.0 * IN),
            "12x12" => (12.0 * IN, 12.0 * IN),
            "12x24" => (12.0 * IN, 24.0 * IN),
            "15x15" => (15.0 * IN, 15.0 * IN),
            "24x24" => (24.0 * IN, 24.0 * IN),
            "6x8" => (6.0 * IN, 8.0 * IN),
            _ => return None,
        };
        Some(Self { width_mm: w, height_mm: h, media: name.to_string(), margin_mm: 6.35, show_cut_border: true, registration: None })
    }

    pub fn rect(&self) -> Rect {
        Rect::new(0.0, 0.0, self.width_mm, self.height_mm)
    }
}

/// A document. `objects` are in z-order, back to front.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub page: Page,
    pub layers: Vec<Layer>,
    pub objects: Vec<Object>,
    /// Background paper artwork shown under the design. Display and print only, never cut.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<ImageData>,
}

impl Default for Document {
    fn default() -> Self {
        Self { page: Page::default(), layers: vec![Layer::new("Layer 1")], objects: Vec::new(), background: None }
    }
}

impl Document {
    pub fn default_layer(&self) -> LayerId {
        self.layers.first().map(|l| l.id).unwrap_or_default()
    }

    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        self.layers.iter().find(|l| l.id == id)
    }

    pub fn find(&self, id: ObjectId) -> Option<&Object> {
        self.objects.iter().find(|o| o.id == id)
    }

    pub fn find_mut(&mut self, id: ObjectId) -> Option<&mut Object> {
        self.objects.iter_mut().find(|o| o.id == id)
    }

    pub fn index_of(&self, id: ObjectId) -> Option<usize> {
        self.objects.iter().position(|o| o.id == id)
    }

    /// Objects that are drawn and cut: visible themselves and on a visible layer.
    pub fn visible_objects(&self) -> impl Iterator<Item = &Object> {
        self.objects.iter().filter(|o| o.visible && self.layer(o.layer).is_none_or(|l| l.visible))
    }

    pub fn bounds(&self) -> Option<Rect> {
        self.objects.iter().filter_map(Object::bounds).reduce(|a, b| a.union(b))
    }

    /// Topmost object hit at `pt` (page mm).
    pub fn hit_test(&self, pt: ss_geom::Point, tol: f64) -> Option<ObjectId> {
        self.visible_objects()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .find(|o| {
                !self.layer(o.layer).is_some_and(|l| l.locked)
                    && o.page_paths().iter().any(|p| ops::hit_test(p, pt, tol, o.style.fill.is_some() || matches!(o.kind, ObjectKind::Image(_))))
            })
            .map(|o| o.id)
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(&NativeFile { format: NATIVE_FORMAT.into(), version: NATIVE_VERSION, document: self.clone() })
    }

    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        let f: NativeFile = serde_json::from_str(s)?;
        Ok(f.document)
    }
}

pub const NATIVE_FORMAT: &str = "silhouette-studio-rs";
pub const NATIVE_VERSION: u32 = 1;
pub const NATIVE_EXTENSION: &str = "ssrs";

#[derive(Serialize, Deserialize)]
struct NativeFile {
    format: String,
    version: u32,
    document: Document,
}
