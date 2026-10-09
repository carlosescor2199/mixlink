package com.mixlink.android

import android.app.Activity
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioTrack
import android.os.Bundle
import android.view.View
import android.widget.AdapterView
import android.widget.ArrayAdapter
import android.widget.Button
import android.widget.EditText
import android.widget.CheckBox
import android.widget.LinearLayout
import android.widget.SeekBar
import android.widget.Spinner
import android.widget.TextView
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import org.json.JSONArray
import org.json.JSONObject
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.SocketException
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.Executors

class MainActivity : Activity() {
    private lateinit var hostInput: EditText
    private lateinit var portInput: EditText
    private lateinit var controlPortInput: EditText
    private lateinit var startButton: Button
    private lateinit var stopButton: Button
    private lateinit var statusText: TextView
    private lateinit var controlStatusText: TextView
    private lateinit var channelContainer: LinearLayout
    private lateinit var statsText: TextView
    private lateinit var errorText: TextView
    private lateinit var volumeSeekBar: SeekBar
    private lateinit var maxLevelSeekBar: SeekBar
    private lateinit var muteCheckBox: CheckBox
    private lateinit var volumeValueText: TextView
    private lateinit var maxLevelValueText: TextView
    private lateinit var bankNameInput: EditText
    private lateinit var saveBankButton: Button
    private lateinit var bankStatusText: TextView
    private lateinit var bankContainer: LinearLayout
    private lateinit var musicianChannelSpinner: Spinner
    private lateinit var moreOfMeCheckBox: CheckBox
    private lateinit var moreOfMeStatusText: TextView
    private lateinit var bankStore: MixBankStore

    private val running = AtomicBoolean(false)
    @Volatile private var socket: DatagramSocket? = null
    @Volatile private var receiverThread: Thread? = null
    @Volatile private var controlWebSocket: WebSocket? = null
    @Volatile private var lastError = "none"
    @Volatile private var controlError = "none"
    @Volatile private var controlState = ControlState.IDLE
    @Volatile private var mixAcknowledged = false
    @Volatile private var sourceChannels = 0
    @Volatile private var channelGains: IntArray = IntArray(0)
    @Volatile private var channelPans: IntArray = IntArray(0)
    @Volatile private var channelMutes: BooleanArray = BooleanArray(0)
    @Volatile private var channelSolos: BooleanArray = BooleanArray(0)
    @Volatile private var volumePercent = 100
    @Volatile private var maxLevelPercent = 100
    @Volatile private var muted = false
    @Volatile private var moreOfMeEnabled = false
    @Volatile private var musicianChannel: Int? = null
    private var updatingMusicianSpinner = false
    private var updatingMoreOfMe = false
    private val controlExecutor = Executors.newSingleThreadExecutor()
    private val httpClient = OkHttpClient()
    private val controlListener = object : WebSocketListener() {
        override fun onOpen(webSocket: WebSocket, response: Response) {
            controlWebSocket = webSocket
            controlState = ControlState.CONNECTED
            renderControlStatus()
            sendCurrentMix()
        }

        override fun onMessage(webSocket: WebSocket, text: String) {
            try {
                val message = JSONObject(text)
                when (message.optString("type")) {
                    "error" -> showControlError(message.optString("message", "unknown control error"))
                    "config" -> {
                        val announcedChannels = message.optInt("source_channels", -1)
                        if (announcedChannels in 1..MAX_CHANNELS) {
                            syncChannelControls(announcedChannels)
                        }
                    }
                    "mix_ack" -> {
                        mixAcknowledged = true
                        renderControlStatus()
                    }
                }
            } catch (error: Exception) {
                showControlError("Invalid control response: ${error.message ?: "unknown error"}")
            }
        }

        override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
            controlWebSocket = null
            controlState = if (running.get()) ControlState.UNAVAILABLE else ControlState.IDLE
            renderControlStatus()
            if (running.get()) showControlError("WebSocket error: ${t.message ?: "connection failed"}")
        }

