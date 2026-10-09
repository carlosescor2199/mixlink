package com.mixlink.android

import kotlin.math.roundToInt

object Pcm16Processor {
    /**
     * Applies the master controls to an interleaved PCM buffer.
     *
     * The buffer the client receives is the server's finished stereo mix, so there is nothing per
     * source left to scale here. Only master volume, mute and the full-scale ceiling are applied.
     * A buffer whose master controls are neutral is passed through unchanged.
     */
    fun apply(
        samples: ShortArray,
        volumePercent: Int,
        maxLevelPercent: Int,
        muted: Boolean,
    ): ShortArray {
        val volume = volumePercent.coerceIn(0, 100) / 100.0
        val maximum = Short.MAX_VALUE * maxLevelPercent.coerceIn(0, 100) / 100
        val output = ShortArray(samples.size)

        for (index in samples.indices) {
            val scaled = if (muted) 0.0 else samples[index] * volume
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
     * When the server has acknowledged a mix for this client it owns that mix: it has applied the
     * per-source gains, pans, volume, ceiling and mute for this client IP and keeps applying them
     * for the rest of the session, including while the control socket is down. Local processing is
     * skipped in that case so the same gain is never applied twice. Local processing still applies
     * when no mix was ever acknowledged, because the server is holding its neutral default then,
     * and when the control channel never came up.
     *
     * The local path can only apply master controls. The audio it holds is the server's finished
     * stereo mix, so the per-source faders and pans do nothing without a working control channel.
     */
    fun applyLocalProtection(
        samples: ShortArray,
        volumePercent: Int,
        maxLevelPercent: Int,
        muted: Boolean,
        serverOwnsMix: Boolean,
    ): ShortArray = if (serverOwnsMix) {
        samples
    } else {
        apply(samples, volumePercent, maxLevelPercent, muted)
    }
}
