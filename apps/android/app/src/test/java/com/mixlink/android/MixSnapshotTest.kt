package com.mixlink.android

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class MixSnapshotTest {
    private fun sample() = MixSnapshot(
        channelGains = intArrayOf(80, 100, 40),
        channelPans = intArrayOf(0, 50, 100),
        channelMutes = booleanArrayOf(false, true, false),
        channelSolos = booleanArrayOf(true, false, false),
        volumePercent = 70,
        maxLevelPercent = 90,
        muted = true,
    )

    @Test
    fun roundTripsThroughJsonWithEveryFieldPreserved() {
        val original = sample()

        val restored = MixSnapshot.fromJson(original.toJson())

        assertArrayEquals(original.channelGains, restored.channelGains)
        assertArrayEquals(original.channelPans, restored.channelPans)
        assertArrayEquals(original.channelMutes, restored.channelMutes)
        assertArrayEquals(original.channelSolos, restored.channelSolos)
        assertEquals(70, restored.volumePercent)
        assertEquals(90, restored.maxLevelPercent)
        assertTrue(restored.muted)
    }

    @Test
    fun moreOfMePushesTheMusicianChannelToFullAndHalvesTheRest() {
        val derived = sample().moreOfMe(1)!!

        assertEquals(40, derived.channelGains[0])
        assertEquals(100, derived.channelGains[1])
        assertEquals(20, derived.channelGains[2])
    }

    @Test
    fun moreOfMeLeavesEveryStoredValueUntouched() {
        val original = sample()

        original.moreOfMe(1)

        assertArrayEquals(intArrayOf(80, 100, 40), original.channelGains)
        assertArrayEquals(intArrayOf(0, 50, 100), original.channelPans)
        assertArrayEquals(booleanArrayOf(false, true, false), original.channelMutes)
        assertArrayEquals(booleanArrayOf(true, false, false), original.channelSolos)
        assertEquals(70, original.volumePercent)
        assertEquals(90, original.maxLevelPercent)
        assertTrue(original.muted)
    }

    @Test
    fun moreOfMeKeepsTheMasterControlsAndFlags() {
        val original = sample()

        val derived = original.moreOfMe(0)!!

        assertArrayEquals(original.channelPans, derived.channelPans)
        assertArrayEquals(original.channelMutes, derived.channelMutes)
        assertArrayEquals(original.channelSolos, derived.channelSolos)
        assertEquals(70, derived.volumePercent)
        assertEquals(90, derived.maxLevelPercent)
        assertTrue(derived.muted)
    }

    @Test
    fun moreOfMeDoesNothingWithoutADesignatedChannel() {
        assertNull(sample().moreOfMe(null))
    }

    @Test
    fun moreOfMeDoesNothingForAChannelOutsideTheMix() {
        assertNull(sample().moreOfMe(9))
    }
}

class MixBankStoreJsonTest {
    private fun snapshot(gain: Int) = MixSnapshot(channelGains = intArrayOf(gain), volumePercent = 100)

    @Test
    fun roundTripsNamedBanksThroughJson() {
        val banks = listOf(MixBank("Rehearsal", snapshot(60)), MixBank("Show \"A\"", snapshot(90)))

        val restored = parseBanks(serializeBanks(banks))

        assertEquals(2, restored.size)
        assertEquals("Rehearsal", restored[0].name)
        assertEquals(60, restored[0].snapshot.channelGains[0])
        assertEquals("Show \"A\"", restored[1].name)
        assertEquals(90, restored[1].snapshot.channelGains[0])
    }

    @Test
    fun fallsBackToAnEmptyListForMalformedJson() {
        assertEquals(emptyList<MixBank>(), parseBanks("{not json"))
    }

    @Test
    fun fallsBackToAnEmptyListForMissingJson() {
        assertEquals(emptyList<MixBank>(), parseBanks(null))
    }

    @Test
    fun skipsBanksWhoseSnapshotIsMalformed() {
        assertEquals(emptyList<MixBank>(), parseBanks("[{\"name\":\"Broken\",\"snapshot\":\"nope\"}]"))
    }
}
