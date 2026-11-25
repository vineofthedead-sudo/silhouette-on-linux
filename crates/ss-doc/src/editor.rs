//! Editing operations with selection and undo. Every mutating call records one undo step.

use std::collections::HashSet;

use ss_geom::boolean::{self, BoolOp};
use ss_geom::fit::FitOptions;
use ss_geom::offset::{self, Join};
use ss_geom::{Affine, BezPath, FillRule, Point, Rect, Vec2, ops};

use crate::history::History;
use crate::model::*;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EditError {
    #[error("nothing is selected")]
    EmptySelection,
    #[error("this needs at least {0} selected objects")]
    NeedMore(usize),
    #[error("the operation produced no geometry")]
    EmptyResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    CenterH,
    Right,
    Top,
    CenterV,
    Bottom,
}

#[derive(Debug, Clone, Default)]
pub struct Editor {
    pub doc: Document,
    pub selection: Vec<ObjectId>,
    pub history: History,
    /// Flattening tolerance for booleans and offsets, mm.
    pub tolerance: f64,
    pub dirty: bool,
}

impl Editor {
    pub fn new(doc: Document) -> Self {
        Self { doc, selection: Vec::new(), history: History::default(), tolerance: ss_geom::DEFAULT_TOLERANCE, dirty: false }
    }

    /// Runs `f` as one undoable step. If `f` fails the document is left untouched.
    pub fn apply<T>(
        &mut self,
        label: &str,
        f: impl FnOnce(&mut Document, &mut Vec<ObjectId>) -> Result<T, EditError>,
    ) -> Result<T, EditError> {
        let before = self.doc.clone();
        let sel_before = self.selection.clone();
        match f(&mut self.doc, &mut self.selection) {
            Ok(v) => {
                self.history.record(label, before);
                self.dirty = true;
                self.prune_selection();
                Ok(v)
            }
            Err(e) => {
                self.doc = before;
                self.selection = sel_before;
                Err(e)
            }
        }
    }

    pub fn undo(&mut self) -> Option<String> {
        let r = self.history.undo(&mut self.doc);
        self.prune_selection();
        if r.is_some() {
            self.dirty = true;
        }
        r
    }

    pub fn redo(&mut self) -> Option<String> {
        let r = self.history.redo(&mut self.doc);
        self.prune_selection();
        if r.is_some() {
            self.dirty = true;
        }
        r
    }

    fn prune_selection(&mut self) {
        let ids: HashSet<ObjectId> = self.doc.objects.iter().map(|o| o.id).collect();
        self.selection.retain(|id| ids.contains(id));
    }

    pub fn select_only(&mut self, id: ObjectId) {
        self.selection = vec![id];
    }

    pub fn toggle_select(&mut self, id: ObjectId) {
        if let Some(i) = self.selection.iter().position(|s| *s == id) {
            self.selection.remove(i);
        } else {
            self.selection.push(id);
        }
    }

    pub fn select_all(&mut self) {
        let doc = &self.doc;
        self.selection = doc
            .visible_objects()
            .filter(|o| !o.locked && !doc.layer(o.layer).is_some_and(|l| l.locked))
            .map(|o| o.id)
            .collect();
    }

    /// Selects unlocked objects whose bounds lie fully inside `r` (marquee).
    pub fn select_in_rect(&mut self, r: Rect, add: bool) {
        if !add {
            self.selection.clear();
        }
        let hits: Vec<ObjectId> = self
            .doc
            .visible_objects()
            .filter(|o| !o.locked)
            .filter(|o| o.bounds().is_some_and(|b| r.contains(b.origin()) && r.contains(Point::new(b.x1, b.y1))))
            .map(|o| o.id)
            .collect();
        for id in hits {
            if !self.selection.contains(&id) {
                self.selection.push(id);
            }
        }
    }

    pub fn selected(&self) -> Vec<&Object> {
        // Keep z-order, not click order.
        self.doc.objects.iter().filter(|o| self.selection.contains(&o.id)).collect()
    }

