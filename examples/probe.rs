//! Coupling probe for the Port-400 (RC-S300): each loop tries Type B, then
//! FeliCa (Type F). Tells us whether the crate's frontend path works at all on
//! this reader (FeliCa) versus Type-B specifically. Place a Type-B card OR a
//! FeliCa card (Suica / e-money) and watch which one answers.
//! Run:  cargo run --release --example probe
use felica::driver::port400::TypeBDetectOptions;
use felica::{RemoteTarget, open_port400};
use std::time::{Duration, Instant};

fn main() {
    let mut dev = match open_port400() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("open_port400 failed: {e}");
            return;
        }
    };
    println!("reader opened. Place a Type-B card OR a FeliCa card on the antenna.");
    println!("polling B + FeliCa for 30s...\n");

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut tick = 0u32;
    while Instant::now() < deadline {
        if let Ok(pupi) = dev.detect_type_b(Some(TypeBDetectOptions::default())) {
            println!("\n[B]      OK  PUPI={}", hex::encode(&pupi));
            return;
        }
        if let Ok(t) = RemoteTarget::new("212F") {
            match dev.detect_type_f(&t, 0xFFFF, 0x00, 0x00) {
                Ok(res) => {
                    println!("\n[FeliCa] OK  {res:?}");
                    return;
                }
                Err(_) => {}
            }
        }
        tick += 1;
        print!("\rno card sensed yet (B/FeliCa)... ({tick})     ");
        use std::io::Write;
        let _ = std::io::stdout().flush();
        std::thread::sleep(Duration::from_millis(300));
    }
    println!("\n\n30s elapsed: neither Type B nor FeliCa answered.");
}
