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
| Drawing tools (select, edit points, rectangle, ellipse, line, polyline, freehand) | ✅ Implemented | Freehand input is fitted to smooth Béziers; Edit Points supports live node/handle dragging, insertion, deletion, Break Path, Corner/Smooth, Make Flat/Curve, and Simplify |
| Selection, Transform & Replicate | ✅ Implemented | Handles + numeric X/Y/W/H, aspect lock, rotate, flip, align, distribute; Replicate panel (directional duplicates, mirror copies, rows/columns, grid array, Fill Page) |
| Group / ungroup, compound paths, group by colour | ✅ Implemented | |
| Boolean weld / subtract / intersect | ✅ Implemented | |
| Offset (round / miter / bevel, internal & external) | ✅ Implemented | |
| Layers (visibility, lock, reorder, per-layer objects) | ✅ Implemented | |
| Undo / redo | ✅ Implemented | Snapshot-based history |
| Text tool | ✅ Implemented | Any installed font, bold/italic (synthetic slant when no italic face), size, letter and line spacing, re-editable, convert to paths. Simple left-aligned layout: no ligatures or right-to-left shaping |
| Raster images & Auto-Trace | ✅ Implemented | PNG/JPEG placement and rendering, images embedded in `.studio3` and `.craft` files, page background image, and Auto-Trace (`Trace` & `Trace Outer Edge` with threshold, smoothing, and live preview) |
| Knife & Eraser tools | ✅ Implemented | Straight (`Shift`-snap) / freehand (`Ctrl`-drag) Knife slicing; Round / Square brush Eraser subtraction preserving `EvenOdd` holes |
| Warp / nest tools | ❌ Not yet implemented | |
| `.studio3` / `.studio` read | ✅ Implemented | Validated against 178 real-world and 39 stock files, including solid/gradient/pattern fills, strokes, placed images, and embedded preview fidelity |
| `.studio3` / `.studio` write | ❌ Not yet implemented | Use the native `.ssrs` project format, `.craft`, or SVG/DXF export |
| `.craft` (Studio_to_SVG project) read & write | ✅ Implemented | Shapes, text, groups, images, paper and registration marks |
| SVG / SVGZ import & export | ✅ Implemented | Including gzip and UTF-8/UTF-16 variants, gradients, patterns, dashed strokes, embedded images, background and registration marks |
| DXF import & export | ✅ Implemented | |
| Native project format (`.ssrs`) | ✅ Implemented | Lossless JSON format, versioned |
| GPGL job generation | ✅ Implemented | Force, speed, passes, blade depth (`TC` / Era 3 `TJ`/`TF`), blade offset, overcut, perforation dashes, media clipping, multi-stage tool/pause sequences, cut-order optimisation; images are never cut |
| USB cutter communication | 🚧 Partial / verified on Cameo 1 & 2 | Full libusb transport (`usblp` auto-detach) + job runner; Cameo 1 (`0b4d:1121`) and Cameo 2 (`0b4d:112b`) verified live on Linux; Cameo 4 (`0b4d:1137`) Era 3 wakeup, tool detection, and AutoBlade GPGL calibrated |
| Dual-carriage (tool holder 2) | 🚧 Partial / experimental | GPGL sequences and Era 3 tool queries implemented; unverified on live cut |
| Print & Cut (registration marks) | 🚧 Partial / experimental | Type 1 marks on the canvas and in SVG export; GPGL mark search (automatic or manual) checked against inkscape-silhouette's values. **Not yet tested on a cutter** |
| Printing | ❌ Not yet implemented | Export SVG (marks included) and print it from another program |
| Bluetooth | ❌ Not yet implemented | |
| PixScan | ❌ Not yet implemented | |
| Rhinestones | ❌ Not yet implemented | |
| Design store / cloud library | ❌ Not planned | Local files only, by design |
| Virtual cutter & tool-path preview | ✅ Implemented | Test jobs without hardware; simulated strokes shown on the canvas |
| `ss-cli` batch tools | ✅ Implemented | `devices`, `probe`, `inspect`, `convert`, `batch`, `fonts`, `text`, `send` |

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

