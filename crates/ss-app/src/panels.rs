//! Side panels: object properties, layers and page setup.

use egui::{Color32, DragValue, Ui};
use ss_config::{Settings, Units};
use ss_doc::{CutAction, CuttingMat, Editor, Layer, LayerId, ObjectKind, Page, RegMarks, ReplicateDir, ReplicateOp, Rgba};
use ss_geom::{Affine, Rect, Vec2};

use crate::app::{Action, units};
use crate::canvas::color32;

const MEDIA_PRESETS: &[(&str, &str)] = &[
    ("12x12", "Cutting mat 12 × 12 in"),
    ("12x24", "Cutting mat 12 × 24 in"),
    ("8x12", "Cutting mat 8 × 12 in"),
    ("15x15", "Cutting mat 15 × 15 in"),
    ("24x24", "Cutting mat 24 × 24 in"),
    ("6x8", "Cutting mat 6 × 8 in"),
    ("letter", "Letter"),
    ("legal", "Legal"),
    ("a4", "A4"),
    ("a3", "A3"),
];

const PALETTE: &[Rgba] = &[
    Rgba::rgb(0xe0, 0x00, 0x00),
    Rgba::rgb(0x00, 0x60, 0xe0),
    Rgba::rgb(0x00, 0x00, 0x00),
    Rgba::rgb(0x00, 0x9a, 0x3c),
    Rgba::rgb(0xf0, 0x8c, 0x00),
    Rgba::rgb(0x8e, 0x24, 0xaa),
    Rgba::rgb(0xe9, 0x1e, 0x63),
    Rgba::rgb(0x00, 0xa5, 0xb5),
    Rgba::rgb(0x79, 0x55, 0x48),
    Rgba::rgb(0x80, 0x80, 0x80),
    Rgba::rgb(0xff, 0xd6, 0x00),
    Rgba::rgb(0xff, 0xff, 0xff),
];

pub const CUT_ACTIONS: [(CutAction, &str); 5] = [
    (CutAction::Cut, "Cut"),
    (CutAction::CutEdge, "Cut edge"),
    (CutAction::Sketch, "Sketch (pen)"),
    (CutAction::Perforate, "Perforate"),
    (CutAction::NoCut, "No cut"),
];

/// Number fields that hold an edited value until the edit is finished, so a drag or typed
/// value becomes one undo step instead of one per frame.
struct Draft<const N: usize> {
    id: egui::Id,
    values: [f64; N],
    active: bool,
    commit: bool,
}

impl<const N: usize> Draft<N> {
    fn new(ui: &Ui, salt: &str, current: [f64; N]) -> Self {
        let id = ui.scope_id().with(salt);
        let values = ui.data(|d| d.get_temp::<[f64; N]>(id)).unwrap_or(current);
        Self { id, values, active: false, commit: false }
    }

    fn field(&mut self, ui: &mut Ui, i: usize, speed: f64, suffix: &str, decimals: usize) {
        let r = ui.add(DragValue::new(&mut self.values[i]).speed(speed).suffix(suffix).fixed_decimals(decimals));
        self.active |= r.dragged() || r.has_focus();
        self.commit |= r.drag_stopped() || r.lost_focus();
    }

    /// Returns the values to commit, if an edit just finished.
    fn finish(self, ui: &Ui) -> Option<[f64; N]> {
        if self.active && !self.commit {
            ui.data_mut(|d| d.insert_temp(self.id, self.values));
        } else {
            ui.data_mut(|d| d.remove::<[f64; N]>(self.id));
        }
        self.commit.then_some(self.values)
    }
}

fn swatches(ui: &mut Ui, current: Option<Rgba>) -> Option<Rgba> {
    let mut picked = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(3.0, 3.0);
        for &c in PALETTE {
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
            let p = ui.painter();
            p.rect_filled(rect, 2.0, color32(c));
            let edge = if current == Some(c) { egui::Stroke::new(2.0, crate::canvas::SELECTION_COLOR) } else { egui::Stroke::new(1.0, Color32::GRAY) };
            p.rect_stroke(rect, 2.0, edge, egui::StrokeKind::Outside);
            if resp.on_hover_text(c.to_hex()).clicked() {
                picked = Some(c);
            }
        }
    });
    picked
}

