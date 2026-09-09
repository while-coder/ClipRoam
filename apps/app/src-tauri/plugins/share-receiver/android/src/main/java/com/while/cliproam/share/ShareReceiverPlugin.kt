package com.while.cliproam.share

import android.app.Activity
import android.content.Intent
import android.database.Cursor
import android.net.Uri
import android.os.Build
import android.provider.OpenableColumns
import android.webkit.MimeTypeMap
import android.webkit.WebView
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import com.fasterxml.jackson.databind.JsonNode
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import java.io.File
import java.util.UUID

@InvokeArg
class AcknowledgeArgs {
    lateinit var id: String
}

@TauriPlugin
class ShareReceiverPlugin(private val activity: Activity) : Plugin(activity) {
    private val scope = CoroutineScope(Dispatchers.IO + SupervisorJob())
    private val inbox = File(activity.filesDir, "cliproam-shares")

    override fun load(webView: WebView) {
        enqueue(activity.intent)
    }

    override fun onNewIntent(intent: Intent) {
        enqueue(intent)
    }

    override fun onDestroy() {
        scope.cancel()
    }

    @Command
    fun pending(invoke: Invoke) {
        scope.launch {
            try {
                val payloads = inbox
                    .listFiles()
                    .orEmpty()
                    .asSequence()
                    .filter { it.isDirectory }
                    .sortedBy { it.name }
                    .mapNotNull { directory ->
                        val manifest = File(directory, "manifest.json")
                        if (!manifest.isFile) null else jsonMapper().readTree(manifest)
                    }
                    .toList<JsonNode>()
                invoke.resolveObject(payloads)
            } catch (error: Exception) {
                invoke.reject(error.message ?: "无法读取系统分享")
            }
        }
    }

    @Command
    fun acknowledge(invoke: Invoke) {
        val args = invoke.parseArgs(AcknowledgeArgs::class.java)
        if (!args.id.matches(Regex("^[0-9a-fA-F-]{36}$"))) {
            invoke.reject("分享请求标识不合法")
            return
        }
        scope.launch {
            try {
                val directory = File(inbox, args.id)
                if (directory.exists() && !directory.deleteRecursively()) {
                    throw IllegalStateException("无法清理已处理的分享内容")
                }
                invoke.resolve()
            } catch (error: Exception) {
                invoke.reject(error.message ?: "无法清理已处理的分享内容")
            }
        }
    }

    private fun enqueue(intent: Intent?) {
        if (intent?.action != Intent.ACTION_SEND && intent?.action != Intent.ACTION_SEND_MULTIPLE) return
        // Activity recreation must not import the same share intent twice.
        activity.intent = Intent(intent).setAction(null)
        scope.launch {
            try {
                stage(intent)
            } catch (error: Exception) {
                val payload = JSObject()
                payload.put("error", error.message ?: "无法接收系统分享")
                trigger("received", payload)
            }
        }
    }

    private fun stage(intent: Intent) {
        val text = intent.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString()?.takeIf { it.isNotBlank() }
            ?: intent.getStringArrayListExtra(Intent.EXTRA_TEXT)?.joinToString("\n")?.takeIf { it.isNotBlank() }
        val html = intent.getStringExtra(Intent.EXTRA_HTML_TEXT)?.takeIf { it.isNotBlank() }
        val uris = sharedUris(intent)
        if (text == null && uris.isEmpty()) return

        inbox.mkdirs()
        val id = UUID.randomUUID().toString()
        val directory = File(inbox, id)
        if (!directory.mkdirs()) throw IllegalStateException("无法创建系统分享缓存")

        try {
            val items = uris.mapIndexed { index, uri ->
                val mimeType = activity.contentResolver.getType(uri)
                    ?: intent.type
                    ?: "application/octet-stream"
                val name = uniqueName(directory, displayName(uri, mimeType, index))
                val target = File(directory, name)
                activity.contentResolver.openInputStream(uri).use { input ->
                    if (input == null) throw IllegalArgumentException("无法读取分享文件：$name")
                    target.outputStream().use(input::copyTo)
                }
                mapOf(
                    "path" to target.absolutePath,
                    "name" to name,
                    "mimeType" to mimeType,
                )
            }

            val manifest = mapOf(
                "id" to id,
                "text" to text,
                "html" to html,
                "items" to items,
            )
            val temporary = File(directory, "manifest.json.tmp")
            jsonMapper().writeValue(temporary, manifest)
            val target = File(directory, "manifest.json")
            if (!temporary.renameTo(target)) throw IllegalStateException("无法保存系统分享清单")

            val payload = JSObject()
            payload.put("id", id)
            trigger("received", payload)
        } catch (error: Exception) {
            directory.deleteRecursively()
            throw error
        }
    }

    private fun sharedUris(intent: Intent): List<Uri> {
        val values = linkedMapOf<String, Uri>()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)?.let { values[it.toString()] = it }
            intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java)
                .orEmpty()
                .forEach { values[it.toString()] = it }
        } else {
            @Suppress("DEPRECATION")
            (intent.getParcelableExtra<Uri>(Intent.EXTRA_STREAM))?.let { values[it.toString()] = it }
            @Suppress("DEPRECATION")
            intent.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM)
                .orEmpty()
                .forEach { values[it.toString()] = it }
        }
        intent.clipData?.let { clip ->
            for (index in 0 until clip.itemCount) {
                clip.getItemAt(index).uri?.let { values[it.toString()] = it }
            }
        }
        return values.values.toList()
    }

    private fun displayName(uri: Uri, mimeType: String, index: Int): String {
        var cursor: Cursor? = null
        val providerName = try {
            cursor = activity.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
            if (cursor?.moveToFirst() == true) cursor.getString(0) else null
        } catch (_: Exception) {
            null
        } finally {
            cursor?.close()
        }
        val extension = MimeTypeMap.getSingleton().getExtensionFromMimeType(mimeType)
        val fallback = "shared-${index + 1}${extension?.let { ".$it" } ?: ""}"
        return sanitizeName(providerName ?: fallback)
    }

    private fun sanitizeName(value: String): String {
        val cleaned = value
            .replace(Regex("[\\\\/:*?\"<>|\\p{Cntrl}]"), "_")
            .trim()
            .trimEnd('.')
        return cleaned.take(180).ifBlank { "shared-file" }
    }

    private fun uniqueName(directory: File, preferred: String): String {
        if (!File(directory, preferred).exists()) return preferred
        val dot = preferred.lastIndexOf('.')
        val stem = if (dot > 0) preferred.substring(0, dot) else preferred
        val extension = if (dot > 0) preferred.substring(dot) else ""
        var counter = 2
        while (File(directory, "$stem ($counter)$extension").exists()) counter += 1
        return "$stem ($counter)$extension"
    }
}
