//! Send panel: cut settings, device choice, preview and job progress.

use std::time::Duration;

use egui::{DragValue, Ui};
use ss_device::{FileTransport, JobEvent, JobHandle, RunOptions, UsbDeviceInfo, UsbTransport, VirtualCutter, VirtualHandle, spawn_cut_job};
use ss_doc::{CutAction, Editor};
use ss_geom::{Point, Vec2};
use ss_gpgl::{EndAction, Job, JobSettings, OffMedia, Perforation, Registration, Tool, ToolSettings};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Virtual,
    File,
    Usb(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lines {
    Cut,
    Sketch,
    Perforate,
}

impl Lines {
    fn actions(self) -> &'static [CutAction] {
        match self {
            Lines::Cut => &[CutAction::Cut, CutAction::CutEdge],
            Lines::Sketch => &[CutAction::Sketch],
            Lines::Perforate => &[CutAction::Perforate],
        }
    }

    fn label(self) -> &'static str {
        match self {
            Lines::Cut => "Cut lines",
            Lines::Sketch => "Sketch lines (pen)",
            Lines::Perforate => "Perforation lines",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    Home,
    FeedOut,
    Stay,
}

struct Running {
    handle: JobHandle,
    virtual_cutter: Option<VirtualHandle>,
    /// Page position of the cutter origin: the top-left mark when aligning to registration marks.
    origin: Vec2,
    progress: f32,
    state: String,
}

/// Moves simulated tool paths from cutter coordinates to page coordinates.
fn to_page(strokes: Vec<Vec<Point>>, origin: Vec2) -> Vec<Vec<Point>> {
    if origin == Vec2::ZERO {
        return strokes;
    }
    strokes.into_iter().map(|s| s.into_iter().map(|p| p + origin).collect()).collect()
}

pub struct SendPanel {
    lines: Lines,
    target: Target,
    devices: Vec<UsbDeviceInfo>,
    scanned: bool,
    force: u32,
    speed: u32,
    passes: u32,
    blade_offset: u32,
    overcut_mm: f64,
    perforation: Perforation,
    track_enhancing: bool,
    end: End,
    off_media: OffMedia,
    optimize_order: bool,
    use_registration: bool,
    auto_search: bool,
    running: Option<Running>,
    last_summary: Option<String>,
    /// Overlay to hand to the canvas on the next poll.
    pending_overlay: Option<Vec<Vec<Point>>>,
}

impl Default for SendPanel {
    fn default() -> Self {
        let t = ToolSettings::blade();
        Self {
            lines: Lines::Cut,
            target: Target::Virtual,
            devices: Vec::new(),
            scanned: false,
            force: t.force,
            speed: t.speed,
            passes: t.passes,
            blade_offset: t.blade_offset,
            overcut_mm: t.overcut_mm,
            perforation: Perforation { cut_mm: 2.0, gap_mm: 1.0 },
            track_enhancing: false,
            end: End::Home,
            off_media: OffMedia::Error,
            optimize_order: true,
            use_registration: true,
            auto_search: true,
            running: None,
            last_summary: None,
            pending_overlay: None,
        }
    }
}

fn summary(job: &Job) -> String {
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

impl SendPanel {
    fn registration(&self, editor: &Editor) -> Option<Registration> {
        let page = &editor.doc.page;
        page.registration.filter(|_| self.use_registration).map(|m| Registration::from_marks(&m, page, self.auto_search))
    }

    fn origin(&self, editor: &Editor) -> Vec2 {
        self.registration(editor).map_or(Vec2::ZERO, |r| Vec2::new(r.origin_mm.0, r.origin_mm.1))
    }

    fn settings(&self, editor: &Editor) -> JobSettings {
        let page = &editor.doc.page;
        let pen = self.lines == Lines::Sketch;
        let tool = ToolSettings {
            tool: if pen { Tool::Pen } else { Tool::Blade },
            force: self.force,
            speed: self.speed,
            passes: self.passes,
            blade_offset: if pen { 0 } else { self.blade_offset },
            overcut_mm: if pen { 0.0 } else { self.overcut_mm },
            tool_holder: None,
            blade_depth: None,
        };
        JobSettings {
            media_mm: (page.width_mm, page.height_mm),
            tool,
            track_enhancing: self.track_enhancing,
            end: match self.end {
                End::Home => EndAction::Home,
                End::FeedOut => EndAction::FeedOut { mm: page.height_mm },
                End::Stay => EndAction::Stay,
            },
            off_media: self.off_media,
            optimize_order: self.optimize_order,
            perforation: (self.lines == Lines::Perforate).then_some(self.perforation),
            registration: self.registration(editor),
            ..JobSettings::default()
        }
    }

    fn build(&self, editor: &Editor) -> Result<Job, String> {
        let paths = ss_gpgl::document_paths(&editor.doc, self.lines.actions());
        if paths.is_empty() {
            return Err(format!("No {} in the design: set objects' cut action in Properties", self.lines.label().to_lowercase()));
        }
        ss_gpgl::encode(&paths, &self.settings(editor)).map_err(|e| e.to_string())
    }

    fn refresh_devices(&mut self, status: &mut String) {
        self.scanned = true;
        match ss_device::enumerate() {
            Ok(d) => {
                self.devices = d;
                if let Target::Usb(i) = self.target
                    && i >= self.devices.len()
                {
                    self.target = Target::Virtual;
                }
                if self.target == Target::Virtual && !self.devices.is_empty() {
                    self.target = Target::Usb(0);
                }
            }
            Err(e) => *status = format!("Could not list USB devices: {e}"),
        }
    }

    fn target_name(&self, t: Target) -> String {
        match t {
            Target::Virtual => "Virtual cutter (preview)".into(),
            Target::File => "Save to file…".into(),
            Target::Usb(i) => self.devices.get(i).map_or("Missing device".into(), |d| format!("{} (bus {} addr {})", d.name(), d.bus, d.address)),
        }
    }

    fn send(&mut self, editor: &Editor, status: &mut String) {
        let job = match self.build(editor) {
            Ok(j) => j,
            Err(e) => {
                *status = e;
                return;
            }
        };
        self.last_summary = Some(summary(&job));
        let origin = self.origin(editor);
        let mut virtual_cutter = None;
        let handle = match self.target {
            Target::Virtual => {
                let (vc, h) = VirtualCutter::new();
                virtual_cutter = Some(h);
                let opts = RunOptions { poll_interval: Duration::from_millis(20), ..RunOptions::default() };
                spawn_cut_job(vc, job, opts)
            }
            Target::File => {
                let Some(mut path) = rfd::FileDialog::new()
                    .set_title("Save cut job")
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
            Target::Usb(i) => {
                let Some(info) = self.devices.get(i) else {
                    *status = "The selected cutter is no longer connected".into();
                    return;
                };
                if let Some(m) = info.model
                    && editor.doc.page.width_mm > m.width_mm + 0.5
                {
                    *status = format!("The page is wider than the {} can cut ({:.0} mm)", m.name, m.width_mm);
                    return;
                }
                match UsbTransport::open(info) {
                    Ok(t) => spawn_cut_job(t, job, RunOptions::default()),
                    Err(e) => {
                        *status = format!("Could not open {}: {e}", info.name());
                        return;
                    }
                }
            }
        };
        *status = format!("Sending to {}", self.target_name(self.target));
        self.running = Some(Running { handle, virtual_cutter, origin, progress: 0.0, state: "Connecting…".into() });
    }

    /// Processes job events. Returns a new tool-path overlay for the canvas when one is ready.
    pub fn poll(&mut self, ctx: &egui::Context, status: &mut String) -> Option<Vec<Vec<Point>>> {
        let mut finished = false;
        if let Some(run) = &mut self.running {
            for ev in run.handle.events.try_iter() {
                match &ev {
                    JobEvent::Connected { device, firmware } => {
                        run.state = match firmware {
                            Some(f) => format!("Connected to {device} (firmware {f})"),
                            None => format!("Connected to {device}"),
                        };
                    }
                    JobEvent::WaitingForMedia => run.state = "Load the mat or media into the cutter".into(),
                    JobEvent::Sending { sent, total } => {
                        run.progress = if *total == 0 { 1.0 } else { *sent as f32 / *total as f32 };
                        run.state = format!("Sending {sent} / {total} bytes");
                    }
                    JobEvent::Registering => run.state = "Looking for the registration marks…".into(),
                    JobEvent::Registered => run.state = "Registration marks found".into(),
                    JobEvent::Finishing => run.state = "Cutting… waiting for the cutter to finish".into(),
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
        if !self.scanned {
            self.refresh_devices(status);
        }
        let busy = self.running.is_some();
        ui.add_enabled_ui(!busy, |ui| {
            ui.heading("Cutter");
            ui.horizontal(|ui| {
                let choices: Vec<(Target, String)> = (0..self.devices.len())
                    .map(Target::Usb)
                    .chain([Target::Virtual, Target::File])
                    .map(|t| (t, self.target_name(t)))
                    .collect();
                egui::ComboBox::from_id_salt("send-target").selected_text(self.target_name(self.target)).width(210.0).show_ui(ui, |ui| {
                    for (t, name) in choices {
                        ui.selectable_value(&mut self.target, t, name);
                    }
                });
                if ui.button("Refresh").on_hover_text("Look for connected cutters").clicked() {
                    self.refresh_devices(status);
                }
            });
            if self.devices.is_empty() {
                ui.weak("No cutter found. Check the USB cable, power, and the udev rule.");
            }

            ui.separator();
            ui.heading("Lines");
            for l in [Lines::Cut, Lines::Sketch, Lines::Perforate] {
                let n = editor.doc.visible_objects().filter(|o| l.actions().contains(&o.cut)).count();
                ui.radio_value(&mut self.lines, l, format!("{} ({n} objects)", l.label()));
            }

            ui.separator();
            ui.heading("Tool");
            egui::Grid::new("tool-settings").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                ui.label("Force");
                ui.add(egui::Slider::new(&mut self.force, 1..=33));
                ui.end_row();
                ui.label("Speed");
                ui.add(egui::Slider::new(&mut self.speed, 1..=10));
                ui.end_row();
                ui.label("Passes");
                ui.add(egui::Slider::new(&mut self.passes, 1..=10));
                ui.end_row();
                if self.lines != Lines::Sketch {
                    ui.label("Blade offset");
                    ui.add(DragValue::new(&mut self.blade_offset).range(0..=100)).on_hover_text("Cutter units (1/20 mm); 18 suits the standard blade");
                    ui.end_row();
                    ui.label("Overcut");
                    ui.add(DragValue::new(&mut self.overcut_mm).speed(0.05).range(0.0..=5.0).suffix(" mm"));
                    ui.end_row();
                }
                if self.lines == Lines::Perforate {
                    ui.label("Dash");
                    ui.add(DragValue::new(&mut self.perforation.cut_mm).speed(0.05).range(0.1..=50.0).suffix(" mm"));
                    ui.end_row();
                    ui.label("Gap");
                    ui.add(DragValue::new(&mut self.perforation.gap_mm).speed(0.05).range(0.1..=50.0).suffix(" mm"));
                    ui.end_row();
                }
            });

            ui.separator();
            ui.heading("Options");
            ui.checkbox(&mut self.track_enhancing, "Track enhancing");
            ui.checkbox(&mut self.optimize_order, "Optimise cut order (inner shapes first)");
            ui.horizontal(|ui| {
                ui.label("Outside the page");
                egui::ComboBox::from_id_salt("off-media")
                    .selected_text(match self.off_media {
                        OffMedia::Error => "Stop with an error",
                        OffMedia::SkipOutside => "Skip shapes outside",
                        OffMedia::Clip => "Clip at the edge",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.off_media, OffMedia::Error, "Stop with an error");
                        ui.selectable_value(&mut self.off_media, OffMedia::SkipOutside, "Skip shapes outside");
                        ui.selectable_value(&mut self.off_media, OffMedia::Clip, "Clip at the edge");
                    });
            });
            ui.separator();
            ui.heading("Print & Cut");
            if editor.doc.page.registration.is_some() {
                ui.checkbox(&mut self.use_registration, "Align to the registration marks");
                ui.add_enabled_ui(self.use_registration, |ui| {
                    ui.checkbox(&mut self.auto_search, "Find the marks automatically")
                        .on_hover_text("Otherwise move the tool over the top-left mark with the cutter's arrow keys first");
                });
                ui.weak("Experimental: the mark search has not been tested on a real cutter yet.");
            } else {
                ui.weak("Turn on registration marks in the Page tab to cut around a printed design.");
            }
            ui.horizontal(|ui| {
                ui.label("When done");
                ui.radio_value(&mut self.end, End::Home, "Return home");
                ui.radio_value(&mut self.end, End::FeedOut, "Feed out");
                ui.radio_value(&mut self.end, End::Stay, "Stay");
            });

            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Preview path").on_hover_text("Show the planned tool path on the canvas").clicked() {
                    match self.build(editor) {
                        Ok(job) => {
                            self.last_summary = Some(summary(&job));
                            self.pending_overlay = Some(to_page(ss_gpgl::simulate(&job.bytes).strokes, self.origin(editor)));
                        }
                        Err(e) => *status = e,
                    }
                }
                if ui.button("Clear preview").clicked() {
                    self.pending_overlay = Some(Vec::new());
                }
            });
            let label = match self.target {
                Target::Usb(_) => "Send to cutter",
                Target::Virtual => "Send to virtual cutter",
                Target::File => "Save job…",
            };
            if ui.add_sized([ui.available_width(), 32.0], egui::Button::new(egui::RichText::new(label).strong())).clicked() {
                self.send(editor, status);
            }
        });

        if let Some(run) = &self.running {
            ui.add_space(6.0);
            ui.add(egui::ProgressBar::new(run.progress).show_percentage());
            ui.label(&run.state);
            if ui.button("Cancel").clicked() {
                run.handle.cancel();
            }
        }
        if let Some(s) = &self.last_summary {
            ui.add_space(4.0);
            ui.weak(s);
        }
    }
}

