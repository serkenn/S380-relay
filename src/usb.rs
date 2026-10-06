//! A USB transport that can select a *specific* RC-S380 by index.
//!
//! The `felica` crate's `open_port100()` opens the first device matching the
//! vendor/product id, so with two identical RC-S380 readers on one host it can
//! only ever reach one of them. This transport enumerates all matching readers
//! and opens the one at a given index, so a server and a client can each drive
//! their own reader on the same machine. It is handed to `felica::init_port100`
//! in place of the crate's own `UsbTransport`.
//!
//! The open/claim/read/write logic mirrors the crate's `UsbTransport`,
//! including the macOS Address-state configuration handling and the
//! zero-length-packet termination for full bulk writes.

use felica::Transport;
use felica::driver::port100::Device;
use log::debug;
use rusb::{ConfigDescriptor, Direction, GlobalContext, TransferType};
use std::io::{self, Error, ErrorKind};
use std::time::Duration;

/// Sony's USB vendor id.
const SONY_VENDOR_ID: u16 = 0x054C;
/// Port-100 product ids: RC-S380/S, RC-S634/UA, RC-S380/P.
const PORT100_PRODUCT_IDS: [u16; 3] = [0x06C1, 0x06C2, 0x06C3];
/// Write timeout, matching the crate's `UsbTransport`.
const WRITE_TIMEOUT: Duration = Duration::from_millis(100);

/// One attached Port-100 reader, identified for selection and display.
#[derive(Debug, Clone)]
pub struct ReaderLocation {
    pub index: usize,
    pub bus: u8,
    pub address: u8,
    pub product_id: u16,
}

/// Lists every attached Port-100 reader, in a stable bus/address order.
pub fn list_port100() -> io::Result<Vec<ReaderLocation>> {
    let devices = rusb::devices().map_err(rusb_to_io)?;
    let mut found: Vec<(u8, u8, u16)> = Vec::new();
    for device in devices.iter() {
        let desc = match device.device_descriptor() {
            Ok(d) => d,
            Err(_) => continue,
        };
        if desc.vendor_id() == SONY_VENDOR_ID && PORT100_PRODUCT_IDS.contains(&desc.product_id()) {
            found.push((device.bus_number(), device.address(), desc.product_id()));
        }
    }
    found.sort_by_key(|(bus, addr, _)| (*bus, *addr));
    Ok(found
        .into_iter()
        .enumerate()
        .map(|(index, (bus, address, product_id))| ReaderLocation {
            index,
            bus,
            address,
            product_id,
        })
        .collect())
}

/// Bulk IN/OUT endpoints of the claimed interface.
struct BulkEndpoints {
    in_ep: u8,
    out_ep: u8,
    max_packet_size: u16,
}

pub struct RusbTransport {
    handle: Option<rusb::DeviceHandle<GlobalContext>>,
    in_ep: u8,
    out_ep: u8,
    max_packet_size: u16,
    interface: u8,
    manufacturer: Option<String>,
    product: Option<String>,
}

impl RusbTransport {
    /// Opens the Port-100 reader at `index` in [`list_port100`] order.
    pub fn open_indexed(index: usize) -> io::Result<Self> {
        let devices = rusb::devices().map_err(rusb_to_io)?;
        let mut matches: Vec<rusb::Device<GlobalContext>> = devices
            .iter()
            .filter(|device| match device.device_descriptor() {
                Ok(desc) => {
                    desc.vendor_id() == SONY_VENDOR_ID
                        && PORT100_PRODUCT_IDS.contains(&desc.product_id())
                }
                Err(_) => false,
            })
            .collect();
        matches.sort_by_key(|device| (device.bus_number(), device.address()));

        let device = matches.into_iter().nth(index).ok_or_else(|| {
            Error::new(
                ErrorKind::NotFound,
                format!("no Port-100 reader at index {index}"),
            )
        })?;

        let descriptor = device.device_descriptor().map_err(rusb_to_io)?;
        // Read config by index, not active_config_descriptor(): on macOS a
        // freshly opened device can be left in the Address state with no active
        // configuration yet.
        let config = device.config_descriptor(0).map_err(rusb_to_io)?;
        let (interface, endpoints) = select_bulk_endpoints(&config)?;

        let handle = device.open().map_err(rusb_to_io)?;

        if handle.kernel_driver_active(interface).unwrap_or(false)
            && let Err(err) = handle.detach_kernel_driver(interface)
        {
            debug!("failed to detach kernel driver: {:?}", err);
        }

        // Only set the configuration when it is not already active: re-issuing
        // SET_CONFIGURATION acts as a lightweight reset and can corrupt the ACK
        // handshake on Linux, but macOS needs it when the device is in the
        // Address state.
        let needs_configuration = match handle.active_configuration() {
            Ok(active) => active != config.number(),
            Err(err) => {
                debug!("failed to read active configuration: {:?}", err);
                true
            }
        };
        if needs_configuration
            && let Err(err) = handle.set_active_configuration(config.number())
        {
            debug!("failed to set active configuration: {:?}", err);
        }

        handle.claim_interface(interface).map_err(rusb_to_io)?;

        let manufacturer = descriptor
            .manufacturer_string_index()
            .and_then(|idx| handle.read_string_descriptor_ascii(idx).ok());
        let product = descriptor
            .product_string_index()
            .and_then(|idx| handle.read_string_descriptor_ascii(idx).ok());

        Ok(Self {
            handle: Some(handle),
            in_ep: endpoints.in_ep,
            out_ep: endpoints.out_ep,
            max_packet_size: endpoints.max_packet_size,
            interface,
            manufacturer,
            product,
        })
    }
}

