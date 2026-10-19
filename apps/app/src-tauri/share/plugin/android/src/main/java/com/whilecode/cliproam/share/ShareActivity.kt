package com.whilecode.cliproam.share

import android.app.Activity
import android.app.AlertDialog
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.provider.OpenableColumns
import android.widget.TextView
import org.json.JSONObject
import org.json.JSONArray
import java.io.File
import java.util.UUID

// A small native share surface; no WebView/login startup is needed when an
// authenticated account is already available to the upload worker.
class ShareActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(TextView(this).apply { text = "正在接收分享…"; setPadding(32, 64, 32, 32) })
        if (savedInstanceState != null) { finish(); return }
        Thread {
            try {
                val result = JSONObject(ShareNative.dispatch(this, JSONObject()
                    .put("command", "stage").put("share", receiveShare()).toString()))
                check(result.optBoolean("ok")) { result.optString("error", "接收分享失败") }
                runOnUiThread {
                    if (isFinishing || isDestroyed) return@runOnUiThread
                    if (result.getJSONObject("value").optBoolean("uploading")) {
                        finish()
                        return@runOnUiThread
                    }
                    val message = result.getJSONObject("value").getString("message")
                    AlertDialog.Builder(this).setTitle("分享到 ClipRoam").setMessage(message)
                        .setPositiveButton("完成") { _, _ -> finish() }
                        .apply { setNeutralButton("打开 ClipRoam") { _, _ ->
                            startActivity(Intent().setClassName(packageName, "$packageName.MainActivity").addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)); finish()
                        } }.setOnCancelListener { finish() }.show()
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
    private fun receiveShare(): JSONObject {
        val text = intent.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString()
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
            uris.forEachIndexed { index, uri ->
                val providerName = contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use {
                    if (it.moveToFirst()) it.getString(0) else null
                }
                val name = (providerName ?: "shared-${index + 1}")
                    .replace(Regex("[\\\\/:*?\"<>|\\p{Cntrl}]"), "_")
                    .trim().trimEnd('.').take(180).ifBlank { "shared-${index + 1}" }
                val target = File(directory, "files/$index/$name")
                check(target.parentFile!!.mkdirs()) { "无法创建分享缓存" }
                contentResolver.openInputStream(uri).use { input ->
                    requireNotNull(input) { "无法读取分享文件" }
                    target.outputStream().use(input::copyTo)
                }
                items.put(JSONObject().put("path", target.absolutePath).put("name", name)
                    .put("mimeType", contentResolver.getType(uri) ?: "application/octet-stream"))
            }
        } catch (error: Exception) {
            directory.deleteRecursively()
            throw error
        }
        return JSONObject().put("id", id).put("text", text ?: JSONObject.NULL)
            .put("html", intent.getStringExtra(Intent.EXTRA_HTML_TEXT) ?: JSONObject.NULL).put("items", items)
    }
}
