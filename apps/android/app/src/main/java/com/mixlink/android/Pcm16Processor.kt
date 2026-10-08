package com.mixlink.android

import kotlin.math.roundToInt

object Pcm16Processor {
    fun apply(samples: ShortArray, volumePercent: Int, maxLevelPercent: Int, muted: Boolean): ShortArray {
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
}