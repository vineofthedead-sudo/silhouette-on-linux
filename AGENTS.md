# Repository Agent Guidelines

## Hardware & Safety
- **Never** connect to physical USB cutting hardware or run live `ss-device` USB enumeration/cutting tests during automated development unless explicitly instructed by the user.
- Keep live USB hardware access gated/disabled in GUI test flows; use `VirtualCutter` and `ss-gpgl::simulate` for verification.

## Verification
- Before committing, run:
  - `cargo check --workspace`
  - `cargo clippy --workspace --all-targets`
  - `cargo test --workspace --exclude ss-device`

## Mandatory Human QA Handoff
Every final response after completing a task **must** end with a short **"Human QA Checklist (Manual Verification)"** section listing concrete, step-by-step checks for anything headless `Xvfb` cannot verify:
- Window manager behavior (tiling, fullscreen, floating drag via top bar, decorations on Wayland/Niri/X11)
- Native XDG portal file dialogs (open, save, export `.gpgl`/`.svg`/`.craft`, place image)
- Trackpad/touchpad/mouse pinch-zoom, smooth scroll, and interactive feel
- Visual alignment, colors, gradients, patterns, and HiDPI scaling
