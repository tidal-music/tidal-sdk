package com.tidal.player

import android.content.Context
import android.net.Uri
import androidx.annotation.OptIn
import androidx.media3.common.MimeTypes
import androidx.media3.common.util.UnstableApi
import androidx.media3.exoplayer.source.DefaultMediaSourceFactory
import androidx.media3.exoplayer.source.MediaSource
import java.time.Duration
import uniffi.player.DrmInput
import uniffi.player.MediaException
import uniffi.player.MediaItem
import androidx.media3.common.MediaItem as Media3Item

@OptIn(UnstableApi::class)
internal class MediaPlayerItem(
    val item: MediaItem,
    val id: String,
    context: Context,
    registry: LicenseRegistry,
) {
    val source: MediaSource
    var prepared = false
    var started = false
    var duration: Duration? = null

    init {
        val input = item.mediaInput()
        val uri = Uri.parse(input.url)
        if (uri.scheme == null) {
            throw MediaException.Content("URL", "Invalid media URL: ${input.url}")
        }
        val mediaItem = Media3Item.Builder().setMediaId(id).setUri(uri).setMimeType(mimeType(input.url)).build()
        val factory = DefaultMediaSourceFactory(context)
        when (val drm = input.drm) {
            null -> Unit
            is DrmInput.Prepared -> {
                val manager = registry.manager(drm.handle)
                    ?: throw MediaException.Other("DRM", "The prepared licence is no longer available")
                factory.setDrmSessionManagerProvider { manager }
            }
            DrmInput.OnDemand -> throw MediaException.Other("DRM", "Widevine licences must be preloaded")
        }
        source = factory.createMediaSource(mediaItem)
    }

    fun release() {
        item.close()
    }
}

private fun mimeType(url: String): String? {
    if (!url.startsWith("data:")) {
        return null
    }
    return when (val type = url.removePrefix("data:").substringBefore(',').substringBefore(';')) {
        "application/vnd.apple.mpegurl" -> MimeTypes.APPLICATION_M3U8
        else -> type
    }
}
