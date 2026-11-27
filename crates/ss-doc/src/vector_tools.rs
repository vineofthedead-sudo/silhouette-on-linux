//! Editor commands for the vector tools: Knife, Eraser, Auto-Trace placement and point editing.
//! Each command is one undo step; live drags edit the document directly and are committed with
//! [`Editor::commit_edit`] when the gesture ends.

use std::collections::HashSet;

use ss_geom::nodes::{NodeId, NodePath};
use ss_geom::tools::{self, BrushShape, ToolGeometryError};
use ss_geom::{Affine, BezPath, FillRule, Point, Rect, Shape, ops};
use uuid::Uuid;

use crate::editor::{EditError, Editor};
use crate::model::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplicateDir {
    Left,
    Right,
    Above,
    Below,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ReplicateOp {
    Step { dir: ReplicateDir, gap_mm: f64 },
    Mirror { dir: ReplicateDir, gap_mm: f64 },
    Grid { rows: usize, cols: usize, gap_x_mm: f64, gap_y_mm: f64 },
    FillPage { gap_x_mm: f64, gap_y_mm: f64 },
}

fn refresh_ids(obj: &mut Object) {
    obj.id = Uuid::new_v4();
    if let ObjectKind::Group { children } = &mut obj.kind {
        for c in children {
            refresh_ids(c);
        }
    }
}

fn clone_transformed(src: &Object, m: Affine) -> Object {
    let mut copy = src.clone();
    refresh_ids(&mut copy);
    copy.transform = m * copy.transform;
    copy
}

type PathOp<'a> = dyn FnMut(&BezPath, FillRule) -> Result<Option<Vec<BezPath>>, ToolGeometryError> + 'a;

struct Pass<'a> {
    skip_layers: HashSet<LayerId>,
    region: Rect,
    op: &'a mut PathOp<'a>,
    selected: Vec<ObjectId>,
    changed: usize,
    error: Option<ToolGeometryError>,
}

fn invalid(e: ToolGeometryError) -> EditError {
    match e {
        ToolGeometryError::InvalidStroke(m) => EditError::Invalid(m),
    }
}

fn touches(a: Rect, b: Rect) -> bool {
    a.x0 <= b.x1 && b.x0 <= a.x1 && a.y0 <= b.y1 && b.y0 <= a.y1
}

/// Runs the pass over `objs`, replacing every changed path or text object by its pieces. Pieces
/// are stored in the coordinate space of their parent group, so they stay where they were.
fn rewrite(objs: Vec<Object>, parent: Affine, pass: &mut Pass, top: bool) -> Vec<Object> {
    let inverse = if parent.determinant().abs() > 1e-12 { parent.inverse() } else { Affine::IDENTITY };
    let mut out = Vec::with_capacity(objs.len());
    for mut obj in objs {
        if !obj.visible || obj.locked || pass.skip_layers.contains(&obj.layer) {
            out.push(obj);
            continue;
        }
        let (page, rule) = match &mut obj.kind {
            ObjectKind::Path { path, fill_rule } => (ops::transformed(path, parent * obj.transform), *fill_rule),
            ObjectKind::Text { outlines, .. } => (ops::transformed(outlines, parent * obj.transform), FillRule::NonZero),
            ObjectKind::Group { children } => {
                let kids = rewrite(std::mem::take(children), parent * obj.transform, pass, false);
                if !kids.is_empty() {
                    *children = kids;
                    out.push(obj);
                }
                continue;
            }
            ObjectKind::Image(_) => {
                out.push(obj);
                continue;
            }
        };
        if page.elements().is_empty() || !touches(page.bounding_box(), pass.region) {
            out.push(obj);
            continue;
        }
        match (pass.op)(&page, rule) {
            Ok(Some(pieces)) => {
                pass.changed += 1;
                for piece in pieces {
                    let mut o = obj.clone();
                    o.id = Uuid::new_v4();
                    o.transform = Affine::IDENTITY;
                    o.kind = ObjectKind::Path { path: SharedPath::new(ops::transformed(&piece, inverse)), fill_rule: FillRule::EvenOdd };
                    if top {
                        pass.selected.push(o.id);
                    }
                    out.push(o);
                }
            }
            Ok(None) => out.push(obj),
            Err(e) => {
                pass.error.get_or_insert(e);
                out.push(obj);
            }
        }
    }
    out
}

