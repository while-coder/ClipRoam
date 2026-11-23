package com.whilecode.cliproam.share

import android.app.PendingIntent
import android.appwidget.AppWidgetManager
import android.appwidget.AppWidgetProvider
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.content.res.Configuration
import android.graphics.BitmapFactory
import android.util.Base64
import android.net.Uri
import android.text.format.DateFormat
import android.view.View
import android.widget.RemoteViews
import android.widget.RemoteViewsService
import org.json.JSONArray
import org.json.JSONObject
import java.util.Date

class StickyWidgetProvider : AppWidgetProvider() {
    override fun onUpdate(context: Context, manager: AppWidgetManager, ids: IntArray) {
        render(context)
        StickyWidgetSync.schedule(context)
        StickyWidgetSync.refresh(context)
    }

    override fun onAppWidgetOptionsChanged(context: Context, manager: AppWidgetManager, id: Int, options: Bundle) { render(context) }
    override fun onDisabled(context: Context) { StickyWidgetSync.stopPeriodic(context) }

    override fun onReceive(context: Context, intent: Intent) {
        super.onReceive(context, intent)
        if (intent.action == REFRESH) StickyWidgetSync.refresh(context)
    }

    companion object {
        private const val REFRESH = "com.whilecode.cliproam.STICKY_REFRESH"
        fun ids(context: Context): IntArray = AppWidgetManager.getInstance(context)
            .getAppWidgetIds(ComponentName(context, StickyWidgetProvider::class.java))

        internal fun openApp(context: Context): PendingIntent? = context.packageManager.getLaunchIntentForPackage(context.packageName)?.let {
            PendingIntent.getActivity(context, 0, it, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        }

        @Suppress("DEPRECATION")
        @Synchronized
        fun render(context: Context) {
            val manager = AppWidgetManager.getInstance(context)
            val state = StickyWidgetStore.load(context)
            val notes = state.optJSONArray("notes") ?: JSONArray()
            val signedIn = state.optString("token").isNotBlank()
            for (id in ids(context)) {
                val options = manager.getAppWidgetOptions(id)
                val landscape = context.resources.configuration.orientation == Configuration.ORIENTATION_LANDSCAPE
                val width = options.getInt(if (landscape) AppWidgetManager.OPTION_APPWIDGET_MAX_WIDTH else AppWidgetManager.OPTION_APPWIDGET_MIN_WIDTH)
                val height = options.getInt(if (landscape) AppWidgetManager.OPTION_APPWIDGET_MIN_HEIGHT else AppWidgetManager.OPTION_APPWIDGET_MAX_HEIGHT)
                val small = width in 1 until 240 && height in 1 until 240
                val hasNotes = signedIn && notes.length() > 0
                val views = RemoteViews(context.packageName, R.layout.sticky_widget)
                views.setTextViewText(R.id.sticky_title, context.getString(R.string.sticky_widget_name))
                val status = when {
                    !signedIn -> context.getString(R.string.sticky_widget_login)
                    state.optBoolean("refreshing") -> context.getString(R.string.sticky_widget_loading)
                    state.optBoolean("offline") -> context.getString(R.string.sticky_widget_offline)
                    state.optLong("updatedAt") == 0L -> context.getString(R.string.sticky_widget_loading)
                    else -> context.getString(R.string.sticky_widget_status, notes.length(),
                        DateFormat.getTimeFormat(context).format(Date(state.getLong("updatedAt"))))
                }
                views.setTextViewText(R.id.sticky_status, status)
                views.setTextViewText(R.id.sticky_empty, context.getString(if (signedIn) R.string.sticky_widget_empty else R.string.sticky_widget_login))
                val adapter = Intent(context, StickyWidgetService::class.java)
                views.setRemoteAdapter(R.id.sticky_list, adapter)
                views.setEmptyView(R.id.sticky_list, R.id.sticky_empty)
                // Hide the launcher's previous account rows before its remote adapter reloads.
                views.setViewVisibility(R.id.sticky_list_container, if (hasNotes && !small) View.VISIBLE else View.GONE)
                views.setViewVisibility(R.id.sticky_empty, if (!signedIn || notes.length() == 0) View.VISIBLE else View.GONE)
                views.setViewVisibility(R.id.sticky_status, if (small && hasNotes && !state.optBoolean("offline")) View.GONE else View.VISIBLE)
                views.setViewVisibility(R.id.sticky_single, if (small && hasNotes) View.VISIBLE else View.GONE)
                views.removeAllViews(R.id.sticky_single)
                if (small && hasNotes) {
                    val note = notes.getJSONObject(0)
                    val single = noteViews(context, note, true)
                    context.packageManager.getLaunchIntentForPackage(context.packageName)?.let { launch ->
                        launch.data = noteUri(note)
                        launch.addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP)
                        single.setOnClickPendingIntent(R.id.sticky_row, PendingIntent.getActivity(context, id, launch,
                            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE))
                    }
                    views.addView(R.id.sticky_single, single)
                }
                views.setOnClickPendingIntent(R.id.sticky_refresh, PendingIntent.getBroadcast(context, 0,
                    Intent(context, StickyWidgetProvider::class.java).setAction(REFRESH),
                    PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE))
                views.setBoolean(R.id.sticky_refresh, "setEnabled", signedIn)
                openApp(context)?.let { launch ->
                    views.setOnClickPendingIntent(R.id.sticky_title, launch)
                    views.setOnClickPendingIntent(R.id.sticky_empty, launch)
                }
                // Collection rows use a mutable template so the launcher can attach their fill-in intent.
                context.packageManager.getLaunchIntentForPackage(context.packageName)?.let { launch ->
                    launch.addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP)
                    views.setPendingIntentTemplate(R.id.sticky_list, PendingIntent.getActivity(context, 1, launch,
                        PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_MUTABLE))
                }
                manager.updateAppWidget(id, views)
                manager.notifyAppWidgetViewDataChanged(id, R.id.sticky_list)
            }
        }
    }
}

