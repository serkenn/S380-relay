package com.s380relay.client

import android.nfc.cardemulation.HostApduService
import android.os.Bundle

/**
 * Presents a Type-A ISO-DEP (Type-4) card to the terminal and relays every
 * APDU to the Rust relay server over TCP. The real (possibly Type-B) card lives
 * on the server's reader; only ISO 7816-4 APDUs cross the link.
 *
 * processCommandApdu must not block the main thread, so each APDU is handed to
 * [RelayLink]'s worker and the response is delivered asynchronously via
 * sendResponseApdu(). The link outlives both taps and this service.
 */
class RelayHostApduService : HostApduService() {

    override fun onCreate() {
        super.onCreate()
        RelayLink.log(".. HCE service started")
        RelayLink.warmUp(this)
    }

    override fun processCommandApdu(commandApdu: ByteArray?, extras: Bundle?): ByteArray? {
        val apdu = commandApdu ?: return SW_ERROR
        RelayLink.log("<= ${RelayClient.toHex(apdu)}")
        RelayLink.exchange(this, apdu) { sendResponseApdu(it) }
        // Response is sent asynchronously above.
        return null
    }

    override fun onDeactivated(reason: Int) {
        // Keep the link: rebuilding it on the next tap is what made terminals
        // time out. A card that dropped out meanwhile is re-activated by the
        // server when an APDU fails.
        RelayLink.log(".. deactivated (reason=$reason)")
    }

    companion object {
        // ISO 7816-4 "no precise diagnosis" status word.
        private val SW_ERROR = byteArrayOf(0x6F.toByte(), 0x00)
    }
}