        override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
            controlWebSocket = null
            controlState = if (running.get()) ControlState.CLOSED else ControlState.IDLE
            renderControlStatus()
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_main)

        hostInput = findViewById(R.id.hostInput)
        portInput = findViewById(R.id.portInput)
        controlPortInput = findViewById(R.id.controlPortInput)
        startButton = findViewById(R.id.startButton)
        stopButton = findViewById(R.id.stopButton)
        statusText = findViewById(R.id.statusText)
        controlStatusText = findViewById(R.id.controlStatusText)
        channelContainer = findViewById(R.id.channelContainer)
        statsText = findViewById(R.id.statsText)
        errorText = findViewById(R.id.errorText)
        volumeSeekBar = findViewById(R.id.volumeSeekBar)
        maxLevelSeekBar = findViewById(R.id.maxLevelSeekBar)
        muteCheckBox = findViewById(R.id.muteCheckBox)
        volumeValueText = findViewById(R.id.volumeValueText)
        maxLevelValueText = findViewById(R.id.maxLevelValueText)
        bankNameInput = findViewById(R.id.bankNameInput)
        saveBankButton = findViewById(R.id.saveBankButton)
        bankStatusText = findViewById(R.id.bankStatusText)
        bankContainer = findViewById(R.id.bankContainer)
        musicianChannelSpinner = findViewById(R.id.musicianChannelSpinner)
        moreOfMeCheckBox = findViewById(R.id.moreOfMeCheckBox)
        moreOfMeStatusText = findViewById(R.id.moreOfMeStatusText)

        bankStore = MixBankStore(this)
        musicianChannel = bankStore.musicianChannel()
        saveBankButton.setOnClickListener { saveCurrentBank() }
        moreOfMeCheckBox.setOnCheckedChangeListener { _, checked -> onMoreOfMeToggled(checked) }
        musicianChannelSpinner.onItemSelectedListener = object : AdapterView.OnItemSelectedListener {
            override fun onItemSelected(parent: AdapterView<*>?, view: View?, position: Int, id: Long) {
                onMusicianChannelSelected(position)
            }

            override fun onNothingSelected(parent: AdapterView<*>?) = Unit
        }
        rebuildBankList()

        volumeSeekBar.setOnSeekBarChangeListener(object : SeekBar.OnSeekBarChangeListener {
            override fun onProgressChanged(seekBar: SeekBar?, progress: Int, fromUser: Boolean) {
                volumePercent = progress
                volumeValueText.text = "$progress%"
                sendMixControl()
            }

            override fun onStartTrackingTouch(seekBar: SeekBar?) = Unit

            override fun onStopTrackingTouch(seekBar: SeekBar?) = Unit
        })
        maxLevelSeekBar.setOnSeekBarChangeListener(object : SeekBar.OnSeekBarChangeListener {
            override fun onProgressChanged(seekBar: SeekBar?, progress: Int, fromUser: Boolean) {
                maxLevelPercent = progress
                maxLevelValueText.text = "$progress%"
                sendMixControl()
            }

            override fun onStartTrackingTouch(seekBar: SeekBar?) = Unit

            override fun onStopTrackingTouch(seekBar: SeekBar?) = Unit
        })
        muteCheckBox.setOnCheckedChangeListener { _, checked ->
            muted = checked
            sendMixControl()
        }

        startButton.setOnClickListener { startReceiver() }
        stopButton.setOnClickListener { stopReceiver() }
    }

    private fun startReceiver() {
        if (running.get()) return
        if (receiverThread?.isAlive == true) {
            showError("The previous receiver is still stopping")
            return
        }

        val host = hostInput.text.toString().trim()
        val port = portInput.text.toString().toIntOrNull()
        val controlPort = controlPortInput.text.toString().toIntOrNull()
        if (host.isEmpty()) {
            showError("Enter the server IP address or host name")
            return
        }
        if (port == null || port !in 1..65535) {
            showError("Enter a valid UDP port between 1 and 65535")
            return
        }
        if (controlPort == null || controlPort !in 1..65535) {
            showError("Enter a valid WebSocket control port between 1 and 65535")
            return
        }
        lastError = "none"
        controlError = "none"
        running.set(true)
        startButton.isEnabled = false
        stopButton.isEnabled = true
        hostInput.isEnabled = false
        portInput.isEnabled = false
        controlPortInput.isEnabled = false
        statusText.text = "Status: Starting on UDP port $port; control port $controlPort"
        updateStats(0, 0, null)
        controlState = ControlState.CONNECTING
        renderControlStatus()
        connectControl(host, controlPort)

        val thread = Thread({ receiveLoop(host, port) }, "pmon-udp-receiver")
        receiverThread = thread
        thread.start()
    }

    private fun stopReceiver() {
        if (!running.getAndSet(false)) return
        socket?.close()
        receiverThread?.interrupt()
        statusText.text = "Status: Stopping"
        startButton.isEnabled = true
        stopButton.isEnabled = false
        hostInput.isEnabled = true
        portInput.isEnabled = true
        controlPortInput.isEnabled = true
        controlWebSocket?.close(1000, "stopped")
        controlWebSocket = null
        controlState = ControlState.IDLE
        renderControlStatus()
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
                showError("Could not resolve server host: ${error.message ?: "unknown error"}")
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
                            volumePercent = volumePercent,
                            maxLevelPercent = maxLevelPercent,
                            muted = muted,
                            serverOwnsMix = mixAcknowledged,
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
                        updateStats(
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
                        showError(error.message ?: "invalid PMON packet")
                    }
                }
            }
        } catch (error: SocketException) {
            if (running.get()) showError("UDP socket error: ${error.message ?: "unknown error"}")
        } catch (error: Exception) {
            if (running.get()) showError("Receiver error: ${error.message ?: "unknown error"}")
        } finally {
            audioTrack?.run {
                pause()
                flush()
                release()
            }
            socket = null
            receiverThread = null
            running.set(false)
            runOnUiThread {
                startButton.isEnabled = true
                stopButton.isEnabled = false
                hostInput.isEnabled = true
                portInput.isEnabled = true
                controlPortInput.isEnabled = true
                statusText.text = "Status: Stopped"
            }
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

    private fun updateStats(
        packets: Long,
        lost: Long,
        sampleRate: Int?,
        packetsPerSecond: Double = 0.0,
        interArrivalMs: Double = 0.0,
        jitterMs: Double = 0.0,
        discardedSamples: Long = 0,
        packetsWithDiscardedSamples: Long = 0,
    ) {
        runOnUiThread {
            statsText.text = "Packets received: $packets\nSequences lost: $lost\nLast sample rate: ${sampleRate?.let { "$it Hz" } ?: "-"}\nPackets per second: %.1f\nInter-arrival: %.1f ms\nJitter: %.1f ms\nAudio samples discarded: $discardedSamples\nAudio packets with discarded samples: $packetsWithDiscardedSamples".format(
                packetsPerSecond,
                interArrivalMs,
                jitterMs,
            )
            statusText.text = if (running.get()) "Status: Receiving" else "Status: Stopped"
        }
    }

    private fun showError(message: String) {
        lastError = message
        renderErrors()
    }

    private fun showControlError(message: String) {
        controlError = message
        renderErrors()
    }

    private fun renderErrors() {
        runOnUiThread {
            errorText.text = "Errors: $lastError\nControl: $controlError"
        }
    }

    private fun renderControlStatus() {
        val label = controlStatusLabel(controlState, mixAcknowledged)
        runOnUiThread { controlStatusText.text = "Control: $label" }
    }

    private fun connectControl(host: String, port: Int) {
        controlExecutor.execute {
            try {
                val request = Request.Builder()
                    .url("ws://$host:$port")
                    .build()
                httpClient.newWebSocket(request, controlListener)
            } catch (error: Exception) {
                if (running.get()) showControlError("Could not start WebSocket: ${error.message ?: "unknown error"}")
            }
        }
    }

    private fun sendCurrentMix() {
        sendMixControl()
    }

    private fun sendMixControl() {
        controlExecutor.execute {
            val webSocket = controlWebSocket ?: return@execute
            val stored = currentSnapshot()
            // `More of me` is derived here, at send time, and never written back. The stored values
            // stay exactly as the musician left them, so turning the control off re-sends them.
            val outgoing = if (moreOfMeEnabled) stored.moreOfMe(musicianChannel) ?: stored else stored
            if (!webSocket.send(buildMixMessage(outgoing)) && running.get()) {
                showControlError("WebSocket rejected mix update")
            }
        }
    }

    private fun currentSnapshot(): MixSnapshot = MixSnapshot(
        channelGains = channelGains,
        channelPans = channelPans,
        channelMutes = channelMutes,
        channelSolos = channelSolos,
        volumePercent = volumePercent,
        maxLevelPercent = maxLevelPercent,
        muted = muted,
    )

    private fun buildMixMessage(snapshot: MixSnapshot): String {
        val channels = JSONArray()
        for (gain in snapshot.channelGains) {
            channels.put(gain)
        }
        val panLevels = JSONArray()
        for (pan in snapshot.channelPans) {
            panLevels.put(pan)
        }
        val muteFlags = JSONArray()
        for (mute in snapshot.channelMutes) {
            muteFlags.put(mute)
        }
        val soloFlags = JSONArray()
        for (solo in snapshot.channelSolos) {
            soloFlags.put(solo)
        }
        return JSONObject()
            .put("type", "mix")
            .put("volume_percent", snapshot.volumePercent)
            .put("max_level_percent", snapshot.maxLevelPercent)
            .put("muted", snapshot.muted)
            .put("channels", channels)
            .put("pans", panLevels)
            .put("mutes", muteFlags)
            .put("solos", soloFlags)
            .toString()
    }

    private fun syncChannelControls(channels: Int) {
        if (sourceChannels == channels && channelGains.size == channels) return
        sourceChannels = channels
        channelGains = IntArray(channels) { 100 }
        channelPans = IntArray(channels) { index -> defaultPan(index) }
        channelMutes = BooleanArray(channels)
        channelSolos = BooleanArray(channels)
        runOnUiThread {
            rebuildChannelControls(channels)
            renderMusicianChannel()
        }
    }

    private fun rebuildChannelControls(channels: Int) {
        channelContainer.removeAllViews()
        for (index in 0 until channels) {
            val initialGain = channelGains.getOrElse(index) { 100 }
            val gainLabel = TextView(this).apply { text = channelLabel(index, initialGain) }
            val gainSeekBar = SeekBar(this).apply {
                max = 100
                progress = initialGain
                setOnSeekBarChangeListener(object : SeekBar.OnSeekBarChangeListener {
                    override fun onProgressChanged(bar: SeekBar?, progress: Int, fromUser: Boolean) {
                        gainLabel.text = channelLabel(index, progress)
                        channelGains = channelGains.copyOf().also { it[index] = progress }
                        sendMixControl()
                    }

                    override fun onStartTrackingTouch(bar: SeekBar?) = Unit

                    override fun onStopTrackingTouch(bar: SeekBar?) = Unit
                })
            }
            val initialPan = channelPans.getOrElse(index) { defaultPan(index) }
            val panLabelView = TextView(this).apply { text = panLabel(index, initialPan) }
            val panSeekBar = SeekBar(this).apply {
                max = 100
                progress = initialPan
                setOnSeekBarChangeListener(object : SeekBar.OnSeekBarChangeListener {
                    override fun onProgressChanged(bar: SeekBar?, progress: Int, fromUser: Boolean) {
                        panLabelView.text = panLabel(index, progress)
                        channelPans = channelPans.copyOf().also { it[index] = progress }
                        sendMixControl()
                    }

                    override fun onStartTrackingTouch(bar: SeekBar?) = Unit

                    override fun onStopTrackingTouch(bar: SeekBar?) = Unit
                })
            }
            channelContainer.addView(gainLabel)
            channelContainer.addView(gainSeekBar)
            channelContainer.addView(panLabelView)
            channelContainer.addView(panSeekBar)
            val muteCheckBox = CheckBox(this).apply {
                text = "Mute ${index + 1}"
                isChecked = channelMutes.getOrElse(index) { false }
                setOnCheckedChangeListener { _, checked ->
                    channelMutes = channelMutes.copyOf().also { it[index] = checked }
                    sendMixControl()
                }
            }
            val soloCheckBox = CheckBox(this).apply {
                text = "Solo ${index + 1}"
                isChecked = channelSolos.getOrElse(index) { false }
                setOnCheckedChangeListener { _, checked ->
                    channelSolos = channelSolos.copyOf().also { it[index] = checked }
                    sendMixControl()
                }
            }
            channelContainer.addView(muteCheckBox)
            channelContainer.addView(soloCheckBox)
        }
    }

    private fun channelLabel(index: Int, percent: Int): String = "Channel ${index + 1}: $percent%"

    private fun panLabel(index: Int, percent: Int): String {
        val position = when {
            percent <= 0 -> "L"
            percent >= 100 -> "R"
            percent == 50 -> "C"
            else -> "$percent%"
        }
        return "Pan ${index + 1}: $position"
    }

    private fun defaultPan(index: Int): Int = if (index % 2 == 0) 0 else 100

    private fun saveCurrentBank() {
        val name = bankNameInput.text.toString().trim()
        if (name.isEmpty()) {
            bankStatusText.text = "Enter a bank name before saving"
            return
        }
        bankStore.saveBank(name, currentSnapshot())
        bankNameInput.setText("")
        bankStatusText.text = "Saved \"$name\""
        rebuildBankList()
    }

    private fun rebuildBankList() {
        bankContainer.removeAllViews()
        val banks = bankStore.loadBanks()
        if (banks.isEmpty()) {
            bankStatusText.text = "No saved banks"
            return
        }
        for (bank in banks) {
            val nameView = TextView(this).apply {
                text = bank.name
                layoutParams = LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f)
            }
            val recallButton = Button(this).apply {
                text = "Recall"
                setOnClickListener { recallBank(bank.name) }
            }
            val deleteButton = Button(this).apply {
                text = "Delete"
                setOnClickListener {
                    bankStore.deleteBank(bank.name)
                    bankStatusText.text = "Deleted \"${bank.name}\""
                    rebuildBankList()
                }
            }
            bankContainer.addView(LinearLayout(this).apply {
                orientation = LinearLayout.HORIZONTAL
                addView(nameView)
                addView(recallButton)
                addView(deleteButton)
            })
        }
    }

    private fun recallBank(name: String) {
        val bank = bankStore.loadBanks().firstOrNull { it.name == name }
        if (bank == null) {
            bankStatusText.text = "Bank \"$name\" is no longer stored"
            rebuildBankList()
            return
        }
        applySnapshot(bank.snapshot)
        bankStatusText.text = "Recalled \"$name\""
    }

    /**
     * Applies a recalled bank to the live controls and sends the resulting mix through the existing
     * `mix` path, exactly as if the musician had moved every control by hand.
     */
    private fun applySnapshot(snapshot: MixSnapshot) {
        channelGains = snapshot.channelGains.copyOf()
        channelPans = snapshot.channelPans.copyOf()
        channelMutes = snapshot.channelMutes.copyOf()
        channelSolos = snapshot.channelSolos.copyOf()
        volumePercent = snapshot.volumePercent
        maxLevelPercent = snapshot.maxLevelPercent
        muted = snapshot.muted
        runOnUiThread {
            volumeSeekBar.progress = volumePercent
            volumeValueText.text = "$volumePercent%"
            maxLevelSeekBar.progress = maxLevelPercent
            maxLevelValueText.text = "$maxLevelPercent%"
            muteCheckBox.isChecked = muted
            rebuildChannelControls(channelGains.size)
            sendMixControl()
        }
    }

    private fun onMoreOfMeToggled(checked: Boolean) {
        if (updatingMoreOfMe) return
        if (checked && musicianChannel == null) {
            updatingMoreOfMe = true
            moreOfMeCheckBox.isChecked = false
            updatingMoreOfMe = false
            moreOfMeStatusText.text = "More of me needs your channel: pick one above first"
            return
        }
        moreOfMeEnabled = checked
        moreOfMeStatusText.text = if (checked) "More of me is on" else "More of me is off"
        sendMixControl()
    }

    private fun onMusicianChannelSelected(position: Int) {
        if (updatingMusicianSpinner) return
        val selected = if (position <= 0) null else position - 1
        musicianChannel = selected
        bankStore.setMusicianChannel(selected)
        when {
            selected == null && moreOfMeEnabled -> {
                updatingMoreOfMe = true
                moreOfMeCheckBox.isChecked = false
                updatingMoreOfMe = false
                moreOfMeEnabled = false
                moreOfMeStatusText.text = "More of me is off: no channel selected"
                sendMixControl()
            }
            moreOfMeEnabled -> sendMixControl()
        }
    }

    private fun renderMusicianChannel() {
        if (sourceChannels > 0 && musicianChannel != null && musicianChannel !in 0 until sourceChannels) {
            musicianChannel = null
            bankStore.setMusicianChannel(null)
        }
        val entries = ArrayList<String>(sourceChannels + 1)
        entries.add("None")
        for (index in 0 until sourceChannels) {
            entries.add("Channel ${index + 1}")
        }
        updatingMusicianSpinner = true
        musicianChannelSpinner.adapter = ArrayAdapter(this, android.R.layout.simple_spinner_item, entries)
            .also { it.setDropDownViewResource(android.R.layout.simple_spinner_dropdown_item) }
        musicianChannelSpinner.setSelection((musicianChannel ?: -1) + 1)
        updatingMusicianSpinner = false
    }

    override fun onDestroy() {
        stopReceiver()
        controlWebSocket?.close(1000, "destroyed")
        controlWebSocket = null
        controlExecutor.shutdownNow()
        httpClient.dispatcher.executorService.shutdown()
        super.onDestroy()
    }
}

private const val MAX_CHANNELS = 32
