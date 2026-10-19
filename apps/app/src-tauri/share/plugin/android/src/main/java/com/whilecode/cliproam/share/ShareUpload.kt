package com.whilecode.cliproam.share

import android.content.Context
import androidx.work.Worker
import androidx.work.WorkerParameters
import org.json.JSONObject

internal object ShareNative {
    init { System.loadLibrary("cliproam_lib") }
    external fun call(context: Context, input: String): String
}

class ShareUploadWorker(context: Context, params: WorkerParameters) : Worker(context, params) {
    override fun doWork(): Result {
        val input = JSONObject().put("command", "upload").put("id", inputData.getString("id")).put("attempt", runAttemptCount)
        val output = JSONObject(ShareNative.call(applicationContext, input.toString()))
        return when (output.optJSONObject("value")?.optString("status")) {
            "success" -> Result.success()
            "retry" -> Result.retry()
            else -> Result.failure()
        }
    }
}
