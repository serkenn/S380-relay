# s380-relay

Relay ISO14443 (**NFC-A / NFC-B**) traffic between two Sony **RC-S380** (NFC
Port-100) readers over the network, built in Rust on top of the
[`felica`](https://crates.io/crates/felica) crate.

One reader (the **server**) holds a real card — a JavaCard applet, a smartcard,
etc. The other reader (the **client**) *emulates* that card. When a phone is
tapped to the client, every APDU/block the phone sends is relayed over TCP to
the server's reader, replayed to the real card, and the card's response is
played back. From the phone's point of view it is talking to the real card; from
the card's point of view it is talking to a real reader.

```
   phone ⇢ [client RC-S380]  ──TCP──▶  [server RC-S380] ⇢ real card
        (NFC-A emulation)                  (initiator)
```

This is an NFC relay/emulation setup for use with **your own readers and cards**
in research, CTF, and interoperability testing.

## What works on the RC-S380

The RC-S380's **card-emulation (target) mode supports NFC-A and NFC-F only — not
NFC-B**. So:

| Technology | Server (poll real card) | Client (emulate to phone) | End-to-end relay |
|------------|:-----------------------:|:-------------------------:|:----------------:|
| NFC-A (Type 2 / Type 4 ISO-DEP) | ✅ | ✅ | ✅ |
| NFC-B | ✅ (poll only) | ❌ (hardware cannot emulate) | ❌ |

NFC-A ISO-DEP (Type 4, i.e. ISO 7816-4 APDUs — what a JavaCard applet uses) is
the fully supported path. NFC-B can be polled on the server side for inspection,
but because no RC-S380 can emulate a Type B card, end-to-end B relay is not
possible with this hardware; the client reports this clearly if you try.

### UID fidelity

The port100 emulation presents a single-size (4-byte) NFCID1 whose first byte is
the `0x08` "dynamically generated UID" tag, so the real card's UID is **not**
reproduced byte-for-byte. This does not affect the ISO-DEP APDU exchange (what a
JavaCard applet cares about), only the UID a reader observes.

## Requirements

- Rust 1.88+ (2024 edition)
- Two Sony RC-S380 readers (one per machine, or both on one host)
- USB access to the readers (on Linux you may need a udev rule or sufficient
  privileges)

## Build

```sh
cargo build --release
```

## Usage

One binary, two subcommands, logging via `RUST_LOG`.

### Server (card side)

Place the real card on this reader, then:

```sh
RUST_LOG=info ./target/release/s380-relay server --listen 0.0.0.0:7878
```

| Flag | Default | Meaning |
|------|---------|---------|
| `-l, --listen <addr:port>` | `127.0.0.1:7878` | TCP listen address |
| `--tech <a\|b>` | `a` | ISO14443 technology (B is poll-only) |
| `-t, --timeout <ms>` | `1000` | Per-command timeout |

### Client (phone side, NFC-A)

Connect to the server and start emulating; then tap a phone to this reader:

```sh
RUST_LOG=info ./target/release/s380-relay client --connect <server-ip>:7878
```

| Flag | Default | Meaning |
|------|---------|---------|
| `-c, --connect <addr:port>` | `127.0.0.1:7878` | Server address |
| `-t, --timeout <ms>` | `1000` | Per-command timeout |
| `-w, --window <seconds>` | `1.0` | Listen window length |

## How the relay works (NFC-A ISO-DEP)

1. The client asks the server for the card (`get_card`). The server runs NFC-A
   anticollision + SEL, sends `RATS` to the real card, and returns the ATQA,
   UID, SAK and **ATS**.
2. The client builds a matching emulated target (SAK advertising ISO-DEP, the
   real ATS) and listens. When a phone taps, the client answers the phone's
   `RATS` locally with that ATS, so both the phone↔client link and the
   server↔card link start ISO-DEP block numbering at 0, in lockstep.
3. Each ISO-DEP block the phone sends (`tt4_cmd`) is relayed verbatim to the
   server, replayed to the card with `transceive`, and the card's response is
   sent back to the phone. S-blocks (WTX) and chaining are relayed transparently
   because blocks are passed through byte-for-byte.

## Wire protocol

Newline-delimited JSON over TCP, mirroring the `felica-rs` remote examples:

```jsonc
// client → server
{"type":"get_card"}
// server → client (NFC-A)
{"type":"card","tech":"A","atqa":"0400","uid":"04AABB...","sak":"20","ats":"0578807002"}

// client → server: relay one ISO-DEP block (PCB + payload, no CRC)
{"type":"relay","data":"0200A4...","timeout_ms":1000}
// server → client
{"type":"response","data":"03..."}     // card answered
{"type":"no_response"}                   // card silent / removed
{"type":"error","message":"..."}
```

All `data` fields are hex-encoded ISO-DEP blocks **without the CRC**, which each
reader appends on transmit and strips on receive — so blocks relay verbatim.

## Notes & limitations

- NFC-B cannot be emulated by the RC-S380 (see table above).
- UID is not reproduced byte-for-byte (see UID fidelity above).
- Relay latency depends on the network link; ISO-DEP frame-waiting times (FWT)
  are bounded, so a high-latency link can cause the phone to time out a session.
- One physical reader per side means the server serves one client at a time.

## License

Apache-2.0 — see `LICENSE`.
