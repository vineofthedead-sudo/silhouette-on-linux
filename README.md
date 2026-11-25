# Silhouette Studio for Linux

![Rust](https://img.shields.io/badge/rust-1.88%2B-orange?logo=rust)
![License](https://img.shields.io/badge/license-GPL--3.0--or--later-blue)
![Platform](https://img.shields.io/badge/platform-Linux-lightgrey?logo=linux)

An independent, native Linux reimplementation of **Silhouette Studio**, the design
and cutting software for Silhouette craft cutters (Cameo, Portrait, and other
Graphtec-based models), written in Rust.

It opens your existing `.studio3`/`.studio` designs, lets you edit them on a
fast native canvas, and drives the cutter directly over USB using the GPGL
protocol — no Windows, no Wine, no virtual machine.

> **Disclaimer:** This is an independent, clean-room interoperability project.
> It is **not affiliated with, endorsed by, or supported by Silhouette
> America, Inc.** "Silhouette" and product names are trademarks of their
> respective owners and are used here only to describe compatibility.
> Protocol and file-format knowledge comes from observing files and from
> public prior art such as inkscape-silhouette.

## Feature status matrix

| Feature area | Status | Notes |
|---|---|---|
| Design canvas (pan, zoom, grid, snap, cut border) | ✅ Implemented | |
| Drawing tools (select, rectangle, ellipse, line, polyline, freehand) | ✅ Implemented | Freehand input is fitted to smooth Béziers |
| Selection & transforms (move, scale, rotate, flip, align, distribute) | ✅ Implemented | Handles + numeric entry, single undo step per gesture |
| Group / ungroup, compound paths | ✅ Implemented | |
| Boolean weld / subtract / intersect | ✅ Implemented | |
| Offset (round / miter / bevel, internal & external) | ✅ Implemented | |
| Layers (visibility, lock, reorder, per-layer objects) | ✅ Implemented | |
| Undo / redo | ✅ Implemented | Snapshot-based history |
| Text tool | ❌ Not yet implemented | Text outlines contained in imported files render and cut, but there is no text creation/editing |
| Raster images (place, render, trace) | ❌ Not yet implemented | Image objects are parsed but not drawn; no auto-trace |
| Knife / eraser / warp / nest tools | ❌ Not yet implemented | |
| `.studio3` / `.studio` read | ✅ Implemented | Validated against 176 real-world files (see tests) |
| `.studio3` / `.studio` write | ❌ Not yet implemented | Use the native `.ssrs` project format or SVG/DXF export |
| SVG / SVGZ import & export | ✅ Implemented | Including gzip and UTF-8/UTF-16 variants |
| DXF import & export | ✅ Implemented | |
| Native project format (`.ssrs`) | ✅ Implemented | Lossless JSON format, versioned |
| GPGL job generation | ✅ Implemented | Force, speed, passes, blade offset, overcut, perforation dashes, media clipping, cut-order optimisation |
| USB cutter communication | 🚧 Partial / experimental | Full libusb transport + job runner; model database from Craft Robo through Cameo 5 Alpha. **Not yet validated on real hardware** |
| Dual-carriage (tool holder 2) | 🚧 Partial / experimental | GPGL sequences implemented, unverified on hardware |
| Bluetooth | ❌ Not yet implemented | |
| Print & Cut (registration marks) | ❌ Not yet implemented | |
| Printing | ❌ Not yet implemented | |
| PixScan | ❌ Not yet implemented | |
| Rhinestones | ❌ Not yet implemented | |
| Design store / cloud library | ❌ Not planned | Local files only, by design |
| Virtual cutter & tool-path preview | ✅ Implemented | Test jobs without hardware; simulated strokes shown on the canvas |
| `ss-cli` batch tools | ✅ Implemented | `devices`, `inspect`, `convert`, `send` |

See [ROADMAP.md](ROADMAP.md) for known limitations, test coverage, and the
next milestones.

## Architecture

A Cargo workspace of small, individually testable crates:

| Crate | Role |
|---|---|
| `ss-geom` | Geometry kernel: flattening, curve fitting, boolean ops, offsets (on [kurbo](https://docs.rs/kurbo)) |
| `ss-doc` | Document model (objects, layers, page) and the undoable editor |
| `ss-formats` | `.studio3`/`.studio` reader, SVG/SVGZ and DXF I/O, native `.ssrs` format |
| `ss-gpgl` | GPGL encoder, cut-order planner, and a full GPGL interpreter used as the virtual cutter |
| `ss-device` | `Transport` abstraction, USB via `rusb`, virtual cutter, file transport, cancellable job runner |
| `ss-config` | XDG settings, recent files |
| `ss-app` | The desktop application ([eframe](https://docs.rs/eframe)/egui, Wayland + X11) |
| `tools/ss-cli` | Command-line interface |

Reverse-engineering notes and the design proposal live in
[`docs/`](docs/PHASE1_INVESTIGATION.md).

## Installation (Arch Linux)

### From the AUR-style PKGBUILD

```sh
cd packaging
makepkg -si          # if the checkout path contains spaces: BUILDDIR=/tmp/makepkg-ss makepkg -si
```

This installs `silhouette-studio`, `ss-cli`, the desktop entry, and the udev
rule. Replug the cutter after installing (or run
`sudo udevadm control --reload-rules && sudo udevadm trigger`).

### From source

```sh
cargo build --release
./target/release/silhouette-studio          # the GUI
./target/release/ss-cli --help              # the CLI
```

### Cutter permissions

USB access needs the bundled udev rule:

```sh
sudo install -m644 packaging/70-silhouette-cutters.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
# unplug and replug the cutter
```

## Usage

### GUI

Open a `.studio3`, `.studio`, `.svg(z)`, `.dxf`, or `.ssrs` file (or pass it on
the command line), edit, then use the **Send** tab. Send targets: a connected
USB cutter, the built-in **virtual cutter** (shows the simulated tool path on
the canvas), or a `.gpgl` file for later use.

### CLI

```sh
ss-cli devices                                   # list connected cutters
ss-cli inspect design.studio3                    # page/object summary
ss-cli inspect --json design.studio3             # machine-readable
ss-cli convert design.studio3 design.svg         # convert between formats
ss-cli send design.studio3 --to usb              # cut on the first cutter
ss-cli send design.studio3 --to virtual          # simulate a job
ss-cli send design.studio3 --dry-run             # just encode and print stats
ss-cli send design.studio3 --off-media clip --lines perforate --perforation 2,1
```

## Testing

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
```

The suite covers geometry, the editor, format round trips (including 176
real-world `.studio3` files validated against reference SVGs when present),
GPGL golden tests, the device job runner, and an end-to-end CLI smoke test.

## License

[GPL-3.0-or-later](LICENSE).