fn report<T>(status: &mut String, r: Result<T, ss_doc::EditError>) {
    if let Err(e) = r {
        *status = e.to_string();
    }
}

/// Properties of the selection. `bounds` is the selection's bounding box in page mm.
pub fn properties(ui: &mut Ui, editor: &mut Editor, bounds: Option<Rect>, unit: Units, actions: &mut Vec<Action>, status: &mut String) {
    let n = editor.selection.len();
    if n == 0 {
        ui.weak("Nothing selected.");
        ui.add_space(6.0);
        ui.label(format!("{} objects in the design.", editor.doc.objects.len()));
        return;
    }
    let (f, suffix) = units(unit);
    let decimals = if unit == Units::Inch { 3 } else { 2 };
    let suffix = format!(" {suffix}");

    if n == 1 {
        let id = editor.selection[0];
        let Some(obj) = editor.doc.find(id) else { return };
        let old_name = obj.name.clone();
        let kind = match &obj.kind {
            ObjectKind::Path { .. } => "Path",
            ObjectKind::Text { .. } => "Text",
            ObjectKind::Group { .. } => "Group",
            ObjectKind::Image(_) => "Image",
        };
        let name_id = ui.scope_id().with("name");
        let mut name = ui.data(|d| d.get_temp::<String>(name_id)).unwrap_or_else(|| old_name.clone());
        ui.horizontal(|ui| {
            ui.label(kind);
            let r = ui.text_edit_singleline(&mut name);
            if r.has_focus() {
                ui.data_mut(|d| d.insert_temp(name_id, name.clone()));
            } else {
                ui.data_mut(|d| d.remove::<String>(name_id));
            }
            if r.lost_focus() && name != old_name {
                let r = editor.apply("Rename", |d, _| {
                    if let Some(o) = d.find_mut(id) {
                        o.name = name.clone();
                    }
                    Ok(())
                });
                report(status, r);
            }
        });
        if kind == "Text" {
            ui.horizontal(|ui| {
                if ui.button("Edit text…").clicked() {
                    actions.push(Action::EditText);
                }
                if ui.button("Convert to path").on_hover_text("Turn the letters into ordinary paths for welding and editing").clicked() {
                    actions.push(Action::ConvertTextToPaths);
                }
            });
        }
    } else {
        ui.label(format!("{n} objects selected"));
    }
    ui.separator();

    if let Some(b) = bounds {
        let lock_id = ui.scope_id().with("lock-aspect");
        let mut lock = ui.data(|d| d.get_temp::<bool>(lock_id)).unwrap_or(true);
        let current = [b.x0 * f, b.y0 * f, b.width() * f, b.height() * f];
        let mut draft = Draft::new(ui, "geometry", current);
        let step = if unit == Units::Inch { 0.01 } else { 0.1 };
        egui::Grid::new("geometry").num_columns(4).spacing([6.0, 4.0]).show(ui, |ui| {
            ui.label("X");
            draft.field(ui, 0, step, &suffix, decimals);
            ui.label("Y");
            draft.field(ui, 1, step, &suffix, decimals);
            ui.end_row();
            ui.label("W");
            draft.field(ui, 2, step, &suffix, decimals);
            ui.label("H");
            draft.field(ui, 3, step, &suffix, decimals);
            ui.end_row();
        });
        if ui.checkbox(&mut lock, "Keep proportions").changed() {
            ui.data_mut(|d| d.insert_temp(lock_id, lock));
        }
        if let Some(mut v) = draft.finish(ui) {
            if lock && b.width() > 1e-9 && b.height() > 1e-9 {
                if (v[2] - current[2]).abs() > 1e-9 {
                    v[3] = v[2] * b.height() / b.width();
                } else if (v[3] - current[3]).abs() > 1e-9 {
                    v[2] = v[3] * b.width() / b.height();
                }
            }
            let v = v.map(|x| x / f);
            let scale = |new: f64, old: f64| if old > 1e-9 && new > 1e-9 { new / old } else { 1.0 };
            let (sx, sy) = (scale(v[2], b.width()), scale(v[3], b.height()));
            let m = Affine::translate(Vec2::new(v[0], v[1])) * Affine::scale_non_uniform(sx, sy) * Affine::translate(Vec2::new(-b.x0, -b.y0));
            if m != Affine::IDENTITY {
                report(status, editor.transform_selected("Resize", m));
            }
        }
    }

    let rot_id = ui.scope_id().with("rotate-by");
    let mut angle = ui.data(|d| d.get_temp::<f64>(rot_id)).unwrap_or(45.0);
    ui.horizontal(|ui| {
        ui.label("Rotate");
        if ui.add(DragValue::new(&mut angle).speed(1.0).range(-360.0..=360.0).suffix("°")).changed() {
            ui.data_mut(|d| d.insert_temp(rot_id, angle));
        }
        if ui.button("Apply").clicked() {
            actions.push(Action::Rotate(angle));
        }
    });
    ui.horizontal(|ui| {
        if ui.button("−90°").on_hover_text("Rotate 90° counter-clockwise").clicked() {
            actions.push(Action::Rotate(-90.0));
        }
        if ui.button("+90°").on_hover_text("Rotate 90° clockwise").clicked() {
            actions.push(Action::Rotate(90.0));
        }
        if ui.button("Flip ↔").clicked() {
            actions.push(Action::FlipH);
        }
        if ui.button("Flip ↕").clicked() {
            actions.push(Action::FlipV);
        }
    });
    ui.separator();

    let selected = editor.selected();
    let first_cut = selected.first().map(|o| o.cut);
    let same_cut = first_cut.filter(|c| selected.iter().all(|o| o.cut == *c));
    let stroke = selected.first().map(|o| o.style.stroke).filter(|c| selected.iter().all(|o| o.style.stroke == *c));
    let fill = selected.first().map(|o| o.style.fill).filter(|c| selected.iter().all(|o| o.style.fill == *c));
    drop(selected);

    ui.horizontal(|ui| {
        ui.label("Cut action");
        let text = same_cut.and_then(|c| CUT_ACTIONS.iter().find(|(a, _)| *a == c)).map_or("Mixed", |(_, l)| *l);
        egui::ComboBox::from_id_salt("cut-action").selected_text(text).show_ui(ui, |ui| {
            for (a, l) in CUT_ACTIONS {
                if ui.selectable_label(same_cut == Some(a), l).clicked() {
                    actions.push(Action::SetCut(a));
                }
            }
        });
    });
    ui.add_space(4.0);
    ui.label("Line colour");
    if let Some(c) = swatches(ui, stroke) {
        report(status, editor.set_style(|s| s.stroke = c));
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label("Fill");
        let has_fill = fill.is_some_and(|f| f.is_some());
        if ui.selectable_label(!has_fill && fill.is_some(), "None").clicked() {
            report(status, editor.set_style(|s| s.fill = None));
        }
    });
    if let Some(c) = swatches(ui, fill.flatten()) {
        report(status, editor.set_style(|s| s.fill = Some(c)));
    }
    ui.separator();
    ui.horizontal_wrapped(|ui| {
        let many = n >= 2;
        for (label, a, on) in [
            ("Group", Action::Group, many),
            ("Ungroup", Action::Ungroup, true),
            ("Weld", Action::Weld, many),
            ("Subtract", Action::Subtract, many),
            ("Intersect", Action::Intersect, many),
            ("Offset…", Action::OffsetDialog, true),
        ] {
            if ui.add_enabled(on, egui::Button::new(label)).clicked() {
                actions.push(a);
            }
        }
    });
}

