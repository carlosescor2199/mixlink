package com.mixlink.android

import android.content.Context
import android.widget.LinearLayout
import android.widget.SeekBar
import android.widget.TextView

/**
 * Builds one fader per engineer-defined group into [container], following the per-channel pattern in
 * [rebuildChannelControls].
 *
 * With no groups the container is emptied and left empty, so a server that announces no groups
 * leaves the client exactly as it is today: no empty section and no stray label.
 */
internal fun rebuildGroupControls(
    context: Context,
    container: LinearLayout,
    groups: List<GroupInfo>,
    mixState: ClientMixState,
    onSendMix: () -> Unit,
) {
    container.removeAllViews()
    for ((index, group) in groups.withIndex()) {
        val initialLevel = mixState.groupLevels.getOrElse(index) { 100 }
        val label = TextView(context).apply { text = groupLabel(group.name, initialLevel) }
        val seekBar = SeekBar(context).apply {
            max = 100
            progress = initialLevel
            setOnSeekBarChangeListener(object : SeekBar.OnSeekBarChangeListener {
                override fun onProgressChanged(bar: SeekBar?, progress: Int, fromUser: Boolean) {
                    label.text = groupLabel(group.name, progress)
                    mixState.groupLevels = IntArray(groups.size) { position ->
                        if (position == index) {
                            progress
                        } else {
                            mixState.groupLevels.getOrElse(position) { 100 }
                        }
                    }
                    onSendMix()
                }

                override fun onStartTrackingTouch(bar: SeekBar?) = Unit

                override fun onStopTrackingTouch(bar: SeekBar?) = Unit
            })
        }
        container.addView(label)
        container.addView(seekBar)
    }
}

private fun groupLabel(name: String, percent: Int): String = "$name: $percent%"
