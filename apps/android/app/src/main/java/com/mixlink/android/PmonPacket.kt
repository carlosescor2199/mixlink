package com.mixlink.android

import java.nio.ByteBuffer
import java.nio.ByteOrder

class PmonPacketParseException(message: String) : IllegalArgumentException(message)

data class PmonPacket(
    val channels: Int,
    val sampleRate: Int,
    val sequence: Long,
    val samples: ShortArray,
)

object PmonPacketParser {
    const val HEADER_SIZE = 20
    private const val VERSION = 1
    private val MAGIC = byteArrayOf('P'.code.toByte(), 'M'.code.toByte(), 'O'.code.toByte(), 'N'.code.toByte())

    fun parse(data: ByteArray, length: Int = data.size): PmonPacket {
        if (length < HEADER_SIZE) {
            throw PmonPacketParseException("packet is shorter than the PMON header")
        }
        if (length > data.size) {
            throw PmonPacketParseException("packet length exceeds the received buffer")
        }
        for (index in MAGIC.indices) {
            if (data[index] != MAGIC[index]) {
                throw PmonPacketParseException("invalid PMON magic")
            }
        }
        if (data[4].toInt() and 0xff != VERSION) {
            throw PmonPacketParseException("unsupported PMON version")
        }

        val buffer = ByteBuffer.wrap(data).order(ByteOrder.LITTLE_ENDIAN)
        buffer.position(5)
        val channels = buffer.get().toInt() and 0xff
        val sampleRate = buffer.int
        val sequence = buffer.long
        val sampleCount = buffer.short.toInt() and 0xffff

        if (channels <= 0) {
            throw PmonPacketParseException("PMON packet has no channels")
        }
        if (sampleRate <= 0) {
            throw PmonPacketParseException("PMON packet has an invalid sample rate")
        }

        val expectedLength = HEADER_SIZE + sampleCount * 2
        if (length != expectedLength) {
            throw PmonPacketParseException(
                "PCM payload length does not match sample count: expected $expectedLength, got $length",
            )
        }

        val samples = ShortArray(sampleCount)
        buffer.position(HEADER_SIZE)
        for (index in samples.indices) {
            samples[index] = buffer.short
        }
        return PmonPacket(channels, sampleRate, sequence, samples)
    }
}