type LayerOp = Box<dyn FnOnce(&mut Editor, &mut LayerId)>;

/// Layer list. The active layer receives new objects.
pub fn layers(ui: &mut Ui, editor: &mut Editor, active: &mut LayerId) {
    let mut op: Option<LayerOp> = None;
    let count = editor.doc.layers.len();
    for (i, layer) in editor.doc.layers.iter().enumerate().rev() {
        let id = layer.id;
        let objects = editor.doc.objects.iter().filter(|o| o.layer == id).count();
        ui.horizontal(|ui| {
            if ui.radio(*active == id, "").on_hover_text("Draw on this layer").clicked() {
                *active = id;
            }
            let (mut visible, mut locked) = (layer.visible, layer.locked);
            if ui.checkbox(&mut visible, "").on_hover_text("Visible").changed() {
                op = Some(Box::new(move |e, _| {
                    let _ = e.apply("Layer Visibility", |d, _| {
                        if let Some(l) = d.layers.iter_mut().find(|l| l.id == id) {
                            l.visible = visible;
                        }
                        Ok(())
                    });
                }));
            }
            if ui.checkbox(&mut locked, "🔒").on_hover_text("Locked").changed() {
                op = Some(Box::new(move |e, _| {
                    let _ = e.apply("Lock Layer", |d, sel| {
                        if let Some(l) = d.layers.iter_mut().find(|l| l.id == id) {
                            l.locked = locked;
                        }
                        if locked {
                            let on_layer: Vec<_> = d.objects.iter().filter(|o| o.layer == id).map(|o| o.id).collect();
                            sel.retain(|s| !on_layer.contains(s));
                        }
                        Ok(())
                    });
                }));
            }
            let name_id = ui.scope_id().with(("layer-name", id));
            let mut name = ui.data(|d| d.get_temp::<String>(name_id)).unwrap_or_else(|| layer.name.clone());
            let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(110.0));
            if r.has_focus() {
                ui.data_mut(|d| d.insert_temp(name_id, name.clone()));
            } else {
                ui.data_mut(|d| d.remove::<String>(name_id));
            }
            if r.lost_focus() && name != layer.name {
                op = Some(Box::new(move |e, _| {
                    let _ = e.apply("Rename Layer", |d, _| {
                        if let Some(l) = d.layers.iter_mut().find(|l| l.id == id) {
                            l.name = name;
                        }
                        Ok(())
                    });
                }));
            }
            ui.weak(format!("{objects}"));
            if ui.add_enabled(i + 1 < count, egui::Button::new("▲").small()).on_hover_text("Move up").clicked() {
                op = Some(Box::new(move |e, _| {
                    let _ = e.apply("Reorder Layers", |d, _| {
                        d.layers.swap(i, i + 1);
                        Ok(())
                    });
                }));
            }
            if ui.add_enabled(i > 0, egui::Button::new("▼").small()).on_hover_text("Move down").clicked() {
                op = Some(Box::new(move |e, _| {
                    let _ = e.apply("Reorder Layers", |d, _| {
                        d.layers.swap(i, i - 1);
                        Ok(())
                    });
                }));
            }
        });
    }
    ui.separator();
    ui.horizontal(|ui| {
        if ui.button("Add layer").clicked() {
            op = Some(Box::new(|e, active| {
                let layer = Layer::new(format!("Layer {}", e.doc.layers.len() + 1));
                let id = layer.id;
                let _ = e.apply("Add Layer", |d, _| {
                    d.layers.push(layer);
                    Ok(())
                });
                *active = id;
            }));
        }
        let target = *active;
        if ui
            .add_enabled(count > 1, egui::Button::new("Delete layer"))
            .on_hover_text("Deletes the active layer and the objects on it")
            .clicked()
        {
            op = Some(Box::new(move |e, active| {
                let _ = e.apply("Delete Layer", |d, _| {
                    d.layers.retain(|l| l.id != target);
                    d.objects.retain(|o| o.layer != target);
                    Ok(())
                });
                *active = e.doc.default_layer();
            }));
        }
    });
    let target = *active;
    if ui.add_enabled(!editor.selection.is_empty(), egui::Button::new("Move selection to active layer")).clicked() {
        op = Some(Box::new(move |e, _| {
            let _ = e.apply("Move to Layer", |d, sel| {
                for o in d.objects.iter_mut().filter(|o| sel.contains(&o.id)) {
                    o.layer = target;
                }
                Ok(())
            });
        }));
    }
    if let Some(op) = op {
        op(editor, active);
    }
}

