package com.whilecode.cliproam.share

import android.content.Context
import android.os.Build
import androidx.work.Constraints
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.OutOfQuotaPolicy
import androidx.work.WorkManager
import androidx.work.Worker
import androidx.work.WorkerParameters
import androidx.work.workDataOf
import org.json.JSONObject

internal object ShareNative {
    init { System.loadLibrary("cliproam_lib") }
    external fun call(input: String): String
    var onChanged: (() -> Unit)? = null

    // Rust returns task IDs; scheduling and Worker lifecycle stay in Kotlin.
    fun dispatch(context: Context, input: String): String {
        val files = context.filesDir
        val args = JSONObject(input).put("files", files.absolutePath)
            .put("inbox", java.io.File(files, "cliproam-shares").absolutePath)
        val output = JSONObject(call(args.toString()))
        if (output.optBoolean("ok")) {
            val jobs = output.optJSONObject("value")?.optJSONArray("jobs")
            if (jobs != null) for (index in 0 until jobs.length()) {
                val id = jobs.getString(index)
                val request = OneTimeWorkRequestBuilder<ShareUploadWorker>()
                    .setInputData(workDataOf("id" to id))
                    .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
                    .apply {
                        if (Build.VERSION.SDK_INT >= 31) {
                            setExpedited(OutOfQuotaPolicy.RUN_AS_NON_EXPEDITED_WORK_REQUEST)
                        }
                    }.build()
                try { WorkManager.getInstance(context.applicationContext)
                    .enqueueUniqueWork("cliproam-share-$id", ExistingWorkPolicy.KEEP, request)
                    .result.get() } catch (error: Exception) {
                        call(JSONObject().put("command", "defer").put("id", id)
                            .put("files", files.absolutePath).put("inbox", java.io.File(files, "cliproam-shares").absolutePath).toString())
                        throw error
                    }
            }
        }
        onChanged?.invoke()
        return output.toString()
    }
}

class ShareUploadWorker(context: Context, params: WorkerParameters) : Worker(context, params) {
    override fun doWork(): Result {
        val input = JSONObject().put("command", "upload").put("id", inputData.getString("id")).put("attempt", runAttemptCount)
        return try {
            val output = JSONObject(ShareNative.dispatch(applicationContext, input.toString()))
            when (output.optJSONObject("value")?.optString("status")) {
                "success" -> Result.success()
                "retry" -> Result.retry()
                else -> Result.failure()
            }
        } catch (_: Exception) {
            if (runAttemptCount < 3) Result.retry() else Result.failure()
        }
    }
}
