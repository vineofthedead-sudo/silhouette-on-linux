//! GPGL, the Graphtec command language spoken by Silhouette cutters.
//!
//! Commands are ASCII, each terminated by ETX (`0x03`). Device control sequences are `ESC` plus one
//! byte and carry no terminator. Coordinates are integer cutter units of 1/20 mm, written as
//! `y,x`: the first value runs along the feed direction (down the mat), the second across it.
//!
//! The command set follows the literals observed in the original application
//! (`SendToCutter_spec_gpgl_generate`, `execute_fast_path_*`, the virtual-cutter table; see
//! `docs/PHASE1_INVESTIGATION.md` §5.2). Items marked *public* come from published open-source
//! drivers rather than the original binary, and need checking on real hardware.

pub mod job;
pub mod plan;
pub mod reply;
pub mod sim;

pub use job::{EncodeError, EndAction, Job, JobSettings, JobStats, OffMedia, Perforation, Tool, ToolSettings, document_paths, encode};
pub use reply::{Status, parse_firmware, parse_status};
pub use sim::{Simulation, simulate};

/// Command terminator.
pub const ETX: u8 = 0x03;
pub const ESC: u8 = 0x1b;
/// Cutter units per millimetre.
pub const UNITS_PER_MM: f64 = 20.0;

/// Raw control sequences (no ETX).
pub mod control {
    /// Initialise the device (*public*).
    pub const INITIALIZE: &[u8] = &[super::ESC, 0x04];
    /// Status poll; the reply is a digit plus ETX, see [`crate::parse_status`].
    pub const STATUS: &[u8] = &[super::ESC, 0x05];
    /// Firmware version query; the reply looks like ` V3.00`.
    pub const FIRMWARE: &[u8] = b"FG\x03";
}

/// Millimetres to cutter units.
pub fn to_units(mm: f64) -> i64 {
    (mm * UNITS_PER_MM).round() as i64
}

/// Cutter units to millimetres.
pub fn from_units(units: f64) -> f64 {
    units / UNITS_PER_MM
}