/// Opens the Port-100 reader at `index` and wraps it in the felica driver.
pub fn open_port100_indexed(index: usize) -> Result<Device<RusbTransport>, Box<dyn std::error::Error>> {
    let transport = RusbTransport::open_indexed(index)?;
    Ok(felica::init_port100(transport)?)
}

fn select_bulk_endpoints(config: &ConfigDescriptor) -> io::Result<(u8, BulkEndpoints)> {
    let mut interface_number = None;
    let mut in_ep = None;
    let mut out_ep = None;
    let mut max_packet = 0u16;

    'outer: for interface_desc in config.interfaces() {
        for descriptor in interface_desc.descriptors() {
            for endpoint in descriptor.endpoint_descriptors() {
                if endpoint.transfer_type() != TransferType::Bulk {
                    continue;
                }
                match endpoint.direction() {
                    Direction::In if in_ep.is_none() => {
                        in_ep = Some(endpoint.address());
                        max_packet = endpoint.max_packet_size();
                    }
                    Direction::Out if out_ep.is_none() => {
                        out_ep = Some(endpoint.address());
                        if max_packet == 0 {
                            max_packet = endpoint.max_packet_size();
                        }
                    }
                    _ => {}
                }
            }
            if in_ep.is_some() && out_ep.is_some() {
                interface_number = Some(descriptor.interface_number());
                break 'outer;
            }
        }
    }

    let in_ep = in_ep.ok_or_else(|| Error::other("missing bulk IN endpoint"))?;
    let out_ep = out_ep.ok_or_else(|| Error::other("missing bulk OUT endpoint"))?;
    let interface = interface_number.unwrap_or(0);
    Ok((
        interface,
        BulkEndpoints {
            in_ep,
            out_ep,
            max_packet_size: max_packet,
        },
    ))
}

impl Transport for RusbTransport {
    fn write(&mut self, data: &[u8]) -> io::Result<()> {
        let handle = self
            .handle
            .as_mut()
            .ok_or_else(|| Error::new(ErrorKind::NotConnected, "USB handle closed"))?;
        handle
            .write_bulk(self.out_ep, data, WRITE_TIMEOUT)
            .map_err(rusb_to_io)?;
        if self.max_packet_size > 0 && (data.len() as u16).is_multiple_of(self.max_packet_size) {
            handle
                .write_bulk(self.out_ep, &[], WRITE_TIMEOUT)
                .map_err(rusb_to_io)?;
        }
        Ok(())
    }

    fn read(&mut self, timeout: Duration) -> io::Result<Vec<u8>> {
        let handle = self
            .handle
            .as_mut()
            .ok_or_else(|| Error::new(ErrorKind::NotConnected, "USB handle closed"))?;
        let mut buffer = [0u8; 512];
        let len = handle
            .read_bulk(self.in_ep, &mut buffer, timeout)
            .map_err(rusb_to_io)?;
        if len == 0 {
            return Err(Error::new(
                ErrorKind::UnexpectedEof,
                "USB bulk read returned zero",
            ));
        }
        Ok(buffer[..len].to_vec())
    }

    fn close(&mut self) -> io::Result<()> {
        if let Some(handle) = self.handle.take() {
            let _ = handle.release_interface(self.interface);
        }
        Ok(())
    }

    fn manufacturer_name(&self) -> Option<&str> {
        self.manufacturer.as_deref()
    }

    fn product_name(&self) -> Option<&str> {
        self.product.as_deref()
    }
}

impl Drop for RusbTransport {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn rusb_to_io(err: rusb::Error) -> io::Error {
    match err {
        rusb::Error::Timeout => Error::new(ErrorKind::TimedOut, err),
        rusb::Error::NoDevice => Error::new(ErrorKind::NotFound, err),
        rusb::Error::Access => Error::new(ErrorKind::PermissionDenied, err),
        other => Error::other(other),
    }
}
