//! Client (phone side): drives a local RC-S380 as a Type A ISO-DEP (Type 4)
//! target. It presents a synthetic Type 4 card to the phone, terminates
//! ISO-DEP locally, and relays the command APDUs to the server — whose real
//! card may be Type A or Type B. Only the APDUs cross the link, so the phone is
//! unaware of the real card's technology.
//!
//! The RC-S380 can only emulate NFC-A (and NFC-F), so this side is always
//! Type A regardless of the server's card.

use crate::isodep;
use crate::protocol::{RelayRequest, RelayResponse};
use crate::usb::{RusbTransport, open_port100_indexed};
use felica::driver::port100::Device;
use felica::{DeviceInfo, LocalTarget};
use hex::{decode as hex_decode, encode as hex_encode};
use log::{debug, info, warn};
use std::error::Error;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;

/// Synthetic Type A activation values presented to the phone. The ATS declares
/// FSCI=8 (256-byte frames) and TA/TB/TC, matching a generic Type 4 card.
const SENS_RES: [u8; 2] = [0x04, 0x00];
const UID_TAIL: [u8; 3] = [0x01, 0x02, 0x03]; // follows the 0x08 CT tag
const SAK: [u8; 1] = [0x20]; // ISO14443-4 supported
const ATS: [u8; 5] = [0x05, 0x78, 0x80, 0x70, 0x02];
/// Assumed phone frame size (FSD) when chaining a long response back.
const PHONE_FSD: usize = 256;

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub server_addr: String,
    pub command_timeout_ms: u16,
    pub listen_window_s: f32,
    pub device_index: usize,
    /// Send S(WTX) to the phone before relaying, to survive network latency.
    pub use_wtx: bool,
    /// Waiting-time extension multiplier used with S(WTX) (1..=59).
    pub wtxm: u8,
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

    /// Relays one command APDU and returns the response APDU bytes.
    fn apdu(&mut self, capdu: &[u8], timeout_ms: u16) -> Result<Vec<u8>, Box<dyn Error>> {
        match self.request(&RelayRequest::Apdu {
            data: hex_encode(capdu),
            timeout_ms: Some(timeout_ms),
        })? {
            RelayResponse::Apdu { data } => Ok(hex_decode(data.trim())?),
            RelayResponse::Error { message } => Err(format!("server: {}", message).into()),
            other => Err(format!("unexpected relay response: {:?}", other).into()),
        }
    }
}

pub fn run(config: ClientConfig) -> Result<(), Box<dyn Error>> {
    let mut device = open_port100_indexed(config.device_index)?;
    info!(
        "reader[{}]: {} - {}",
        config.device_index,
        device.vendor_name().unwrap_or("Unknown"),
        device.product_name().unwrap_or("Unknown"),
    );

    let mut link = ServerLink::connect(&config.server_addr)?;
    println!("connected to relay server at {}", config.server_addr);

    match link.request(&RelayRequest::GetCard)? {
        RelayResponse::Card { tech, info } => {
            println!("real card activated: NFC-{:?} ({})", tech, info);
        }
        RelayResponse::Error { message } => {
            return Err(format!("server could not activate the card: {}", message).into());
        }
        other => return Err(format!("unexpected response to get_card: {:?}", other).into()),
    }

    let target = build_local_target()?;
    println!("emulating a Type 4 (NFC-A) card; tap a phone to this reader");

    loop {
        if let Err(e) = emulate_once(&mut device, &target, &mut link, &config) {
            warn!("relay session error: {}", e);
        }
    }
}

/// Builds the synthetic Type A Type-4 target presented to the phone.
fn build_local_target() -> Result<LocalTarget, Box<dyn Error>> {
    let mut target = LocalTarget::new("106A")?;
    target.data.sens_res = Some(SENS_RES.to_vec());
    let mut sdd = vec![0x08];
    sdd.extend_from_slice(&UID_TAIL);
    target.data.sdd_res = Some(sdd);
    target.data.sel_res = Some(SAK.to_vec());
    target.data.rats_res = Some(ATS.to_vec());
    Ok(target)
}

