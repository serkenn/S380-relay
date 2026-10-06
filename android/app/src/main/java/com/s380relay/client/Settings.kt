package com.s380relay.client

import android.content.Context

/** Server host/port persisted in SharedPreferences. */
object Settings {
    private const val PREFS = "relay"
    private const val KEY_HOST = "host"
    private const val KEY_PORT = "port"
    const val DEFAULT_PORT = 7878

    fun load(ctx: Context): Pair<String, Int> {
        val p = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        return p.getString(KEY_HOST, "") .orEmpty() to p.getInt(KEY_PORT, DEFAULT_PORT)
    }

    fun save(ctx: Context, host: String, port: Int) {
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
            .putString(KEY_HOST, host.trim())
            .putInt(KEY_PORT, port)
            .apply()
    }
}
