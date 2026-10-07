//! Probe the real card (via the running server) with a list of candidate AIDs
//! to identify it. Prints the status word for each SELECT; anything that is not
//! "not found" (6A82/6A86/6A80) is a hit worth noting.
//!   cargo run --release --example find_aid [host:port]
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;

fn main() {
    let addr = std::env::args().nth(1).unwrap_or_else(|| "127.0.0.1:7878".into());
    let mut stream = TcpStream::connect(&addr).expect("connect");
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    let mut send = |s: &mut TcpStream, msg: &str| {
        s.write_all(msg.as_bytes()).unwrap();
        s.write_all(b"\n").unwrap();
        s.flush().unwrap();
    };

    send(&mut stream, r#"{"type":"get_card"}"#);
    line.clear();
    reader.read_line(&mut line).unwrap();
    println!("get_card -> {}\n", line.trim());

    // name, AID (hex). Japanese Type-B cards + common applets.
    let candidates: &[(&str, &str)] = &[
        ("My Number: kenmen-hojo",  "D3921000310001010408"),
        ("My Number: kenmen-kakunin","D3921000310001010402"),
        ("My Number: JPKI",         "D392f0002601000000"),
        ("My Number: juki",         "D3921000310001010100"),
        ("Driver license DF1",      "A0000002310100000000"),
        ("Driver license common",   "A0000002310101"),
        ("Residence card",          "D392f000d401000000"),
        ("eMRTD LDS1",              "A0000002471001"),
        ("eMRTD LDS2 travel",       "A0000002472001"),
        ("EMV PPSE (by name)",      "325041592e5359532e4444463031"),
        ("NDEF Type 4",             "D2760000850101"),
    ];

    for (name, aid) in candidates {
        // SELECT by DF name: 00 A4 04 00 Lc <AID> 00 (with Le).
        let lc = (aid.len() / 2) as u8;
        let apdu = format!("00a40400{:02x}{}00", lc, aid);
        let req = format!(r#"{{"type":"apdu","data":"{apdu}","timeout_ms":3000}}"#);
        send(&mut stream, &req);
        line.clear();
        if reader.read_line(&mut line).unwrap() == 0 {
            println!("{name:28}: connection closed");
            break;
        }
        let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap_or_default();
        let resp = v.get("data").and_then(|d| d.as_str()).unwrap_or("");
        let msg = v.get("message").and_then(|m| m.as_str()).unwrap_or("");
        let sw = resp.get(resp.len().saturating_sub(4)..).unwrap_or(resp);
        let hit = !matches!(sw, "6a82" | "6a86" | "6a80" | "6d00" | "6e00") && !resp.is_empty();
        let flag = if hit { "  <== HIT" } else { "" };
        if resp.is_empty() {
            println!("{name:28}: ERR {msg}");
        } else {
            println!("{name:28}: {resp}{flag}");
        }
    }
}
