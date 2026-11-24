# Android player

The `player` library in `platforms/android/player` bundles the `player` crate's
generated Kotlin bindings (`uniffi.player`) and shared libraries with Media3
implementations of its platform ports:

- `OkHttpTransport` implements `HttpTransport` with the `OkHttpClient` the host passes
  in. The client owns the HTTP cache and the connect, read, write and call timeouts.
- `WidevineDrmProvider` implements `DrmProvider` with licence preloading, so the
  keys are installed before the item reaches a media player.
- `ExoMediaPlayerFactory` implements `MediaPlayerFactory`. Each media player owns
  one `ExoPlayer`. In queued playback the core's next item is the second item in
  the ExoPlayer playlist, so ExoPlayer advances to it without a gap. With
  `PlayerConfig.crossfade` the core uses two media players instead and fades them
  with `setVolume`.

Ordering, advancement, crossfade, retries and the HTTP for manifests and licences
stay in Rust. The adapters only report media facts and execute commands; see the
[player API guide](../../crates/player/README.md) for the contract.

ExoPlayer reports a next item that cannot load only when playback reaches it.
The item first becomes current and then fails, instead of failing while it is
staged.

## Development

You need the Android SDK with NDK 28.2.13676358, Rust through rustup and
`cargo-ndk` (`cargo install cargo-ndk`).

```sh
tools/build-player-android.sh
cd platforms/android/player
./gradlew connectedDebugAndroidTest
```

The script builds the `player` crate for `arm64-v8a` and `x86_64` and writes the
shared libraries and the generated Kotlin bindings to the ignored `generated`
directory; rerun it after changing the crate.

## Use

```kotlin
val events = object : PlayerEvents {
    override fun onEvent(event: PlayerEvent) {
        if (event is PlayerEvent.StateChanged) Log.d("Player", "${event.item}: ${event.state}")
    }
}
val config = PlayerConfig(
    baseUrl = "https://openapi.tidal.com/v2",
    manifestConfiguration = ManifestConfiguration(
        TrackManifestConfiguration(
            manifestType = ManifestType.MPEG_DASH,
            audioFormats = listOf(AudioFormat.AAC_LC),
            adaptive = false,
            uriScheme = UriScheme.HTTPS,
            usage = ManifestUsage.PLAYBACK,
        ),
        VideoManifestConfiguration(UriScheme.HTTPS, ManifestUsage.PLAYBACK),
    ),
    crossfade = null,
)
val http = OkHttpClient.Builder()
    .cache(Cache(File(context.cacheDir, "player-http"), 10L * 1024 * 1024))
    .connectTimeout(10, TimeUnit.SECONDS)
    .readTimeout(30, TimeUnit.SECONDS)
    .build()
val drm = WidevineDrmProvider()
val player = Player(
    config,
    OkHttpTransport(http),
    authenticator,
    ExoMediaPlayerFactory(context, drm) { exoPlayer -> playerView.player = exoPlayer },
    drm,
    eventSender,
    events,
)

scope.launch {
    player.play(player.prepare(MediaProduct.Track("1")))
    player.setNext(player.prepare(MediaProduct.Track("2")))
    player.seek(Duration.ofSeconds(30))
    player.pause()
}
```

Pass the same `WidevineDrmProvider` to the factory and the Player, so media
players find the licences it preloaded. The factory's callback receives a media
player's `ExoPlayer` when it starts rendering video, for the host's `PlayerView` or surface. Media
players set media audio attributes and a network wake lock, but leave audio
focus, becoming-noisy handling and the media session to the host. Crossfade runs
two players, which would take focus from each other. Closing the Player disposes
the media players it created.

The library needs `minSdk` 26 for the bindings' `java.time` types, and keeps JNA
and the bindings from R8 through its consumer rules.

## Checks

`./gradlew connectedDebugAndroidTest` runs the real core through the adapters on a
device or emulator with generated WAV tones, covering playback commands, gapless
advancement, failures, late commands, crossfade and release. One test plays
Widevine-protected DASH audio from Google's public test proxy and needs network
access. The checks do not cover HLS, network stalls or video.
