package com.mixlink.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Pure JVM coverage for the discovery beacon parsing and the stale-entry bookkeeping. The UDP
 * socket itself has no meaningful JVM test; this covers the two-facts parse and the expiry rule.
 */
class ServerDiscoveryTest {
    @Test
    fun parsesTheControlPortFromAValidBeacon() {
        assertEquals(50001, ServerBeacon.parseControlPort(beacon(controlPort = 50001)))
    }

    @Test
    fun aShortDatagramIsIgnored() {
        assertNull(ServerBeacon.parseControlPort(byteArrayOf(0x4d, 0x4c, 0x4e, 0x4b, 1, 0x51)))
    }

    @Test
    fun aWrongMagicIsIgnored() {
        val datagram = beacon(controlPort = 50001)
        datagram[0] = 'X'.code.toByte()

        assertNull(ServerBeacon.parseControlPort(datagram))
    }

    @Test
    fun aWrongVersionIsIgnored() {
        val datagram = beacon(controlPort = 50001)
        datagram[4] = 2

        assertNull(ServerBeacon.parseControlPort(datagram))
    }

    @Test
    fun aDiscoveredServerReportsItsAddressAndControlPort() {
        val servers = DiscoveredServers(staleTimeoutMs = 5_000)

        servers.observe("192.168.1.27", beacon(controlPort = 50001), nowMs = 1_000)
        val found = servers.list(nowMs = 1_000)

        assertEquals(1, found.size)
        assertEquals("192.168.1.27", found[0].address)
        assertEquals(50001, found[0].controlPort)
    }

    @Test
    fun aMalformedDatagramNeverCreatesAnEntry() {
        val servers = DiscoveredServers(staleTimeoutMs = 5_000)
        val datagram = beacon(controlPort = 50001)
        datagram[4] = 2

        servers.observe("192.168.1.27", datagram, nowMs = 1_000)

        assertTrue(servers.list(nowMs = 1_000).isEmpty())
    }

    @Test
    fun anEntryExpiresOnceItIsStale() {
        val servers = DiscoveredServers(staleTimeoutMs = 5_000)

        servers.observe("192.168.1.27", beacon(controlPort = 50001), nowMs = 1_000)

        assertTrue(servers.list(nowMs = 6_000).isEmpty())
    }

    @Test
    fun aFreshEntrySurvivesWhileItIsWithinTheTimeout() {
        val servers = DiscoveredServers(staleTimeoutMs = 5_000)

        servers.observe("192.168.1.27", beacon(controlPort = 50001), nowMs = 1_000)

        assertEquals(1, servers.list(nowMs = 5_999).size)
    }

    @Test
    fun rediscoveryRefreshesTheLastSeenTimeAndThePort() {
        val servers = DiscoveredServers(staleTimeoutMs = 5_000)

        servers.observe("192.168.1.27", beacon(controlPort = 50001), nowMs = 1_000)
        servers.observe("192.168.1.27", beacon(controlPort = 50002), nowMs = 6_000)

        val found = servers.list(nowMs = 6_000)

        assertEquals(1, found.size)
        assertEquals(50002, found[0].controlPort)
        assertEquals(6_000, found[0].lastSeenMs)
    }

    @Test
    fun severalServersAreListedRatherThanChosen() {
        val servers = DiscoveredServers(staleTimeoutMs = 5_000)

        servers.observe("192.168.1.27", beacon(controlPort = 50001), nowMs = 1_000)
        servers.observe("192.168.1.30", beacon(controlPort = 50001), nowMs = 1_000)

        val found = servers.list(nowMs = 1_000)

        assertEquals(listOf("192.168.1.27", "192.168.1.30"), found.map { it.address })
    }

    private fun beacon(controlPort: Int): ByteArray = byteArrayOf(
        'M'.code.toByte(),
        'L'.code.toByte(),
        'N'.code.toByte(),
        'K'.code.toByte(),
        ServerBeacon.VERSION,
        (controlPort and 0xFF).toByte(),
        ((controlPort shr 8) and 0xFF).toByte(),
        0x80.toByte(),
        0xBB.toByte(),
        0x00.toByte(),
        0x00.toByte(),
    )
}
