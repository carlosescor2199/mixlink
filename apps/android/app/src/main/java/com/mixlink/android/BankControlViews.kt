package com.mixlink.android

import android.content.Context
import android.widget.ArrayAdapter
import android.widget.Button
import android.widget.LinearLayout
import android.widget.Spinner
import android.widget.TextView

/**
 * Builds the saved-bank rows and the musician-channel spinner.
 *
 * Both builders take their data and callbacks as parameters instead of reading the Activity, so the
 * caller decides what recall, delete and selection mean. Views are added to the container the caller
 * passes in; the caller keeps the empty-state handling and status text.
 */
internal fun buildBankRows(
    context: Context,
    container: LinearLayout,
    banks: List<MixBank>,
    onRecall: (String) -> Unit,
    onDelete: (String) -> Unit,
) {
    for (bank in banks) {
        val nameView = TextView(context).apply {
            text = bank.name
            layoutParams = LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f)
        }
        val recallButton = Button(context).apply {
            text = "Recall"
            setOnClickListener { onRecall(bank.name) }
        }
        val deleteButton = Button(context).apply {
            text = "Delete"
            setOnClickListener { onDelete(bank.name) }
        }
        container.addView(LinearLayout(context).apply {
            orientation = LinearLayout.HORIZONTAL
            addView(nameView)
            addView(recallButton)
            addView(deleteButton)
        })
    }
}

/**
 * Replaces [spinner]'s adapter with the `None`/`Channel N` entries for [sourceChannels] and selects
 * [musicianChannel]. The caller owns the update guard and the persistence side effects.
 */
internal fun buildMusicianChannelAdapter(
    context: Context,
    spinner: Spinner,
    sourceChannels: Int,
    musicianChannel: Int?,
) {
    val entries = ArrayList<String>(sourceChannels + 1)
    entries.add("None")
    for (index in 0 until sourceChannels) {
        entries.add("Channel ${index + 1}")
    }
    spinner.adapter = ArrayAdapter(context, android.R.layout.simple_spinner_item, entries)
        .also { it.setDropDownViewResource(android.R.layout.simple_spinner_dropdown_item) }
    spinner.setSelection((musicianChannel ?: -1) + 1)
}
