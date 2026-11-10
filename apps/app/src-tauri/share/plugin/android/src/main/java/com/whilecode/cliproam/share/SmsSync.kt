package com.whilecode.cliproam.share

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.provider.Telephony
import android.util.AtomicFile
import android.util.Log
import androidx.work.Constraints
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.Worker
import androidx.work.WorkerParameters
import org.json.JSONObject
import java.io.File
import java.net.HttpURLConnection
import java.net.URI
import java.net.URL
import java.security.MessageDigest
import java.text.SimpleDateFormat
import java.util.Locale
import java.util.TimeZone
import java.util.UUID
import java.util.concurrent.TimeUnit

/** Native-only session and outbox survive WebView/process shutdown; excluded from backup. */
internal object SmsSync {
    private const val WORK = "cliproam-sms-upload"
    private const val MAX_AGE = 10 * 60 * 1000L
    private fun stateFile(context: Context) = File(context.noBackupFilesDir, "sms-session.json")
    private fun outbox(context: Context) = File(context.noBackupFilesDir, "sms-outbox")

    private fun read(file: File): JSONObject? = try {
        JSONObject(AtomicFile(file).openRead().bufferedReader().use { it.readText() })
    } catch (_: java.io.FileNotFoundException) { null }

    private fun write(file: File, value: JSONObject) {
        val atomic = AtomicFile(file)
        val output = atomic.startWrite()
        try {
            output.write(value.toString().toByteArray(Charsets.UTF_8))
            atomic.finishWrite(output)
        } catch (error: Exception) {
            atomic.failWrite(output)
            throw error
        }
    }

    private fun endpoint(address: String, path: String): String {
        val uri = URI(address)
        require(uri.scheme in listOf("http", "https") && !uri.host.isNullOrBlank()
            && uri.userInfo == null && uri.query == null && uri.fragment == null) { "服务器地址无效" }
        return address.trimEnd('/') + path
    }

    private fun request(url: String, token: String, method: String, body: JSONObject? = null): JSONObject {
        val connection = URL(url).openConnection() as HttpURLConnection
        try {
            connection.instanceFollowRedirects = false
            connection.connectTimeout = 2000
            connection.readTimeout = 3000
            connection.requestMethod = method
            connection.setRequestProperty("Authorization", "Bearer $token")
            connection.setRequestProperty("Accept", "application/json")
            if (body != null) {
                val bytes = body.toString().toByteArray(Charsets.UTF_8)
                connection.doOutput = true
                connection.setRequestProperty("Content-Type", "application/json")
                connection.setFixedLengthStreamingMode(bytes.size)
                connection.outputStream.use { it.write(bytes) }
            }
            val status = connection.responseCode
            if (status !in 200..299) throw UploadFailure(status)
            return if (status == 204) JSONObject() else
                JSONObject(connection.inputStream.bufferedReader().use { it.readText() })
        } finally { connection.disconnect() }
    }

    private class UploadFailure(val status: Int) : java.io.IOException("短信同步请求失败 ($status)")

    private fun expiresAt(value: String): Long = SimpleDateFormat("yyyy-MM-dd'T'HH:mm:ss.SSS'Z'", Locale.US)
        .apply { timeZone = TimeZone.getTimeZone("UTC") }.parse(value)?.time ?: 0L

    @Synchronized
    fun configure(context: Context, address: String?, loginToken: String?) {
        require((address == null && loginToken == null) || (!address.isNullOrBlank() && !loginToken.isNullOrBlank()))
        val uploadUrl = address?.let { endpoint(it, "/shortcuts/text") }
        val fingerprint = loginToken?.let {
            MessageDigest.getInstance("SHA-256").digest(it.toByteArray()).joinToString("") { byte -> "%02x".format(byte) }
        }
        val previous = read(stateFile(context))
        if (uploadUrl != null && previous?.optString("uploadUrl") == uploadUrl
            && previous.optString("fingerprint") == fingerprint
            && previous.optLong("expiresAt") > System.currentTimeMillis()) {
            schedule(context)
            return
        }
        // Clear before network work: failed login/account switching must never retain another account.
        AtomicFile(stateFile(context)).delete()
        outbox(context).listFiles()?.forEach { it.delete() }
        WorkManager.getInstance(context).cancelUniqueWork(WORK)
        if (previous != null) try {
            request(previous.getString("tokenUrl"), previous.getString("token"), "DELETE")
        } catch (_: Exception) { /* Local credentials are already gone. */ }
        if (address == null || loginToken == null) return
        val tokenUrl = endpoint(address, "/shortcuts/token")
        val credential = request(tokenUrl, loginToken, "POST")
        val token = credential.getString("token")
        require(token.isNotBlank()) { "上传凭证为空" }
        write(stateFile(context), JSONObject().put("uploadUrl", uploadUrl).put("tokenUrl", tokenUrl)
            .put("token", token).put("fingerprint", fingerprint)
            .put("expiresAt", expiresAt(credential.getString("expiresAt")))
            .put("generation", UUID.randomUUID().toString()))
    }

