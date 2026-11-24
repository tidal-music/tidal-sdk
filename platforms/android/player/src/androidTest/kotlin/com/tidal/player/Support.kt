package com.tidal.player

import android.net.Uri
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.time.Duration
import kotlin.math.PI
import kotlin.math.sin
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import okhttp3.OkHttpClient
import org.junit.Assert.fail
import uniffi.player.Authenticator
import uniffi.player.AudioFormat
import uniffi.player.CrossfadeConfig
import uniffi.player.HttpRequest
import uniffi.player.HttpResponse
import uniffi.player.HttpTransport
import uniffi.player.ManifestConfiguration
import uniffi.player.ManifestType
import uniffi.player.ManifestUsage
import uniffi.player.MediaEvent
import uniffi.player.MediaItem
import uniffi.player.MediaPlayer
import uniffi.player.MediaPlayerEvent
import uniffi.player.MediaPlayerEvents
import uniffi.player.MediaPlayerFactory
import uniffi.player.MediaProduct
import uniffi.player.NoHandle
import uniffi.player.PlaybackSeed
import uniffi.player.Player
import uniffi.player.PlayerConfig
import uniffi.player.PlayerEvent
import uniffi.player.PlayerEvents
import uniffi.player.PlayerState
import uniffi.player.ReportingEvent
import uniffi.player.TidalEventSender
import uniffi.player.TrackManifestConfiguration
import uniffi.player.UriScheme
import uniffi.player.VideoManifestConfiguration

class Manifests(private val network: HttpTransport) : HttpTransport {
    private val media = mutableMapOf<String, String>()

    fun add(id: String, uri: String, drm: Widevine? = null) = synchronized(media) {
        media[id] = """{"uri":"$uri","formats":["AACLC"]${drm?.let { ",\"drmData\":${it.json}" } ?: ""}}"""
    }

    override suspend fun execute(request: HttpRequest): HttpResponse {
        val url = Uri.parse(request.url)
        if (url.host != API_HOST) {
            return network.execute(request)
        }
        val attributes = synchronized(media) { media[url.lastPathSegment] } ?: return HttpResponse(404u, ByteArray(0))
        val body = """{"data":{"id":"${url.lastPathSegment}","attributes":$attributes}}"""
        return HttpResponse(200u, body.toByteArray())
    }
}

class Widevine(licenseUrl: String, pssh: String) {
    val json = """{"drmSystem":"WIDEVINE","licenseUrl":"$licenseUrl","certificateUrl":"$licenseUrl","initData":["$pssh"]}"""
}

class Credentials : Authenticator {
    override suspend fun accessToken() = "token"
    override suspend fun refreshedAccessToken() = "token"
}

class Telemetry : TidalEventSender {
    override fun sendEvent(event: ReportingEvent) {}
}

class Events : PlayerEvents {
    private val events = mutableListOf<PlayerEvent>()
    private val received = mutableListOf<PlayerEvent>()

    override fun onEvent(event: PlayerEvent) = synchronized(events) {
        events.add(event)
        received.add(event)
        Unit
    }

    val all: List<PlayerEvent>
        get() = synchronized(events) { received.toList() }

    suspend fun next(timeout: Duration = Duration.ofSeconds(10), predicate: (PlayerEvent) -> Boolean): PlayerEvent {
        val deadline = System.nanoTime() + timeout.toNanos()
        while (System.nanoTime() < deadline) {
            val found = synchronized(events) {
                val index = events.indexOfFirst(predicate)
                if (index < 0) {
                    return@synchronized null
                }
                events[index].also { repeat(index + 1) { events.removeAt(0) } }
            }
            if (found != null) {
                return found
            }
            delay(20)
        }
        fail("Timed out waiting for an event; received $all")
        error("unreachable")
    }

    suspend fun state(product: MediaProduct?, state: PlayerState) {
        next {
            when (it) {
                is PlayerEvent.CurrentItemChanged -> it.item == product && it.state == state
                is PlayerEvent.StateChanged -> it.item == product && it.state == state
                else -> false
            }
        }
    }

    suspend fun current(product: MediaProduct?): PlayerState =
        (next { it is PlayerEvent.CurrentItemChanged && it.item == product } as PlayerEvent.CurrentItemChanged).state

    suspend fun position(product: MediaProduct, matches: (Duration) -> Boolean) {
        next { it is PlayerEvent.PositionChanged && it.item == product && matches(it.position) }
    }
}

internal class RecordingFactory(private val factory: ExoMediaPlayerFactory, private val gate: Gate?) : MediaPlayerFactory {
    private val created = mutableListOf<ExoMediaPlayer>()
    private val recorded = mutableListOf<RecordedMediaEvents>()

    val players: List<ExoMediaPlayer>
        get() = synchronized(created) { created.toList() }

    val mediaEvents: List<RecordedMediaEvents>
        get() = synchronized(created) { recorded.toList() }

    override fun create(events: MediaPlayerEvents): MediaPlayer {
        val recording = RecordedMediaEvents(events)
        val player = factory.create(recording) as ExoMediaPlayer
        synchronized(created) {
            created.add(player)
            recorded.add(recording)
        }
        return gate?.let { GatedMediaPlayer(player, it) } ?: player
    }
}

