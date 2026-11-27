//! Drawing canvas: mat and page, objects, selection handles and the drawing tools.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use egui::{Color32, Key, PointerButton, Pos2, Sense, Shape, Stroke, StrokeKind, pos2, vec2};
use ss_config::Settings;
use ss_doc::{CutAction, Document, Editor, ImageData, LayerId, Object, ObjectId, ObjectKind, RegMarks, ToolGesture, ToolState};
use ss_geom::fit::{FitOptions, fit_contour};
use ss_geom::flatten::{Polyline, flatten};
use ss_geom::nodes::{HandleSide, NodeId, NodePath, Pick};
use ss_geom::tools::{self as geom_tools, BrushShape};
use ss_geom::{Affine, BezPath, Point, Rect, Shape as _, Vec2};

use crate::app::Action;
pub use ss_doc::Tool;
use ss_doc::CuttingMat;

const RENDER_TOLERANCE: f64 = 0.02;
const HANDLE_HALF: f32 = 4.0;
const ROTATE_OFFSET: f32 = 22.0;
const PICK_PX: f64 = 5.0;
pub const DEFAULT_ERASER_RADIUS_MM: f64 = 2.5;
const MIN_ZOOM: f32 = 0.02;
const MAX_ZOOM: f32 = 400.0;
/// Points per mm at 100 % (96 dpi).
pub const ACTUAL_SIZE_ZOOM: f32 = 96.0 / 25.4;
pub const SELECTION_COLOR: Color32 = Color32::from_rgb(0x1e, 0x88, 0xe5);
const OVERLAY_COLOR: Color32 = Color32::from_rgb(0x00, 0xa0, 0x50);
/// Larger images are downscaled before upload; GPUs commonly cap texture sides at 8192.
const MAX_TEXTURE_SIDE: u32 = 4096;
/// Scale handles as fractions of the selection box, clockwise from the top-left corner.
const HANDLES: [(f64, f64); 8] = [(0.0, 0.0), (0.5, 0.0), (1.0, 0.0), (1.0, 0.5), (1.0, 1.0), (0.5, 1.0), (0.0, 1.0), (0.0, 0.5)];

pub fn tool_key(tool: Tool) -> Key {
    match tool {
        Tool::Select => Key::V,
        Tool::EditPoints => Key::A,
        Tool::Rectangle => Key::R,
        Tool::Ellipse => Key::E,
        Tool::Line => Key::L,
        Tool::Polyline => Key::P,
        Tool::Freehand => Key::F,
        Tool::Text => Key::T,
        Tool::Knife => Key::K,
        Tool::Eraser => Key::X,
    }
}

/// Screen position = canvas origin + `offset` + document mm × `zoom`.
#[derive(Debug, Clone, Copy)]
pub struct View {
    pub offset: egui::Vec2,
    pub zoom: f32,
}

impl Default for View {
    fn default() -> Self {
        Self { offset: vec2(40.0, 40.0), zoom: 2.0 }
    }
}

impl View {
    pub fn to_screen(self, origin: Pos2, p: Point) -> Pos2 {
        pos2(origin.x + self.offset.x + p.x as f32 * self.zoom, origin.y + self.offset.y + p.y as f32 * self.zoom)
    }

    pub fn to_doc(self, origin: Pos2, s: Pos2) -> Point {
        Point::new(f64::from((s.x - origin.x - self.offset.x) / self.zoom), f64::from((s.y - origin.y - self.offset.y) / self.zoom))
    }

    fn fit(&mut self, canvas: egui::Rect, r: Rect) {
        let margin = 28.0;
        let w = (canvas.width() - 2.0 * margin).max(20.0);
        let h = (canvas.height() - 2.0 * margin).max(20.0);
        self.zoom = (w / r.width().max(1.0) as f32).min(h / r.height().max(1.0) as f32).clamp(MIN_ZOOM, MAX_ZOOM);
        let c = r.center();
        self.offset = canvas.center() - canvas.min - vec2(c.x as f32 * self.zoom, c.y as f32 * self.zoom);
    }

    fn zoom_at(&mut self, origin: Pos2, at: Pos2, factor: f32) {
        let d = self.to_doc(origin, at);
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.offset = at - origin - vec2(d.x as f32 * self.zoom, d.y as f32 * self.zoom);
    }
}

struct Cached {
    key: u64,
    lines: Vec<Polyline>,
    bbox: Option<Rect>,
    fill_verts: Vec<Point>,
    fill_indices: Vec<u32>,
}

#[derive(Debug, Clone, Copy)]
enum Handle {
    Scale(usize),
    Rotate,
}

#[derive(Debug, Clone, Copy)]
enum Drag {
    None,
    Marquee { start: Point },
    Move { start: Point },
    Scale { anchor: Point, start: Point, sx_on: bool, sy_on: bool },
    Rotate { center: Point, start_angle: f64 },
    Create { start: Point },
    Freehand,
}

/// A point or handle being dragged with Edit Points. The document is edited live and the whole
/// drag is recorded as one undo step on release.
struct NodeEdit {
    id: ObjectId,
    pick: Pick,
    nodes: NodePath,
    inverse: Affine,
    grab: Vec2,
    before: Document,
    moved: bool,
}

pub struct Canvas {
    pub view: View,
    pub mat: CuttingMat,
    pending_fit: Option<Rect>,
    last_rect: egui::Rect,
    drag: Drag,
    /// Live transform of the selection while dragging; committed as one undo step on release.
    preview: Affine,
    current: Point,
    points: Vec<Point>,
    cache: HashMap<ObjectId, Cached>,
    pub cursor_mm: Option<Point>,
    /// Tool paths reported by the virtual cutter, drawn on top of the design.
    pub overlay: Vec<Vec<Point>>,
    /// Uploaded image textures by content fingerprint; `None` when the image could not be decoded.
    textures: HashMap<u64, Option<egui::TextureHandle>>,
    /// Eraser brush half-size in mm and shape; session-only tool options.
    pub eraser_radius_mm: f64,
    pub eraser_shape: BrushShape,
    /// Edit Points: the selected node of the single selected path.
    selected_node: Option<(ObjectId, NodeId)>,
    node_drag: Option<NodeEdit>,
    /// Nodes explicitly toggled to Corner while their handles are still collinear.
    corner_nodes: HashSet<(ObjectId, usize, usize)>,
    /// Simplify tolerance (mm) for Edit Points Tool Options.
    pub simplify_tolerance_mm: f64,
    /// Auto-Trace preview contours in page mm, drawn over the design.
    pub trace_preview: Vec<Vec<Point>>,
}

impl Default for Canvas {
    fn default() -> Self {
        Self {
            view: View::default(),
            mat: CuttingMat::default(),
            pending_fit: None,
            last_rect: egui::Rect::NOTHING,
            drag: Drag::None,
            preview: Affine::IDENTITY,
            current: Point::ZERO,
            points: Vec::new(),
            cache: HashMap::new(),
            cursor_mm: None,
            overlay: Vec::new(),
            textures: HashMap::new(),
            eraser_radius_mm: DEFAULT_ERASER_RADIUS_MM,
            eraser_shape: BrushShape::Round,
            selected_node: None,
            node_drag: None,
            corner_nodes: HashSet::new(),
            simplify_tolerance_mm: 0.2,
            trace_preview: Vec::new(),
        }
    }
}

fn content_key(o: &Object, h: &mut DefaultHasher) {
    (o.style.fill.is_some() || o.style.fill_gradient.is_some() || o.style.fill_pattern.is_some()).hash(h);
    for c in o.transform.as_coeffs() {
        c.to_bits().hash(h);
    }
    match &o.kind {
        ObjectKind::Path { path, .. } | ObjectKind::Text { outlines: path, .. } => {
            (Arc::as_ptr(&path.0) as usize).hash(h);
            path.elements().len().hash(h);
        }
        ObjectKind::Group { children } => {
            children.len().hash(h);
            for c in children {
                content_key(c, h);
            }
        }
        ObjectKind::Image(img) => img.rect.map(f64::to_bits).hash(h),
    }
}

