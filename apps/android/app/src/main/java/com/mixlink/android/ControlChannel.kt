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
        fun onConfig(channels: Int, groups: List<GroupInfo>)
        fun onAcknowledged()
        fun onClosed()
        fun onControlError(message: String)
    }

    private val controlExecutor = Executors.newSingleThreadExecutor()
    private val httpClient = OkHttpClient()

    @Volatile private var controlWebSocket: WebSocket? = null
    @Volatile private var registrationMessage: String? = null
    @Volatile var controlState = ControlState.IDLE
    @Volatile var mixAcknowledged = false
        private set

    private val controlListener = object : WebSocketListener() {
        override fun onOpen(webSocket: WebSocket, response: Response) {
            controlWebSocket = webSocket
            controlState = ControlState.CONNECTED
            listener.onOpened()
            // Registration goes first: the server adds this client as a target from it, so the mix
            // that follows finds a target to join.
            sendRegistration()
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
                            listener.onConfig(announcedChannels, parseGroups(message.optJSONArray("groups")))
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

    /**
     * Opens the control channel and remembers what to announce when it opens: the musician's name
     * and the UDP port this client listens on. A server that does not understand the registration
     * answers with an error or ignores it, and the rest of the session behaves as before.
     */
    fun connect(host: String, port: Int, udpPort: Int, musicianName: String) {
        registrationMessage = buildRegisterMessage(musicianName, udpPort)
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

    /**
     * Sends the registration once, on open, ahead of the first mix. Queued on the same executor as
     * the mix sends, so the server always sees the registration first.
     */
    private fun sendRegistration() {
        val message = registrationMessage ?: return
        controlExecutor.execute {
            val webSocket = controlWebSocket ?: return@execute
            if (!webSocket.send(message) && running.get()) {
                listener.onControlError("WebSocket rejected the registration")
            }
        }
    }

    fun sendCurrentMix() {
        sendMixControl()
    }

    /**
     * Clears ownership so a fresh session starts with the client owning its mix. The client re-sends
     * its mix on connect and regains ownership on the next acknowledgement; until then the local
     * master volume stays live even if the control channel never comes up.
     */
    fun clearMixOwnership() {
        mixAcknowledged = false
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
        val groupLevels = JSONArray()
        for (level in snapshot.groupLevels) {
            groupLevels.put(level)
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
            .put("group_levels", groupLevels)
            .toString()
    }
}

/**
 * Builds the registration message a client sends when the control channel opens: its name and the
 * UDP port it listens on. The server takes the address from the socket, so none is carried here.
 */
internal fun buildRegisterMessage(name: String, udpPort: Int): String =
    JSONObject()
        .put("type", "register")
        .put("name", name.trim())
        .put("udp_port", udpPort)
        .toString()

/**
 * Reads the `groups` array from a `config` message. Missing or malformed entries are skipped, and a
 * server that announces no groups yields an empty list, which leaves the client's group section
 * empty rather than inventing one.
 */
private fun parseGroups(array: JSONArray?): List<GroupInfo> {
    if (array == null) return emptyList()
    val groups = ArrayList<GroupInfo>(array.length())
    for (index in 0 until array.length()) {
        val entry = array.optJSONObject(index) ?: continue
        val name = entry.optString("name")
        if (name.isEmpty()) continue
        val channelsJson = entry.optJSONArray("channels")
        val channels = if (channelsJson == null) {
            emptyList()
        } else {
            (0 until channelsJson.length()).map { channelsJson.optInt(it, -1) }.filter { it >= 0 }
        }
        groups.add(GroupInfo(name, channels))
    }
    return groups
}
