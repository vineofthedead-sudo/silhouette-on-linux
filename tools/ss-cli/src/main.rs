//! ss-cli: inspect, convert and send Silhouette design files from the command line.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use ss_device::{FileTransport, JobEvent, RunOptions, UsbTransport, VirtualCutter, spawn_job};
use ss_doc::{CutAction, Document, ObjectKind};
use ss_gpgl::{EndAction, JobSettings, OffMedia, Perforation, Tool, ToolSettings};

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
        /// Design files (.studio3, .studio, .svg, .svgz, .dxf, .ssrs).
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// Print one JSON object per file instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Convert a design file to another format (.ssrs, .svg, .dxf).
    Convert {
        input: PathBuf,
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
                    if let Some(b) = &s["bounds_mm"].as_array() {
                        println!("  bounds: ({:.1}, {:.1}) - ({:.1}, {:.1}) mm", b[0], b[1], b[2], b[3]);
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
    dry_run: bool,
) -> Result<()> {
    let doc = ss_formats::open(file).with_context(|| format!("cannot open {}", file.display()))?;
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
            run(spawn_job(vc, job.bytes, RunOptions { poll_interval: Duration::from_millis(20), ..RunOptions::default() }))?;
            let sim = handle.simulation();
            println!("virtual cutter: {} strokes, {:.0} mm cut, {:.0} mm travel", sim.strokes.len(), job.stats.cut_mm, sim.travel_mm);
            if !sim.errors.is_empty() || !sim.unknown.is_empty() {
                println!("  warnings: {} malformed, {} unknown commands", sim.errors.len(), sim.unknown.len());
            }
        }
        "file" => {
            let path = output.unwrap_or_else(|| PathBuf::from("job.gpgl"));
            let t = FileTransport::create(&path).with_context(|| format!("cannot create {}", path.display()))?;
            run(spawn_job(t, job.bytes, RunOptions { poll_interval: Duration::from_millis(1), ..RunOptions::default() }))?;
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
            run(spawn_job(t, job.bytes, RunOptions::default()))?;
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
        Cmd::Send { file, to, output, lines, force, speed, passes, blade_offset, overcut, off_media, no_optimize, perforation, end, track_enhancing, dry_run } => {
            send(&file, &to, output, lines, force, speed, passes, blade_offset, overcut, off_media, no_optimize, perforation, end, track_enhancing, dry_run)
        }
    }
}
