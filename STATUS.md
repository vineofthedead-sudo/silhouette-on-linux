# Project Status

A snapshot of what has been verified and how. Last updated with the V5-style
tool rail, collapsible right dock and mocked Send panel.

## Summary

| Area | State | Evidence |
|---|---|---|
| Editing, canvas, layers | Stable | Editor unit tests; GUI exercised under Xvfb |
| V5 tool rail and dock | Working | `Edit Points`, `Knife`, `Eraser`, and `Auto-Trace` unit-tested in `ss-geom` and `ss-doc`; wired in `ss-app` |
| Text tool | Working | `ss-text` unit tests; created, edited and restyled in the GUI |
| Images, background, Auto-Trace | Working | Real-file tests (612 placed images in 137 files); `Auto-Trace` contour extraction tested on synthetic bitmaps |
| `.studio3` / `.studio` reading | Stable | 178 real-world and 39 stock files parsed; gradients, patterns, strokes, and thumbnail fidelity verified against embedded previews |
| `.craft` read/write | Working | Round-trip tests; checked against the companion project's own code |
| SVG / DXF | Stable | Round-trip tests |
| GPGL encoding | Stable in tests | Golden tests replayed through the interpreter; Era 3 Cameo 4 AutoBlade protocol calibrated from USB capture |
| Print & Cut | **Experimental** | Golden tests against inkscape-silhouette values; virtual cutter only |
| USB cutting | **Cameo 1 & 2 verified live; Cameo 4 calibrated** | Cameo 1 (`0b4d:1121`) and Cameo 2 (`0b4d:112b`) verified live on Linux (`usblp` auto-detach); Cameo 4 (`0b4d:1137`) Era 3 handshake, tool detection, and AutoBlade GPGL calibrated and virtual-cutter tested; GUI Send output remains mocked/gated for safety |

## V5 UI and tool states

The active tool state is owned by the application session via `ss_doc::ToolState`,
tracking `ToolGesture`s with full press, drag and release coordinates in document
millimetres. Switching active tools cleanly cancels any unfinished interaction without
affecting the document history. Double-clicking an ungrouped path enters Edit Points
mode, and double-clicking again returns to Select.

| Tool | Key | State |
|---|---|---|
| Select | V | Working: pick, marquee, move, scale, rotate |
| Edit Points | A | Working: double-click path to enter/exit; drag anchor nodes and cubic Bézier handles live; click segment to insert point; `Delete`/`Backspace` or double-click point to remove; `Break Path`, `Corner`/`Smooth` toggle, `Make Flat`/`Make Curve`, and `Simplify` (with tolerance) in Tool Options and context menu |
| Line, Rectangle, Ellipse | L, R, E | Working: Shift constrains angle/square/circle |
| Polyline | P | Working: successive clicks, double-click or Enter to commit |
| Freehand | F | Working: continuous drag with curve fitting and auto-close |
| Text | T | Working: creates text dialog at click position |
| Knife | K | Working: straight-line slice (`Shift` snaps 45°) or `Ctrl`-drag freehand slice; splits closed and open unlocked paths into independent objects as one undo step (`ss_geom::tools::knife_split`) |
| Eraser | X | Working: Round or Square brush (`Tool Options` radius/shape); subtracts swept brush via `i_overlay` preserving `EvenOdd` holes and trimming open paths as one undo step (`ss_geom::tools::erase`) |

The right dock is a set of collapsible panels: Tool Options (contextual for Edit Points, Knife, Eraser), Page Setup (open), Send (open), Auto-Trace, Properties (with Transform X/Y/W/H, aspect lock, Rotate, Flip H/V), Replicate (directional duplicate, mirror copy, row/col, grid array, Fill Page), and Layers.

- **Auto-Trace**: traces any selected placed raster image into `kurbo::BezPath` cut contours (`ss_geom::trace::trace_mask`), with threshold, corner smoothing, speck-area filter, invert toggle, live canvas preview (`All lines` / `Outer edge`), and `Trace` / `Trace Outer Edge` actions placing the result onto the active layer as one undo step.

- **Page Setup**: cutting mat selection (None, Cameo 12 × 12, Cameo 12 × 24,
  Portrait 8 × 12, Cameo Plus 15 × 15, Cameo Pro 24 × 24, Curio 8.5 × 6, Curio 8.5 × 12;
  session-only display that does not alter media size), media size presets,
  custom width/height/border fields, Portrait / Landscape orientation toggles,
  cut border display, grid visibility and snap controls, Print & Cut registration
  marks (inset, length, thickness, square) and background image controls.
