//! Wire protocol between the relay server (card side) and client (phone side).
//!
//! Messages are newline-delimited JSON, in the style of the `felica-rs`
//! `remote_server`/`remote_client` examples. The relay works at the **APDU**
//! layer: each side terminates ISO-DEP (ISO14443-4) on its own reader, so only
//! ISO 7816-4 APDUs cross the link. This is what makes a cross-technology relay
//! possible — the real card may be Type A *or* Type B, while the client always
//! presents a Type A Type-4 card to the phone.
//!
//! The client drives every exchange: it first activates the card (`get_card`),
//! then relays each command APDU the phone sends and plays back the response.

use serde::{Deserialize, Serialize};

/// Which ISO14443 technology the server's real card uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Tech {
    A,
    B,
}

/// Request sent by the client (phone side) to the server (card side).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RelayRequest {
    /// Activate the real card into ISO-DEP (layer 4) and report it.
    GetCard,
    /// Relay one command APDU to the real card and return its response.
    Apdu {
        /// Hex-encoded ISO 7816-4 command APDU.
        data: String,
        /// Per-exchange timeout in milliseconds.
        #[serde(default)]
        timeout_ms: Option<u16>,
    },
}

/// Response sent by the server (card side) back to the client (phone side).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RelayResponse {
    /// The real card has been activated into ISO-DEP.
    Card {
        /// Technology of the real card.
        tech: Tech,
        /// Human-readable identification (ATS / ATQB, hex), for logging.
        info: String,
    },
    /// The real card's response APDU.
    Apdu {
        /// Hex-encoded ISO 7816-4 response APDU.
        data: String,
    },
    /// An error occurred on the server side (card removed, protocol error, …).
    Error { message: String },
}

impl RelayResponse {
    pub fn error(message: impl Into<String>) -> Self {
        RelayResponse::Error {
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_card_request_round_trips() {
        let json = serde_json::to_string(&RelayRequest::GetCard).unwrap();
        assert_eq!(json, r#"{"type":"get_card"}"#);
        assert!(matches!(
            serde_json::from_str::<RelayRequest>(&json).unwrap(),
            RelayRequest::GetCard
        ));
    }

    #[test]
    fn apdu_request_round_trips_with_and_without_timeout() {
        let with = RelayRequest::Apdu {
            data: "00a40400".into(),
            timeout_ms: Some(500),
        };
        let json = serde_json::to_string(&with).unwrap();
        match serde_json::from_str::<RelayRequest>(&json).unwrap() {
            RelayRequest::Apdu { data, timeout_ms } => {
                assert_eq!(data, "00a40400");
                assert_eq!(timeout_ms, Some(500));
            }
            other => panic!("unexpected {other:?}"),
        }

        let no_timeout = r#"{"type":"apdu","data":"00a4"}"#;
        match serde_json::from_str::<RelayRequest>(no_timeout).unwrap() {
            RelayRequest::Apdu { timeout_ms, .. } => assert_eq!(timeout_ms, None),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn card_and_apdu_responses_round_trip() {
        let card = RelayResponse::Card {
            tech: Tech::B,
            info: "5090be4e5b".into(),
        };
        let json = serde_json::to_string(&card).unwrap();
        match serde_json::from_str::<RelayResponse>(&json).unwrap() {
            RelayResponse::Card { tech, info } => {
                assert_eq!(tech, Tech::B);
                assert_eq!(info, "5090be4e5b");
            }
            other => panic!("unexpected {other:?}"),
        }

        let apdu = RelayResponse::Apdu { data: "9000".into() };
        let back: RelayResponse = serde_json::from_str(&serde_json::to_string(&apdu).unwrap()).unwrap();
        assert!(matches!(back, RelayResponse::Apdu { data } if data == "9000"));
    }

    #[test]
    fn tech_serializes_uppercase() {
        assert_eq!(serde_json::to_string(&Tech::A).unwrap(), r#""A""#);
        assert_eq!(serde_json::to_string(&Tech::B).unwrap(), r#""B""#);
    }
}
