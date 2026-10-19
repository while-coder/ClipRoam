package com.whilecode.cliproam.share

import android.app.Activity
import android.app.AlertDialog
import android.content.Intent
import android.os.Bundle
import android.widget.TextView
import org.json.JSONObject

// A small native share surface; no WebView/login startup is needed when an
// authenticated account is already available to the upload worker.
class ShareActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(TextView(this).apply { text = "正在接收分享…"; setPadding(32, 64, 32, 32) })
        if (savedInstanceState != null) { finish(); return }
        Thread {
            try {
                val result = JSONObject(ShareNative.call(this, "{\"command\":\"stage\"}"))
                check(result.optBoolean("ok")) { result.optString("error", "接收分享失败") }
                runOnUiThread {
                    if (isFinishing || isDestroyed) return@runOnUiThread
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
}
