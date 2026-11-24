package com.tidal.player

import android.content.Context
import androidx.media3.exoplayer.ExoPlayer
import uniffi.player.MediaPlayer
import uniffi.player.MediaPlayerEvents
import uniffi.player.MediaPlayerFactory

class ExoMediaPlayerFactory(
    context: Context,
    drmProvider: WidevineDrmProvider,
    private val onVideo: (ExoPlayer) -> Unit = {},
) : MediaPlayerFactory {
    private val context = context.applicationContext
    private val registry = drmProvider.registry

    override fun create(events: MediaPlayerEvents): MediaPlayer = ExoMediaPlayer(context, events, registry, onVideo)
}
