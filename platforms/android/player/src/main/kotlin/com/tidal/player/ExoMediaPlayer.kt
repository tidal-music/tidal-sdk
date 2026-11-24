package com.tidal.player

import android.content.Context
import android.os.Handler
import android.os.Looper
import androidx.annotation.OptIn
import androidx.media3.common.AudioAttributes
import androidx.media3.common.C
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.common.Timeline
import androidx.media3.common.util.UnstableApi
import androidx.media3.exoplayer.ExoPlayer
import java.time.Duration
import java.time.Instant
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.player.MediaEvent
import uniffi.player.MediaItem
import uniffi.player.MediaPlayer
import uniffi.player.MediaPlayerEvent
import uniffi.player.MediaPlayerEvents
import uniffi.player.PlaybackPosition

private const val POSITION_INTERVAL_MS = 100L

@OptIn(UnstableApi::class)
internal class ExoMediaPlayer(
    private val context: Context,
    private val events: MediaPlayerEvents,
    private val registry: LicenseRegistry,
    private val onVideo: (ExoPlayer) -> Unit,
) : MediaPlayer, Player.Listener {
    private val handler = Handler(Looper.getMainLooper())
    private val scope = MainScope()
    private var current: MediaPlayerItem? = null
    private var next: MediaPlayerItem? = null
    private var pendingSeek: PendingSeek? = null
    private var positions: Job? = null
    private var nextId = 0
    private var disposed = false

    val player: ExoPlayer = createPlayer()

    // MARK: MediaPlayer

    override suspend fun setCurrentAndPlay(item: MediaItem) = placing(item) {
        replaceItems(it)
        player.play()
    }

    override suspend fun setCurrent(item: MediaItem) = placing(item) {
        replaceItems(it)
    }

    override suspend fun setNext(item: MediaItem?) {
        if (item == null) {
            return onMain { removeNext() }
        }
        placing(item) {
            if (current == null) {
                return@placing replaceItems(it)
            }
            removeNext()
            next = it
            emit(MediaPlayerEvent.Preparing(it.item))
            player.addMediaSource(it.source)
        }
    }

    override suspend fun pause() = onMain {
        player.pause()
    }

    override suspend fun resume() = onMain {
        player.play()
    }

    override suspend fun seek(position: Duration) = onMain {
        val seeking = current ?: return@onMain
        emit(MediaPlayerEvent.Seeking(seeking.item, position(seeking).position))
        val seek = PendingSeek(seeking)
        pendingSeek?.done?.complete(Unit)
        pendingSeek = seek
        player.seekTo(position.toMillis())
        seek.done.await()
    }

    override suspend fun setVolume(gain: Float) = onMain {
        player.volume = gain
    }

    override suspend fun reset() = onMain {
        releaseItems()
        player.pause()
    }

    override fun dispose() {
        handler.post(::tearDown)
    }

    // MARK: Placement

    private suspend fun <T> onMain(block: suspend CoroutineScope.() -> T): T =
        withContext(Dispatchers.Main.immediate) {
            check(!disposed) { "The media player is disposed" }
            block()
        }

    private suspend fun placing(item: MediaItem, place: (MediaPlayerItem) -> Unit) {
        var placed = false
        try {
            onMain {
                place(MediaPlayerItem(item, "${nextId++}", context, registry))
                placed = true
            }
        } finally {
            if (!placed) {
                item.close()
            }
        }
    }

    private fun createPlayer(): ExoPlayer {
        val audioAttributes = AudioAttributes.Builder()
            .setUsage(C.USAGE_MEDIA)
            .setContentType(C.AUDIO_CONTENT_TYPE_MUSIC)
            .build()
        return ExoPlayer.Builder(context)
            .setAudioAttributes(audioAttributes, false)
            .setWakeMode(C.WAKE_MODE_NETWORK)
            .setLooper(Looper.getMainLooper())
            .setPlaybackLooper(registry.looper)
            .build()
            .also { it.addListener(this) }
    }

    private fun replaceItems(item: MediaPlayerItem) {
        releaseItems()
        current = item
        emit(MediaPlayerEvent.Preparing(item.item))
        emit(MediaPlayerEvent.Current(item.item))
        player.volume = item.item.volume()
        player.setMediaSource(item.source)
        player.prepare()
    }

    private fun removeNext() {
        val next = next ?: return
        this.next = null
        player.removeMediaItems(1, player.mediaItemCount)
        next.release()
    }

    private fun releaseItems() {
        pendingSeek?.done?.complete(Unit)
        pendingSeek = null
        val released = listOfNotNull(current, next)
        current = null
        next = null
        player.clearMediaItems()
        released.forEach(MediaPlayerItem::release)
    }

    private fun tearDown() {
        if (disposed) {
            return
        }
        disposed = true
        scope.cancel()
        releaseItems()
        player.release()
        events.close()
    }

    // MARK: Player.Listener

    override fun onTimelineChanged(timeline: Timeline, reason: Int) {
        val window = Timeline.Window()
        for (index in 0 until timeline.windowCount) {
            timeline.getWindow(index, window)
            val item = held(window.mediaItem.mediaId) ?: continue
            val duration = window.durationMs.takeIf { it != C.TIME_UNSET }?.let(Duration::ofMillis)
            if (duration != null && duration != item.duration) {
                item.duration = duration
                emit(MediaPlayerEvent.Duration(item.item, duration))
            }
        }
    }

    override fun onMediaItemTransition(mediaItem: androidx.media3.common.MediaItem?, reason: Int) {
        val current = current ?: return
        val next = next
        if (reason != Player.MEDIA_ITEM_TRANSITION_REASON_AUTO || next == null || next.id != mediaItem?.mediaId) {
            return
        }
        emit(MediaPlayerEvent.Ended(current.item, current.duration ?: Duration.ZERO))
        this.current = next
        this.next = null
        player.removeMediaItem(0)
        current.release()
        emit(MediaPlayerEvent.Current(next.item))
        player.volume = next.item.volume()
        if (player.isPlaying) {
            onItemStarted(next)
        }
    }

    override fun onPlaybackStateChanged(playbackState: Int) {
        when (playbackState) {
            Player.STATE_READY -> {
                completeSeek()
                onReady()
            }
            Player.STATE_BUFFERING -> onBuffering()
            Player.STATE_ENDED -> {
                completeSeek()
                onEnded()
            }
        }
    }

    override fun onPlayWhenReadyChanged(playWhenReady: Boolean, reason: Int) {
        val current = current ?: return
        if (!playWhenReady) {
            emit(MediaPlayerEvent.Paused(current.item, position(current)))
            return
        }
        if (player.playbackState == Player.STATE_BUFFERING) {
            onBuffering()
        }
    }

    override fun onIsPlayingChanged(isPlaying: Boolean) {
        positions?.cancel()
        positions = null
        if (!isPlaying) {
            return
        }
        positions = scope.launch {
            while (true) {
                delay(POSITION_INTERVAL_MS)
                onPositionChanged()
            }
        }
        val current = current ?: return
        if (current.started) {
            emit(MediaPlayerEvent.Resumed(current.item, position(current)))
        } else {
            onItemStarted(current)
        }
    }

    override fun onPlayerError(error: PlaybackException) {
        val current = current ?: return
        emit(MediaPlayerEvent.Failed(current.item, position(current).position, error.toMediaError()))
        releaseItems()
    }

    // MARK: Player observations

    private fun completeSeek() {
        val seek = pendingSeek ?: return
        pendingSeek = null
        if (seek.item === current) {
            emit(MediaPlayerEvent.Seeked(seek.item.item, position(seek.item)))
        }
        seek.done.complete(Unit)
    }

    private fun onReady() {
        val current = current ?: return
        if (!current.prepared) {
            current.prepared = true
            emit(MediaPlayerEvent.Prepared(current.item))
        }
    }

    private fun onBuffering() {
        val current = current ?: return
        if (current.started && player.playWhenReady) {
            emit(MediaPlayerEvent.Stalled(current.item, position(current)))
        }
    }

    private fun onEnded() {
        val current = current ?: return
        emit(MediaPlayerEvent.Ended(current.item, current.duration ?: position(current).position))
        this.current = null
        current.release()
        emit(MediaPlayerEvent.Completed)
    }

    private fun onPositionChanged() {
        val current = current ?: return
        if (pendingSeek == null) {
            emit(MediaPlayerEvent.Position(current.item, position(current)))
        }
    }

    private fun onItemStarted(item: MediaPlayerItem) {
        item.started = true
        emit(MediaPlayerEvent.Started(item.item, position(item).position))
        if (player.currentTracks.containsType(C.TRACK_TYPE_VIDEO)) {
            onVideo(player)
        }
    }

    // MARK: Helpers

    private class PendingSeek(val item: MediaPlayerItem) {
        val done = CompletableDeferred<Unit>()
    }

    private fun held(id: String): MediaPlayerItem? =
        listOfNotNull(current, next).firstOrNull { it.id == id }

    private fun position(item: MediaPlayerItem): PlaybackPosition {
        val position = Duration.ofMillis(player.currentPosition.coerceAtLeast(0))
        val remaining = item.duration?.minus(position)?.takeUnless(Duration::isNegative) ?: Duration.ZERO
        return PlaybackPosition(position, remaining)
    }

    private fun emit(event: MediaPlayerEvent) {
        events.onEvent(MediaEvent(event, Instant.now()))
    }
}
