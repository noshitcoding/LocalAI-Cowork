package dev.opencowork.mobile_push

import android.Manifest
import android.app.Activity
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.app.ActivityCompat
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import com.google.firebase.FirebaseApp
import com.google.firebase.FirebaseOptions
import com.google.firebase.messaging.FirebaseMessaging
import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage
import org.json.JSONArray
import org.json.JSONObject

private const val PREFERENCES = "open_cowork_mobile_push_v1"
private const val CONFIG_PROJECT_ID = "project_id"
private const val CONFIG_APPLICATION_ID = "application_id"
private const val CONFIG_API_KEY = "api_key"
private const val CONFIG_SENDER_ID = "sender_id"
private const val CURRENT_TOKEN = "current_token"
private const val PENDING_EVENTS = "pending_events"
private const val CHANNEL_ID = "open_cowork_runs"
private const val NOTIFICATION_PERMISSION_REQUEST = 44219
private const val MAX_PENDING_EVENTS = 100

@InvokeArg
class FirebaseConfig {
    lateinit var projectId: String
    lateinit var applicationId: String
    lateinit var apiKey: String
    lateinit var senderId: String
}

@TauriPlugin
class MobilePushPlugin(private val activity: Activity) : Plugin(activity) {
    @Command
    fun token(invoke: Invoke) {
        try {
            val config = invoke.parseArgs(FirebaseConfig::class.java)
            validateConfig(config)
            persistConfig(activity, config)
            ensureFirebase(activity)
            FirebaseMessaging.getInstance().isAutoInitEnabled = true
            FirebaseMessaging.getInstance().token.addOnCompleteListener { task ->
                if (!task.isSuccessful || task.result.isNullOrBlank()) {
                    invoke.reject("FCM token retrieval failed: ${task.exception?.message ?: "unknown error"}")
                    return@addOnCompleteListener
                }
                preferences(activity).edit().putString(CURRENT_TOKEN, task.result).commit()
                invoke.resolve(JSObject().apply { put("token", task.result) })
            }
        } catch (error: Exception) {
            invoke.reject("FCM configuration failed: ${error.message}")
        }
    }

    @Command
    fun permissionStatus(invoke: Invoke) {
        invoke.resolve(permissionResponse(activity, requested = false))
    }

    @Command
    fun requestPermission(invoke: Invoke) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            ContextCompat.checkSelfPermission(activity, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            ActivityCompat.requestPermissions(
                activity,
                arrayOf(Manifest.permission.POST_NOTIFICATIONS),
                NOTIFICATION_PERMISSION_REQUEST
            )
            invoke.resolve(permissionResponse(activity, requested = true))
            return
        }
        invoke.resolve(permissionResponse(activity, requested = false))
    }

    @Command
    fun consumeEvents(invoke: Invoke) {
        try {
            val preferences = preferences(activity)
            val encoded = preferences.getString(PENDING_EVENTS, "[]") ?: "[]"
            val parsed = JSONArray(encoded)
            val events = JSONArray()
            for (index in 0 until parsed.length()) events.put(parsed.getJSONObject(index))
            preferences.edit().putString(PENDING_EVENTS, "[]").commit()
            invoke.resolve(JSObject().apply { put("events", events) })
        } catch (error: Exception) {
            invoke.reject("push event read failed: ${error.message}")
        }
    }

    private fun validateConfig(config: FirebaseConfig) {
        require(config.projectId.matches(Regex("[A-Za-z0-9._:-]{1,200}"))) { "invalid Firebase project ID" }
        require(config.applicationId.length in 10..300) { "invalid Firebase application ID" }
        require(config.apiKey.length in 20..300) { "invalid Firebase API key" }
        require(config.senderId.matches(Regex("[0-9]{5,30}"))) { "invalid Firebase sender ID" }
    }
}

class OpenCoworkMessagingService : FirebaseMessagingService() {
    override fun onNewToken(token: String) {
        preferences(this).edit().putString(CURRENT_TOKEN, token).commit()
    }

    override fun onMessageReceived(message: RemoteMessage) {
        val runId = message.data["run_id"] ?: return
        val eventKind = message.data["event_kind"] ?: return
        val sequence = message.data["sequence"]?.toLongOrNull() ?: return
        if (!runId.matches(Regex("[0-9a-fA-F-]{36}")) || eventKind.length !in 1..100) return
        appendEvent(this, runId, eventKind, sequence)
        showGenericNotification(this, runId.hashCode())
    }
}

private fun persistConfig(context: Context, config: FirebaseConfig) {
    preferences(context).edit()
        .putString(CONFIG_PROJECT_ID, config.projectId)
        .putString(CONFIG_APPLICATION_ID, config.applicationId)
        .putString(CONFIG_API_KEY, config.apiKey)
        .putString(CONFIG_SENDER_ID, config.senderId)
        .commit()
}

private fun ensureFirebase(context: Context): FirebaseApp {
    FirebaseApp.getApps(context).firstOrNull()?.let { return it }
    val preferences = preferences(context)
    val projectId = preferences.getString(CONFIG_PROJECT_ID, null)
        ?: throw IllegalStateException("Firebase is not configured")
    val applicationId = preferences.getString(CONFIG_APPLICATION_ID, null)
        ?: throw IllegalStateException("Firebase is not configured")
    val apiKey = preferences.getString(CONFIG_API_KEY, null)
        ?: throw IllegalStateException("Firebase is not configured")
    val senderId = preferences.getString(CONFIG_SENDER_ID, null)
        ?: throw IllegalStateException("Firebase is not configured")
    val options = FirebaseOptions.Builder()
        .setProjectId(projectId)
        .setApplicationId(applicationId)
        .setApiKey(apiKey)
        .setGcmSenderId(senderId)
        .build()
    return FirebaseApp.initializeApp(context.applicationContext, options)
        ?: throw IllegalStateException("Firebase initialization failed")
}

private fun appendEvent(context: Context, runId: String, eventKind: String, sequence: Long) {
    val preferences = preferences(context)
    val events = try {
        JSONArray(preferences.getString(PENDING_EVENTS, "[]"))
    } catch (_: Exception) {
        JSONArray()
    }
    while (events.length() >= MAX_PENDING_EVENTS) events.remove(0)
    events.put(JSONObject().apply {
        put("runId", runId)
        put("eventKind", eventKind)
        put("sequence", sequence)
        put("receivedAt", System.currentTimeMillis())
    })
    preferences.edit().putString(PENDING_EVENTS, events.toString()).commit()
}

private fun showGenericNotification(context: Context, notificationId: Int) {
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
        ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
    ) return
    val manager = context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
        manager.createNotificationChannel(NotificationChannel(
            CHANNEL_ID,
            "Open Cowork runs",
            NotificationManager.IMPORTANCE_DEFAULT
        ))
    }
    val launchIntent = context.packageManager.getLaunchIntentForPackage(context.packageName)
    val pendingIntent = launchIntent?.let {
        PendingIntent.getActivity(
            context,
            0,
            it,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        )
    }
    val notification = NotificationCompat.Builder(context, CHANNEL_ID)
        .setSmallIcon(android.R.drawable.stat_notify_sync)
        .setContentTitle("Open Cowork")
        .setContentText("A run needs your attention.")
        .setAutoCancel(true)
        .setContentIntent(pendingIntent)
        .build()
    manager.notify(notificationId, notification)
}

private fun permissionResponse(context: Context, requested: Boolean) = JSObject().apply {
    val granted = Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU ||
        ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED
    put("granted", granted)
    put("requested", requested)
}

private fun preferences(context: Context) =
    context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)
