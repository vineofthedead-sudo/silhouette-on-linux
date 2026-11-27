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

The active tool lives in `ss_doc::Tool`. The canvas owns the current tool,
cancels any unfinished drag or polyline when the tool changes, and sends each
press, drag and release to that tool's handler.

| Tool | Key | State |
|---|---|---|
| Select | V | Working: pick, marquee, move, scale, rotate |
| Edit Points | A | **Inspection only**: selects a path and shows its end points; no point moving, adding, deleting, breaking, corner/smooth or simplify |
| Line, Rectangle, Ellipse | L, R, E | Working |
| Polyline | P | Working (kept from the earlier rail) |
| Freehand | F | Working |
| Text | T | Working |
| Knife | K | **Preview only**: draws the cut line; the document is not changed |
| Eraser | X | **Preview only**: draws the eraser stroke; the document is not changed |

The right dock is a set of collapsible panels: Page Setup (open), Send (open),
Properties and Layers.

- **Page Setup**: cutting mat choice (None, Cameo 12 × 12, Cameo 12 × 24,
  Portrait 8 × 12; session-only display that does not change the media size),
  media presets, size, cut border, grid, Print & Cut marks and background.
- **Send (mocked)**: Simple, Line and Fill tabs. Simple chooses No Cut, Cut or
  Cut Edge. Line and Fill list visible objects grouped by line or fill colour,
  each with an on/off box. Material presets and limits come from the Cameo rows
  of `SPEC_S/condition.csv`: force 2–33, speed 1–10, passes 1–2, blade depth
  0.1–0.9. The settings are kept for the session only and the Send button is
  disabled. The panel makes no USB or device calls.

The earlier working sender (`send.rs`: virtual cutter, GPGL file, USB, tool
path preview) is kept in the source but is not shown in the GUI. Use
`ss-cli send` for virtual-cutter and file jobs until it is wired back in.

### Missing geometry

- Eraser: buffer the stroke into a closed outline (offset of the polyline by the
  eraser radius) and subtract it from the paths it touches with
  `ss_geom::boolean`, as one undo step.
- Knife: split closed and open paths along the knife line into separate
  objects, closing the cut edges of filled shapes.
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
