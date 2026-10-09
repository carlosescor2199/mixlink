package com.mixlink.android

import org.junit.Assert.assertArrayEquals
import org.junit.Test

class Pcm16ProcessorTest {
    @Test
    fun appliesVolumeAndHardMaximumWithoutClipping() {
        val output = Pcm16Processor.apply(
            samples = shortArrayOf(Short.MIN_VALUE, -16_000, 16_000, Short.MAX_VALUE),
            volumePercent = 50,
            maxLevelPercent = 25,
            muted = false,
        )

        assertArrayEquals(shortArrayOf(-8_191, -8_000, 8_000, 8_191), output)
    }

    @Test
    fun muteOverridesVolumeAndMaximum() {
        val output = Pcm16Processor.apply(
            samples = shortArrayOf(Short.MIN_VALUE, 0, Short.MAX_VALUE),
            volumePercent = 100,
            maxLevelPercent = 100,
            muted = true,
        )

        assertArrayEquals(shortArrayOf(0, 0, 0), output)
    }

    @Test
    fun skipsLocalProcessingWhenTheServerOwnsTheMix() {
        val samples = shortArrayOf(-16_000, 0, 16_000)

        val output = Pcm16Processor.applyLocalProtection(
            samples = samples,
            volumePercent = 0,
            maxLevelPercent = 25,
            muted = true,
            serverOwnsMix = true,
        )

        assertArrayEquals(samples, output)
    }

    @Test
    fun appliesLocalProcessingWhenTheServerNeverOwnedTheMix() {
        val output = Pcm16Processor.applyLocalProtection(
            samples = shortArrayOf(-16_000, 16_000),
            volumePercent = 50,
            maxLevelPercent = 100,
            muted = false,
            serverOwnsMix = false,
        )

        assertArrayEquals(shortArrayOf(-8_000, 8_000), output)
    }
}