    pub fn selection_bounds(&self) -> Option<Rect> {
        self.selected().iter().filter_map(|o| o.bounds()).reduce(|a, b| a.union(b))
    }

    pub fn add(&mut self, obj: Object) -> ObjectId {
        let id = obj.id;
        let _ = self.apply("Add", |d, sel| {
            d.objects.push(obj);
            *sel = vec![id];
            Ok(())
        });
        id
    }

    pub fn delete_selected(&mut self) -> Result<(), EditError> {
        self.require(1)?;
        self.apply("Delete", |d, sel| {
            d.objects.retain(|o| !sel.contains(&o.id));
            sel.clear();
            Ok(())
        })
    }

    fn require(&self, n: usize) -> Result<(), EditError> {
        match self.selection.len() {
            0 => Err(EditError::EmptySelection),
            k if k < n => Err(EditError::NeedMore(n)),
            _ => Ok(()),
        }
    }

    /// Pre-multiplies the page transform of every selected, unlocked object.
    pub fn transform_selected(&mut self, label: &str, m: Affine) -> Result<(), EditError> {
        self.require(1)?;
        self.apply(label, |d, sel| {
            for o in d.objects.iter_mut().filter(|o| sel.contains(&o.id) && !o.locked) {
                o.transform = m * o.transform;
            }
            Ok(())
        })
    }

    pub fn move_selected(&mut self, delta: Vec2) -> Result<(), EditError> {
        self.transform_selected("Move", Affine::translate(delta))
    }

    /// Rotates around the selection centre (degrees, clockwise on screen since y points down).
    pub fn rotate_selected(&mut self, degrees: f64) -> Result<(), EditError> {
        let c = self.selection_bounds().ok_or(EditError::EmptySelection)?.center();
        let m = Affine::translate(c.to_vec2()) * Affine::rotate(degrees.to_radians()) * Affine::translate(-c.to_vec2());
        self.transform_selected("Rotate", m)
    }

    pub fn scale_selected(&mut self, sx: f64, sy: f64, anchor: Point) -> Result<(), EditError> {
        let m = Affine::translate(anchor.to_vec2()) * Affine::scale_non_uniform(sx, sy) * Affine::translate(-anchor.to_vec2());
        self.transform_selected("Scale", m)
    }

    pub fn flip_selected(&mut self, horizontal: bool) -> Result<(), EditError> {
        let c = self.selection_bounds().ok_or(EditError::EmptySelection)?.center();
        let (sx, sy) = if horizontal { (-1.0, 1.0) } else { (1.0, -1.0) };
        self.scale_selected(sx, sy, c)
    }

    pub fn duplicate_selected(&mut self, offset: Vec2) -> Result<Vec<ObjectId>, EditError> {
        self.require(1)?;
        self.apply("Duplicate", |d, sel| {
            let mut copies: Vec<Object> = d.objects.iter().filter(|o| sel.contains(&o.id)).cloned().collect();
            for c in &mut copies {
                c.reassign_ids();
                c.transform = Affine::translate(offset) * c.transform;
            }
            let ids: Vec<ObjectId> = copies.iter().map(|c| c.id).collect();
            d.objects.extend(copies);
            *sel = ids.clone();
            Ok(ids)
        })
    }

    pub fn group_selected(&mut self) -> Result<ObjectId, EditError> {
        self.require(2)?;
        self.apply("Group", |d, sel| {
            let first = d.objects.iter().position(|o| sel.contains(&o.id)).ok_or(EditError::EmptySelection)?;
            let (children, rest): (Vec<Object>, Vec<Object>) = d.objects.drain(..).partition(|o| sel.contains(&o.id));
            d.objects = rest;
            let layer = children[0].layer;
            let mut g = Object::path("Group", layer, BezPath::new());
            g.kind = ObjectKind::Group { children };
            let id = g.id;
            d.objects.insert(first.min(d.objects.len()), g);
            *sel = vec![id];
            Ok(id)
        })
    }

