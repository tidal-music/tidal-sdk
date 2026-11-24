package com.tidal.player

import androidx.media3.common.PlaybackException
import uniffi.player.MediaException

internal fun PlaybackException.toMediaError(): MediaException {
    val causes = generateSequence<Throwable>(this) { it.cause }
    val code = errorCode.toString()
    val description = "$errorCodeName: ${causes.joinToString(" <- ")}"
    return when {
        errorCode in networkErrors -> MediaException.Network(code, description)
        errorCode in contentErrors -> MediaException.Content(code, description)
        else -> MediaException.Other(code, description)
    }
}

private val networkErrors = setOf(
    PlaybackException.ERROR_CODE_IO_NETWORK_CONNECTION_FAILED,
    PlaybackException.ERROR_CODE_IO_NETWORK_CONNECTION_TIMEOUT,
)

private val contentErrors = setOf(
    PlaybackException.ERROR_CODE_IO_INVALID_HTTP_CONTENT_TYPE,
    PlaybackException.ERROR_CODE_PARSING_CONTAINER_MALFORMED,
    PlaybackException.ERROR_CODE_PARSING_MANIFEST_MALFORMED,
    PlaybackException.ERROR_CODE_PARSING_CONTAINER_UNSUPPORTED,
    PlaybackException.ERROR_CODE_PARSING_MANIFEST_UNSUPPORTED,
    PlaybackException.ERROR_CODE_DECODING_FAILED,
    PlaybackException.ERROR_CODE_DECODING_FORMAT_EXCEEDS_CAPABILITIES,
    PlaybackException.ERROR_CODE_DECODING_FORMAT_UNSUPPORTED,
    PlaybackException.ERROR_CODE_DRM_CONTENT_ERROR,
    PlaybackException.ERROR_CODE_DRM_LICENSE_EXPIRED,
)
