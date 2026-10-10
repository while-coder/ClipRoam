package com.whilecode.cliproam.share

import android.app.Activity
import android.content.Intent
import android.database.Cursor
import android.net.Uri
import android.os.Build
import android.provider.OpenableColumns
import android.provider.DocumentsContract
import android.webkit.MimeTypeMap
import android.webkit.WebView
import app.tauri.annotation.Command
import app.tauri.annotation.ActivityCallback
import androidx.activity.result.ActivityResult
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

@InvokeArg
class ExportDirectoryArgs {
    lateinit var source: String
    lateinit var uri: String
}

@InvokeArg
class CopyDocumentsArgs {
    lateinit var uris: List<String>
    lateinit var directory: String
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
    fun pickDirectory(invoke: Invoke) {
        try {
            val intent = Intent(Intent.ACTION_OPEN_DOCUMENT_TREE).addFlags(
                Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION
            )
            startActivityForResult(invoke, intent, "directoryPickerResult")
        } catch (error: Exception) {
            invoke.reject(error.message ?: "无法打开目录选择框")
        }
    }

    @Command
    fun copyDocuments(invoke: Invoke) {
        val args = invoke.parseArgs(CopyDocumentsArgs::class.java)
        scope.launch {
            var directory: File? = null
            try {
                val targetDirectory = File(args.directory).canonicalFile
                require(targetDirectory.path.startsWith(activity.filesDir.canonicalPath + File.separator)) {
                    "文件导入目录必须位于应用数据目录中"
                }
                require(!targetDirectory.exists() && targetDirectory.mkdirs()) { "无法创建文件导入目录" }
                directory = targetDirectory
                val paths = args.uris.mapIndexed { index, value ->
                    val uri = Uri.parse(value)
                    require(uri.scheme == "content") { "不支持的文件地址" }
                    val mime = activity.contentResolver.getType(uri) ?: "application/octet-stream"
                    val name = uniqueName(targetDirectory, displayName(uri, mime, index))
                    val target = File(targetDirectory, name)
                    activity.contentResolver.openInputStream(uri).use { input ->
                        requireNotNull(input) { "无法读取文件：$name" }
                        target.outputStream().use(input::copyTo)
                    }
                    target.absolutePath
                }
                invoke.resolveObject(mapOf("paths" to paths))
            } catch (error: Exception) {
                directory?.deleteRecursively()
                invoke.reject(error.message ?: "无法导入所选文件")
            }
        }
    }

    @ActivityCallback
    fun directoryPickerResult(invoke: Invoke, result: ActivityResult) {
        val uri = if (result.resultCode == Activity.RESULT_OK) result.data?.data else null
        if (result.resultCode == Activity.RESULT_OK && uri == null) {
            invoke.reject("目录选择结果为空")
        } else {
            invoke.resolveObject(mapOf("uri" to uri?.toString()))
        }
    }

    @Command
    fun exportDirectory(invoke: Invoke) {
        val args = invoke.parseArgs(ExportDirectoryArgs::class.java)
        scope.launch {
            val createdRoots = mutableListOf<Uri>()
            try {
                val source = File(args.source)
                require(source.isDirectory) { "导出文件尚未准备完成" }
                val tree = Uri.parse(args.uri)
                val destination = DocumentsContract.buildDocumentUriUsingTree(
                    tree, DocumentsContract.getTreeDocumentId(tree)
                )
                val files = source.listFiles()?.sumOf { file ->
                    copyDocument(file, destination, createdRoots)
                } ?: throw IllegalStateException("无法读取导出目录")
                invoke.resolveObject(mapOf("files" to files))
            } catch (error: Exception) {
                // Only newly created documents are rolled back; pre-existing
                // documents in the user's directory are never overwritten.
                createdRoots.forEach { uri ->
                    try { DocumentsContract.deleteDocument(activity.contentResolver, uri) } catch (_: Exception) {}
                }
                invoke.reject(error.message ?: "无法保存到所选目录")
            }
        }
    }

    private fun copyDocument(source: File, parent: Uri, createdRoots: MutableList<Uri>? = null): Int {
        val mime = if (source.isDirectory) DocumentsContract.Document.MIME_TYPE_DIR else
            MimeTypeMap.getSingleton().getMimeTypeFromExtension(source.extension.lowercase())
                ?: "application/octet-stream"
        val target = DocumentsContract.createDocument(activity.contentResolver, parent, mime, source.name)
            ?: throw IllegalStateException("无法创建文件：${source.name}")
        createdRoots?.add(target)
        if (source.isDirectory) {
            return source.listFiles()?.sumOf { copyDocument(it, target) }
                ?: throw IllegalStateException("无法读取目录：${source.name}")
        }
        activity.contentResolver.openOutputStream(target, "w").use { output ->
            if (output == null) throw IllegalStateException("无法写入文件：${source.name}")
            source.inputStream().use { input -> input.copyTo(output) }
        }
        return 1
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