    pub fn ungroup_selected(&mut self) -> Result<(), EditError> {
        self.require(1)?;
        self.apply("Ungroup", |d, sel| {
            let mut out = Vec::with_capacity(d.objects.len());
            let mut new_sel = Vec::new();
            for o in d.objects.drain(..) {
                match o.kind {
                    ObjectKind::Group { children } if sel.contains(&o.id) => {
                        for mut c in children {
                            c.transform = o.transform * c.transform;
                            new_sel.push(c.id);
                            out.push(c);
                        }
                    }
                    kind => out.push(Object { kind, ..o }),
                }
            }
            d.objects = out;
            *sel = new_sel;
            Ok(())
        })
    }

    /// Replaces the selected objects with one path object built from `path` (page coords).
    fn replace_selection_with(d: &mut Document, sel: &mut Vec<ObjectId>, name: &str, path: BezPath) -> Result<ObjectId, EditError> {
        if path.elements().is_empty() {
            return Err(EditError::EmptyResult);
        }
        let first = d.objects.iter().position(|o| sel.contains(&o.id)).ok_or(EditError::EmptySelection)?;
        let template = d.objects[first].clone();
        d.objects.retain(|o| !sel.contains(&o.id));
        let mut obj = Object::path(name, template.layer, path);
        obj.style = template.style;
        obj.cut = template.cut;
        let id = obj.id;
        d.objects.insert(first.min(d.objects.len()), obj);
        *sel = vec![id];
        Ok(id)
    }

    fn selected_paths(&self) -> Vec<BezPath> {
        self.selected().iter().map(|o| ops::combine(&o.page_paths())).collect()
    }

    /// Weld (union) all selected objects into one.
    pub fn weld(&mut self) -> Result<ObjectId, EditError> {
        self.require(2)?;
        let polys = boolean::weld(&self.selected_paths(), FillRule::EvenOdd, self.tolerance);
        let path = boolean::polygons_to_path(&polys, Some(FitOptions::default()));
        self.apply("Weld", |d, sel| Self::replace_selection_with(d, sel, "Weld", path))
    }

    /// Subtract the front objects from the backmost selected object.
    pub fn subtract(&mut self) -> Result<ObjectId, EditError> {
        self.require(2)?;
        let paths = self.selected_paths();
        let polys = boolean::subtract(&paths[0], &paths[1..], FillRule::EvenOdd, self.tolerance);
        let path = boolean::polygons_to_path(&polys, Some(FitOptions::default()));
        self.apply("Subtract", |d, sel| Self::replace_selection_with(d, sel, "Subtract", path))
    }

    pub fn intersect(&mut self) -> Result<ObjectId, EditError> {
        self.require(2)?;
        let paths = self.selected_paths();
        let mut acc: Vec<ss_geom::Contour> = ss_geom::flatten::to_contours([&paths[0]], self.tolerance);
        for p in &paths[1..] {
            let c = ss_geom::flatten::to_contours([p], self.tolerance);
            acc = boolean::boolean_contours(&acc, &c, BoolOp::Intersect, FillRule::EvenOdd).into_iter().flatten().collect();
        }
        let path = boolean::polygons_to_path(&[acc], Some(FitOptions::default()));
        self.apply("Intersect", |d, sel| Self::replace_selection_with(d, sel, "Intersect", path))
    }

    /// Offset (positive) or inset (negative) the selection as a new object behind it.
    pub fn offset_selected(&mut self, distance_mm: f64, join: Join) -> Result<ObjectId, EditError> {
        self.require(1)?;
        let paths = self.selected_paths();
        let polys = offset::offset(&paths, distance_mm, join, FillRule::EvenOdd, self.tolerance);
        let path = boolean::polygons_to_path(&polys, Some(FitOptions::default()));
        if path.elements().is_empty() {
            return Err(EditError::EmptyResult);
        }
        self.apply("Offset", |d, sel| {
            let first = d.objects.iter().position(|o| sel.contains(&o.id)).ok_or(EditError::EmptySelection)?;
            let layer = d.objects[first].layer;
            let obj = Object::path("Offset", layer, path);
            let id = obj.id;
            d.objects.insert(first, obj);
            *sel = vec![id];
            Ok(id)
        })
    }

