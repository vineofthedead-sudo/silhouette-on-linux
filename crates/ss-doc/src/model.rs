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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Style {
    pub stroke: Rgba,
    pub stroke_width_mm: f64,
    pub fill: Option<Rgba>,
}

impl Default for Style {
    fn default() -> Self {
        Self { stroke: Rgba::RED, stroke_width_mm: 0.25, fill: None }
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
    pub size_mm: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageData {
    /// Encoded PNG/JPEG bytes.
    #[serde(with = "bytes_b64")]
    pub encoded: Vec<u8>,
    pub width_px: u32,
    pub height_px: u32,
    /// Placement rectangle in local coordinates (mm).
    pub rect: [f64; 4],
}

mod bytes_b64 {
    use serde::{Deserialize, Deserializer, Serializer};
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn serialize<S: Serializer>(b: &[u8], s: S) -> Result<S::Ok, S::Error> {
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
        s.serialize_str(&out)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        let mut out = Vec::with_capacity(s.len() / 4 * 3);
        let mut acc = 0u32;
        let mut bits = 0;
        for ch in s.bytes().filter(|c| *c != b'=' && !c.is_ascii_whitespace()) {
            let v = T.iter().position(|t| *t == ch).ok_or_else(|| serde::de::Error::custom("bad base64"))?;
            acc = acc << 6 | v as u32;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((acc >> bits) as u8);
                acc &= (1 << bits) - 1;
            }
        }
        Ok(out)
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

    /// Outlines in page coordinates, flattened through groups.
    pub fn page_paths(&self) -> Vec<BezPath> {
        let mut out = Vec::new();
        self.collect_paths(Affine::IDENTITY, &mut out);
        out
    }

    fn collect_paths(&self, parent: Affine, out: &mut Vec<BezPath>) {
        let m = parent * self.transform;
        match &self.kind {
            ObjectKind::Path { path, .. } | ObjectKind::Text { outlines: path, .. } => {
                if !path.elements().is_empty() {
                    out.push(ops::transformed(path, m));
                }
            }
            ObjectKind::Group { children } => {
                for c in children {
                    c.collect_paths(m, out);
                }
            }
            ObjectKind::Image(img) => {
                let [x0, y0, x1, y1] = img.rect;
                out.push(ops::transformed(&ss_geom::Shape::to_path(&Rect::new(x0, y0, x1, y1), 0.0), m));
            }
        }
    }

    pub fn bounds(&self) -> Option<Rect> {
        ops::bounds(&self.page_paths())
    }

    pub fn fill_rule(&self) -> FillRule {
        match &self.kind {
            ObjectKind::Path { fill_rule, .. } => *fill_rule,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Page {
    pub width_mm: f64,
    pub height_mm: f64,
    /// Media preset name such as `12x12`, `letter`, `a4`.
    pub media: String,
    /// Cutter "no cut" border, mm.
    pub margin_mm: f64,
    pub show_cut_border: bool,
}

impl Default for Page {
    fn default() -> Self {
        Self::preset("12x12").unwrap_or(Self {
            width_mm: 304.8,
            height_mm: 304.8,
            media: "12x12".into(),
            margin_mm: 6.35,
            show_cut_border: true,
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
        Some(Self { width_mm: w, height_mm: h, media: name.to_string(), margin_mm: 6.35, show_cut_border: true })
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
}

impl Default for Document {
    fn default() -> Self {
        Self { page: Page::default(), layers: vec![Layer::new("Layer 1")], objects: Vec::new() }
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
                    && o.page_paths().iter().any(|p| ops::hit_test(p, pt, tol, o.style.fill.is_some()))
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
