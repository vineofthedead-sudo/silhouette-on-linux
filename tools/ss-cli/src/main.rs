//! ss-cli: inspect, convert and send Silhouette design files from the command line.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use ss_device::{FileTransport, JobEvent, RunOptions, UsbTransport, VirtualCutter, spawn_cut_job};
use ss_doc::{CutAction, Document, ObjectKind, TextData};
use ss_gpgl::{EndAction, JobSettings, OffMedia, Perforation, Registration, Tool, ToolSettings};

#[derive(Parser)]
#[command(name = "ss-cli", version, about = "Silhouette Studio for Linux command-line tools")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List connected USB cutters.
    Devices,
    /// Show page, layer and object information about design files.
    Inspect {
        /// Design files (.studio3, .studio, .svg, .svgz, .dxf, .craft, .ssrs).
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// Print one JSON object per file instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Convert a design file to another format, chosen by the output extension
    /// (.ssrs, .svg, .svgz, .dxf, .craft).
    Convert {
        input: PathBuf,
        output: PathBuf,
    },
    /// Convert every design file in a folder.
    Batch {
        /// Folder to convert.
        dir: PathBuf,
        /// Output format.
        #[arg(long, value_enum)]
        to: BatchFormat,
        /// Output folder (default: next to each input file).
        #[arg(long, short)]
        out: Option<PathBuf>,
        /// Also convert files in subfolders.
        #[arg(long, short)]
        recursive: bool,
        /// Replace output files that already exist.
        #[arg(long)]
        overwrite: bool,
    },
    /// List the font families available for text.
    Fonts,
    /// Lay out text with an installed font and save it as a design.
    Text {
        /// The text; use \n for a line break.
        text: String,
        /// Font family (falls back to a similar installed font).
        #[arg(long, default_value = "Sans")]
        font: String,
        /// Em size in mm.
        #[arg(long, default_value = "20")]
        size: f64,
        #[arg(long)]
        bold: bool,
        #[arg(long)]
        italic: bool,
        /// Extra space between letters, mm.
        #[arg(long, default_value = "0")]
        letter_spacing: f64,
        /// Line spacing as a multiple of the font's line height.
        #[arg(long, default_value = "1")]
        line_spacing: f64,
        /// Store outlines as plain paths instead of editable text.
        #[arg(long)]
        outline: bool,
        /// Output file (.svg, .dxf, .craft, .ssrs).
        #[arg(long, short)]
        output: PathBuf,
    },
    /// Encode a design as a cut job and send it to a cutter, a file, or the virtual cutter.
    Send {
        /// Design file to cut.
        file: PathBuf,
        /// Where the job goes: `usb` (first cutter found), `usb:N` (N-th listed), `virtual`, or `file`.
        #[arg(long, default_value = "usb")]
        to: String,
        /// Output file for `--to file`.
        #[arg(long, short)]
        output: Option<PathBuf>,
        /// Which lines to process.
        #[arg(long, value_enum, default_value = "cut")]
        lines: LinesArg,
        /// Blade force, 1-33.
        #[arg(long, default_value = "10")]
        force: u32,
        /// Speed, 1-10.
        #[arg(long, default_value = "5")]
        speed: u32,
        /// Cut each path this many times.
        #[arg(long, default_value = "1")]
        passes: u32,
        /// Blade offset in cutter units (1/20 mm); 18 for the standard blade.
        #[arg(long, default_value = "18")]
        blade_offset: u32,
        /// Extra cut past the start of closed paths, mm.
        #[arg(long, default_value = "0")]
        overcut: f64,
        /// Geometry outside the page: refuse, skip, or clip it.
        #[arg(long, value_enum, default_value = "error")]
        off_media: OffMediaArg,
        /// Disable cut-order optimisation.
        #[arg(long)]
        no_optimize: bool,
        /// Perforation as CUT,GAP in mm (only with --lines perforate).
        #[arg(long, value_parser = parse_perforation)]
        perforation: Option<Perforation>,
        /// What the carriage does at the end.
        #[arg(long, value_enum, default_value = "home")]
        end: EndArg,
        /// Grip the media harder while feeding.
        #[arg(long)]
        track_enhancing: bool,
        /// Print & Cut (experimental): register on the printed marks first. Uses the
        /// document's marks, or the default layout when it has none.
        #[arg(long)]
        regmarks: bool,
        /// Let the cutter search for the marks automatically instead of positioning the tool
        /// over the first mark by hand.
        #[arg(long, requires = "regmarks")]
        auto_search: bool,
        /// Only encode the job and print statistics; do not send anything.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum LinesArg {
    Cut,
    Sketch,
    Perforate,
}

impl LinesArg {
    fn actions(self) -> &'static [CutAction] {
        match self {
            Self::Cut => &[CutAction::Cut, CutAction::CutEdge],
            Self::Sketch => &[CutAction::Sketch],
            Self::Perforate => &[CutAction::Perforate],
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum OffMediaArg {
    Error,
    Skip,
    Clip,
}

impl From<OffMediaArg> for OffMedia {
    fn from(v: OffMediaArg) -> Self {
        match v {
            OffMediaArg::Error => Self::Error,
            OffMediaArg::Skip => Self::SkipOutside,
            OffMediaArg::Clip => Self::Clip,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum BatchFormat {
    Svg,
    Svgz,
    Dxf,
    Craft,
    Ssrs,
}

impl BatchFormat {
    fn extension(self) -> &'static str {
        match self {
            Self::Svg => "svg",
            Self::Svgz => "svgz",
            Self::Dxf => "dxf",
            Self::Craft => "craft",
            Self::Ssrs => ss_doc::NATIVE_EXTENSION,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum EndArg {
    Home,
    Feed,
    Stay,
}

fn parse_perforation(s: &str) -> Result<Perforation, String> {
    let (cut, gap) = s.split_once(',').ok_or("expected CUT,GAP in millimetres")?;
    let cut_mm: f64 = cut.parse().map_err(|_| "invalid cut length")?;
    let gap_mm: f64 = gap.parse().map_err(|_| "invalid gap length")?;
    if cut_mm <= 0.0 || gap_mm <= 0.0 {
        return Err("lengths must be positive".into());
    }
    Ok(Perforation { cut_mm, gap_mm })
}

fn devices() -> Result<()> {
    let devs = ss_device::enumerate()?;
    if devs.is_empty() {
        println!("No cutters found. Check the USB cable, power, and the udev rule.");
        return Ok(());
    }
    for (i, d) in devs.iter().enumerate() {
        println!("{i}: {} ({:04x}:{:04x}, bus {} address {})", d.name(), d.vendor_id, d.product_id, d.bus, d.address);
    }
    Ok(())
}

fn summarize(doc: &Document) -> serde_json::Value {
    let kind = |o: &ss_doc::Object| match &o.kind {
        ObjectKind::Path { .. } => "path",
        ObjectKind::Text { .. } => "text",
        ObjectKind::Group { .. } => "group",
        ObjectKind::Image(_) => "image",
    };
    let mut by_kind = serde_json::Map::new();
    let mut by_cut = serde_json::Map::new();
    let mut elements = 0usize;
    let bump = |m: &mut serde_json::Map<String, serde_json::Value>, k: &str| {
        let v = m.entry(k.to_string()).or_insert(0.into());
        *v = v.as_i64().unwrap_or(0).saturating_add(1).into();
    };
    for o in &doc.objects {
        bump(&mut by_kind, kind(o));
        bump(&mut by_cut, &format!("{:?}", o.cut));
        if let ObjectKind::Path { path, .. } = &o.kind {
            elements += path.elements().len();
        }
    }
    serde_json::json!({
        "page_mm": [doc.page.width_mm, doc.page.height_mm],
        "media": doc.page.media,
        "layers": doc.layers.iter().map(|l| serde_json::json!({
            "name": l.name,
            "visible": l.visible,
            "locked": l.locked,
            "objects": doc.objects.iter().filter(|o| o.layer == l.id).count(),
        })).collect::<Vec<_>>(),
        "objects": doc.objects.len(),
        "by_kind": by_kind,
        "by_cut_action": by_cut,
        "path_elements": elements,
        "bounds_mm": doc.bounds().map(|r| [r.x0, r.y0, r.x1, r.y1]),
        "registration_marks": doc.page.registration.is_some(),
        "background": doc.background.is_some(),
    })
}

fn inspect(files: &[PathBuf], json: bool) -> Result<()> {
    let mut failed = false;
    for f in files {
        match ss_formats::open(f) {
            Ok(doc) => {
                let s = summarize(&doc);
                if json {
                    let mut s = s;
                    s["file"] = f.display().to_string().into();
                    println!("{}", serde_json::to_string(&s)?);
                } else {
                    println!("{}:", f.display());
                    println!("  page: {:.1} x {:.1} mm ({})", s["page_mm"][0].as_f64().unwrap_or(0.0), s["page_mm"][1].as_f64().unwrap_or(0.0), s["media"].as_str().unwrap_or("?"));
                    println!("  layers: {}", doc.layers.len());
                    println!("  objects: {} ({})", s["objects"], serde_json::to_string(&s["by_cut_action"])?);
                    println!("  kinds: {}", serde_json::to_string(&s["by_kind"])?);
                    if let Some(b) = &s["bounds_mm"].as_array() {
                        println!("  bounds: ({:.1}, {:.1}) - ({:.1}, {:.1}) mm", b[0], b[1], b[2], b[3]);
                    }
                    if let Some(r) = &doc.page.registration {
                        println!("  registration marks: inset {:.1} mm, length {:.1} mm", r.inset_mm, r.length_mm);
                    }
                    if doc.background.is_some() {
                        println!("  background image: yes");
                    }
                }
            }
            Err(e) => {
                eprintln!("{}: {e}", f.display());
                failed = true;
            }
        }
    }
    if failed { bail!("some files could not be opened"); }
    Ok(())
}

fn convert(input: &Path, output: &Path) -> Result<()> {
    let doc = ss_formats::open(input).with_context(|| format!("cannot open {}", input.display()))?;
    let n = doc.objects.len();
    ss_formats::save(&doc, output).with_context(|| format!("cannot write {}", output.display()))?;
    println!("{} -> {} ({n} objects)", input.display(), output.display());
    Ok(())
}

const DESIGN_EXTENSIONS: &[&str] = &["studio3", "studio", "svg", "svgz", "dxf", "craft", ss_doc::NATIVE_EXTENSION];

fn design_files(dir: &Path, recursive: bool) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).with_context(|| format!("cannot read {}", d.display()))? {
            let p = e?.path();
            if p.is_dir() {
                if recursive {
                    stack.push(p);
                }
            } else if p.extension().is_some_and(|x| DESIGN_EXTENSIONS.iter().any(|d| x.eq_ignore_ascii_case(d))) {
                out.push(p);
            }
        }
    }
    out.sort();
    Ok(out)
}

fn batch(dir: &Path, to: BatchFormat, out: Option<&Path>, recursive: bool, overwrite: bool) -> Result<()> {
    if let Some(o) = out {
        std::fs::create_dir_all(o).with_context(|| format!("cannot create {}", o.display()))?;
    }
    let ext = to.extension();
    let (mut done, mut skipped, mut failed) = (0, 0, Vec::new());
    for input in design_files(dir, recursive)? {
        if input.extension().is_some_and(|x| x.eq_ignore_ascii_case(ext)) {
            continue;
        }
        let target = match out {
            // Keep the folder structure so equal names in different subfolders do not collide.
            Some(o) => o.join(input.strip_prefix(dir).unwrap_or(&input)).with_extension(ext),
            None => input.with_extension(ext),
        };
        if target.exists() && !overwrite {
            println!("skip {} (exists; use --overwrite)", target.display());
            skipped += 1;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let result = ss_formats::open(&input).map_err(anyhow::Error::from).and_then(|doc| {
            ss_formats::save(&doc, &target)?;
            Ok(doc.objects.len())
        });
        match result {
            Ok(n) => {
                println!("{} -> {} ({n} objects)", input.display(), target.display());
                done += 1;
            }
            Err(e) => {
                eprintln!("{}: {e}", input.display());
                failed.push(input);
            }
        }
    }
    println!("converted {done}, skipped {skipped}, failed {}", failed.len());
    if !failed.is_empty() {
        bail!("{} file(s) could not be converted", failed.len());
    }
    Ok(())
}

fn fonts() -> Result<()> {
    let lib = ss_text::FontLibrary::system();
    if lib.is_empty() {
        bail!("no fonts found; install a font package such as ttf-dejavu");
    }
    for f in lib.families() {
        println!("{f}");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn text_cmd(text: &str, font: &str, size_mm: f64, bold: bool, italic: bool, letter_spacing_mm: f64, line_spacing: f64, outline: bool, output: &Path) -> Result<()> {
    let data = TextData { text: text.replace("\\n", "\n"), font_family: font.into(), size_mm, bold, italic, letter_spacing_mm, line_spacing };
    let lib = ss_text::FontLibrary::system();
    let laid = lib.layout(&data)?;
    if laid.missing_glyphs > 0 {
        eprintln!("warning: {} has no glyph for {} character(s)", laid.face.family, laid.missing_glyphs);
    }
    let mut doc = Document::default();
    let layer = doc.default_layer();
    let mut obj = ss_doc::Object::text(layer, data, laid.outlines);
    let m = doc.page.margin_mm;
    obj.transform = ss_geom::Affine::translate((m, m));
    doc.objects.push(obj);
    if outline {
        let id = doc.objects[0].id;
        let mut ed = ss_doc::Editor::new(doc);
        ed.selection = vec![id];
        ed.convert_text_to_paths()?;
        doc = ed.doc;
    }
    ss_formats::save(&doc, output).with_context(|| format!("cannot write {}", output.display()))?;
    println!("{} via {}: {:.1} x {:.1} mm -> {}", text.replace('\n', " "), laid.face.family, laid.width_mm, laid.height_mm, output.display());
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn send(
    file: &Path,
    to: &str,
    output: Option<PathBuf>,
    lines: LinesArg,
    force: u32,
    speed: u32,
    passes: u32,
    blade_offset: u32,
    overcut: f64,
    off_media: OffMediaArg,
    no_optimize: bool,
    perforation: Option<Perforation>,
    end: EndArg,
    track_enhancing: bool,
    regmarks: bool,
    auto_search: bool,
    dry_run: bool,
) -> Result<()> {
    let doc = ss_formats::open(file).with_context(|| format!("cannot open {}", file.display()))?;
    let registration = if regmarks {
        let marks = doc.page.registration.unwrap_or_default();
        marks.validate(&doc.page).map_err(|e| anyhow::anyhow!(e))?;
        println!("print & cut (experimental): marks {:.1} mm from the edges; {} search", marks.inset_mm, if auto_search { "automatic" } else { "manual" });
        Some(Registration::from_marks(&marks, &doc.page, auto_search))
    } else {
        if doc.page.registration.is_some() {
            println!("note: the design has registration marks; add --regmarks to align the cut to a printed sheet");
        }
        None
    };
    let paths = ss_gpgl::document_paths(&doc, lines.actions());
    if paths.is_empty() {
        bail!("no matching lines in {}; check the objects' cut actions", file.display());
    }
    let pen = matches!(lines, LinesArg::Sketch);
    let settings = JobSettings {
        media_mm: (doc.page.width_mm, doc.page.height_mm),
        tool: ToolSettings {
            tool: if pen { Tool::Pen } else { Tool::Blade },
            force,
            speed,
            passes,
            blade_offset: if pen { 0 } else { blade_offset },
            overcut_mm: if pen { 0.0 } else { overcut },
            tool_holder: None,
            blade_depth: None,
        },
        track_enhancing,
        end: match end {
            EndArg::Home => EndAction::Home,
            EndArg::Feed => EndAction::FeedOut { mm: doc.page.height_mm },
            EndArg::Stay => EndAction::Stay,
        },
        off_media: off_media.into(),
        optimize_order: !no_optimize,
        perforation: if matches!(lines, LinesArg::Perforate) { perforation.or(Some(Perforation { cut_mm: 2.0, gap_mm: 1.0 })) } else { None },
        registration,
        ..JobSettings::default()
    };
    let job = ss_gpgl::encode(&paths, &settings).map_err(|e| anyhow::anyhow!(e.to_string()))?;
    println!(
        "job: {} strokes, {:.0} mm cut, {:.0} mm travel, {} commands ({} bytes)",
        job.stats.strokes, job.stats.cut_mm, job.stats.travel_mm, job.stats.commands, job.bytes.len()
    );
    if dry_run {
        return Ok(());
    }

    let (kind, index) = to.split_once(':').map_or((to, None), |(k, v)| (k, v.parse().ok()));
    match kind {
        "virtual" => {
            let (vc, handle) = VirtualCutter::new();
            run(spawn_cut_job(vc, job.clone(), RunOptions { poll_interval: Duration::from_millis(20), ..RunOptions::default() }))?;
            let sim = handle.simulation();
            println!("virtual cutter: {} strokes, {:.0} mm cut, {:.0} mm travel", sim.strokes.len(), job.stats.cut_mm, sim.travel_mm);
            if !sim.errors.is_empty() || !sim.unknown.is_empty() {
                println!("  warnings: {} malformed, {} unknown commands", sim.errors.len(), sim.unknown.len());
            }
        }
        "file" => {
            let path = output.unwrap_or_else(|| PathBuf::from("job.gpgl"));
            let t = FileTransport::create(&path).with_context(|| format!("cannot create {}", path.display()))?;
            run(spawn_cut_job(t, job, RunOptions { poll_interval: Duration::from_millis(1), ..RunOptions::default() }))?;
            println!("wrote {}", path.display());
        }
        "usb" => {
            let devs = ss_device::enumerate()?;
            let i = index.unwrap_or(0);
            let Some(info) = devs.get(i) else {
                if devs.is_empty() {
                    bail!("no cutter found; use `--to virtual` or `--to file` to test without hardware");
                }
                bail!("no cutter #{i}; `ss-cli devices` lists {}", devs.len());
            };
            if let Some(m) = info.model
                && doc.page.width_mm > m.width_mm + 0.5
            {
                bail!("the page is wider than the {} can cut ({:.0} mm)", m.name, m.width_mm);
            }
            println!("sending to {} (Ctrl-C to stop between chunks)", info.name());
            let t = UsbTransport::open(info).with_context(|| format!("cannot open {}", info.name()))?;
            run(spawn_cut_job(t, job, RunOptions::default()))?;
        }
        other => bail!("unknown target {other:?}; expected usb, usb:N, virtual or file"),
    }
    Ok(())
}

/// Streams job events and returns an error if the job fails.
fn run(handle: ss_device::JobHandle) -> Result<()> {
    let mut failed = None;
    for ev in handle.events {
        match &ev {
            JobEvent::Connected { device, firmware } => {
                println!("connected: {device}{}", firmware.as_ref().map_or(String::new(), |f| format!(" (firmware {f})")));
            }
            JobEvent::WaitingForMedia => println!("load the mat or media into the cutter"),
            JobEvent::Sending { sent, total } => {
                print!("\rsending {sent}/{total} bytes   ");
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
            JobEvent::Registering => println!("\nlooking for the registration marks"),
            JobEvent::Registered => println!("marks found"),
            JobEvent::Finishing => println!("\ncutting; waiting for the cutter to finish"),
            JobEvent::Finished => println!("\nfinished"),
            JobEvent::Cancelled => println!("\ncancelled"),
            JobEvent::Failed(e) => failed = Some(e.clone()),
        }
        if ev.is_terminal() {
            break;
        }
    }
    if let Some(e) = failed { bail!(e); }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Devices => devices(),
        Cmd::Inspect { files, json } => inspect(&files, json),
        Cmd::Convert { input, output } => convert(&input, &output),
        Cmd::Batch { dir, to, out, recursive, overwrite } => batch(&dir, to, out.as_deref(), recursive, overwrite),
        Cmd::Fonts => fonts(),
        Cmd::Text { text, font, size, bold, italic, letter_spacing, line_spacing, outline, output } => {
            text_cmd(&text, &font, size, bold, italic, letter_spacing, line_spacing, outline, &output)
        }
        Cmd::Send {
            file,
            to,
            output,
            lines,
            force,
            speed,
            passes,
            blade_offset,
            overcut,
            off_media,
            no_optimize,
            perforation,
            end,
            track_enhancing,
            regmarks,
            auto_search,
            dry_run,
        } => send(
            &file,
            &to,
            output,
            lines,
            force,
            speed,
            passes,
            blade_offset,
            overcut,
            off_media,
            no_optimize,
            perforation,
            end,
            track_enhancing,
            regmarks,
            auto_search,
            dry_run,
        ),
    }
}
