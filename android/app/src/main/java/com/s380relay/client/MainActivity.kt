package com.s380relay.client

import android.content.Intent
import android.os.Bundle
import android.provider.Settings as AndroidSettings
import android.widget.Button
import android.widget.EditText
import android.widget.ScrollView
import android.widget.TextView
import android.widget.Toast
import androidx.appcompat.app.AppCompatActivity

/**
 * Minimal UI: set the relay server IP/port and watch the APDU log kept by
 * RelayLink. The relay itself runs in RelayHostApduService whenever a terminal
 * taps the phone — the activity does not need to be open for it, but opening
 * it connects to the server ahead of the tap. Lines logged while the activity
 * was in the background are shown when it comes back.
 */
class MainActivity : AppCompatActivity() {

    private lateinit var logView: TextView
    private lateinit var scroll: ScrollView

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
            RelayLink.log(".. saved ${hostEdit.text}:$p")
            RelayLink.reconnect(this)
        }
        findViewById<Button>(R.id.nfcSettings).setOnClickListener {
            // Card-emulation / NFC settings vary by OEM; NFC settings is the
            // portable entry point to enable this app for contactless.
            startActivity(Intent(AndroidSettings.ACTION_NFC_SETTINGS))
        }
        findViewById<Button>(R.id.clear).setOnClickListener {
            RelayLink.clearLog()
            logView.text = ""
        }
    }

    override fun onResume() {
        super.onResume()
        logView.text = ""
        RelayLink.setLogListener { line -> runOnUiThread { appendLog(line) } }
        // Open the relay link before the tap; see RelayLink.
        RelayLink.warmUp(this)
    }

    override fun onPause() {
        super.onPause()
        RelayLink.setLogListener(null)
    }

    private fun appendLog(line: String) {
        logView.append("$line\n")
        scroll.post { scroll.fullScroll(ScrollView.FOCUS_DOWN) }
    }
}