fn transform_rect(m: Affine, r: Rect) -> Rect {
    let pts = [Point::new(r.x0, r.y0), Point::new(r.x1, r.y0), Point::new(r.x1, r.y1), Point::new(r.x0, r.y1)].map(|p| m * p);
    pts[1..].iter().fold(Rect::from_points(pts[0], pts[0]), |acc, p| acc.union_pt(*p))
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.x0 <= b.x1 && b.x0 <= a.x1 && a.y0 <= b.y1 && b.y0 <= a.y1
}

fn seg_distance(p: Point, a: Point, b: Point) -> f64 {
    let ab = b - a;
    let len2 = ab.hypot2();
    let t = if len2 == 0.0 { 0.0 } else { ((p - a).dot(ab) / len2).clamp(0.0, 1.0) };
    p.distance(a.lerp(b, t))
}

fn inside(p: Point, poly: &[Point]) -> bool {
    let mut c = false;
    let mut j = poly.len().wrapping_sub(1);
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            c = !c;
        }
        j = i;
    }
    c
}

/// Cheap identity for an embedded image. Hashing every byte each frame would be too slow for
/// multi-megabyte photos, so only the length and samples from the start, middle and end are used.
fn bytes_key(b: &[u8], w: u32, h_px: u32) -> u64 {
    let mut h = DefaultHasher::new();
    (b.len(), w, h_px).hash(&mut h);
    let mid = b.len() / 2;
    b[..b.len().min(1024)].hash(&mut h);
    b[mid.saturating_sub(512)..(mid + 512).min(b.len())].hash(&mut h);
    b[b.len().saturating_sub(4096)..].hash(&mut h);
    h.finish()
}

fn image_key(img: &ImageData) -> u64 {
    bytes_key(&img.encoded, img.width_px, img.height_px)
}

fn decode_bytes(encoded: &[u8]) -> Option<egui::ColorImage> {
    let mut decoded = image::load_from_memory(encoded).ok()?;
    if decoded.width() > MAX_TEXTURE_SIDE || decoded.height() > MAX_TEXTURE_SIDE {
        decoded = decoded.thumbnail(MAX_TEXTURE_SIDE, MAX_TEXTURE_SIDE);
    }
    let rgba = decoded.to_rgba8();
    Some(egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw()))
}

fn decode_image(img: &ImageData) -> Option<egui::ColorImage> {
    decode_bytes(&img.encoded)
}

/// The three registration marks as axis-aligned filled rectangles (page mm).
fn regmark_rects(m: &RegMarks, page: &ss_doc::Page) -> [Rect; 5] {
    let a = m.area(page);
    let (l, t, s) = (m.length_mm, m.thickness_mm, m.square_mm);
    [
        Rect::new(a.x0, a.y0, a.x0 + s, a.y0 + s),
        Rect::new(a.x1 - l, a.y0, a.x1, a.y0 + t),
        Rect::new(a.x1 - t, a.y0, a.x1, a.y0 + l),
        Rect::new(a.x0, a.y1 - l, a.x0 + t, a.y1),
        Rect::new(a.x0, a.y1 - t, a.x0 + l, a.y1),
    ]
}

/// The Knife cut for a finished or in-progress drag: a straight line (Shift snaps it to 45°), or
/// the freehand path with Ctrl held.
fn knife_cut_points(gesture: &ToolGesture, mods: egui::Modifiers) -> Vec<Point> {
    if mods.ctrl {
        return gesture.points.clone();
    }
    let end = if mods.shift { geom_tools::snap_angle(gesture.start, gesture.current) } else { gesture.current };
    vec![gesture.start, end]
}

