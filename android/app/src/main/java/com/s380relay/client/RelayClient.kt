package com.s380relay.client

import org.json.JSONObject
import java.io.BufferedReader
import java.io.InputStreamReader
import java.io.OutputStream
import java.net.InetSocketAddress
import java.net.Socket

/**
 * One TCP connection to the Rust relay server, speaking its newline-delimited
 * JSON protocol (see src/protocol.rs):
 *   client -> {"type":"get_card"}             server -> {"type":"card","tech":..,"info":..}
 *   client -> {"type":"apdu","data":hex,..}   server -> {"type":"apdu","data":hex}
 *   (either) server -> {"type":"error","message":..}
 *
 * Not thread-safe: drive it from a single worker thread.
 */
class RelayClient(
    private val host: String,
    private val port: Int,
    private val connectTimeoutMs: Int = 3000,
    private val readTimeoutMs: Int = 5000,
) {
    private var socket: Socket? = null
    private var reader: BufferedReader? = null
    private var writer: OutputStream? = null

    val isConnected: Boolean get() = socket?.isConnected == true && socket?.isClosed == false

    /** Opens the socket and performs get_card. Returns the card "info" string. */
    fun connectAndGetCard(): String {
        close()
        val s = Socket()
        s.tcpNoDelay = true
        s.connect(InetSocketAddress(host, port), connectTimeoutMs)
        s.soTimeout = readTimeoutMs
        socket = s
        reader = BufferedReader(InputStreamReader(s.getInputStream(), Charsets.UTF_8))
        writer = s.getOutputStream()

        sendLine(JSONObject().put("type", "get_card").toString())
        val resp = JSONObject(readLine())
        return when (resp.optString("type")) {
            "card" -> "tech=${resp.optString("tech")} info=${resp.optString("info")}"
            "error" -> throw RelayException(resp.optString("message", "server error"))
            else -> throw RelayException("unexpected get_card reply: $resp")
        }
    }

    /** Relays one command APDU and returns the response APDU bytes. */
    fun exchange(apdu: ByteArray, timeoutMs: Int?): ByteArray {
        val req = JSONObject().put("type", "apdu").put("data", toHex(apdu))
        if (timeoutMs != null) req.put("timeout_ms", timeoutMs)
        sendLine(req.toString())
        val resp = JSONObject(readLine())
        return when (resp.optString("type")) {
            "apdu" -> fromHex(resp.optString("data"))
            "error" -> throw RelayException(resp.optString("message", "server error"))
            else -> throw RelayException("unexpected apdu reply: $resp")
        }
    }

    fun close() {
        try { socket?.close() } catch (_: Exception) {}
        socket = null; reader = null; writer = null
    }

    private fun sendLine(line: String) {
        val w = writer ?: throw RelayException("not connected")
        w.write((line + "\n").toByteArray(Charsets.UTF_8))
        w.flush()
    }

    private fun readLine(): String =
        reader?.readLine() ?: throw RelayException("connection closed by server")

    companion object {
        private val HEX = "0123456789abcdef".toCharArray()

        fun toHex(bytes: ByteArray): String {
            val sb = StringBuilder(bytes.size * 2)
            for (b in bytes) {
                val v = b.toInt() and 0xff
                sb.append(HEX[v ushr 4]).append(HEX[v and 0x0f])
            }
            return sb.toString()
        }

        fun fromHex(s: String): ByteArray {
            val clean = s.trim()
            require(clean.length % 2 == 0) { "odd hex length" }
            val out = ByteArray(clean.length / 2)
            for (i in out.indices) {
                out[i] = clean.substring(i * 2, i * 2 + 2).toInt(16).toByte()
            }
            return out
        }
    }
}

class RelayException(message: String) : Exception(message)
