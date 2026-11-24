# Browser player

`@tidal/player-web` bundles the `player` crate's generated TypeScript/Wasm bindings
with browser implementations of its platform ports:

- `FetchHttpTransport` implements `HttpTransport` with Fetch and abort signals. A
  request that takes longer than 5 seconds, body included, fails with `TimeoutError`,
  which the core retries.
- `ShakaMediaPlayerFactory` implements `MediaPlayerFactory`. Its media players play
  one item at a time and reject `setNext`. A short network outage does not fail the
  item.

The core sequences items across two media players with its crossfade controller,
so hosts must set `PlayerConfig.crossfade`; an overlap of zero switches to the
preloaded item without fading. Ordering, advancement and crossfade stay in Rust.
The adapters only report media facts and execute commands; see the
[player API guide](../../../crates/player/README.md) for the contract.

## Development

You need:

- Rust through rustup. `pnpm generate` adds the `wasm32-unknown-unknown` target and
  installs the wasm-bindgen CLI matching `Cargo.lock` under
  `target/player-bindings/tools`; the first run takes a few minutes.
- Node 22 or later and pnpm 12.
- Chrome or another Chromium for the browser tests.

```sh
cd platforms/web/player
pnpm install --frozen-lockfile
pnpm generate
pnpm build
```

`generate` compiles the `player` crate to Wasm and writes its TypeScript bindings to
`generated/`; rerun it after changing the crate. `build` type-checks and bundles them
with the adapters into `dist/`; rerun it after changing `src/`. Both directories are
ignored. Change the Rust exports and regenerate rather than editing generated files.

### Using a local build in an app

Pack the package and install the tarball in the app:

```sh
pnpm pack --pack-destination ../../../target/player-web
# in the app
pnpm add /path/to/sdk/target/player-web/tidal-player-web-0.1.0.tgz
```

Packing runs `generate` and `build` first, so the tarball always matches the checkout.
After each change, repack and run `pnpm update @tidal/player-web` in the app; a
plain `pnpm install` does not notice a changed tarball at the same path. Browsers
load the package as native ESM with `player.wasm` next to `index.js`; an app whose
bundler cannot handle that serves the installed `dist/` as static files.

### Publishing

`pnpm publish` runs the same `prepack` and ships `dist/` only.

## Use

```ts
import {
  initialize, AudioFormat, FetchHttpTransport, ManifestConfiguration, ManifestType,
  ManifestUsage, MediaProduct, Player, PlayerConfig, PlayerEvent, ShakaDrmProvider,
  ShakaMediaPlayerFactory, TrackManifestConfiguration, UriScheme,
  VideoManifestConfiguration, type PlayerEvents,
} from '@tidal/player-web';

await initialize(); // loads player.wasm next to the bundle

const events: PlayerEvents = {
  onEvent(event) {
    if (PlayerEvent.StateChanged.instanceOf(event)) console.log(event.inner.state);
  },
};
const config = PlayerConfig.create({
  baseUrl: 'https://openapi.tidal.com/v2',
  manifestConfiguration: ManifestConfiguration.create({
    trackConfiguration: TrackManifestConfiguration.create({
      manifestType: ManifestType.MpegDash,
      audioFormats: [AudioFormat.AacLc],
      adaptive: false,
      uriScheme: UriScheme.Https,
      usage: ManifestUsage.Playback,
    }),
    videoConfiguration: VideoManifestConfiguration.create({
      uriScheme: UriScheme.Https,
      usage: ManifestUsage.Playback,
    }),
  }),
  crossfade: { overlap: 5000 },
});
const player = new Player(config, new FetchHttpTransport(), authenticator,
  new ShakaMediaPlayerFactory(showVideo), new ShakaDrmProvider(), eventSender, events);

await player.play(player.prepare(new MediaProduct.Track({ id: '1' })));
await player.setNext(player.prepare(new MediaProduct.Track({ id: '2' })));
await player.seek(30_000);
await player.pause();
```

`ShakaDrmProvider` does not preload licences; media players fetch them through the
core when Shaka asks. They use Widevine where the browser supports it and FairPlay
otherwise (Safari), so the host requests DASH manifests for Widevine and HLS for
FairPlay.

The host supplies the authenticator and the `TidalEventSender`, which delivers the
core's play logs and streaming metrics, for example through
`@tidal-music/event-producer`.

`ShakaMediaPlayerFactory` takes a callback that receives the element of the video
that most recently started rendering, for the host to attach, and nothing when
audio-only media starts or that media player is disposed. Release objects
with `uniffiDestroy()`; the core disposes media players it created. A pending
exported call keeps the core alive after `uniffiDestroy()`, so pass an
`AbortSignal` to calls and abort it before releasing the Player.

`@tidal/player-web/bindings` exports the generated API without Shaka, for tests
that run outside a browser.

## Checks

```sh
pnpm check
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
  --remote-debugging-port=9222 --user-data-dir="$(mktemp -d)" &
pnpm test:browser http://127.0.0.1:9222
```

The browser test attaches to a running Chromium through its DevTools port, serves
local clear DASH and MP4 fixtures and runs the real adapters against the core,
covering cancellation, timeouts, stalls, failures, seeks, the video surface,
crossfade and release. It does not cover DRM licences, catalogue media or audible
quality. Failure scenarios wait for Shaka's retries, so the run takes about two
minutes.
