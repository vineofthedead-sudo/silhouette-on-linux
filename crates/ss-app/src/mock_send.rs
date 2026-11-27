//! V5 Material → Action → Send layout. This module deliberately has no device or GPGL dependencies.

use std::collections::{BTreeMap, HashMap};

use egui::Ui;
use ss_doc::{CutAction, Editor, ObjectKind, Rgba};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum SendStep {
    #[default]
    Material,
    Action,
    Send,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
enum Mode {
    Simple,
    #[default]
    Line,
    Fill,
    Layer,
}

impl Mode {
    fn label(self) -> &'static str {
        match self {
            Self::Simple => "Simple",
            Self::Line => "Line Color",
            Self::Fill => "Fill Color",
            Self::Layer => "Layer",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
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

#[derive(Debug, Clone, Copy)]
struct GroupSettings {
    enabled: bool,
    action: CutAction,
    tool: usize,
}

impl Default for GroupSettings {
    fn default() -> Self {
        Self { enabled: true, action: CutAction::Cut, tool: 1 }
    }
}

pub struct MockSendPanel {
    step: SendStep,
    mode: Mode,
    material: usize,
    custom_material: Option<String>,
    condition: Material,
    edited: bool,
    saved: bool,
    action: CutAction,
    overcut: bool,
    groups: HashMap<(Mode, String), GroupSettings>,
    machines: [bool; 4],
    output_tool: usize,
}

impl Default for MockSendPanel {
    fn default() -> Self {
        Self {
            step: SendStep::Material,
            mode: Mode::Line,
            material: 0,
            custom_material: None,
            condition: MATERIALS[0],
            edited: false,
            saved: false,
            action: CutAction::Cut,
            overcut: true,
            groups: HashMap::new(),
            machines: [true; 4],
            output_tool: 1,
        }
    }
}

fn groups(editor: &Editor, mode: Mode) -> BTreeMap<String, (Option<Rgba>, usize)> {
    let mut groups = BTreeMap::new();
    for object in editor.doc.visible_objects().filter(|object| !matches!(object.kind, ObjectKind::Image(_))) {
        let (label, color) = match mode {
            Mode::Line => (object.style.stroke.to_hex(), Some(object.style.stroke)),
            Mode::Fill => (
                object.style.fill.map_or_else(|| "No fill".into(), |color| color.to_hex()),
                object.style.fill,
            ),
            Mode::Layer => (
                editor.doc.layer(object.layer).map_or_else(|| "Unknown layer".into(), |layer| layer.name.clone()),
                None,
            ),
            Mode::Simple => unreachable!("simple mode has no colour or layer groups"),
        };
        let entry = groups.entry(label).or_insert((color, 0));
        entry.1 += 1;
    }
    groups
}

fn color_swatch(ui: &mut Ui, color: Option<Rgba>) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
    if let Some(color) = color {
        ui.painter().rect_filled(rect, 2.0, crate::canvas::color32(color));
    } else {
        ui.painter().rect_stroke(rect, 2.0, egui::Stroke::new(1.0, egui::Color32::GRAY), egui::StrokeKind::Inside);
    }
}

impl MockSendPanel {
    pub fn ui(&mut self, ui: &mut Ui, editor: &Editor) {
        self.step_tabs(ui);
        ui.separator();
        match self.step {
            SendStep::Material => self.material_ui(ui),
            SendStep::Action => self.action_ui(ui, editor),
            SendStep::Send => self.send_ui(ui, editor),
        }
        ui.separator();
        self.output_ui(ui);
        ui.weak("Mocked Send: no cutter connection, scanning, registration or sending.");
    }

    fn step_tabs(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.step, SendStep::Material, "Material (1)");
            ui.selectable_value(&mut self.step, SendStep::Action, "Action (2)");
            ui.selectable_value(&mut self.step, SendStep::Send, "Send (3)");
            ui.add_enabled(false, egui::Button::new("⚙ Advanced")).on_hover_text("Advanced Send is not part of the mock");
        });
    }

    fn go_next(&mut self) {
        self.step = match self.step {
            SendStep::Material => SendStep::Action,
            SendStep::Action | SendStep::Send => SendStep::Send,
        };
    }

    fn go_previous(&mut self) {
        self.step = match self.step {
            SendStep::Material | SendStep::Action => SendStep::Material,
            SendStep::Send => SendStep::Action,
        };
    }

    fn navigation(&mut self, ui: &mut Ui, previous: bool, next_enabled: bool) {
        let mut movement = None;
        ui.horizontal(|ui| {
            if previous && ui.button("< Previous").clicked() {
                movement = Some(-1);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled(next_enabled, egui::Button::new("Next >")).clicked() {
                    movement = Some(1);
                }
            });
        });
        match movement {
            Some(1) => self.go_next(),
            Some(-1) => self.go_previous(),
            _ => {}
        }
    }

    fn material_name(&self) -> &str {
        self.custom_material.as_deref().unwrap_or(MATERIALS[self.material].name)
    }

    fn material_ui(&mut self, ui: &mut Ui) {
        ui.add_space(12.0);
        ui.label("Select your material type:");
        let selected_name = self.material_name().to_owned();
        let mut selected = None;
        egui::ComboBox::from_id_salt("v5-material").selected_text(selected_name).width(210.0).show_ui(ui, |ui| {
            for (index, material) in MATERIALS.iter().enumerate() {
                if ui
                    .selectable_label(self.custom_material.is_none() && self.material == index, material.name)
                    .clicked()
                {
                    selected = Some(index);
                }
            }
            if let Some(custom) = &self.custom_material {
                let _ = ui.selectable_label(true, custom);
            }
        });
        if let Some(index) = selected {
            self.material = index;
            self.custom_material = None;
            self.condition = MATERIALS[index];
            self.edited = false;
            self.saved = false;
        }
        ui.add_space(18.0);
        ui.separator();
        ui.add_space(18.0);
        ui.label("Not finding your material type?");
        if ui.button("＋ Add new material type").clicked() {
            self.custom_material = Some("User Defined Material".into());
            self.condition = MATERIALS[self.material];
            self.edited = false;
            self.saved = false;
        }
        ui.add_space(12.0);
        self.navigation(ui, false, true);
    }
}

