//! Server (card side): drives a local RC-S380 as an ISO14443 reader, activates
//! the real card (Type A or Type B) into ISO-DEP, and relays command APDUs to
//! it for a remote client.
//!
//! Only one physical reader is involved, so connections are served one at a
//! time; a new client waits until the previous one disconnects.

use crate::isodep::{self, Pcd};
use crate::protocol::{RelayRequest, RelayResponse, Tech};
use crate::usb::{RusbTransport, open_port100_indexed};
use felica::driver::port100::Device;
use felica::{DeviceInfo, RemoteTarget};
use hex::{decode as hex_decode, encode as hex_encode};
use log::{debug, info, warn};
use std::error::Error;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};

/// Caps a single request line, as in the `felica-rs` remote server.
const MAX_LINE_BYTES: u64 = 64 * 1024;
/// SAK bit 6 (`0x20`): the Type A card supports ISO14443-4 (ISO-DEP).
const SAK_ISO_DEP: u8 = 0x20;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub listen_addr: String,
    pub tech: Tech,
    pub timeout_ms: u16,
    pub device_index: usize,
}

pub fn run(config: ServerConfig) -> Result<(), Box<dyn Error>> {
    let mut device = open_port100_indexed(config.device_index)?;
    info!(
        "reader[{}]: {} - {}",
        config.device_index,
        device.vendor_name().unwrap_or("Unknown"),
        device.product_name().unwrap_or("Unknown"),
    );

    let listener = TcpListener::bind(&config.listen_addr)?;
    println!(
        "S380 relay server (card side, NFC-{:?}) listening on {}",
        config.tech, config.listen_addr
    );
    println!("place the real card on this reader and tap the phone to the client");

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let peer = stream.peer_addr().ok();
                info!("client connected: {:?}", peer);
                if let Err(e) = handle_client(stream, &mut device, &config) {
                    warn!("client session ended with error: {}", e);
                }
                info!("client disconnected: {:?}", peer);
            }
            Err(e) => warn!("accept failed: {}", e),
        }
    }

    Ok(())
}

/// Per-connection state: the activated target and its ISO-DEP (PCD) layer.
struct Session {
    target: Option<RemoteTarget>,
    pcd: Option<Pcd>,
}

fn handle_client(
    stream: TcpStream,
    device: &mut Device<RusbTransport>,
    config: &ServerConfig,
) -> Result<(), Box<dyn Error>> {
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    let mut session = Session {
        target: None,
        pcd: None,
    };

    loop {
        let mut line = String::new();
        let read = (&mut reader).take(MAX_LINE_BYTES).read_line(&mut line)?;
        if read == 0 {
            break;
        }
        if line.trim().is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<RelayRequest>(&line) {
            Ok(request) => process_request(device, config, &mut session, request),
            Err(e) => RelayResponse::error(format!("invalid request: {}", e)),
        };

        writeln!(writer, "{}", serde_json::to_string(&response)?)?;
        writer.flush()?;
    }
    Ok(())
}

fn process_request(
    device: &mut Device<RusbTransport>,
    config: &ServerConfig,
    session: &mut Session,
    request: RelayRequest,
) -> RelayResponse {
    match request {
        RelayRequest::GetCard => match config.tech {
            Tech::A => activate_type_a(device, config, session),
            Tech::B => activate_type_b(device, config, session),
        },
        RelayRequest::Apdu { data, timeout_ms } => relay_apdu(device, config, session, data, timeout_ms),
    }
}

fn relay_apdu(
    device: &mut Device<RusbTransport>,
    config: &ServerConfig,
    session: &mut Session,
    data: String,
    timeout_ms: Option<u16>,
) -> RelayResponse {
    let (Some(target), Some(pcd)) = (session.target.as_ref(), session.pcd.as_mut()) else {
        return RelayResponse::error("no activated card; send get_card first");
    };
    let apdu = match hex_decode(data.trim()) {
        Ok(bytes) => bytes,
        Err(e) => return RelayResponse::error(format!("bad hex APDU: {}", e)),
    };
    let timeout = timeout_ms.unwrap_or(config.timeout_ms);

    let send = |block: &[u8]| -> Result<Vec<u8>, String> {
        match device.transceive(target, block, Some(timeout)) {
            Ok(resp) if resp.is_empty() => Err("card gave no response".into()),
            Ok(resp) => Ok(resp),
            Err(e) => Err(e.to_string()),
        }
    };

    match pcd.transmit(send, &apdu) {
        Ok(response) => {
            debug!("card APDU response: {}", hex_encode(&response));
            RelayResponse::Apdu {
                data: hex_encode(&response),
            }
        }
        Err(e) => RelayResponse::error(format!("APDU exchange failed: {}", e)),
    }
}

