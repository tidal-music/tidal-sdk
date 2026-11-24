package com.tidal.player

import android.os.Handler
import android.os.HandlerThread
import android.os.Looper
import android.os.Process
import android.util.Base64
import androidx.annotation.OptIn
import androidx.media3.common.C
import androidx.media3.common.DrmInitData
import androidx.media3.common.Format
import androidx.media3.common.MimeTypes
import androidx.media3.common.util.UnstableApi
import androidx.media3.datasource.DefaultHttpDataSource
import androidx.media3.exoplayer.analytics.PlayerId
import androidx.media3.exoplayer.drm.DefaultDrmSessionManager
import androidx.media3.exoplayer.drm.DrmSession
import androidx.media3.exoplayer.drm.DrmSessionEventListener
import androidx.media3.exoplayer.drm.DrmSessionManager
import androidx.media3.exoplayer.drm.ExoMediaDrm
import androidx.media3.exoplayer.drm.FrameworkMediaDrm
import androidx.media3.exoplayer.drm.HttpMediaDrmCallback
import androidx.media3.exoplayer.drm.MediaDrmCallback
import androidx.media3.exoplayer.source.MediaSource
import androidx.media3.exoplayer.upstream.DefaultLoadErrorHandlingPolicy
import androidx.media3.extractor.mp4.PsshAtomUtil
import java.lang.ref.WeakReference
import java.util.UUID
import kotlin.coroutines.cancellation.CancellationException
import kotlin.time.Duration.Companion.minutes
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.android.asCoroutineDispatcher
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import uniffi.player.CertificateRequest
import uniffi.player.DrmException
import uniffi.player.DrmProvider
import uniffi.player.LicenseSession

private val SERVICE_CERTIFICATE_REQUEST = byteArrayOf(0x08, 0x04)
private val LICENSE_TIMEOUT = 2.minutes

private val playbackLooper: Looper by lazy {
    HandlerThread("TidalPlayerPlayback", Process.THREAD_PRIORITY_AUDIO).apply { start() }.looper
}

class WidevineDrmProvider : DrmProvider {
    internal val registry = LicenseRegistry()

    override fun preloadsLicenses() = true

    override fun createSession(): LicenseSession = WidevineLicenseSession(registry)
}

@OptIn(UnstableApi::class)
internal class WidevineLicenseSession(private val registry: LicenseRegistry) : LicenseSession {
    private val handle = UUID.randomUUID().toString()
    private val exchange = KeyExchange()

    override fun certificateRequest() = CertificateRequest.Post(SERVICE_CERTIFICATE_REQUEST)

    override suspend fun createChallenge(certificate: ByteArray, initData: ByteArray?): ByteArray = drmCall {
        val pssh = initData?.let(::widevinePssh)
            ?: throw DrmException.Failed("Widevine content has no init data", null)
        val serviceCertificate = signedCertificate(certificate)
        val manager = DefaultDrmSessionManager.Builder()
            .setUuidAndExoMediaDrmProvider(C.WIDEVINE_UUID) { uuid ->
                FrameworkMediaDrm.newInstance(uuid).apply {
                    setPropertyString("privacyMode", "enable")
                    setPropertyByteArray("serviceCertificate", serviceCertificate)
                }
            }
            .setMultiSession(false)
            .setLoadErrorHandlingPolicy(DefaultLoadErrorHandlingPolicy(0))
            .build(exchange)

        val format = Format.Builder()
            .setDrmInitData(DrmInitData(DrmInitData.SchemeData(C.WIDEVINE_UUID, MimeTypes.VIDEO_MP4, pssh)))
            .build()

        withContext(registry.dispatcher) {
            manager.setPlayer(registry.looper, PlayerId.UNSET)
            manager.prepare()
            val events = DrmSessionEventListener.EventDispatcher().withParameters(0, null)
            events.addEventListener(registry.handler, exchange)
            val session = checkNotNull(manager.acquireSession(events, format))
            registry.register(handle, this@WidevineLicenseSession, manager, session)
        }
        exchange.challenge.await()
    }

