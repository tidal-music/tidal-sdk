# iOS player

The `PlayerRS` product bundles the `player` crate's generated Swift bindings
(`Player.swift`) with AVFoundation implementations of its platform ports:

- `URLSessionHttpTransport` implements `HttpTransport` with URLSession. Requests time
  out after 30 seconds.
- `FairPlayDrmProvider` implements `DrmProvider` and preloads licences into an
  `AVContentKeySession`, so the key is installed before the item reaches a media
  player. Only the preloaded key is supported.
- `AVQueueMediaPlayerFactory` implements `MediaPlayerFactory`. Each media player owns
  one `AVQueuePlayer`. In queued playback the core's next item waits in the player's
  queue, so AVFoundation advances to it without a gap. With `PlayerConfig.crossfade`
  the core uses two media players instead and fades them with `setVolume`.

Ordering, advancement, crossfade, retries and the HTTP for manifests, certificates
and licences stay in Rust. The adapters only report media facts and execute
commands; see the [player API guide](../../crates/player/README.md) for the contract.

## Development

You need Xcode with the iOS 18 SDK or later and Rust through rustup.

```sh
tools/build-player-xcframework.sh
swift test
```

The script builds the `player` crate for device and simulator and writes the
XCFramework and the generated `Player.swift`. Both are ignored; rerun the script after
changing the crate.

## Use

```swift
import PlayerRS

final class Events: PlayerEvents {
	func onEvent(event: PlayerEvent) {
		if case .stateChanged(let item, let state) = event {
			print(item as Any, state)
		}
	}
}

let config = PlayerConfig(
	baseUrl: "https://openapi.tidal.com/v2",
	manifestConfiguration: ManifestConfiguration(
		trackConfiguration: TrackManifestConfiguration(
			manifestType: .hls, audioFormats: [.aacLc], adaptive: false, uriScheme: .https,
			usage: .playback
		),
		videoConfiguration: VideoManifestConfiguration(uriScheme: .https, usage: .playback)
	),
	crossfade: nil
)
let drm = FairPlayDrmProvider()
let media = AVQueueMediaPlayerFactory(drmProvider: drm) { player in
	playerLayer.player = player
}
let player = Player(
	config: config,
	transport: URLSessionHttpTransport(),
	authenticator: authenticator,
	mediaFactory: media,
	drmProvider: drm,
	eventSender: eventSender,
	events: Events()
)

try await player.play(seed: player.prepare(product: .track(id: "1")))
try await player.setNext(seed: player.prepare(product: .track(id: "2")))
try await player.seek(position: 30)
await player.pause()
```

Pass the same `FairPlayDrmProvider` to the factory and the Player, so media players
find the keys the provider preloaded. The factory's callback receives a media player's
`AVPlayer` when it starts rendering video, for the host's `AVPlayerLayer`. The host
configures `AVAudioSession`. Releasing the Player disposes the media players it created.

## Checks

`swift test` runs the real core through the adapters on macOS with local AAC files,
covering playback commands, gapless advancement, failures, late commands, crossfade
and release. It does not cover FairPlay, HLS, network stalls or video.
