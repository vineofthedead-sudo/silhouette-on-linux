# Project Status

A snapshot of what has been verified and how. Last updated with the text,
image, `.craft`, and Print & Cut work.

## Summary

| Area | State | Evidence |
|---|---|---|
| Editing, canvas, layers | Stable | Editor unit tests; GUI exercised under Xvfb |
| Text tool | Working | `ss-text` unit tests; created, edited and restyled in the GUI |
| Images and background | Working | Real-file tests (612 placed images in 137 files); rendered in the GUI |
| `.studio3` / `.studio` reading | Stable | 176 real-world and stock files compared against reference SVGs |
| `.craft` read/write | Working | Round-trip tests; checked against the companion project's own code |
| SVG / DXF | Stable | Round-trip tests |
| GPGL encoding | Stable in tests | Golden tests replayed through the interpreter |
| Print & Cut | **Experimental** | Golden tests against inkscape-silhouette values; virtual cutter only |
| USB cutting | **Untested on hardware** | Job runner tests against the virtual cutter |

## Tests

- `cargo test --workspace`: 92 passed, 0 failed, 1 ignored (a timing
  benchmark).
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
