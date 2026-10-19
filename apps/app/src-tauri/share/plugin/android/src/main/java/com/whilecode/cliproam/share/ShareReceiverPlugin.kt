package com.whilecode.cliproam.share

import android.app.Activity
import android.app.AlertDialog
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.provider.DocumentsContract
import android.provider.OpenableColumns
import android.webkit.MimeTypeMap
import android.widget.TextView
import androidx.activity.result.ActivityResult
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.File
import java.util.UUID
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import org.json.JSONArray
import org.json.JSONObject

@InvokeArg
class AcknowledgeArgs {
    lateinit var id: String
}
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

    private val inbox = File(activity.filesDir, "cliproam-shares")

    override fun onResume() {
        trigger("share-received", JSObject())
    }

    override fun onDestroy() {
        scope.cancel()
    }

    @Command
    fun pending(invoke: Invoke) {
        val args = invoke.parseArgs(PendingArgs::class.java)
        scope.launch {
            try {
                val shares = inbox.listFiles().orEmpty().filter { it.isDirectory }.sortedBy { it.name }
                    .mapNotNull { directory ->
                        val manifest = File(directory, "manifest.json")
                        if (!manifest.isFile) null else jsonMapper().readTree(manifest)
                    }
                    .filter { share ->
                        val owner = share.path("ownerScope")
                        owner.isMissingNode || owner.isNull || owner.asText() == args.scope
                    }
                invoke.resolveObject(shares)
            } catch (error: Exception) {
                invoke.reject(error.message ?: "无法读取系统分享")
            }
        }
    }

    @Command
    fun acknowledge(invoke: Invoke) {
        val args = invoke.parseArgs(AcknowledgeArgs::class.java)
        try {
            UUID.fromString(args.id)
        } catch (_: IllegalArgumentException) {
            invoke.reject("分享请求标识不合法")
            return
        }
        scope.launch {
            try {
                val directory = File(inbox, args.id)
                check(!directory.exists() || directory.deleteRecursively()) { "无法清理已处理的分享内容" }
                invoke.resolve()
            } catch (error: Exception) {
                invoke.reject(error.message ?: "无法清理已处理的分享内容")
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
        val providerName = try {
            activity.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use {
                if (it.moveToFirst()) it.getString(0) else null
            }
        } catch (_: Exception) {
            null
        }
        val extension = MimeTypeMap.getSingleton().getExtensionFromMimeType(mimeType)
        val fallback = "shared-${index + 1}${extension?.let { ".$it" } ?: ""}"
        return sanitizeFileName(providerName ?: fallback)
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

// Persist temporary share attachments before handing off to the main app.
class ShareActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(TextView(this).apply { text = "正在接收分享…"; setPadding(32, 64, 32, 32) })
        if (savedInstanceState != null) { finish(); return }
        Thread {
            try {
                receiveShare()
                runOnUiThread {
                    if (isFinishing || isDestroyed) return@runOnUiThread
                    try {
                        startActivity(Intent().setClassName(packageName, "$packageName.MainActivity")
                            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP))
                        finish()
                    } catch (error: Exception) {
                        AlertDialog.Builder(this).setTitle("分享已保存")
                            .setMessage("请打开 ClipRoam 后自动导入。")
                            .setPositiveButton("完成") { _, _ -> finish() }.show()
                    }
                }
            } catch (error: Exception) {
                runOnUiThread {
                    if (!isFinishing && !isDestroyed) AlertDialog.Builder(this).setTitle("接收分享失败")
                        .setMessage(error.message ?: "请重新分享").setPositiveButton("关闭") { _, _ -> finish() }
                        .setOnCancelListener { finish() }.show()
                }
            }
        }.start()
    }

    @Suppress("DEPRECATION")
    private fun receiveShare() {
        val text = intent.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString()?.takeIf { it.isNotBlank() }
            ?: intent.getStringArrayListExtra(Intent.EXTRA_TEXT)?.joinToString("\n")?.takeIf { it.isNotBlank() }
        val uris = linkedSetOf<Uri>()
        if (intent.action == Intent.ACTION_SEND_MULTIPLE) {
            intent.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM)?.let(uris::addAll)
        } else { intent.getParcelableExtra<Uri>(Intent.EXTRA_STREAM)?.let(uris::add) }
        intent.clipData?.let { clip ->
            for (index in 0 until clip.itemCount) clip.getItemAt(index).uri?.let(uris::add)
        }
        require(!text.isNullOrBlank() || uris.isNotEmpty()) { "没有可接收的分享内容" }
        val id = UUID.randomUUID().toString()
        val directory = File(filesDir, "cliproam-shares/$id")
        val items = JSONArray()
        try {
            check(directory.mkdirs()) { "无法创建分享缓存" }
            uris.forEachIndexed { index, uri ->
                val providerName = contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use {
                    if (it.moveToFirst()) it.getString(0) else null
                }
                val fallback = "shared-${index + 1}"
                val name = sanitizeFileName(providerName ?: fallback, fallback)
                val target = File(directory, "files/$index/$name")
                check(target.parentFile!!.mkdirs()) { "无法创建分享缓存" }
                contentResolver.openInputStream(uri).use { input ->
                    requireNotNull(input) { "无法读取分享文件" }
                    target.outputStream().use(input::copyTo)
                }
                items.put(JSONObject().put("path", target.absolutePath).put("name", name))
            }
            val share = JSONObject().put("id", id).put("text", text ?: JSONObject.NULL)
                .put("html", intent.getStringExtra(Intent.EXTRA_HTML_TEXT) ?: JSONObject.NULL).put("items", items)
            val temporary = File(directory, "manifest.json.tmp")
            temporary.writeText(share.toString())
            check(temporary.renameTo(File(directory, "manifest.json"))) { "无法保存分享清单" }
        } catch (error: Exception) {
            directory.deleteRecursively()
            throw error
        }
    }
}

private fun sanitizeFileName(value: String, fallback: String = "shared-file"): String = value
    .replace(Regex("[\\\\/:*?\"<>|\\p{Cntrl}]"), "_")
    .trim()
    .trimEnd('.')
    .take(180)
    .ifBlank { fallback }
