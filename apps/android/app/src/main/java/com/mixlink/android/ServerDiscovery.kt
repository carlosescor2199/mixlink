package com.mixlink.android

import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.SocketTimeoutException
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

/**
 * The MixLink discovery beacon: its constants and its pure parser.
 *
 * The beacon is a separate, tiny protocol, not PMON and not the control channel. A client only
 * needs two facts from it: the address, which comes from the packet's source, and the control port,
 * which [parseControlPort] reads from version-1 payloads. Anything else is ignored.
 */
internal object ServerBeacon {
    /** Fixed UDP port the server broadcasts to and the client listens on. */
    const val DISCOVERY_PORT = 50002

    const val VERSION: Byte = 1

    /** How long a discovered server stays listed without being heard from again. */
    const val STALE_TIMEOUT_MS = 5_000L

    /** How often an idle listener wakes to drop stale entries even when no beacon arrives. */
    const val SCAN_INTERVAL_MS = 1_000

    // magic[4] | version[1] | control_port[2 LE] | sample_rate[4 LE]
    private const val MIN_DATAGRAM_LENGTH = 7
    private val MAGIC = byteArrayOf(
        'M'.code.toByte(),
        'L'.code.toByte(),
        'N'.code.toByte(),
        'K'.code.toByte(),
    )

    /**
     * Returns the control port carried by a version-1 beacon, or `null` for a short datagram, a
     * wrong magic, a wrong version or an impossible port. Malformed input is ignored, never thrown.
     */
    fun parseControlPort(datagram: ByteArray): Int? {
        if (datagram.size < MIN_DATAGRAM_LENGTH) return null
        for (index in MAGIC.indices) {
            if (datagram[index] != MAGIC[index]) return null
        }
        if (datagram[4] != VERSION) return null
        val port = (datagram[5].toInt() and 0xFF) or ((datagram[6].toInt() and 0xFF) shl 8)
        if (port !in 1..65535) return null
        return port
    }
}

/** A server seen advertising: where it was heard from, its control port and when. */
internal data class DiscoveredServer(
    val address: String,
    val controlPort: Int,
    val lastSeenMs: Long,
)

/**
 * Keeps the servers currently advertising, keyed by source address so a server that moves updates
 * in place instead of appearing twice. Time is injected so the expiry rule is an ordinary JVM test.
 */
internal class DiscoveredServers(private val staleTimeoutMs: Long = ServerBeacon.STALE_TIMEOUT_MS) {
    private val servers = LinkedHashMap<String, DiscoveredServer>()

    /**
     * Records a datagram heard from [sourceAddress]. Returns the updated entry, or `null` when the
     * datagram is malformed so a bad packet never creates or refreshes an entry.
     */
    fun observe(sourceAddress: String, datagram: ByteArray, nowMs: Long): DiscoveredServer? {
        val controlPort = ServerBeacon.parseControlPort(datagram) ?: return null
        return record(sourceAddress, controlPort, nowMs)
    }

    fun record(address: String, controlPort: Int, nowMs: Long): DiscoveredServer {
        val entry = DiscoveredServer(address, controlPort, nowMs)
        servers[address] = entry
        return entry
    }

    /** The servers heard from within the timeout, dropping any that have gone stale. */
    fun list(nowMs: Long): List<DiscoveredServer> {
        servers.entries.removeAll { nowMs - it.value.lastSeenMs >= staleTimeoutMs }
        return servers.values.toList()
    }
}

/**
 * Listens on the discovery port while idle and reports the servers it finds through [Listener].
 *
 * The socket work has no meaningful JVM test; the two-facts parse and the expiry rule live in the
 * pure types above. Listeners are notified off the main thread, so the Activity hops to the UI
 * thread before touching views.
 */
internal class ServerDiscovery(
    private val listener: Listener,
) {
    interface Listener {
        fun onServersChanged(servers: List<DiscoveredServer>)
    }

    private val registry = DiscoveredServers()
    private val executor = Executors.newSingleThreadExecutor()
    private val running = AtomicBoolean(false)

    @Volatile private var socket: DatagramSocket? = null
    private var lastReported: List<DiscoveredServer> = emptyList()

    fun start() {
        if (!running.compareAndSet(false, true)) return
        executor.execute {
            val bound = try {
                DatagramSocket(ServerBeacon.DISCOVERY_PORT)
            } catch (error: Exception) {
                running.set(false)
                return@execute
            }
            bound.soTimeout = ServerBeacon.SCAN_INTERVAL_MS
            socket = bound
            val buffer = ByteArray(64)
            while (running.get()) {
                val packet = DatagramPacket(buffer, buffer.size)
                try {
                    bound.receive(packet)
                } catch (_: SocketTimeoutException) {
                    report(System.currentTimeMillis())
                    continue
                } catch (_: Exception) {
                    break
                }
                val source = packet.address?.hostAddress
                if (source != null) {
                    val datagram = packet.data.copyOfRange(packet.offset, packet.offset + packet.length)
                    registry.observe(source, datagram, System.currentTimeMillis())
                }
                report(System.currentTimeMillis())
            }
        }
    }

    /** Pauses listening; a later [start] resumes it. */
    fun stop() {
        running.set(false)
        socket?.close()
        socket = null
    }

    /** Stops listening for good. */
    fun shutdown() {
        stop()
        executor.shutdownNow()
    }

    private fun report(nowMs: Long) {
        val current = registry.list(nowMs)
        if (current != lastReported) {
            lastReported = current
            listener.onServersChanged(current)
        }
    }
}