    @Synchronized
    fun enqueue(context: Context, texts: List<String>): String? {
        val session = read(stateFile(context)) ?: return null
        if (session.optLong("expiresAt") <= System.currentTimeMillis()) return null
        val directory = outbox(context)
        check(directory.isDirectory || directory.mkdirs()) { "无法保存待同步短信" }
        var latest: String? = null
        for (text in texts.filter { it.isNotBlank() }) {
            val name = "${UUID.randomUUID()}.json"
            write(File(directory, name), JSONObject()
                .put("text", text).put("createdAt", System.currentTimeMillis())
                .put("generation", session.getString("generation")))
            latest = name
        }
        // Persist a recovery job before attempting immediate delivery.
        schedule(context)
        return latest
    }

    private fun schedule(context: Context) {
        if (outbox(context).listFiles().isNullOrEmpty()) return
        // Append so a receiver racing a worker's completion cannot lose its recovery job.
        WorkManager.getInstance(context).enqueueUniqueWork(WORK, ExistingWorkPolicy.APPEND_OR_REPLACE,
            OneTimeWorkRequestBuilder<SmsUploadWorker>()
                .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
                .setInitialDelay(30, TimeUnit.SECONDS).build())
    }

    /** Upload one at a time, serialized with logout so queued texts cannot change accounts. */
    @Synchronized
    fun uploadNext(context: Context, preferredName: String? = null): Boolean {
        val file = preferredName?.let { File(outbox(context), it).takeIf { file -> file.isFile } }
            ?: outbox(context).listFiles()?.filter { it.extension == "json" }?.maxByOrNull { it.lastModified() }
            ?: return false
        val sms = read(file)
        val session = read(stateFile(context))
        if (sms == null || session == null || sms.optString("generation") != session.optString("generation")
            || System.currentTimeMillis() - sms.optLong("createdAt") > MAX_AGE
            || session.optLong("expiresAt") <= System.currentTimeMillis()) {
            file.delete()
            return true
        }
        try {
            val result = request(session.getString("uploadUrl"), session.getString("token"), "POST",
                JSONObject().put("text", sms.getString("text")))
            require(result.getJSONObject("entry").getString("id").isNotBlank()) { "服务器未确认短信上传" }
            file.delete()
        } catch (error: UploadFailure) {
            // Authentication and invalid payloads cannot be fixed by retrying.
            if (error.status in 400..499 && error.status != 429) {
                file.delete()
                if (error.status == 401 || error.status == 403) AtomicFile(stateFile(context)).delete()
            } else throw error
        }
        return true
    }
}

class SmsReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != Telephony.Sms.Intents.SMS_RECEIVED_ACTION) return
        val texts = Telephony.Sms.Intents.getMessagesFromIntent(intent)
            .groupBy { it.originatingAddress }.values.map { parts -> parts.joinToString("") { it.messageBody.orEmpty() } }
        val pending = goAsync()
        val app = context.applicationContext
        Thread {
            try {
                // Only one immediate HTTP request within the receiver's short execution window.
                SmsSync.enqueue(app, texts)?.let { SmsSync.uploadNext(app, it) }
            } catch (_: Exception) {
                Log.w("ClipRoamSms", "短信即时同步失败，等待后台重试")
            } finally { pending.finish() }
        }.start()
    }
}

class SmsUploadWorker(context: Context, params: WorkerParameters) : Worker(context, params) {
    override fun doWork(): Result = try {
        while (!isStopped && SmsSync.uploadNext(applicationContext)) { /* Drain native outbox. */ }
        Result.success()
    } catch (_: Exception) { Result.retry() }
}
