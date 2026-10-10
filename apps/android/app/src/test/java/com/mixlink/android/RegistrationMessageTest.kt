package com.mixlink.android

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Test

/**
 * Pure JVM coverage for the registration message the client sends when the control channel opens.
 * The server takes the address from the socket, so the message must not carry one, and a server
 * that does not understand the message is allowed to answer with an error or ignore it.
 */
class RegistrationMessageTest {
    @Test
    fun carriesTheTypeTheNameAndTheUdpPort() {
        val message = JSONObject(buildRegisterMessage("Ana", 50000))

        assertEquals("register", message.getString("type"))
        assertEquals("Ana", message.getString("name"))
        assertEquals(50000, message.getInt("udp_port"))
    }

    @Test
    fun trimsTheNameButKeepsAnEmptyOne() {
        assertEquals("Ana", JSONObject(buildRegisterMessage("  Ana  ", 50001)).getString("name"))
        assertEquals("", JSONObject(buildRegisterMessage("   ", 50001)).getString("name"))
    }

    @Test
    fun carriesNoAddressBecauseTheServerUsesTheSocket() {
        val message = JSONObject(buildRegisterMessage("Ana", 50000))

        assertFalse(message.has("address"))
        assertFalse(message.has("ip"))
    }

    @Test
    fun survivesANameWithQuotesAndAccents() {
        val name = "Ana \"La Jefa\" Güemes"

        assertEquals(name, JSONObject(buildRegisterMessage(name, 50000)).getString("name"))
    }
}