pub fn color32(c: ss_doc::Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

impl Canvas {
    pub fn fit_to(&mut self, r: Rect) {
        self.pending_fit = Some(r);
    }

    /// Zooms around the centre of the canvas.
    pub fn zoom_by(&mut self, factor: f32) {
        if self.last_rect.is_positive() {
            let r = self.last_rect;
            self.view.zoom_at(r.min, r.center(), factor);
        }
    }

    pub fn zoom_actual(&mut self) {
        let f = ACTUAL_SIZE_ZOOM / self.view.zoom;
        self.zoom_by(f);
    }

    pub fn cancel_interaction(&mut self) {
        self.drag = Drag::None;
        self.preview = Affine::IDENTITY;
        self.points.clear();
        self.node_drag = None;
    }

    /// Abandons a point drag in progress and restores the document to how it was before it began.
    pub fn abort_node_drag(&mut self, editor: &mut Editor) {
        if let Some(edit) = self.node_drag.take()
            && edit.moved
        {
            editor.doc = edit.before;
        }
    }

    /// The selected node if it still exists and its path is still the only selection.
    fn valid_node(&self, editor: &Editor) -> Option<(ObjectId, NodeId)> {
        let (id, node) = self.selected_node?;
        let (sole, _) = Self::sole_path(editor)?;
        (sole == id && editor.path_nodes(id)?.node(node).is_some()).then_some((id, node))
    }

    /// Deletes the selected Edit Points node. Returns whether a node was selected (even if it
    /// could not be deleted), so the caller does not delete the whole object instead.
    pub fn delete_selected_node(&mut self, editor: &mut Editor, status: &mut String) -> bool {
        let Some((id, node)) = self.valid_node(editor) else { return false };
        match editor.delete_path_node(id, node) {
            Ok(()) => {
                self.selected_node = None;
                *status = "Deleted point".into();
            }
            Err(e) => *status = e.to_string(),
        }
        true
    }

    pub fn break_selected_node(&mut self, editor: &mut Editor, status: &mut String) {
        let Some((id, node)) = self.valid_node(editor) else { return };
        match editor.break_path_node(id, node) {
            Ok(ep) => {
                self.selected_node = Some((id, ep));
                *status = "Broke path at selected point".into();
            }
            Err(e) => *status = e.to_string(),
        }
    }

    pub fn set_selected_node_smooth(&mut self, editor: &mut Editor, smooth: bool, status: &mut String) {
        let Some((id, node)) = self.valid_node(editor) else { return };
        match editor.set_node_smooth(id, node, smooth) {
            Ok(()) => {
                if smooth {
                    self.corner_nodes.remove(&(id, node.sub, node.index));
                } else {
                    self.corner_nodes.insert((id, node.sub, node.index));
                }
                *status = if smooth { "Set point to Smooth" } else { "Set point to Corner" }.into();
            }
            Err(e) => *status = e.to_string(),
        }
    }

    pub fn set_selected_segment_curved(&mut self, editor: &mut Editor, curved: bool, status: &mut String) {
        let Some((id, node)) = self.valid_node(editor) else { return };
        match editor.set_segment_curved(id, node, curved) {
            Ok(()) => *status = if curved { "Converted segment to Curve" } else { "Converted segment to Flat" }.into(),
            Err(e) => *status = e.to_string(),
        }
    }

    pub fn simplify_selected_path(&mut self, editor: &mut Editor, status: &mut String) {
        let Some((id, _)) = Self::sole_path(editor) else { return };
        match editor.simplify_path(id, self.simplify_tolerance_mm) {
            Ok((before, after)) => {
                self.selected_node = None;
                *status = format!("Simplified path: {before} → {after} points");
            }
            Err(e) => *status = e.to_string(),
        }
    }

    /// Point Editing toolbar controls rendered inside Tool Options.
    pub fn edit_points_ui(&mut self, ui: &mut egui::Ui, editor: &mut Editor, status: &mut String) {
        let Some((id, _)) = Self::sole_path(editor) else {
            ui.weak("Select one ungrouped path to edit its points.");
            return;
        };
        let valid = self.valid_node(editor).map(|(_, n)| n);
        let nodes = editor.path_nodes(id);
        let is_smooth = valid
            .and_then(|n| {
                (!self.corner_nodes.contains(&(id, n.sub, n.index)))
                    .then(|| nodes.as_ref()?.node(n).map(|nd| nd.smooth))
                    .flatten()
            })
            .unwrap_or(false);
        let seg_curved = valid.and_then(|n| nodes.as_ref()?.segment_is_curved(n));

        ui.add_enabled_ui(valid.is_some(), |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button("Break Path").on_hover_text("Split the path at the selected point").clicked() {
                    self.break_selected_node(editor, status);
                }
                if ui.button("Delete Point").on_hover_text("Delete the selected point (Delete / Backspace)").clicked() {
                    self.delete_selected_node(editor, status);
                }
            });
            ui.horizontal_wrapped(|ui| {
                if ui.selectable_label(!is_smooth, "Corner").on_hover_text("Independent sharp-corner handles").clicked() {
                    self.set_selected_node_smooth(editor, false, status);
                }
                if ui.selectable_label(is_smooth, "Smooth").on_hover_text("Collinear smooth handles").clicked() {
                    self.set_selected_node_smooth(editor, true, status);
                }
            });
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(seg_curved == Some(true), egui::Button::new("Make Flat"))
                    .on_hover_text("Convert the red segment to a straight line")
                    .clicked()
                {
                    self.set_selected_segment_curved(editor, false, status);
                }
                if ui
                    .add_enabled(seg_curved == Some(false), egui::Button::new("Make Curve"))
                    .on_hover_text("Convert the red segment to a Bézier curve")
                    .clicked()
                {
                    self.set_selected_segment_curved(editor, true, status);
                }
            });
        });
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Simplify").on_hover_text("Reduce redundant points within tolerance").clicked() {
                self.simplify_selected_path(editor, status);
            }
            ui.add(egui::DragValue::new(&mut self.simplify_tolerance_mm).range(0.02..=5.0).speed(0.02).suffix(" mm"));
        });
    }

    /// The single selected, ungrouped path and its transform.
    fn sole_path(editor: &Editor) -> Option<(ObjectId, Affine)> {
        let [id] = editor.selection.as_slice() else { return None };
        let o = editor.doc.find(*id)?;
        (matches!(o.kind, ObjectKind::Path { .. }) && !o.locked && o.transform.determinant().abs() > 1e-12).then_some((*id, o.transform))
    }

    /// Union of the cached bounds of `ids` (fast, no re-flattening).
    pub fn bounds_of(&self, ids: &[ObjectId]) -> Option<Rect> {
        ids.iter().filter_map(|id| self.cache.get(id)?.bbox).reduce(|a, b| a.union(b))
    }

    fn refresh_cache(&mut self, editor: &Editor) {
        let mut seen = HashSet::with_capacity(editor.doc.objects.len());
        for o in &editor.doc.objects {
            let mut h = DefaultHasher::new();
            content_key(o, &mut h);
            let key = h.finish();
            seen.insert(o.id);
            if self.cache.get(&o.id).is_some_and(|c| c.key == key) {
                continue;
            }
            let lines: Vec<Polyline> = o.page_paths().iter().flat_map(|p| flatten(p, RENDER_TOLERANCE)).collect();
            let bbox = lines
                .iter()
                .flat_map(|l| l.points.iter())
                .fold(None::<Rect>, |acc, p| Some(acc.map_or(Rect::from_points(*p, *p), |r| r.union_pt(*p))));
            let (mut fill_verts, mut fill_indices) = (Vec::new(), Vec::new());
            if o.style.fill.is_some() || o.style.fill_gradient.is_some() || o.style.fill_pattern.is_some() {
                for p in o.page_paths() {
                    let (v, idx) = ss_geom::triangulate::triangulate_path(&p, o.fill_rule(), RENDER_TOLERANCE);
                    let base = fill_verts.len() as u32;
                    fill_verts.extend(v);
                    fill_indices.extend(idx.into_iter().map(|i| base + i));
                }
            }
            self.cache.insert(o.id, Cached { key, lines, bbox, fill_verts, fill_indices });
        }
        if self.cache.len() != seen.len() {
            self.cache.retain(|id, _| seen.contains(id));
        }
    }

    /// Uploads textures for the background, every visible image, and pattern fills, and drops unused ones.
    fn refresh_textures(&mut self, ctx: &egui::Context, editor: &Editor) {
        let mut images: Vec<&ImageData> = editor.doc.background.iter().collect();
        let mut patterns = Vec::new();
        for o in editor.doc.visible_objects() {
            images.extend(o.page_images().into_iter().map(|(img, _)| img));
            if let Some(pat) = &o.style.fill_pattern {
                patterns.push(pat);
            }
        }
        let mut used = HashSet::with_capacity(images.len() + patterns.len());
        for img in images {
            let key = image_key(img);
            used.insert(key);
            self.textures.entry(key).or_insert_with(|| {
                decode_image(img).map(|ci| ctx.load_texture(format!("image-{key:x}"), ci, egui::TextureOptions::LINEAR))
            });
        }
        for pat in patterns {
            let key = bytes_key(&pat.encoded, pat.width_px, pat.height_px);
            used.insert(key);
            self.textures.entry(key).or_insert_with(|| {
                let opts = egui::TextureOptions {
                    wrap_mode: egui::TextureWrapMode::Repeat,
                    ..egui::TextureOptions::LINEAR
                };
                decode_bytes(&pat.encoded).map(|ci| ctx.load_texture(format!("pattern-{key:x}"), ci, opts))
            });
        }
        self.textures.retain(|k, _| used.contains(k));
    }

    fn image_shape(&self, origin: Pos2, img: &ImageData, m: Affine) -> Option<Shape> {
        let tex = self.textures.get(&image_key(img))?.as_ref()?;
        let r = img.placement();
        let corners = [(r.x0, r.y0), (r.x1, r.y0), (r.x1, r.y1), (r.x0, r.y1)].map(|(x, y)| self.view.to_screen(origin, m * Point::new(x, y)));
        let uvs = [pos2(0.0, 0.0), pos2(1.0, 0.0), pos2(1.0, 1.0), pos2(0.0, 1.0)];
        let mut mesh = egui::Mesh::with_texture(tex.id());
        for (pos, uv) in corners.into_iter().zip(uvs) {
            mesh.vertices.push(egui::epaint::Vertex { pos, uv, color: Color32::WHITE });
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        Some(Shape::mesh(mesh))
    }

    /// Topmost visible, unlocked object under `p`.
    fn pick(&self, editor: &Editor, p: Point, tol: f64) -> Option<ObjectId> {
        let objs: Vec<&Object> = editor.doc.visible_objects().collect();
        objs.into_iter()
            .rev()
            .filter(|o| !o.locked && !editor.doc.layer(o.layer).is_some_and(|l| l.locked))
            .find(|o| {
                let Some(c) = self.cache.get(&o.id) else { return false };
                if !c.bbox.is_some_and(|b| b.inflate(tol, tol).contains(p)) {
                    return false;
                }
                c.lines.iter().any(|l| {
                    let n = l.points.len();
                    let segs = if l.closed { n } else { n - 1 };
                    (0..segs).any(|i| seg_distance(p, l.points[i], l.points[(i + 1) % n]) <= tol)
                        // Closed outlines count as solid even without a fill, so unfilled
                        // cut shapes can be grabbed and dragged from the inside.
                        || (l.closed && inside(p, &l.points))
                })
            })
            .map(|o| o.id)
    }

    fn snap(&self, p: Point, s: &Settings) -> Point {
        if s.snap_to_grid && s.grid_spacing_mm > 0.0 {
            let g = s.grid_spacing_mm;
            Point::new((p.x / g).round() * g, (p.y / g).round() * g)
        } else {
            p
        }
    }

    fn handle_at(&self, origin: Pos2, b: Rect, s: Pos2) -> Option<Handle> {
        let top = self.view.to_screen(origin, Point::new(b.center().x, b.y0));
        if (top - vec2(0.0, ROTATE_OFFSET)).distance(s) <= HANDLE_HALF + 3.0 {
            return Some(Handle::Rotate);
        }
        HANDLES.iter().position(|&(fx, fy)| {
            let h = self.view.to_screen(origin, Point::new(b.x0 + b.width() * fx, b.y0 + b.height() * fy));
            (h.x - s.x).abs() <= HANDLE_HALF + 2.0 && (h.y - s.y).abs() <= HANDLE_HALF + 2.0
        })
        .map(Handle::Scale)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        editor: &mut Editor,
        tool_state: &mut ToolState,
        settings: &Settings,
        layer: LayerId,
        actions: &mut Vec<Action>,
        status: &mut String,
    ) {
        let tool = tool_state.active();
        let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let rect = resp.rect;
        self.last_rect = rect;
        let origin = rect.min;
        if let Some(r) = self.pending_fit.take() {
            self.view.fit(rect, r);
        }
        self.refresh_cache(editor);
        self.refresh_textures(ui.ctx(), editor);

        // View: wheel pans, Ctrl+wheel or pinch zooms, middle drag or Space+drag pans.
        let (scroll, zoom_delta, mods, press, space, typing) = ui.input(|i| {
            (i.smooth_scroll_delta, i.zoom_delta(), i.modifiers, i.pointer.press_origin(), i.key_down(Key::Space), false)
        });
        let typing = typing || ui.ctx().egui_wants_keyboard_input();
        if let Some(h) = resp.hover_pos() {
            if zoom_delta != 1.0 {
                self.view.zoom_at(origin, h, zoom_delta);
            } else if scroll != egui::Vec2::ZERO {
                self.view.offset += scroll;
            }
        }
        let panning = resp.dragged_by(PointerButton::Middle) || (space && !typing && resp.dragged_by(PointerButton::Primary));
        if panning {
            self.view.offset += resp.drag_delta();
        }
        self.cursor_mm = resp.hover_pos().map(|p| self.view.to_doc(origin, p));

        if !typing {
            let (esc, enter) = ui.input(|i| (i.key_pressed(Key::Escape), i.key_pressed(Key::Enter)));
            if esc {
                if matches!(self.drag, Drag::None) && self.points.is_empty() {
                    editor.selection.clear();
                }
                self.abort_node_drag(editor);
                self.cancel_interaction();
                tool_state.cancel();
            }
            if enter && tool == Tool::Polyline {
                self.finish_polyline(editor, layer, false);
            }
        }

        if !panning && !(space && !typing) {
            let pointer_doc = resp.interact_pointer_pos().map(|p| self.view.to_doc(origin, p));
            let press_doc = press.map(|p| self.view.to_doc(origin, p));
            let mut completed = None;

            if resp.drag_started_by(PointerButton::Primary)
                && let Some(p) = press_doc
            {
                tool_state.press(p);
            }
            if resp.dragged_by(PointerButton::Primary)
                && let Some(p) = pointer_doc
            {
                tool_state.drag(p);
            }
            if resp.drag_stopped_by(PointerButton::Primary) {
                if let Some(p) = pointer_doc.or(press_doc) {
                    completed = tool_state.release(p);
                }
            } else if resp.clicked()
                && let Some(p) = pointer_doc.or(press_doc)
            {
                tool_state.press(p);
                completed = tool_state.release(p);
            }

            if resp.double_clicked()
                && let Some(p) = pointer_doc
            {
                if tool == Tool::EditPoints {
                    // Handled by the Edit Points tool: delete a point or leave the mode.
                } else if let Some(id) = self.pick(editor, p, PICK_PX / f64::from(self.view.zoom))
                    && matches!(editor.doc.find(id).map(|o| &o.kind), Some(ObjectKind::Path { .. }))
                {
                    editor.select_only(id);
                    actions.push(Action::SetTool(Tool::EditPoints));
                }
            }

            match tool {
                Tool::Select => self.select_tool(&resp, editor, origin, mods, press, press_doc, pointer_doc, settings, status),
                Tool::EditPoints => self.edit_points_tool(&resp, editor, press_doc, pointer_doc, actions, status),
                Tool::Rectangle | Tool::Ellipse | Tool::Line => {
                    self.shape_tool(tool, &resp, editor, mods, press_doc, pointer_doc, settings, layer);
                }
                Tool::Polyline => self.polyline_tool(&resp, editor, origin, pointer_doc, settings, layer),
                Tool::Freehand => self.freehand_tool(&resp, editor, origin, press_doc, pointer_doc, layer),
                Tool::Text => {
                    if resp.clicked()
                        && let Some(p) = pointer_doc
                    {
                        let p = self.snap(p, settings);
                        actions.push(Action::NewText(p.x, p.y));
                    }
                }
                Tool::Knife | Tool::Eraser => self.finish_vector_tool(tool, &resp, completed.as_ref(), editor, mods, status),
            }
        }

        self.paint(&painter, rect, origin, editor, settings, tool, tool_state.gesture());
        let has_node = tool == Tool::EditPoints && self.valid_node(editor).is_some();
        let mut node_cmd = None;
        resp.context_menu(|ui| {
            if has_node {
                for (label, cmd) in [
                    ("Delete Point", 0u8),
                    ("Break Path", 1),
                    ("Corner", 2),
                    ("Smooth", 3),
                    ("Make Flat", 4),
                    ("Make Curve", 5),
                ] {
                    if ui.button(label).clicked() {
                        node_cmd = Some(cmd);
                        ui.close();
                    }
                }
                ui.separator();
            }
            context_menu(ui, editor, actions);
        });
        match node_cmd {
            Some(0) => {
                self.delete_selected_node(editor, status);
            }
            Some(1) => self.break_selected_node(editor, status),
            Some(2) => self.set_selected_node_smooth(editor, false, status),
            Some(3) => self.set_selected_node_smooth(editor, true, status),
            Some(4) => self.set_selected_segment_curved(editor, false, status),
            Some(5) => self.set_selected_segment_curved(editor, true, status),
            _ => {}
        }
    }

    /// Knife and Eraser act when the drag ends: one undoable edit on every unlocked shape it touches.
    fn finish_vector_tool(
        &mut self,
        tool: Tool,
        resp: &egui::Response,
        completed: Option<&ToolGesture>,
        editor: &mut Editor,
        mods: egui::Modifiers,
        status: &mut String,
    ) {
        if !(resp.drag_stopped_by(PointerButton::Primary) || resp.clicked()) {
            return;
        }
        let Some(gesture) = completed else {
            *status = format!("{} drag ended without coordinates; document unchanged", tool.label());
            return;
        };
        let result = match tool {
            Tool::Knife => editor.knife_cut(&knife_cut_points(gesture, mods)).map(|n| format!("Knife split {n} shape(s)")),
            Tool::Eraser => editor
                .erase_with_brush(&gesture.points, self.eraser_radius_mm, self.eraser_shape)
                .map(|n| format!("Eraser changed {n} shape(s)")),
            _ => return,
        };
        *status = result.unwrap_or_else(|e| format!("{}: {e}", tool.label()));
    }

    /// Edit Points: drag nodes and curve handles, click a line to add a point, double-click a
    /// point to delete it (Delete/Backspace are handled by the app), double-click elsewhere to
    /// leave the mode. Drags edit the document live and commit one undo step on release.
    fn edit_points_tool(
        &mut self,
        resp: &egui::Response,
        editor: &mut Editor,
        press_doc: Option<Point>,
        pointer_doc: Option<Point>,
        actions: &mut Vec<Action>,
        status: &mut String,
    ) {
        let tol_page = PICK_PX / f64::from(self.view.zoom);

        if let Some(mut edit) = self.node_drag.take() {
            let mut keep = true;
            if resp.dragged_by(PointerButton::Primary)
                && let Some(p) = pointer_doc
            {
                let target = edit.inverse * p + edit.grab;
                match edit.pick {
                    Pick::Anchor(n) => edit.nodes.move_anchor(n, target),
                    Pick::Handle(n, side) => edit.nodes.move_handle(n, side, target),
                }
                edit.moved = true;
                editor.set_path_live(edit.id, edit.nodes.to_bez());
            }
            if resp.drag_stopped_by(PointerButton::Primary) {
                keep = false;
                if edit.moved {
                    let label = if matches!(edit.pick, Pick::Anchor(_)) { "Move Point" } else { "Adjust Curve" };
                    editor.commit_edit(label, std::mem::take(&mut edit.before));
                }
            }
            if keep {
                self.node_drag = Some(edit);
            }
            return;
        }

        let sole = Self::sole_path(editor);
        let local_tol = |inv: Affine| tol_page * inv.determinant().abs().sqrt();

        if resp.double_clicked()
            && let Some(p) = pointer_doc
        {
            if let Some((id, m)) = sole {
                let inv = m.inverse();
                if let Some(Pick::Anchor(n)) = editor.path_nodes(id).and_then(|nodes| nodes.pick(inv * p, local_tol(inv), None)) {
                    match editor.delete_path_node(id, n) {
                        Ok(()) => *status = "Deleted point".into(),
                        Err(e) => *status = e.to_string(),
                    }
                    self.selected_node = None;
                    return;
                }
            }
            actions.push(Action::SetTool(Tool::Select));
            return;
        }

        let started = resp.drag_started_by(PointerButton::Primary);
        if !(started || resp.clicked()) {
            return;
        }
        let Some(p) = press_doc.or(pointer_doc) else { return };

        if let Some((id, m)) = sole
            && let Some(mut nodes) = editor.path_nodes(id)
        {
            let inv = m.inverse();
            let tol = local_tol(inv);
            let local = inv * p;
            let selected = self.selected_node.filter(|(sid, _)| *sid == id).map(|(_, n)| n);
            let mut pick = nodes.pick(local, tol, selected);
            if pick.is_none()
                && let Ok(n) = editor.insert_path_node(id, local, tol)
            {
                nodes = editor.path_nodes(id).unwrap_or(nodes);
                pick = Some(Pick::Anchor(n));
                *status = "Added point".into();
            }
            if let Some(pick) = pick {
                let node_id = match pick {
                    Pick::Anchor(n) | Pick::Handle(n, _) => n,
                };
                self.selected_node = Some((id, node_id));
                if started {
                    for &(cid, sub, idx) in &self.corner_nodes {
                        if cid == id
                            && let Some(s) = nodes.subs.get_mut(sub)
                            && let Some(nd) = s.nodes.get_mut(idx)
                        {
                            nd.smooth = false;
                        }
                    }
                    let held = nodes.node(node_id).and_then(|n| match pick {
                        Pick::Anchor(_) => Some(n.pos),
                        Pick::Handle(_, HandleSide::In) => n.hin,
                        Pick::Handle(_, HandleSide::Out) => n.hout,
                    });
                    self.node_drag = Some(NodeEdit {
                        id,
                        pick,
                        nodes,
                        inverse: inv,
                        grab: held.map_or(Vec2::ZERO, |h| h - local),
                        before: editor.doc.clone(),
                        moved: false,
                    });
                }
                return;
            }
        }

        // Nothing under the pointer: pick another path, or clear the selection.
        self.selected_node = None;
        match self.pick(editor, p, tol_page).filter(|id| matches!(editor.doc.find(*id).map(|o| &o.kind), Some(ObjectKind::Path { .. }))) {
            Some(id) => {
                editor.select_only(id);
                *status = "Edit Points: drag a point or handle; click a line to add a point; Delete removes the selected point".into();
            }
            None => {
                editor.selection.clear();
                *status = "Edit Points needs one ungrouped path; click a path to edit its points".into();
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn select_tool(
        &mut self,
        resp: &egui::Response,
        editor: &mut Editor,
        origin: Pos2,
        mods: egui::Modifiers,
        press: Option<Pos2>,
        press_doc: Option<Point>,
        pointer_doc: Option<Point>,
        settings: &Settings,
        status: &mut String,
    ) {
        let tol = PICK_PX / f64::from(self.view.zoom);
        if resp.drag_started_by(PointerButton::Primary)
            && let (Some(ps), Some(pd)) = (press, press_doc)
        {
            let sel_bounds = self.bounds_of(&editor.selection);
            let handle = sel_bounds.and_then(|b| self.handle_at(origin, b, ps).map(|h| (b, h)));
            self.preview = Affine::IDENTITY;
            self.drag = match handle {
                Some((b, Handle::Scale(i))) => {
                    let (fx, fy) = HANDLES[i];
                    Drag::Scale {
                        anchor: Point::new(b.x0 + b.width() * (1.0 - fx), b.y0 + b.height() * (1.0 - fy)),
                        start: Point::new(b.x0 + b.width() * fx, b.y0 + b.height() * fy),
                        sx_on: fx != 0.5,
                        sy_on: fy != 0.5,
                    }
                }
                Some((b, Handle::Rotate)) => {
                    let c = b.center();
                    Drag::Rotate { center: c, start_angle: (pd.y - c.y).atan2(pd.x - c.x) }
                }
                None => match self.pick(editor, pd, tol) {
                    Some(id) => {
                        if !editor.selection.contains(&id) {
                            if mods.shift {
                                editor.toggle_select(id);
                            } else {
                                editor.select_only(id);
                            }
                        }
                        Drag::Move { start: pd }
                    }
                    None => {
                        self.current = pd;
                        Drag::Marquee { start: pd }
                    }
                },
            };
        }

        if resp.dragged_by(PointerButton::Primary)
            && let Some(cur) = pointer_doc
        {
            match self.drag {
                Drag::Move { start } => {
                    let mut d = cur - start;
                    if mods.shift {
                        if d.x.abs() > d.y.abs() {
                            d.y = 0.0;
                        } else {
                            d.x = 0.0;
                        }
                    }
                    if settings.snap_to_grid && settings.grid_spacing_mm > 0.0 {
                        let g = settings.grid_spacing_mm;
                        d = Vec2::new((d.x / g).round() * g, (d.y / g).round() * g);
                    }
                    self.preview = Affine::translate(d);
                }
                Drag::Scale { anchor, start, sx_on, sy_on } => {
                    let ratio = |c: f64, s: f64, a: f64| if (s - a).abs() < 1e-9 { 1.0 } else { (c - a) / (s - a) };
                    let mut sx = if sx_on { ratio(cur.x, start.x, anchor.x) } else { 1.0 };
                    let mut sy = if sy_on { ratio(cur.y, start.y, anchor.y) } else { 1.0 };
                    if mods.shift && sx_on && sy_on {
                        let s = sx.abs().max(sy.abs());
                        sx = s.copysign(sx);
                        sy = s.copysign(sy);
                    }
                    let clamp = |v: f64| if v.abs() < 1e-4 { 1e-4f64.copysign(v) } else { v };
                    self.preview = Affine::translate(anchor.to_vec2())
                        * Affine::scale_non_uniform(clamp(sx), clamp(sy))
                        * Affine::translate(-anchor.to_vec2());
                }
                Drag::Rotate { center, start_angle } => {
                    let mut a = (cur.y - center.y).atan2(cur.x - center.x) - start_angle;
                    if mods.shift {
                        let step = 15f64.to_radians();
                        a = (a / step).round() * step;
                    }
                    self.preview = Affine::translate(center.to_vec2()) * Affine::rotate(a) * Affine::translate(-center.to_vec2());
                }
                Drag::Marquee { .. } => self.current = cur,
                _ => {}
            }
        }

        if resp.drag_stopped() {
            let label = match self.drag {
                Drag::Move { .. } => Some("Move"),
                Drag::Scale { .. } => Some("Scale"),
                Drag::Rotate { .. } => Some("Rotate"),
                Drag::Marquee { start } => {
                    let r = Rect::from_points(start, self.current);
                    editor.select_in_rect(r, mods.shift);
                    None
                }
                _ => None,
            };
            if let Some(label) = label
                && self.preview != Affine::IDENTITY
                && let Err(e) = editor.transform_selected(label, self.preview)
            {
                *status = e.to_string();
            }
            self.preview = Affine::IDENTITY;
            self.drag = Drag::None;
        }

        if resp.clicked()
            && let Some(p) = pointer_doc
        {
            match (self.pick(editor, p, tol), mods.shift) {
                (Some(id), true) => editor.toggle_select(id),
                (Some(id), false) => editor.select_only(id),
                (None, false) => editor.selection.clear(),
                (None, true) => {}
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn shape_tool(
        &mut self,
        tool: Tool,
        resp: &egui::Response,
        editor: &mut Editor,
        mods: egui::Modifiers,
        press_doc: Option<Point>,
        pointer_doc: Option<Point>,
        settings: &Settings,
        layer: LayerId,
    ) {
        if resp.drag_started_by(PointerButton::Primary)
            && let Some(p) = press_doc
        {
            let p = self.snap(p, settings);
            self.drag = Drag::Create { start: p };
            self.current = p;
        }
        if resp.dragged_by(PointerButton::Primary)
            && let (Drag::Create { start }, Some(cur)) = (self.drag, pointer_doc)
        {
            let mut c = self.snap(cur, settings);
            if mods.shift {
                let d = c - start;
                c = if tool == Tool::Line {
                    let step = std::f64::consts::FRAC_PI_4;
                    let a = (d.y.atan2(d.x) / step).round() * step;
                    start + Vec2::new(a.cos(), a.sin()) * d.hypot()
                } else {
                    let s = d.x.abs().max(d.y.abs());
                    start + Vec2::new(s.copysign(d.x), s.copysign(d.y))
                };
            }
            self.current = c;
        }
        if resp.drag_stopped()
            && let Drag::Create { start } = self.drag
        {
            self.drag = Drag::None;
            if let Some((name, path)) = self.creation_path(tool, start, self.current) {
                editor.add(Object::path(name, layer, path));
            }
        }
    }

    fn creation_path(&self, tool: Tool, a: Point, b: Point) -> Option<(&'static str, BezPath)> {
        let r = Rect::from_points(a, b);
        match tool {
            Tool::Rectangle if r.width() > 0.05 && r.height() > 0.05 => Some(("Rectangle", r.to_path(0.01))),
            Tool::Ellipse if r.width() > 0.05 && r.height() > 0.05 => {
                Some(("Ellipse", ss_geom::kurbo::Ellipse::from_rect(r).to_path(0.001)))
            }
            Tool::Line if a.distance(b) > 0.05 => {
                let mut p = BezPath::new();
                p.move_to(a);
                p.line_to(b);
                Some(("Line", p))
            }
            _ => None,
        }
    }

    fn polyline_tool(
        &mut self,
        resp: &egui::Response,
        editor: &mut Editor,
        origin: Pos2,
        pointer_doc: Option<Point>,
        settings: &Settings,
        layer: LayerId,
    ) {
        if let Some(p) = resp.hover_pos() {
            self.current = self.snap(self.view.to_doc(origin, p), settings);
        }
        if resp.double_clicked() {
            self.finish_polyline(editor, layer, false);
        } else if resp.clicked()
            && let Some(p) = pointer_doc
        {
            let p = self.snap(p, settings);
            let near_first = self.points.first().is_some_and(|f| {
                self.view.to_screen(origin, *f).distance(self.view.to_screen(origin, p)) <= 8.0
            });
            if self.points.len() >= 3 && near_first {
                self.finish_polyline(editor, layer, true);
            } else if self.points.last().is_none_or(|l| l.distance(p) > 1e-6) {
                self.points.push(p);
            }
        }
    }

    fn finish_polyline(&mut self, editor: &mut Editor, layer: LayerId, closed: bool) {
        let pts = std::mem::take(&mut self.points);
        if pts.len() < 2 {
            return;
        }
        let mut p = BezPath::new();
        p.move_to(pts[0]);
        for q in &pts[1..] {
            p.line_to(*q);
        }
        if closed {
            p.close_path();
        }
        editor.add(Object::path("Polyline", layer, p));
    }

    fn freehand_tool(
        &mut self,
        resp: &egui::Response,
        editor: &mut Editor,
        origin: Pos2,
        press_doc: Option<Point>,
        pointer_doc: Option<Point>,
        layer: LayerId,
    ) {
        if resp.drag_started_by(PointerButton::Primary)
            && let Some(p) = press_doc
        {
            self.points = vec![p];
            self.drag = Drag::Freehand;
        }
        if resp.dragged_by(PointerButton::Primary)
            && matches!(self.drag, Drag::Freehand)
            && let Some(p) = pointer_doc
            && self.points.last().is_some_and(|l| l.distance(p) * f64::from(self.view.zoom) >= 1.5)
        {
            self.points.push(p);
        }
        if resp.drag_stopped() && matches!(self.drag, Drag::Freehand) {
            self.drag = Drag::None;
            let mut pts = std::mem::take(&mut self.points);
            if pts.len() < 2 {
                return;
            }
            let (f, l) = (pts[0], pts[pts.len() - 1]);
            let closed = pts.len() > 3 && self.view.to_screen(origin, f).distance(self.view.to_screen(origin, l)) <= 10.0;
            if closed {
                pts.pop();
            }
            let opts = FitOptions { max_error: (1.0 / f64::from(self.view.zoom)).clamp(0.05, 2.0), ..FitOptions::default() };
            editor.add(Object::path("Freehand", layer, fit_contour(&pts, closed, &opts)));
        }
    }

    fn screen_points(&self, origin: Pos2, pts: &[Point], m: Affine) -> Vec<Pos2> {
        let mut out: Vec<Pos2> = Vec::with_capacity(pts.len());
        for &p in pts {
            let s = self.view.to_screen(origin, m * p);
            if out.last().is_none_or(|l| l.distance_sq(s) >= 0.5) {
                out.push(s);
            }
        }
        out
    }

    /// Nodes of the edited path: dark grey squares, the selected one white with its blue handles
    /// and its outgoing segment in red.
    fn paint_nodes(&self, painter: &egui::Painter, origin: Pos2, nodes: &NodePath, m: Affine, selected: Option<NodeId>) {
        let v = self.view;
        let screen = |p: Point| v.to_screen(origin, m * p);
        let sel_stroke = Stroke::new(1.0, SELECTION_COLOR);
        for (si, sub) in nodes.subs.iter().enumerate() {
            if let Some(NodeId { sub: s, index }) = selected
                && s == si
                && let Some(a) = sub.nodes.get(index)
            {
                let next = if index + 1 < sub.nodes.len() { Some(index + 1) } else if sub.closed { Some(0) } else { None };
                if let Some(b) = next.and_then(|j| sub.nodes.get(j)) {
                    let mut seg = BezPath::new();
                    seg.move_to(a.pos);
                    if a.hout.is_none() && b.hin.is_none() {
                        seg.line_to(b.pos);
                    } else {
                        seg.curve_to(a.hout.unwrap_or(a.pos), b.hin.unwrap_or(b.pos), b.pos);
                    }
                    for l in flatten(&seg, RENDER_TOLERANCE) {
                        let pts: Vec<Pos2> = l.points.iter().map(|p| screen(*p)).collect();
                        painter.add(Shape::line(pts, Stroke::new(2.5, Color32::from_rgb(0xe0, 0x20, 0x20))));
                    }
                }
            }
            for (ni, n) in sub.nodes.iter().enumerate() {
                let is_sel = selected == Some(NodeId { sub: si, index: ni });
                let c = screen(n.pos);
                if is_sel {
                    for h in [n.hin, n.hout].into_iter().flatten() {
                        let hp = screen(h);
                        painter.line_segment([c, hp], Stroke::new(1.0, Color32::from_rgb(0x30, 0x60, 0xff)));
                        painter.circle_filled(hp, 3.5, Color32::from_rgb(0x30, 0x60, 0xff));
                        painter.circle_stroke(hp, 3.5, Stroke::new(1.0, Color32::WHITE));
                    }
                }
                let node = egui::Rect::from_center_size(c, vec2(7.0, 7.0));
                painter.rect_filled(node, 0.0, if is_sel { Color32::WHITE } else { Color32::from_gray(70) });
                painter.rect_stroke(node, 0.0, sel_stroke, StrokeKind::Inside);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        origin: Pos2,
        editor: &Editor,
        settings: &Settings,
        tool: Tool,
        gesture: Option<&ToolGesture>,
    ) {
        let v = self.view;
        let dark = painter.ctx().global_style().visuals.dark_mode;
        painter.rect_filled(rect, 0.0, if dark { Color32::from_gray(38) } else { Color32::from_gray(200) });

        let page = &editor.doc.page;
        if let Some((w, h)) = self.mat.size_mm() {
            let mat = egui::Rect::from_min_max(
                v.to_screen(origin, Point::new(-8.0, -20.0)),
                v.to_screen(origin, Point::new(w + 8.0, h + 8.0)),
            );
            painter.rect_filled(mat, 3.0, Color32::from_gray(145));
            painter.rect_stroke(mat, 3.0, Stroke::new(1.0, Color32::from_gray(100)), StrokeKind::Inside);
        }
        let pr = egui::Rect::from_min_max(v.to_screen(origin, Point::ZERO), v.to_screen(origin, Point::new(page.width_mm, page.height_mm)));
        painter.rect_filled(pr.translate(vec2(3.0, 3.0)), 0.0, Color32::from_black_alpha(70));
        painter.rect_filled(pr, 0.0, Color32::WHITE);
        if let Some(bg) = &editor.doc.background
            && let Some(s) = self.image_shape(origin, bg, Affine::IDENTITY)
        {
            painter.add(s);
        }
        if settings.show_grid && settings.grid_spacing_mm > 0.0 {
            let g = settings.grid_spacing_mm;
            if g as f32 * v.zoom >= 8.0 {
                let minor = Stroke::new(1.0, Color32::from_gray(232));
                let major = Stroke::new(1.0, Color32::from_gray(210));
                let nx = (page.width_mm / g).floor() as i64;
                let ny = (page.height_mm / g).floor() as i64;
                for k in 1..=nx {
                    let x = k as f64 * g;
                    let s = if k % 5 == 0 { major } else { minor };
                    painter.line_segment([v.to_screen(origin, Point::new(x, 0.0)), v.to_screen(origin, Point::new(x, page.height_mm))], s);
                }
                for k in 1..=ny {
                    let y = k as f64 * g;
                    let s = if k % 5 == 0 { major } else { minor };
                    painter.line_segment([v.to_screen(origin, Point::new(0.0, y)), v.to_screen(origin, Point::new(page.width_mm, y))], s);
                }
            }
        }
        if page.show_cut_border && page.margin_mm > 0.0 {
            let m = page.margin_mm;
            let corners = [(m, m), (page.width_mm - m, m), (page.width_mm - m, page.height_mm - m), (m, page.height_mm - m), (m, m)]
                .map(|(x, y)| v.to_screen(origin, Point::new(x, y)));
            painter.extend(Shape::dashed_line(&corners, Stroke::new(1.0, Color32::from_rgb(230, 80, 80)), 6.0, 4.0));
        }
        if let Some(marks) = &page.registration {
            for r in regmark_rects(marks, page) {
                let sr = egui::Rect::from_two_pos(v.to_screen(origin, Point::new(r.x0, r.y0)), v.to_screen(origin, Point::new(r.x1, r.y1)));
                // Keep hairline marks visible when zoomed out.
                painter.rect_filled(sr.expand2(vec2((1.0 - sr.width()).max(0.0) / 2.0, (1.0 - sr.height()).max(0.0) / 2.0)), 0.0, Color32::BLACK);
            }
        }
        painter.rect_stroke(pr, 0.0, Stroke::new(1.0, Color32::from_gray(120)), StrokeKind::Outside);

        let visible = Rect::from_points(v.to_doc(origin, rect.min), v.to_doc(origin, rect.max));
        let selected: HashSet<ObjectId> = editor.selection.iter().copied().collect();
        let mut shapes = Vec::new();
        for o in editor.doc.visible_objects() {
            let Some(c) = self.cache.get(&o.id) else { continue };
            let sel = selected.contains(&o.id);
            let m = if sel { self.preview } else { Affine::IDENTITY };
            let Some(bb) = c.bbox.map(|b| if sel { transform_rect(m, b) } else { b }) else { continue };
            if !overlaps(bb, visible) {
                continue;
            }
            for (img, im) in o.page_images() {
                shapes.extend(self.image_shape(origin, img, m * im));
            }
            if !c.fill_indices.is_empty() {
                if let Some(pat) = &o.style.fill_pattern
                    && let Some(Some(tex)) = self.textures.get(&bytes_key(&pat.encoded, pat.width_px, pat.height_px))
                {
                    let mut mesh = egui::Mesh::with_texture(tex.id());
                    // Page point -> texture coordinates (1.0 = one tile).
                    let to_uv = match pat.transform {
                        Some(t) => {
                            let (w, h) = (f64::from(pat.width_px.max(1)), f64::from(pat.height_px.max(1)));
                            Affine::scale_non_uniform(1.0 / w, 1.0 / h) * (o.transform * Affine::new(t)).inverse()
                        }
                        None => Affine::scale_non_uniform(1.0 / pat.tile_width_mm.max(1.0), 1.0 / pat.tile_height_mm.max(1.0)),
                    };
                    mesh.vertices.extend(c.fill_verts.iter().map(|&pt| {
                        let uv = to_uv * pt;
                        egui::epaint::Vertex { pos: v.to_screen(origin, m * pt), uv: pos2(uv.x as f32, uv.y as f32), color: Color32::WHITE }
                    }));
                    mesh.indices.extend_from_slice(&c.fill_indices);
                    shapes.push(Shape::mesh(mesh));
                } else if let Some(grad) = &o.style.fill_gradient
                    && !grad.stops.is_empty()
                    && let Some(raw_b) = c.bbox
                {
                    let mut mesh = egui::Mesh::default();
                    let center = raw_b.center();
                    let rad = grad.angle_deg.to_radians();
                    let dir = Vec2::new(rad.cos(), rad.sin());
                    let half_span = (raw_b.width() * dir.x.abs() + raw_b.height() * dir.y.abs()) * 0.5;
                    let radius = (raw_b.width().hypot(raw_b.height()) * 0.5).max(1e-6);
                    mesh.vertices.extend(c.fill_verts.iter().map(|&pt| {
                        let t = if grad.radial {
                            (pt.distance(center) / radius).clamp(0.0, 1.0)
                        } else if half_span > 1e-6 {
                            (((pt - center).dot(dir) / (2.0 * half_span)) + 0.5).clamp(0.0, 1.0)
                        } else {
                            0.5
                        };
                        egui::epaint::Vertex {
                            pos: v.to_screen(origin, m * pt),
                            uv: egui::epaint::WHITE_UV,
                            color: color32(grad.sample(t)),
                        }
                    }));
                    mesh.indices.extend_from_slice(&c.fill_indices);
                    shapes.push(Shape::mesh(mesh));
                } else if let Some(fill) = o.style.fill
                    && fill.a > 0
                {
                    let mut mesh = egui::Mesh::default();
                    let fc = color32(fill);
                    mesh.vertices.extend(c.fill_verts.iter().map(|&pt| egui::epaint::Vertex {
                        pos: v.to_screen(origin, m * pt),
                        uv: egui::epaint::WHITE_UV,
                        color: fc,
                    }));
                    mesh.indices.extend_from_slice(&c.fill_indices);
                    shapes.push(Shape::mesh(mesh));
                }
            }
            if sel || o.style.stroke.a > 0 {
                let color = if o.style.stroke.a == 0 {
                    SELECTION_COLOR
                } else if o.cut == CutAction::NoCut {
                    Color32::from_gray(150)
                } else {
                    color32(o.style.stroke)
                };
                let width = if sel {
                    1.8
                } else if o.style.stroke_width_mm > 0.0 {
                    (o.style.stroke_width_mm as f32 * v.zoom).max(1.0)
                } else {
                    1.0
                };
                let stroke = Stroke::new(width, color);
                let dash_pair: Option<(f32, f32)> = if !o.style.dash_mm.is_empty() {
                    let on = (o.style.dash_mm[0] as f32 * v.zoom).clamp(2.0, 40.0);
                    let off = (o.style.dash_mm.get(1).copied().unwrap_or(o.style.dash_mm[0]) as f32 * v.zoom).clamp(2.0, 40.0);
                    Some((on, off))
                } else if o.cut == CutAction::Perforate {
                    Some(((2.0 * v.zoom).clamp(2.0, 40.0), (1.0 * v.zoom).clamp(2.0, 40.0)))
                } else {
                    None
                };
                for l in &c.lines {
                    let mut pts = self.screen_points(origin, &l.points, m);
                    if pts.len() < 2 {
                        continue;
                    }
                    if let Some((on, off)) = dash_pair {
                        if l.closed {
                            pts.push(pts[0]);
                        }
                        shapes.extend(Shape::dashed_line(&pts, stroke, on, off));
                    } else {
                        shapes.push(if l.closed { Shape::closed_line(pts, stroke) } else { Shape::line(pts, stroke) });
                    }
                }
            }
        }
        for s in &self.overlay {
            let pts = self.screen_points(origin, s, Affine::IDENTITY);
            if pts.len() >= 2 {
                shapes.push(Shape::line(pts, Stroke::new(1.5, OVERLAY_COLOR)));
            }
        }
        painter.extend(shapes);

        let sel_stroke = Stroke::new(1.0, SELECTION_COLOR);
        if tool == Tool::EditPoints
            && let Some((id, m)) = Self::sole_path(editor)
            && let Some(nodes) = editor.path_nodes(id)
        {
            self.paint_nodes(painter, origin, &nodes, m, self.valid_node(editor).map(|(_, n)| n));
        }
        if tool == Tool::Select
            && let Some(b) = self.bounds_of(&editor.selection).map(|b| transform_rect(self.preview, b))
        {
            let corners = [(b.x0, b.y0), (b.x1, b.y0), (b.x1, b.y1), (b.x0, b.y1), (b.x0, b.y0)]
                .map(|(x, y)| v.to_screen(origin, Point::new(x, y)));
            painter.extend(Shape::dashed_line(&corners, sel_stroke, 5.0, 3.0));
            let top = v.to_screen(origin, Point::new(b.center().x, b.y0));
            let rot = top - vec2(0.0, ROTATE_OFFSET);
            painter.line_segment([top, rot], sel_stroke);
            painter.circle_filled(rot, HANDLE_HALF + 1.0, Color32::WHITE);
            painter.circle_stroke(rot, HANDLE_HALF + 1.0, sel_stroke);
            for (fx, fy) in HANDLES {
                let c = v.to_screen(origin, Point::new(b.x0 + b.width() * fx, b.y0 + b.height() * fy));
                let r = egui::Rect::from_center_size(c, vec2(HANDLE_HALF * 2.0, HANDLE_HALF * 2.0));
                painter.rect_filled(r, 0.0, Color32::WHITE);
                painter.rect_stroke(r, 0.0, sel_stroke, StrokeKind::Inside);
            }
        }
        match self.drag {
            Drag::Marquee { start } => {
                let r = egui::Rect::from_two_pos(v.to_screen(origin, start), v.to_screen(origin, self.current));
                painter.rect_filled(r, 0.0, SELECTION_COLOR.gamma_multiply(0.12));
                painter.rect_stroke(r, 0.0, sel_stroke, StrokeKind::Inside);
            }
            Drag::Create { start } => {
                if let Some((_, path)) = self.creation_path(tool, start, self.current) {
                    for l in flatten(&path, RENDER_TOLERANCE) {
                        let pts = self.screen_points(origin, &l.points, Affine::IDENTITY);
                        painter.add(if l.closed { Shape::closed_line(pts, sel_stroke) } else { Shape::line(pts, sel_stroke) });
                    }
                }
            }
            Drag::Freehand => {
                painter.add(Shape::line(self.screen_points(origin, &self.points, Affine::IDENTITY), sel_stroke));
            }
            _ => {}
        }
        for line in &self.trace_preview {
            let pts = self.screen_points(origin, line, Affine::IDENTITY);
            if pts.len() >= 3 {
                painter.add(Shape::closed_line(pts, Stroke::new(1.5, Color32::from_rgb(0xd0, 0x30, 0xc0))));
            }
        }
        if let Some(gesture) = gesture {
            let mods = painter.ctx().input(|i| i.modifiers);
            match tool {
                Tool::Knife => {
                    let cut = knife_cut_points(gesture, mods);
                    let pts = self.screen_points(origin, &cut, Affine::IDENTITY);
                    if cut.len() >= 2 && pts.len() >= 2 {
                        painter.add(Shape::line(pts, Stroke::new(2.0, Color32::RED)));
                    }
                }
                Tool::Eraser => {
                    let radius = self.eraser_radius_mm as f32 * v.zoom;
                    let ink = Color32::from_rgba_unmultiplied(230, 80, 80, 100);
                    let pts = self.screen_points(origin, &gesture.points, Affine::IDENTITY);
                    if pts.len() >= 2 {
                        painter.add(Shape::line(pts, Stroke::new(radius * 2.0, ink)));
                    }
                    let c = v.to_screen(origin, gesture.current);
                    match self.eraser_shape {
                        BrushShape::Round => {
                            painter.circle_filled(c, radius, ink);
                            painter.circle_stroke(c, radius, Stroke::new(1.0, Color32::RED));
                        }
                        BrushShape::Square => {
                            let r = egui::Rect::from_center_size(c, vec2(radius * 2.0, radius * 2.0));
                            painter.rect_filled(r, 0.0, ink);
                            painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::RED), StrokeKind::Inside);
                        }
                    }
                }
                _ => {}
            }
        }
        if tool == Tool::Polyline && !self.points.is_empty() {
            let mut pts = self.screen_points(origin, &self.points, Affine::IDENTITY);
            pts.push(v.to_screen(origin, self.current));
            painter.add(Shape::line(pts, sel_stroke));
            painter.circle_stroke(v.to_screen(origin, self.points[0]), 6.0, sel_stroke);
        }
    }
}

fn context_menu(ui: &mut egui::Ui, editor: &Editor, actions: &mut Vec<Action>) {
    let any = !editor.selection.is_empty();
    let many = editor.selection.len() >= 2;
    let items: [(&str, Action, bool); 10] = [
        ("Cut", Action::Cut, any),
        ("Copy", Action::Copy, any),
        ("Paste", Action::Paste, true),
        ("Duplicate", Action::Duplicate, any),
        ("Delete", Action::Delete, any),
        ("Group", Action::Group, many),
        ("Ungroup", Action::Ungroup, any),
        ("Weld", Action::Weld, many),
        ("Bring to front", Action::Reorder { front: true, all: true }, any),
        ("Send to back", Action::Reorder { front: false, all: true }, any),
    ];
    for (label, action, enabled) in items {
        if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
            actions.push(action);
            ui.close();
        }
    }
}
