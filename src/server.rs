//! Server (card side): holds the real card on a local reader, activates it into
//! ISO-DEP, and relays command APDUs to it for a remote client.
//!
//! The reader backend is pluggable ([`crate::cardside`]): an RC-S380 (Port-100)
//! or an RC-S300 (Port-400). An APDU exchange that fails is retried once after
//! re-activating the card, which recovers a card that dropped out of layer 4
//! while the field was idle.
//!
//! Only one physical reader is involved, so connections are served one at a
//! time; a new client waits until the previous one disconnects.

use crate::cardside::{self, CardSide, Reader};
use crate::protocol::{RelayRequest, RelayResponse, Tech};
use hex::{decode as hex_decode, encode as hex_encode};
use log::{debug, info, warn};
use std::error::Error;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};

/// Caps a single request line, as in the `felica-rs` remote server.
const MAX_LINE_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub listen_addr: String,
    pub tech: Tech,
    pub reader: Reader,
    pub timeout_ms: u16,
    pub device_index: usize,
}

pub fn run(config: ServerConfig) -> Result<(), Box<dyn Error>> {
    let mut card = cardside::open(config.reader, config.tech, config.device_index)?;
    info!("card-side reader: {}", card.label());

    let listener = TcpListener::bind(&config.listen_addr)?;
    println!(
        "S380 relay server (card side, NFC-{:?} via {:?}) listening on {}",
        config.tech, config.reader, config.listen_addr
    );
    println!("place the real card on this reader and tap the phone to the client");

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let peer = stream.peer_addr().ok();
                info!("client connected: {:?}", peer);
                if let Err(e) = handle_client(stream, card.as_mut(), &config) {
                    warn!("client session ended with error: {}", e);
                }
                info!("client disconnected: {:?}", peer);
            }
            Err(e) => warn!("accept failed: {}", e),
        }
    }

    Ok(())
}

/// Per-connection state: whether the card has been activated yet.
#[derive(Default)]
struct Session {
    activated: bool,
}

fn handle_client(
    stream: TcpStream,
    card: &mut dyn CardSide,
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
            Ok(request) => process_request(card, config, &mut session, request),
            Err(e) => RelayResponse::error(format!("invalid request: {}", e)),
        };

        writeln!(writer, "{}", serde_json::to_string(&response)?)?;
        writer.flush()?;
    }
    Ok(())
}

fn process_request(
    card: &mut dyn CardSide,
    config: &ServerConfig,
    session: &mut Session,
    request: RelayRequest,
) -> RelayResponse {
    match request {
        RelayRequest::GetCard => match card.activate(config.timeout_ms) {
            Ok(info) => {
                session.activated = true;
                RelayResponse::Card {
                    tech: card.tech(),
                    info,
                }
            }
            Err(e) => {
                session.activated = false;
                RelayResponse::error(e)
            }
        },
        RelayRequest::Apdu { data, timeout_ms } => relay_apdu(card, config, session, data, timeout_ms),
    }
}

fn relay_apdu(
    card: &mut dyn CardSide,
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

    if !session.activated {
        match card.activate(timeout) {
            Ok(_) => session.activated = true,
            Err(e) => return RelayResponse::error(e),
        }
    }

    // First attempt against the already-activated card.
    let first_err = match card.exchange(&apdu, timeout) {
        Ok(response) => return apdu_response(&response),
        Err(e) => e,
    };
    debug!("APDU failed ({first_err}); re-activating card and retrying once");

    // Retry once after re-activating, keeping the original error visible if the
    // card can no longer be activated.
    session.activated = false;
    if let Err(e) = card.activate(timeout) {
        return RelayResponse::error(format!(
            "APDU exchange failed: {first_err} (re-activation also failed: {e})"
        ));
    }
    session.activated = true;
    match card.exchange(&apdu, timeout) {
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