- **Send (Virtual Cutter, Tool Path Preview & `.gpgl` Export)**: 3-step wizard workflow (Material (1) → Action (2) → Send (3)) wired to `ss_gpgl::JobSettings` and `ss_gpgl::encode_stages`:
  - **Material (1)**: Material type dropdown (Cardstock default), user-defined
    material creation ("Add new material type"), and forward navigation.
  - **Action (2)**: Simple, Line Color, Fill Color, and Layer tabs. Simple controls
    cut styles (No Cut, Cut, Cut Edge, Sketch). Line, Fill, and Layer display ordered
    action tables with row reordering (`▲`/`▼`), On/Off toggles, color swatches,
    action dropdowns (Cut, Cut Edge, Sketch, No Cut), Tool 1 / Tool 2 holder
    assignments, and right-click pause insertion between stages (`H`). Includes
    Cameo condition limits from `condition.csv` (force 2–33, speed 1–10, passes
    1–2, blade depth 0.1–0.9 emitted as `TC`), Line Segment Overcut toggle, and
    restore defaults.
  - **Send (3)**: Setup instructions, Print & Cut registration marks toggle and
    alignment (`Auto Search` / `Manual`), and Send action.
  - **Send to (Output controls)**: Reconnected non-hardware backends for **Virtual
    Cutter** simulation, live **Tool Path Preview** overlay on the canvas (aligned
    to registration marks when enabled), and **Export `.gpgl` File…**. Live USB
    device scanning and cutting remain disabled behind a safe mock toggle until
    physical hardware testing.

### Remaining items

- USB hardware validation: live USB transport and Print & Cut optical mark
  search remain untested on physical hardware (use `ss-cli send` or `ss-device`
  when a cutter is attached).

## Tests

- `cargo test --workspace --exclude ss-device`:
  all workspace tests including `user_fixtures_parse` (178 files, 79,452 shapes)
  pass; `SharedBytes` (`Arc<[u8]>`) sharing reduced `user_fixtures_parse` peak RSS
  from ~6.6 GB to ~655 MB. `ss-device` was excluded during UI work so no live USB
  enumeration runs.
- `cargo clippy --workspace --all-targets`: no warnings.
- Real-file tests read the stock files from `SS_STOCK_DIR` and the extra
  fixtures from `SS_FIXTURES_DIR`; they skip themselves when the folders are
  absent.

## Hardware

Cameo 1 (`0x0b4d:0x1121`, firmware `CAMEO V1.10`) and Cameo 2 (`0x0b4d:0x112b`,
firmware `CAMEO 2 V1.20`) have been verified live on Linux over USB (`Class=7`
`usblp` auto-detach, Bulk OUT `0x01` / IN `0x82`). Cameo 4 (`0x0b4d:0x1137`,
firmware `CAMEO 4 V1.20`) Era 3 wakeup (`\x1b\x11`), optical tool queries
(`\x1b\x15` / `\x1b\x16`), and AutoBlade GPGL (`TG1`, `TJ`, `TF`, `L0`) have
been calibrated from USB captures and verified against the virtual cutter and
`ss-cli probe`. Always test first on scrap material with low force; see
[DISCLAIMER.md](DISCLAIMER.md).

## GUI checks

Run by hand under Xvfb with a debug build:

- V5 rail: draw a rectangle; Edit Points shows its four corners; Knife and
  Eraser draw a preview while dragging, and the object count is unchanged
  after release; `K` switches to Knife;
- dock: collapse and expand Page Setup; Send Line and Fill tabs group the
  rectangle by colour; choosing Chipboard sets 33 / 1 / 2 / 0.5; the
  12 × 24 mat is drawn under the page;
- open `.studio3` and `.craft` files with embedded images; images render in
  place;
- open an SVG with a background image; remove it and undo;
- turn on registration marks and change their inset; the marks move;
- create multi-line text, edit it, change font and style; placement is kept;
- group by colour (images stay out of the groups);
- send to the virtual cutter with registration; the tool path lines up with
  the design and is clipped to the mark area.

Not checked: the actions that open a file picker (*Place image…*, *Choose
image…*, *Export Craft project…*), because the portal dialog cannot be driven
in the test display. Saving `.craft` uses the same code as `ss-cli convert`,
which was checked, and the PNG/JPEG loader they share has its own unit test.
