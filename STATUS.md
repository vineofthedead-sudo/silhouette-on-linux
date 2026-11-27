# Project Status

A snapshot of what has been verified and how. Last updated with the V5-style
tool rail, collapsible right dock and mocked Send panel.

## Summary

| Area | State | Evidence |
|---|---|---|
| Editing, canvas, layers | Stable | Editor unit tests; GUI exercised under Xvfb |
| V5 tool rail and dock | **Partial** | Tool routing and panels exercised under Xvfb; Edit Points, Knife and Eraser geometry missing |
| Text tool | Working | `ss-text` unit tests; created, edited and restyled in the GUI |
| Images and background | Working | Real-file tests (612 placed images in 137 files); rendered in the GUI |
| `.studio3` / `.studio` reading | Stable | 176 real-world and stock files compared against reference SVGs |
| `.craft` read/write | Working | Round-trip tests; checked against the companion project's own code |
| SVG / DXF | Stable | Round-trip tests |
| GPGL encoding | Stable in tests | Golden tests replayed through the interpreter |
| Print & Cut | **Experimental** | Golden tests against inkscape-silhouette values; virtual cutter only |
| USB cutting | **Untested on hardware; hidden in the GUI** | Job runner tests against the virtual cutter; the GUI Send panel is a mock for now |

## V5 UI and tool states

The active tool state is owned by the application session via `ss_doc::ToolState`,
tracking `ToolGesture`s with full press, drag and release coordinates in document
millimetres. Switching active tools cleanly cancels any unfinished interaction without
affecting the document history. Double-clicking an ungrouped path enters Edit Points
mode, and double-clicking again returns to Select.

| Tool | Key | State |
|---|---|---|
| Select | V | Working: pick, marquee, move, scale, rotate |
| Edit Points | A | **Inspection only**: double-click path to enter/exit; inspects endpoints; point moving, adding, deleting, breaking, corner/smooth and simplify queued for next phase |
| Line, Rectangle, Ellipse | L, R, E | Working: Shift constrains angle/square/circle |
| Polyline | P | Working: successive clicks, double-click or Enter to commit |
| Freehand | F | Working: continuous drag with curve fitting and auto-close |
| Text | T | Working: creates text dialog at click position |
| Knife | K | **Preview & stub**: straight drag preview via `ss_geom::tools::knife_preview`; destructive splitting cleanly stubbed with `split_with_knife` (document unchanged) |
| Eraser | X | **Preview & stub**: continuous drag preview via `ss_geom::tools::eraser_preview` with radius indicator; boolean subtraction cleanly stubbed with `subtract_with_eraser` (document unchanged) |

The right dock is a set of collapsible panels: Page Setup (open), Send (open),
Properties and Layers.

- **Page Setup**: cutting mat selection (None, Cameo 12 × 12, Cameo 12 × 24,
  Portrait 8 × 12, Cameo Plus 15 × 15, Cameo Pro 24 × 24, Curio 8.5 × 6, Curio 8.5 × 12;
  session-only display that does not alter media size), media size presets,
  custom width/height/border fields, Portrait / Landscape orientation toggles,
  cut border display, grid visibility and snap controls, Print & Cut registration
  marks (inset, length, thickness, square) and background image controls.
- **Send (mocked)**: 3-step wizard workflow (Material (1) → Action (2) → Send (3)):
  - **Material (1)**: Material type dropdown (Cardstock default), user-defined
    material creation ("Add new material type"), and forward navigation.
  - **Action (2)**: Simple, Line Color, Fill Color, and Layer tabs. Simple controls
    cut styles (No Cut, Cut, Cut Edge). Line, Fill, and Layer display action tables
    with On/Off toggles, color swatches, action dropdowns (Cut, Cut Edge, Sketch, No Cut),
    and Tool 1 / Tool 2 holder assignments. Includes Cameo condition limits from
    `condition.csv` (force 2–33, speed 1–10, passes 1–2, blade depth 0.1–0.9),
    Line Segment Overcut toggle, test cut button (disabled), and restore defaults.
  - **Send (3)**: Setup instructions, registration marks status block with manual
    registration button, and step guidance.
  - **Send to (Output controls)**: Machine checklist (CAMEO 2 - A, B, C, D),
    settings summary, Tool 1 (Blade) / Tool 2 (Pen) holder selection, and a large
    mock Send button. All device communication, scanning and USB access remain
    strictly disabled.

The earlier working sender (`send.rs`: virtual cutter, GPGL file, USB, tool
path preview) is kept in the source but is not shown in the GUI. Use
`ss-cli send` for virtual-cutter and file jobs until it is wired back in.

### Missing geometry

- Eraser: buffer the stroke into a closed outline (offset of the polyline by the
  eraser radius) and subtract it from the paths it touches with
  `ss_geom::boolean`, as one undo step (cleanly stubbed in `ss_geom::tools::subtract_with_eraser`).
- Knife: split closed and open paths along the knife line into separate
  objects, closing the cut edges of filled shapes (cleanly stubbed in
  `ss_geom::tools::split_with_knife`).
- Edit Points: hit-test individual nodes and handles; move, add and delete
  points; break a path; corner/smooth; make flat or curved; simplify.
- Send: connect the Line and Fill groups and per-group conditions to
  `ss_gpgl::JobSettings` (per-pass tool settings, order and pauses), and pass
  blade depth to cutters that support it.

## Tests

- `cargo test --workspace --exclude ss-device -- --skip user_fixtures_parse`:
  89 passed, 0 failed, 1 ignored (a timing benchmark). `ss-device` was left
  out on purpose during the UI work (one of its tests lists USB devices).
  `user_fixtures_parse` now reaches about 6.6 GB of memory on the full
  fixture folder and was killed by `earlyoom` on a machine that was low on
  memory; it needs a memory fix or a smaller corpus.
- `cargo clippy --workspace --all-targets`: no warnings.
- Real-file tests read the stock files from `SS_STOCK_DIR` and the extra
  fixtures from `SS_FIXTURES_DIR`; they skip themselves when the folders are
  absent.

## Hardware

No physical cutter has been available during development. Everything that
talks to a machine has only been run against the virtual cutter, which
replays GPGL as a cutter would and answers status and registration queries.
Reports from people who try it on a real machine (with scrap material and low
force; see [DISCLAIMER.md](DISCLAIMER.md)) are very welcome.

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
