//! V5 Material → Action → Send wizard wired to `ss_gpgl` job generation, Virtual Cutter
//! simulation, Tool Path Preview, and `.gpgl` file export. Live USB hardware access remains
//! disabled behind a safe mock toggle.

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use egui::Ui;
use ss_device::{FileTransport, JobEvent, JobHandle, RunOptions, VirtualCutter, VirtualHandle, spawn_cut_job};
use ss_doc::{CutAction, Editor, Object, ObjectKind, Rgba};
use ss_geom::{BezPath, Point, Vec2};
use ss_gpgl::{EndAction, Job, JobSettings, JobStage, OffMedia, Registration, Tool, ToolSettings};

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

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum OutputTarget {
    #[default]
    Virtual,
    File,
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
    pause_after: bool,
}

impl Default for GroupSettings {
    fn default() -> Self {
        Self { enabled: true, action: CutAction::Cut, tool: 1, pause_after: false }
    }
}

struct Running {
    handle: JobHandle,
    virtual_cutter: Option<VirtualHandle>,
    origin: Vec2,
    progress: f32,
    state: String,
}

fn to_page(strokes: Vec<Vec<Point>>, origin: Vec2) -> Vec<Vec<Point>> {
    if origin == Vec2::ZERO {
        return strokes;
    }
    strokes.into_iter().map(|s| s.into_iter().map(|p| p + origin).collect()).collect()
}

fn job_summary(job: &Job) -> String {
    let s = &job.stats;
    format!(
        "{} strokes, {:.0} mm cut, {:.0} mm travel, {} commands ({} bytes)",
        s.strokes,
        s.cut_mm,
        s.travel_mm,
        s.commands,
        job.bytes.len()
    )
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
    group_order: HashMap<Mode, Vec<String>>,
    machines: [bool; 4],
    output_tool: usize,
    target: OutputTarget,
    use_registration: bool,
    auto_search: bool,
    show_preview: bool,
    running: Option<Running>,
    last_summary: Option<String>,
    pending_overlay: Option<Vec<Vec<Point>>>,
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
            group_order: HashMap::new(),
            machines: [true; 4],
            output_tool: 1,
            target: OutputTarget::Virtual,
            use_registration: true,
            auto_search: true,
            show_preview: false,
            running: None,
            last_summary: None,
            pending_overlay: None,
        }
    }
}

fn object_group_key(editor: &Editor, object: &Object, mode: Mode) -> (String, Option<Rgba>) {
    match mode {
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
    }
}

