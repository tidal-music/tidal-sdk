import java.time.Duration
import java.time.Instant
import java.util.concurrent.Semaphore
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.locks.ReentrantLock
import kotlin.concurrent.withLock
import kotlinx.coroutines.runBlocking
import uniffi.player.*

// Playing is reported with the selection or, if the media starts later, as a state change.
private fun PlayerEvent.isPlaying(product: MediaProduct) = when (this) {
    is PlayerEvent.CurrentItemChanged -> item == product && state == PlayerState.Playing
    is PlayerEvent.StateChanged -> item == product && state == PlayerState.Playing
    else -> false
}

private class Host(private val malformed: Boolean = false) : HttpTransport, Authenticator, DrmProvider, TidalEventSender, PlayerEvents, MediaPlayerFactory {
    val released = Semaphore(0)
    private val lock = ReentrantLock()
    private val changed = lock.newCondition()
    private val events = mutableListOf<PlayerEvent>()
    private val licenses = java.util.concurrent.atomic.AtomicInteger()
    // Rejecting the media player's license fetch once makes the core refresh the token,
    // which needs the core's executor rather than the caller's.
    override suspend fun execute(request: HttpRequest) = if (request.url == "https://license.example" && licenses.incrementAndGet() == 2)
        HttpResponse(401u, ByteArray(0))
    else HttpResponse(200u,
        (if (malformed) "invalid json" else
        """{"data":{"id":"42","attributes":{"uri":"https://media.example/track","formats":["FLAC"],"drmData":{"drmSystem":"WIDEVINE","licenseUrl":"https://license.example","certificateUrl":"https://certificate.example","initData":["init"]}}}}""").toByteArray())
    override suspend fun accessToken() = "token"
    override suspend fun refreshedAccessToken() = "token"
    override fun preloadsLicenses() = true
    override fun createSession(): LicenseSession = Session()
    override fun sendEvent(event: ReportingEvent) {}
    override fun onEvent(event: PlayerEvent) = lock.withLock { events.add(event); changed.signalAll() }
    val retained = mutableListOf<Media>()
    override fun create(events: MediaPlayerEvents): MediaPlayer {
        val media = Media(events, released)
        retained.add(media)
        return media
    }
    fun awaitEvent(predicate: (PlayerEvent) -> Boolean): PlayerEvent = lock.withLock {
        val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(5)
        while (true) {
            events.firstOrNull(predicate)?.let { return it }
            val remaining = deadline - System.nanoTime()
            check(remaining > 0) { "event timed out" }
            changed.awaitNanos(remaining)
        }
        error("unreachable")
    }
}

private class Session : LicenseSession {
    override fun certificateRequest(): CertificateRequest = CertificateRequest.Post("request".toByteArray())
    override suspend fun createChallenge(certificate: ByteArray, initData: ByteArray?): ByteArray {
        check(initData?.contentEquals("init".toByteArray()) == true)
        return "challenge".toByteArray()
    }
    override suspend fun installLicense(license: ByteArray) = "handle".toByteArray()
}

private class Media(private val events: MediaPlayerEvents, private val released: Semaphore) : MediaPlayer {
    private val disposed = AtomicBoolean(false)
    val disposeCalls = AtomicInteger(0)
    override fun dispose() {
        disposeCalls.incrementAndGet()
        if (disposed.compareAndSet(false, true)) {
            events.close()
            released.release()
        }
    }
    private fun emit(event: MediaPlayerEvent) {
        events.onEvent(MediaEvent(event, Instant.now()))
    }
    private fun select(item: MediaItem) {
        emit(MediaPlayerEvent.Prepared(item))
        emit(MediaPlayerEvent.Current(item))
        emit(MediaPlayerEvent.Duration(item, Duration.ofMillis(120500)))
        emit(MediaPlayerEvent.Seeking(item, Duration.ZERO))
        emit(MediaPlayerEvent.Seeked(item,
            PlaybackPosition(Duration.ofMillis(1250), Duration.ofMillis(119250))))
    }
    override suspend fun setCurrentAndPlay(item: MediaItem) {
        val drm = item.mediaInput().drm as DrmInput.Prepared
        check(drm.handle.contentEquals("handle".toByteArray()))
        // An engine may still ask for a license; the item fetches it.
        item.fetchCertificate(CertificateRequest.Post("request".toByteArray()))
        item.fetchLicense("challenge".toByteArray())
        select(item)
        emit(MediaPlayerEvent.Started(item, Duration.ofMillis(1250)))
        emit(MediaPlayerEvent.Position(item,
            PlaybackPosition(Duration.ofMillis(1250), Duration.ofMillis(119250))))
        item.close()
    }
    override suspend fun setCurrent(item: MediaItem) { select(item); item.close() }
    override suspend fun setNext(item: MediaItem?) { item?.close() }
    override suspend fun pause() {}
    override suspend fun resume() {}
    override suspend fun seek(position: Duration) {}
    override suspend fun setVolume(gain: Float) {}
    override suspend fun reset() {}
}

fun main() {
    val manifest = ManifestConfiguration(
        TrackManifestConfiguration(ManifestType.HLS, listOf(AudioFormat.FLAC), false, UriScheme.HTTPS, ManifestUsage.PLAYBACK),
        VideoManifestConfiguration(UriScheme.HTTPS, ManifestUsage.PLAYBACK))
    val config = PlayerConfig("https://api.example", manifest, null)
    val host = Host()
    val player = Player(config, host, host, host, host, host, host)
    val seed = player.prepare(MediaProduct.Track("42"))
    runBlocking {
        player.play(seed)
        host.awaitEvent { it.isPlaying(MediaProduct.Track("42")) }
        val position = host.awaitEvent { it is PlayerEvent.PositionChanged && it.position == Duration.ofMillis(1250) } as PlayerEvent.PositionChanged
        check(position.item == MediaProduct.Track("42") && position.duration == Duration.ofMillis(120500))
        player.reset()
        host.awaitEvent { it is PlayerEvent.CurrentItemChanged && it.item == null && it.state == PlayerState.Idle }
    }
    player.close()
    check(host.released.tryAcquire(5, TimeUnit.SECONDS))
    check(host.retained.all { it.disposeCalls.get() == 1 })
    seed.close()

    val failures = Host(malformed = true)
    val failing = Player(config, failures, failures, failures, failures, failures, failures)
    val badSeed = failing.prepare(MediaProduct.Track("bad"))
    runBlocking {
        val returned = runCatching { failing.play(badSeed) }.exceptionOrNull() as PlayerException.Other
        check(!returned.canRetry && returned.code == "mdec")
        val event = failures.awaitEvent { it is PlayerEvent.StateChanged && it.state is PlayerState.Failed } as PlayerEvent.StateChanged
        val error = (event.state as PlayerState.Failed).v1 as PlayerException.Other
        check(event.item == MediaProduct.Track("bad") && !error.canRetry && error.code == "mdec")
    }
    failing.close()
    check(failures.released.tryAcquire(5, TimeUnit.SECONDS))
    badSeed.close()
    println("PASS Kotlin: typed state/position events, failure callbacks, DRM, items and release")
}