fn activate_type_a(
    device: &mut Device<RusbTransport>,
    config: &ServerConfig,
    session: &mut Session,
) -> RelayResponse {
    let probe = match RemoteTarget::new("106A") {
        Ok(t) => t,
        Err(e) => return RelayResponse::error(format!("bad target: {}", e)),
    };
    let found = match device.detect_type_a(&probe) {
        Ok(Some(found)) => found,
        Ok(None) => return RelayResponse::error("no NFC-A card detected"),
        Err(e) => return RelayResponse::error(format!("NFC-A activation failed: {}", e)),
    };

    let sak = found
        .data
        .sel_res
        .as_ref()
        .and_then(|b| b.first())
        .copied()
        .unwrap_or(0);
    if sak & SAK_ISO_DEP == 0 {
        return RelayResponse::error("NFC-A card is not ISO14443-4 (no ISO-DEP to relay)");
    }

    // RATS: FSDI=8 (256-byte frames), CID=0.
    let ats = match device.transceive(&found, &[0xE0, 0x80], Some(config.timeout_ms)) {
        Ok(ats) => ats,
        Err(e) => return RelayResponse::error(format!("RATS failed: {}", e)),
    };
    let fsc = fsc_from_ats(&ats);
    info!("NFC-A ISO-DEP card: ATS={} (FSC={})", hex_encode(&ats), fsc);

    session.target = Some(found);
    session.pcd = Some(Pcd::new(fsc));
    RelayResponse::Card {
        tech: Tech::A,
        info: hex_encode(&ats),
    }
}

fn activate_type_b(
    device: &mut Device<RusbTransport>,
    config: &ServerConfig,
    session: &mut Session,
) -> RelayResponse {
    let probe = match RemoteTarget::new("106B") {
        Ok(t) => t,
        Err(e) => return RelayResponse::error(format!("bad target: {}", e)),
    };
    let found = match device.detect_type_b(&probe) {
        Ok(Some(found)) => found,
        Ok(None) => return RelayResponse::error("no NFC-B card detected"),
        Err(e) => return RelayResponse::error(format!("NFC-B activation failed: {}", e)),
    };

    let sensb_res = found.data.sensb_res.clone().unwrap_or_default();
    if sensb_res.len() < 12 {
        return RelayResponse::error(format!(
            "SENSB_RES too short: {}",
            hex_encode(&sensb_res)
        ));
    }

    // ATTRIB: 0x1D + PUPI(4) + Param1..4. Param2=0x80 → FSDI=8 (256-byte PCD
    // frames); Param3=0x01 selects the ISO14443-4 protocol; CID=0.
    let pupi = &sensb_res[1..5];
    let mut attrib = vec![0x1D];
    attrib.extend_from_slice(pupi);
    attrib.extend_from_slice(&[0x00, 0x80, 0x01, 0x00]);
    match device.transceive(&found, &attrib, Some(config.timeout_ms)) {
        Ok(resp) => debug!("ATTRIB response: {}", hex_encode(&resp)),
        Err(e) => return RelayResponse::error(format!("ATTRIB failed: {}", e)),
    }

    // Type B frame size: high nibble of the first Protocol-Info byte (byte 9).
    let fsc = isodep::frame_size_from_code(sensb_res[9] >> 4);
    info!(
        "NFC-B ISO-DEP card: SENSB_RES={} (FSC={})",
        hex_encode(&sensb_res),
        fsc
    );

    session.target = Some(found);
    session.pcd = Some(Pcd::new(fsc));
    RelayResponse::Card {
        tech: Tech::B,
        info: hex_encode(&sensb_res),
    }
}

/// Derives the card's frame size (FSC) from its ATS; the FSCI is the low nibble
/// of T0 (the byte after the length byte TL), defaulting to 32 when absent.
fn fsc_from_ats(ats: &[u8]) -> usize {
    match ats.get(1) {
        Some(t0) => isodep::frame_size_from_code(t0 & 0x0F),
        None => 32,
    }
}
