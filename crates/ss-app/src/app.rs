//! Application state, menus, shortcuts and file handling.

use std::path::{Path, PathBuf};

use egui::{Key, KeyboardShortcut, Modifiers, ViewportCommand};
use ss_config::{AppPaths, Settings, Theme, Units};
use ss_doc::{Align, CutAction, Document, Editor, LayerId, Object, Page};
use ss_geom::Vec2;
use ss_geom::offset::Join;

use crate::canvas::{Canvas, Tool};
use crate::panels;
use crate::send::SendPanel;

const OPEN_EXTENSIONS: &[&str] = &["studio3", "studio", "svg", "svgz", "dxf", "ssrs"];
const PASTE_OFFSET_MM: f64 = 5.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    New,
    Open,
    OpenRecent(usize),
    Import,
    Save,
    SaveAs,
    ExportSvg,
    ExportDxf,
    Quit,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    Duplicate,
    Delete,
    SelectAll,
    SelectNone,
    Group,
    Ungroup,
    MakeCompound,
    ReleaseCompound,
    Weld,
    Subtract,
    Intersect,
    OffsetDialog,
    FlipH,
    FlipV,
    Rotate(f64),
    Align(Align),
    Distribute(bool),
    Reorder { front: bool, all: bool },
    SetCut(CutAction),
    ZoomPage,
    ZoomSelection,
    ZoomActual,
    ZoomIn,
    ZoomOut,
    Nudge(f64, f64),
    SetTool(Tool),
    About,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Properties,
    Layers,
    Page,
    Send,
}

struct OffsetDialog {
    distance: f64,
    join: Join,
}

pub struct StudioApp {
    paths: Option<AppPaths>,
    settings: Settings,
    editor: Editor,
    file: Option<PathBuf>,
    canvas: Canvas,
    send: SendPanel,
    active_layer: LayerId,
    tab: Tab,
    status: String,
    clipboard: Vec<Object>,
    /// An action waiting for the "save changes?" answer.
    confirm: Option<Action>,
    force_discard: bool,
    allow_close: bool,
    offset: Option<OffsetDialog>,
    about: bool,
    last_title: String,
}

fn shortcut(m: Modifiers, k: Key) -> KeyboardShortcut {
    KeyboardShortcut::new(m, k)
}

fn shortcuts() -> Vec<(KeyboardShortcut, Action)> {
    let c = Modifiers::COMMAND;
    let cs = Modifiers::COMMAND | Modifiers::SHIFT;
    vec![
        (shortcut(cs, Key::Z), Action::Redo),
        (shortcut(c, Key::Z), Action::Undo),
        (shortcut(c, Key::Y), Action::Redo),
        (shortcut(cs, Key::S), Action::SaveAs),
        (shortcut(c, Key::S), Action::Save),
        (shortcut(c, Key::O), Action::Open),
        (shortcut(c, Key::N), Action::New),
        (shortcut(c, Key::I), Action::Import),
        (shortcut(c, Key::Q), Action::Quit),
        (shortcut(cs, Key::A), Action::SelectNone),
        (shortcut(c, Key::A), Action::SelectAll),
        (shortcut(c, Key::D), Action::Duplicate),
        (shortcut(cs, Key::G), Action::Ungroup),
        (shortcut(c, Key::G), Action::Group),
        (shortcut(c, Key::C), Action::Copy),
        (shortcut(c, Key::X), Action::Cut),
        (shortcut(c, Key::V), Action::Paste),
        (shortcut(c, Key::Num0), Action::ZoomPage),
        (shortcut(c, Key::Num1), Action::ZoomActual),
        (shortcut(c, Key::Num2), Action::ZoomSelection),
        (shortcut(c, Key::Equals), Action::ZoomIn),
        (shortcut(c, Key::Plus), Action::ZoomIn),
        (shortcut(c, Key::Minus), Action::ZoomOut),
        (shortcut(Modifiers::NONE, Key::Delete), Action::Delete),
        (shortcut(Modifiers::NONE, Key::Backspace), Action::Delete),
    ]
}