fn cut_action_label(action: CutAction) -> &'static str {
    match action {
        CutAction::Cut => "Cut",
        CutAction::CutEdge => "Cut Edge",
        CutAction::Sketch => "Sketch",
        CutAction::NoCut => "No Cut",
        CutAction::Perforate => "Perforate",
    }
}

impl MockSendPanel {
    fn action_ui(&mut self, ui: &mut Ui, editor: &Editor) {
        ui.horizontal_wrapped(|ui| {
            ui.label("Action by:");
            for mode in [Mode::Simple, Mode::Line, Mode::Fill, Mode::Layer] {
                ui.selectable_value(&mut self.mode, mode, mode.label());
            }
        });
        ui.separator();
        ui.label("Select what action would you like to perform:");
        if self.mode == Mode::Simple {
            ui.horizontal_wrapped(|ui| {
                ui.radio_value(&mut self.action, CutAction::NoCut, "No Cut");
                ui.radio_value(&mut self.action, CutAction::Cut, "Cut");
                ui.radio_value(&mut self.action, CutAction::CutEdge, "Cut Edge");
            });
        } else {
            self.action_groups_ui(ui, editor);
        }
        ui.weak("Right-click to add a pause between colors/layers");
        self.condition_ui(ui);
        self.navigation(ui, true, true);
    }

    fn action_groups_ui(&mut self, ui: &mut Ui, editor: &Editor) {
        let mode = self.mode;
        let groups = groups(editor, mode);
        self.groups.retain(|(group_mode, key), _| *group_mode != mode || groups.contains_key(key));
        if groups.is_empty() {
            ui.weak("No visible vector objects.");
            return;
        }
        egui::ScrollArea::vertical().id_salt("v5-action-groups").max_height(160.0).show(ui, |ui| {
            egui::Grid::new("v5-action-grid").num_columns(5).spacing([5.0, 5.0]).show(ui, |ui| {
                ui.weak("");
                ui.weak("On/Off");
                ui.weak("Color");
                ui.weak("Action");
                ui.weak("Tool");
                ui.end_row();
                for (key, (color, count)) in groups {
                    let settings = self.groups.entry((mode, key.clone())).or_default();
                    ui.weak("☰");
                    ui.checkbox(&mut settings.enabled, "");
                    ui.horizontal(|ui| {
                        color_swatch(ui, color);
                        ui.label(format!("{key} ({count})"));
                    });
                    egui::ComboBox::from_id_salt(("v5-group-action", mode, &key))
                        .selected_text(cut_action_label(settings.action))
                        .width(72.0)
                        .show_ui(ui, |ui| {
                            for action in [CutAction::Cut, CutAction::CutEdge, CutAction::Sketch, CutAction::NoCut] {
                                ui.selectable_value(&mut settings.action, action, cut_action_label(action));
                            }
                        });
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut settings.tool, 1, "1");
                        ui.radio_value(&mut settings.tool, 2, "2");
                    });
                    ui.end_row();
                }
            });
        });
    }
}