/// Waits for a tap, then runs the PICC-side ISO-DEP loop for one session.
fn emulate_once(
    device: &mut Device<RusbTransport>,
    target: &LocalTarget,
    link: &mut ServerLink,
    config: &ClientConfig,
) -> Result<(), Box<dyn Error>> {
    let Some(local) = device.listen_type_a(target, config.listen_window_s)? else {
        return Ok(()); // no tap within the window
    };
    let Some(first) = local.data.tt4_cmd else {
        return Ok(()); // not an ISO-DEP activation
    };

    info!("phone activated ISO-DEP; relaying APDUs");
    let mut frame = first;
    loop {
        match process_phone_frame(device, link, config, frame)? {
            Some(next) => frame = next,
            None => return Ok(()),
        }
    }
}

/// Handles one block from the phone, returning the next block to process (or
/// `None` when the session ends).
fn process_phone_frame(
    device: &mut Device<RusbTransport>,
    link: &mut ServerLink,
    config: &ClientConfig,
    frame: Vec<u8>,
) -> Result<Option<Vec<u8>>, Box<dyn Error>> {
    let Some(&pcb) = frame.first() else {
        return Ok(None);
    };
    let t = config.command_timeout_ms;

    if isodep::is_s_deselect(pcb) {
        debug!("phone sent S(DESELECT); ending session");
        let _ = device.send_response_receive_command(&frame, t)?;
        return Ok(None);
    }
    if isodep::is_s_wtx(pcb) {
        // Unusual from a phone; echo the WTXM to keep the dialogue alive.
        let wtxm = frame.get(1).copied().unwrap_or(1);
        return Ok(device.send_response_receive_command(&isodep::s_wtx(wtxm), t)?);
    }
    if !isodep::is_i_block(pcb) {
        debug!("phone sent unexpected PCB {:#04X}; ignoring", pcb);
        return Ok(None);
    }

    // Reassemble a (possibly chained) command APDU.
    let mut bn = isodep::block_number(pcb);
    let mut capdu = isodep::inf(&frame).to_vec();
    let mut chaining = isodep::has_chaining(pcb);
    while chaining {
        let Some(next) = device.send_response_receive_command(&[isodep::r_ack(bn)], t)? else {
            return Ok(None);
        };
        let npcb = next[0];
        if !isodep::is_i_block(npcb) {
            return Ok(None);
        }
        bn = isodep::block_number(npcb);
        capdu.extend_from_slice(isodep::inf(&next));
        chaining = isodep::has_chaining(npcb);
    }
    debug!("phone -> card APDU: {}", hex_encode(&capdu));

    // Buy time over the network link before relaying, if enabled.
    if config.use_wtx {
        let Some(_wtx_resp) = device.send_response_receive_command(&isodep::s_wtx(config.wtxm), t)?
        else {
            return Ok(None);
        };
    }

    let rapdu = link.apdu(&capdu, t)?;
    debug!("card -> phone APDU: {}", hex_encode(&rapdu));

    send_response_apdu(device, bn, &rapdu, t)
}

/// Sends a response APDU back to the phone as one or more I-blocks (chaining it
/// when it exceeds the phone's frame size), returning the phone's next block.
fn send_response_apdu(
    device: &mut Device<RusbTransport>,
    bn: u8,
    rapdu: &[u8],
    timeout: u16,
) -> Result<Option<Vec<u8>>, Box<dyn Error>> {
    let max_inf = PHONE_FSD.saturating_sub(2).max(1);
    if rapdu.len() <= max_inf {
        let mut block = Vec::with_capacity(rapdu.len() + 1);
        block.push(isodep::i_block(bn, false));
        block.extend_from_slice(rapdu);
        return Ok(device.send_response_receive_command(&block, timeout)?);
    }

    let chunks: Vec<&[u8]> = rapdu.chunks(max_inf).collect();
    let mut cur_bn = bn;
    for (i, chunk) in chunks.iter().enumerate() {
        let last = i == chunks.len() - 1;
        let mut block = Vec::with_capacity(chunk.len() + 1);
        block.push(isodep::i_block(cur_bn, !last));
        block.extend_from_slice(chunk);
        let Some(got) = device.send_response_receive_command(&block, timeout)? else {
            return Ok(None);
        };
        if last {
            return Ok(Some(got));
        }
        // Mid-chain: the phone acknowledges with an R(ACK); advance.
        if got.first().copied().is_none_or(|p| !isodep::is_r_block(p)) {
            return Ok(None);
        }
        cur_bn ^= 1;
    }
    Ok(None)
}