impl StudioApp {
    pub fn new(cc: &eframe::CreationContext<'_>, file: Option<PathBuf>) -> Self {
        let paths = AppPaths::from_system().ok();
        let mut status = String::from("Ready");
        let settings = match paths.as_ref().map(Settings::load) {
            Some(Ok(s)) => s,
            Some(Err(e)) => {
                status = format!("Settings not loaded ({e}); using defaults");
                Settings::default()
            }
            None => Settings::default(),
        };
        let doc = Document { page: Page::preset(&settings.default_media).unwrap_or_default(), ..Document::default() };
        let mut app = Self {
            paths,
            active_layer: doc.default_layer(),
            editor: Editor::new(doc),
            settings,
            file: None,
            canvas: Canvas::default(),
            send: SendPanel::default(),
            tab: Tab::Properties,
            status,
            clipboard: Vec::new(),
            confirm: None,
            force_discard: false,
            allow_close: false,
            offset: None,
            about: false,
            last_title: String::new(),
        };
        app.editor.tolerance = app.settings.curve_tolerance_mm;
        app.apply_theme(&cc.egui_ctx);
        app.canvas.fit_to(app.editor.doc.page.rect());
        if let Some(f) = file {
            app.open_path(f);
        }
        app
    }

    fn apply_theme(&self, ctx: &egui::Context) {
        ctx.set_theme(match self.settings.theme {
            Theme::System => egui::ThemePreference::System,
            Theme::Light => egui::ThemePreference::Light,
            Theme::Dark => egui::ThemePreference::Dark,
        });
    }

    fn save_settings(&mut self) {
        if let Some(p) = &self.paths
            && let Err(e) = self.settings.save(p)
        {
            self.status = format!("Could not save settings: {e}");
        }
    }

    fn set_document(&mut self, doc: Document, file: Option<PathBuf>) {
        self.active_layer = doc.default_layer();
        self.editor = Editor::new(doc);
        self.editor.tolerance = self.settings.curve_tolerance_mm;
        self.file = file;
        self.canvas.cancel_interaction();
        self.canvas.overlay.clear();
        let page = self.editor.doc.page.rect();
        let r = self.editor.doc.bounds().map_or(page, |b| b.union(page));
        self.canvas.fit_to(r);
    }

    fn open_path(&mut self, path: PathBuf) {
        match ss_formats::open(&path) {
            Ok(doc) => {
                let n = doc.objects.len();
                let native = path.extension().is_some_and(|e| e.eq_ignore_ascii_case(ss_doc::NATIVE_EXTENSION));
                self.settings.push_recent(path.clone());
                self.save_settings();
                self.status = format!("Opened {} ({n} objects)", display_name(&path));
                // Only native projects are saved back in place; other formats are imported.
                self.set_document(doc, native.then_some(path));
            }
            Err(e) => self.status = format!("Could not open {}: {e}", display_name(&path)),
        }
    }

    fn pick_open_file(&self, title: &str) -> Option<PathBuf> {
        let mut d = rfd::FileDialog::new()
            .set_title(title)
            .add_filter("All supported", OPEN_EXTENSIONS)
            .add_filter("Silhouette Studio", &["studio3", "studio"])
            .add_filter("SVG", &["svg", "svgz"])
            .add_filter("DXF", &["dxf"])
            .add_filter("Project", &[ss_doc::NATIVE_EXTENSION]);
        if let Some(dir) = self.settings.recent_files.first().and_then(|f| f.parent()) {
            d = d.set_directory(dir);
        }
        d.pick_file()
    }

    fn default_name(&self, ext: &str) -> String {
        let stem = self.file.as_deref().and_then(Path::file_stem).map_or("Untitled".into(), |s| s.to_string_lossy().into_owned());
        format!("{stem}.{ext}")
    }

    fn save(&mut self, as_new: bool) -> bool {
        let native = ss_doc::NATIVE_EXTENSION;
        let target = match &self.file {
            Some(p) if !as_new => Some(p.clone()),
            _ => rfd::FileDialog::new().set_title("Save project").add_filter("Project", &[native]).set_file_name(self.default_name(native)).save_file(),
        };
        let Some(mut path) = target else { return false };
        if path.extension().is_none_or(|e| !e.eq_ignore_ascii_case(native)) {
            path.set_extension(native);
        }
        match ss_formats::save(&self.editor.doc, &path) {
            Ok(()) => {
                self.editor.dirty = false;
                self.status = format!("Saved {}", display_name(&path));
                self.settings.push_recent(path.clone());
                self.save_settings();
                self.file = Some(path);
                true
            }
            Err(e) => {
                self.status = format!("Could not save {}: {e}", display_name(&path));
                false
            }
        }
    }