    override suspend fun installLicense(license: ByteArray): ByteArray = drmCall {
        exchange.license.complete(license)
        exchange.keysLoaded.await()
        handle.toByteArray()
    }
}

@OptIn(UnstableApi::class)
internal class LicenseRegistry {
    private class Entry(
        val session: WeakReference<WidevineLicenseSession>,
        val manager: DrmSessionManager,
        val drmSession: DrmSession,
    )

    val looper: Looper = playbackLooper
    val handler = Handler(looper)
    val dispatcher = handler.asCoroutineDispatcher()
    private val entries = mutableMapOf<String, Entry>()

    fun register(handle: String, session: WidevineLicenseSession, manager: DrmSessionManager, drmSession: DrmSession) {
        synchronized(entries) {
            entries.values.removeAll { entry -> (entry.session.get() == null).also { if (it) release(entry) } }
            entries[handle] = Entry(WeakReference(session), manager, drmSession)
        }
    }

    fun manager(handle: ByteArray): DrmSessionManager? =
        synchronized(entries) { entries[String(handle)]?.manager }

    private fun release(entry: Entry) {
        handler.post {
            entry.drmSession.release(null)
            entry.manager.release()
        }
    }
}

@OptIn(UnstableApi::class)
private class KeyExchange : MediaDrmCallback, DrmSessionEventListener {
    val challenge = CompletableDeferred<ByteArray>()
    val license = CompletableDeferred<ByteArray>()
    val keysLoaded = CompletableDeferred<Unit>()
    private val provisioning = HttpMediaDrmCallback(null, DefaultHttpDataSource.Factory())

    override fun executeProvisionRequest(uuid: UUID, request: ExoMediaDrm.ProvisionRequest) =
        provisioning.executeProvisionRequest(uuid, request)

    override fun executeKeyRequest(uuid: UUID, request: ExoMediaDrm.KeyRequest): MediaDrmCallback.Response {
        check(challenge.complete(request.data)) { "Only the preloaded Widevine licence is supported" }
        val license = runBlocking { withTimeout(LICENSE_TIMEOUT) { license.await() } }
        return MediaDrmCallback.Response.Builder(license).build()
    }

    override fun onDrmKeysLoaded(windowIndex: Int, mediaPeriodId: MediaSource.MediaPeriodId?) {
        keysLoaded.complete(Unit)
    }

    override fun onDrmSessionManagerError(windowIndex: Int, mediaPeriodId: MediaSource.MediaPeriodId?, error: Exception) {
        listOf(challenge, license, keysLoaded).forEach { it.completeExceptionally(error) }
    }
}

private suspend fun <T> drmCall(block: suspend () -> T): T = try {
    block()
} catch (exception: DrmException) {
    throw exception
} catch (exception: CancellationException) {
    throw exception
} catch (exception: Exception) {
    val code = (exception as? DrmSession.DrmSessionException)?.errorCode?.toString()
    throw DrmException.Failed(exception.toString(), code)
}

@OptIn(UnstableApi::class)
private fun widevinePssh(initData: ByteArray): ByteArray {
    val data = Base64.decode(initData, Base64.DEFAULT)
    return if (PsshAtomUtil.parseUuid(data) == null) PsshAtomUtil.buildPsshAtom(C.WIDEVINE_UUID, data) else data
}

private fun signedCertificate(response: ByteArray): ByteArray {
    var offset = 0
    fun varint(): Long {
        var value = 0L
        var shift = 0
        while (offset < response.size) {
            val byte = response[offset++].toInt() and 0xff
            value = value or ((byte and 0x7f).toLong() shl shift)
            if (byte < 0x80) {
                return value
            }
            shift += 7
        }
        throw DrmException.Failed("Truncated Widevine service certificate", null)
    }
    while (offset < response.size) {
        val key = varint()
        when ((key and 7).toInt()) {
            0 -> varint()
            1 -> offset += 8
            2 -> {
                val length = varint().toInt()
                if (key shr 3 == 2L) {
                    return response.copyOfRange(offset, offset + length)
                }
                offset += length
            }
            5 -> offset += 4
            else -> break
        }
    }
    throw DrmException.Failed("Widevine service certificate response has no certificate", null)
}