## Installation

Silhouette Studio for Linux is built from source. It needs **Rust 1.88 or newer**
and a few system libraries. Pick your distribution below, install the build
dependencies, then run the installer.

### 1. Install Rust

Use [rustup](https://rustup.rs) (distribution Rust packages are often too old,
especially on Debian and Ubuntu LTS):

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Open a new terminal afterwards so `cargo` is on your `PATH`.

### 2. Install build dependencies

| Distribution | Command |
| --- | --- |
| Debian, Ubuntu, Linux Mint, Pop!_OS, elementary, Zorin, Raspberry Pi OS | `sudo apt install build-essential pkg-config git libusb-1.0-0-dev libxkbcommon-dev libwayland-dev libxcb1-dev libgl1-mesa-dev xdg-desktop-portal` |
| Fedora, Nobara, RHEL / Rocky / Alma (with EPEL) | `sudo dnf install gcc pkgconf-pkg-config git libusb1-devel libxkbcommon-devel wayland-devel libxcb-devel mesa-libGL-devel xdg-desktop-portal` |
| openSUSE Tumbleweed / Leap | `sudo zypper install gcc pkgconf git libusb-1_0-devel libxkbcommon-devel wayland-devel libxcb-devel Mesa-libGL-devel xdg-desktop-portal` |
| Arch, Manjaro, EndeavourOS, CachyOS | `sudo pacman -S --needed base-devel git libusb libxkbcommon wayland libxcb mesa xdg-desktop-portal` |
| Void Linux | `sudo xbps-install base-devel pkg-config git libusb-devel libxkbcommon-devel wayland-devel libxcb-devel MesaLib-devel xdg-desktop-portal` |
| Alpine Linux | `sudo apk add build-base pkgconf git libusb-dev libxkbcommon-dev wayland-dev libxcb-dev mesa-dev xdg-desktop-portal` |
| Gentoo | `sudo emerge --ask dev-libs/libusb x11-libs/libxkbcommon dev-libs/wayland x11-libs/libxcb media-libs/mesa sys-apps/xdg-desktop-portal` |
| NixOS / Nix | `nix-shell -p cargo rustc pkg-config libusb1 libxkbcommon wayland libxcb libGL` (run the build from inside this shell) |

`xdg-desktop-portal` provides the Open/Save dialogs. Most desktops (GNOME, KDE,
Xfce, Cinnamon) already have it installed.

### 3. Build and install

```sh
git clone https://github.com/vineofthedead-sudo/silhouette-on-linux.git
cd silhouette-on-linux
sh packaging/install.sh
```

This builds the app, installs `silhouette-studio` and `ss-cli` to `/usr/local/bin`,
adds the menu entry, icon and file associations, and installs the USB rule that
lets you use a cutter without root. It asks for your password with `sudo`.
Unplug and replug the cutter once afterwards.

Other options:

```sh
sh packaging/install.sh --user        # install only for you, into ~/.local (no sudo)
sh packaging/install.sh --prefix DIR  # install somewhere else
sh packaging/install.sh --uninstall   # remove (add --user if you installed with --user)
```

To update later, run `git pull` and then `sh packaging/install.sh` again.

### Arch Linux package (optional)

On Arch-based systems you can build a real pacman package instead:

```sh
cd packaging
BUILDDIR=/tmp/makepkg-ss makepkg -sif
```

`-f` forces a rebuild. Without it, `makepkg` reinstalls the previously built
package and you keep running the old version. `BUILDDIR` is only needed when
the checkout path contains spaces.

### Run without installing

```sh
cargo build --release
./target/release/silhouette-studio          # the GUI
./target/release/ss-cli --help              # the CLI
```

### Cutter permissions

`install.sh` handles this unless you used `--user`. To add the USB rule by hand:

```sh
sudo install -Dm644 packaging/70-silhouette-cutters.rules /etc/udev/rules.d/70-silhouette-cutters.rules
sudo udevadm control --reload-rules
# unplug and replug the cutter
```

Check that the cutter is detected with `ss-cli devices`.
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