    fn export(&mut self, ext: &str, label: &str) {
        let Some(mut path) = rfd::FileDialog::new()
            .set_title(format!("Export {label}"))
            .add_filter(label, &[ext])
            .set_file_name(self.default_name(ext))
            .save_file()
        else {
            return;
        };
        if path.extension().is_none_or(|e| !e.eq_ignore_ascii_case(ext)) {
            path.set_extension(ext);
        }
        self.status = match ss_formats::save(&self.editor.doc, &path) {
            Ok(()) => format!("Exported {}", display_name(&path)),
            Err(e) => format!("Export failed: {e}"),
        };
    }

    fn import(&mut self) {
        let Some(path) = self.pick_open_file("Import into the current design") else { return };
        match ss_formats::open(&path) {
            Ok(imported) => {
                let layer = self.active_layer;
                let r = self.editor.apply("Import", |d, sel| {
                    sel.clear();
                    for mut o in imported.objects {
                        o.reassign_ids();
                        o.layer = layer;
                        sel.push(o.id);
                        d.objects.push(o);
                    }
                    Ok(())
                });
                self.status = match r {
                    Ok(()) => format!("Imported {} objects from {}", self.editor.selection.len(), display_name(&path)),
                    Err(e) => e.to_string(),
                };
            }
            Err(e) => self.status = format!("Could not import {}: {e}", display_name(&path)),
        }
    }

    /// Returns false (and asks first) when unsaved changes would be lost.
    fn may_discard(&mut self, a: Action) -> bool {
        if self.editor.dirty && !std::mem::take(&mut self.force_discard) {
            self.confirm = Some(a);
            return false;
        }
        true
    }

    fn report<T>(&mut self, r: Result<T, ss_doc::EditError>) {
        if let Err(e) = r {
            self.status = e.to_string();
        }
    }

