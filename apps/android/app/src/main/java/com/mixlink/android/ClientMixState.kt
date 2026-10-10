package com.mixlink.android

/** A named group of source channels announced by the server in `config`. Channels are 0-based. */
data class GroupInfo(val name: String, val channels: List<Int>)

/**
 * The device-local mix state shared by the receive loop, the control channel and the UI.
 *
 * The values keep the original `@Volatile` copy-on-write pattern: every update writes a fresh
 * array or Boolean so a reader on another thread always sees a complete value, never a half-written
 * one. This type deliberately adds no synchronisation beyond that.
 */
internal class ClientMixState {
    @Volatile var sourceChannels = 0
    @Volatile var channelGains: IntArray = IntArray(0)
    @Volatile var channelPans: IntArray = IntArray(0)
    @Volatile var channelMutes: BooleanArray = BooleanArray(0)
    @Volatile var channelSolos: BooleanArray = BooleanArray(0)
    @Volatile var groups: List<GroupInfo> = emptyList()
    @Volatile var groupLevels: IntArray = IntArray(0)
    @Volatile var volumePercent = 100
    @Volatile var maxLevelPercent = 100
    @Volatile var muted = false
}

/** The PMON protocol's upper bound on source channels, used by the receiver and the control channel. */
internal const val MAX_CHANNELS = 32
