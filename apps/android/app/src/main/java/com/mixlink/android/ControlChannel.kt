package com.mixlink.android

import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

/**
 * The OkHttp WebSocket control channel, its connection state machine, sending and closing.
 *
 * The channel owns the executor, the OkHttp client and the WebSocket listener. It reports opened,
 * acknowledged, config, error and closed events through [Listener], so the Activity only renders
 * and wires. [running] is the shared session flag owned by the caller.
 */
internal class ControlChannel(
    private val listener: Listener,
    private val running: AtomicBoolean,
    private val currentSnapshot: () -> MixSnapshot,
    private val moreOfMeEnabled: () -> Boolean,
    private val musicianChannel: () -> Int?,
) {
    interface Listener {
        fun onOpened()
        fun onConfig(channels: Int)
        fun onAcknowledged()
        fun onClosed()
        fun onControlError(message: String)
    }

    private val controlExecutor = Executors.newSingleThreadExecutor()
    private val httpClient = OkHttpClient()

    @Volatile private var controlWebSocket: WebSocket? = null
    @Volatile var controlState = ControlState.IDLE
    @Volatile var mixAcknowledged = false
        private set

    private val controlListener = object : WebSocketListener() {
        override fun onOpen(webSocket: WebSocket, response: Response) {
            controlWebSocket = webSocket
            controlState = ControlState.CONNECTED
            listener.onOpened()
            sendCurrentMix()
        }

        override fun onMessage(webSocket: WebSocket, text: String) {
            try {
                val message = JSONObject(text)
                when (message.optString("type")) {
                    "error" -> listener.onControlError(message.optString("message", "unknown control error"))
                    "config" -> {
                        val announcedChannels = message.optInt("source_channels", -1)
                        if (announcedChannels in 1..MAX_CHANNELS) {
                            listener.onConfig(announcedChannels)
                        }
                    }
                    "mix_ack" -> {
                        mixAcknowledged = true
                        listener.onAcknowledged()
                    }
                }
            } catch (error: Exception) {
                listener.onControlError("Invalid control response: ${error.message ?: "unknown error"}")
            }
        }

        override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
            controlWebSocket = null
            controlState = if (running.get()) ControlState.UNAVAILABLE else ControlState.IDLE
            listener.onClosed()
            if (running.get()) listener.onControlError("WebSocket error: ${t.message ?: "connection failed"}")
        }

        override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
            controlWebSocket = null
            controlState = if (running.get()) ControlState.CLOSED else ControlState.IDLE
            listener.onClosed()
        }
    }

    fun connect(host: String, port: Int) {
        controlExecutor.execute {
            try {
                val request = Request.Builder()
                    .url("ws://$host:$port")
                    .build()
                httpClient.newWebSocket(request, controlListener)
            } catch (error: Exception) {
                if (running.get()) listener.onControlError("Could not start WebSocket: ${error.message ?: "unknown error"}")
            }
        }
    }

    fun sendCurrentMix() {
        sendMixControl()
    }

    fun sendMixControl() {
        controlExecutor.execute {
            val webSocket = controlWebSocket ?: return@execute
            val stored = currentSnapshot()
            // `More of me` is derived here, at send time, and never written back. The stored values
            // stay exactly as the musician left them, so turning the control off re-sends them.
            val outgoing = if (moreOfMeEnabled()) stored.moreOfMe(musicianChannel()) ?: stored else stored
            if (!webSocket.send(buildMixMessage(outgoing)) && running.get()) {
                listener.onControlError("WebSocket rejected mix update")
            }
        }
    }

    fun close(code: Int, reason: String) {
        controlWebSocket?.close(code, reason)
        controlWebSocket = null
    }

    fun shutdown() {
        controlExecutor.shutdownNow()
        httpClient.dispatcher.executorService.shutdown()
    }

    private fun buildMixMessage(snapshot: MixSnapshot): String {
        val channels = JSONArray()
        for (gain in snapshot.channelGains) {
            channels.put(gain)
        }
        val panLevels = JSONArray()
        for (pan in snapshot.channelPans) {
            panLevels.put(pan)
        }
        val muteFlags = JSONArray()
        for (mute in snapshot.channelMutes) {
            muteFlags.put(mute)
        }
        val soloFlags = JSONArray()
        for (solo in snapshot.channelSolos) {
            soloFlags.put(solo)
        }
        return JSONObject()
            .put("type", "mix")
            .put("volume_percent", snapshot.volumePercent)
            .put("max_level_percent", snapshot.maxLevelPercent)
            .put("muted", snapshot.muted)
            .put("channels", channels)
            .put("pans", panLevels)
            .put("mutes", muteFlags)
            .put("solos", soloFlags)
            .toString()
    }
}