class RecordedMediaEvents(private val events: MediaPlayerEvents) : MediaPlayerEvents(NoHandle) {
    private val recorded = mutableListOf<MediaPlayerEvent>()

    val all: List<MediaPlayerEvent>
        get() = synchronized(recorded) { recorded.toList() }

    override fun onEvent(event: MediaEvent) {
        synchronized(recorded) { recorded.add(event.event) }
        events.onEvent(event)
    }

    override fun close() = events.close()
}

class Gate {
    private val held = CompletableDeferred<Unit>()
    private val opened = CompletableDeferred<Unit>()

    val isHolding: Boolean
        get() = held.isCompleted && !opened.isCompleted

    suspend fun pass() = withContext(NonCancellable) {
        if (held.complete(Unit)) {
            opened.await()
        }
    }

    fun open() {
        opened.complete(Unit)
    }
}

internal class GatedMediaPlayer(private val player: ExoMediaPlayer, private val gate: Gate) : MediaPlayer by player {
    override suspend fun setCurrentAndPlay(item: MediaItem) {
        gate.pass()
        player.setCurrentAndPlay(item)
    }
}

internal class Host(crossfade: Duration? = null, gate: Gate? = null) {
    val events = Events()
    private val manifests = Manifests(OkHttpTransport(OkHttpClient()))
    private val drm = WidevineDrmProvider()
    val factory = RecordingFactory(ExoMediaPlayerFactory(context, drm), gate)
    val player = Player(
        PlayerConfig(
            baseUrl = "https://$API_HOST",
            manifestConfiguration = ManifestConfiguration(
                TrackManifestConfiguration(
                    manifestType = ManifestType.MPEG_DASH,
                    audioFormats = listOf(AudioFormat.AAC_LC),
                    adaptive = false,
                    uriScheme = UriScheme.HTTPS,
                    usage = ManifestUsage.PLAYBACK,
                ),
                VideoManifestConfiguration(uriScheme = UriScheme.HTTPS, usage = ManifestUsage.PLAYBACK),
            ),
            crossfade = crossfade?.let(::CrossfadeConfig),
        ),
        manifests,
        Credentials(),
        factory,
        drm,
        Telemetry(),
        events,
    )

    fun seed(id: String, uri: String, drm: Widevine? = null): PlaybackSeed {
        manifests.add(id, uri, drm)
        return player.prepare(MediaProduct.Track(id))
    }
}

private const val API_HOST = "api.test"

val context = InstrumentationRegistry.getInstrumentation().targetContext!!

private val directory = File(context.cacheDir, "player-tests").apply { mkdirs() }

object Tones {
    private const val SAMPLE_RATE = 44100

    val missing: String = Uri.fromFile(File(directory, "missing.wav")).toString()

    fun tone(seconds: Double): String {
        val file = File(directory, "tone-$seconds.wav")
        if (!file.exists()) {
            file.writeBytes(wav((seconds * SAMPLE_RATE).toInt()))
        }
        return Uri.fromFile(file).toString()
    }

    private fun wav(frames: Int): ByteArray {
        val data = frames * 2
        return ByteBuffer.allocate(44 + data).order(ByteOrder.LITTLE_ENDIAN).apply {
            put("RIFF".toByteArray()).putInt(36 + data).put("WAVE".toByteArray())
            put("fmt ".toByteArray()).putInt(16).putShort(1).putShort(1)
            putInt(SAMPLE_RATE).putInt(SAMPLE_RATE * 2).putShort(2).putShort(16)
            put("data".toByteArray()).putInt(data)
            repeat(frames) { frame ->
                putShort((300 * sin(2 * PI * 440 * frame / SAMPLE_RATE)).toInt().toShort())
            }
        }.array()
    }
}

object Encrypted {
    private const val MEDIA = "https://storage.googleapis.com/wvmedia/cenc/h264/tears/tears_audio_eng.mp4"
    private const val PSSH = "AAAARHBzc2gAAAAA7e+LqXnWSs6jyCfc1R0h7QAAACQIARIBNRoNd2lkZXZpbmVfdGVzdCIKMjAxNV90ZWFycyoCU0Q="

    val widevine = Widevine("https://proxy.uat.widevine.com/proxy?provider=widevine_test", PSSH)

    val audio: String by lazy {
        val file = File(directory, "tears.mpd")
        file.writeText(
            """
            <?xml version="1.0" encoding="UTF-8"?>
            <MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" profiles="urn:mpeg:dash:profile:isoff-on-demand:2011" mediaPresentationDuration="PT734S" minBufferTime="PT2S">
              <Period id="0">
                <AdaptationSet id="0" contentType="audio">
                  <ContentProtection schemeIdUri="urn:mpeg:dash:mp4protection:2011" value="cenc"/>
                  <ContentProtection schemeIdUri="urn:uuid:edef8ba9-79d6-4ace-a3c8-27dcd51d21ed"/>
                  <Representation id="0" bandwidth="134359" codecs="mp4a.40.2" mimeType="audio/mp4" audioSamplingRate="44100">
                    <BaseURL>$MEDIA</BaseURL>
                    <SegmentBase indexRange="974-1893"><Initialization range="0-973"/></SegmentBase>
                  </Representation>
                </AdaptationSet>
              </Period>
            </MPD>
            """.trimIndent(),
        )
        Uri.fromFile(file).toString()
    }
}
