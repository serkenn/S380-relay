package com.s380relay.client

import android.content.Context
import android.content.Intent
import android.util.Log
import java.io.IOException
import java.net.SocketTimeoutException
import java.util.concurrent.Executors

/**
 * Process-wide relay link shared by the HCE service and the UI.
 *
 * A cold link costs a TCP connect plus get_card (a full card activation on the
 * server, slow for Type B). Terminals such as the Sony PC/SC driver give up on
 * the first APDU long before that finishes, drop the card and start over, so a
 * per-tap link never gets anywhere. The link is therefore opened ahead of the
 * tap ([warmUp]) and kept across field deactivations; a tap then only pays one
 * APDU round trip.
 *
 * All network I/O runs on one worker thread, which also keeps APDUs in order.
 * [client] is touched only from that thread.
 */
object RelayLink {
    private const val TAG = "S380Relay"
    private const val EXCHANGE_TIMEOUT_MS = 3000
    // ISO 7816-4 "no precise diagnosis" status word, returned on relay failure.
    private val SW_ERROR = byteArrayOf(0x6F.toByte(), 0x00)

    const val ACTION_LOG = "com.s380relay.client.LOG"
    const val EXTRA_LINE = "line"

    private val worker = Executors.newSingleThreadExecutor()
    private var client: RelayClient? = null

    /** Opens the link in the background if it is not already up. */
    fun warmUp(ctx: Context) {
        val app = ctx.applicationContext
        worker.execute {
            try {
                ensureConnected(app)
            } catch (e: Exception) {
                log(app, "!! warm-up failed: ${e.message}")
                close()
            }
        }
    }

    /** Drops the link (e.g. after the server address changed) and reopens it. */
    fun reconnect(ctx: Context) {
        worker.execute { close() }
        warmUp(ctx)
    }

    /** Relays [apdu] on the worker thread and hands the response to [onResponse]. */
    fun exchange(ctx: Context, apdu: ByteArray, onResponse: (ByteArray) -> Unit) {
        val app = ctx.applicationContext
        worker.execute {
            val resp = try {
                relay(app, apdu)
            } catch (e: Exception) {
                log(app, "!! ${e.message}")
                close()
                SW_ERROR
            }
            log(app, "=> ${RelayClient.toHex(resp)}")
            onResponse(resp)
        }
    }

    private fun relay(ctx: Context, apdu: ByteArray): ByteArray {
        val reused = client?.isConnected == true
        val c = ensureConnected(ctx)
        return try {
            c.exchange(apdu, EXCHANGE_TIMEOUT_MS)
        } catch (e: IOException) {
            // A kept-alive link may have been closed by the server while idle;
            // that surfaces on first use. Reconnect once — unless the read
            // timed out, in which case the card may already have run the APDU.
            if (!reused || e is SocketTimeoutException) throw e
            log(ctx, ".. stale link (${e.message}), reconnecting")
            close()
            ensureConnected(ctx).exchange(apdu, EXCHANGE_TIMEOUT_MS)
        }
    }

    private fun ensureConnected(ctx: Context): RelayClient {
        client?.let { if (it.isConnected) return it }
        val (host, port) = Settings.load(ctx)
        if (host.isBlank()) throw RelayException("server host not set — open the app and save it")
        log(ctx, ".. connecting to $host:$port")
        val c = RelayClient(host, port)
        val info = c.connectAndGetCard()
        log(ctx, "++ card: $info")
        client = c
        return c
    }

    private fun close() {
        client?.close()
        client = null
    }

    fun log(ctx: Context, line: String) {
        Log.d(TAG, line)
        ctx.sendBroadcast(Intent(ACTION_LOG).setPackage(ctx.packageName).putExtra(EXTRA_LINE, line))
    }
}
