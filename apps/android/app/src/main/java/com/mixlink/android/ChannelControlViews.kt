package com.mixlink.android

import android.content.Context
import android.widget.CheckBox
import android.widget.LinearLayout
import android.widget.SeekBar
import android.widget.TextView

/**
 * Builds the per-channel fader, pan, mute and solo rows into [container].
 *
 * The builder takes the shared [ClientMixState] and an [onSendMix] callback instead of reaching
 * back into the Activity, so every value it writes is the same value the rest of the app reads.
 */
internal fun rebuildChannelControls(
    context: Context,
    container: LinearLayout,
    channels: Int,
    mixState: ClientMixState,
    onSendMix: () -> Unit,
) {
    container.removeAllViews()
    for (index in 0 until channels) {
        val initialGain = mixState.channelGains.getOrElse(index) { 100 }
        val gainLabel = TextView(context).apply { text = channelLabel(index, initialGain) }
        val gainSeekBar = SeekBar(context).apply {
            max = 100
            progress = initialGain
            setOnSeekBarChangeListener(object : SeekBar.OnSeekBarChangeListener {
                override fun onProgressChanged(bar: SeekBar?, progress: Int, fromUser: Boolean) {
                    gainLabel.text = channelLabel(index, progress)
                    mixState.channelGains = mixState.channelGains.copyOf().also { it[index] = progress }
                    onSendMix()
                }

                override fun onStartTrackingTouch(bar: SeekBar?) = Unit

                override fun onStopTrackingTouch(bar: SeekBar?) = Unit
            })
        }
        val initialPan = mixState.channelPans.getOrElse(index) { defaultPan(index) }
        val panLabelView = TextView(context).apply { text = panLabel(index, initialPan) }
        val panSeekBar = SeekBar(context).apply {
            max = 100
            progress = initialPan
            setOnSeekBarChangeListener(object : SeekBar.OnSeekBarChangeListener {
                override fun onProgressChanged(bar: SeekBar?, progress: Int, fromUser: Boolean) {
                    panLabelView.text = panLabel(index, progress)
                    mixState.channelPans = mixState.channelPans.copyOf().also { it[index] = progress }
                    onSendMix()
                }

                override fun onStartTrackingTouch(bar: SeekBar?) = Unit

                override fun onStopTrackingTouch(bar: SeekBar?) = Unit
            })
        }
        container.addView(gainLabel)
        container.addView(gainSeekBar)
        container.addView(panLabelView)
        container.addView(panSeekBar)
        val muteCheckBox = CheckBox(context).apply {
            text = "Mute ${index + 1}"
            isChecked = mixState.channelMutes.getOrElse(index) { false }
            setOnCheckedChangeListener { _, checked ->
                mixState.channelMutes = mixState.channelMutes.copyOf().also { it[index] = checked }
                onSendMix()
            }
        }
        val soloCheckBox = CheckBox(context).apply {
            text = "Solo ${index + 1}"
            isChecked = mixState.channelSolos.getOrElse(index) { false }
            setOnCheckedChangeListener { _, checked ->
                mixState.channelSolos = mixState.channelSolos.copyOf().also { it[index] = checked }
                onSendMix()
            }
        }
        container.addView(muteCheckBox)
        container.addView(soloCheckBox)
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
