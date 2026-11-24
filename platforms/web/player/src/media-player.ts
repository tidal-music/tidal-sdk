import shaka from 'shaka-player';
import {
  MediaEvent,
  MediaItemImpl,
  MediaPlayerEventsImpl,
  type MediaItem,
  type MediaPlayer,
  type MediaPlayerEvent,
  type MediaPlayerEvents,
} from '../generated/player';
import { abortable } from './cancellation';
import { mediaError } from './errors';
import { MediaPlayerItem } from './media-player-item';

type AsyncOptions = { signal: AbortSignal };

export class ShakaMediaPlayer implements MediaPlayer {
  // A video element also renders audio-only assets, so the surface never needs swapping.
  readonly element = document.createElement('video');
  private readonly player = new shaka.Player();
  private item?: MediaPlayerItem;
  private playing = false;
  private disposed = false;

  constructor(
    private readonly events: MediaPlayerEvents,
    private readonly onStartedRendering: (
      videoElement?: HTMLVideoElement,
    ) => void,
    private readonly onDisposed: () => void,
  ) {
    this.element.preload = 'auto';
    this.element.playsInline = true;
    // Shaka's load() waits for the attach and fails if it did not succeed.
    void this.player.attach(this.element).catch(() => {});
  }

  async setCurrent(item: MediaItem, options?: AsyncOptions): Promise<void> {
    const current = await this.replaceItem(item, options);
    if (this.playing) current.play();
  }

  async setCurrentAndPlay(
    item: MediaItem,
    options?: AsyncOptions,
  ): Promise<void> {
    this.playing = true;
    const current = await this.replaceItem(item, options);
    current.play();
  }

  async setNext(): Promise<void> {
    throw mediaError(
      new Error('The Shaka media player has no next item; configure crossfade'),
    );
  }

  async pause(): Promise<void> {
    this.playing = false;
    this.item?.pause();
  }

  async resume(): Promise<void> {
    this.playing = true;
    this.item?.play();
  }

  async seek(milliseconds: number): Promise<void> {
    await this.item?.seek(milliseconds);
  }

  async setVolume(gain: number): Promise<void> {
    this.element.volume = Math.min(1, Math.max(0, gain));
  }

  async reset(options?: AsyncOptions): Promise<void> {
    this.playing = false;
    this.releaseItem();
    await abortable(this.player.unload(), options?.signal);
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.releaseItem();
    this.onDisposed();
    // Destroying the Shaka player also clears the element's media.
    void this.player
      .destroy()
      .finally(() => this.element.remove())
      .catch(() => {});
    if (MediaPlayerEventsImpl.instanceOf(this.events))
      this.events.uniffiDestroy();
  }

  private async replaceItem(
    item: MediaItem,
    options?: AsyncOptions,
  ): Promise<MediaPlayerItem> {
    if (this.disposed) {
      release(item);
      throw mediaError(new Error('Media player disposed'));
    }
    this.releaseItem();
    // An installation failure leaves the item placed until the core replaces it.
    const current = new MediaPlayerItem(item, this.element, this.player, {
      emit: event => this.emit(event),
      startedRendering: () =>
        this.onStartedRendering(
          this.player.isAudioOnly() ? undefined : this.element,
        ),
      ended: () => {
        this.playing = false;
      },
    });
    this.item = current;
    try {
      await current.load(options?.signal);
    } catch (error) {
      throw mediaError(error);
    }
    return current;
  }

  private releaseItem(): void {
    this.item?.close();
    this.item = undefined;
    this.element.pause();
  }

  private emit(event: MediaPlayerEvent): void {
    this.events.onEvent(MediaEvent.create({ event, emittedAt: new Date() }));
  }
}

function release(item: MediaItem): void {
  if (MediaItemImpl.instanceOf(item)) item.uniffiDestroy();
}
