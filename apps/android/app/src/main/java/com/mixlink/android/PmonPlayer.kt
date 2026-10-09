package com.mixlink.android

import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioTrack
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.SocketException
import java.util.concurrent.atomic.AtomicBoolean

/**
 * The UDP receive loop, AudioTrack playback and the receiver metrics.
 *
 * The player owns the socket, the receiver thread and the audio track, so it needs no `Context`.
 * It reports metrics, errors, completion and the `server owns the mix` decision through [Listener];
 * it never touches the UI itself. [running] is the shared session flag owned by the caller, so the
 * start/stop gating is identical to before the split.
 */
internal class PmonPlayer(
    private val listener: Listener,
    private val running: AtomicBoolean,
    private val mixState: ClientMixState,
) {
    interface Listener {
        /** True once the server has acknowledged a mix, so local processing must not run. */
        fun serverOwnsMix(): Boolean

        fun onStats(
            packets: Long,
            lost: Long,
            sampleRate: Int?,
            packetsPerSecond: Double,
            interArrivalMs: Double,
            jitterMs: Double,
            discardedSamples: Long,
            packetsWithDiscardedSamples: Long,
        )

        fun onError(message: String)

        fun onStopped()
    }

    @Volatile private var socket: DatagramSocket? = null
    @Volatile private var receiverThread: Thread? = null

    fun isReceiverAlive(): Boolean = receiverThread?.isAlive == true

    fun launch(host: String, port: Int) {
        val thread = Thread({ receiveLoop(host, port) }, "pmon-udp-receiver")
        receiverThread = thread
        thread.start()
    }

    fun stopReceiving() {
        socket?.close()
        receiverThread?.interrupt()
    }

    private fun receiveLoop(host: String, port: Int) {
        var packetsReceived = 0L
        var sequencesLost = 0L
        var expectedSequence: Long? = null
        var lastSampleRate: Int? = null
        var sourceSampleRate: Int? = null
        var audioTrack: AudioTrack? = null
        var playbackRate: Int? = null
        val metricsStartedAtNanos = System.nanoTime()
        var previousPacketAtNanos: Long? = null
        var interArrivalMs = 0.0
        var jitterMs = 0.0
        var packetsPerSecond = 0.0
        var discardedSamples = 0L
        var packetsWithDiscardedSamples = 0L
        val buffer = ByteArray(65_507)

        try {
            try {
                InetAddress.getByName(host)
            } catch (error: Exception) {
                listener.onError("Could not resolve server host: ${error.message ?: "unknown error"}")
                return
            }
            DatagramSocket(port).also { openedSocket ->
                socket = openedSocket
                openedSocket.soTimeout = 250
            }.use { openedSocket ->
                while (running.get()) {
                    val datagram = DatagramPacket(buffer, buffer.size)
                    datagram.setLength(buffer.size)
                    try {
                        openedSocket.receive(datagram)
                    } catch (_: java.net.SocketTimeoutException) {
                        continue
                    }

                    try {
                        val packet = PmonPacketParser.parse(datagram.data, datagram.length)
                        if (packet.channels > MAX_CHANNELS) {
                            throw PmonPacketParseException(
                                "PMON packet announces ${packet.channels} channels, the limit is $MAX_CHANNELS",
                            )
                        }
                        if (expectedSequence != null && packet.sequence > expectedSequence!!) {
                            sequencesLost += packet.sequence - expectedSequence!!
                        }
                        expectedSequence = packet.sequence + 1
                        packetsReceived++
                        lastSampleRate = packet.sampleRate
                        val packetReceivedAtNanos = System.nanoTime()
                        previousPacketAtNanos?.let { previousAtNanos ->
                            val intervalMs = (packetReceivedAtNanos - previousAtNanos) / 1_000_000.0
                            if (interArrivalMs == 0.0) {
                                interArrivalMs = intervalMs
                            } else {
                                interArrivalMs += 0.1 * (intervalMs - interArrivalMs)
                            }
                            jitterMs += 0.1 * (kotlin.math.abs(intervalMs - interArrivalMs) - jitterMs)
                        }
                        previousPacketAtNanos = packetReceivedAtNanos
                        val elapsedSeconds = (packetReceivedAtNanos - metricsStartedAtNanos) / 1_000_000_000.0
                        packetsPerSecond = if (elapsedSeconds > 0.0) packetsReceived / elapsedSeconds else 0.0

                        if (audioTrack == null) {
                            sourceSampleRate = packet.sampleRate
                            playbackRate = selectPlaybackRate(packet.sampleRate)
                            audioTrack = createAudioTrack(playbackRate)
                            audioTrack.play()
                        } else if (packet.sampleRate != sourceSampleRate) {
                            throw PmonPacketParseException(
                                "sample rate changed from $sourceSampleRate to ${packet.sampleRate}",
                            )
                        }

                        val processedSamples = Pcm16Processor.applyLocalProtection(
                            samples = packet.samples,
                            volumePercent = mixState.volumePercent,
                            maxLevelPercent = mixState.maxLevelPercent,
                            muted = mixState.muted,
                            serverOwnsMix = listener.serverOwnsMix(),
                        )
                        val writtenSamples = audioTrack.write(
                            processedSamples,
                            0,
                            processedSamples.size,
                            AudioTrack.WRITE_NON_BLOCKING,
                        )
                        if (writtenSamples < processedSamples.size) {
                            discardedSamples += if (writtenSamples >= 0) {
                                (processedSamples.size - writtenSamples).toLong()
                            } else {
                                processedSamples.size.toLong()
                            }
                            packetsWithDiscardedSamples++
                        }
                        listener.onStats(
                            packetsReceived,
                            sequencesLost,
                            lastSampleRate,
                            packetsPerSecond,
                            interArrivalMs,
                            jitterMs,
                            discardedSamples,
                            packetsWithDiscardedSamples,
                        )
                    } catch (error: Exception) {
                        listener.onError(error.message ?: "invalid PMON packet")
                    }
                }
            }
        } catch (error: SocketException) {
            if (running.get()) listener.onError("UDP socket error: ${error.message ?: "unknown error"}")
        } catch (error: Exception) {
            if (running.get()) listener.onError("Receiver error: ${error.message ?: "unknown error"}")
        } finally {
            audioTrack?.run {
                pause()
                flush()
                release()
            }
            socket = null
            receiverThread = null
            running.set(false)
            listener.onStopped()
        }
    }

    private fun selectPlaybackRate(announcedRate: Int): Int {
        val minimum = AudioTrack.getMinBufferSize(
            announcedRate,
            AudioFormat.CHANNEL_OUT_STEREO,
            AudioFormat.ENCODING_PCM_16BIT,
        )
        return if (minimum > 0) announcedRate else 48_000
    }

    private fun createAudioTrack(sampleRate: Int): AudioTrack {
        val minimum = AudioTrack.getMinBufferSize(
            sampleRate,
            AudioFormat.CHANNEL_OUT_STEREO,
            AudioFormat.ENCODING_PCM_16BIT,
        )
        if (minimum <= 0) throw IllegalStateException("sample rate $sampleRate is not supported")
        return AudioTrack.Builder()
            .setAudioAttributes(
                AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_GAME)
                    .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                    .build(),
            )
            .setAudioFormat(
                AudioFormat.Builder()
                    .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                    .setSampleRate(sampleRate)
                    .setChannelMask(AudioFormat.CHANNEL_OUT_STEREO)
                    .build(),
            )
            .setBufferSizeInBytes(minimum)
            .setTransferMode(AudioTrack.MODE_STREAM)
            .setPerformanceMode(AudioTrack.PERFORMANCE_MODE_LOW_LATENCY)
            .build()
            .also { track ->
                if (track.state != AudioTrack.STATE_INITIALIZED) {
                    track.release()
                    throw IllegalStateException("could not initialize AudioTrack")
                }
            }
    }
}