    fn run(&mut self, a: Action, ctx: &egui::Context) {
        match a {
            Action::New => {
                if self.may_discard(a) {
                    let page = Page::preset(&self.settings.default_media).unwrap_or_default();
                    self.set_document(Document { page, ..Document::default() }, None);
                    self.status = "New design".into();
                }
            }
            Action::Open => {
                if self.may_discard(a)
                    && let Some(p) = self.pick_open_file("Open")
                {
                    self.open_path(p);
                }
            }
            Action::OpenRecent(i) => {
                if self.may_discard(a)
                    && let Some(p) = self.settings.recent_files.get(i).cloned()
                {
                    self.open_path(p);
                }
            }
            Action::Import => self.import(),
            Action::Save => {
                self.save(false);
            }
            Action::SaveAs => {
                self.save(true);
            }
            Action::ExportSvg => self.export("svg", "SVG"),
            Action::ExportDxf => self.export("dxf", "DXF"),
            Action::Quit => {
                if self.may_discard(a) {
                    self.allow_close = true;
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
            Action::Undo => match self.editor.undo() {
                Some(l) => self.status = format!("Undid {l}"),
                None => self.status = "Nothing to undo".into(),
            },
            Action::Redo => match self.editor.redo() {
                Some(l) => self.status = format!("Redid {l}"),
                None => self.status = "Nothing to redo".into(),
            },
            Action::Copy | Action::Cut => {
                self.clipboard = self.editor.selected().into_iter().cloned().collect();
                if a == Action::Cut {
                    let r = self.editor.delete_selected();
                    self.report(r);
                }
            }
            Action::Paste => {
                if self.clipboard.is_empty() {
                    return;
                }
                let mut objs = self.clipboard.clone();
                for o in &mut objs {
                    o.reassign_ids();
                    o.transform = ss_geom::Affine::translate(Vec2::new(PASTE_OFFSET_MM, PASTE_OFFSET_MM)) * o.transform;
                }
                // The next paste lands a further step away.
                self.clipboard = objs.clone();
                let r = self.editor.apply("Paste", |d, sel| {
                    *sel = objs.iter().map(|o| o.id).collect();
                    d.objects.extend(objs);
                    Ok(())
                });
                self.report(r);
            }
            Action::Duplicate => {
                let r = self.editor.duplicate_selected(Vec2::new(PASTE_OFFSET_MM, PASTE_OFFSET_MM));
                self.report(r);
            }
            Action::Delete => {
                let r = self.editor.delete_selected();
                self.report(r);
            }
            Action::SelectAll => self.editor.select_all(),
            Action::SelectNone => self.editor.selection.clear(),
            Action::Group => {
                let r = self.editor.group_selected();
                self.report(r);
            }
            Action::Ungroup => {
                let r = self.editor.ungroup_selected();
                self.report(r);
            }
            Action::MakeCompound => {
                let r = self.editor.make_compound();
                self.report(r);
            }
            Action::ReleaseCompound => {
                let r = self.editor.release_compound();
                self.report(r);
            }
            Action::Weld => {
                let r = self.editor.weld();
                self.report(r);
            }
            Action::Subtract => {
                let r = self.editor.subtract();
                self.report(r);
            }
            Action::Intersect => {
                let r = self.editor.intersect();
                self.report(r);
            }
            Action::OffsetDialog => {
                if self.editor.selection.is_empty() {
                    self.status = ss_doc::EditError::EmptySelection.to_string();
                } else {
                    self.offset = Some(OffsetDialog { distance: 2.0, join: Join::Round });
                }
            }
            Action::FlipH => {
                let r = self.editor.flip_selected(true);
                self.report(r);
            }
            Action::FlipV => {
                let r = self.editor.flip_selected(false);
                self.report(r);
            }
            Action::Rotate(deg) => {
                let r = self.editor.rotate_selected(deg);
                self.report(r);
            }
            Action::Align(how) => {
                let r = self.editor.align_selected(how);
                self.report(r);
            }
            Action::Distribute(h) => {
                let r = self.editor.distribute_selected(h);
                self.report(r);
            }
            Action::Reorder { front, all } => {
                let r = self.editor.reorder_selected(front, all);
                self.report(r);
            }
            Action::SetCut(c) => {
                let r = self.editor.set_cut_action(c);
                self.report(r);
            }
            Action::ZoomPage => self.canvas.fit_to(self.editor.doc.page.rect()),
            Action::ZoomSelection => {
                let ids = if self.editor.selection.is_empty() {
                    self.editor.doc.objects.iter().map(|o| o.id).collect()
                } else {
                    self.editor.selection.clone()
                };
                if let Some(b) = self.canvas.bounds_of(&ids) {
                    self.canvas.fit_to(b.inflate(2.0, 2.0));
                }
            }
            Action::ZoomActual => self.canvas.zoom_actual(),
            Action::ZoomIn => self.canvas.zoom_by(1.25),
            Action::ZoomOut => self.canvas.zoom_by(0.8),
            Action::Nudge(dx, dy) => {
                if !self.editor.selection.is_empty() {
                    let r = self.editor.move_selected(Vec2::new(dx, dy));
                    self.report(r);
                }
            }
            Action::SetTool(t) => {
                self.canvas.cancel_interaction();
                self.canvas.tool = t;
            }
            Action::About => self.about = true,
        }
    }

    fn keyboard(&self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        for (s, a) in shortcuts() {
            if ctx.input_mut(|i| i.consume_shortcut(&s)) {
                actions.push(a);
            }
        }
        ctx.input(|i| {
            if i.modifiers.any() {
                return;
            }
            for t in Tool::ALL {
                if i.key_pressed(t.key()) {
                    actions.push(Action::SetTool(t));
                }
            }
        });
        let step = ctx.input(|i| if i.modifiers.shift { 10.0 } else { 1.0 });
        for (k, dx, dy) in [(Key::ArrowLeft, -1.0, 0.0), (Key::ArrowRight, 1.0, 0.0), (Key::ArrowUp, 0.0, -1.0), (Key::ArrowDown, 0.0, 1.0)] {
            if ctx.input(|i| i.key_pressed(k) && !i.modifiers.command) {
                actions.push(Action::Nudge(dx * step, dy * step));
            }
        }
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let sc: std::collections::HashMap<String, String> = shortcuts()
            .into_iter()
            .map(|(s, a)| (format!("{a:?}"), ui.ctx().format_shortcut(&s)))
            .rev()
            .collect();
        let any = !self.editor.selection.is_empty();
        let many = self.editor.selection.len() >= 2;
        let item = |ui: &mut egui::Ui, label: &str, a: Action, enabled: bool, actions: &mut Vec<Action>| {
            let mut b = egui::Button::new(label);
            if let Some(s) = sc.get(&format!("{a:?}")) {
                b = b.shortcut_text(s.clone());
            }
            if ui.add_enabled(enabled, b).clicked() {
                actions.push(a);
                ui.close();
            }
        };
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                item(ui, "New", Action::New, true, actions);
                item(ui, "Open…", Action::Open, true, actions);
                ui.menu_button("Open recent", |ui| {
                    if self.settings.recent_files.is_empty() {
                        ui.label("No recent files");
                    }
                    for (i, f) in self.settings.recent_files.iter().enumerate() {
                        if ui.button(f.display().to_string()).clicked() {
                            actions.push(Action::OpenRecent(i));
                            ui.close();
                        }
                    }
                });
                item(ui, "Import…", Action::Import, true, actions);
                ui.separator();
                item(ui, "Save", Action::Save, true, actions);
                item(ui, "Save as…", Action::SaveAs, true, actions);
                ui.separator();
                item(ui, "Export SVG…", Action::ExportSvg, true, actions);
                item(ui, "Export DXF…", Action::ExportDxf, true, actions);
                ui.separator();
                item(ui, "Quit", Action::Quit, true, actions);
            });
            ui.menu_button("Edit", |ui| {
                let undo = self.editor.history.undo_label().map_or("Undo".into(), |l| format!("Undo {l}"));
                let redo = self.editor.history.redo_label().map_or("Redo".into(), |l| format!("Redo {l}"));
                item(ui, &undo, Action::Undo, self.editor.history.can_undo(), actions);
                item(ui, &redo, Action::Redo, self.editor.history.can_redo(), actions);
                ui.separator();
                item(ui, "Cut", Action::Cut, any, actions);
                item(ui, "Copy", Action::Copy, any, actions);
                item(ui, "Paste", Action::Paste, !self.clipboard.is_empty(), actions);
                item(ui, "Duplicate", Action::Duplicate, any, actions);
                item(ui, "Delete", Action::Delete, any, actions);
                ui.separator();
                item(ui, "Select all", Action::SelectAll, true, actions);
                item(ui, "Deselect", Action::SelectNone, any, actions);
            });
            ui.menu_button("Object", |ui| {
                item(ui, "Group", Action::Group, many, actions);
                item(ui, "Ungroup", Action::Ungroup, any, actions);
                item(ui, "Make compound path", Action::MakeCompound, many, actions);
                item(ui, "Release compound path", Action::ReleaseCompound, any, actions);
                ui.separator();
                item(ui, "Weld", Action::Weld, many, actions);
                item(ui, "Subtract", Action::Subtract, many, actions);
                item(ui, "Intersect", Action::Intersect, many, actions);
                item(ui, "Offset…", Action::OffsetDialog, any, actions);
                ui.separator();
                item(ui, "Flip horizontally", Action::FlipH, any, actions);
                item(ui, "Flip vertically", Action::FlipV, any, actions);
                item(ui, "Rotate 90° clockwise", Action::Rotate(90.0), any, actions);
                item(ui, "Rotate 90° counter-clockwise", Action::Rotate(-90.0), any, actions);
                ui.menu_button("Align", |ui| {
                    for (l, a) in [
                        ("Left", Align::Left),
                        ("Centre horizontally", Align::CenterH),
                        ("Right", Align::Right),
                        ("Top", Align::Top),
                        ("Centre vertically", Align::CenterV),
                        ("Bottom", Align::Bottom),
                    ] {
                        item(ui, l, Action::Align(a), many, actions);
                    }
                    ui.separator();
                    item(ui, "Distribute horizontally", Action::Distribute(true), self.editor.selection.len() >= 3, actions);
                    item(ui, "Distribute vertically", Action::Distribute(false), self.editor.selection.len() >= 3, actions);
                });
                ui.menu_button("Arrange", |ui| {
                    item(ui, "Bring to front", Action::Reorder { front: true, all: true }, any, actions);
                    item(ui, "Bring forward", Action::Reorder { front: true, all: false }, any, actions);
                    item(ui, "Send backward", Action::Reorder { front: false, all: false }, any, actions);
                    item(ui, "Send to back", Action::Reorder { front: false, all: true }, any, actions);
                });
            });
            ui.menu_button("View", |ui| {
                item(ui, "Zoom to page", Action::ZoomPage, true, actions);
                item(ui, "Zoom to selection", Action::ZoomSelection, true, actions);
                item(ui, "Actual size", Action::ZoomActual, true, actions);
                item(ui, "Zoom in", Action::ZoomIn, true, actions);
                item(ui, "Zoom out", Action::ZoomOut, true, actions);
                ui.separator();
                let mut changed = ui.checkbox(&mut self.settings.show_grid, "Show grid").changed();
                changed |= ui.checkbox(&mut self.settings.snap_to_grid, "Snap to grid").changed();
                ui.menu_button("Units", |ui| {
                    changed |= ui.radio_value(&mut self.settings.units, Units::Mm, "Millimetres").changed();
                    changed |= ui.radio_value(&mut self.settings.units, Units::Inch, "Inches").changed();
                });
                ui.menu_button("Theme", |ui| {
                    let mut t = false;
                    t |= ui.radio_value(&mut self.settings.theme, Theme::System, "Follow system").changed();
                    t |= ui.radio_value(&mut self.settings.theme, Theme::Light, "Light").changed();
                    t |= ui.radio_value(&mut self.settings.theme, Theme::Dark, "Dark").changed();
                    if t {
                        self.apply_theme(ui.ctx());
                        changed = true;
                    }
                });
                if changed {
                    self.save_settings();
                }
            });
            ui.menu_button("Help", |ui| item(ui, "About", Action::About, true, actions));
        });
    }

    fn dialogs(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        if let Some(pending) = self.confirm {
            let mut answer = None;
            egui::Modal::new(egui::Id::unique("confirm-discard")).show(ctx, |ui| {
                ui.heading("Save changes?");
                ui.label("The design has unsaved changes.");
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        answer = Some(1);
                    }
                    if ui.button("Don't save").clicked() {
                        answer = Some(2);
                    }
                    if ui.button("Cancel").clicked() {
                        answer = Some(0);
                    }
                });
            });
            match answer {
                Some(1) => {
                    self.confirm = None;
                    if self.save(false) {
                        actions.push(pending);
                    }
                }
                Some(2) => {
                    self.confirm = None;
                    self.force_discard = true;
                    actions.push(pending);
                }
                Some(_) => self.confirm = None,
                None => {}
            }
        }
        if let Some(dlg) = &mut self.offset {
            let mut done = None;
            egui::Modal::new(egui::Id::unique("offset")).show(ctx, |ui| {
                ui.heading("Offset");
                ui.horizontal(|ui| {
                    ui.label("Distance");
                    ui.add(egui::DragValue::new(&mut dlg.distance).speed(0.1).range(-100.0..=100.0).suffix(" mm"));
                });
                ui.label("Negative values make an internal offset.");
                ui.horizontal(|ui| {
                    ui.label("Corners");
                    ui.radio_value(&mut dlg.join, Join::Round, "Round");
                    ui.radio_value(&mut dlg.join, Join::Miter, "Sharp");
                    ui.radio_value(&mut dlg.join, Join::Bevel, "Bevel");
                });
                ui.horizontal(|ui| {
                    if ui.button("Apply").clicked() {
                        done = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        done = Some(false);
                    }
                });
            });
            if let Some(ok) = done {
                let (d, j) = (dlg.distance, dlg.join);
                self.offset = None;
                if ok {
                    let r = self.editor.offset_selected(d, j);
                    self.report(r);
                }
            }
        }
        if self.about {
            let r = egui::Modal::new(egui::Id::unique("about")).show(ctx, |ui| {
                ui.heading("Silhouette Studio for Linux");
                ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                ui.label("An independent, native Linux reimplementation. Not affiliated with Silhouette America.");
                ui.label("Licensed under the GNU GPL, version 3 or later.");
                ui.button("Close").clicked()
            });
            if r.inner || r.should_close() {
                self.about = false;
            }
        }
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let name = self.file.as_deref().and_then(Path::file_name).map_or("Untitled".into(), |n| n.to_string_lossy().into_owned());
        let title = format!("{}{} — Silhouette Studio", name, if self.editor.dirty { " *" } else { "" });
        if title != self.last_title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        let (f, unit) = units(self.settings.units);
        egui::Sides::new().shrink_right().truncate().show(
            ui,
            |ui| {
                match self.canvas.cursor_mm {
                    Some(p) => ui.monospace(format!("{:8.2}, {:8.2} {unit}", p.x * f, p.y * f)),
                    None => ui.monospace(format!("{:>20}", "")),
                };
                ui.separator();
                ui.label(format!("{:.0} %", self.canvas.view.zoom / crate::canvas::ACTUAL_SIZE_ZOOM * 100.0));
                ui.separator();
                ui.label(format!("{} objects, {} selected", self.editor.doc.objects.len(), self.editor.selection.len()));
                ui.separator();
                ui.add(egui::Label::new(&self.status).truncate());
            },
            |ui| {
                ui.weak(self.canvas.tool.hint());
            },
        );
    }
}

