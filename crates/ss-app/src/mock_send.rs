//! V5 Send layout. This module deliberately has no device or GPGL dependencies.

use std::collections::{BTreeMap, HashMap};

use egui::Ui;
use ss_doc::{CutAction, Editor, ObjectKind, Rgba};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
enum Mode {
    #[default]
    Simple,
    Line,
    Fill,
}

#[derive(Debug, Clone, Copy)]
struct Material {
    name: &'static str,
    force: u32,
    speed: u32,
    passes: u32,
    blade_depth: f64,
}

// SPEC_S/condition.csv, Cameo leaf rows. Depth is the adjustable-blade value
// (0.1–0.9), not an AutoBlade dial number. Passes in this resource are 1–2.
const MATERIALS: &[Material] = &[
    Material { name: "Cardstock", force: 20, speed: 4, passes: 1, blade_depth: 0.3 },
    Material { name: "Vinyl", force: 10, speed: 5, passes: 1, blade_depth: 0.1 },
    Material { name: "Print Paper", force: 15, speed: 10, passes: 1, blade_depth: 0.2 },
    Material { name: "Adhesive-Backed Cardstock", force: 29, speed: 3, passes: 1, blade_depth: 0.4 },
    Material { name: "Adhesive Magnet Paper", force: 33, speed: 1, passes: 2, blade_depth: 0.6 },
    Material { name: "Chipboard", force: 33, speed: 1, passes: 2, blade_depth: 0.5 },
    Material { name: "Clear Sticker Paper", force: 33, speed: 4, passes: 2, blade_depth: 0.3 },
    Material { name: "Coverstock", force: 33, speed: 1, passes: 2, blade_depth: 0.7 },
    Material { name: "Glitter Cardstock", force: 33, speed: 3, passes: 2, blade_depth: 0.6 },
    Material { name: "Kraft Paper", force: 20, speed: 3, passes: 1, blade_depth: 0.3 },
    Material { name: "Printable Sticker Paper", force: 14, speed: 8, passes: 1, blade_depth: 0.2 },
    Material { name: "Stamp Material", force: 18, speed: 1, passes: 1, blade_depth: 0.9 },
];

pub struct MockSendPanel {
    mode: Mode,
    material: usize,
    condition: Material,
    action: CutAction,
    enabled: HashMap<(Mode, String), bool>,
}

impl Default for MockSendPanel {
    fn default() -> Self {
        Self { mode: Mode::Simple, material: 0, condition: MATERIALS[0], action: CutAction::Cut, enabled: HashMap::new() }
    }
}

fn groups(editor: &Editor, mode: Mode) -> BTreeMap<String, (Option<Rgba>, usize)> {
    let mut groups = BTreeMap::new();
    for o in editor.doc.visible_objects().filter(|o| !matches!(o.kind, ObjectKind::Image(_))) {
        let color = if mode == Mode::Fill { o.style.fill } else { Some(o.style.stroke) };
        let label = color.map_or_else(|| "No fill".into(), |c| format!("{} / α{}", c.to_hex(), c.a));
        let entry = groups.entry(label).or_insert((color, 0));
        entry.1 += 1;
    }
    groups
}

impl MockSendPanel {
    pub fn ui(&mut self, ui: &mut Ui, editor: &Editor) {
        ui.weak("Mocked Send: no cutter connection, scanning or sending.");
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.mode, Mode::Simple, "Simple");
            ui.selectable_value(&mut self.mode, Mode::Line, "Line");
            ui.selectable_value(&mut self.mode, Mode::Fill, "Fill");
        });
        ui.separator();
        if self.mode == Mode::Simple {
            ui.label("Action by: Simple");
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.action, CutAction::NoCut, "No Cut");
                ui.radio_value(&mut self.action, CutAction::Cut, "Cut");
                ui.radio_value(&mut self.action, CutAction::CutEdge, "Cut Edge");
            });
        } else {
            ui.label(if self.mode == Mode::Line { "Action by: Line Color" } else { "Action by: Fill Color" });
            let groups = groups(editor, self.mode);
            self.enabled.retain(|(mode, key), _| *mode != self.mode || groups.contains_key(key));
            if groups.is_empty() {
                ui.weak("No visible vector objects.");
            }
            for (key, (color, count)) in groups {
                ui.push_id(&key, |ui| {
                    ui.horizontal(|ui| {
                        let enabled = self.enabled.entry((self.mode, key.clone())).or_insert(true);
                        ui.checkbox(enabled, "");
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                        if let Some(c) = color {
                            ui.painter().rect_filled(rect, 2.0, crate::canvas::color32(c));
                        }
                        ui.label(format!("{key} ({count})"));
                    });
                });
            }
        }
        ui.separator();
        ui.label("Material (Cameo reference)");
        let old = self.material;
        egui::ComboBox::from_id_salt("mock-material").selected_text(MATERIALS[self.material].name).show_ui(ui, |ui| {
            for (i, material) in MATERIALS.iter().enumerate() {
                ui.selectable_value(&mut self.material, i, material.name);
            }
        });
        if self.material != old {
            self.condition = MATERIALS[self.material];
        }
        egui::Grid::new("mock-condition").num_columns(2).show(ui, |ui| {
            ui.label("Force");
            ui.add(egui::Slider::new(&mut self.condition.force, 2..=33));
            ui.end_row();
            ui.label("Speed");
            ui.add(egui::Slider::new(&mut self.condition.speed, 1..=10));
            ui.end_row();
            ui.label("Passes");
            ui.add(egui::Slider::new(&mut self.condition.passes, 1..=2));
            ui.end_row();
            ui.label("Blade Depth");
            ui.add(egui::Slider::new(&mut self.condition.blade_depth, 0.1..=0.9).step_by(0.1).fixed_decimals(1));
            ui.end_row();
        });
        ui.weak("Adjustable blade depth; bounds from Resources. These settings are session-only.");
        ui.add_enabled(false, egui::Button::new("Send (mocked)"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ss_doc::{Document, Object};
    use ss_geom::BezPath;

    #[test]
    fn reference_materials_fit_resource_bounds() {
        for m in MATERIALS {
            assert!((2..=33).contains(&m.force));
            assert!((1..=10).contains(&m.speed));
            assert!((1..=2).contains(&m.passes));
            assert!((0.1..=0.9).contains(&m.blade_depth));
        }
        let c = MockSendPanel::default().condition;
        assert_eq!((c.force, c.speed, c.passes, c.blade_depth), (20, 4, 1, 0.3));
    }

    #[test]
    fn grouping_uses_line_or_fill_without_mutating_document() {
        let mut editor = Editor::new(Document::default());
        let layer = editor.doc.default_layer();
        for fill in [None, Some(Rgba::rgb(0, 0, 255))] {
            let mut o = Object::path("Shape", layer, BezPath::new());
            o.style.fill = fill;
            editor.add(o);
        }
        let before = editor.doc.clone();
        assert_eq!(groups(&editor, Mode::Line).len(), 1);
        assert_eq!(groups(&editor, Mode::Fill).len(), 2);
        assert_eq!(editor.doc, before);
    }
}
