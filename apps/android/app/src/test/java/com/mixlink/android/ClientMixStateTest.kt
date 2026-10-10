package com.mixlink.android

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Pure JVM coverage for how a client handles the `config` the server re-sends when a device switch
 * changes the source channel count mid-session. The view wiring has no meaningful JVM test; this
 * covers the state transition that decides whether the controls are rebuilt.
 */
class ClientMixStateTest {
    @Test
    fun theFirstConfigBuildsNeutralPerChannelState() {
        val state = ClientMixState()

        assertTrue(state.applySourceChannels(2))

        assertEquals(2, state.sourceChannels)
        assertArrayEquals(intArrayOf(100, 100), state.channelGains)
        assertArrayEquals(intArrayOf(0, 100), state.channelPans)
        assertArrayEquals(booleanArrayOf(false, false), state.channelMutes)
        assertArrayEquals(booleanArrayOf(false, false), state.channelSolos)
    }

    @Test
    fun aRepeatedConfigWithTheSameCountDoesNotRebuildTheControls() {
        val state = ClientMixState()
        state.applySourceChannels(2)
        state.channelGains = intArrayOf(40, 70)

        assertFalse(state.applySourceChannels(2))

        assertArrayEquals(intArrayOf(40, 70), state.channelGains)
    }

    @Test
    fun aReSentConfigWithMoreChannelsKeepsSurvivingValuesAndDefaultsTheNewOnes() {
        val state = ClientMixState()
        state.applySourceChannels(2)
        state.channelGains = intArrayOf(40, 70)
        state.channelPans = intArrayOf(100, 0)
        state.channelMutes = booleanArrayOf(true, false)
        state.channelSolos = booleanArrayOf(false, true)

        assertTrue(state.applySourceChannels(4))

        assertEquals(4, state.sourceChannels)
        assertArrayEquals(intArrayOf(40, 70, 100, 100), state.channelGains)
        assertArrayEquals(intArrayOf(100, 0, 0, 100), state.channelPans)
        assertArrayEquals(booleanArrayOf(true, false, false, false), state.channelMutes)
        assertArrayEquals(booleanArrayOf(false, true, false, false), state.channelSolos)
    }

    @Test
    fun aReSentConfigWithFewerChannelsDropsTheChannelsThatNoLongerExist() {
        val state = ClientMixState()
        state.applySourceChannels(4)
        state.channelGains = intArrayOf(10, 20, 30, 40)
        state.channelMutes = booleanArrayOf(false, false, true, true)

        assertTrue(state.applySourceChannels(2))

        assertEquals(2, state.sourceChannels)
        assertArrayEquals(intArrayOf(10, 20), state.channelGains)
        assertArrayEquals(booleanArrayOf(false, false), state.channelMutes)
    }
}
