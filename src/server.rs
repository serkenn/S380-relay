//! Server (card side): drives a local RC-S380 as an ISO14443 initiator against
//! the real card on the reader, and relays on-air blocks to it for a remote
//! client.
//!
//! Only one physical reader is involved, so connections are served one at a
//! time; a new client waits until the previous one disconnects.

use crate::protocol::{RelayRequest, RelayResponse, Tech};
use crate::usb::{RusbTransport, open_port100_indexed};
use felica::driver::port100::Device;
use felica::{DeviceInfo, RemoteTarget};
use hex::{decode as hex_decode, encode as hex_encode};
use log::{info, warn};
use std::error::Error;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};

/// Caps a single request line, as in the `felica-rs` remote server: a frame is
/// at most ~255 bytes (~510 hex chars), so 64 KiB bounds how much a client can
/// force us to buffer for one line.
const MAX_LINE_BYTES: u64 = 64 * 1024;

/// SAK bit 6 (`0x20`) indicates the card supports ISO14443-4 (ISO-DEP).
const SAK_ISO_DEP: u8 = 0x20;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub listen_addr: String,
    pub tech: Tech,
    pub timeout_ms: u16,
    /// Which attached RC-S380 to use (0-based), for multi-reader hosts.
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

/// Per-connection state: the activated target to relay blocks against.
struct Session {
    target: Option<RemoteTarget>,
}

fn handle_client(
    stream: TcpStream,
    device: &mut Device<RusbTransport>,
    config: &ServerConfig,
) -> Result<(), Box<dyn Error>> {
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    let mut session = Session { target: None };

    loop {
        let mut line = String::new();
        let read = (&mut reader).take(MAX_LINE_BYTES).read_line(&mut line)?;
        if read == 0 {
            break; // peer closed
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
        RelayRequest::Relay { data, timeout_ms } => {
            let Some(target) = session.target.as_ref() else {
                return RelayResponse::error("no activated card; send get_card first");
            };
            let frame = match hex_decode(data.trim()) {
                Ok(bytes) => bytes,
                Err(e) => return RelayResponse::error(format!("bad hex frame: {}", e)),
            };
            let timeout = timeout_ms.unwrap_or(config.timeout_ms);
            match device.transceive(target, &frame, Some(timeout)) {
                Ok(response) if response.is_empty() => RelayResponse::NoResponse,
                Ok(response) => RelayResponse::Response {
                    data: hex_encode(&response),
                },
                Err(e) => RelayResponse::error(format!("transceive failed: {}", e)),
            }
        }
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

    let atqa = found.data.sens_res.clone().unwrap_or_default();
    let uid = found.data.sdd_res.clone().unwrap_or_default();
    let sak = found.data.sel_res.clone().unwrap_or_default();
    let sak_byte = sak.first().copied().unwrap_or(0);

    // Put an ISO-DEP card into PROTOCOL state so it will answer I-blocks. The
    // RATS is driven by the server, not relayed: the client answers the phone's
    // RATS locally with this ATS, and both sides start block numbering at 0.
    let ats = if sak_byte & SAK_ISO_DEP != 0 {
        // RATS: FSDI=8 (256-byte frames), CID=0.
        match device.transceive(&found, &[0xE0, 0x80], Some(config.timeout_ms)) {
            Ok(ats) => ats,
            Err(e) => return RelayResponse::error(format!("RATS failed: {}", e)),
        }
    } else {
        Vec::new()
    };

    info!(
        "NFC-A card: ATQA={} UID={} SAK={} ATS={}",
        hex_encode(&atqa),
        hex_encode(&uid),
        hex_encode(&sak),
        hex_encode(&ats),
    );

    session.target = Some(found);
    RelayResponse::Card {
        tech: Tech::A,
        atqa: hex_encode(&atqa),
        uid: hex_encode(&uid),
        sak: hex_encode(&sak),
        ats: hex_encode(&ats),
    }
}

fn activate_type_b(
    device: &mut Device<RusbTransport>,
    _config: &ServerConfig,
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
    info!("NFC-B card: SENSB_RES={}", hex_encode(&sensb_res));

    session.target = Some(found);
    RelayResponse::Card {
        tech: Tech::B,
        atqa: String::new(),
        uid: String::new(),
        sak: String::new(),
        ats: hex_encode(&sensb_res),
    }
}
