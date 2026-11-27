//! Auto-Trace panel: turns a selected raster image into vector cut contours, following the V5
//! Trace workflow (threshold, smoothing, a live preview, then Trace or Trace Outer Edge).

use egui::Ui;
use ss_doc::{Editor, LayerId, ObjectId, ObjectKind};
use ss_geom::flatten::flatten;
use ss_geom::trace::{TraceOptions, mask_from_rgba, trace_mask};
use ss_geom::{BezPath, ops};

use crate::canvas::Canvas;

/// Larger images are scaled down before tracing; this is V5's "Scale" smoothing in effect.
const MAX_TRACE_SIDE: u32 = 768;

struct Raster {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

fn decode(encoded: &[u8]) -> Option<Raster> {
    let mut img = image::load_from_memory(encoded).ok()?;
    if img.width().max(img.height()) > MAX_TRACE_SIDE {
        img = img.resize(MAX_TRACE_SIDE, MAX_TRACE_SIDE, image::imageops::FilterType::Triangle);
    }
    let rgba = img.to_rgba8();
    Some(Raster { width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw() })
}

pub struct TracePanel {
    opts: TraceOptions,
    live_preview: bool,
    preview_outer: bool,
    source: Option<ObjectId>,
    raster: Option<Raster>,
    dirty: bool,
    contours: usize,
}

impl Default for TracePanel {
    fn default() -> Self {
        Self { opts: TraceOptions::default(), live_preview: true, preview_outer: false, source: None, raster: None, dirty: true, contours: 0 }
    }
}

impl TracePanel {
    /// Forgets the current image, e.g. when another document is opened.
    pub fn reset(&mut self) {
        self.source = None;
        self.raster = None;
        self.dirty = true;
        self.contours = 0;
    }

    fn trace(&self, outer_only: bool) -> Vec<BezPath> {
        let Some(r) = &self.raster else { return Vec::new() };
        let mask = mask_from_rgba(&r.rgba, r.width as usize, r.height as usize, self.opts.threshold, self.opts.invert);
        trace_mask(&mask, r.width as usize, r.height as usize, &TraceOptions { outer_only, ..self.opts })
    }

    /// The selected image, if exactly one placed image is selected.
    fn target(editor: &Editor) -> Option<ObjectId> {
        let [id] = editor.selection.as_slice() else { return None };
        matches!(editor.doc.find(*id)?.kind, ObjectKind::Image(_)).then_some(*id)
    }

    fn update_preview(&mut self, editor: &Editor, canvas: &mut Canvas) {
        canvas.trace_preview.clear();
        self.contours = 0;
        let (Some(id), Some(r)) = (self.source, &self.raster) else { return };
        let Some(m) = editor.trace_transform(id, (r.width, r.height)) else { return };
        let paths = self.trace(self.preview_outer);
        self.contours = paths.len();
        canvas.trace_preview = paths
            .iter()
            .flat_map(|p| flatten(&ops::transformed(p, m), 0.05))
            .map(|l| l.points)
            .collect();
    }

    fn commit(&mut self, editor: &mut Editor, layer: LayerId, canvas: &mut Canvas, outer_only: bool, status: &mut String) {
        let (Some(id), Some(r)) = (self.source, &self.raster) else { return };
        let paths = self.trace(outer_only);
        let name = if outer_only { "Trace Outer Edge" } else { "Trace" };
        match editor.add_trace(id, layer, &paths, (r.width, r.height), name) {
            Ok(_) => {
                *status = format!("{name}: placed {} contour(s) on the active layer", paths.len());
                canvas.trace_preview.clear();
                // The new cut path is selected now; the preview returns when the image is selected again.
                self.source = None;
            }
            Err(e) => *status = format!("{name}: {e}"),
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, editor: &mut Editor, layer: LayerId, canvas: &mut Canvas, status: &mut String) {
        let Some(id) = Self::target(editor) else {
            ui.weak("Select one placed image to trace it into cut lines.");
            if self.source.take().is_some() || !canvas.trace_preview.is_empty() {
                canvas.trace_preview.clear();
                self.raster = None;
                self.contours = 0;
            }
            return;
        };
        if self.source != Some(id) {
            self.source = Some(id);
            self.raster = match editor.doc.find(id).map(|o| &o.kind) {
                Some(ObjectKind::Image(img)) => decode(&img.encoded),
                _ => None,
            };
            self.dirty = true;
        }
        if self.raster.is_none() {
            ui.weak("This image could not be read for tracing.");
            return;
        }

        ui.label("Opaque images trace their dark areas; images with transparency trace their opaque shape.");
        let mut changed = false;
        changed |= ui.add(egui::Slider::new(&mut self.opts.threshold, 1..=254).text("Threshold")).changed();
        changed |= ui.add(egui::Slider::new(&mut self.opts.smoothing, 0.0..=1.0).text("Corner smoothing")).changed();
        changed |= ui
            .add(egui::DragValue::new(&mut self.opts.min_area_px).range(0.0..=10_000.0).speed(0.5).prefix("Ignore specks under ").suffix(" px²"))
            .changed();
        changed |= ui.checkbox(&mut self.opts.invert, "Invert (trace the light areas)").changed();
        ui.horizontal(|ui| {
            changed |= ui.checkbox(&mut self.live_preview, "Preview").changed();
            changed |= ui.selectable_value(&mut self.preview_outer, false, "All lines").changed();
            changed |= ui.selectable_value(&mut self.preview_outer, true, "Outer edge").changed();
        });
        if changed || self.dirty {
            self.dirty = false;
            if self.live_preview {
                self.update_preview(editor, canvas);
            } else {
                canvas.trace_preview.clear();
                self.contours = 0;
            }
        }
        if self.live_preview {
            ui.weak(format!("{} contour(s) in the preview", self.contours));
        }
        ui.horizontal(|ui| {
            if ui.button("Trace").on_hover_text("Create every cut line shown in the preview").clicked() {
                self.commit(editor, layer, canvas, false, status);
            }
            if ui.button("Trace Outer Edge").on_hover_text("Create only the outermost cut lines").clicked() {
                self.commit(editor, layer, canvas, true, status);
            }
        });
    }
}
