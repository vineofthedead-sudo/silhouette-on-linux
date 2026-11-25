# Phase 2 — Rust workspace proposal

Status: **awaiting approval**. No application code has been written yet.

Toolchain: Rust 1.100 (Arch), edition 2024. Crate versions below were checked against crates.io.

## Workspace layout

```
silhouette-studio/                     (workspace root = this project folder)
├── Cargo.toml                         workspace manifest, shared deps and lints
├── crates/
│   ├── ss-geom/        vector engine: paths, transforms, flatten/refit, booleans, offset
│   ├── ss-doc/         document model: Document → Page → Layer → Shape, undo stack, selection
│   ├── ss-formats/     .studio3 reader, SVG import/export, DXF import/export, native save format
│   ├── ss-gpgl/        GPGL encoder (job header, M/D/BZ geometry, regmarks) + reply parser
│   ├── ss-device/      transports: USB (rusb), virtual cutter, file dump; device table; job runner thread
│   ├── ss-config/      settings, recent files, paths (directories crate), device/material tables
│   └── ss-app/         eframe/egui desktop app: canvas, tools, panels, send dialog  (binary: silhouette-studio)
├── tools/
│   └── ss-cli/         headless CLI: inspect .studio3, convert to SVG, send a file to a cutter
├── packaging/
│   ├── 70-silhouette-cutters.rules    udev rule (VID 0x0b4d, TAG+="uaccess")
│   ├── silhouette-studio.desktop
│   └── PKGBUILD
├── tests/fixtures/                    our own small test files (no Silhouette assets)
└── docs/
```

Dependency direction: `ss-geom` ← `ss-doc` ← `ss-formats` / `ss-gpgl` ← `ss-device` ← `ss-app`.
`ss-config` is used by `ss-device` and `ss-app`. Only `ss-app` depends on the GUI stack, so every
other crate builds and tests headless.

## Crate responsibilities

| Crate | Key contents | Main deps |
|---|---|---|
| `ss-geom` | `BezPath` wrappers on kurbo; affine transforms; bounds/hit-test; adaptive flatten (`AsLinear`); cubic refit (`CreateFromLinear_adaptive` equivalent); weld/subtract/intersect/divide via polygon booleans (same flatten → boolean → refit pipeline the original uses); offset/inset with round/miter joins | `kurbo`, `i_overlay` |
| `ss-doc` | Shapes (path, text, group, autoshape), layers incl. cut-action layers, pages/mat/media size, cut settings per shape, command-based undo/redo | `ss-geom`, `uuid`, `serde` |
| `ss-formats` | `.studio3` container (`silhouette05;` header, 16×u64 section table, watermark/xor decode) and incremental studio-section field decoder; SVG import (usvg normalises transforms, styles and shapes) and SVG export; DXF import/export; our own `.ssrs` save format (versioned JSON or binary) | `usvg`, `dxf`, `flate2`, `byteorder`, `png` |
| `ss-gpgl` | Pure functions: document + cut settings → GPGL byte stream (`FU`, `FC`, `FX`, `!`, `FN`, `TB99`, `M`, `D`, `BZ`, `H`, ETX framing, 1/20 mm units); reply parsing (`ESC 0x05` status, `FG` firmware) | `ss-doc` |
| `ss-device` | `Transport` trait; `UsbTransport` (enumerate VID 0x0b4d, claim interface 0, detect bulk endpoints, chunked writes, status polling); `VirtualCutter` (records/renders output for tests and preview); `FileTransport` (write `.gpgl`); background job thread with progress/cancel | `rusb`, `crossbeam-channel` |
| `ss-config` | `~/.config/silhouette-studio/settings.toml`, `~/.local/share/silhouette-studio/` (library, device and material tables we write ourselves), `~/.cache/silhouette-studio/` | `directories`, `toml`, `serde` |
| `ss-app` | Canvas with pan/zoom, mat and grid, selection with transform handles, draw tools (line, rect, ellipse, polygon, bezier pen, text later), node edit, layers panel, modify panel (weld/subtract/offset), Send panel with device picker, file dialogs | `eframe`, `egui`, `rfd` |

## Root `Cargo.toml`

```toml
[workspace]
resolver = "3"
members = ["crates/*", "tools/ss-cli"]

[workspace.package]
version = "0.1.0"
edition = "2024"
rust-version = "1.85"
license = "GPL-3.0-or-later"

[workspace.dependencies]
# internal
ss-geom    = { path = "crates/ss-geom" }
ss-doc     = { path = "crates/ss-doc" }
ss-formats = { path = "crates/ss-formats" }
ss-gpgl    = { path = "crates/ss-gpgl" }
ss-device  = { path = "crates/ss-device" }
ss-config  = { path = "crates/ss-config" }

# geometry
kurbo     = "0.13"
i_overlay = "9.2"

# formats
usvg      = "0.50"
dxf       = "0.6"
flate2    = "1.1"
byteorder = "1.5"
png       = "0.18"

# device / platform
rusb              = "0.9"
crossbeam-channel = "0.5"
directories       = "6"

# GUI (ss-app only); default features give wgpu/glow plus Wayland and X11 via winit
eframe = { version = "0.37", features = ["wayland", "x11"] }
egui   = "0.37"
rfd    = "0.18"

# common
serde              = { version = "1", features = ["derive"] }
serde_json         = "1"
toml               = "1.2"
uuid               = { version = "1.29", features = ["v4", "serde"] }
thiserror          = "2"
anyhow             = "1"
tracing            = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
clap               = { version = "4.6", features = ["derive"] }

# dev
proptest = "1.12"

[workspace.lints.rust]
unsafe_code = "forbid"

[profile.release]
lto = "thin"
codegen-units = 1
```

## Build order (Phase 3), `cargo check` + tests after each step
1. Workspace skeleton, `ss-config`, udev rule.
2. `ss-geom`: paths, transforms, flatten, booleans, offset, refit (property tests).
3. `ss-doc`: model and undo.
4. `ss-formats`: SVG and DXF import/export first, then `.studio3` container and watermark decode, then the
   studio-section decoder, tested against the 39 sample files without copying them into the repo.
5. `ss-gpgl` + `ss-device` virtual cutter, with golden-output tests for the command stream.
6. `ss-device` USB backend (hotplug-aware enumeration, endpoint discovery).
7. `ss-app`: canvas, tools, panels, send dialog.
8. `ss-cli` and packaging (PKGBUILD, .desktop, udev rule install).

## Decisions taken in this proposal
- **egui/eframe** over iced: immediate mode suits a canvas editor with many live overlays, and eframe has
  mature Wayland + X11 support through winit.
- **kurbo + i_overlay** instead of lyon for editing math: kurbo has the curve operations (nearest point,
  arc length, subdivision); i_overlay gives robust union/difference/xor with even-odd and non-zero rules.
  lyon is only worth adding if we need GPU tessellation of fills, so it is left out for now.
- No GTK dependency: `rfd` uses the XDG desktop portal for file dialogs.
- The port ships no Silhouette resources (encrypted DBs, fonts, designs). Device PIDs and cut presets come
  from our own tables.
