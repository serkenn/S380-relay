//! Test "terminal" that drives the RC-S380 (Port-100) as an ISO-DEP reader to
//! tap the phone running the HCE client, exercising the whole relay with the
//! hardware you already have:
//!
//!   RC-S380 (this tool) --NFC--> phone HCE --TCP--> server --NFC--> RC-S300 --> real card
//!
//! It polls NFC-A, activates ISO-DEP (RATS), then relays APDUs through the real
//! [`isodep::Pcd`] state machine (so S(WTX) waits from the slow HCE round trip
//! are handled) and prints each response. A real status word means HCE fired and
//! the relay carried the APDU back from the real card.
//!   cargo run --release --example terminal
use felica::{RemoteTarget, open_port100};
use s380_relay::isodep::{self, Pcd};
use std::time::{Duration, Instant};

fn main() {
    let mut dev = match open_port100() {
        Ok(d) => d,
        Err(e) => { eprintln!("open_port100 failed: {e}"); return; }
    };
    println!("RC-S380 terminal ready. Tap the phone (HCE app running) to this reader...");

    let target = RemoteTarget::new("106A").expect("106A");
    let deadline = Instant::now() + Duration::from_secs(20);
    let found = loop {
        if let Ok(Some(f)) = dev.detect_type_a(&target) {
            break f;
        }
        if Instant::now() > deadline {
            println!("no NFC-A target (phone) detected in 20s");
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let uid = found.data.sdd_res.clone().unwrap_or_default();
    println!("NFC-A target: UID={}", hex::encode(&uid));

    // RATS (FSDI=8, CID=0) to enter ISO-DEP layer 4.
    let ats = match dev.transceive(&found, &[0xE0, 0x80], Some(1000)) {
        Ok(ats) => ats,
        Err(e) => { println!("RATS failed: {e}"); return; }
    };
    println!("ATS = {}", hex::encode(&ats));
    let fsci = ats.get(1).map(|t0| t0 & 0x0F).unwrap_or(8);
    let fsc = isodep::frame_size_from_code(fsci);
    let mut pcd = Pcd::new(fsc);

    // Real applets of the identified card (My Number): a successful SELECT
    // returns an FCI template and 9000, proving the full relay carries real
    // transactions (reads past this need the card's PIN, which is out of scope).
    let apdus: [(&str, &str); 3] = [
        ("SELECT kenmen-hojo AP", "00a404000ad392100031000101040800"),
        ("SELECT JPKI AP",        "00a404000ad392f00026010000000100"),
        ("SELECT juki AP",        "00a404000ad392100031000101010000"),
    ];
    for (name, apdu_hex) in apdus {
        let apdu = hex::decode(apdu_hex).unwrap();
        // The HCE round trip is slow, so give each block a generous window; the
        // Pcd answers any S(WTX) the phone raises while it talks to the server.
        let send = |block: &[u8]| {
            dev.transceive(&found, block, Some(5000)).map_err(|e| e.to_string())
        };
        match pcd.transmit(send, &apdu) {
            Ok(resp) => println!("{name} -> {}", hex::encode(resp)),
            Err(e) => println!("{name} -> ERROR: {e}"),
        }
    }
}
