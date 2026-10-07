# S380 Relay — Android HCE client

Presents a **Type-A ISO-DEP (Type-4) card** to a terminal via Android Host Card
Emulation (HCE) and relays every APDU to the Rust relay **server** over TCP. The
real (possibly Type-B) card lives on the server's reader; only ISO 7816-4 APDUs
cross the link, so this is an **A-to-B bridge** — it works only against a
terminal that cares about APDUs, not the RF technology (A vs B) underneath.

> Android cannot emulate NFC-B: the public HCE API only exposes Type-A. A
> genuine "B-to-B" phone client is not possible; this is the closest viable
> approach.

## Build

Open the `android/` folder in **Android Studio** (it will create the Gradle
wrapper and sync), then Run on a physical NFC phone. Or from the CLI with a
local Gradle 8.x:

```bash
cd android
gradle wrapper        # once, to generate ./gradlew
./gradlew assembleDebug
./gradlew installDebug # with a device attached (USB debugging on)
```

An emulator has no NFC — you need a real phone with HCE
(`android.hardware.nfc.hce`).

## Run

1. **Server must listen on the LAN**, not loopback, so the phone can reach it:
   ```powershell
   .\target\release\s380-relay.exe server --listen 0.0.0.0:7878 --tech b --reader port400
   ```
   Allow it through the Windows firewall, and make sure the phone is on the
   same network. Find the PC's IP with `ipconfig`.
2. Open the app, enter the PC's **IP** and **port** (7878), tap **Save**.
3. Enable the app for contactless if your device asks: **NFC settings** button →
   OEM NFC / "Other" card-emulation category → pick this app. `category="other"`
   AIDs sometimes need the app selected as the handler.
4. Tap the phone to the terminal/reader. The live log shows `<=` (APDU in from
   the terminal) and `=>` (response relayed back from the real card).

## AID routing (important)

Android only routes a SELECT to this app if the terminal's **AID** matches an
entry in `app/src/main/res/xml/apduservice.xml`. There is no true catch-all, so
that file registers broad payment/ID prefixes as a best-effort net.

**When you learn the real card's AID** (from the first SELECT in the log, the
server-side card info, or a capture), add it as an exact `<aid-filter>` and
reinstall — exact matches route most reliably.

## Status / limitations

- **End-to-end verified** against an RC-S300 card side: tapping a terminal to the
  phone connects to the server, activates the real card and relays real APDU
  responses back. (Type B on the RC-S300 needs the patched `felica` fork the
  server build already uses.)
- **Timing:** each APDU makes a terminal→phone→server→card round trip. On a LAN
  this is usually within the ISO-DEP frame-waiting time, but a slow link or a
  slow card may trip the terminal's timeout. The app cannot send S(WTX).
  To keep the first APDU fast, the app connects and runs `get_card` ahead of
  the tap (when the app is opened or the HCE service starts) and keeps the
  link across taps. Open the app once before tapping; a cold first tap may
  still time out on strict terminals (e.g. the Sony PC/SC driver) and succeed
  on the next one.
- The terminal sees the **phone's** random Type-A UID, not the real card's. A
  terminal that checks UID or enforces Type B will reject the relay.
```
