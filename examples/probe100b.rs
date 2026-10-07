//! RC-S380 Type-B data-phase probe. Does detect_type_b -> ATTRIB -> one I-block
//! (SELECT PPSE) and prints the card's reply, so we can see whether the data
//! phase answers and whether the CID byte matters.
//!   cargo run --release --example probe100b            # I-block WITH CID (0x0A 00)
//!   cargo run --release --example probe100b -- nocid   # I-block WITHOUT CID (0x02)
use felica::{RemoteTarget, open_port100};

fn main() {
    let use_cid = std::env::args().nth(1).as_deref() != Some("nocid");
    let mut dev = match open_port100() {
        Ok(d) => d,
        Err(e) => { eprintln!("open_port100 failed: {e}"); return; }
    };
    let target = RemoteTarget::new("106B").expect("106B");

    let found = match dev.detect_type_b(&target) {
        Ok(Some(f)) => f,
        Ok(None) => { println!("no NFC-B card detected"); return; }
        Err(e) => { println!("detect_type_b error: {e}"); return; }
    };
    let sensb = found.data.sensb_res.clone().unwrap_or_default();
    println!("SENSB_RES = {}", hex::encode(&sensb));
    if sensb.len() < 12 { println!("SENSB_RES too short"); return; }

    // ATTRIB: 1D + PUPI(4) + Param1..4 (FSDI=8, ISO14443-4, CID=0)
    let pupi = &sensb[1..5];
    let mut attrib = vec![0x1D];
    attrib.extend_from_slice(pupi);
    attrib.extend_from_slice(&[0x00, 0x80, 0x01, 0x00]);
    match dev.transceive(&found, &attrib, Some(2000)) {
        Ok(r) => println!("ATTRIB -> {}", hex::encode(&r)),
        Err(e) => { println!("ATTRIB failed: {e}"); return; }
    }

    // First I-block, block number 0, carrying SELECT PPSE.
    let select_ppse = hex::decode("00a404000e325041592e5359532e4444463031").unwrap();
    let mut iblock = if use_cid { vec![0x0A, 0x00] } else { vec![0x02] };
    iblock.extend_from_slice(&select_ppse);
    println!("I-block ({}) TX = {}", if use_cid {"CID"} else {"no-CID"}, hex::encode(&iblock));
    match dev.transceive(&found, &iblock, Some(2000)) {
        Ok(r) => println!("I-block RX = {}", hex::encode(&r)),
        Err(e) => println!("I-block failed: {e}"),
    }
}
