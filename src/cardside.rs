//! The card side of the relay: activating the real card into ISO-DEP and
//! exchanging APDUs with it. Two reader backends implement the same
//! [`CardSide`] trait:
//!
//! - [`Port100Side`] drives an RC-S380 (Port-100) with the crate's raw
//!   `transceive` and a hand-rolled ISO-DEP PCD layer ([`crate::isodep`]).
//!   This works for Type A ISO-DEP but not for the Type B data phase on this
//!   chipset.
//! - [`Port400Side`] drives an RC-S300 (Port-400), whose driver has a complete
//!   PC/SC-backed ISO-DEP stack (`iso_dep_exchange`) handling WTX, chaining and
//!   IFS — which is what a slow Type B card needs.

use crate::isodep::{self, Pcd};
use crate::protocol::Tech;
use crate::usb::{RusbTransport, open_port100_indexed};
use felica::driver::port100::Device as Device100;
use felica::driver::port400::{
    Device as Device400, IsoDepConfig, TypeADetectOptions, TypeBDetectOptions,
};
use felica::{DeviceInfo, RemoteTarget, UsbTransport, open_port400};
use hex::encode as hex_encode;
use log::info;

/// SAK bit 6 (`0x20`): the Type A card supports ISO14443-4 (ISO-DEP).
const SAK_ISO_DEP: u8 = 0x20;

/// A reader holding the real card, able to activate it and exchange APDUs.
pub trait CardSide {
    /// Human-readable reader identification, for logging.
    fn label(&self) -> String;
    /// The technology of the real card.
    fn tech(&self) -> Tech;
    /// (Re)activate the card into ISO-DEP; returns identification info (hex).
    fn activate(&mut self, timeout: u16) -> Result<String, String>;
    /// Exchange a single command APDU, returning the response APDU.
    fn exchange(&mut self, apdu: &[u8], timeout: u16) -> Result<Vec<u8>, String>;
}

/// Opens the configured card-side reader.
pub fn open(reader: Reader, tech: Tech, device_index: usize) -> Result<Box<dyn CardSide>, String> {
    match reader {
        Reader::Port100 => Ok(Box::new(Port100Side::open(tech, device_index)?)),
        Reader::Port400 => Ok(Box::new(Port400Side::open(tech)?)),
    }
}

/// Which reader backend drives the card side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reader {
    /// RC-S380 (Port-100).
    Port100,
    /// RC-S300 (Port-400).
    Port400,
}

// ---- Port-100 (RC-S380) ----------------------------------------------------

pub struct Port100Side {
    device: Device100<RusbTransport>,
    tech: Tech,
    target: Option<RemoteTarget>,
    pcd: Option<Pcd>,
}

impl Port100Side {
    fn open(tech: Tech, device_index: usize) -> Result<Self, String> {
        let device = open_port100_indexed(device_index).map_err(|e| e.to_string())?;
        Ok(Self {
            device,
            tech,
            target: None,
            pcd: None,
        })
    }
}

impl CardSide for Port100Side {
    fn label(&self) -> String {
        format!(
            "{} - {}",
            self.device.vendor_name().unwrap_or("Unknown"),
            self.device.product_name().unwrap_or("Unknown"),
        )
    }

    fn tech(&self) -> Tech {
        self.tech
    }

    fn activate(&mut self, timeout: u16) -> Result<String, String> {
        match self.tech {
            Tech::A => self.activate_type_a(timeout),
            Tech::B => self.activate_type_b(timeout),
        }
    }

    fn exchange(&mut self, apdu: &[u8], timeout: u16) -> Result<Vec<u8>, String> {
        let Self {
            device,
            target: Some(target),
            pcd: Some(pcd),
            ..
        } = self
        else {
            return Err("no activated card".into());
        };
        let send = |block: &[u8]| -> Result<Vec<u8>, String> {
            match device.transceive(target, block, Some(timeout)) {
                Ok(resp) if resp.is_empty() => Err("card gave no response".into()),
                Ok(resp) => Ok(resp),
                Err(e) => Err(e.to_string()),
            }
        };
        pcd.transmit(send, apdu)
    }
}

impl Port100Side {
    fn activate_type_a(&mut self, timeout: u16) -> Result<String, String> {
        let probe = RemoteTarget::new("106A").map_err(|e| format!("bad target: {e}"))?;
        let found = match self.device.detect_type_a(&probe) {
            Ok(Some(found)) => found,
            Ok(None) => return Err("no NFC-A card detected".into()),
            Err(e) => return Err(format!("NFC-A activation failed: {e}")),
        };
        let sak = found
            .data
            .sel_res
            .as_ref()
            .and_then(|b| b.first())
            .copied()
            .unwrap_or(0);
        if sak & SAK_ISO_DEP == 0 {
            return Err("NFC-A card is not ISO14443-4 (no ISO-DEP to relay)".into());
        }
        // RATS: FSDI=8 (256-byte frames), CID=0.
        let ats = self
            .device
            .transceive(&found, &[0xE0, 0x80], Some(timeout))
            .map_err(|e| format!("RATS failed: {e}"))?;
        let fsc = fsc_from_ats(&ats);
        info!("NFC-A ISO-DEP card: ATS={} (FSC={})", hex_encode(&ats), fsc);
        self.target = Some(found);
        self.pcd = Some(Pcd::new(fsc));
        Ok(hex_encode(&ats))
    }

