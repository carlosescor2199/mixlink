package com.mixlink.android

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Pure JVM coverage for the group levels carried inside a [MixSnapshot]. The view wiring itself has
 * no meaningful JVM test; this covers only the serialisation and the `More of me` interaction.
 */
class GroupSnapshotTest {
    @Test
    fun roundTripsGroupLevelsThroughJson() {
        val original = MixSnapshot(
            channelGains = intArrayOf(100, 100),
            groupLevels = intArrayOf(80, 40),
        )

        val restored = MixSnapshot.fromJson(original.toJson())

        assertArrayEquals(intArrayOf(80, 40), restored.groupLevels)
    }

    @Test
    fun aSnapshotWithoutGroupLevelsSerializesAnEmptyList() {
        val snapshot = MixSnapshot(channelGains = intArrayOf(100))

        val restored = MixSnapshot.fromJson(snapshot.toJson())

        assertEquals(0, restored.groupLevels.size)
    }

    @Test
    fun anOlderBankWithNoGroupLevelsReadsAsEmpty() {
        val legacy = """{"channelGains":[100],"volumePercent":70}"""

        val restored = MixSnapshot.fromJson(legacy)

        assertArrayEquals(intArrayOf(100), restored.channelGains)
        assertEquals(0, restored.groupLevels.size)
    }

    @Test
    fun moreOfMeKeepsTheGroupLevelsUntouched() {
        val original = MixSnapshot(
            channelGains = intArrayOf(80, 100),
            groupLevels = intArrayOf(30, 60),
        )

        val derived = original.moreOfMe(1)!!

        assertArrayEquals(intArrayOf(30, 60), derived.groupLevels)
        assertArrayEquals(intArrayOf(30, 60), original.groupLevels)
    }

    @Test
    fun theDefaultSnapshotStartsWithNoGroups() {
        assertTrue(MixSnapshot().groupLevels.isEmpty())
    }
}
