package com.mixlink.android

import android.content.Context
import android.content.SharedPreferences
import org.json.JSONArray
import org.json.JSONObject

/** A named mix the musician saved on their own device. */
data class MixBank(val name: String, val snapshot: MixSnapshot)

/**
 * Device-local persistence for saved mix banks and the designated musician channel.
 *
 * Everything lives under one `SharedPreferences` key as JSON. Reading is guarded: malformed or
 * missing data yields an empty bank list and no designated channel instead of crashing, because a
 * bank can sit on a phone for weeks and may have been written by an older version of the app.
 *
 * The designated musician channel is deliberately kept beside the banks rather than inside a saved
 * mix: it describes who the musician is, not a particular balance, so recalling a bank must never
 * overwrite it.
 */
class MixBankStore(context: Context) {
    private val preferences: SharedPreferences =
        context.getSharedPreferences(PREFERENCES_NAME, Context.MODE_PRIVATE)

    fun loadBanks(): List<MixBank> = parseBanks(preferences.getString(KEY_BANKS, null))

    /** Saves a snapshot under [name], replacing an existing bank with the same trimmed name. */
    fun saveBank(name: String, snapshot: MixSnapshot) {
        val trimmed = name.trim()
        if (trimmed.isEmpty()) return
        val banks = loadBanks().filterNot { it.name == trimmed } + MixBank(trimmed, snapshot)
        preferences.edit().putString(KEY_BANKS, serializeBanks(banks)).apply()
    }

    /** Deletes the bank named [name]. Returns whether a bank was actually removed. */
    fun deleteBank(name: String): Boolean {
        val trimmed = name.trim()
        val banks = loadBanks()
        val remaining = banks.filterNot { it.name == trimmed }
        if (remaining.size == banks.size) return false
        preferences.edit().putString(KEY_BANKS, serializeBanks(remaining)).apply()
        return true
    }

    /** The designated musician channel, or `null` when none has been chosen. */
    fun musicianChannel(): Int? =
        preferences.getInt(KEY_MUSICIAN_CHANNEL, NO_CHANNEL).takeIf { it >= 0 }

    fun setMusicianChannel(channel: Int?) {
        preferences.edit().putInt(KEY_MUSICIAN_CHANNEL, channel ?: NO_CHANNEL).apply()
    }

    private companion object {
        const val PREFERENCES_NAME = "mixlink_banks"
        const val KEY_BANKS = "banks"
        const val KEY_MUSICIAN_CHANNEL = "musician_channel"
        const val NO_CHANNEL = -1
    }
}

/**
 * Encodes banks as a JSON array of `{name, snapshot}` objects, with each snapshot kept as an
 * embedded JSON string. Kept pure and top-level so the encoding, and the tolerant decoding below,
 * can be exercised as ordinary JVM tests.
 */
internal fun serializeBanks(banks: List<MixBank>): String {
    val array = JSONArray()
    for (bank in banks) {
        array.put(JSONObject().apply {
            put("name", bank.name)
            put("snapshot", bank.snapshot.toJson())
        })
    }
    return array.toString()
}

/**
 * Decodes banks written by [serializeBanks]. Never throws: a missing, malformed or partially
 * corrupted document falls back to an empty list, skipping only the entries that cannot be read.
 */
internal fun parseBanks(json: String?): List<MixBank> {
    if (json.isNullOrBlank()) return emptyList()
    val parsed = try {
        JSONArray(json)
    } catch (_: Exception) {
        return emptyList()
    }
    val banks = ArrayList<MixBank>(parsed.length())
    for (index in 0 until parsed.length()) {
        val entry = parsed.optJSONObject(index) ?: continue
        val name = entry.opt("name") as? String ?: continue
        val snapshotJson = entry.opt("snapshot") as? String ?: continue
        val snapshot = try {
            MixSnapshot.fromJson(snapshotJson)
        } catch (_: Exception) {
            continue
        }
        banks.add(MixBank(name, snapshot))
    }
    return banks
}
