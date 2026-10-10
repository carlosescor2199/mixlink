package com.mixlink.android

import android.content.Context
import android.widget.Button
import android.widget.LinearLayout

/**
 * Builds one tap target per discovered server into [container], following the group-row pattern.
 *
 * With no servers the container is emptied and left empty, so a client that hears no beacon looks
 * exactly as it does today: no empty section and no stray label. Selecting a server only reports
 * the choice through [onSelect]; it never presses Start.
 */
internal fun rebuildFoundServers(
    context: Context,
    container: LinearLayout,
    servers: List<DiscoveredServer>,
    onSelect: (DiscoveredServer) -> Unit,
) {
    container.removeAllViews()
    for (server in servers) {
        val button = Button(context).apply {
            text = "${server.address}:${server.controlPort}"
            setOnClickListener { onSelect(server) }
        }
        container.addView(button)
    }
}
