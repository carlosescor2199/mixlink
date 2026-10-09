package com.mixlink.android

import org.json.JSONArray
import org.json.JSONObject
import kotlin.math.roundToInt

/**
 * A complete, device-local picture of a mix: the four per-channel arrays plus the master volume,
 * ceiling and mute.
 *
 * The type is deliberately pure. It imports nothing from Android and performs no IO, so its JSON
 * round trip and its [moreOfMe] derivation are ordinary JVM tests. [moreOfMe] never mutates the
 * receiver: it returns a new snapshot computed from the stored values, which is what lets the UI
 * turn the control off and restore the previous mix with nothing to save or replay.
 */
data class MixSnapshot(
    val channelGains: IntArray = IntArray(0),
    val channelPans: IntArray = IntArray(0),
    val channelMutes: BooleanArray = BooleanArray(0),
    val channelSolos: BooleanArray = BooleanArray(0),
    val volumePercent: Int = 100,
    val maxLevelPercent: Int = 100,
    val muted: Boolean = false,
) {
    fun toJson(): String = JSONObject().apply {
        put("channelGains", JSONArray(channelGains.toList()))
        put("channelPans", JSONArray(channelPans.toList()))
        put("channelMutes", JSONArray(channelMutes.toList()))
        put("channelSolos", JSONArray(channelSolos.toList()))
        put("volumePercent", volumePercent)
        put("maxLevelPercent", maxLevelPercent)
        put("muted", muted)
    }.toString()

    /**
     * Applies the `More of me` rule: the musician's channel is pushed to [MORE_OF_ME_CHANNEL_GAIN]
     * and every other channel is scaled by [MORE_OF_ME_OTHER_FACTOR]. Everything else is untouched.
     *
     * Returns `null` when no valid channel is designated, so the caller can do nothing and tell the
     * musician instead of guessing. The receiver itself is never modified.
     */
    fun moreOfMe(musicianChannel: Int?): MixSnapshot? {
        if (musicianChannel == null || musicianChannel !in channelGains.indices) return null
        val gains = IntArray(channelGains.size) { index ->
            if (index == musicianChannel) {
                MORE_OF_ME_CHANNEL_GAIN
            } else {
                (channelGains[index] * MORE_OF_ME_OTHER_FACTOR).roundToInt()
            }
        }
        return copy(channelGains = gains)
    }

    companion object {
        const val MORE_OF_ME_CHANNEL_GAIN = 100
        const val MORE_OF_ME_OTHER_FACTOR = 0.5

        /**
         * Rebuilds a snapshot from the JSON written by [toJson]. Throws when the text is not a
         * snapshot object; callers that read untrusted storage are expected to catch that.
         */
        fun fromJson(json: String?): MixSnapshot {
            val parsed = JSONObject(json ?: throw IllegalArgumentException("snapshot JSON is missing"))
            return MixSnapshot(
                channelGains = parsed.intArray("channelGains"),
                channelPans = parsed.intArray("channelPans"),
                channelMutes = parsed.booleanArray("channelMutes"),
                channelSolos = parsed.booleanArray("channelSolos"),
                volumePercent = parsed.optInt("volumePercent", 100),
                maxLevelPercent = parsed.optInt("maxLevelPercent", 100),
                muted = parsed.optBoolean("muted", false),
            )
        }

        private fun JSONObject.intArray(key: String): IntArray {
            val array = optJSONArray(key) ?: return IntArray(0)
            return IntArray(array.length()) { index -> array.optInt(index, 0) }
        }

        private fun JSONObject.booleanArray(key: String): BooleanArray {
            val array = optJSONArray(key) ?: return BooleanArray(0)
            return BooleanArray(array.length()) { index -> array.optBoolean(index, false) }
        }
    }
}