class StickyWidgetService : RemoteViewsService() {
    override fun onGetViewFactory(intent: Intent): RemoteViewsFactory = NotesFactory(applicationContext)

    private class NotesFactory(private val context: Context) : RemoteViewsFactory {
        private var notes = JSONArray()
        override fun onCreate() { onDataSetChanged() }
        override fun onDataSetChanged() { notes = StickyWidgetStore.load(context).optJSONArray("notes") ?: JSONArray() }
        override fun onDestroy() { notes = JSONArray() }
        override fun getCount() = notes.length()
        override fun getViewTypeCount() = 1
        override fun hasStableIds() = false
        override fun getItemId(position: Int) = position.toLong()
        override fun getLoadingView(): RemoteViews? = null
        override fun getViewAt(position: Int): RemoteViews? {
            val note = notes.optJSONObject(position) ?: return null
            return noteViews(context, note).apply {
                setOnClickFillInIntent(R.id.sticky_row, Intent().setData(noteUri(note)))
            }
        }
    }
}

private fun noteUri(note: JSONObject): Uri = Uri.Builder()
    .scheme("cliproam").authority("widget").appendQueryParameter("id", note.getString("id")).build()

private fun noteViews(context: Context, note: JSONObject, expanded: Boolean = false): RemoteViews {
    val views = RemoteViews(context.packageName, if (expanded) R.layout.sticky_widget_single else R.layout.sticky_widget_row)
    views.setTextViewText(R.id.sticky_note, note.optString("content"))
    val thumbnail = note.optJSONObject("imageInfo")?.optString("thumbnail").orEmpty()
    val bitmap = if (note.optString("kind") == "image" && thumbnail.isNotBlank()) {
        try {
            val bytes = Base64.decode(thumbnail, Base64.DEFAULT)
            val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
            BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
            val options = BitmapFactory.Options().apply {
                inSampleSize = 1
                while (bounds.outWidth / inSampleSize > 512 || bounds.outHeight / inSampleSize > 512) inSampleSize *= 2
            }
            BitmapFactory.decodeByteArray(bytes, 0, bytes.size, options)
        } catch (_: IllegalArgumentException) { null }
    } else null
    views.setViewVisibility(R.id.sticky_thumbnail, if (bitmap != null) View.VISIBLE else View.GONE)
    views.setViewVisibility(R.id.sticky_note, if (expanded && bitmap != null) View.GONE else View.VISIBLE)
    views.setImageViewBitmap(R.id.sticky_thumbnail, bitmap)
    views.setContentDescription(R.id.sticky_thumbnail, note.optString("content"))
    return views
}
