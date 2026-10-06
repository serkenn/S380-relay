//! Wire protocol between the relay server (card side) and client (phone side).
//!
//! Messages are newline-delimited JSON, in the style of the `felica-rs`
//! `remote_server`/`remote_client` examples. The client drives every exchange:
//! it first asks for the card's activation parameters so it can emulate it, then
//! relays each on-air ISO14443 block the phone sends and plays back the real
//! card's response.
//!
//! Frames (`data` fields) are hex-encoded ISO-DEP blocks — PCB + payload, i.e.
//! **without the trailing CRC**, which each reader appends on transmit and
//! strips on receive. Blocks are therefore relayed verbatim in both directions.

use serde::{Deserialize, Serialize};

/// Which ISO14443 technology the relay is operating on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Tech {
    /// ISO14443 Type A (NFC-A). Fully supported end-to-end on the RC-S380.
    A,
    /// ISO14443 Type B (NFC-B). The RC-S380 can poll a B card but cannot
    /// *emulate* one, so end-to-end B relay is not possible with this hardware.
    B,
}

/// Request sent by the client (phone side) to the server (card side).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RelayRequest {
    /// Activate the real card and return its parameters so the client can
    /// emulate it.
    GetCard,
    /// Relay one on-air ISO-DEP block to the real card.
    Relay {
        /// Hex-encoded block (PCB + payload, no CRC).
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
    /// Activation parameters of the real card.
    Card {
        tech: Tech,
        /// Type A: ATQA (SENS_RES), 2 bytes, hex. Type B: empty.
        #[serde(default)]
        atqa: String,
        /// Type A: UID (NFCID1), hex. Type B: empty.
        #[serde(default)]
        uid: String,
        /// Type A: SAK (SEL_RES), 1 byte, hex. Type B: empty.
        #[serde(default)]
        sak: String,
        /// Type A: ATS from RATS, hex (empty when the card is not ISO-DEP).
        /// Type B: SENSB_RES (ATQB), hex.
        #[serde(default)]
        ats: String,
    },
    /// The real card's response to a relayed block.
    Response {
        /// Hex-encoded block (PCB + payload, no CRC).
        data: String,
    },
    /// The card did not answer within the timeout (likely removed from field).
    NoResponse,
    /// An error occurred on the server side.
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
    fn relay_request_round_trips_with_and_without_timeout() {
        let with = RelayRequest::Relay {
            data: "0200".into(),
            timeout_ms: Some(500),
        };
        let json = serde_json::to_string(&with).unwrap();
        match serde_json::from_str::<RelayRequest>(&json).unwrap() {
            RelayRequest::Relay { data, timeout_ms } => {
                assert_eq!(data, "0200");
                assert_eq!(timeout_ms, Some(500));
            }
            other => panic!("unexpected {other:?}"),
        }

        // timeout_ms is optional on the wire.
        let no_timeout = r#"{"type":"relay","data":"03"}"#;
        match serde_json::from_str::<RelayRequest>(no_timeout).unwrap() {
            RelayRequest::Relay { data, timeout_ms } => {
                assert_eq!(data, "03");
                assert_eq!(timeout_ms, None);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn card_response_round_trips() {
        let card = RelayResponse::Card {
            tech: Tech::A,
            atqa: "0400".into(),
            uid: "04aabb".into(),
            sak: "20".into(),
            ats: "0578807002".into(),
        };
        let json = serde_json::to_string(&card).unwrap();
        match serde_json::from_str::<RelayResponse>(&json).unwrap() {
            RelayResponse::Card { tech, sak, ats, .. } => {
                assert_eq!(tech, Tech::A);
                assert_eq!(sak, "20");
                assert_eq!(ats, "0578807002");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn tech_serializes_uppercase() {
        assert_eq!(serde_json::to_string(&Tech::A).unwrap(), r#""A""#);
        assert_eq!(serde_json::to_string(&Tech::B).unwrap(), r#""B""#);
    }

    #[test]
    fn response_variants_round_trip() {
        for value in [
            RelayResponse::Response { data: "9000".into() },
            RelayResponse::NoResponse,
            RelayResponse::error("boom"),
        ] {
            let json = serde_json::to_string(&value).unwrap();
            let back: RelayResponse = serde_json::from_str(&json).unwrap();
            assert_eq!(
                std::mem::discriminant(&value),
                std::mem::discriminant(&back)
            );
        }
    }
}
