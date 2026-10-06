//! Minimal ISO14443-4 (ISO-DEP) layer-4 logic shared by both relay sides.
//!
//! The relay converges at the APDU layer: a Type A or Type B card and a Type A
//! emulated card all speak the same ISO-DEP block protocol above layers 2/3, so
//! each side terminates ISO-DEP independently and only the APDUs are relayed.
//!
//! This module provides the block (PCB) helpers plus a PCD-side (reader)
//! state machine, [`Pcd`], that turns a single APDU exchange into the right
//! sequence of I-/R-/S-blocks over a caller-supplied transceive closure. The
//! PICC-side (emulated card) sequencing lives in `client.rs`, where it is driven
//! by the reader's listen loop, but uses the same helpers.
//!
//! Scope: no CID and no NAD (single card, no node addressing), which is what a
//! point-to-point relay needs. Command and response chaining and S(WTX) are
//! handled; these paths are best-effort and most real APDUs use neither.

/// Base PCB for an I-block (no CID, no NAD, block number 0, no chaining).
const I_BLOCK: u8 = 0x02;

pub fn is_i_block(pcb: u8) -> bool {
    pcb & 0xC0 == 0x00
}

pub fn is_r_block(pcb: u8) -> bool {
    pcb & 0xE0 == 0xA0
}

pub fn is_s_block(pcb: u8) -> bool {
    pcb & 0xC0 == 0xC0
}

pub fn block_number(pcb: u8) -> u8 {
    pcb & 0x01
}

pub fn has_chaining(pcb: u8) -> bool {
    is_i_block(pcb) && pcb & 0x10 != 0
}

pub fn has_cid(pcb: u8) -> bool {
    pcb & 0x08 != 0
}

pub fn has_nad(pcb: u8) -> bool {
    is_i_block(pcb) && pcb & 0x04 != 0
}

pub fn is_s_wtx(pcb: u8) -> bool {
    is_s_block(pcb) && pcb & 0x30 == 0x30
}

pub fn is_s_deselect(pcb: u8) -> bool {
    is_s_block(pcb) && pcb & 0x30 == 0x00
}

/// Builds an I-block PCB with the given block number and chaining flag.
pub fn i_block(block: u8, chaining: bool) -> u8 {
    I_BLOCK | (block & 1) | if chaining { 0x10 } else { 0 }
}

/// Builds an R(ACK) PCB for the given block number.
pub fn r_ack(block: u8) -> u8 {
    0xA2 | (block & 1)
}

/// Builds an S(WTX) block (request and response share this format): PCB `0xF2`
/// followed by the waiting-time extension multiplier in the low 6 bits.
pub fn s_wtx(wtxm: u8) -> [u8; 2] {
    [0xF2, wtxm & 0x3F]
}

/// Returns the INF field (payload) of a block, skipping PCB and any CID/NAD.
pub fn inf(frame: &[u8]) -> &[u8] {
    let Some(&pcb) = frame.first() else {
        return &[];
    };
    let mut offset = 1;
    if has_cid(pcb) {
        offset += 1;
    }
    if has_nad(pcb) {
        offset += 1;
    }
    frame.get(offset..).unwrap_or(&[])
}

/// Decodes a frame-size code (FSCI/FSDI, 0..=8) into a byte count.
pub fn frame_size_from_code(code: u8) -> usize {
    match code {
        0 => 16,
        1 => 24,
        2 => 32,
        3 => 40,
        4 => 48,
        5 => 64,
        6 => 96,
        7 => 128,
        _ => 256,
    }
}

/// PCD-side (reader) ISO-DEP state machine over a transceive closure.
///
/// The block number persists across APDUs for the life of the activation.
pub struct Pcd {
    block: u8,
    /// Maximum INF bytes per block when sending to the card (FSC minus CRC).
    max_inf: usize,
    /// Card Identifier to include in every block, when one was assigned. Type B
    /// cards that advertise CID support (ATQB Protocol-Info FO bit) expect the
    /// CID byte even when it is 0; Type A with CID 0 omits it (`None`).
    cid: Option<u8>,
}

