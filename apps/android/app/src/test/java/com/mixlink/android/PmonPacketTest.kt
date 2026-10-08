package com.mixlink.android

import java.nio.ByteBuffer
import java.nio.ByteOrder
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Test

class PmonPacketTest {
    @Test
    fun parsesLittleEndianHeaderAndPcmPayload() {
        val packet = PmonPacketParser.parse(packetBytes(sequence = 9, samples = shortArrayOf(-1, 0x1234)))

        assertEquals(2, packet.channels)
        assertEquals(48_000, packet.sampleRate)
        assertEquals(9L, packet.sequence)
        assertArrayEquals(shortArrayOf(-1, 0x1234), packet.samples)
    }

    @Test(expected = PmonPacketParseException::class)
    fun rejectsInvalidMagic() {
        val bytes = packetBytes(sequence = 1, samples = shortArrayOf(1))
        bytes[0] = 'X'.code.toByte()
        PmonPacketParser.parse(bytes)
    }

    @Test(expected = PmonPacketParseException::class)
    fun rejectsPayloadLengthMismatch() {
        val bytes = packetBytes(sequence = 1, samples = shortArrayOf(1, 2))
        PmonPacketParser.parse(bytes, bytes.size - 2)
    }

    private fun packetBytes(sequence: Long, samples: ShortArray): ByteArray {
        val bytes = ByteBuffer.allocate(PmonPacketParser.HEADER_SIZE + samples.size * 2)
            .order(ByteOrder.LITTLE_ENDIAN)
        bytes.put(byteArrayOf('P'.code.toByte(), 'M'.code.toByte(), 'O'.code.toByte(), 'N'.code.toByte()))
        bytes.put(1)
        bytes.put(2)
        bytes.putInt(48_000)
        bytes.putLong(sequence)
        bytes.putShort(samples.size.toShort())
        samples.forEach(bytes::putShort)
        return bytes.array()
    }
}