    pub fn make_compound(&mut self) -> Result<ObjectId, EditError> {
        self.require(2)?;
        let path = ops::combine(&self.selected_paths());
        self.apply("Make Compound Path", |d, sel| Self::replace_selection_with(d, sel, "Compound Path", path))
    }

    pub fn release_compound(&mut self) -> Result<(), EditError> {
        self.require(1)?;
        self.apply("Release Compound Path", |d, sel| {
            let mut out = Vec::new();
            let mut new_sel = Vec::new();
            for o in d.objects.drain(..) {
                let split = sel.contains(&o.id) && matches!(o.kind, ObjectKind::Path { .. });
                if split {
                    for p in o.page_paths().iter().flat_map(ops::subpaths) {
                        let mut n = Object::path(o.name.clone(), o.layer, p);
                        n.style = o.style.clone();
                        n.cut = o.cut;
                        new_sel.push(n.id);
                        out.push(n);
                    }
                } else {
                    out.push(o);
                }
            }
            d.objects = out;
            *sel = new_sel;
            Ok(())
        })
    }

    pub fn align_selected(&mut self, how: Align) -> Result<(), EditError> {
        self.require(2)?;
        let all = self.selection_bounds().ok_or(EditError::EmptySelection)?;
        let moves: Vec<(ObjectId, Vec2)> = self
            .selected()
            .iter()
            .filter_map(|o| {
                let b = o.bounds()?;
                let d = match how {
                    Align::Left => Vec2::new(all.x0 - b.x0, 0.0),
                    Align::Right => Vec2::new(all.x1 - b.x1, 0.0),
                    Align::CenterH => Vec2::new(all.center().x - b.center().x, 0.0),
                    Align::Top => Vec2::new(0.0, all.y0 - b.y0),
                    Align::Bottom => Vec2::new(0.0, all.y1 - b.y1),
                    Align::CenterV => Vec2::new(0.0, all.center().y - b.center().y),
                };
                Some((o.id, d))
            })
            .collect();
        self.apply("Align", |d, _| {
            for (id, v) in moves {
                if let Some(o) = d.find_mut(id) {
                    o.transform = Affine::translate(v) * o.transform;
                }
            }
            Ok(())
        })
    }

    /// Equal gaps between selected objects along x (`horizontal`) or y.
    pub fn distribute_selected(&mut self, horizontal: bool) -> Result<(), EditError> {
        self.require(3)?;
        let mut items: Vec<(ObjectId, Rect)> = self.selected().iter().filter_map(|o| Some((o.id, o.bounds()?))).collect();
        let key = |r: &Rect| if horizontal { r.x0 } else { r.y0 };
        let size = |r: &Rect| if horizontal { r.width() } else { r.height() };
        items.sort_by(|a, b| key(&a.1).total_cmp(&key(&b.1)));
        let (first, last) = (items[0].1, items[items.len() - 1].1);
        let span = if horizontal { last.x1 - first.x0 } else { last.y1 - first.y0 };
        let gap = (span - items.iter().map(|(_, r)| size(r)).sum::<f64>()) / (items.len() - 1) as f64;
        let mut cursor = key(&first);
        let mut moves = Vec::new();
        for (id, r) in &items {
            let delta = cursor - key(r);
            moves.push((*id, if horizontal { Vec2::new(delta, 0.0) } else { Vec2::new(0.0, delta) }));
            cursor += size(r) + gap;
        }
        self.apply("Distribute", |d, _| {
            for (id, v) in moves {
                if let Some(o) = d.find_mut(id) {
                    o.transform = Affine::translate(v) * o.transform;
                }
            }
            Ok(())
        })
    }

