package com.whilecode.cliproam.share

import android.content.Context
import androidx.work.Constraints
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.Worker
import androidx.work.WorkerParameters
import org.json.JSONArray
import org.json.JSONObject
import java.net.HttpURLConnection
import java.net.URL
import java.util.concurrent.TimeUnit

internal object StickyWidgetSync {
    private const val IMMEDIATE = "cliproam-sticky-refresh"
    private const val PERIODIC = "cliproam-sticky-periodic"
    private val network = Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build()

    fun configure(context: Context) {
        val manager = WorkManager.getInstance(context)
        if (StickyWidgetStore.load(context).optString("token").isBlank()) {
            manager.cancelUniqueWork(IMMEDIATE)
            manager.cancelUniqueWork(PERIODIC)
        } else {
            schedule(context)
            refresh(context)
        }
    }

    fun schedule(context: Context) {
        if (StickyWidgetProvider.ids(context).isEmpty() || StickyWidgetStore.load(context).optString("token").isBlank()) return
        WorkManager.getInstance(context).enqueueUniquePeriodicWork(PERIODIC, ExistingPeriodicWorkPolicy.KEEP,
            PeriodicWorkRequestBuilder<StickyWidgetWorker>(30, TimeUnit.MINUTES)
                .setConstraints(network).setInitialDelay(30, TimeUnit.MINUTES).build())
    }

    fun stopPeriodic(context: Context) { WorkManager.getInstance(context).cancelUniqueWork(PERIODIC) }

    fun refresh(context: Context) {
        if (!StickyWidgetStore.begin(context)) return
        StickyWidgetProvider.render(context)
        // Replace pending work so a notification arriving during a fetch gets a fresh request.
        WorkManager.getInstance(context).enqueueUniqueWork(IMMEDIATE, ExistingWorkPolicy.REPLACE,
            OneTimeWorkRequestBuilder<StickyWidgetWorker>().setConstraints(network).build())
    }
}

class StickyWidgetWorker(context: Context, params: WorkerParameters) : Worker(context, params) {
    override fun doWork(): Result {
        val context = applicationContext
        val state = StickyWidgetStore.load(context)
        val token = state.optString("token")
        if (token.isBlank()) return Result.success()
        val generation = state.getString("generation")
        var connection: HttpURLConnection? = null
        try {
            connection = URL(StickyWidgetStore.endpoint(state.getString("address"))).openConnection() as HttpURLConnection
            connection.instanceFollowRedirects = false
            connection.connectTimeout = 15000
            connection.readTimeout = 20000
            connection.setRequestProperty("Authorization", "Bearer $token")
            connection.setRequestProperty("Accept", "application/json")
            when (connection.responseCode) {
                401, 403 -> {
                    if (!isStopped && StickyWidgetStore.complete(context, generation, expired = true)) {
                        StickyWidgetProvider.render(context)
                        StickyWidgetSync.configure(context)
                    }
                    return Result.success()
                }
                200 -> Unit
                else -> throw java.io.IOException("Widget sync failed")
            }
            val response = JSONObject(connection.inputStream.bufferedReader().use { it.readText() })
            val entries = response.getJSONArray("entries")
            val notes = JSONArray()
            for (index in 0 until entries.length()) {
                val entry = entries.getJSONObject(index)
                notes.put(JSONObject().put("id", entry.getString("id"))
                    .put("kind", entry.getString("kind"))
                    .put("content", entry.getString("content").take(1024)))
            }
            if (!isStopped && StickyWidgetStore.complete(context, generation, notes = notes)) StickyWidgetProvider.render(context)
            return Result.success()
        } catch (_: Exception) {
            if (!isStopped && StickyWidgetStore.complete(context, generation, offline = true)) StickyWidgetProvider.render(context)
            return if (!isStopped && runAttemptCount < 3) Result.retry() else Result.failure()
        } finally { connection?.disconnect() }
    }
}
