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

/**
 * Rebuilds the per-channel state for a `config` that announces [channels] source channels.
 *
 * The server re-sends `config` mid-session when a device switch changes the channel count. This
 * resizes the per-channel arrays rather than resetting them, so a channel that still exists keeps
 * the gain, pan, mute and solo the musician set and the controls the client rebuilds stay honest
 * about the mix the server is still applying. New channels start neutral; channels that no longer
 * exist are dropped because they cannot be controlled or heard.
 *
 * Returns `true` when the controls must be rebuilt. An unchanged count is a no-op, so a repeated
 * `config` never restarts the controls.
 */
internal fun ClientMixState.applySourceChannels(channels: Int): Boolean {
    if (sourceChannels == channels && channelGains.size == channels) return false
    sourceChannels = channels
    channelGains = IntArray(channels) { index -> channelGains.getOrElse(index) { 100 } }
    channelPans = IntArray(channels) { index -> channelPans.getOrElse(index) { defaultPan(index) } }
    channelMutes = BooleanArray(channels) { index -> channelMutes.getOrElse(index) { false } }
    channelSolos = BooleanArray(channels) { index -> channelSolos.getOrElse(index) { false } }
    return true
}

/** The PMON protocol's upper bound on source channels, used by the receiver and the control channel. */
internal const val MAX_CHANNELS = 32

/** The default pan keeps the captured layout: even sources hard left, odd sources hard right. */
internal fun defaultPan(index: Int): Int = if (index % 2 == 0) 0 else 100
