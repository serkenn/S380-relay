package com.s380relay.client

import android.content.Intent
import android.nfc.cardemulation.HostApduService
import android.os.Bundle
import android.util.Log
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

/**
 * Presents a Type-A ISO-DEP (Type-4) card to the terminal and relays every
 * APDU to the Rust relay server over TCP. The real (possibly Type-B) card lives
 * on the server's reader; only ISO 7816-4 APDUs cross the link.
 *
 * processCommandApdu must not block the main thread, so each APDU is handed to
 * a single-threaded executor (preserving order) and the response is delivered
 * asynchronously via sendResponseApdu().
 */
class RelayHostApduService : HostApduService() {

    private val worker: ExecutorService = Executors.newSingleThreadExecutor()
    @Volatile private var client: RelayClient? = null

    override fun processCommandApdu(commandApdu: ByteArray?, extras: Bundle?): ByteArray? {
        val apdu = commandApdu ?: return SW_ERROR
        log("<= ${RelayClient.toHex(apdu)}")
        worker.execute {
            val resp = try {
                relay(apdu)
            } catch (e: Exception) {
                log("!! ${e.message}")
                closeClient()
                SW_ERROR
            }
            log("=> ${RelayClient.toHex(resp)}")
            sendResponseApdu(resp)
        }
        // Response is sent asynchronously above.
        return null
    }

    private fun relay(apdu: ByteArray): ByteArray {
        val c = ensureConnected()
        return c.exchange(apdu, EXCHANGE_TIMEOUT_MS)
    }

    private fun ensureConnected(): RelayClient {
        client?.let { if (it.isConnected) return it }
        val (host, port) = Settings.load(this)
        if (host.isBlank()) throw RelayException("server host not set — open the app and save it")
        log(".. connecting to $host:$port")
        val c = RelayClient(host, port)
        val info = c.connectAndGetCard()
        log("++ card: $info")
        client = c
        return c
    }

    override fun onDeactivated(reason: Int) {
        // Field lost or a different AID selected: drop the link so the next
        // activation re-runs get_card against a freshly activated card.
        log(".. deactivated (reason=$reason)")
        closeClient()
    }

    override fun onDestroy() {
        closeClient()
        worker.shutdownNow()
        super.onDestroy()
    }

    private fun closeClient() {
        client?.close()
        client = null
    }

    private fun log(line: String) {
        Log.d(TAG, line)
        sendBroadcast(Intent(ACTION_LOG).setPackage(packageName).putExtra(EXTRA_LINE, line))
    }

    companion object {
        private const val TAG = "S380Relay"
        private const val EXCHANGE_TIMEOUT_MS = 3000
        // ISO 7816-4 "no precise diagnosis" status word, returned on relay failure.
        private val SW_ERROR = byteArrayOf(0x6F.toByte(), 0x00)

        const val ACTION_LOG = "com.s380relay.client.LOG"
        const val EXTRA_LINE = "line"
    }
}
