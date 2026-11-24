package com.tidal.player

import androidx.media3.common.Player.STATE_IDLE
import androidx.test.ext.junit.runners.AndroidJUnit4
import java.time.Duration
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.After
import org.junit.Assert.fail
import org.junit.Test
import org.junit.runner.RunWith
import uniffi.player.MediaPlayerEvent
import uniffi.player.MediaProduct
import uniffi.player.PlayerEvent
import uniffi.player.PlayerException
import uniffi.player.PlayerState

@RunWith(AndroidJUnit4::class)
class ExoMediaPlayerTest {
    private val a = MediaProduct.Track("a")
    private val b = MediaProduct.Track("b")
    private val hosts = mutableListOf<Host>()

    @After
    fun closePlayers() = hosts.forEach { it.player.close() }

    private fun host(crossfade: Duration? = null, gate: Gate? = null) = Host(crossfade, gate).also(hosts::add)

    @Test
    fun playsPausesSeeksAndResets() = runBlocking {
        val host = host()
        host.player.play(host.seed("a", Tones.tone(5.0)))
        host.events.state(a, PlayerState.Playing)
        host.events.position(a) { it > millis(200) }

        host.player.pause()
        host.events.state(a, PlayerState.Paused)
        host.player.seek(Duration.ofSeconds(3))
        host.events.position(a) { it > millis(1000) }

        host.player.resume()
        host.events.state(a, PlayerState.Playing)
        host.events.position(a) { it > millis(3100) }

        host.player.reset()
        host.events.state(null, PlayerState.Idle)
        assertEquals(0, host.mediaItemCount(0))
    }

    @Test
    fun doesNotReportPositionWhileSeeking() = runBlocking {
        val host = host()
        host.player.play(host.seed("a", Tones.tone(5.0)))
        host.events.position(a) { it > millis(200) }

        host.player.seek(Duration.ofSeconds(3))
        host.events.position(a) { it > millis(3100) }

        val events = host.factory.mediaEvents[0].all
        val seeking = events.indexOfFirst { it is MediaPlayerEvent.Seeking }
        val seeked = events.indexOfFirst { it is MediaPlayerEvent.Seeked }
        assertTrue(seeking in 0 until seeked)
        assertFalse(events.subList(seeking, seeked).any { it is MediaPlayerEvent.Position })
    }

    @Test
    fun seekPastTheEndReportsSeekedThenEnded() = runBlocking {
        val host = host()
        host.player.play(host.seed("a", Tones.tone(2.0)))
        host.events.position(a) { it > millis(200) }

        withTimeout(5000) { host.player.seek(Duration.ofSeconds(10)) }
        host.events.state(null, PlayerState.Completed)

        val events = host.factory.mediaEvents[0].all
        val seeked = events.indexOfFirst { it is MediaPlayerEvent.Seeked }
        val ended = events.indexOfFirst { it is MediaPlayerEvent.Ended }
        assertTrue(seeked in 0 until ended)
    }

    @Test
    fun advancesToTheQueuedNextItemAndCompletes() = runBlocking {
        val host = host()
        host.player.play(host.seed("a", Tones.tone(1.0)))
        host.player.setNext(host.seed("b", Tones.tone(1.0)))

        host.events.state(b, PlayerState.Playing)
        host.events.position(b) { it > millis(200) }
        host.events.state(null, PlayerState.Completed)
        assertEquals(1, host.factory.players.size)
    }

    @Test
    fun nextItemSetAfterCompletionPlays() = runBlocking {
        val host = host()
        host.player.play(host.seed("a", Tones.tone(1.0)))
        host.events.state(null, PlayerState.Completed)

        host.player.setNext(host.seed("b", Tones.tone(1.0)))
        host.events.state(b, PlayerState.Playing)
    }

    @Test
    fun clearedNextItemIsNotPlayed() = runBlocking {
        val host = host()
        host.player.play(host.seed("a", Tones.tone(1.0)))
        host.player.setNext(host.seed("b", Tones.tone(1.0)))
        host.player.setNext(null)

        host.events.state(null, PlayerState.Completed)
        assertFalse(host.events.all.any { it is PlayerEvent.CurrentItemChanged && it.item == b })
    }

    @Test
    fun rejectedNextItemKeepsTheStagedOne() = runBlocking {
        val host = host()
        host.player.play(host.seed("a", Tones.tone(1.0)))
        host.player.setNext(host.seed("b", Tones.tone(1.0)))
        try {
            host.player.setNext(host.seed("c", ""))
            fail("Expected the next item to be rejected")
        } catch (_: PlayerException) {
        }

        host.events.current(b)
        host.events.state(null, PlayerState.Completed)
    }

