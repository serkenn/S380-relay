//! NFC-B detection probe for the RC-S380 (Port-100). Keeps polling
//! detect_type_b for 30s so you can place/reposition a Type-B card and see
//! whether this reader senses it (SENSB_RES). The RC-S380 uses SENSB_REQ
//! `05 00 10`, unlike the RC-S300's `05 00 00`.
//! Run:  cargo run --release --example probe100
use felica::{DeviceInfo, RemoteTarget, open_port100};
use std::time::{Duration, Instant};

fn main() {
    let mut dev = match open_port100() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("open_port100 failed: {e}");
            return;
        }
    };
    println!(
        "RC-S380 opened: {} - {}",
        dev.vendor_name().unwrap_or("?"),
        dev.product_name().unwrap_or("?")
    );
    println!("polling NFC-B for 30s — place the Type-B card and adjust position...\n");

    let probe = RemoteTarget::new("106B").expect("106B target");
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut tick = 0u32;
    let mut last = String::new();
    while Instant::now() < deadline {
        match dev.detect_type_b(&probe) {
            Ok(Some(found)) => {
                let sensb = found.data.sensb_res.clone().unwrap_or_default();
                println!("\n[B] DETECTED  SENSB_RES={}", hex::encode(&sensb));
                return;
            }
            Ok(None) => last = "no card in field".into(),
            Err(e) => last = e.to_string(),
        }
        tick += 1;
        print!("\rno NFC-B yet... ({tick})  {last}        ");
        use std::io::Write;
        let _ = std::io::stdout().flush();
        std::thread::sleep(Duration::from_millis(300));
    }
    println!("\n\n30s elapsed: RC-S380 did not sense an NFC-B card.");
}
