//! `s380-relay` — relay ISO14443 (NFC-A/B) traffic between two Sony RC-S380
//! readers.
//!
//! Two roles, one per reader:
//!
//! - `server` holds the real card on its reader and relays blocks to it.
//! - `client` emulates that card; tapping a phone to it relays the phone's
//!   commands to the server's card and plays the responses back.
//!
//! Run `s380-relay --help` for usage.

mod client;
mod protocol;
mod server;

use protocol::Tech;
use std::error::Error;
use std::process::exit;

const DEFAULT_ADDR: &str = "127.0.0.1:7878";

fn main() {
    env_logger::init();

    let args: Vec<String> = std::env::args().collect();
    let result = match args.get(1).map(String::as_str) {
        Some("server") => run_server(&args[2..]),
        Some("client") => run_client(&args[2..]),
        Some("--help") | Some("-h") | None => {
            print_usage();
            return;
        }
        Some(other) => {
            eprintln!("Error: unknown command '{}'", other);
            print_usage();
            exit(1);
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        exit(1);
    }
}

fn run_server(args: &[String]) -> Result<(), Box<dyn Error>> {
    let mut config = server::ServerConfig {
        listen_addr: DEFAULT_ADDR.to_string(),
        tech: Tech::A,
        timeout_ms: 1000,
    };

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--listen" | "-l" => config.listen_addr = take_value(args, &mut i, "--listen")?,
            "--tech" => config.tech = parse_tech(&take_value(args, &mut i, "--tech")?)?,
            "--timeout" | "-t" => {
                config.timeout_ms = parse_u16(&take_value(args, &mut i, "--timeout")?)?
            }
            "--help" | "-h" => {
                print_server_usage();
                return Ok(());
            }
            other => return Err(format!("unknown server option '{}'", other).into()),
        }
        i += 1;
    }

    server::run(config)
}

fn run_client(args: &[String]) -> Result<(), Box<dyn Error>> {
    let mut config = client::ClientConfig {
        server_addr: DEFAULT_ADDR.to_string(),
        command_timeout_ms: 1000,
        listen_window_s: 1.0,
    };

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--connect" | "-c" => config.server_addr = take_value(args, &mut i, "--connect")?,
            "--timeout" | "-t" => {
                config.command_timeout_ms = parse_u16(&take_value(args, &mut i, "--timeout")?)?
            }
            "--window" | "-w" => {
                config.listen_window_s = take_value(args, &mut i, "--window")?
                    .parse()
                    .map_err(|e| format!("invalid --window value: {}", e))?
            }
            "--help" | "-h" => {
                print_client_usage();
                return Ok(());
            }
            other => return Err(format!("unknown client option '{}'", other).into()),
        }
        i += 1;
    }

    client::run(config)
}

fn take_value(args: &[String], i: &mut usize, flag: &str) -> Result<String, Box<dyn Error>> {
    let value = args
        .get(*i + 1)
        .ok_or_else(|| format!("{} requires a value", flag))?
        .clone();
    *i += 1;
    Ok(value)
}

fn parse_tech(value: &str) -> Result<Tech, Box<dyn Error>> {
    match value.to_ascii_uppercase().as_str() {
        "A" => Ok(Tech::A),
        "B" => Ok(Tech::B),
        other => Err(format!("invalid --tech '{}' (expected a or b)", other).into()),
    }
}

fn parse_u16(value: &str) -> Result<u16, Box<dyn Error>> {
    value
        .parse()
        .map_err(|e| format!("invalid number '{}': {}", value, e).into())
}

fn print_usage() {
    eprintln!("s380-relay — relay ISO14443 (NFC-A/B) traffic between two RC-S380 readers");
    eprintln!();
    eprintln!("Usage:");
    eprintln!("  s380-relay server [options]   Card side: hold the real card, relay to it");
    eprintln!("  s380-relay client [options]   Phone side: emulate the card, relay taps");
    eprintln!();
    eprintln!("Run 's380-relay <command> --help' for command-specific options.");
    eprintln!();
    eprintln!("Logging is controlled by RUST_LOG (e.g. RUST_LOG=info or RUST_LOG=debug).");
}

fn print_server_usage() {
    eprintln!("s380-relay server — card side");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  -l, --listen <addr:port>    Listen address (default: {})", DEFAULT_ADDR);
    eprintln!("      --tech <a|b>            ISO14443 technology (default: a)");
    eprintln!("                              NB: NFC-B can be polled but not emulated on RC-S380");
    eprintln!("  -t, --timeout <ms>          Per-command timeout (default: 1000)");
    eprintln!("  -h, --help                  Show this help");
}

fn print_client_usage() {
    eprintln!("s380-relay client — phone side (NFC-A only)");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  -c, --connect <addr:port>   Server address (default: {})", DEFAULT_ADDR);
    eprintln!("  -t, --timeout <ms>          Per-command timeout (default: 1000)");
    eprintln!("  -w, --window <seconds>      Listen window length (default: 1.0)");
    eprintln!("  -h, --help                  Show this help");
}
