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
> America, Inc. or Graphtec Corporation.** It ships no proprietary code or
> content, and cutting support has **not yet been tested on real hardware**.
> Read the full [DISCLAIMER.md](DISCLAIMER.md) before using it with a cutter.

## Feature status matrix

| Feature area | Status | Notes |
|---|---|---|
| Design canvas (pan, zoom, grid, snap, cut border) | ✅ Implemented | |
| Drawing tools (select, rectangle, ellipse, line, polyline, freehand) | ✅ Implemented | Freehand input is fitted to smooth Béziers |
| Selection & transforms (move, scale, rotate, flip, align, distribute) | ✅ Implemented | Handles + numeric entry, single undo step per gesture |
| Group / ungroup, compound paths, group by colour | ✅ Implemented | |
| Boolean weld / subtract / intersect | ✅ Implemented | |
| Offset (round / miter / bevel, internal & external) | ✅ Implemented | |
| Layers (visibility, lock, reorder, per-layer objects) | ✅ Implemented | |
| Undo / redo | ✅ Implemented | Snapshot-based history |
| Text tool | ✅ Implemented | Any installed font, bold/italic (synthetic slant when no italic face), size, letter and line spacing, re-editable, convert to paths. Simple left-aligned layout: no ligatures or right-to-left shaping |
| Raster images | 🚧 Partial | PNG/JPEG placement and rendering, images embedded in `.studio3` and `.craft` files, page background image. No auto-trace |
| Knife / eraser / warp / nest tools | ❌ Not yet implemented | |
| `.studio3` / `.studio` read | ✅ Implemented | Validated against 176 real-world files, including 612 placed images (see tests) |
| `.studio3` / `.studio` write | ❌ Not yet implemented | Use the native `.ssrs` project format, `.craft`, or SVG/DXF export |
| `.craft` (Studio_to_SVG project) read & write | ✅ Implemented | Shapes, text, groups, images, paper and registration marks |
| SVG / SVGZ import & export | ✅ Implemented | Including gzip and UTF-8/UTF-16 variants, embedded images, background and registration marks |
| DXF import & export | ✅ Implemented | |
| Native project format (`.ssrs`) | ✅ Implemented | Lossless JSON format, versioned |
| GPGL job generation | ✅ Implemented | Force, speed, passes, blade offset, overcut, perforation dashes, media clipping, cut-order optimisation; images are never cut |
| USB cutter communication | 🚧 Partial / experimental | Full libusb transport + job runner; model database from Craft Robo through Cameo 5 Alpha. **Not yet validated on real hardware** |
| Dual-carriage (tool holder 2) | 🚧 Partial / experimental | GPGL sequences implemented, unverified on hardware |
| Print & Cut (registration marks) | 🚧 Partial / experimental | Type 1 marks on the canvas and in SVG export; GPGL mark search (automatic or manual) checked against inkscape-silhouette's values. **Not yet tested on a cutter** |
| Printing | ❌ Not yet implemented | Export SVG (marks included) and print it from another program |
| Bluetooth | ❌ Not yet implemented | |
| PixScan | ❌ Not yet implemented | |
| Rhinestones | ❌ Not yet implemented | |
| Design store / cloud library | ❌ Not planned | Local files only, by design |
| Virtual cutter & tool-path preview | ✅ Implemented | Test jobs without hardware; simulated strokes shown on the canvas |
| `ss-cli` batch tools | ✅ Implemented | `devices`, `inspect`, `convert`, `batch`, `fonts`, `text`, `send` |

See [STATUS.md](STATUS.md) for what has been verified and how, and
[ROADMAP.md](ROADMAP.md) for known limitations and the next milestones.

## Architecture

A Cargo workspace of small, individually testable crates:

| Crate | Role |
|---|---|
| `ss-geom` | Geometry kernel: flattening, curve fitting, boolean ops, offsets (on [kurbo](https://docs.rs/kurbo)) |
| `ss-doc` | Document model (objects, layers, page, registration marks, background) and the undoable editor |
| `ss-text` | Font discovery ([fontdb](https://docs.rs/fontdb)) and text-to-outline layout ([ab_glyph](https://docs.rs/ab_glyph)) |
| `ss-formats` | `.studio3`/`.studio` reader, `.craft`, SVG/SVGZ and DXF I/O, native `.ssrs` format |
| `ss-gpgl` | GPGL encoder (including registration), cut-order planner, and a full GPGL interpreter used as the virtual cutter |
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

Open a `.studio3`, `.studio`, `.craft`, `.svg(z)`, `.dxf`, or `.ssrs` file (or
pass it on the command line), edit, then use the **Send** tab. Send targets: a
connected USB cutter, the built-in **virtual cutter** (shows the simulated tool
path on the canvas), or a `.gpgl` file for later use.

- **Text:** pick the Text tool (`T`), click where the text should start, and
  choose the font and style. Select a text object and use *Edit text…* to change
  it, or *Convert to path* to weld and edit the letters.
- **Images:** *File → Place image…* adds a PNG or JPEG. The *Page* tab sets a
  background image. Images are shown and exported but never cut.
- **Print & Cut (experimental):** turn on *Registration marks* in the *Page*
  tab, export to SVG and print it, then load the printout and send with
  *Align to the registration marks* checked in the *Send* tab.

### CLI

```sh
ss-cli devices                                   # list connected cutters
ss-cli inspect design.studio3                    # page/object summary
ss-cli inspect --json design.studio3             # machine-readable
ss-cli convert design.studio3 design.svg         # convert between formats
ss-cli convert design.studio3 design.craft       # Studio_to_SVG project
ss-cli batch ~/designs --to svg -r -o ~/svg      # convert a whole folder
ss-cli fonts                                     # font families for text
ss-cli text "Hello\nWorld" --font "Liberation Sans" --size 25 --bold -o hello.svg
ss-cli send design.studio3 --to usb              # cut on the first cutter
ss-cli send design.studio3 --to virtual          # simulate a job
ss-cli send design.studio3 --dry-run             # just encode and print stats
ss-cli send design.studio3 --off-media clip --lines perforate --perforation 2,1
ss-cli send printed.ssrs --regmarks --auto-search  # Print & Cut (experimental)
```

## Companion project: Studio_to_SVG

[Studio_to_SVG](https://github.com/vineofthedead-sudo/Studio_to_SVG) is a
sister project focused on converting Silhouette Studio files to SVG and on the
`.craft` project format. This application reads and writes `.craft` files, so
designs move freely between the two.

## Testing

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
```

The suite covers geometry, the editor, text layout, format round trips
(including 176 real-world `.studio3` files validated against reference SVGs
when present), `.craft` interchange, GPGL golden tests including registration,
the device job runner, and an end-to-end CLI smoke test.

## Acknowledgments

- [Studio_to_SVG](https://github.com/vineofthedead-sudo/Studio_to_SVG), the
  companion project, for the `.craft` format and conversion work.
- [studio3svg](https://github.com/zaydmulani09/studio3svg) by Zayd Mulani, for
  public research into the `.studio3` format.
- [inkscape-silhouette](https://github.com/fablabnbg/inkscape-silhouette), for
  documenting the GPGL protocol and the registration-mark sequence.

## License

[GPL-3.0-or-later](LICENSE). See also [DISCLAIMER.md](DISCLAIMER.md).