    /// Moves selected objects one step (or all the way) towards the front or back.
    pub fn reorder_selected(&mut self, to_front: bool, all_the_way: bool) -> Result<(), EditError> {
        self.require(1)?;
        self.apply("Arrange", |d, sel| {
            if all_the_way {
                let (picked, rest): (Vec<Object>, Vec<Object>) = d.objects.drain(..).partition(|o| sel.contains(&o.id));
                d.objects = if to_front { rest.into_iter().chain(picked).collect() } else { picked.into_iter().chain(rest).collect() };
                return Ok(());
            }
            let n = d.objects.len();
            let picked = |o: &Object| sel.contains(&o.id);
            if to_front {
                for i in (0..n.saturating_sub(1)).rev() {
                    if picked(&d.objects[i]) && !picked(&d.objects[i + 1]) {
                        d.objects.swap(i, i + 1);
                    }
                }
            } else {
                for i in 1..n {
                    if picked(&d.objects[i]) && !picked(&d.objects[i - 1]) {
                        d.objects.swap(i, i - 1);
                    }
                }
            }
            Ok(())
        })
    }
    pub fn set_cut_action(&mut self, action: CutAction) -> Result<(), EditError> {
        self.require(1)?;
        self.apply("Cut Settings", |d, sel| {
            for o in d.objects.iter_mut().filter(|o| sel.contains(&o.id)) {
                o.cut = action;
            }
            Ok(())
        })
    }

