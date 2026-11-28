//! Known cutter models.
//!
//! USB IDs and cutting widths are public facts collected by open-source cutter drivers; the
//! original application keeps its own table in an encrypted database. Unknown products from the
//! Silhouette vendor are still offered, under a generic name.

/// Silhouette / Graphtec USB vendor ID.
pub const VENDOR_GRAPHTEC: u16 = 0x0b4d;
/// Vendor ID used by the Cameo 5 Alpha models.
pub const VENDOR_CAMEO5_ALPHA: u16 = 0x3844;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProtocolEra {
    /// Classic single-carriage GPGL (Cameo 1, Cameo 2, Portrait 1, SD, Craft Robo).
    #[default]
    Classic,
    /// Dual-carriage GPGL with `J1`/`J2` (Cameo 3, Portrait 2).
    DualCarriage,
    /// Cameo 4 / 5 / Portrait 3+ optical tool detection & AutoBlade `TJ`/`TF` setup.
    Era3,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Model {
    pub name: &'static str,
    pub vendor_id: u16,
    pub product_id: u16,
    /// Maximum cutting width in mm.
    pub width_mm: f64,
    /// Two carriages (tool holders 1 and 2).
    pub two_tools: bool,
    pub era: ProtocolEra,
}

impl Model {
    pub fn gpgl_protocol(self) -> ss_gpgl::Protocol {
        match self.era {
            ProtocolEra::Era3 => ss_gpgl::Protocol::Era3,
            ProtocolEra::Classic | ProtocolEra::DualCarriage => ss_gpgl::Protocol::Classic,
        }
    }
}

const fn m(name: &'static str, vendor_id: u16, product_id: u16, width_mm: f64, two_tools: bool, era: ProtocolEra) -> Model {
    Model { name, vendor_id, product_id, width_mm, two_tools, era }
}

pub const MODELS: &[Model] = &[
    m("Craft Robo CC200-20", VENDOR_GRAPHTEC, 0x110a, 200.0, false, ProtocolEra::Classic),
    m("Craft Robo CC300-20", VENDOR_GRAPHTEC, 0x111a, 304.0, false, ProtocolEra::Classic),
    m("Silhouette SD 1", VENDOR_GRAPHTEC, 0x111c, 304.0, false, ProtocolEra::Classic),
    m("Silhouette SD 2", VENDOR_GRAPHTEC, 0x111d, 304.0, false, ProtocolEra::Classic),
    m("Silhouette Cameo", VENDOR_GRAPHTEC, 0x1121, 304.0, false, ProtocolEra::Classic),
    m("Silhouette Portrait", VENDOR_GRAPHTEC, 0x1123, 206.0, false, ProtocolEra::Classic),
    m("Silhouette Cameo 2", VENDOR_GRAPHTEC, 0x112b, 304.0, false, ProtocolEra::Classic),
    m("Silhouette Cameo 3", VENDOR_GRAPHTEC, 0x112f, 304.8, true, ProtocolEra::DualCarriage),
    m("Silhouette Portrait 2", VENDOR_GRAPHTEC, 0x1132, 203.0, false, ProtocolEra::DualCarriage),
    m("Silhouette Cameo 4", VENDOR_GRAPHTEC, 0x1137, 304.8, true, ProtocolEra::Era3),
    m("Silhouette Cameo 4 Plus", VENDOR_GRAPHTEC, 0x1138, 372.0, true, ProtocolEra::Era3),
    m("Silhouette Cameo 4 Pro", VENDOR_GRAPHTEC, 0x1139, 600.0, true, ProtocolEra::Era3),
    m("Silhouette Portrait 3", VENDOR_GRAPHTEC, 0x113a, 203.0, false, ProtocolEra::Era3),
    m("Silhouette Portrait 4", VENDOR_GRAPHTEC, 0x113f, 216.0, false, ProtocolEra::Era3),
    m("Silhouette Cameo 5", VENDOR_GRAPHTEC, 0x1140, 330.2, true, ProtocolEra::Era3),
    m("Silhouette Cameo 5 Plus", VENDOR_GRAPHTEC, 0x1141, 372.0, true, ProtocolEra::Era3),
    m("Silhouette Cameo Pro MK II", VENDOR_GRAPHTEC, 0x1146, 609.0, true, ProtocolEra::Era3),
    m("Silhouette Cameo 5 Alpha", VENDOR_CAMEO5_ALPHA, 0x0001, 330.2, true, ProtocolEra::Era3),
    m("Silhouette Cameo 5 Alpha Plus", VENDOR_CAMEO5_ALPHA, 0x0002, 372.0, true, ProtocolEra::Era3),
];

pub fn find_model(vendor_id: u16, product_id: u16) -> Option<&'static Model> {
    MODELS.iter().find(|m| m.vendor_id == vendor_id && m.product_id == product_id)
}

/// Whether a USB device should be offered as a cutter.
pub fn is_cutter(vendor_id: u16, product_id: u16) -> bool {
    vendor_id == VENDOR_GRAPHTEC || find_model(vendor_id, product_id).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup() {
        assert_eq!(find_model(0x0b4d, 0x1137).unwrap().name, "Silhouette Cameo 4");
        assert!(find_model(0x0b4d, 0xffff).is_none());
        assert!(is_cutter(0x0b4d, 0xffff));
        assert!(is_cutter(0x3844, 0x0001));
        assert!(!is_cutter(0x3844, 0x0099));
        let mut ids: Vec<_> = MODELS.iter().map(|m| (m.vendor_id, m.product_id)).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), MODELS.len());
    }
}