fn display_name(p: &Path) -> String {
    p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// Display factor from mm and the unit suffix.
pub fn units(u: Units) -> (f64, &'static str) {
    match u {
        Units::Mm => (1.0, "mm"),
        Units::Inch => (1.0 / 25.4, "in"),
    }
}

impl eframe::App for StudioApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.editor.dirty && !self.allow_close {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                self.confirm = Some(Action::Quit);
            } else {
                self.save_settings();
            }
        }
        let mut actions = Vec::new();
        self.keyboard(&ctx, &mut actions);
        if let Some(overlay) = self.send.poll(&ctx, &mut self.status) {
            self.canvas.overlay = overlay;
        }

        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui, &mut actions));
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("tools").resizable(false).exact_size(96.0).show(ui, |ui| {
            ui.add_space(4.0);
            for t in Tool::ALL {
                let r = ui.add_sized([84.0, 26.0], egui::Button::selectable(self.canvas.tool == t, t.label()));
                if r.on_hover_text(format!("{} ({:?})", t.hint(), t.key())).clicked() {
                    actions.push(Action::SetTool(t));
                }
            }
        });
        egui::Panel::right("side").default_size(300.0).min_size(240.0).show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Properties, "Properties");
                ui.selectable_value(&mut self.tab, Tab::Layers, "Layers");
                ui.selectable_value(&mut self.tab, Tab::Page, "Page");
                ui.selectable_value(&mut self.tab, Tab::Send, "Send");
            });
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| match self.tab {
                Tab::Properties => {
                    let b = self.canvas.bounds_of(&self.editor.selection);
                    panels::properties(ui, &mut self.editor, b, self.settings.units, &mut actions, &mut self.status);
                }
                Tab::Layers => panels::layers(ui, &mut self.editor, &mut self.active_layer),
                Tab::Page => {
                    let change = panels::page(ui, &mut self.editor, &mut self.settings);
                    if change.settings {
                        self.save_settings();
                    }
                    if change.page {
                        self.canvas.fit_to(self.editor.doc.page.rect());
                    }
                }
                Tab::Send => self.send.ui(ui, &self.editor, &mut self.status),
            });
        });
        egui::CentralPanel::no_frame().show(ui, |ui| {
            self.canvas.show(ui, &mut self.editor, &self.settings, self.active_layer, &mut actions, &mut self.status);
        });

        self.dialogs(&ctx, &mut actions);
        for a in actions {
            self.run(a, &ctx);
        }
        if self.editor.doc.layer(self.active_layer).is_none() {
            self.active_layer = self.editor.doc.default_layer();
        }
        self.update_title(&ctx);
    }

    fn on_exit(&mut self) {
        self.save_settings();
    }
}
