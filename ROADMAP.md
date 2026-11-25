# Status & Roadmap

This document tracks what the project can do today, its known limitations, and
where it goes next. Last updated for the initial public release.

## Current capabilities

### Design & editing
- Canvas with mat/page rendering, cut border, configurable grid and snapping,
  pan/zoom with actual-size mode.
- Tools: select (click/marquee/multi, move/scale/rotate handles), rectangle,
  ellipse, line, polyline, freehand (Bézier-fitted).
- Editing: undo/redo, duplicate, group/ungroup, compound paths, weld,
  subtract, intersect, offset (round/miter/bevel, negative supported),
  align/distribute, z-order arrange, layers, cut-action per object
  (cut / cut edge / sketch / perforate / no cut), stroke color and fill.

### File formats
- **Read:** `.studio3` and legacy `.studio`, SVG/SVGZ (gzip + UTF-16 aware),
  DXF, native `.ssrs`.
- **Write:** `.ssrs`, SVG, SVGZ, DXF.
- Validated against 39 stock Silhouette files and 137 additional real-world
  `.studio3` files, compared shape-by-shape against reference SVGs (see
  `crates/ss-formats/tests/real_files.rs`).

### Cutting
- GPGL encoding with force, speed, passes, blade offset, overcut, perforation
  dashes, track enhancing, end-of-job action, and off-media handling
  (refuse / skip / clip at the media edge).
- Inside-first, travel-optimised cut ordering.
- USB transport (libusb) with device enumeration, endpoint detection, stall
  handling, and a cancellable background job runner with progress events.
- Model database: Craft Robo CC200/CC300, Silhouette SD, Cameo 1–4 (+Plus/Pro),
  Portrait 1–3, Cameo 5 Alpha / Alpha Plus, Curio.
- Virtual cutter: simulates a machine end to end and draws the resulting tool
  path back onto the canvas — the primary way to test without hardware.
- `ss-cli` for scripting: `devices`, `inspect`, `convert`, `send`.

## Test coverage

- 66 tests passing workspace-wide; clippy-clean.
- Geometry unit tests (boolean, offset, fit, flatten).
- Editor tests (history, booleans, arrange/align, JSON round trip).
- Format tests (detection, round trips, real-file integration).
- GPGL golden tests replayed through the interpreter (10 tests + a
  benchmark-gated large-file encode, ~7k shapes in ~0.9 s debug).
- Device runner tests against the virtual cutter (stalls, busy, unload,
  cancellation).
- CLI end-to-end smoke test (inspect/convert/send/virtual/file).
- GUI exercised interactively (Xvfb): file open, drawing, selection,
  drag-move, undo/redo, panels, send workflows.

## Known limitations

- **USB sending has not been validated on real hardware yet.** The protocol
  implementation follows documented GPGL and prior art, and the virtual
  cutter round trip passes, but the first physical cut may need timing or
  handshake adjustments.
- Dual-carriage (tool holder 2) GPGL sequences are implemented but unverified.
- No text tool: text outlines inside imported files render and cut, but text
  cannot be created or edited.
- Raster image objects are parsed but not rendered; no auto-trace.
- `.studio3` writing is unsupported (the format is only partially documented;
  use `.ssrs` or SVG/DXF export instead).
- No Print & Cut (registration marks), printing, PixScan, rhinestones,
  nesting, knife/eraser/warp tools, Bluetooth, or cloud library.
- File dialogs go through the XDG desktop portal; on minimal window managers
  a portal backend must be installed.

## Milestones

1. **Hardware validation** — cut a known pattern on a real Cameo/Portrait;
   verify handshake, status polling, force/speed ranges; publish a hardware
   compatibility list. *(next)*
2. **Text tool** — font discovery, editing, conversion to outlines.
3. **Images** — render placed raster images, basic auto-trace to outlines.
4. **Print & Cut** — registration-mark printing and detection workflow.
5. **Distribution** — AUR package, release tarballs, screenshots in the
   README, issue templates.