    fn activate_type_b(&mut self, timeout: u16) -> Result<String, String> {
        let probe = RemoteTarget::new("106B").map_err(|e| format!("bad target: {e}"))?;
        let found = match self.device.detect_type_b(&probe) {
            Ok(Some(found)) => found,
            Ok(None) => return Err("no NFC-B card detected".into()),
            Err(e) => return Err(format!("NFC-B activation failed: {e}")),
        };
        let sensb_res = found.data.sensb_res.clone().unwrap_or_default();
        if sensb_res.len() < 12 {
            return Err(format!("SENSB_RES too short: {}", hex_encode(&sensb_res)));
        }
        // ATTRIB: 0x1D + PUPI(4) + Param1..4. Param2=0x80 → FSDI=8; Param3=0x01
        // selects ISO14443-4; CID=0.
        let pupi = &sensb_res[1..5];
        let mut attrib = vec![0x1D];
        attrib.extend_from_slice(pupi);
        attrib.extend_from_slice(&[0x00, 0x80, 0x01, 0x00]);
        self.device
            .transceive(&found, &attrib, Some(timeout))
            .map_err(|e| format!("ATTRIB failed: {e}"))?;
        let fsc = isodep::frame_size_from_code(sensb_res[9] >> 4);
        let cid = (sensb_res[11] & 0x01 != 0).then_some(0u8);
        info!(
            "NFC-B ISO-DEP card: SENSB_RES={} (FSC={}, CID={:?})",
            hex_encode(&sensb_res),
            fsc,
            cid
        );
        self.target = Some(found);
        self.pcd = Some(match cid {
            Some(c) => Pcd::with_cid(fsc, c),
            None => Pcd::new(fsc),
        });
        Ok(hex_encode(&sensb_res))
    }
}

/// Retries a Port-400 detect/activation until it succeeds or `timeout_ms`
/// elapses, polling roughly every 50 ms.
///
/// The driver's Type B sense (`request_type_b_info`) sends a single REQB with a
/// hardcoded 10 ms RF window and no retry, so a card that is still powering up
/// when the field comes on misses that one frame and the call fails with a
/// `036401 (no response packet received)` timeout. Relay activation is lazy —
/// it fires on client connect with no warm-up polling — so one retry loop here
/// absorbs the card's power-up latency and re-arms the field each attempt
/// (each `detect_*` re-runs `switch_protocol`). The last error is returned if
/// the deadline passes.
fn retry_activation<T, E, F>(timeout_ms: u16, mut attempt: F) -> Result<T, E>
where
    F: FnMut() -> Result<T, E>,
{
    use std::time::{Duration, Instant};
    let deadline = Instant::now() + Duration::from_millis(u64::from(timeout_ms.max(1)));
    loop {
        match attempt() {
            Ok(value) => return Ok(value),
            Err(err) => {
                if Instant::now() >= deadline {
                    return Err(err);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

/// Derives FSC from an ATS: FSCI is the low nibble of T0 (byte after TL).
fn fsc_from_ats(ats: &[u8]) -> usize {
    match ats.get(1) {
        Some(t0) => isodep::frame_size_from_code(t0 & 0x0F),
        None => 32,
    }
}

// ---- Port-400 (RC-S300) ----------------------------------------------------

pub struct Port400Side {
    device: Device400<UsbTransport>,
    tech: Tech,
}

impl Port400Side {
    fn open(tech: Tech) -> Result<Self, String> {
        let device = open_port400().map_err(|e| e.to_string())?;
        Ok(Self { device, tech })
    }
}

impl CardSide for Port400Side {
    fn label(&self) -> String {
        format!(
            "{} - {}",
            self.device.serial_number().unwrap_or("Unknown"),
            self.device.firmware_version().unwrap_or("RC-S300"),
        )
    }

    fn tech(&self) -> Tech {
        self.tech
    }

    fn activate(&mut self, timeout: u16) -> Result<String, String> {
        match self.tech {
            Tech::A => {
                let device = &mut self.device;
                let uid = retry_activation(timeout, || {
                    let options = TypeADetectOptions {
                        iso_dep: Some(IsoDepConfig::type_a_defaults()),
                    };
                    device.detect_type_a(Some(options))
                })
                .map_err(|e| format!("NFC-A activation failed: {e}"))?;
                info!("NFC-A ISO-DEP card (Port-400): UID={}", hex_encode(&uid));
                Ok(hex_encode(&uid))
            }
            Tech::B => {
                let device = &mut self.device;
                let pupi = retry_activation(timeout, || {
                    device.detect_type_b(Some(TypeBDetectOptions::default()))
                })
                .map_err(|e| format!("NFC-B activation failed: {e}"))?;
                info!("NFC-B ISO-DEP card (Port-400): PUPI={}", hex_encode(&pupi));
                Ok(hex_encode(&pupi))
            }
        }
    }

    fn exchange(&mut self, apdu: &[u8], _timeout: u16) -> Result<Vec<u8>, String> {
        self.device
            .iso_dep_exchange(apdu, false)
            .map_err(|e| e.to_string())
    }
}
