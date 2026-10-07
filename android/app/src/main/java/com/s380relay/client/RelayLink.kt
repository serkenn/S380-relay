package com.s380relay.client

import android.content.Context
import android.util.Log
import java.io.IOException
import java.net.SocketTimeoutException
import java.text.SimpleDateFormat
import java.util.ArrayDeque
import java.util.Date
import java.util.Locale
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

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
 *
 * The log is kept here rather than only broadcast, so lines written while the
 * UI is in the background still show up when it is reopened.
 */
object RelayLink {
    private const val TAG = "S380Relay"
    private const val EXCHANGE_TIMEOUT_MS = 3000
    // Android keeps the terminal waiting (S(WTX)) until the app answers, so a
    // lost or stuck relay would hang the terminal forever. Answer SW_ERROR
    // ourselves after this long; a late real response is then dropped.
    private const val RESPONSE_DEADLINE_MS = 5000L
    private const val LOG_LIMIT = 500
    // ISO 7816-4 "no precise diagnosis" status word, returned on relay failure.
    private val SW_ERROR = byteArrayOf(0x6F.toByte(), 0x00)

    private val worker = Executors.newSingleThreadExecutor()
    private val watchdog = Executors.newSingleThreadScheduledExecutor()
    private var client: RelayClient? = null

    private val timeFmt = SimpleDateFormat("HH:mm:ss", Locale.US)
    private val history = ArrayDeque<String>()
    private var listener: ((String) -> Unit)? = null

    /** Opens the link in the background if it is not already up. */
    fun warmUp(ctx: Context) {
        val app = ctx.applicationContext
        worker.execute {
            try {
                ensureConnected(app)
            } catch (e: Exception) {
                log("!! warm-up failed: ${e.message}")
                close()
            }
        }
    }

    /** Drops the link (e.g. after the server address changed) and reopens it. */
    fun reconnect(ctx: Context) {
        worker.execute { close() }
        warmUp(ctx)
    }

    /**
     * Relays [apdu] on the worker thread and hands the response to [onResponse]
     * exactly once: the card's answer, or SW_ERROR if none arrives within
     * [RESPONSE_DEADLINE_MS].
     */
    fun exchange(ctx: Context, apdu: ByteArray, onResponse: (ByteArray) -> Unit) {
        val app = ctx.applicationContext
        val answered = AtomicBoolean(false)
        val deadline = watchdog.schedule({
            if (answered.compareAndSet(false, true)) {
                log("!! no response within ${RESPONSE_DEADLINE_MS} ms, answering ${RelayClient.toHex(SW_ERROR)}")
                onResponse(SW_ERROR)
            }
        }, RESPONSE_DEADLINE_MS, TimeUnit.MILLISECONDS)
        worker.execute {
            val resp = try {
                relay(app, apdu)
            } catch (e: Exception) {
                log("!! ${e.message}")
                close()
                SW_ERROR
            }
            deadline.cancel(false)
            if (answered.compareAndSet(false, true)) {
                log("=> ${RelayClient.toHex(resp)}")
                onResponse(resp)
            } else {
                log(".. late response dropped: ${RelayClient.toHex(resp)}")
            }
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
            log(".. stale link (${e.message}), reconnecting")
            close()
            ensureConnected(ctx).exchange(apdu, EXCHANGE_TIMEOUT_MS)
        }
    }

    private fun ensureConnected(ctx: Context): RelayClient {
        client?.let { if (it.isConnected) return it }
        val (host, port) = Settings.load(ctx)
        if (host.isBlank()) throw RelayException("server host not set — open the app and save it")
        log(".. connecting to $host:$port")
        val c = RelayClient(host, port)
        val info = c.connectAndGetCard()
        log("++ card: $info")
        client = c
        return c
    }

    private fun close() {
        client?.close()
        client = null
    }

    /** Appends a timestamped line to the log and passes it to the listener. */
    fun log(line: String) {
        Log.d(TAG, line)
        val stamped = "${synchronized(timeFmt) { timeFmt.format(Date()) }}  $line"
        synchronized(history) {
            history.addLast(stamped)
            while (history.size > LOG_LIMIT) history.removeFirst()
            listener?.invoke(stamped)
        }
    }

    /**
     * Sets the log listener (null to remove). The current history is replayed
     * to it first, under the same lock, so no line is lost or duplicated.
     * The listener is called on whichever thread logged the line.
     */
    fun setLogListener(l: ((String) -> Unit)?) {
        synchronized(history) {
            listener = l
            if (l != null) for (line in history) l(line)
        }
    }

    fun clearLog() {
        synchronized(history) { history.clear() }
    }
}
