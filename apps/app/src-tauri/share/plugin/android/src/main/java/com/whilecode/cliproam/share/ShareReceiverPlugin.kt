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
import org.json.JSONObject

@InvokeArg
class AcknowledgeArgs {
    lateinit var id: String
}
@InvokeArg
class ShareCallArgs { lateinit var input: Map<String, Any> }
@InvokeArg
class PendingArgs { lateinit var scope: String }

@InvokeArg
class CopyFileArgs {
    lateinit var source: String
    lateinit var parent: String
    lateinit var name: String
}
@InvokeArg
class CreateDirectoryArgs {
    lateinit var parent: String
    lateinit var name: String
}
@InvokeArg
class RemoveDocumentArgs {
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

    override fun load(webView: WebView) {
        ShareNative.onChanged = { activity.runOnUiThread { trigger("share-received", JSObject()) } }
    }

    override fun onResume() {
        scope.launch {
            try { ShareNative.dispatch(activity, "{\"command\":\"resume\"}") }
            catch (error: Exception) { android.util.Log.w("ClipRoam", "无法调度分享上传", error) }
        }
    }

    override fun onDestroy() {
        ShareNative.onChanged = null
        scope.cancel()
    }

    @Command
    fun call(invoke: Invoke) {
        val args = invoke.parseArgs(ShareCallArgs::class.java)
        scope.launch {
            try {
                val result = jsonMapper().readTree(ShareNative.dispatch(activity, JSONObject(args.input).toString()))
                if (result.path("ok").asBoolean()) invoke.resolveObject(result.path("value"))
                else invoke.reject(result.path("error").asText("分享处理失败"))
            } catch (error: Exception) {
                invoke.reject(error.message ?: "无法调度分享上传")
            }
        }
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
                // Tauri app_data_dir resolves to dataDir; profile caches live
                // under dataDir/histories, alongside the files subdirectory.
                require(targetDirectory.path.startsWith(activity.dataDir.canonicalPath + File.separator)) {
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

    private fun parentDocument(value: String): Uri {
        val uri = Uri.parse(value)
        require(uri.scheme == "content" && DocumentsContract.isTreeUri(uri)) { "不支持的目录地址" }
        return if (DocumentsContract.isDocumentUri(activity, uri)) uri else
            DocumentsContract.buildDocumentUriUsingTree(uri, DocumentsContract.getTreeDocumentId(uri))
    }

    private fun createDocument(parent: String, name: String, mime: String): Uri {
        require(name.isNotBlank() && name != "." && name != ".." && !name.contains('/') && !name.contains('\\')) { "文件名称不合法" }
        return DocumentsContract.createDocument(activity.contentResolver, parentDocument(parent), mime, name)
            ?: throw IllegalStateException("无法创建文件：$name")
    }

    @Command
    fun copyFile(invoke: Invoke) {
        val args = invoke.parseArgs(CopyFileArgs::class.java)
        scope.launch {
            var target: Uri? = null
            try {
                val source = File(args.source)
                require(source.isFile) { "复制源必须是文件" }
                val mime = MimeTypeMap.getSingleton().getMimeTypeFromExtension(args.name.substringAfterLast('.', "").lowercase())
                    ?: "application/octet-stream"
                val document = createDocument(args.parent, args.name, mime)
                target = document
                activity.contentResolver.openOutputStream(document, "w").use { output ->
                    requireNotNull(output) { "无法写入文件：${args.name}" }
                    source.inputStream().use { input -> input.copyTo(output) }
                }
                invoke.resolveObject(mapOf("uri" to document.toString()))
            } catch (error: Exception) {
                target?.let { try { DocumentsContract.deleteDocument(activity.contentResolver, it) } catch (_: Exception) {} }
                invoke.reject(error.message ?: "无法复制文件")
            }
        }
    }

    @Command
    fun createDirectory(invoke: Invoke) {
        val args = invoke.parseArgs(CreateDirectoryArgs::class.java)
        scope.launch {
            try {
                val uri = createDocument(args.parent, args.name, DocumentsContract.Document.MIME_TYPE_DIR)
                invoke.resolveObject(mapOf("uri" to uri.toString()))
            } catch (error: Exception) {
                invoke.reject(error.message ?: "无法创建目录")
            }
        }
    }

    @Command
    fun removeDocument(invoke: Invoke) {
        val args = invoke.parseArgs(RemoveDocumentArgs::class.java)
        scope.launch {
            try {
                val uri = Uri.parse(args.uri)
                require(uri.scheme == "content" && DocumentsContract.isTreeUri(uri) && DocumentsContract.isDocumentUri(activity, uri)
                    && DocumentsContract.getDocumentId(uri) != DocumentsContract.getTreeDocumentId(uri)) { "只能清理系统目录中的子项" }
                require(DocumentsContract.deleteDocument(activity.contentResolver, uri)) { "无法删除文件" }
                invoke.resolve()
            } catch (error: Exception) {
                invoke.reject(error.message ?: "无法删除文件")
            }
        }
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
