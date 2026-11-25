# Status & Roadmap

This document tracks what the project can do today, its known limitations, and
where it goes next. For what has been verified and how, see
[STATUS.md](STATUS.md).

## Current capabilities

### Design & editing
- Canvas with mat/page rendering, cut border, configurable grid and snapping,
  pan/zoom with actual-size mode.
- Tools: select (click/marquee/multi, move/scale/rotate handles), rectangle,
  ellipse, line, polyline, freehand (Bézier-fitted), text.
- Editing: undo/redo, duplicate, group/ungroup, group by colour, compound
  paths, weld, subtract, intersect, offset (round/miter/bevel, negative
  supported), align/distribute, z-order arrange, layers, cut-action per object
  (cut / cut edge / sketch / perforate / no cut), stroke color and fill.
- Text: installed system fonts, bold/italic (slanted when the family has no
  italic), em size, letter and line spacing, multi-line, re-editable, convert
  to paths.
- Images: place PNG/JPEG, render images embedded in `.studio3` and `.craft`
  files, page background image. Images are displayed and exported, never cut.
- Print & Cut: Silhouette "type 1" registration marks (square plus two
  L-brackets) with adjustable inset, length, thickness and square size.

### File formats
- **Read:** `.studio3` and legacy `.studio` (including placed images),
  `.craft`, SVG/SVGZ (gzip + UTF-16 aware, PNG/JPEG `<image>`), DXF, native
  `.ssrs`.
- **Write:** `.ssrs`, `.craft`, SVG, SVGZ, DXF.
- Validated against 39 stock Silhouette files and 137 additional real-world
  `.studio3` files, compared shape-by-shape against reference SVGs (see
  `crates/ss-formats/tests/real_files.rs`).
- `.craft` interchange checked in both directions against the companion
  project's own reader and writer.

### Cutting
- GPGL encoding with force, speed, passes, blade offset, overcut, perforation
  dashes, track enhancing, end-of-job action, and off-media handling
  (refuse / skip / clip at the media or registration-mark edge).
- Registration-mark search (`TB` sequence with automatic or manual search);
  cut coordinates measured from the top-left mark; the job runner waits for the
  cutter's reply before sending the cut.
- Inside-first, travel-optimised cut ordering.
- USB transport (libusb) with device enumeration, endpoint detection, stall
  handling, and a cancellable background job runner with progress events.
- Model database: Craft Robo CC200/CC300, Silhouette SD, Cameo 1–4 (+Plus/Pro),
  Portrait 1–3, Cameo 5 Alpha / Alpha Plus, Curio.
- Virtual cutter: simulates a machine end to end (including the mark search)
  and draws the resulting tool path back onto the canvas — the primary way to
  test without hardware.
- `ss-cli` for scripting: `devices`, `inspect`, `convert`, `batch`, `fonts`,
  `text`, `send`.

## Known limitations

- **No cutting has been validated on real hardware yet.** The protocol
  implementation follows documented GPGL and prior art, and the virtual
  cutter round trip passes, but the first physical cut may need timing or
  handshake adjustments.
- **Print & Cut is experimental.** The registration sequence matches
  inkscape-silhouette's values in golden tests, but the mark search, its
  timeout, and the coordinate offset are unverified on a machine.
- Dual-carriage (tool holder 2) GPGL sequences are implemented but unverified.
- Text layout is simple: left-aligned lines, `kern`-table kerning, no
  ligatures, complex scripts, or right-to-left text. Only installed system
  fonts are available.
- Images cannot be traced into cut lines. Placed images default to 96 dpi.
- No built-in printing: export SVG (marks included) and print it elsewhere.
- `.studio3` writing is unsupported (the format is only partially documented;
  use `.ssrs`, `.craft`, or SVG/DXF export instead).
- No PixScan, rhinestones, nesting, knife/eraser/warp tools, Bluetooth, or
  cloud library.
- File dialogs go through the XDG desktop portal; on minimal window managers
  a portal backend must be installed.

## Milestones

1. **Hardware validation** — cut a known pattern on a real Cameo/Portrait;
   verify handshake, status polling, force/speed ranges, and the Print & Cut
   mark search; publish a hardware compatibility list. *(next; needs a
   volunteer with a cutter)*
2. **Printing** — print the design with registration marks directly from the
   app.
3. **Image trace** — turn raster images into cut outlines.
4. **Text improvements** — alignment, text on a path, complex shaping.
5. **Distribution** — AUR package, release tarballs, screenshots in the
   README, issue templates.