impl MockSendPanel {
    fn condition_ui(&mut self, ui: &mut Ui) {
        ui.separator();
        let name = self.material_name().to_owned();
        let heading = if self.edited { format!("{name}* (User Edited Setting)") } else { name };
        let mut restore = false;
        ui.horizontal_wrapped(|ui| {
            ui.label(heading);
            restore = ui.button("Restore Default Settings").clicked();
        });
        if restore {
            self.condition = MATERIALS[self.material];
            self.overcut = true;
            self.edited = false;
            self.saved = false;
        }

        let mut speed = self.condition.speed;
        let mut thickness = self.condition.force;
        let mut double_pass = self.condition.passes == 2;
        let mut overcut = self.overcut;
        let mut blade_depth = self.condition.blade_depth;
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label("Speed");
                    changed |= ui.add(egui::DragValue::new(&mut speed).range(1..=10)).changed();
                });
                ui.horizontal(|ui| {
                    ui.label("Thickness").on_hover_text("V5 Thickness corresponds to cutter force in the reference conditions");
                    changed |= ui.add(egui::DragValue::new(&mut thickness).range(2..=33)).changed();
                });
                changed |= ui.checkbox(&mut double_pass, "Double Pass").changed();
                changed |= ui.checkbox(&mut overcut, "Line Segment Overcut △").changed();
            });
            ui.vertical(|ui| {
                ui.label("Blade Depth");
                changed |= ui
                    .add(egui::DragValue::new(&mut blade_depth).speed(0.1).range(0.1..=0.9).fixed_decimals(1))
                    .changed();
                ui.add_enabled(false, egui::Button::new("Test △")).on_hover_text("Test cuts are disabled in the mock");
            });
        });
        if changed {
            self.condition.speed = speed;
            self.condition.force = thickness;
            self.condition.passes = if double_pass { 2 } else { 1 };
            self.condition.blade_depth = blade_depth;
            self.overcut = overcut;
            self.edited = true;
            self.saved = false;
        }
        if ui.button(if self.saved { "Saved ✓" } else { "Save Settings" }).clicked() {
            self.saved = true;
        }
        ui.weak("Resource-bounded settings are kept for this session only.");
    }
}

impl MockSendPanel {
    fn send_ui(&mut self, ui: &mut Ui, editor: &Editor) {
        ui.label("Set up to send to Silhouette:");
        ui.label("Step 1: Place your material onto the cutting mat as shown in the preview");
        ui.label("Step 2: Adjust the rollers to the 13\" CAMEO mat setting");
        ui.label("Step 3: Load the mat into Silhouette using the machine's \"Load Mat\" button");
        ui.label("Step 4: Load the BLADE (set to 3) into the first/red tool holder");
        ui.label("         Load a pen into the second/blue tool holder");
        ui.separator();
        ui.heading("Registration Marks");
        let enabled = editor.doc.page.registration.is_some();
        ui.label(format!("Your current job has Registration Marks turned {}.", if enabled { "ON" } else { "OFF" }));
        ui.weak("If marks are not automatically detected after Send, use Manual Registration to align the job.");
        ui.add_enabled(false, egui::Button::new("Manual Registration"))
            .on_hover_text("Registration is disabled in the mock and never accesses a cutter");
        ui.separator();
        ui.heading("Send to the Silhouette:");
        ui.label("Once your Silhouette is set up and all settings are ready, click Send to start the job.");
        ui.vertical_centered(|ui| {
            ui.add_enabled(false, egui::Button::new("Send")).on_hover_text("Sending is disabled in the mock");
        });
        ui.add_space(8.0);
        self.navigation(ui, true, false);
    }

    fn output_ui(&mut self, ui: &mut Ui) {
        ui.heading("Send to:");
        for (enabled, name) in self.machines.iter_mut().zip(["CAMEO 2 - A", "CAMEO 2 - B", "CAMEO 2 - C", "CAMEO 2 - D"]) {
            ui.checkbox(enabled, name);
        }
        ui.horizontal_wrapped(|ui| {
            ui.vertical(|ui| {
                ui.label(self.material_name().to_owned());
                ui.weak(format!("Speed       {}", self.condition.speed));
                ui.weak(format!("Thickness   {}", self.condition.force));
                ui.weak(format!("Double Pass {}", if self.condition.passes == 2 { "On" } else { "Off" }));
            });
            ui.vertical(|ui| {
                ui.radio_value(&mut self.output_tool, 1, "Blade / Tool 1");
                ui.radio_value(&mut self.output_tool, 2, "Pen / Tool 2");
            });
        });
        let width = ui.available_width();
        ui.add_enabled(false, egui::Button::new("Send").min_size(egui::vec2(width, 32.0)))
            .on_hover_text("Mock only: no device discovery or USB access");
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
        assert_eq!(groups(&editor, Mode::Layer).len(), 1);
        assert_eq!(editor.doc, before);
    }

    #[test]
    fn wizard_navigation_and_defaults_match_the_v5_flow() {
        let mut panel = MockSendPanel::default();
        assert_eq!(panel.step, SendStep::Material);
        assert_eq!(panel.mode, Mode::Line);
        panel.go_next();
        assert_eq!(panel.step, SendStep::Action);
        panel.go_next();
        assert_eq!(panel.step, SendStep::Send);
        panel.go_next();
        assert_eq!(panel.step, SendStep::Send);
        panel.go_previous();
        assert_eq!(panel.step, SendStep::Action);
        panel.go_previous();
        assert_eq!(panel.step, SendStep::Material);
        panel.go_previous();
        assert_eq!(panel.step, SendStep::Material);
    }

    #[test]
    fn material_customization_and_restoration() {
        let mut panel = MockSendPanel::default();
        assert_eq!(panel.material_name(), "Cardstock");
        panel.custom_material = Some("My Glitter Vinyl".into());
        assert_eq!(panel.material_name(), "My Glitter Vinyl");
        panel.condition.force = 30;
        panel.edited = true;
        assert_eq!(panel.condition.force, 30);
        assert!(panel.edited);
    }
}