fn points_bounds(points: &[Point], margin: f64) -> Rect {
    let mut r = Rect::new(f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in points {
        r = Rect::new(r.x0.min(p.x), r.y0.min(p.y), r.x1.max(p.x), r.y1.max(p.y));
    }
    r.inflate(margin, margin)
}

impl Editor {
    fn rewrite_unlocked(&mut self, label: &str, region: Rect, op: &mut PathOp) -> Result<usize, EditError> {
        let skip_layers: HashSet<LayerId> = self.doc.layers.iter().filter(|l| l.locked || !l.visible).map(|l| l.id).collect();
        self.apply(label, |d, sel| {
            let mut pass = Pass { skip_layers, region, op, selected: Vec::new(), changed: 0, error: None };
            let objs = std::mem::take(&mut d.objects);
            d.objects = rewrite(objs, Affine::IDENTITY, &mut pass, true);
            if let Some(e) = pass.error {
                return Err(invalid(e));
            }
            if pass.changed == 0 {
                return Err(EditError::NotApplicable("The cut did not separate any unlocked shape"));
            }
            if !pass.selected.is_empty() {
                *sel = pass.selected;
            }
            Ok(pass.changed)
        })
    }

    /// Slices every visible, unlocked shape along `cut` (two points for a straight cut, more for a
    /// freehand slice). The pieces become independent objects and are selected. Returns how many
    /// shapes were split.
    pub fn knife_cut(&mut self, cut: &[Point]) -> Result<usize, EditError> {
        if cut.len() < 2 {
            return Err(EditError::Invalid("Knife needs a drag with two distinct points"));
        }
        let tol = self.tolerance;
        let cut = cut.to_vec();
        let region = points_bounds(&cut, 0.1);
        self.rewrite_unlocked("Knife", region, &mut |p, f| tools::knife_split(p, f, &cut, tol))
    }

    /// Subtracts a brush swept along `stroke` from every visible, unlocked shape it touches.
    /// Returns how many shapes changed; shapes erased completely are removed.
    pub fn erase_with_brush(&mut self, stroke: &[Point], radius_mm: f64, shape: BrushShape) -> Result<usize, EditError> {
        let brush = tools::brush_polygons(stroke, radius_mm, shape).map_err(invalid)?;
        let tol = self.tolerance;
        let region = points_bounds(stroke, radius_mm * 1.5 + 0.1);
        self.rewrite_unlocked("Eraser", region, &mut |p, f| tools::erase(p, f, &brush, tol))
    }

    /// Maps pixel coordinates of a `raster.0 × raster.1` bitmap of image `source` to page mm.
    pub fn trace_transform(&self, source: ObjectId, raster: (u32, u32)) -> Option<Affine> {
        let obj = self.doc.find(source)?;
        let ObjectKind::Image(img) = &obj.kind else { return None };
        if raster.0 == 0 || raster.1 == 0 {
            return None;
        }
        let r = img.placement();
        let to_local = Affine::new([r.width() / f64::from(raster.0), 0.0, 0.0, r.height() / f64::from(raster.1), r.x0, r.y0]);
        Some(obj.transform * to_local)
    }

    /// Places traced contours (pixel coordinates of a `raster.0 × raster.1` bitmap) over the image
    /// `source` as one compound cut path on `layer`, and selects it.
    pub fn add_trace(&mut self, source: ObjectId, layer: LayerId, paths_px: &[BezPath], raster: (u32, u32), name: &str) -> Result<ObjectId, EditError> {
        let m = self.trace_transform(source, raster).ok_or(EditError::NotApplicable("Trace needs a placed image"))?;
        if paths_px.is_empty() {
            return Err(EditError::EmptyResult);
        }
        let path = ops::transformed(&ops::combine(paths_px), m);
        let label = name.to_string();
        self.apply(name, |d, sel| {
            let o = Object::path(label, layer, path);
            let id = o.id;
            d.objects.push(o);
            *sel = vec![id];
            Ok(id)
        })
    }

    /// The editable path of a single, ungrouped path object in its own coordinates.
    pub fn path_nodes(&self, id: ObjectId) -> Option<NodePath> {
        match &self.doc.find(id)?.kind {
            ObjectKind::Path { path, .. } => Some(NodePath::from_bez(path)),
            _ => None,
        }
    }

    /// Replaces a path without recording history. Pair with [`Editor::commit_edit`] using a
    /// document snapshot taken when the gesture began.
    pub fn set_path_live(&mut self, id: ObjectId, path: BezPath) -> bool {
        match self.doc.find_mut(id).map(|o| &mut o.kind) {
            Some(ObjectKind::Path { path: p, .. }) => {
                *p = SharedPath::new(path);
                true
            }
            _ => false,
        }
    }

    /// Records the end of a live edit as one undo step.
    pub fn commit_edit(&mut self, label: &str, before: Document) {
        self.history.record(label, before);
        self.dirty = true;
    }

    fn edit_path_nodes<T>(
        &mut self,
        id: ObjectId,
        label: &str,
        f: impl FnOnce(&mut NodePath) -> Result<T, EditError>,
    ) -> Result<T, EditError> {
        let mut np = self.path_nodes(id).ok_or(EditError::NotApplicable("Edit Points needs a single path"))?;
        let out = f(&mut np)?;
        let path = np.to_bez();
        self.apply(label, |d, _| {
            if let Some(ObjectKind::Path { path: p, .. }) = d.find_mut(id).map(|o| &mut o.kind) {
                *p = SharedPath::new(path);
            }
            Ok(out)
        })
    }

    pub fn delete_path_node(&mut self, id: ObjectId, node: NodeId) -> Result<(), EditError> {
        self.edit_path_nodes(id, "Delete Point", |np| np.delete_node(node).map_err(EditError::Invalid))
    }

    /// Adds a node on the segment of path `id` nearest `local_pt` (the path's own coordinates).
    pub fn insert_path_node(&mut self, id: ObjectId, local_pt: Point, tol: f64) -> Result<NodeId, EditError> {
        self.edit_path_nodes(id, "Add Point", |np| {
            np.insert_at(local_pt, tol).ok_or(EditError::NotApplicable("No line under the pointer"))
        })
    }

    /// Splits a closed or continuous subpath at `node` into open endpoints. Breaking a closed
    /// shape removes its fill attributes, matching Silhouette Studio's Edit Points behavior.
    pub fn break_path_node(&mut self, id: ObjectId, node: NodeId) -> Result<NodeId, EditError> {
        let mut np = self.path_nodes(id).ok_or(EditError::NotApplicable("Edit Points needs a single path"))?;
        let (endpoint, broke_closed) = np.break_at(node).map_err(EditError::Invalid)?;
        let path = np.to_bez();
        self.apply("Break Path", |d, _| {
            if let Some(obj) = d.find_mut(id) {
                if let ObjectKind::Path { path: p, .. } = &mut obj.kind {
                    *p = SharedPath::new(path);
                }
                if broke_closed {
                    obj.style.fill = None;
                    obj.style.fill_pattern = None;
                }
            }
            Ok(endpoint)
        })
    }

    pub fn set_node_smooth(&mut self, id: ObjectId, node: NodeId, smooth: bool) -> Result<(), EditError> {
        let label = if smooth { "Smooth Point" } else { "Corner Point" };
        self.edit_path_nodes(id, label, |np| np.set_smooth(node, smooth).map_err(EditError::Invalid))
    }

    pub fn set_segment_curved(&mut self, id: ObjectId, node: NodeId, curved: bool) -> Result<(), EditError> {
        let label = if curved { "Make Curve" } else { "Make Flat" };
        self.edit_path_nodes(id, label, |np| np.set_segment_curved(node, curved).map_err(EditError::Invalid))
    }

    /// Reduces redundant points on path `id` within `tolerance_mm`. Returns `(before, after)` node counts.
    pub fn simplify_path(&mut self, id: ObjectId, tolerance_mm: f64) -> Result<(usize, usize), EditError> {
        let (before_path, before_count) = match &self.doc.find(id).ok_or(EditError::NotApplicable("Edit Points needs a single path"))?.kind {
            ObjectKind::Path { path, .. } => (
                (**path).clone(),
                NodePath::from_bez(path).subs.iter().map(|s| s.nodes.len()).sum::<usize>(),
            ),
            _ => return Err(EditError::NotApplicable("Edit Points needs a single path")),
        };
        let simplified = ss_geom::nodes::simplify_path(&before_path, tolerance_mm);
        if simplified.elements().is_empty() {
            return Err(EditError::EmptyResult);
        }
        let after_count: usize = NodePath::from_bez(&simplified).subs.iter().map(|s| s.nodes.len()).sum();
        self.apply("Simplify Path", |d, _| {
            if let Some(ObjectKind::Path { path: p, .. }) = d.find_mut(id).map(|o| &mut o.kind) {
                *p = SharedPath::new(simplified);
            }
            Ok((before_count, after_count))
        })
    }

    /// Replicates the current selection as a single undoable command (directional duplicates,
    /// mirror copies, `rows × cols` grid arrays, or `FillPage` auto-nesting inside cut borders).
    pub fn replicate_selected(&mut self, op: ReplicateOp) -> Result<Vec<ObjectId>, EditError> {
        let b = self.selection_bounds().ok_or(EditError::EmptySelection)?;
        let (w, h) = (b.width(), b.height());
        let label = match op {
            ReplicateOp::Step { .. } => "Duplicate",
            ReplicateOp::Mirror { .. } => "Mirror Copy",
            ReplicateOp::Grid { .. } => "Replicate Grid",
            ReplicateOp::FillPage { .. } => "Fill Page",
        };
        let transforms: Vec<Affine> = match op {
            ReplicateOp::Step { dir, gap_mm } => {
                let d = match dir {
                    ReplicateDir::Right => ss_geom::Vec2::new(w + gap_mm, 0.0),
                    ReplicateDir::Left => ss_geom::Vec2::new(-(w + gap_mm), 0.0),
                    ReplicateDir::Below => ss_geom::Vec2::new(0.0, h + gap_mm),
                    ReplicateDir::Above => ss_geom::Vec2::new(0.0, -(h + gap_mm)),
                };
                vec![Affine::translate(d)]
            }
            ReplicateOp::Mirror { dir, gap_mm } => {
                let m = match dir {
                    ReplicateDir::Right => Affine::translate((2.0 * b.x1 + gap_mm, 0.0)) * Affine::scale_non_uniform(-1.0, 1.0),
                    ReplicateDir::Left => Affine::translate((2.0 * b.x0 - gap_mm, 0.0)) * Affine::scale_non_uniform(-1.0, 1.0),
                    ReplicateDir::Below => Affine::translate((0.0, 2.0 * b.y1 + gap_mm)) * Affine::scale_non_uniform(1.0, -1.0),
                    ReplicateDir::Above => Affine::translate((0.0, 2.0 * b.y0 - gap_mm)) * Affine::scale_non_uniform(1.0, -1.0),
                };
                vec![m]
            }
            ReplicateOp::Grid { rows, cols, gap_x_mm, gap_y_mm } => {
                let (rows, cols) = (rows.max(1), cols.max(1));
                let mut out = Vec::with_capacity(rows * cols);
                for r in 0..rows {
                    for c in 0..cols {
                        if r == 0 && c == 0 {
                            continue;
                        }
                        out.push(Affine::translate((c as f64 * (w + gap_x_mm), r as f64 * (h + gap_y_mm))));
                    }
                }
                if out.is_empty() {
                    return Err(EditError::Invalid("Grid must have at least 2 cells"));
                }
                out
            }
            ReplicateOp::FillPage { gap_x_mm, gap_y_mm } => {
                let page = &self.doc.page;
                let m = page.margin_mm;
                let area_w = (page.width_mm - 2.0 * m).max(0.0);
                let area_h = (page.height_mm - 2.0 * m).max(0.0);
                if w <= 1e-6 || h <= 1e-6 || area_w < w || area_h < h {
                    return Err(EditError::NotApplicable("Selection is larger than the cuttable page area"));
                }
                let step_x = (w + gap_x_mm).max(w);
                let step_y = (h + gap_y_mm).max(h);
                let cols = ((area_w + gap_x_mm) / step_x).floor().max(1.0) as usize;
                let rows = ((area_h + gap_y_mm) / step_y).floor().max(1.0) as usize;
                let base_dx = m - b.x0;
                let base_dy = m - b.y0;
                let mut out = Vec::with_capacity(rows * cols);
                for r in 0..rows {
                    for c in 0..cols {
                        out.push(Affine::translate((base_dx + c as f64 * step_x, base_dy + r as f64 * step_y)));
                    }
                }
                out
            }
        };

        self.apply(label, |d, sel| {
            let originals: Vec<Object> = d.objects.iter().filter(|o| sel.contains(&o.id)).cloned().collect();
            if originals.is_empty() {
                return Err(EditError::EmptySelection);
            }
            let mut added = Vec::new();
            if let ReplicateOp::FillPage { .. } = op {
                let first_m = transforms[0];
                for o in d.objects.iter_mut().filter(|o| sel.contains(&o.id)) {
                    o.transform = first_m * o.transform;
                }
                for &m in &transforms[1..] {
                    for src in &originals {
                        let copy = clone_transformed(src, m);
                        added.push(copy.id);
                        d.objects.push(copy);
                    }
                }
            } else {
                for m in transforms {
                    for src in &originals {
                        let copy = clone_transformed(src, m);
                        added.push(copy.id);
                        d.objects.push(copy);
                    }
                }
            }
            if !added.is_empty() {
                *sel = added.clone();
            }
            Ok(added)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ss_geom::kurbo::Rect as KRect;

    fn editor_with_square() -> (Editor, ObjectId) {
        let mut ed = Editor::new(Document::default());
        let layer = ed.doc.default_layer();
        let id = ed.add(Object::path("Square", layer, KRect::new(0.0, 0.0, 10.0, 10.0).to_path(0.0)));
        (ed, id)
    }

    #[test]
    fn knife_makes_one_undo_step_and_selects_the_pieces() {
        let (mut ed, _) = editor_with_square();
        let n = ed.knife_cut(&[Point::new(5.0, -1.0), Point::new(5.0, 11.0)]).unwrap();
        assert_eq!(n, 1);
        assert_eq!(ed.doc.objects.len(), 2);
        assert_eq!(ed.selection.len(), 2);
        assert_eq!(ed.undo().as_deref(), Some("Knife"));
        assert_eq!(ed.doc.objects.len(), 1);
        // The undo step above was the knife, not the earlier "Add".
        assert_eq!(ed.undo().as_deref(), Some("Add"));
    }

    #[test]
    fn knife_respects_locks_and_reports_a_miss() {
        let (mut ed, id) = editor_with_square();
        ed.doc.find_mut(id).unwrap().locked = true;
        let before = ed.doc.clone();
        assert!(ed.knife_cut(&[Point::new(5.0, -1.0), Point::new(5.0, 11.0)]).is_err());
        assert_eq!(ed.doc, before);
        ed.doc.find_mut(id).unwrap().locked = false;
        assert!(ed.knife_cut(&[Point::new(50.0, -1.0), Point::new(50.0, 11.0)]).is_err());
        assert_eq!(ed.doc.objects.len(), 1);
    }

    #[test]
    fn knife_cuts_inside_groups_and_keeps_placement() {
        let (mut ed, id) = editor_with_square();
        let layer = ed.doc.default_layer();
        let other = ed.add(Object::path("Far", layer, KRect::new(0.0, 50.0, 2.0, 52.0).to_path(0.0)));
        ed.selection = vec![id, other];
        let g = ed.group_selected().unwrap();
        ed.doc.find_mut(g).unwrap().transform = Affine::translate((100.0, 0.0));
        ed.knife_cut(&[Point::new(105.0, -1.0), Point::new(105.0, 11.0)]).unwrap();
        let group = ed.doc.find(g).unwrap();
        let ObjectKind::Group { children } = &group.kind else { panic!("group lost") };
        assert_eq!(children.len(), 3);
        let b = group.bounds().unwrap();
        assert!((b.x0 - 100.0).abs() < 0.05 && (b.x1 - 110.0).abs() < 0.05);
    }

    #[test]
    fn eraser_commits_a_single_step_and_can_delete_a_shape() {
        let (mut ed, _) = editor_with_square();
        let n = ed.erase_with_brush(&[Point::new(10.0, 5.0), Point::new(10.0, 6.0)], 2.0, BrushShape::Round).unwrap();
        assert_eq!(n, 1);
        assert_eq!(ed.doc.objects.len(), 1);
        let bitten = ed.doc.objects[0].page_paths().remove(0);
        assert!(!ops::contains(&bitten, Point::new(9.5, 5.5), FillRule::EvenOdd));
        assert!(ops::contains(&bitten, Point::new(5.0, 5.0), FillRule::EvenOdd));
        assert!(ops::contains(&bitten, Point::new(9.5, 0.5), FillRule::EvenOdd));
        assert_eq!(ed.undo().as_deref(), Some("Eraser"));

        ed.erase_with_brush(&[Point::new(5.0, 5.0)], 50.0, BrushShape::Square).unwrap();
        assert!(ed.doc.objects.is_empty());
        assert!(ed.selection.is_empty());
        assert!(ed.erase_with_brush(&[Point::new(5.0, 5.0)], 0.0, BrushShape::Square).is_err());
    }

    #[test]
    fn trace_is_placed_over_the_image_in_page_coordinates() {
        let mut ed = Editor::new(Document::default());
        let layer = ed.doc.default_layer();
        let img = ImageData { encoded: SharedBytes::default(), width_px: 10, height_px: 10, rect: [0.0, 0.0, 50.0, 50.0] };
        let mut o = Object::image("Picture", layer, img);
        o.transform = Affine::translate((20.0, 30.0));
        let src = o.id;
        ed.doc.objects.push(o);
        let mut sq = BezPath::new();
        sq.move_to((2.0, 2.0));
        sq.line_to((4.0, 2.0));
        sq.line_to((4.0, 4.0));
        sq.close_path();
        let id = ed.add_trace(src, layer, &[sq], (10, 10), "Trace").unwrap();
        let b = ed.doc.find(id).unwrap().bounds().unwrap();
        assert_eq!((b.x0, b.y0, b.x1, b.y1), (30.0, 40.0, 40.0, 50.0));
        assert_eq!(ed.selection, vec![id]);
        assert_eq!(ed.undo().as_deref(), Some("Trace"));
        assert!(ed.add_trace(id, layer, &[], (10, 10), "Trace").is_err());
    }

    #[test]
    fn live_edits_commit_as_one_step() {
        let (mut ed, id) = editor_with_square();
        let before = ed.doc.clone();
        let mut np = ed.path_nodes(id).unwrap();
        np.move_anchor(NodeId { sub: 0, index: 2 }, Point::new(12.0, 12.0));
        assert!(ed.set_path_live(id, np.to_bez()));
        np.move_anchor(NodeId { sub: 0, index: 2 }, Point::new(14.0, 14.0));
        assert!(ed.set_path_live(id, np.to_bez()));
        ed.commit_edit("Move Point", before);
        assert_eq!(ed.doc.objects[0].bounds().unwrap().x1, 14.0);
        assert_eq!(ed.undo().as_deref(), Some("Move Point"));
        assert_eq!(ed.doc.objects[0].bounds().unwrap().x1, 10.0);
    }

    #[test]
    fn node_delete_and_insert_are_undoable() {
        let (mut ed, id) = editor_with_square();
        let n = ed.insert_path_node(id, Point::new(5.0, 0.0), 0.1).unwrap();
        assert_eq!(ed.path_nodes(id).unwrap().subs[0].nodes.len(), 5);
        ed.delete_path_node(id, n).unwrap();
        assert_eq!(ed.path_nodes(id).unwrap().subs[0].nodes.len(), 4);
        assert_eq!(ed.undo().as_deref(), Some("Delete Point"));
        assert_eq!(ed.undo().as_deref(), Some("Add Point"));
        assert!(ed.insert_path_node(id, Point::new(50.0, 50.0), 0.1).is_err());
    }

    #[test]
    fn advanced_edit_points_ops_are_undoable() {
        let (mut ed, id) = editor_with_square();
        ed.doc.find_mut(id).unwrap().style.fill = Some(Rgba::RED);

        let n0 = NodeId { sub: 0, index: 0 };
        ed.set_segment_curved(id, n0, true).unwrap();
        assert_eq!(ed.path_nodes(id).unwrap().segment_is_curved(n0), Some(true));

        ed.set_node_smooth(id, n0, true).unwrap();
        let node = ed.path_nodes(id).unwrap().node(n0).copied().unwrap();
        assert!(node.smooth && node.hin.is_some() && node.hout.is_some());
        ed.set_node_smooth(id, n0, false).unwrap();
        assert_eq!(ed.undo().as_deref(), Some("Corner Point"));

        ed.set_segment_curved(id, n0, false).unwrap();
        assert_eq!(ed.path_nodes(id).unwrap().segment_is_curved(n0), Some(false));

        let ep = ed.break_path_node(id, n0).unwrap();
        assert_eq!(ep, n0);
        assert!(!ed.path_nodes(id).unwrap().subs[0].closed);
        assert!(ed.doc.find(id).unwrap().style.fill.is_none());
        assert_eq!(ed.undo().as_deref(), Some("Break Path"));
        assert!(ed.doc.find(id).unwrap().style.fill.is_some());

        // Simplify collapses collinear inserted points back down to 4 corners.
        let (mut ed2, id2) = editor_with_square();
        ed2.insert_path_node(id2, Point::new(5.0, 0.0), 0.1).unwrap();
        let (before, after) = ed2.simplify_path(id2, 0.1).unwrap();
        assert_eq!(before, 5);
        assert_eq!(after, 4);
        assert_eq!(ed2.undo().as_deref(), Some("Simplify Path"));
    }

    #[test]
    fn replicate_step_mirror_grid_and_fill_page_work_as_single_undo_steps() {
        let (mut ed, id) = editor_with_square();
        ed.selection = vec![id];
        ed.replicate_selected(ReplicateOp::Step { dir: ReplicateDir::Right, gap_mm: 2.0 }).unwrap();
        assert_eq!(ed.doc.objects.len(), 2);
        let b1 = ed.doc.objects[1].bounds().unwrap();
        assert!((b1.x0 - 12.0).abs() < 1e-6 && (b1.x1 - 22.0).abs() < 1e-6);
        assert_eq!(ed.undo().as_deref(), Some("Duplicate"));
        assert_eq!(ed.doc.objects.len(), 1);

        ed.selection = vec![id];
        ed.replicate_selected(ReplicateOp::Mirror { dir: ReplicateDir::Below, gap_mm: 4.0 }).unwrap();
        assert_eq!(ed.doc.objects.len(), 2);
        let bm = ed.doc.objects[1].bounds().unwrap();
        assert!((bm.y0 - 14.0).abs() < 1e-6 && (bm.y1 - 24.0).abs() < 1e-6);
        assert_eq!(ed.undo().as_deref(), Some("Mirror Copy"));

        ed.selection = vec![id];
        ed.replicate_selected(ReplicateOp::Grid { rows: 2, cols: 3, gap_x_mm: 2.0, gap_y_mm: 2.0 }).unwrap();
        assert_eq!(ed.doc.objects.len(), 6);
        assert_eq!(ed.undo().as_deref(), Some("Replicate Grid"));

        // 10×10 square on default 304.8×304.8 page with 6.35mm margin (292.1×292.1 cuttable):
        // with 2mm gap, step = 12mm -> floor((292.1 + 2)/12) = 24 per side => 576 total.
        ed.selection = vec![id];
        ed.replicate_selected(ReplicateOp::FillPage { gap_x_mm: 2.0, gap_y_mm: 2.0 }).unwrap();
        assert_eq!(ed.doc.objects.len(), 24 * 24);
        assert_eq!(ed.undo().as_deref(), Some("Fill Page"));
        assert_eq!(ed.doc.objects.len(), 1);
    }
}