/// What the page panel changed.
#[derive(Debug, Default, Clone, Copy)]
pub struct PageChange {
    /// The page size changed; the view should be refitted.
    pub page: bool,
    /// Settings changed and should be saved.
    pub settings: bool,
}

pub fn page(ui: &mut Ui, editor: &mut Editor, settings: &mut Settings, mat: &mut CuttingMat, actions: &mut Vec<Action>, status: &mut String) -> PageChange {
    let mut change = PageChange::default();
    let (f, unit) = units(settings.units);
    let decimals = if settings.units == Units::Inch { 3 } else { 1 };
    let suffix = format!(" {unit}");
    let page = editor.doc.page.clone();
    let mut new_page: Option<(&str, Page)> = None;

    ui.label("Cutting Mat");
    egui::ComboBox::from_id_salt("cutting-mat").selected_text(mat.label()).show_ui(ui, |ui| {
        for m in CuttingMat::ALL {
            ui.selectable_value(mat, m, m.label());
        }
    });
    ui.weak("Mat display is session-only; media size is independent.");
    ui.heading("Media");
    let label = MEDIA_PRESETS.iter().find(|(k, _)| page.media.eq_ignore_ascii_case(k)).map_or("Custom", |(_, l)| *l);
    egui::ComboBox::from_id_salt("media").selected_text(label).width(200.0).show_ui(ui, |ui| {
        for (key, l) in MEDIA_PRESETS {
            if ui.selectable_label(page.media.eq_ignore_ascii_case(key), *l).clicked()
                && let Some(mut p) = Page::preset(key)
            {
                p.margin_mm = page.margin_mm;
                p.show_cut_border = page.show_cut_border;
                p.registration = page.registration;
                new_page = Some(("Media Size", p));
            }
        }
    });
    let mut draft = Draft::new(ui, "page-size", [page.width_mm * f, page.height_mm * f, page.margin_mm * f]);
    egui::Grid::new("page-size").num_columns(2).spacing([6.0, 4.0]).show(ui, |ui| {
        ui.label("Width");
        draft.field(ui, 0, 0.5 * f, &suffix, decimals);
        ui.end_row();
        ui.label("Height");
        draft.field(ui, 1, 0.5 * f, &suffix, decimals);
        ui.end_row();
        ui.label("Cut border");
        draft.field(ui, 2, 0.1 * f, &suffix, decimals);
        ui.end_row();
    });
    if let Some(v) = draft.finish(ui) {
        let w = (v[0] / f).clamp(10.0, 5000.0);
        let h = (v[1] / f).clamp(10.0, 50000.0);
        let m = (v[2] / f).clamp(0.0, w.min(h) / 2.0);
        if (w, h, m) != (page.width_mm, page.height_mm, page.margin_mm) {
            let media = if (w, h) == (page.width_mm, page.height_mm) { page.media.clone() } else { "custom".into() };
            new_page = Some(("Page Size", Page { width_mm: w, height_mm: h, margin_mm: m, media, ..page.clone() }));
        }
    }
    let is_landscape = page.width_mm > page.height_mm;
    ui.horizontal(|ui| {
        ui.label("Orientation");
        if ui.selectable_label(!is_landscape, "Portrait").clicked() && is_landscape {
            let mut p = page.clone();
            std::mem::swap(&mut p.width_mm, &mut p.height_mm);
            new_page = Some(("Page Orientation", p));
        }
        if ui.selectable_label(is_landscape, "Landscape").clicked() && !is_landscape {
            let mut p = page.clone();
            std::mem::swap(&mut p.width_mm, &mut p.height_mm);
            new_page = Some(("Page Orientation", p));
        }
    });
    let mut border = page.show_cut_border;
    if ui.checkbox(&mut border, "Show cut border").changed() {
        new_page = Some(("Cut Border", Page { show_cut_border: border, ..page.clone() }));
    }
    if let Some((label, p)) = new_page {
        change.page = (p.width_mm, p.height_mm) != (page.width_mm, page.height_mm);
        let _ = editor.apply(label, |d, _| {
            d.page = p;
            Ok(())
        });
    }
    let is_default = settings.default_media.eq_ignore_ascii_case(&editor.doc.page.media);
    if ui
        .add_enabled(!is_default && Page::preset(&editor.doc.page.media).is_some(), egui::Button::new("Use for new designs"))
        .clicked()
    {
        settings.default_media = editor.doc.page.media.clone();
        change.settings = true;
    }

    ui.separator();
    ui.heading("Grid");
    change.settings |= ui.checkbox(&mut settings.show_grid, "Show grid").changed();
    change.settings |= ui.checkbox(&mut settings.snap_to_grid, "Snap to grid").changed();
    ui.horizontal(|ui| {
        ui.label("Spacing");
        let mut g = settings.grid_spacing_mm * f;
        let r = ui.add(DragValue::new(&mut g).speed(0.1 * f).range((0.5 * f)..=(100.0 * f)).suffix(&suffix).fixed_decimals(decimals));
        if r.changed() {
            settings.grid_spacing_mm = g / f;
            change.settings = true;
        }
    });

    ui.separator();
    ui.heading("Print & Cut");
    let marks = editor.doc.page.registration;
    let mut on = marks.is_some();
    if ui.checkbox(&mut on, "Registration marks").on_hover_text("Print the design with these marks, then let the cutter find them to line up the cut").changed() {
        report(status, editor.set_registration(on.then(RegMarks::default)));
    }
    if let Some(m) = marks {
        let mut draft = Draft::new(ui, "regmarks", [m.inset_mm * f, m.length_mm * f, m.thickness_mm * f, m.square_mm * f]);
        let fine = if settings.units == Units::Inch { 3 } else { 2 };
        egui::Grid::new("regmarks").num_columns(2).spacing([6.0, 4.0]).show(ui, |ui| {
            ui.label("Inset");
            draft.field(ui, 0, 0.1 * f, &suffix, decimals);
            ui.end_row();
            ui.label("Length");
            draft.field(ui, 1, 0.1 * f, &suffix, decimals);
            ui.end_row();
            ui.label("Thickness");
            draft.field(ui, 2, 0.01 * f, &suffix, fine);
            ui.end_row();
            ui.label("Square");
            draft.field(ui, 3, 0.1 * f, &suffix, decimals);
            ui.end_row();
        });
        if let Some(v) = draft.finish(ui) {
            let new = RegMarks { inset_mm: v[0] / f, length_mm: v[1] / f, thickness_mm: v[2] / f, square_mm: v[3] / f };
            if new != m {
                report(status, editor.set_registration(Some(new)));
            }
        }
        ui.weak("Keep artwork and cut lines clear of the marks.");
    }

    ui.separator();
    ui.heading("Background");
    match &editor.doc.background {
        Some(bg) => {
            ui.label(format!("Image {} × {} px (shown, never cut)", bg.width_px, bg.height_px));
        }
        None => {
            ui.weak("No background image.");
        }
    }
    ui.horizontal(|ui| {
        if ui.button("Choose image…").clicked() {
            actions.push(Action::SetBackground);
        }
        if ui.add_enabled(editor.doc.background.is_some(), egui::Button::new("Remove")).clicked() {
            actions.push(Action::ClearBackground);
        }
    });
    change
}