fn groups(editor: &Editor, mode: Mode) -> BTreeMap<String, (Option<Rgba>, usize)> {
    let mut groups = BTreeMap::new();
    for object in editor.doc.visible_objects().filter(|object| !matches!(object.kind, ObjectKind::Image(_))) {
        let (label, color) = object_group_key(editor, object, mode);
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
    fn registration(&self, editor: &Editor) -> Option<Registration> {
        let page = &editor.doc.page;
        page.registration.filter(|_| self.use_registration).map(|m| Registration::from_marks(&m, page, self.auto_search))
    }

    fn origin(&self, editor: &Editor) -> Vec2 {
        self.registration(editor).map_or(Vec2::ZERO, |r| Vec2::new(r.origin_mm.0, r.origin_mm.1))
    }

    fn tool_settings(&self, action: CutAction, holder: usize) -> ToolSettings {
        let pen = action == CutAction::Sketch;
        let depth = ((self.condition.blade_depth * 10.0).round() as u32).clamp(1, 9);
        ToolSettings {
            tool: if pen { Tool::Pen } else { Tool::Blade },
            force: self.condition.force.clamp(1, 33),
            speed: self.condition.speed.clamp(1, 10),
            passes: self.condition.passes.max(1),
            blade_offset: if pen { 0 } else { 18 },
            overcut_mm: if !pen && self.overcut { 0.2 } else { 0.0 },
            tool_holder: Some(u8::try_from(holder.clamp(1, 2)).unwrap_or(1)),
            blade_depth: (!pen).then_some(depth),
        }
    }

    pub fn job_settings(&self, editor: &Editor) -> JobSettings {
        let page = &editor.doc.page;
        JobSettings {
            media_mm: (page.width_mm, page.height_mm),
            tool: self.tool_settings(self.action, self.output_tool),
            track_enhancing: false,
            end: EndAction::Home,
            tolerance_mm: editor.tolerance.max(0.005),
            optimize_order: true,
            off_media: OffMedia::Error,
            perforation: None,
            registration: self.registration(editor),
        }
    }

    fn ordered_keys(&self, editor: &Editor, mode: Mode) -> Vec<String> {
        let current = groups(editor, mode);
        let mut order: Vec<String> = self
            .group_order
            .get(&mode)
            .into_iter()
            .flatten()
            .filter(|k| current.contains_key(*k))
            .cloned()
            .collect();
        for key in current.keys() {
            if !order.contains(key) {
                order.push(key.clone());
            }
        }
        order
    }

    pub fn build_stages(&self, editor: &Editor) -> Vec<JobStage> {
        if self.mode == Mode::Simple {
            if self.action == CutAction::NoCut {
                return Vec::new();
            }
            let raw: Vec<BezPath> = editor
                .doc
                .visible_objects()
                .filter(|o| !matches!(o.kind, ObjectKind::Image(_)))
                .flat_map(|o| o.cut_paths())
                .collect();
            let paths = if self.action == CutAction::CutEdge { ss_gpgl::cut_edge_paths(&raw) } else { raw };
            if paths.is_empty() {
                return Vec::new();
            }
            return vec![JobStage {
                paths,
                tool: self.tool_settings(self.action, self.output_tool),
                perforation: None,
                pause_after: false,
            }];
        }

        let keys = self.ordered_keys(editor, self.mode);
        let mut stages = Vec::new();
        for key in keys {
            let settings = self.groups.get(&(self.mode, key.clone())).copied().unwrap_or_default();
            if !settings.enabled || settings.action == CutAction::NoCut {
                continue;
            }
            let raw: Vec<BezPath> = editor
                .doc
                .visible_objects()
                .filter(|o| !matches!(o.kind, ObjectKind::Image(_)))
                .filter(|o| object_group_key(editor, o, self.mode).0 == key)
                .flat_map(|o| o.cut_paths())
                .collect();
            let paths = if settings.action == CutAction::CutEdge { ss_gpgl::cut_edge_paths(&raw) } else { raw };
            if !paths.is_empty() {
                stages.push(JobStage {
                    paths,
                    tool: self.tool_settings(settings.action, settings.tool),
                    perforation: None,
                    pause_after: settings.pause_after,
                });
            }
        }
        stages
    }

    pub fn build_job(&self, editor: &Editor) -> Result<Job, String> {
        let stages = self.build_stages(editor);
        if stages.is_empty() {
            return Err("No active cut or sketch paths in the current Send selection".into());
        }
        ss_gpgl::encode_stages(&stages, &self.job_settings(editor)).map_err(|e| e.to_string())
    }

    fn refresh_preview(&mut self, editor: &Editor, status: &mut String) {
        if !self.show_preview {
            self.pending_overlay = Some(Vec::new());
            return;
        }
        match self.build_job(editor) {
            Ok(job) => {
                self.last_summary = Some(job_summary(&job));
                let sim = ss_gpgl::simulate(&job.bytes);
                self.pending_overlay = Some(to_page(sim.strokes, self.origin(editor)));
            }
            Err(e) => {
                *status = e;
                self.pending_overlay = Some(Vec::new());
            }
        }
    }

    fn send_job(&mut self, editor: &Editor, status: &mut String) {
        let job = match self.build_job(editor) {
            Ok(j) => j,
            Err(e) => {
                *status = e;
                return;
            }
        };
        self.last_summary = Some(job_summary(&job));
        let origin = self.origin(editor);
        let mut virtual_cutter = None;
        let handle = match self.target {
            OutputTarget::Virtual => {
                let (vc, h) = VirtualCutter::new();
                virtual_cutter = Some(h);
                let opts = RunOptions { poll_interval: Duration::from_millis(20), ..RunOptions::default() };
                spawn_cut_job(vc, job, opts)
            }
            OutputTarget::File => {
                let Some(mut path) = rfd::FileDialog::new()
                    .set_title("Export GPGL cut job")
                    .add_filter("GPGL job", &["gpgl"])
                    .set_file_name("job.gpgl")
                    .save_file()
                else {
                    return;
                };
                if path.extension().is_none() {
                    path.set_extension("gpgl");
                }
                match FileTransport::create(&path) {
                    Ok(t) => {
                        let opts = RunOptions { poll_interval: Duration::from_millis(1), ..RunOptions::default() };
                        spawn_cut_job(t, job, opts)
                    }
                    Err(e) => {
                        *status = format!("Could not create {}: {e}", path.display());
                        return;
                    }
                }
            }
        };
        *status = match self.target {
            OutputTarget::Virtual => "Running job on Virtual Cutter…".into(),
            OutputTarget::File => "Writing .gpgl job file…".into(),
        };
        self.running = Some(Running { handle, virtual_cutter, origin, progress: 0.0, state: "Starting…".into() });
    }

    /// Processes background job events. Returns a new tool-path overlay for the canvas when ready.
    pub fn poll(&mut self, ctx: &egui::Context, status: &mut String) -> Option<Vec<Vec<Point>>> {
        let mut finished = false;
        if let Some(run) = &mut self.running {
            for ev in run.handle.events.try_iter() {
                match &ev {
                    JobEvent::Connected { device, firmware } => {
                        run.state = match firmware {
                            Some(f) => format!("Connected to {device} ({f})"),
                            None => format!("Connected to {device}"),
                        };
                    }
                    JobEvent::WaitingForMedia => run.state = "Waiting for media…".into(),
                    JobEvent::Sending { sent, total } => {
                        run.progress = if *total == 0 { 1.0 } else { *sent as f32 / *total as f32 };
                        run.state = format!("Sending {sent} / {total} bytes");
                    }
                    JobEvent::Registering => run.state = "Searching for registration marks…".into(),
                    JobEvent::Registered => run.state = "Registration marks aligned".into(),
                    JobEvent::Finishing => run.state = "Simulating cut…".into(),
                    JobEvent::Finished => *status = "Job finished".into(),
                    JobEvent::Cancelled => *status = "Job cancelled".into(),
                    JobEvent::Failed(e) => *status = format!("Job failed: {e}"),
                }
                if ev.is_terminal() {
                    finished = true;
                    if ev == JobEvent::Finished
                        && let Some(v) = &run.virtual_cutter
                    {
                        self.pending_overlay = Some(to_page(v.simulation().strokes, run.origin));
                    }
                }
            }
            if !finished {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
        if finished {
            self.running = None;
        }
        self.pending_overlay.take()
    }

    pub fn ui(&mut self, ui: &mut Ui, editor: &Editor, status: &mut String) {
        let busy = self.running.is_some();
        ui.add_enabled_ui(!busy, |ui| {
            self.step_tabs(ui);
            ui.separator();
            match self.step {
                SendStep::Material => self.material_ui(ui),
                SendStep::Action => self.action_ui(ui, editor),
                SendStep::Send => self.send_ui(ui, editor, status),
            }
            ui.separator();
            self.output_ui(ui, editor, status);
        });
        if let Some(run) = &self.running {
            ui.separator();
            ui.label(&run.state);
            ui.add(egui::ProgressBar::new(run.progress).show_percentage());
            if ui.button("Cancel").clicked() {
                run.handle.cancel();
            }
        }
        if let Some(sum) = &self.last_summary {
            ui.weak(sum);
        }
    }

    fn step_tabs(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.step, SendStep::Material, "Material (1)");
            ui.selectable_value(&mut self.step, SendStep::Action, "Action (2)");
            ui.selectable_value(&mut self.step, SendStep::Send, "Send (3)");
            ui.add_enabled(false, egui::Button::new("⚙ Advanced")).on_hover_text("Advanced Send options are not yet wired");
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
                ui.radio_value(&mut self.action, CutAction::Sketch, "Sketch");
            });
        } else {
            self.action_groups_ui(ui, editor);
        }
        ui.weak("Right-click a row to add/remove a pause between colors/layers");
        self.condition_ui(ui);
        self.navigation(ui, true, true);
    }

    fn action_groups_ui(&mut self, ui: &mut Ui, editor: &Editor) {
        let mode = self.mode;
        let groups = groups(editor, mode);
        self.groups.retain(|(group_mode, key), _| *group_mode != mode || groups.contains_key(key));
        let order = self.ordered_keys(editor, mode);
        self.group_order.insert(mode, order.clone());

        if order.is_empty() {
            ui.weak("No visible vector objects.");
            return;
        }
        let mut swap: Option<(usize, usize)> = None;
        egui::ScrollArea::vertical().id_salt("v5-action-groups").max_height(160.0).show(ui, |ui| {
            egui::Grid::new("v5-action-grid").num_columns(5).spacing([5.0, 5.0]).show(ui, |ui| {
                ui.weak("Order");
                ui.weak("On");
                ui.weak("Group");
                ui.weak("Action");
                ui.weak("Tool");
                ui.end_row();
                let len = order.len();
                for (idx, key) in order.iter().enumerate() {
                    let Some(&(color, count)) = groups.get(key) else { continue };
                    let settings = self.groups.entry((mode, key.clone())).or_default();
                    ui.horizontal(|ui| {
                        if ui.add_enabled(idx > 0, egui::Button::new("▲").small()).clicked() {
                            swap = Some((idx, idx - 1));
                        }
                        if ui.add_enabled(idx + 1 < len, egui::Button::new("▼").small()).clicked() {
                            swap = Some((idx, idx + 1));
                        }
                    });
                    ui.checkbox(&mut settings.enabled, "");
                    let label = if settings.pause_after {
                        format!("{key} ({count}) ⏸")
                    } else {
                        format!("{key} ({count})")
                    };
                    let row_resp = ui.horizontal(|ui| {
                        color_swatch(ui, color);
                        ui.label(label)
                    });
                    row_resp.inner.context_menu(|ui| {
                        let text = if settings.pause_after { "Remove pause after" } else { "Add pause after" };
                        if ui.button(text).clicked() {
                            settings.pause_after = !settings.pause_after;
                            ui.close();
                        }
                    });
                    egui::ComboBox::from_id_salt(("v5-group-action", mode, key))
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
        if let Some((a, b)) = swap
            && let Some(list) = self.group_order.get_mut(&mode)
        {
            list.swap(a, b);
        }
    }

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
                    ui.label("Thickness").on_hover_text("V5 Thickness corresponds to cutter force (1–33)");
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
    }

    fn send_ui(&mut self, ui: &mut Ui, editor: &Editor, status: &mut String) {
        ui.label("Set up to send to Silhouette:");
        ui.label("Step 1: Place your material onto the cutting mat as shown in the preview");
        ui.label("Step 2: Adjust the rollers to the 13\" CAMEO mat setting");
        ui.label("Step 3: Load the mat into Silhouette using the machine's \"Load Mat\" button");
        ui.label("Step 4: Load the BLADE into Tool 1 (red) or a pen into Tool 2 (blue)");
        ui.separator();
        ui.heading("Registration Marks");
        let enabled = editor.doc.page.registration.is_some();
        ui.label(format!("Page registration marks: {}.", if enabled { "ON" } else { "OFF" }));
        if enabled {
            ui.checkbox(&mut self.use_registration, "Align job to registration marks");
            ui.add_enabled(
                self.use_registration,
                egui::Checkbox::new(&mut self.auto_search, "Automatic mark search (uncheck for manual)"),
            );
        }
        ui.separator();
        ui.heading("Send to the Silhouette:");
        ui.label("Run on the Virtual Cutter or export a .gpgl job file below.");
        ui.vertical_centered(|ui| {
            if ui.button("Send").clicked() {
                self.send_job(editor, status);
            }
        });
        ui.add_space(8.0);
        self.navigation(ui, true, false);
    }

    fn output_ui(&mut self, ui: &mut Ui, editor: &Editor, status: &mut String) {
        ui.heading("Send to:");
        ui.radio_value(&mut self.target, OutputTarget::Virtual, "Virtual Cutter (simulate)");
        ui.radio_value(&mut self.target, OutputTarget::File, "Export .gpgl File…");
        ui.add_enabled(false, egui::RadioButton::new(false, "USB Cutter (disabled — hardware untested)"))
            .on_hover_text("Live USB device scanning and cutting remain disabled until physical hardware testing");

        ui.collapsing("Simulated Machines (Mock)", |ui| {
            for (enabled, name) in self.machines.iter_mut().zip(["CAMEO 2 - A", "CAMEO 2 - B", "CAMEO 2 - C", "CAMEO 2 - D"]) {
                ui.checkbox(enabled, name);
            }
        });

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

        if ui.checkbox(&mut self.show_preview, "Tool Path Preview").changed() {
            self.refresh_preview(editor, status);
        }
        if self.show_preview && ui.button("Refresh Preview").clicked() {
            self.refresh_preview(editor, status);
        }

        let width = ui.available_width();
        let label = match self.target {
            OutputTarget::Virtual => "Run Virtual Cutter",
            OutputTarget::File => "Export .gpgl File…",
        };
        if ui.add_sized([width, 32.0], egui::Button::new(label)).clicked() {
            self.send_job(editor, status);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ss_doc::{Document, Layer, Object};
    use ss_geom::{Rect, Shape};

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

    #[test]
    fn multi_group_line_color_job_encodes_per_group_tools_and_pauses() {
        let mut editor = Editor::new(Document::default());
        let l2 = Layer::new("Sketch");
        let l2_id = l2.id;
        editor.doc.layers.push(l2);
        let l1 = editor.doc.default_layer();

        let mut red = Object::path("Cut", l1, Rect::new(20.0, 20.0, 40.0, 40.0).to_path(0.0));
        red.style.stroke = Rgba::RED;
        editor.add(red);

        let mut blue = Object::path("Sketch", l2_id, Rect::new(50.0, 20.0, 70.0, 40.0).to_path(0.0));
        blue.style.stroke = Rgba::rgb(0, 0, 255);
        editor.add(blue);

        let mut panel = MockSendPanel::default();
        panel.groups.insert(
            (Mode::Line, Rgba::RED.to_hex()),
            GroupSettings { enabled: true, action: CutAction::Cut, tool: 1, pause_after: true },
        );
        panel.groups.insert(
            (Mode::Line, Rgba::rgb(0, 0, 255).to_hex()),
            GroupSettings { enabled: true, action: CutAction::Sketch, tool: 2, pause_after: false },
        );
        panel.group_order.insert(Mode::Line, vec![Rgba::RED.to_hex(), Rgba::rgb(0, 0, 255).to_hex()]);

        let job = panel.build_job(&editor).unwrap();
        let sim = ss_gpgl::simulate(&job.bytes);
        assert!(sim.errors.is_empty());
        assert_eq!(sim.strokes.len(), 2);
        assert_eq!(sim.tool_holder, Some(2));
        assert_eq!(sim.blade_depth, Some(3));
        assert_eq!(sim.homes, 2);
    }
}
