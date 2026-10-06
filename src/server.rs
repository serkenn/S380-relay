//! Server (card side): drives a local RC-S380 as an ISO14443 reader, activates
//! the real card (Type A or Type B) into ISO-DEP, and relays command APDUs to
//! it for a remote client.
//!
//! A card can drop out of its ISO14443-4 (layer 4) state while the field sits
//! idle — for instance during the seconds between `get_card` and the phone
//! actually being tapped to the client. So an APDU exchange that fails is
//! retried once after transparently re-activating the card (re-RATS/-ATTRIB and
//! resetting the block number), which is what lets a fresh phone tap succeed.
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

/// A card that has been activated into ISO-DEP.
struct Activated {
    target: RemoteTarget,
    fsc: usize,
    /// Card Identifier to use in blocks, when the card expects one.
    cid: Option<u8>,
    info: String,
}

/// Per-connection state: the activated target and its ISO-DEP (PCD) layer.
#[derive(Default)]
struct Session {
    target: Option<RemoteTarget>,
    pcd: Option<Pcd>,
    info: String,
}

fn handle_client(
    stream: TcpStream,
    device: &mut Device<RusbTransport>,
    config: &ServerConfig,
) -> Result<(), Box<dyn Error>> {
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    let mut session = Session::default();

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
        RelayRequest::GetCard => match reactivate(device, config, session) {
            Ok(()) => RelayResponse::Card {
                tech: config.tech,
                info: session.info.clone(),
            },
            Err(e) => RelayResponse::error(e),
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
    let apdu = match hex_decode(data.trim()) {
        Ok(bytes) => bytes,
        Err(e) => return RelayResponse::error(format!("bad hex APDU: {}", e)),
    };
    let timeout = timeout_ms.unwrap_or(config.timeout_ms);

    if session.target.is_none()
        && let Err(e) = reactivate(device, config, session)
    {
        return RelayResponse::error(e);
    }

    // First attempt against the already-activated card.
    let first_err = match exchange(session, device, &apdu, timeout) {
        Ok(response) => return apdu_response(&response),
        Err(e) => e,
    };
    debug!("APDU failed ({first_err}); re-activating card and retrying once");

    // Retry once after re-activating, but keep the original error visible if the
    // card can no longer be activated (it may still be in layer 4 and simply
    // ignoring our blocks, in which case re-polling it will not answer).
    if let Err(e) = reactivate(device, config, session) {
        return RelayResponse::error(format!(
            "APDU exchange failed: {first_err} (re-activation also failed: {e})"
        ));
    }
    match exchange(session, device, &apdu, timeout) {
        Ok(response) => apdu_response(&response),
        Err(e) => RelayResponse::error(format!("APDU exchange failed: {}", e)),
    }
}

fn apdu_response(response: &[u8]) -> RelayResponse {
    debug!("card APDU response: {}", hex_encode(response));
    RelayResponse::Apdu {
        data: hex_encode(response),
    }
}

/// Runs one APDU exchange over the session's activated target and PCD layer.
fn exchange(
    session: &mut Session,
    device: &mut Device<RusbTransport>,
    apdu: &[u8],
    timeout: u16,
) -> Result<Vec<u8>, String> {
    let Session {
        target: Some(target),
        pcd: Some(pcd),
        ..
    } = session
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

/// (Re)activates the real card into ISO-DEP and resets the session's PCD layer.
fn reactivate(
    device: &mut Device<RusbTransport>,
    config: &ServerConfig,
    session: &mut Session,
) -> Result<(), String> {
    let activated = match config.tech {
        Tech::A => activate_type_a(device, config)?,
        Tech::B => activate_type_b(device, config)?,
    };
    session.info = activated.info;
    session.pcd = Some(match activated.cid {
        Some(cid) => Pcd::with_cid(activated.fsc, cid),
        None => Pcd::new(activated.fsc),
    });
    session.target = Some(activated.target);
    Ok(())
}

fn activate_type_a(
    device: &mut Device<RusbTransport>,
    config: &ServerConfig,
) -> Result<Activated, String> {
    let probe = RemoteTarget::new("106A").map_err(|e| format!("bad target: {e}"))?;
    let found = match device.detect_type_a(&probe) {
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
    let ats = device
        .transceive(&found, &[0xE0, 0x80], Some(config.timeout_ms))
        .map_err(|e| format!("RATS failed: {e}"))?;
    let fsc = fsc_from_ats(&ats);
    info!("NFC-A ISO-DEP card: ATS={} (FSC={})", hex_encode(&ats), fsc);

    Ok(Activated {
        target: found,
        fsc,
        cid: None,
        info: hex_encode(&ats),
    })
}

fn activate_type_b(
    device: &mut Device<RusbTransport>,
    config: &ServerConfig,
) -> Result<Activated, String> {
    let probe = RemoteTarget::new("106B").map_err(|e| format!("bad target: {e}"))?;
    let found = match device.detect_type_b(&probe) {
        Ok(Some(found)) => found,
        Ok(None) => return Err("no NFC-B card detected".into()),
        Err(e) => return Err(format!("NFC-B activation failed: {e}")),
    };

    let sensb_res = found.data.sensb_res.clone().unwrap_or_default();
    if sensb_res.len() < 12 {
        return Err(format!("SENSB_RES too short: {}", hex_encode(&sensb_res)));
    }

    // ATTRIB: 0x1D + PUPI(4) + Param1..4. Param2=0x80 → FSDI=8 (256-byte PCD
    // frames); Param3=0x01 selects the ISO14443-4 protocol; CID=0.
    let pupi = &sensb_res[1..5];
    let mut attrib = vec![0x1D];
    attrib.extend_from_slice(pupi);
    attrib.extend_from_slice(&[0x00, 0x80, 0x01, 0x00]);
    match device.transceive(&found, &attrib, Some(config.timeout_ms)) {
        Ok(resp) => debug!("ATTRIB response: {}", hex_encode(&resp)),
        Err(e) => return Err(format!("ATTRIB failed: {e}")),
    }

    // Type B frame size: high nibble of the first Protocol-Info byte (byte 9).
    let fsc = isodep::frame_size_from_code(sensb_res[9] >> 4);
    // Frame Option bit 1 of the last Protocol-Info byte (byte 11): the card
    // supports a CID and therefore expects it in every block (we assign 0).
    let cid = (sensb_res[11] & 0x01 != 0).then_some(0u8);
    info!(
        "NFC-B ISO-DEP card: SENSB_RES={} (FSC={}, CID={})",
        hex_encode(&sensb_res),
        fsc,
        cid.map_or_else(|| "none".to_string(), |c| c.to_string()),
    );

    Ok(Activated {
        target: found,
        fsc,
        cid,
        info: hex_encode(&sensb_res),
    })
}

/// Derives the card's frame size (FSC) from its ATS; the FSCI is the low nibble
/// of T0 (the byte after the length byte TL), defaulting to 32 when absent.
fn fsc_from_ats(ats: &[u8]) -> usize {
    match ats.get(1) {
        Some(t0) => isodep::frame_size_from_code(t0 & 0x0F),
        None => 32,
    }
}
