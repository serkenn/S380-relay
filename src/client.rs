//! Client (phone side): drives a local RC-S380 as an ISO14443 Type A target
//! that emulates the remote card. When a phone taps this reader, every block it
//! sends is relayed to the server, and the real card's response is played back.
//!
//! The RC-S380 can only emulate NFC-A (and NFC-F), not NFC-B, so this side
//! supports Type A relay only.

use crate::protocol::{RelayRequest, RelayResponse, Tech};
use felica::driver::port100::Device;
use felica::{DeviceInfo, LocalTarget, UsbTransport, open_port100};
use hex::{decode as hex_decode, encode as hex_encode};
use log::{debug, info, warn};
use std::error::Error;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub server_addr: String,
    pub command_timeout_ms: u16,
    /// How long a single `listen_type_a` window waits for a tap, in seconds.
    pub listen_window_s: f32,
}

/// Activation parameters of the card being emulated.
struct EmulatedCard {
    atqa: Vec<u8>,
    uid: Vec<u8>,
    sak: Vec<u8>,
    ats: Vec<u8>,
}

/// A buffered line-based connection to the relay server.
struct ServerLink {
    writer: TcpStream,
    reader: BufReader<TcpStream>,
}

impl ServerLink {
    fn connect(addr: &str) -> Result<Self, Box<dyn Error>> {
        let stream = TcpStream::connect(addr)?;
        let writer = stream.try_clone()?;
        Ok(Self {
            writer,
            reader: BufReader::new(stream),
        })
    }

    fn request(&mut self, request: &RelayRequest) -> Result<RelayResponse, Box<dyn Error>> {
        writeln!(self.writer, "{}", serde_json::to_string(request)?)?;
        self.writer.flush()?;
        let mut line = String::new();
        if self.reader.read_line(&mut line)? == 0 {
            return Err("server closed the connection".into());
        }
        Ok(serde_json::from_str::<RelayResponse>(line.trim())?)
    }
}

pub fn run(config: ClientConfig) -> Result<(), Box<dyn Error>> {
    let mut device = open_port100()?;
    info!(
        "reader: {} - {}",
        device.vendor_name().unwrap_or("Unknown"),
        device.product_name().unwrap_or("Unknown"),
    );

    let mut link = ServerLink::connect(&config.server_addr)?;
    println!("connected to relay server at {}", config.server_addr);

    let card = match link.request(&RelayRequest::GetCard)? {
        RelayResponse::Card {
            tech: Tech::A,
            atqa,
            uid,
            sak,
            ats,
        } => EmulatedCard {
            atqa: hex_decode(atqa.trim())?,
            uid: hex_decode(uid.trim())?,
            sak: hex_decode(sak.trim())?,
            ats: hex_decode(ats.trim())?,
        },
        RelayResponse::Card { tech: Tech::B, .. } => {
            return Err("the server is relaying NFC-B, which the RC-S380 cannot emulate; \
                        run the server with --tech a"
                .into());
        }
        RelayResponse::Error { message } => {
            return Err(format!("server could not read the card: {}", message).into());
        }
        other => return Err(format!("unexpected response to get_card: {:?}", other).into()),
    };

    let target = build_local_target(&card)?;
    println!(
        "emulating NFC-A card UID={} SAK={} ATS={}; tap a phone to this reader",
        hex_encode(&card.uid),
        hex_encode(&card.sak),
        hex_encode(&card.ats),
    );

    loop {
        if let Err(e) = emulate_once(&mut device, &target, &mut link, &config) {
            warn!("relay session error: {}", e);
        }
    }
}

/// Builds the `LocalTarget` the RC-S380 emulates from the card parameters.
///
/// The port100 emulation presents a single-size (4-byte) NFCID1 whose first
/// byte is the `0x08` "dynamically generated UID" tag, so the real UID cannot be
/// reproduced byte-for-byte. This does not affect ISO-DEP APDU exchange (what a
/// JavaCard applet cares about), only the UID a reader sees.
fn build_local_target(card: &EmulatedCard) -> Result<LocalTarget, Box<dyn Error>> {
    if card.atqa.len() != 2 {
        return Err(format!("expected 2-byte ATQA, got {}", hex_encode(&card.atqa)).into());
    }
    if card.sak.len() != 1 {
        return Err(format!("expected 1-byte SAK, got {}", hex_encode(&card.sak)).into());
    }

    let mut target = LocalTarget::new("106A")?;
    target.data.sens_res = Some(card.atqa.clone());

    // sdd_res is 0x08 (CT tag) followed by 3 UID bytes; borrow the first three
    // bytes of the real UID so at least part of it is visible.
    let mut sdd = vec![0x08];
    let mut uid_tail = card.uid.clone();
    uid_tail.resize(3, 0x00);
    sdd.extend_from_slice(&uid_tail[..3]);
    target.data.sdd_res = Some(sdd);

    target.data.sel_res = Some(card.sak.clone());
    if !card.ats.is_empty() {
        target.data.rats_res = Some(card.ats.clone());
    }
    Ok(target)
}

/// Runs one listen window: wait for a tap, then relay the session's blocks.
fn emulate_once(
    device: &mut Device<UsbTransport>,
    target: &LocalTarget,
    link: &mut ServerLink,
    config: &ClientConfig,
) -> Result<(), Box<dyn Error>> {
    let listened = device.listen_type_a(target, config.listen_window_s)?;

    let Some(local) = listened else {
        return Ok(()); // no tap within the window
    };

    // Either an ISO-DEP I-block (Type 4) or a Type 2 command, whichever the
    // phone sent first; both are relayed the same way.
    let first_frame = local.data.tt4_cmd.or(local.data.tt2_cmd);
    let Some(first_frame) = first_frame else {
        return Ok(());
    };

    info!("phone activated; relaying session");
    let mut next_frame = Some(first_frame);
    while let Some(frame) = next_frame {
        debug!("phone -> card: {}", hex_encode(&frame));
        let response = match link.request(&RelayRequest::Relay {
            data: hex_encode(&frame),
            timeout_ms: Some(config.command_timeout_ms),
        })? {
            RelayResponse::Response { data } => hex_decode(data.trim())?,
            RelayResponse::NoResponse => {
                debug!("card gave no response; ending session");
                break;
            }
            RelayResponse::Error { message } => {
                warn!("server error during relay: {}", message);
                break;
            }
            other => {
                warn!("unexpected relay response: {:?}", other);
                break;
            }
        };

        debug!("card -> phone: {}", hex_encode(&response));
        next_frame = device.send_response_receive_command(&response, config.command_timeout_ms)?;
    }

    Ok(())
}
