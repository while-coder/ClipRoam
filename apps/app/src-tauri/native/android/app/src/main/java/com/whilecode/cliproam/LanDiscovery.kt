package com.whilecode.cliproam

import android.content.Context
import android.net.wifi.WifiManager
import androidx.annotation.Keep

/** Keep multicast reception enabled only while the LAN service is running. */
@Keep
object LanDiscovery {
    private var lock: WifiManager.MulticastLock? = null

    @JvmStatic
    @Synchronized
    fun setEnabled(context: Context, enabled: Boolean) {
        if (!enabled) {
            lock?.let { if (it.isHeld) it.release() }
            lock = null
        } else if (lock == null) {
            val wifi = context.applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
            lock = wifi.createMulticastLock("cliproam-lan").apply {
                setReferenceCounted(false)
                acquire()
            }
        }
    }
}