impl Pcd {
    /// Creates a PCD layer without a CID. `fsc` is the card's frame size (from
    /// ATS / ATQB); block numbering starts at 0, as required right after RATS /
    /// ATTRIB.
    pub fn new(fsc: usize) -> Self {
        Self {
            block: 0,
            max_inf: fsc.saturating_sub(2).max(1),
            cid: None,
        }
    }

    /// Creates a PCD layer that includes `cid` in every block.
    pub fn with_cid(fsc: usize, cid: u8) -> Self {
        Self {
            block: 0,
            max_inf: fsc.saturating_sub(2).max(1),
            cid: Some(cid & 0x0F),
        }
    }

    /// Builds an I-block header (PCB, plus the CID byte when one is used).
    fn i_header(&self, chaining: bool) -> Vec<u8> {
        self.with_cid_byte(i_block(self.block, chaining))
    }

    /// Builds a complete R(ACK) block (PCB, plus the CID byte when used).
    fn r_ack_block(&self) -> Vec<u8> {
        self.with_cid_byte(r_ack(self.block))
    }

    /// Builds a complete S(WTX) block (PCB, optional CID, then the WTXM byte).
    fn s_wtx_block(&self, wtxm: u8) -> Vec<u8> {
        let mut block = self.with_cid_byte(0xF2);
        block.push(wtxm & 0x3F);
        block
    }

    /// Prefixes a PCB with the CID flag and byte when a CID is in use.
    fn with_cid_byte(&self, mut pcb: u8) -> Vec<u8> {
        match self.cid {
            Some(cid) => vec![pcb | 0x08, cid],
            None => {
                pcb &= !0x08;
                vec![pcb]
            }
        }
    }

    /// Sends one command APDU and returns the response APDU, handling command
    /// chaining, response chaining and the card's S(WTX) requests.
    ///
    /// `send` transmits one block (PCB + INF, no CRC) and returns the card's
    /// reply block; it must map "no answer" to an error.
    pub fn transmit<F>(&mut self, mut send: F, apdu: &[u8]) -> Result<Vec<u8>, String>
    where
        F: FnMut(&[u8]) -> Result<Vec<u8>, String>,
    {
        let chunks: Vec<&[u8]> = if apdu.len() <= self.max_inf {
            vec![apdu]
        } else {
            apdu.chunks(self.max_inf).collect()
        };

        for (i, chunk) in chunks.iter().enumerate() {
            let last = i == chunks.len() - 1;
            let mut frame = self.i_header(!last);
            frame.extend_from_slice(chunk);

            let first = send(&frame)?;
            let reply = self.drain_wtx(&mut send, first)?;

            if last {
                return self.receive(&mut send, reply);
            }

            // Mid-chain: the card acknowledges with an R(ACK); advance.
            if reply.first().copied().is_none_or(|pcb| !is_r_block(pcb)) {
                return Err("expected R(ACK) during command chaining".into());
            }
            self.block ^= 1;
        }

        Err("empty APDU could not be sent".into())
    }

    /// Collects a (possibly chained) response starting from `first`.
    fn receive<F>(&mut self, send: &mut F, first: Vec<u8>) -> Result<Vec<u8>, String>
    where
        F: FnMut(&[u8]) -> Result<Vec<u8>, String>,
    {
        let mut data = Vec::new();
        let mut cur = first;
        loop {
            cur = self.drain_wtx(send, cur)?;
            let Some(&pcb) = cur.first() else {
                return Err("empty response block".into());
            };
            if !is_i_block(pcb) {
                return Err(format!("unexpected PCB {pcb:#04X} in response"));
            }
            data.extend_from_slice(inf(&cur));
            self.block ^= 1;
            if !has_chaining(pcb) {
                return Ok(data);
            }
            // More to come: acknowledge with the next block number.
            let ack = self.r_ack_block();
            cur = send(&ack)?;
        }
    }