    pub fn set_style(&mut self, f: impl Fn(&mut Style)) -> Result<(), EditError> {
        self.require(1)?;
        self.apply("Style", |d, sel| {
            for o in d.objects.iter_mut().filter(|o| sel.contains(&o.id)) {
                f(&mut o.style);
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ss_geom::kurbo::{Circle, Shape};

    fn ed_with_two_squares() -> (Editor, ObjectId, ObjectId) {
        let mut ed = Editor::new(Document::default());
        let l = ed.doc.default_layer();
        let a = ed.add(Object::path("A", l, Rect::new(0.0, 0.0, 10.0, 10.0).to_path(0.0)));
        let b = ed.add(Object::path("B", l, Rect::new(5.0, 5.0, 15.0, 15.0).to_path(0.0)));
        (ed, a, b)
    }

    #[test]
    fn weld_then_undo_redo() {
        let (mut ed, a, b) = ed_with_two_squares();
        ed.selection = vec![a, b];
        let w = ed.weld().unwrap();
        assert_eq!(ed.doc.objects.len(), 1);
        let area = ed.doc.find(w).unwrap().page_paths()[0].area().abs();
        assert!((area - 175.0).abs() < 1e-6, "area {area}");
        assert_eq!(ed.undo().as_deref(), Some("Weld"));
        assert_eq!(ed.doc.objects.len(), 2);
        assert_eq!(ed.redo().as_deref(), Some("Weld"));
        assert_eq!(ed.doc.objects.len(), 1);
    }

    #[test]
    fn subtract_uses_back_object_as_base() {
        let (mut ed, a, b) = ed_with_two_squares();
        ed.selection = vec![b, a];
        let id = ed.subtract().unwrap();
        let area: f64 = ed.doc.find(id).unwrap().page_paths()[0].area().abs();
        assert!((area - 75.0).abs() < 1e-6);
    }

    #[test]
    fn failed_op_does_not_record_history() {
        let (mut ed, a, _) = ed_with_two_squares();
        ed.selection = vec![a];
        assert_eq!(ed.weld(), Err(EditError::NeedMore(2)));
        assert_eq!(ed.history.undo_label(), Some("Add"));
    }

    #[test]
    fn group_transform_ungroup_keeps_geometry() {
        let (mut ed, a, b) = ed_with_two_squares();
        ed.selection = vec![a, b];
        let g = ed.group_selected().unwrap();
        assert_eq!(ed.doc.objects.len(), 1);
        ed.select_only(g);
        ed.move_selected(Vec2::new(100.0, 0.0)).unwrap();
        ed.rotate_selected(90.0).unwrap();
        let before = ed.doc.find(g).unwrap().bounds().unwrap();
        ed.ungroup_selected().unwrap();
        assert_eq!(ed.doc.objects.len(), 2);
        let after = ed.doc.bounds().unwrap();
        assert!((before.x0 - after.x0).abs() < 1e-9 && (before.y1 - after.y1).abs() < 1e-9);
    }

    #[test]
    fn align_distribute_and_arrange() {
        let mut ed = Editor::new(Document::default());
        let l = ed.doc.default_layer();
        let ids: Vec<_> = (0..3)
            .map(|i| ed.add(Object::path(format!("{i}"), l, Rect::new(0.0, 0.0, 10.0, 10.0).to_path(0.0))))
            .collect();
        ed.selection = ids.clone();
        ed.move_selected(Vec2::new(0.0, 0.0)).unwrap();
        ed.select_only(ids[2]);
        ed.move_selected(Vec2::new(90.0, 30.0)).unwrap();
        ed.select_only(ids[1]);
        ed.move_selected(Vec2::new(20.0, 10.0)).unwrap();
        ed.selection = ids.clone();
        ed.distribute_selected(true).unwrap();
        let x = |ed: &Editor, i: usize| ed.doc.find(ids[i]).unwrap().bounds().unwrap().x0;
        assert!((x(&ed, 1) - 45.0).abs() < 1e-9);
        ed.align_selected(Align::Top).unwrap();
        assert!(ed.doc.objects.iter().all(|o| o.bounds().unwrap().y0.abs() < 1e-9));
        ed.select_only(ids[0]);
        ed.reorder_selected(true, false).unwrap();
        assert_eq!(ed.doc.objects[1].id, ids[0]);
        ed.reorder_selected(true, true).unwrap();
        assert_eq!(ed.doc.objects[2].id, ids[0]);
        ed.reorder_selected(false, true).unwrap();
        assert_eq!(ed.doc.objects[0].id, ids[0]);
    }

    #[test]
    fn offset_compound_and_json_roundtrip() {
        let mut ed = Editor::new(Document::default());
        let l = ed.doc.default_layer();
        let c = ed.add(Object::path("C", l, Circle::new((50.0, 50.0), 10.0).to_path(0.01)));
        let s = ed.add(Object::path("S", l, Rect::new(45.0, 45.0, 55.0, 55.0).to_path(0.0)));
        ed.select_only(c);
        let off = ed.offset_selected(2.0, Join::Round).unwrap();
        let r = ed.doc.find(off).unwrap().bounds().unwrap();
        assert!((r.width() - 24.0).abs() < 0.05, "{}", r.width());
        ed.selection = vec![c, s];
        let comp = ed.make_compound().unwrap();
        assert_eq!(ops::subpaths(&ed.doc.find(comp).unwrap().page_paths()[0]).len(), 2);
        ed.select_only(comp);
        ed.release_compound().unwrap();
        assert_eq!(ed.selection.len(), 2);
        let json = ed.doc.to_json().unwrap();
        let back = Document::from_json(&json).unwrap();
        assert_eq!(back.objects.len(), ed.doc.objects.len());
        assert_eq!(back.page, ed.doc.page);
        let (a, b) = (back.bounds().unwrap(), ed.doc.bounds().unwrap());
        assert!((a.x0 - b.x0).abs() < 1e-6 && (a.y1 - b.y1).abs() < 1e-6);
    }

    #[test]
    fn hit_test_topmost() {
        let (ed, _a, b) = ed_with_two_squares();
        assert_eq!(ed.doc.hit_test(Point::new(5.0, 7.0), 0.2), Some(b));
        assert_eq!(ed.doc.hit_test(Point::new(30.0, 30.0), 0.2), None);
    }
}
