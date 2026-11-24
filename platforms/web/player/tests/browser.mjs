import * as sdk from '../dist/index.js';

function assert(condition, message) {
  if (!condition) throw new Error(message);
}
function signal() {
  const values = [];
  const listeners = new Set();
  return {
    values,
    send(value) {
      values.push(value);
      for (const listener of [...listeners]) listener(value);
    },
    wait(predicate = () => true, timeout = 15_000) {
      const found = values.find(predicate);
      if (found !== undefined) return Promise.resolve(found);
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          listeners.delete(listener);
          reject(new Error(`timed out; observed ${describe(values)}`));
        }, timeout);
        const listener = value => {
          if (predicate(value)) {
            clearTimeout(timer);
            listeners.delete(listener);
            resolve(value);
          }
        };
        listeners.add(listener);
      });
    },
  };
}
const describe = values =>
  JSON.stringify(
    values.map(value =>
      value?.tag === undefined
        ? value
        : [value.tag, value.inner?.item?.inner?.id ?? value.inner?.state?.tag],
    ),
  );
function release(seed) {
  if (sdk.PlaybackSeed.instanceOf(seed)) seed.uniffiDestroy();
}
const waitRequest = (kind, id) =>
  fetch(`/control/wait?kind=${kind}&id=${id}`).then(response =>
    response.json(),
  );
const openGate = id => fetch(`/control/release/${id}`, { method: 'POST' });
const config = crossfade => ({
  baseUrl: `${location.origin}/api`,
  manifestConfiguration: {
    trackConfiguration: {
      manifestType: sdk.ManifestType.MpegDash,
      audioFormats: [sdk.AudioFormat.AacLc],
      adaptive: false,
      uriScheme: sdk.UriScheme.Https,
      usage: sdk.ManifestUsage.Playback,
    },
    videoConfiguration: {
      uriScheme: sdk.UriScheme.Https,
      usage: sdk.ManifestUsage.Playback,
    },
  },
  crossfade,
});
const auth = {
  accessToken: () => Promise.resolve('fixture-token'),
  refreshedAccessToken: () => Promise.resolve('fixture-refreshed'),
};
const track = id => new sdk.MediaProduct.Track({ id });
const tag = event => event.event.tag;
const M = sdk.MediaPlayerEvent_Tags;
const isState = (state, id) => event =>
  (sdk.PlayerEvent.StateChanged.instanceOf(event) ||
    sdk.PlayerEvent.CurrentItemChanged.instanceOf(event)) &&
  event.inner.state.tag === state &&
  (id === undefined || event.inner.item?.inner.id === id);
const hosts = [];
let mediaPlayers = 0;

/** A Player over the real adapters, recording player events, media events and manifest requests. */
function host(crossfade) {
  const events = signal();
  const media = signal();
  const released = signal();
  const manifests = [];
  const transport = new sdk.FetchHttpTransport();
  const video = { element: undefined };
  class Factory extends sdk.ShakaMediaPlayerFactory {
    created = [];
    constructor() {
      super(element => {
        video.element = element;
      });
    }
    create(events) {
      const player = super.create({
        onEvent(event) {
          media.send(event);
          events.onEvent(event);
        },
      });
      const dispose = player.dispose.bind(player);
      let calls = 0;
      player.dispose = () => {
        assert(++calls === 1, 'core invoked dispose more than once');
        dispose();
        released.send(player);
      };
      this.created.push(player);
      mediaPlayers++;
      return player;
    }
  }
  const factory = new Factory();
  const player = new sdk.Player(
    config(crossfade),
    {
      execute(request, options) {
        manifests.push(new URL(request.url).pathname.split('/').at(-1));
        return transport.execute(request, options);
      },
    },
    auth,
    factory,
    new sdk.ShakaDrmProvider(),
    { sendEvent() {} },
    { onEvent: event => events.send(event) },
  );
  const h = { player, factory, events, media, released, manifests, video };
  hosts.push(h);
  return h;
}