/// Replicate panel: directional duplicates, mirror copies, rows/columns, custom grid, and Fill Page.
pub fn replicate(ui: &mut Ui, editor: &mut Editor, unit: Units, status: &mut String) {
    if editor.selection.is_empty() {
        ui.weak("Select one or more objects to replicate.");
        return;
    }
    let (f, suffix) = units(unit);
    let decimals = if unit == Units::Inch { 3 } else { 1 };
    let suffix = format!(" {suffix}");

    let rows_id = ui.scope_id().with("rep-rows");
    let cols_id = ui.scope_id().with("rep-cols");
    let gx_id = ui.scope_id().with("rep-gx");
    let gy_id = ui.scope_id().with("rep-gy");
    let mut rows = ui.data(|d| d.get_temp::<usize>(rows_id)).unwrap_or(2);
    let mut cols = ui.data(|d| d.get_temp::<usize>(cols_id)).unwrap_or(3);
    let mut gap_x_mm = ui.data(|d| d.get_temp::<f64>(gx_id)).unwrap_or(2.0);
    let mut gap_y_mm = ui.data(|d| d.get_temp::<f64>(gy_id)).unwrap_or(2.0);

    ui.horizontal(|ui| {
        ui.label("Gap X");
        let mut gx = gap_x_mm * f;
        if ui.add(DragValue::new(&mut gx).speed(0.2 * f).range(0.0..=(200.0 * f)).suffix(&suffix).fixed_decimals(decimals)).changed() {
            gap_x_mm = gx / f;
            ui.data_mut(|d| d.insert_temp(gx_id, gap_x_mm));
        }
        ui.label("Y");
        let mut gy = gap_y_mm * f;
        if ui.add(DragValue::new(&mut gy).speed(0.2 * f).range(0.0..=(200.0 * f)).suffix(&suffix).fixed_decimals(decimals)).changed() {
            gap_y_mm = gy / f;
            ui.data_mut(|d| d.insert_temp(gy_id, gap_y_mm));
        }
    });

    ui.label("Duplicate");
    ui.horizontal_wrapped(|ui| {
        for (label, dir, g) in [
            ("Left", ReplicateDir::Left, gap_x_mm),
            ("Right", ReplicateDir::Right, gap_x_mm),
            ("Above", ReplicateDir::Above, gap_y_mm),
            ("Below", ReplicateDir::Below, gap_y_mm),
        ] {
            if ui.button(label).clicked() {
                report(status, editor.replicate_selected(ReplicateOp::Step { dir, gap_mm: g }));
            }
        }
    });

    ui.label("Mirror Copy");
    ui.horizontal_wrapped(|ui| {
        for (label, dir, g) in [
            ("Left", ReplicateDir::Left, gap_x_mm),
            ("Right", ReplicateDir::Right, gap_x_mm),
            ("Above", ReplicateDir::Above, gap_y_mm),
            ("Below", ReplicateDir::Below, gap_y_mm),
        ] {
            if ui.button(label).clicked() {
                report(status, editor.replicate_selected(ReplicateOp::Mirror { dir, gap_mm: g }));
            }
        }
    });

    ui.label("Row / Column");
    ui.horizontal_wrapped(|ui| {
        for (label, r, c) in [("Row ×3", 1, 3), ("Row ×4", 1, 4), ("Col ×3", 3, 1), ("Col ×4", 4, 1)] {
            if ui.button(label).clicked() {
                report(status, editor.replicate_selected(ReplicateOp::Grid { rows: r, cols: c, gap_x_mm, gap_y_mm }));
            }
        }
    });

    ui.separator();
    ui.label("Grid Array");
    ui.horizontal(|ui| {
        ui.label("Rows");
        if ui.add(DragValue::new(&mut rows).range(1..=50)).changed() {
            ui.data_mut(|d| d.insert_temp(rows_id, rows));
        }
        ui.label("Cols");
        if ui.add(DragValue::new(&mut cols).range(1..=50)).changed() {
            ui.data_mut(|d| d.insert_temp(cols_id, cols));
        }
        if ui.button("Apply Grid").clicked() {
            report(status, editor.replicate_selected(ReplicateOp::Grid { rows, cols, gap_x_mm, gap_y_mm }));
        }
    });

    if ui.button("Fill Page").on_hover_text("Auto-nest copies across the active media inside its cut border").clicked() {
        report(status, editor.replicate_selected(ReplicateOp::FillPage { gap_x_mm, gap_y_mm }));
    }
}
