//! USB transport over libusb (`rusb`).

use std::time::Duration;

use rusb::{Device, DeviceHandle, Direction, GlobalContext, TransferType, UsbContext};

use crate::models::{Model, find_model, is_cutter};
use crate::{DeviceError, Transport};

#[derive(Debug, Clone, PartialEq)]
pub struct UsbDeviceInfo {
    pub bus: u8,
    pub address: u8,
    pub vendor_id: u16,
    pub product_id: u16,
    pub model: Option<&'static Model>,
}

impl UsbDeviceInfo {
    pub fn name(&self) -> String {
        match self.model {
            Some(m) => m.name.to_string(),
            None => format!("Silhouette device {:04x}:{:04x}", self.vendor_id, self.product_id),
        }
    }
}

/// Lists connected cutters.
pub fn enumerate() -> Result<Vec<UsbDeviceInfo>, DeviceError> {
    let mut out = Vec::new();
    for dev in GlobalContext::default().devices()?.iter() {
        let Ok(d) = dev.device_descriptor() else { continue };
        if is_cutter(d.vendor_id(), d.product_id()) {
            out.push(UsbDeviceInfo {
                bus: dev.bus_number(),
                address: dev.address(),
                vendor_id: d.vendor_id(),
                product_id: d.product_id(),
                model: find_model(d.vendor_id(), d.product_id()),
            });
        }
    }
    Ok(out)
}

/// An endpoint as read from the descriptors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndpointInfo {
    pub interface: u8,
    pub address: u8,
    pub bulk: bool,
    pub input: bool,
    pub max_packet: u16,
}

/// First interface with both a bulk OUT and a bulk IN endpoint: (interface, out, in, packet size).
pub fn pick_endpoints(eps: &[EndpointInfo]) -> Option<(u8, u8, u8, usize)> {
    let mut interfaces: Vec<u8> = eps.iter().map(|e| e.interface).collect();
    interfaces.dedup();
    interfaces.into_iter().find_map(|i| {
        let out = eps.iter().find(|e| e.interface == i && e.bulk && !e.input)?;
        let inp = eps.iter().find(|e| e.interface == i && e.bulk && e.input)?;
        Some((i, out.address, inp.address, usize::from(out.max_packet.max(8))))
    })
}

fn endpoints(dev: &Device<GlobalContext>) -> Result<Vec<EndpointInfo>, DeviceError> {
    let cfg = dev.active_config_descriptor()?;
    let mut out = Vec::new();
    for iface in cfg.interfaces() {
        // Alternate setting 0 is the one active after claiming.
        if let Some(alt) = iface.descriptors().next() {
            for ep in alt.endpoint_descriptors() {
                out.push(EndpointInfo {
                    interface: alt.interface_number(),
                    address: ep.address(),
                    bulk: ep.transfer_type() == TransferType::Bulk,
                    input: ep.direction() == Direction::In,
                    max_packet: ep.max_packet_size(),
                });
            }
        }
    }
    Ok(out)
}

pub struct UsbTransport {
    handle: DeviceHandle<GlobalContext>,
    info: UsbDeviceInfo,
    interface: u8,
    ep_out: u8,
    ep_in: u8,
    packet: usize,
}

impl UsbTransport {
    pub fn open(info: &UsbDeviceInfo) -> Result<Self, DeviceError> {
        let dev = GlobalContext::default()
            .devices()?
            .iter()
            .find(|d| d.bus_number() == info.bus && d.address() == info.address)
            .ok_or(DeviceError::NotFound)?;
        // Fall back to the endpoints seen on Silhouette machines if the descriptors are odd.
        let (interface, ep_out, ep_in, packet) = pick_endpoints(&endpoints(&dev)?).unwrap_or((0, 0x01, 0x82, 64));
        let handle = dev.open()?;
        // Not supported on every platform; claiming reports the real problem if a driver is bound.
        let _ = handle.set_auto_detach_kernel_driver(true);
        handle.claim_interface(interface)?;
        Ok(Self { handle, info: info.clone(), interface, ep_out, ep_in, packet })
    }

    /// Opens the first connected cutter.
    pub fn open_first() -> Result<Self, DeviceError> {
        Self::open(enumerate()?.first().ok_or(DeviceError::NotFound)?)
    }

    pub fn info(&self) -> &UsbDeviceInfo {
        &self.info
    }
}

impl Drop for UsbTransport {
    fn drop(&mut self) {
        let _ = self.handle.release_interface(self.interface);
    }
}

impl Transport for UsbTransport {
    fn write(&mut self, data: &[u8], timeout: Duration) -> Result<usize, DeviceError> {
        // One packet per transfer: a timed-out transfer then means that packet was not taken,
        // so the byte count stays exact and nothing is sent twice on retry.
        let mut written = 0;
        for chunk in data.chunks(self.packet) {
            match self.handle.write_bulk(self.ep_out, chunk, timeout) {
                Ok(n) => {
                    written += n;
                    if n < chunk.len() {
                        break;
                    }
                }
                Err(rusb::Error::Timeout) if written > 0 => break,
                Err(e) => return Err(e.into()),
            }
        }
        Ok(written)
    }

    fn read(&mut self, timeout: Duration) -> Result<Vec<u8>, DeviceError> {
        let mut buf = vec![0u8; self.packet.max(64)];
        match self.handle.read_bulk(self.ep_in, &mut buf, timeout) {
            Ok(n) => {
                buf.truncate(n);
                Ok(buf)
            }
            Err(rusb::Error::Timeout) => Ok(Vec::new()),
            Err(e) => Err(e.into()),
        }
    }

    fn description(&self) -> String {
        format!("{} (USB bus {} device {})", self.info.name(), self.info.bus, self.info.address)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(interface: u8, address: u8, bulk: bool, max_packet: u16) -> EndpointInfo {
        EndpointInfo { interface, address, bulk, input: address & 0x80 != 0, max_packet }
    }

    #[test]
    fn endpoint_selection() {
        let eps = [ep(0, 0x81, false, 8), ep(1, 0x02, true, 512), ep(1, 0x83, false, 16), ep(1, 0x84, true, 512)];
        assert_eq!(pick_endpoints(&eps), Some((1, 0x02, 0x84, 512)));
        assert_eq!(pick_endpoints(&[ep(0, 0x01, true, 64)]), None);
        assert_eq!(pick_endpoints(&[ep(0, 0x01, true, 64), ep(0, 0x82, true, 64)]), Some((0, 0x01, 0x82, 64)));
        // Cameo 1 / 2 / 4 Class=7 Printer interface: Bulk OUT 0x01, Bulk IN 0x82, Interrupt IN 0x83 ignored.
        let cameo1 = [ep(0, 0x01, true, 64), ep(0, 0x82, true, 64), ep(0, 0x83, false, 8)];
        assert_eq!(pick_endpoints(&cameo1), Some((0, 0x01, 0x82, 64)));
    }

    #[test]
    fn enumerate_runs() {
        // No cutter has to be connected; this checks that libusb initialises and filtering works.
        match enumerate() {
            Ok(list) => assert!(list.iter().all(|d| is_cutter(d.vendor_id, d.product_id))),
            Err(e) => eprintln!("libusb unavailable here: {e}"),
        }
    }
}
