//! Drawing canvas: mat and page, objects, selection handles and the drawing tools.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use egui::{Color32, Key, PointerButton, Pos2, Sense, Shape, Stroke, StrokeKind, pos2, vec2};
use ss_config::Settings;
use ss_doc::{CutAction, Editor, LayerId, Object, ObjectId, ObjectKind};
use ss_geom::fit::{FitOptions, fit_contour};
use ss_geom::flatten::{Polyline, flatten};
use ss_geom::{Affine, BezPath, Point, Rect, Shape as _, Vec2};

use crate::app::Action;

const RENDER_TOLERANCE: f64 = 0.02;
const HANDLE_HALF: f32 = 4.0;
const ROTATE_OFFSET: f32 = 22.0;
const PICK_PX: f64 = 5.0;
const MIN_ZOOM: f32 = 0.02;
const MAX_ZOOM: f32 = 400.0;
/// Points per mm at 100 % (96 dpi).
pub const ACTUAL_SIZE_ZOOM: f32 = 96.0 / 25.4;
pub const SELECTION_COLOR: Color32 = Color32::from_rgb(0x1e, 0x88, 0xe5);
const OVERLAY_COLOR: Color32 = Color32::from_rgb(0x00, 0xa0, 0x50);
/// Scale handles as fractions of the selection box, clockwise from the top-left corner.
const HANDLES: [(f64, f64); 8] = [(0.0, 0.0), (0.5, 0.0), (1.0, 0.0), (1.0, 0.5), (1.0, 1.0), (0.5, 1.0), (0.0, 1.0), (0.0, 0.5)];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Select,
    Rectangle,
    Ellipse,
    Line,
    Polyline,
    Freehand,
}

impl Tool {
    pub const ALL: [Tool; 6] = [Tool::Select, Tool::Rectangle, Tool::Ellipse, Tool::Line, Tool::Polyline, Tool::Freehand];

    pub fn label(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Rectangle => "Rectangle",
            Tool::Ellipse => "Ellipse",
            Tool::Line => "Line",
            Tool::Polyline => "Polyline",
            Tool::Freehand => "Freehand",
        }
    }

    pub fn key(self) -> Key {
        match self {
            Tool::Select => Key::V,
            Tool::Rectangle => Key::R,
            Tool::Ellipse => Key::E,
            Tool::Line => Key::L,
            Tool::Polyline => Key::P,
            Tool::Freehand => Key::F,
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Tool::Select => "Click to select, Shift-click to add, drag to move, drag handles to scale or rotate",
            Tool::Rectangle => "Drag to draw a rectangle; Shift for a square",
            Tool::Ellipse => "Drag to draw an ellipse; Shift for a circle",
            Tool::Line => "Drag to draw a line; Shift snaps to 45°",
            Tool::Polyline => "Click to add points; click the first point to close, double-click or Enter to finish",
            Tool::Freehand => "Drag to draw; end near the start to close the shape",
        }
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

pub struct Canvas {
    pub view: View,
    pub tool: Tool,
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
}

impl Default for Canvas {
    fn default() -> Self {
        Self {
            view: View::default(),
            tool: Tool::Select,
            pending_fit: None,
            last_rect: egui::Rect::NOTHING,
            drag: Drag::None,
            preview: Affine::IDENTITY,
            current: Point::ZERO,
            points: Vec::new(),
            cache: HashMap::new(),
            cursor_mm: None,
            overlay: Vec::new(),
        }
    }
}

fn content_key(o: &Object, h: &mut DefaultHasher) {
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
            self.cache.insert(o.id, Cached { key, lines, bbox });
        }
        if self.cache.len() != seen.len() {
            self.cache.retain(|id, _| seen.contains(id));
        }
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
        settings: &Settings,
        layer: LayerId,
        actions: &mut Vec<Action>,
        status: &mut String,
    ) {
        let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let rect = resp.rect;
        self.last_rect = rect;
        let origin = rect.min;
        if let Some(r) = self.pending_fit.take() {
            self.view.fit(rect, r);
        }
        self.refresh_cache(editor);

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
                self.cancel_interaction();
            }
            if enter && self.tool == Tool::Polyline {
                self.finish_polyline(editor, layer, false);
            }
        }

        if !panning && !(space && !typing) {
            let pointer_doc = resp.interact_pointer_pos().map(|p| self.view.to_doc(origin, p));
            let press_doc = press.map(|p| self.view.to_doc(origin, p));
            match self.tool {
                Tool::Select => self.select_tool(&resp, editor, origin, mods, press, press_doc, pointer_doc, settings, status),
                Tool::Rectangle | Tool::Ellipse | Tool::Line => {
                    self.shape_tool(&resp, editor, mods, press_doc, pointer_doc, settings, layer);
                }
                Tool::Polyline => self.polyline_tool(&resp, editor, origin, pointer_doc, settings, layer),
                Tool::Freehand => self.freehand_tool(&resp, editor, origin, press_doc, pointer_doc, layer),
            }
        }

        self.paint(&painter, rect, origin, editor, settings);
        resp.context_menu(|ui| context_menu(ui, editor, actions));
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
                c = if self.tool == Tool::Line {
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
            if let Some((name, path)) = self.creation_path(start, self.current) {
                editor.add(Object::path(name, layer, path));
            }
        }
    }

    fn creation_path(&self, a: Point, b: Point) -> Option<(&'static str, BezPath)> {
        let r = Rect::from_points(a, b);
        match self.tool {
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

    fn paint(&self, painter: &egui::Painter, rect: egui::Rect, origin: Pos2, editor: &Editor, settings: &Settings) {
        let v = self.view;
        let dark = painter.ctx().global_style().visuals.dark_mode;
        painter.rect_filled(rect, 0.0, if dark { Color32::from_gray(38) } else { Color32::from_gray(200) });

        let page = &editor.doc.page;
        let pr = egui::Rect::from_min_max(v.to_screen(origin, Point::ZERO), v.to_screen(origin, Point::new(page.width_mm, page.height_mm)));
        painter.rect_filled(pr.translate(vec2(3.0, 3.0)), 0.0, Color32::from_black_alpha(70));
        painter.rect_filled(pr, 0.0, Color32::WHITE);
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
            let color = if o.cut == CutAction::NoCut { Color32::from_gray(150) } else { color32(o.style.stroke) };
            let stroke = Stroke::new(if sel { 1.8 } else { 1.0 }, color);
            for l in &c.lines {
                let pts = self.screen_points(origin, &l.points, m);
                if pts.len() < 2 {
                    continue;
                }
                shapes.push(if l.closed { Shape::closed_line(pts, stroke) } else { Shape::line(pts, stroke) });
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
        if self.tool == Tool::Select
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
                if let Some((_, path)) = self.creation_path(start, self.current) {
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
        if self.tool == Tool::Polyline && !self.points.is_empty() {
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
