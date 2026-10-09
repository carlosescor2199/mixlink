package com.mixlink.android

import kotlin.math.roundToInt

object Pcm16Processor {
    /**
     * Applies the local mix to an interleaved PCM buffer.
     *
     * Source channel `k` maps to output slot `k`, mirroring the server, so a buffer whose gains
     * are all 100% is passed through unchanged apart from the full-scale ceiling clamp.
     */
    fun apply(
        samples: ShortArray,
        channelGains: IntArray,
        volumePercent: Int,
        maxLevelPercent: Int,
        muted: Boolean,
    ): ShortArray {
        val volume = volumePercent.coerceIn(0, 100) / 100.0
        val maximum = Short.MAX_VALUE * maxLevelPercent.coerceIn(0, 100) / 100
        val channels = channelGains.size.coerceAtLeast(1)
        val output = ShortArray(samples.size)

        for (index in samples.indices) {
            val channelGain = if (channelGains.isEmpty()) {
                1.0
            } else {
                channelGains[index % channels].coerceIn(0, 100) / 100.0
            }
            val scaled = if (muted) 0.0 else samples[index] * channelGain * volume
            val limited = scaled.coerceIn(-maximum.toDouble(), maximum.toDouble())
            output[index] = limited.roundToInt()
                .coerceIn(Short.MIN_VALUE.toInt(), Short.MAX_VALUE.toInt())
                .toShort()
        }
        return output
    }

    /**
     * Returns the samples to play for one received packet.
     *
     * When the server has acknowledged a mix for this client it owns that mix: it has applied
     * per-channel gains, volume, ceiling and mute for this client IP and keeps applying them for
     * the rest of the session, including while the control socket is down. Local processing is
     * skipped in that case so the same gain is never applied twice. Local processing still
     * applies when no mix was ever acknowledged, because the server is holding its neutral
     * default then, and when the control channel never came up.
     */
    fun applyLocalProtection(
        samples: ShortArray,
        channelGains: IntArray,
        volumePercent: Int,
        maxLevelPercent: Int,
        muted: Boolean,
        serverOwnsMix: Boolean,
    ): ShortArray = if (serverOwnsMix) {
        samples
    } else {
        apply(samples, channelGains, volumePercent, maxLevelPercent, muted)
    }
}
