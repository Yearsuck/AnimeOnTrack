package com.animeontrack.webfetch

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat

/**
 * Foreground service running while a library or airing scan is active.
 *
 * Keeps Android from freezing the off-screen WebView or throttling the process
 * while the screen is off or the app is not in the foreground.
 */
class ScanService : Service() {
    private var wakeLock: PowerManager.WakeLock? = null

    companion object {
        private const val TAG = "AOT-ScanService"
        private const val CHANNEL_ID = "scan"
        private const val NOTIFICATION_ID = 4201
        private const val WAKE_LOCK_TAG = "AnimeOnTrack:scan"
        private const val WAKE_LOCK_TIMEOUT_MS = 30 * 60 * 1000L // 30 minutes
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        createNotificationChannel()

        val notification = NotificationCompat.Builder(this, CHANNEL_ID)
            .setSmallIcon(applicationInfo.icon)
            .setContentTitle("AnimeOnTrack")
            .setContentText("Buscando episodios nuevos…")
            .setOngoing(true)
            .setSound(null)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .build()

        // Android 12+ throws ForegroundServiceStartNotAllowedException when the app is
        // already in the background; that throw happens here, outside the plugin's
        // try/catch, so catch it to avoid crashing the app and just skip the keep-alive.
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
            } else {
                startForeground(NOTIFICATION_ID, notification)
            }
        } catch (e: Exception) {
            Log.w(TAG, "startForeground failed: ${e.message}")
            stopSelf()
            return START_NOT_STICKY
        }

        if (wakeLock == null) {
            val pm = getSystemService(Context.POWER_SERVICE) as? PowerManager
            wakeLock = pm?.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, WAKE_LOCK_TAG)?.apply {
                setReferenceCounted(false)
            }
        }
        if (wakeLock?.isHeld != true) {
            wakeLock?.acquire(WAKE_LOCK_TIMEOUT_MS)
        }

        return START_NOT_STICKY
    }

    override fun onDestroy() {
        try {
            if (wakeLock?.isHeld == true) {
                wakeLock?.release()
            }
        } catch (e: Exception) {
            Log.w(TAG, "failed to release wake lock: ${e.message}")
        }
        wakeLock = null

        ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channel = NotificationChannel(
                CHANNEL_ID,
                "AnimeOnTrack",
                NotificationManager.IMPORTANCE_LOW
            ).apply {
                description = "Escaneo de episodios"
                setSound(null, null)
            }
            val manager = getSystemService(NotificationManager::class.java)
            manager?.createNotificationChannel(channel)
        }
    }
}
