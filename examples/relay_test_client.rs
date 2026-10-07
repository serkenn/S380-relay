//! Minimal relay test client — no reader needed. Connects to the relay server
//! over TCP, sends get_card, then a few probe APDUs, printing each JSON reply.
//! Lets us exercise the server's card-side activate + ISO-DEP data phase
//! against the real card without tying up the second reader.
//! Run:  cargo run --release --example relay_test_client [host:port]
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;

fn main() {
    let addr = std::env::args().nth(1).unwrap_or_else(|| "127.0.0.1:7878".into());
    let mut stream = match TcpStream::connect(&addr) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("connect {addr} failed: {e}");
            return;
        }
    };
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();

    let mut send = |stream: &mut TcpStream, msg: &str| {
        stream.write_all(msg.as_bytes()).unwrap();
        stream.write_all(b"\n").unwrap();
        stream.flush().unwrap();
    };

    send(&mut stream, r#"{"type":"get_card"}"#);
    line.clear();
    reader.read_line(&mut line).unwrap();
    println!("get_card -> {}", line.trim());

    // A spread of harmless SELECTs / commands to see whether the data phase
    // carries APDUs and status words back from the real Type-B card.
    let apdus = [
        ("SELECT PPSE", "00a404000e325041592e5359532e4444463031"),
        ("SELECT eMRTD LDS1", "00a4040007a0000002471001"),
        ("SELECT MF", "00a40000023f00"),
        ("GET CHALLENGE", "0084000008"),
    ];
    for (name, apdu) in apdus {
        let req = format!(r#"{{"type":"apdu","data":"{apdu}","timeout_ms":2000}}"#);
        send(&mut stream, &req);
        line.clear();
        if reader.read_line(&mut line).unwrap() == 0 {
            println!("{name}: connection closed");
            break;
        }
        println!("{name} ({apdu}) -> {}", line.trim());
    }
}
