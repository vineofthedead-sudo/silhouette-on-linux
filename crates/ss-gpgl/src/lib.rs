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

pub use job::{
    EncodeError, EndAction, Job, JobSettings, JobStage, JobStats, OffMedia, Perforation, Protocol, Registration, Tool, ToolSettings,
    cut_edge_paths, document_paths, encode, encode_stages,
};
pub use reply::{DetectedTool, Status, parse_firmware, parse_registration, parse_status, parse_tool_sensor};
pub use sim::{SimRegistration, Simulation, simulate};

/// Command terminator.
pub const ETX: u8 = 0x03;
pub const ESC: u8 = 0x1b;
/// Cutter units per millimetre.
pub const UNITS_PER_MM: f64 = 20.0;

/// Raw control and query sequences.
pub mod control {
    /// Initialise the device (*public*).
    pub const INITIALIZE: &[u8] = &[super::ESC, 0x04];
    /// Status poll; the reply is a digit plus ETX, see [`crate::parse_status`].
    pub const STATUS: &[u8] = &[super::ESC, 0x05];
    /// Wakeup / model identification query (Cameo 4 returns `"CAMEO 4 V1.20    \x03"`).
    pub const WAKEUP: &[u8] = &[super::ESC, 0x11];
    /// Carriage 1 optical tool-sensor query (`" 0, 0\x03"` empty, `" 2, 0\x03"` AutoBlade).
    pub const TOOL_QUERY_1: &[u8] = &[super::ESC, 0x15];
    /// Carriage 2 optical tool-sensor query.
    pub const TOOL_QUERY_2: &[u8] = &[super::ESC, 0x16];
    /// Firmware version query; the reply looks like ` V3.00` or `CAMEO 4 V1.20`.
    pub const FIRMWARE: &[u8] = b"FG\x03";
    pub const TI_QUERY: &[u8] = b"TI\x03";
    pub const TO_QUERY: &[u8] = b"TO\x03";
    pub const TB71_QUERY: &[u8] = b"TB71\x03";
    pub const FA_QUERY: &[u8] = b"FA\x03";
}

/// Millimetres to cutter units.
pub fn to_units(mm: f64) -> i64 {
    (mm * UNITS_PER_MM).round() as i64
}

/// Cutter units to millimetres.
pub fn from_units(units: f64) -> f64 {
    units / UNITS_PER_MM
}