    @Test
    fun failedNextItemFailsWhenPlaybackReachesIt() = runBlocking {
        val host = host()
        host.player.play(host.seed("a", Tones.tone(1.0)))
        host.player.setNext(host.seed("b", Tones.missing))

        host.events.next {
            when (it) {
                is PlayerEvent.CurrentItemChanged -> it.item == b && it.state is PlayerState.Failed
                is PlayerEvent.StateChanged -> it.item == b && it.state is PlayerState.Failed
                else -> false
            }
        }
        Unit
    }

    @Test
    fun itemThatCannotLoadIsReportedAsFailed() = runBlocking {
        val host = host()
        host.player.play(host.seed("a", Tones.missing))
        host.events.next {
            it is PlayerEvent.StateChanged && it.item == a &&
                (it.state as? PlayerState.Failed)?.v1?.isPlaybackError == true
        }
        Unit
    }

    @Test
    fun playingAnotherItemReplacesTheCurrentOne() = runBlocking {
        val host = host()
        host.player.play(host.seed("a", Tones.tone(5.0)))
        host.events.position(a) { it > millis(200) }

        host.player.play(host.seed("b", Tones.tone(5.0)))
        host.events.state(b, PlayerState.Playing)
        host.events.position(b) { it > millis(200) }
        assertEquals(1, host.mediaItemCount(0))
    }

    @Test
    fun commandCancelledBeforeItRunsDoesNothing() = runBlocking {
        val gate = Gate()
        val host = host(gate = gate)
        val seedA = host.seed("a", Tones.tone(5.0))
        val first = launch(Dispatchers.Default) {
            runCatching { host.player.play(seedA) }
        }
        while (!gate.isHolding) {
            delay(10)
        }

        host.player.play(host.seed("b", Tones.tone(5.0)))
        host.events.state(b, PlayerState.Playing)
        gate.open()
        first.join()
        host.events.position(b) { it > millis(500) }

        assertEquals(1, host.mediaItemCount(0))
        assertFalse(host.events.all.any { it is PlayerEvent.CurrentItemChanged && it.item == a })
    }

    @Test
    fun cancelledResetDoesNothing() = runBlocking {
        val host = host()
        host.player.play(host.seed("a", Tones.tone(5.0)))
        host.events.state(a, PlayerState.Playing)
        val media = host.factory.players[0]
        val gate = Gate()
        val reset = launch(Dispatchers.Default) {
            gate.pass()
            media.reset()
        }
        while (!gate.isHolding) {
            delay(10)
        }
        reset.cancel()
        gate.open()
        reset.join()

        host.events.position(a) { it > millis(500) }
        assertEquals(1, host.mediaItemCount(0))
    }

    @Test
    fun crossfadesToTheNextItem() = runBlocking {
        val host = host(crossfade = millis(500))
        host.player.play(host.seed("a", Tones.tone(1.5)))
        host.player.setNext(host.seed("b", Tones.tone(1.5)))

        assertEquals(PlayerState.Playing, host.events.current(b))
        host.events.state(null, PlayerState.Completed)
        assertEquals(2, host.factory.players.size)
    }

    @Test
    fun playsWidevineProtectedAudioWithAPreloadedLicence() = runBlocking {
        val host = host()
        host.player.play(host.seed("a", Encrypted.audio, Encrypted.widevine))
        host.events.state(a, PlayerState.Playing)
        host.events.position(a) { it > millis(200) }
        Unit
    }

    @Test
    fun releasingThePlayerDisposesItsMediaPlayers() = runBlocking {
        val host = host()
        host.player.play(host.seed("a", Tones.tone(5.0)))
        host.events.state(a, PlayerState.Playing)
        val players = withContext(Dispatchers.Main) { host.factory.players.map { it.player } }

        host.player.close()
        val released = suspend {
            withContext(Dispatchers.Main) { players.all { it.mediaItemCount == 0 && it.playbackState == STATE_IDLE } }
        }
        val deadline = System.nanoTime() + Duration.ofSeconds(5).toNanos()
        while (!released() && System.nanoTime() < deadline) {
            delay(20)
        }
        assertTrue(released())
    }
}

private fun millis(value: Long) = Duration.ofMillis(value)

private suspend fun Host.mediaItemCount(player: Int) =
    withContext(Dispatchers.Main) { factory.players[player].player.mediaItemCount }

private val PlayerException.isPlaybackError: Boolean
    get() = when (this) {
        is PlayerException.Network -> code
        is PlayerException.Other -> code
        is PlayerException.Denied -> code
    }.startsWith("p")
