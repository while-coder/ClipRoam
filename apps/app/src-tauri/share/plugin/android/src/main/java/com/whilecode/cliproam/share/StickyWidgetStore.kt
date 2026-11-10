package com.whilecode.cliproam.share

import android.content.Context
import android.util.AtomicFile
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.net.URI
import java.util.UUID

/** Private native snapshot, excluded from backup. Never writes frontend IndexedDB. */
internal object StickyWidgetStore {
    private fun file(context: Context) = AtomicFile(File(context.noBackupFilesDir, "sticky-widget.json"))

    @Synchronized
    fun load(context: Context): JSONObject {
        val file = file(context)
        return try {
            JSONObject(file.openRead().bufferedReader().use { it.readText() })
        } catch (_: java.io.FileNotFoundException) { JSONObject() }
    }

    private fun write(context: Context, state: JSONObject) {
        val file = file(context)
        val stream = file.startWrite()
        try {
            stream.write(state.toString().toByteArray(Charsets.UTF_8))
            file.finishWrite(stream)
        } catch (error: Exception) {
            file.failWrite(stream)
            throw error
        }
    }

    fun endpoint(address: String): String {
        val uri = URI(address)
        require(uri.scheme in listOf("http", "https") && !uri.host.isNullOrBlank()
            && uri.userInfo == null && uri.query == null && uri.fragment == null) { "Invalid server address" }
        return address.trimEnd('/') + "/entries/sticky"
    }

    @Synchronized
    fun configure(context: Context, address: String?, token: String?) {
        require((address == null && token == null) || (!address.isNullOrBlank() && !token.isNullOrBlank()))
        if (address != null) endpoint(address)
        val previous = load(context)
        if (address != null && previous.optString("address") == address && previous.optString("token") == token) return
        val next = JSONObject().put("generation", UUID.randomUUID().toString())
        if (address != null) next.put("address", address).put("token", token)
        write(context, next)
    }

    /** Expose queued refreshes to the widget, including when waiting for a network. */
    @Synchronized
    fun begin(context: Context): Boolean {
        val state = load(context)
        if (state.optString("token").isBlank()) return false
        state.put("refreshing", true)
        write(context, state)
        return true
    }

    /** Ignore requests that finished after logout or an account switch. */
    @Synchronized
    fun complete(context: Context, generation: String, notes: JSONArray? = null, expired: Boolean = false, offline: Boolean = false): Boolean {
        val state = load(context)
        if (state.optString("generation") != generation) return false
        if (expired) {
            write(context, JSONObject().put("generation", UUID.randomUUID().toString()).put("requiresLogin", true))
        } else {
            if (notes != null) state.put("notes", notes).put("updatedAt", System.currentTimeMillis())
            state.put("offline", offline).put("refreshing", false)
            write(context, state)
        }
        return true
    }
}