    /// Answers any S(WTX) request from the card until a non-WTX block arrives.
    fn drain_wtx<F>(&mut self, send: &mut F, mut reply: Vec<u8>) -> Result<Vec<u8>, String>
    where
        F: FnMut(&[u8]) -> Result<Vec<u8>, String>,
    {
        while reply.first().copied().is_some_and(is_s_wtx) {
            // The WTXM is the last byte of the card's S(WTX) request (after an
            // optional CID byte).
            let wtxm = reply.last().copied().unwrap_or(1);
            let block = self.s_wtx_block(wtxm);
            reply = send(&block)?;
        }
        Ok(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcb_classification() {
        assert!(is_i_block(0x02));
        assert!(is_i_block(0x03));
        assert!(has_chaining(0x12));
        assert!(!has_chaining(0x02));
        assert!(is_r_block(0xA2));
        assert!(is_r_block(0xB3));
        assert!(is_s_block(0xF2));
        assert!(is_s_wtx(0xF2));
        assert!(is_s_deselect(0xC2));
        assert_eq!(block_number(0x03), 1);
    }

    #[test]
    fn inf_skips_cid_and_nad() {
        // I-block, no CID/NAD
        assert_eq!(inf(&[0x02, 0xAA, 0xBB]), &[0xAA, 0xBB]);
        // I-block with CID (0x08) and NAD (0x04): skip two extra bytes
        assert_eq!(inf(&[0x0E, 0x00, 0x00, 0xAA]), &[0xAA]);
    }

    #[test]
    fn single_block_exchange_toggles_block_number() {
        let mut pcd = Pcd::new(256);
        let mut sent = Vec::new();
        let response = pcd
            .transmit(
                |frame| {
                    sent.push(frame.to_vec());
                    // Echo an I-block with the same block number and a payload.
                    Ok(vec![i_block(block_number(frame[0]), false), 0x90, 0x00])
                },
                &[0x00, 0xA4, 0x04, 0x00],
            )
            .unwrap();
        assert_eq!(response, vec![0x90, 0x00]);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0][0], 0x02); // first I-block, block number 0
        assert_eq!(pcd.block, 1); // toggled for the next APDU
    }

    #[test]
    fn wtx_request_is_answered_then_response_read() {
        let mut pcd = Pcd::new(256);
        let mut step = 0;
        let response = pcd
            .transmit(
                |frame| {
                    step += 1;
                    match step {
                        1 => Ok(vec![0xF2, 0x05]), // card asks for more time
                        2 => {
                            assert_eq!(frame, &[0xF2, 0x05]); // we echo WTXM
                            Ok(vec![i_block(0, false), 0x6A, 0x82])
                        }
                        _ => panic!("too many sends"),
                    }
                },
                &[0x00, 0xA4],
            )
            .unwrap();
        assert_eq!(response, vec![0x6A, 0x82]);
    }

    #[test]
    fn cid_is_included_in_blocks_and_skipped_on_response() {
        let mut pcd = Pcd::with_cid(256, 0);
        let mut sent = Vec::new();
        let response = pcd
            .transmit(
                |frame| {
                    sent.push(frame.to_vec());
                    // Card echoes an I-block carrying the CID byte, then INF.
                    Ok(vec![i_block(0, false) | 0x08, 0x00, 0x90, 0x00])
                },
                &[0x00, 0xA4],
            )
            .unwrap();
        // Sent frame: PCB with CID flag (0x0A), CID byte (0x00), then the APDU.
        assert_eq!(sent[0], vec![0x0A, 0x00, 0x00, 0xA4]);
        // The CID byte is stripped from the response INF.
        assert_eq!(response, vec![0x90, 0x00]);
    }

    #[test]
    fn chained_response_is_reassembled() {
        let mut pcd = Pcd::new(256);
        let mut step = 0;
        let response = pcd
            .transmit(
                |_frame| {
                    step += 1;
                    match step {
                        1 => Ok(vec![i_block(0, true), 0x11, 0x22]), // chaining
                        2 => Ok(vec![i_block(1, false), 0x33]),      // final
                        _ => panic!("too many sends"),
                    }
                },
                &[0x00, 0xB0],
            )
            .unwrap();
        assert_eq!(response, vec![0x11, 0x22, 0x33]);
    }
}
