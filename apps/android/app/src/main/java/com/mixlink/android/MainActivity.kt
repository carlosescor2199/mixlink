package com.mixlink.android

import android.app.Activity
import android.os.Bundle
import android.view.View
import android.widget.AdapterView
import android.widget.Button
import android.widget.CheckBox
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.SeekBar
import android.widget.Spinner
import android.widget.TextView
import java.util.concurrent.atomic.AtomicBoolean

class MainActivity : Activity(), PmonPlayer.Listener, ControlChannel.Listener {
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
    private val mixState = ClientMixState()
    @Volatile private var lastError = "none"
    @Volatile private var controlError = "none"
    @Volatile private var moreOfMeEnabled = false
    @Volatile private var musicianChannel: Int? = null
    private var updatingMusicianSpinner = false
    private var updatingMoreOfMe = false

    private val player = PmonPlayer(this, running, mixState)
    private val control = ControlChannel(
        listener = this,
        running = running,
        currentSnapshot = ::currentSnapshot,
        moreOfMeEnabled = { moreOfMeEnabled },
        musicianChannel = { musicianChannel },
    )

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
                mixState.volumePercent = progress
                volumeValueText.text = "$progress%"
                control.sendMixControl()
            }

            override fun onStartTrackingTouch(seekBar: SeekBar?) = Unit

            override fun onStopTrackingTouch(seekBar: SeekBar?) = Unit
        })
        maxLevelSeekBar.setOnSeekBarChangeListener(object : SeekBar.OnSeekBarChangeListener {
            override fun onProgressChanged(seekBar: SeekBar?, progress: Int, fromUser: Boolean) {
                mixState.maxLevelPercent = progress
                maxLevelValueText.text = "$progress%"
                control.sendMixControl()
            }

            override fun onStartTrackingTouch(seekBar: SeekBar?) = Unit

            override fun onStopTrackingTouch(seekBar: SeekBar?) = Unit
        })
        muteCheckBox.setOnCheckedChangeListener { _, checked ->
            mixState.muted = checked
            control.sendMixControl()
        }

        startButton.setOnClickListener { startReceiver() }
        stopButton.setOnClickListener { stopReceiver() }
    }

    private fun startReceiver() {
        if (running.get()) return
        if (player.isReceiverAlive()) {
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
        control.controlState = ControlState.CONNECTING
        renderControlStatus()
        control.connect(host, controlPort)

        player.launch(host, port)
    }

    private fun stopReceiver() {
        if (!running.getAndSet(false)) return
        player.stopReceiving()
        statusText.text = "Status: Stopping"
        startButton.isEnabled = true
        stopButton.isEnabled = false
        hostInput.isEnabled = true
        portInput.isEnabled = true
        controlPortInput.isEnabled = true
        control.close(1000, "stopped")
        control.controlState = ControlState.IDLE
        renderControlStatus()
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
        val label = controlStatusLabel(control.controlState, control.mixAcknowledged)
        runOnUiThread { controlStatusText.text = "Control: $label" }
    }

    private fun currentSnapshot(): MixSnapshot = MixSnapshot(
        channelGains = mixState.channelGains,
        channelPans = mixState.channelPans,
        channelMutes = mixState.channelMutes,
        channelSolos = mixState.channelSolos,
        volumePercent = mixState.volumePercent,
        maxLevelPercent = mixState.maxLevelPercent,
        muted = mixState.muted,
    )

    private fun syncChannelControls(channels: Int) {
        if (mixState.sourceChannels == channels && mixState.channelGains.size == channels) return
        mixState.sourceChannels = channels
        mixState.channelGains = IntArray(channels) { 100 }
        mixState.channelPans = IntArray(channels) { index -> defaultPan(index) }
        mixState.channelMutes = BooleanArray(channels)
        mixState.channelSolos = BooleanArray(channels)
        runOnUiThread {
            rebuildChannelControls(this, channelContainer, channels, mixState) { control.sendMixControl() }
            renderMusicianChannel()
        }
    }

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
        buildBankRows(
            context = this,
            container = bankContainer,
            banks = banks,
            onRecall = { name -> recallBank(name) },
            onDelete = { name ->
                bankStore.deleteBank(name)
                bankStatusText.text = "Deleted \"$name\""
                rebuildBankList()
            },
        )
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
        mixState.channelGains = snapshot.channelGains.copyOf()
        mixState.channelPans = snapshot.channelPans.copyOf()
        mixState.channelMutes = snapshot.channelMutes.copyOf()
        mixState.channelSolos = snapshot.channelSolos.copyOf()
        mixState.volumePercent = snapshot.volumePercent
        mixState.maxLevelPercent = snapshot.maxLevelPercent
        mixState.muted = snapshot.muted
        runOnUiThread {
            volumeSeekBar.progress = mixState.volumePercent
            volumeValueText.text = "${mixState.volumePercent}%"
            maxLevelSeekBar.progress = mixState.maxLevelPercent
            maxLevelValueText.text = "${mixState.maxLevelPercent}%"
            muteCheckBox.isChecked = mixState.muted
            rebuildChannelControls(this, channelContainer, mixState.channelGains.size, mixState) { control.sendMixControl() }
            control.sendMixControl()
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
        control.sendMixControl()
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
                control.sendMixControl()
            }
            moreOfMeEnabled -> control.sendMixControl()
        }
    }

    private fun renderMusicianChannel() {
        if (mixState.sourceChannels > 0 && musicianChannel != null && musicianChannel !in 0 until mixState.sourceChannels) {
            musicianChannel = null
            bankStore.setMusicianChannel(null)
        }
        updatingMusicianSpinner = true
        buildMusicianChannelAdapter(this, musicianChannelSpinner, mixState.sourceChannels, musicianChannel)
        updatingMusicianSpinner = false
    }

    override fun serverOwnsMix(): Boolean = control.mixAcknowledged

    override fun onStats(
        packets: Long,
        lost: Long,
        sampleRate: Int?,
        packetsPerSecond: Double,
        interArrivalMs: Double,
        jitterMs: Double,
        discardedSamples: Long,
        packetsWithDiscardedSamples: Long,
    ) {
        updateStats(
            packets,
            lost,
            sampleRate,
            packetsPerSecond,
            interArrivalMs,
            jitterMs,
            discardedSamples,
            packetsWithDiscardedSamples,
        )
    }

    override fun onError(message: String) {
        showError(message)
    }

    override fun onStopped() {
        runOnUiThread {
            startButton.isEnabled = true
            stopButton.isEnabled = false
            hostInput.isEnabled = true
            portInput.isEnabled = true
            controlPortInput.isEnabled = true
            statusText.text = "Status: Stopped"
        }
    }

    override fun onOpened() {
        renderControlStatus()
    }

    override fun onConfig(channels: Int) {
        syncChannelControls(channels)
    }

    override fun onAcknowledged() {
        renderControlStatus()
    }

    override fun onClosed() {
        renderControlStatus()
    }

    override fun onControlError(message: String) {
        showControlError(message)
    }

    override fun onDestroy() {
        stopReceiver()
        control.close(1000, "destroyed")
        control.shutdown()
        super.onDestroy()
    }
}
