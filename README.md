# s380-relay

Relay ISO14443 smartcard traffic between two readers over the network, at the
**APDU layer**, built in Rust on top of the
[`felica`](https://crates.io/crates/felica) crate. Supports Sony **RC-S380**
(NFC Port-100) and **RC-S300 / PaSoRi 4.0** (NFC Port-400) on the card side, and
either an RC-S380 or an **Android phone (HCE)** on the terminal-facing side.

> 日本語版は [README_ja.md](README_ja.md) にあります。

The **server** holds a real card — a JavaCard applet, a smartcard, an ISO14443-4
card — on its reader. The **client** presents a card to a terminal/phone. When a
terminal is tapped to the client, every command APDU it sends is relayed over
TCP to the server's reader, replayed to the real card, and the response APDU is
played back. From the terminal's point of view it is talking to the real card.

```
 terminal ⇢ (Type A / ISO-DEP) ⇢ [client: RC-S380 or Android HCE] ──APDU over TCP──▶ [server: RC-S380/RC-S300] ⇢ real card
            └ ISO-DEP terminated here                                                 └ ISO-DEP terminated here
                                        only ISO 7816-4 APDUs cross the link
```

This is an NFC relay/emulation setup for use with **your own readers and cards**
(or cards you are authorised to test) in research, CTF, and interoperability
testing. Relaying a card you do not own or are not authorised to test may be
illegal; the burden is on you to have authorisation.

## Cross-technology relay (A ⇄ B)

Type A (Type 4) and Type B both converge at **ISO14443-4 (the APDU layer)**.
Because each side terminates ISO-DEP on its own reader and only the APDUs are
relayed, the real card may be **Type A or Type B**, while the client always
presents a **Type A Type-4 card** to the phone. The phone is unaware of the real
card's technology.

| Real card (server) | Emulated to phone (client) | Relay |
|--------------------|----------------------------|:-----:|
| NFC-A, ISO-DEP (Type 4) | NFC-A Type 4 | ✅ |
| NFC-B, ISO14443-4 | NFC-A Type 4 | ✅ |
| NFC-A, non-ISO-DEP (Type 2, e.g. MIFARE Ultralight) | — | ❌ (no layer-4 APDUs) |

The RC-S380 cannot emulate NFC-B, but it does not need to: the client side is
always NFC-A. Only a real card that speaks ISO14443-4 can be relayed (a plain
Type 2 tag has no APDU layer).

### UID / identity

The client presents a synthetic Type 4 identity (a 4-byte `0x08`-tagged UID and
a generic ATS); the real card's UID/ATS/ATQB is not reproduced. This does not
affect the ISO 7816-4 APDU exchange (what a JavaCard applet cares about), only
the lower-layer identity a reader observes.

## Requirements

- Rust 1.88+ (2024 edition)
- Card side: a Sony RC-S380 (Port-100), or an RC-S300 / PaSoRi 4.0 (Port-400)
  — **Type B requires the RC-S300** (see the note under *Server* below)
- Terminal side: a second RC-S380, **or** an Android phone running the HCE
  client in [`android/`](android/README.md)
- USB access to the readers (on Linux you may need a udev rule or sufficient
  privileges; on Windows bind the reader to WinUSB with Zadig, below)
- `git` — `cargo build` fetches the patched `felica` fork over git (see the
  Type B note); no manual step is needed

### Windows: install a WinUSB driver with Zadig

This tool talks to the reader through `libusb` (via the `rusb` crate), which on
Windows needs the device bound to the **WinUSB** driver. The RC-S380 normally
uses Sony's own driver (for FeliCa Port software), so `libusb` cannot open it
until you replace that driver.

Use [Zadig](https://zadig.akeo.ie/):

1. Plug in the RC-S380 and run Zadig.
2. **Options → List All Devices**, then select the RC-S380 (`SONY RC-S380`,
   USB ID `054C:06C1`).
3. Choose **WinUSB** as the target driver and click **Replace Driver** (or
   *Install Driver*).
4. Repeat for the second reader.

Do this per reader. To go back to the Sony FeliCa software later, uninstall the
WinUSB driver for the device in Device Manager and let Windows restore Sony's
driver. macOS and Linux need no such step.

## Build

```sh
cargo build --release
```

## Usage

One binary, subcommands `server` / `client` / `list`, logging via `RUST_LOG`.

### Two readers on one host

`list` shows the attached readers and their indices:

```sh
$ ./target/release/s380-relay list
attached RC-S380 readers:
  index 0  bus 003 addr 003  pid 0x06C1
  index 1  bus 003 addr 005  pid 0x06C1
```

On a single host with two readers, give each side a different reader with
`--device-index` (the server defaults to `0`, the client to `1`). On two
separate hosts, each sees only its own reader, so the defaults work and you can
omit the flag.

### Server (card side)

Place the real card on this reader, then:

```sh
RUST_LOG=info ./target/release/s380-relay server --listen 0.0.0.0:7878
```

| Flag | Default | Meaning |
|------|---------|---------|
| `-l, --listen <addr:port>` | `127.0.0.1:7878` | TCP listen address |
| `--tech <a\|b>` | `a` | Real card's ISO14443 technology |
| `--reader <port100\|port400>` | `port100` | Card-side reader backend |
| `-d, --device-index <n>` | `0` | Which RC-S380 to use (port100; see `list`) |
| `-t, --timeout <ms>` | `1000` | Per-command timeout |

> **Type B needs an RC-S300.** The RC-S380 (Port-100) driver activates a Type B
> card (ATTRIB) but cannot complete the Type B *data phase* — the card never
> answers the data-phase I-blocks. Use `--reader port400` with an RC-S300
> (PaSoRi 4.0), whose driver has a full PC/SC ISO-DEP stack (WTX/chaining/IFS),
> for the card side when relaying a Type B card. Type A works on either reader.
>
> Type B on the RC-S300 relies on a patched `felica` (see
> [`[patch.crates-io]`](Cargo.toml)): the stock crate read detection from a
> separate REQB that the already-activated card never answers (`036401`), and its
> ISO-DEP data phase aborted on an optional S(IFS) the card ignores. The fork
> reads detection from the SwitchProtocol ATR and makes S(IFS) best-effort. The
> patch is fetched automatically by `cargo build`; it will be dropped once the
> fix lands upstream.

### Client (phone side)

Connect to the server and start emulating a Type 4 card; then tap a phone:

```sh
RUST_LOG=info ./target/release/s380-relay client --connect <server-ip>:7878
```

| Flag | Default | Meaning |
|------|---------|---------|
| `-c, --connect <addr:port>` | `127.0.0.1:7878` | Server address |
| `-d, --device-index <n>` | `1` | Which RC-S380 to use (see `list`) |
| `--no-wtx` | (off) | Do not send S(WTX) before relaying |
| `--wtxm <1-59>` | `10` | Waiting-time extension multiplier |
| `-t, --timeout <ms>` | `1000` | Per-command timeout |
| `-w, --window <seconds>` | `1.0` | Listen window length |

`S(WTX)` is sent to the phone before each relayed command so the network
round-trip stays within the phone's frame-waiting time; raise `--wtxm` (or lower
it) to tune, or `--no-wtx` to disable.

### Android HCE client (alternative)

Instead of a second RC-S380, an Android phone can be the client using Host Card
Emulation: it presents a Type-A ISO-DEP card to the terminal and relays the
APDUs to the server over the same JSON protocol. This frees the RC-S380 for the
card side and is easy to keep running. See [`android/`](android/README.md) for
the app, build (GitHub Actions publishes an APK to Releases) and routing notes.

Android HCE only emulates **Type A**, so a genuine Type-B emulation is not
possible — but since the link is APDU-only, a Type-B card on the server is still
presented to the terminal as Type A. This works only with terminals that accept
a Type-A ISO-DEP card (i.e. that check APDUs, not the RF technology).

## Diagnostic examples

`cargo run --release --example <name>` — handy during bring-up:

| Example | What it does |
|---------|--------------|
| `probe` | Polls a Port-400 reader for Type B / FeliCa and prints what it finds |
| `probe100` | Polls a Port-100 (RC-S380) reader for Type B |
| `relay_test_client` | Connects to the server and sends a few APDUs (no reader needed) |
| `find_aid` | SELECTs a list of candidate AIDs against the real card to identify it |
| `terminal` | Drives an RC-S380 as an ISO-DEP reader to tap the phone and exercise the full relay |

## How the relay works

1. The client asks the server for the card (`get_card`). The server activates
   the real card into ISO-DEP — Type A via anticollision + SEL + `RATS`, or
   Type B via `SENSB` + `ATTRIB` — and reports its ATS/ATQB.
2. The client presents a synthetic Type 4 (NFC-A) card and listens. When a phone
   taps, the client terminates ISO-DEP with the phone (answering `RATS` locally)
   and reassembles each **command APDU**.
3. The command APDU is relayed over TCP to the server, which runs its own
   PCD-side ISO-DEP state machine (`src/isodep.rs`) to exchange it with the real
   card — handling block-number toggling, command/response chaining and the
   card's `S(WTX)` requests — and returns the response APDU.
4. The client wraps the response back into ISO-DEP I-block(s) for the phone. The
   two ISO-DEP sessions (phone↔client and server↔card) are independent; only the
   APDUs are shared, which is what allows a Type B card to be presented as Type A.

## Wire protocol

Newline-delimited JSON over TCP, mirroring the `felica-rs` remote examples:

```jsonc
// client → server
{"type":"get_card"}
// server → client
{"type":"card","tech":"B","info":"5090be4e5b000005e0b381a100"}

// client → server: relay one command APDU
{"type":"apdu","data":"00A4040007A0000002471001","timeout_ms":1000}
// server → client
{"type":"apdu","data":"6F..9000"}      // response APDU
{"type":"error","message":"..."}
```

All `data` fields are hex-encoded ISO 7816-4 APDUs. ISO-DEP framing (PCB, CRC,
chaining, WTX) is handled on each side and never crosses the link.

## Notes & limitations

- Only ISO14443-4 cards can be relayed; a plain Type 2 tag has no APDU layer.
- The emulated card's UID/ATS is synthetic, not the real card's (see above).
- Relay latency depends on the network link; `S(WTX)` buys time, but a very slow
  link can still make a phone drop the session.
- Command/response chaining paths are implemented but, lacking multi-reader
  hardware for every case, are best-effort; most APDUs fit a single block.
- One physical reader per side means the server serves one client at a time.

## License

Apache-2.0 — see `LICENSE`.
