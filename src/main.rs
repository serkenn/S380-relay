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
mod isodep;
mod protocol;
mod server;
mod usb;

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
        Some("list") => run_list(),
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
        device_index: 0,
    };

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--listen" | "-l" => config.listen_addr = take_value(args, &mut i, "--listen")?,
            "--tech" => config.tech = parse_tech(&take_value(args, &mut i, "--tech")?)?,
            "--device-index" | "-d" => {
                config.device_index = parse_usize(&take_value(args, &mut i, "--device-index")?)?
            }
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
        // Defaults to reader 1 so that, on a single host with two readers, the
        // client and a default server (reader 0) do not collide.
        device_index: 1,
        use_wtx: true,
        wtxm: 10,
    };

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--connect" | "-c" => config.server_addr = take_value(args, &mut i, "--connect")?,
            "--device-index" | "-d" => {
                config.device_index = parse_usize(&take_value(args, &mut i, "--device-index")?)?
            }
            "--no-wtx" => config.use_wtx = false,
            "--wtxm" => {
                config.wtxm = parse_u8_dec(&take_value(args, &mut i, "--wtxm")?)?.clamp(1, 59)
            }
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

fn run_list() -> Result<(), Box<dyn Error>> {
    let readers = usb::list_port100()?;
    if readers.is_empty() {
        println!("no RC-S380 (Port-100) readers found");
        return Ok(());
    }
    println!("attached RC-S380 readers:");
    for reader in readers {
        println!(
            "  index {}  bus {:03} addr {:03}  pid 0x{:04X}",
            reader.index, reader.bus, reader.address, reader.product_id
        );
    }
    Ok(())
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

fn parse_usize(value: &str) -> Result<usize, Box<dyn Error>> {
    value
        .parse()
        .map_err(|e| format!("invalid index '{}': {}", value, e).into())
}

fn parse_u8_dec(value: &str) -> Result<u8, Box<dyn Error>> {
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
    eprintln!("  s380-relay list               List attached RC-S380 readers and indices");
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
    eprintln!("      --tech <a|b>            Real card's ISO14443 technology (default: a)");
    eprintln!("  -d, --device-index <n>      Which RC-S380 to use (default: 0; see 'list')");
    eprintln!("  -t, --timeout <ms>          Per-command timeout (default: 1000)");
    eprintln!("  -h, --help                  Show this help");
}

fn print_client_usage() {
    eprintln!("s380-relay client — phone side (emulates a Type 4 NFC-A card)");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  -c, --connect <addr:port>   Server address (default: {})", DEFAULT_ADDR);
    eprintln!("  -d, --device-index <n>      Which RC-S380 to use (default: 1; see 'list')");
    eprintln!("      --no-wtx                Do not send S(WTX) before relaying");
    eprintln!("      --wtxm <1-59>           Waiting-time extension multiplier (default: 10)");
    eprintln!("  -t, --timeout <ms>          Per-command timeout (default: 1000)");
    eprintln!("  -w, --window <seconds>      Listen window length (default: 1.0)");
    eprintln!("  -h, --help                  Show this help");
}
