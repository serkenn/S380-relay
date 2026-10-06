package com.s380relay.client

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.Build
import android.os.Bundle
import android.provider.Settings as AndroidSettings
import android.widget.Button
import android.widget.EditText
import android.widget.ScrollView
import android.widget.TextView
import android.widget.Toast
import androidx.appcompat.app.AppCompatActivity
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * Minimal UI: set the relay server IP/port and watch the live APDU log that the
 * HCE service broadcasts. The relay itself runs in RelayHostApduService whenever
 * a terminal taps the phone — the activity does not need to be open for it.
 */
class MainActivity : AppCompatActivity() {

    private lateinit var logView: TextView
    private lateinit var scroll: ScrollView
    private val timeFmt = SimpleDateFormat("HH:mm:ss", Locale.US)

    private val logReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context?, intent: Intent?) {
            val line = intent?.getStringExtra(RelayHostApduService.EXTRA_LINE) ?: return
            appendLog(line)
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_main)

        val hostEdit = findViewById<EditText>(R.id.host)
        val portEdit = findViewById<EditText>(R.id.port)
        logView = findViewById(R.id.log)
        scroll = findViewById(R.id.scroll)

        val (host, port) = Settings.load(this)
        hostEdit.setText(host)
        portEdit.setText(port.toString())

        findViewById<Button>(R.id.save).setOnClickListener {
            val p = portEdit.text.toString().toIntOrNull() ?: Settings.DEFAULT_PORT
            Settings.save(this, hostEdit.text.toString(), p)
            Toast.makeText(this, "Saved", Toast.LENGTH_SHORT).show()
            appendLog(".. saved ${hostEdit.text}:$p")
        }
        findViewById<Button>(R.id.nfcSettings).setOnClickListener {
            // Card-emulation / NFC settings vary by OEM; NFC settings is the
            // portable entry point to enable this app for contactless.
            startActivity(Intent(AndroidSettings.ACTION_NFC_SETTINGS))
        }
        findViewById<Button>(R.id.clear).setOnClickListener { logView.text = "" }
    }

    override fun onResume() {
        super.onResume()
        val filter = IntentFilter(RelayHostApduService.ACTION_LOG)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            registerReceiver(logReceiver, filter, Context.RECEIVER_NOT_EXPORTED)
        } else {
            @Suppress("UnspecifiedRegisterReceiverFlag")
            registerReceiver(logReceiver, filter)
        }
    }

    override fun onPause() {
        super.onPause()
        try { unregisterReceiver(logReceiver) } catch (_: Exception) {}
    }

    private fun appendLog(line: String) {
        logView.append("${timeFmt.format(Date())}  $line\n")
        scroll.post { scroll.fullScroll(ScrollView.FOCUS_DOWN) }
    }
}