export async function run() {
  await sdk.initialize(new URL('../dist/player.wasm', import.meta.url));
  const results = [];
  const log = message => {
    results.push(message);
    console.info(message);
    document.querySelector('#results').textContent += `${message}\n`;
  };
  const elements = [];
  const create = document.createElement.bind(document);
  document.createElement = (name, options) => {
    const element = create(name, options);
    if (element instanceof HTMLMediaElement) elements.push(element);
    return element;
  };
  // Media players keep their element, so scenarios count only the elements they created.
  const live = (from = 0) =>
    elements.slice(from).filter(element => element.readyState >= 3);
  try {
    {
      const h = host();
      const seed = h.player.prepare(track('fetch-hold'));
      await waitRequest('request', 'fetch-hold');
      release(seed);
      await waitRequest('abort', 'fetch-hold');
      log('PASS final seed release aborts the Fetch request');
    }
    {
      const h = host();
      const seed = h.player.prepare(track('media-hold'));
      const load = h.player.play(seed);
      await waitRequest('request', 'media-hold');
      await h.player.reset();
      await load;
      await waitRequest('abort', 'media-hold');
      await openGate('media-hold');
      const replacement = h.player.prepare(track('after-reset'));
      await h.player.play(replacement);
      assert(
        !h.media.values.some(event => tag(event) === M.Failed) &&
          h.released.values.length === 0,
        `reset failed or disposed the reusable media: ${describe(h.media.values)}`,
      );
      await h.player.reset();
      release(seed);
      release(replacement);
      log(
        'PASS reset cancels pending Shaka networking and the media stays reusable',
      );
    }
    {
      const h = host();
      const seed = h.player.prepare(track('missing'));
      const error = await h.player.play(seed).then(
        () => undefined,
        error => error,
      );
      assert(
        sdk.PlayerError.instanceOf(error),
        `installation failure was not returned: ${error}`,
      );
      await h.events.wait(isState(sdk.PlayerState_Tags.Failed, 'missing'));
      assert(
        !h.media.values.some(event => tag(event) === M.Failed),
        'installation failure was also reported as Failed',
      );
      await h.player.reset();
      release(seed);
      log(
        'PASS installation failure returns its error without a Failed observation',
      );
    }
    {
      // Shaka's load() resolves before media segments arrive; a later failure is Failed.
      const h = host();
      const seed = h.player.prepare(track('segment-fail'));
      await h.player.play(seed);
      await waitRequest('request', 'segment-fail');
      await openGate('segment-fail');
      await h.media.wait(event => tag(event) === M.Failed, 40_000);
      assert(
        !h.media.values.some(event => tag(event) === M.Prepared),
        'an item whose media failed was reported Prepared',
      );
      await h.player.reset();
      release(seed);
      log('PASS a media failure before Prepared is reported as Failed');
    }
    {
      // Playback requested before media arrives stalls; pausing it must report Paused.
      const h = host();
      const seed = h.player.prepare(track('segment-stall'));
      await h.player.play(seed);
      await h.media.wait(event => tag(event) === M.Stalled);
      await h.player.pause();
      await h.media.wait(event => tag(event) === M.Paused);
      const observed = h.media.values
        .map(tag)
        .filter(name => name !== M.Duration);
      assert(
        !observed.includes(M.Started) && !observed.includes(M.Prepared),
        `stalled item reported readiness or rendering: ${observed}`,
      );
      await h.player.reset();
      await openGate('segment-stall');
      release(seed);
      log('PASS pausing a stall before the first frame reports Paused');
    }
    {
      // A seek before the element has metadata seeks once the metadata arrives.
      const h = host();
      const seed = h.player.prepare(track('segment-metadata-hold'));
      await h.player.play(seed);
      assert(
        elements.at(-1).readyState === HTMLMediaElement.HAVE_NOTHING,
        'the element had metadata before the init segment was released',
      );
      const seek = h.player.seek(1000);
      await openGate('segment-metadata-hold');
      await seek;
      const seeked = await h.media.wait(event => tag(event) === M.Seeked);
      assert(
        Math.abs(Number(seeked.event.inner[1].position) - 1000) < 100,
        `seek before metadata landed at ${Number(seeked.event.inner[1].position)}`,
      );
      await h.player.reset();
      release(seed);
      log('PASS a seek before metadata completes once the metadata arrives');
    }
    {
      // Reset before the item is Prepared; the late segment must not revive the item.
      const h = host();
      const seed = h.player.prepare(track('segment-reset'));
      const load = h.player.play(seed).then(
        () => undefined,
        error => error,
      );
      await waitRequest('request', 'segment-reset');
      await h.player.reset();
      const afterReset = h.media.values.length;
      await openGate('segment-reset');
      await load;
      await new Promise(resolve => setTimeout(resolve, 500));
      const late = h.media.values.slice(afterReset).map(tag);
      assert(
        late.length === 0,
        `reset item reported after its late readiness: ${late}`,
      );
      release(seed);
      log('PASS a reset before Prepared keeps late media from reviving the item');
    }
    {
      const mark = elements.length;
      const h = host();
      const seed = h.player.prepare(track('drm-clear'));
      await h.player.play(seed);
      await h.media.wait(event => tag(event) === M.Started);
      assert(live(mark).length === 1, 'Widevine configuration blocked clear media');
      assert(
        !h.manifests.some(path => ['certificate', 'license'].includes(path)),
        `clear media acquired a licence: ${h.manifests}`,
      );
      await h.player.reset();
      release(seed);
      log('PASS Widevine configuration leaves clear media playable');
    }
    {
      // Shaka's stall detection: the element may not fire 'playing' after the nudge,
      // so the next Position ends the stall.
      const h = host();
      const seed = h.player.prepare(track('stall-detected'));
      await h.player.play(seed);
      await h.media.wait(event => tag(event) === M.Started);
      const before = h.media.values.length;
      h.factory.created[0].player.dispatchEvent({ type: 'stalldetected' });
      const after = event => h.media.values.indexOf(event) >= before;
      const stalled = await h.media.wait(
        event => tag(event) === M.Stalled && after(event),
      );
      await h.media.wait(
        event =>
          tag(event) === M.Position &&
          h.media.values.indexOf(event) > h.media.values.indexOf(stalled),
        2_000,
      );
      const observed = h.media.values
        .slice(before)
        .map(tag)
        .filter(name => name !== M.Duration);
      assert(
        observed.join() === [M.Stalled, M.Position].join(),
        `stall detection observations: ${observed}`,
      );
      await h.player.reset();
      release(seed);
      log('PASS a detected stall reports Stalled, then Position once playback advances');
    }
    {
      // Replacing a playing item pauses the element; that pause belongs to the old item.
      const h = host();
      const first = h.player.prepare(track('tone'));
      await h.player.play(first);
      await h.media.wait(event => tag(event) === M.Started);
      const before = h.media.values.length;
      const second = h.player.prepare(track('after-reset'));
      await h.player.play(second);
      await h.media.wait(
        event =>
          tag(event) === M.Started && h.media.values.indexOf(event) >= before,
      );
      const observed = h.media.values.slice(before).map(tag);
      assert(
        observed.indexOf(M.Paused) === -1 ||
          observed.indexOf(M.Paused) > observed.indexOf(M.Started),
        `replacement reported Paused before Started: ${observed}`,
      );
      assert(
        h.factory.created.length === 1,
        'the replacement did not reuse the element',
      );
      await h.player.reset();
      release(first);
      release(second);
      log('PASS a replaced item does not report the old item\'s pause');
    }
    {
      // A play() interrupted outside the core's commands must not fail the item.
      const h = host();
      const seed = h.player.prepare(track('interrupted-play'));
      await h.player.play(seed);
      await h.media.wait(event => tag(event) === M.Started);
      await h.player.pause();
      const element = h.factory.created[0].element;
      let interrupted = 0;
      element.play = () => {
        interrupted++;
        return Promise.reject(
          new DOMException('The play() request was interrupted', 'AbortError'),
        );
      };
      await h.player.resume();
      await new Promise(resolve => setTimeout(resolve, 200));
      delete element.play;
      await h.player.resume();
      await h.media.wait(event => tag(event) === M.Resumed);
      assert(
        interrupted === 1 &&
          !h.media.values.some(event => tag(event) === M.Failed),
        `interrupted play failed the item: ${describe(h.media.values.filter(event => tag(event) !== M.Position))}`,
      );
      await h.player.reset();
      release(seed);
      log('PASS an interrupted play() leaves the item playable');
    }
    {
      // The segment a playing seek needs is held, then fails after Shaka's retries.
      const h = host();
      const seed = h.player.prepare(track('segment-seek-fail'));
      await h.player.play(seed);
      await h.media.wait(event => tag(event) === M.Started);
      const seek = h.player.seek(2500).then(
        () => 'resolved',
        () => 'rejected',
      );
      await h.media.wait(event => tag(event) === M.Seeking);
      await openGate('segment-seek-fail');
      const outcome = await Promise.race([
        seek,
        new Promise(resolve => setTimeout(() => resolve('pending'), 40_000)),
      ]);
      const observed = h.media.values.map(tag);
      assert(
        outcome === 'rejected' &&
          observed.includes(M.Failed) &&
          !observed.includes(M.Seeked),
        `failed seek ${outcome}: ${describe(h.media.values.filter(event => tag(event) !== M.Position))}`,
      );
      await h.player.reset();
      release(seed);
      log('PASS a media failure during a playing seek rejects the seek with Failed');
    }
    {
      const transport = new sdk.FetchHttpTransport();
      const request = id =>
        transport
          .execute({
            method: sdk.HttpMethod.Get,
            url: `${location.origin}/api/trackManifests/${id}`,
            headers: new Map(),
          })
          .then(
            () => undefined,
            error => error,
          );
      const begin = performance.now();
      const errors = await Promise.all([
        request('fetch-hold-timeout'),
        request('fetch-stall-body'),
      ]);
      const elapsed = performance.now() - begin;
      assert(
        errors.every(error => sdk.TransportError.TimeoutError.instanceOf(error)) &&
          elapsed > 4_900 &&
          elapsed < 7_000,
        `fetch timeouts after ${elapsed} ms: ${errors}`,
      );
      log(
        'PASS Fetch times out after 5 s, including a stalled body, as TimeoutError',
      );
    }
    {
      const h = host();
      const seed = h.player.prepare(new sdk.MediaProduct.Video({ id: 'video' }));
      await h.player.play(seed);
      await h.media.wait(event => tag(event) === M.Started);
      await h.player.pause();
      await h.media.wait(event => tag(event) === M.Paused);
      const before = h.media.values.length;
      await h.player.seek(500);
      const seek = h.media.values.slice(before).map(tag);
      assert(
        seek.join() === [M.Seeking, M.Seeked].join(),
        `paused seek observations: ${seek}`,
      );
      await h.player.resume();
      await h.events.wait(
        event =>
          sdk.PlayerEvent.PositionChanged.instanceOf(event) &&
          event.inner.position > 600,
      );
      assert(
        h.video.element instanceof HTMLVideoElement,
        'rendering video has no surface',
      );
      await h.player.pause();
      const paused = h.media.values.length;
      await new Promise(resolve => setTimeout(resolve, 300));
      assert(
        h.media.values.slice(paused).every(event => tag(event) !== M.Position),
        'Position reported while paused',
      );
      await h.player.reset();
      release(seed);
      log(
        'PASS video: paused seek reports Seeking/Seeked only, playback exposes the surface',
      );
    }
    {
      const h = host();
      const a = h.player.prepare(track('sequential-a'));
      const b = h.player.prepare(track('sequential-b'));
      await h.player.play(a);
      const error = await h.player.setNext(b).then(
        () => undefined,
        error => error,
      );
      assert(
        sdk.PlayerError.instanceOf(error),
        `a media player without crossfade accepted a next item: ${error}`,
      );
      await h.player.reset();
      release(a);
      release(b);
      log('PASS without crossfade the media player rejects a next item');
    }
    {
      // Without overlap the core still switches to the preloaded second media player.
      const h = host({ overlap: 0 });
      const seed = h.player.prepare(track('tone'));
      await h.player.play(seed);
      await h.player.setNext(seed);
      await h.events.wait(isState(sdk.PlayerState_Tags.Completed));
      const selections = h.events.values.filter(
        event =>
          sdk.PlayerEvent.CurrentItemChanged.instanceOf(event) &&
          event.inner.item?.inner.id === 'tone',
      );
      const ended = h.media.values.filter(event => tag(event) === M.Ended);
      assert(
        selections.length === 2 &&
          ended.length === 2 &&
          h.manifests.filter(id => id === 'tone').length === 1,
        `repeated product did not play as two occurrences from one preparation: ${describe(h.events.values)} ${h.manifests}`,
      );
      assert(
        h.video.element === undefined,
        'audio-only media exposed a video surface',
      );
      release(seed);
      log(
        'PASS staged successor advances naturally, a repeated product plays twice from one preparation, then completes',
      );
    }
    {
      const h = host({ overlap: 0 });
      const a = h.player.prepare(track('manual-a'));
      const b = h.player.prepare(track('manual-b'));
      await h.player.play(a);
      await h.player.setNext(b);
      await h.player.play(b);
      await h.events.wait(isState(sdk.PlayerState_Tags.Playing, 'manual-b'));
      assert(
        h.manifests.filter(id => id === 'manual-b').length === 1,
        `manual Next prepared the staged successor twice: ${h.manifests}`,
      );
      await h.player.reset();
      release(a);
      release(b);
      log('PASS manual Next reuses the staged preparation');
    }
    {
      const mark = elements.length;
      const h = host({ overlap: 1000 });
      const a = h.player.prepare(track('crossfade-a'));
      const b = h.player.prepare(track('crossfade-b'));
      await h.player.play(a);
      await h.player.setNext(b);
      // Both media players are Prepared once their elements can play.
      await h.media.wait(
        () => h.media.values.filter(event => tag(event) === M.Prepared).length >= 2,
      );
      const active = live(mark);
      assert(
        active.length === 2,
        `expected two prepared elements, got ${active.length}`,
      );
      const overlap = new Promise(resolve => {
        const check = () => {
          if (
            active.every(
              element =>
                !element.paused &&
                element.currentTime > 0.03 &&
                element.volume > 0 &&
                element.volume < 1,
            )
          ) {
            active.forEach(element =>
              element.removeEventListener('volumechange', check),
            );
            resolve(
              active.map(element => ({
                position: element.currentTime,
                gain: element.volume,
              })),
            );
          }
        };
        active.forEach(element =>
          element.addEventListener('volumechange', check),
        );
      });
      await h.player.resume();
      const samples = await overlap;
      await h.events.wait(
        event =>
          sdk.PlayerEvent.CurrentItemChanged.instanceOf(event) &&
          event.inner.item?.inner.id === 'crossfade-b',
      );
      await h.player.reset();
      assert(
        active.every(element => element.paused && element.readyState === 0),
        'reset did not stop and release the elements',
      );
      release(a);
      release(b);
      log(`PASS crossfade overlaps two elements; samples ${JSON.stringify(samples)}`);
    }
    {
      const h = host({ overlap: 1000 });
      const a = h.player.prepare(track('rejected-a'));
      const b = h.player.prepare(track('missing'));
      await h.player.play(a);
      const error = await h.player.setNext(b).then(
        () => undefined,
        error => error,
      );
      assert(
        sdk.PlayerError.instanceOf(error),
        `a failing successor was accepted: ${error}`,
      );
      await h.events.wait(isState(sdk.PlayerState_Tags.Completed));
      assert(
        !h.media.values.some(event => tag(event) === M.Failed) &&
          h.media.values.filter(event => tag(event) === M.Started).length === 1,
        `rejected successor: ${describe(h.media.values.filter(event => tag(event) !== M.Position))}`,
      );
      release(a);
      release(b);
      log(
        'PASS a successor that fails to load rejects setNext, the current plays to completion and the successor never renders',
      );
    }
    {
      const h = host({ overlap: 1000 });
      const a = h.player.prepare(track('race-a'));
      const b = h.player.prepare(track('race-b'));
      await h.player.play(a);
      await h.player.setNext(b);
      // Manual Next while the core is already fading into the same successor.
      await h.media.wait(
        event => tag(event) === M.Started && h.media.values.filter(e => tag(e) === M.Started).length === 2,
      );
      await h.player.play(b);
      const playing = elements.filter(element => !element.paused).length;
      await h.events.wait(isState(sdk.PlayerState_Tags.Completed));
      const started = h.media.values.filter(
        event => tag(event) === M.Started,
      ).length;
      assert(
        playing === 1 &&
          started === 3 &&
          !h.media.values.some(event => tag(event) === M.Failed) &&
          h.manifests.filter(id => id === 'race-b').length === 1 &&
          elements.every(element => element.paused),
        `manual Next racing the crossfade: ${playing} playing, ${started} started, ${h.manifests}`,
      );
      release(a);
      release(b);
      log(
        'PASS manual Next during the crossfade restarts the staged successor once from its preparation, then completes',
      );
    }
    {
      // A held call retains the core, so the host aborts its calls before release.
      const mark = elements.length;
      const playing = host({ overlap: 1000 });
      const abort = new AbortController();
      const a = playing.player.prepare(track('release-a'));
      await playing.player.play(a, { signal: abort.signal });
      await playing.events.wait(isState(sdk.PlayerState_Tags.Playing, 'release-a'));
      const b = playing.player.prepare(track('fetch-hold-release-b'));
      const staging = playing.player.setNext(b, { signal: abort.signal });
      await waitRequest('request', 'fetch-hold-release-b');
      const loading = host();
      const c = loading.player.prepare(track('media-hold-release-c'));
      const load = loading.player.play(c, { signal: abort.signal });
      await waitRequest('request', 'media-hold-release-c');
      abort.abort();
      for (const h of [playing, loading]) {
        h.player.uniffiDestroy();
        hosts.splice(hosts.indexOf(h), 1);
      }
      await Promise.allSettled([staging, load]);
      for (const h of [playing, loading])
        await h.released.wait(
          () => h.released.values.length === h.factory.created.length,
          2_000,
        );
      await waitRequest('abort', 'media-hold-release-c');
      assert(
        live(mark).length === 0,
        'released Player left rendering media behind its held calls',
      );
      release(a);
      release(b);
      release(c);
      await waitRequest('abort', 'fetch-hold-release-b');
      log(
        'PASS aborting held setNext and load calls lets Player release dispose its media promptly',
      );
    }
    for (const h of hosts) h.player.uniffiDestroy();
    for (const h of hosts)
      await h.released.wait(
        () => h.released.values.length === h.factory.created.length,
      );
    await Promise.all(
      elements.map(element => {
        const closed = () =>
          element.paused &&
          element.readyState === 0 &&
          !element.getAttribute('src');
        if (closed()) return Promise.resolve();
        return new Promise(resolve => {
          const check = () => {
            if (closed()) {
              element.removeEventListener('emptied', check);
              resolve();
            }
          };
          element.addEventListener('emptied', check);
        });
      }),
    );
    assert(
      elements.length === mediaPlayers,
      `${elements.length} media elements for ${mediaPlayers} media players`,
    );
    log(
      'PASS final Player release disposes every media instance once, each reused one element across its items, and no element stays live',
    );
    return results;
  } finally {
    document.createElement = create;
  }
}
