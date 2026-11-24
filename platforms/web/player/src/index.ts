import { uniffiInitAsync } from '../generated/index';

export * from './bindings';
export { FetchHttpTransport } from './fetch-transport';
export { ShakaDrmProvider } from './drm';
export { ShakaMediaPlayerFactory } from './media-player-factory';

/** Opens the Wasm module once before constructing players; the caller owns player configuration. */
export function initialize(
  source: URL | ArrayBuffer | Uint8Array = new URL(
    './player.wasm',
    import.meta.url,
  ),
): Promise<void> {
  return uniffiInitAsync(source);
}
