# Silhouette Studio for Linux

An independent, native Linux reimplementation of Silhouette Studio, written in Rust.
Not affiliated with Silhouette America. Licensed GPL-3.0-or-later.

## Features

- Opens Silhouette `.studio3` / `.studio` files, SVG/SVGZ and DXF
- Design canvas: select, rectangle, ellipse, line, polyline, freehand tools,
  move/scale/rotate handles, grid and snapping
- Editing: undo/redo, group, weld, subtract, intersect, offset, compound paths,
  align/distribute, arrange, layers
- Send panel: cut/sketch/perforate lines, force/speed/passes/blade offset,
  overcut, perforation dashes, off-media handling (error/skip/clip),
  tool-path preview and a virtual cutter for testing without hardware
- Talks to Silhouette/Graphtec USB cutters (Cameo, Portrait, and others; see
  `crates/ss-device/src/models.rs`)
- `ss-cli`: batch inspect/convert/send from the command line

## Build

```sh
cargo build --release
./target/release/silhouette-studio          # GUI
./target/release/ss-cli --help              # CLI
```

## Install (Arch Linux)

```sh
cd packaging && makepkg -si
```

This installs both binaries, the desktop entry, and the udev rule granting
access to the cutters. Replug the cutter after installing.

## CLI examples

```sh
ss-cli devices                                        # list connected cutters
ss-cli inspect design.studio3                         # page/object summary
ss-cli convert design.studio3 design.svg              # convert formats
ss-cli send design.studio3 --to usb                   # cut on the first cutter
ss-cli send design.studio3 --to virtual               # simulate a job
ss-cli send design.studio3 --off-media clip --lines perforate --perforation 2,1
```

## Workspace layout

- `crates/ss-geom` — geometry: flattening, curve fitting, booleans, offsets
- `crates/ss-doc` — document model and undoable editor
- `crates/ss-formats` — `.studio3`/`.studio`, SVG/SVGZ, DXF, native `.ssrs`
- `crates/ss-gpgl` — GPGL encoder, cut-order planner, interpreter
- `crates/ss-device` — USB transport, virtual cutter, job runner
- `crates/ss-config` — settings and recent files (XDG)
- `crates/ss-app` — the desktop application (eframe/egui)
- `tools/ss-cli` — command-line tools
- `docs/` — reverse-engineering report and design proposal
