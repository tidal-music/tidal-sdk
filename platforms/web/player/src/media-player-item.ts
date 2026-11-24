import shaka from 'shaka-player';
import {
  MediaItemImpl,
  MediaPlayerEvent,
  type MediaItem,
  type PlaybackPosition,
} from '../generated/player';
import { abortable } from './cancellation';
import { configureDrm } from './drm';
import { mediaError } from './errors';

// Rides out short outages before a segment fails the item.
const RETRY_PARAMETERS = {
  maxAttempts: 5,
  baseDelay: 1000,
  backoffFactor: 2,
  fuzzFactor: 0.5,
  timeout: 5000,
};

export interface MediaPlayerItemHost {
  emit(event: MediaPlayerEvent): void;
  startedRendering(): void;
  ended(): void;
}

export class MediaPlayerItem {
  private readonly lifetime = new AbortController();
  private readonly listeners = new shaka.util.EventManager();
  private loaded = false;
  private started = false;
  private reportedPosition?: number;
  private positionTimer?: ReturnType<typeof setInterval>;

  constructor(
    readonly item: MediaItem,
    private readonly element: HTMLVideoElement,
    private readonly player: shaka.Player,
    private readonly host: MediaPlayerItemHost,
  ) {}

  async load(cancel?: AbortSignal): Promise<void> {
    const signal = this.lifetime.signal;
    const cancelled = () => this.finish(cancel?.reason);
    cancel?.addEventListener('abort', cancelled, { once: true });
    try {
      await this.install(signal);
    } finally {
      cancel?.removeEventListener('abort', cancelled);
    }
    this.loaded = true;
    this.element.volume = Math.min(1, Math.max(0, this.item.volume()));
    this.host.emit(new MediaPlayerEvent.Current({ item: this.item }));
  }

  private async install(signal: AbortSignal): Promise<void> {
    this.host.emit(new MediaPlayerEvent.Preparing(this.item));
    this.observe();
    const input = this.item.mediaInput();
    this.player.resetConfiguration();
    this.player.configure({
      manifest: { retryParameters: RETRY_PARAMETERS },
      streaming: {
        retryParameters: RETRY_PARAMETERS,
        bufferingGoal: 40,
        bufferBehind: 40,
      },
    });
    if (input.drm) configureDrm(this.player, this.item, signal);
    await abortable(this.player.load(input.url), signal);
  }

  play(): void {
    // An ended or failed item has stopped observing, so its playback would go unreported.
    if (this.lifetime.signal.aborted) return;
    // Browser events report rendering; a rejected request is a playback failure. An
    // AbortError only means a pause or load interrupted it, which events already report.
    this.element.play().catch(error => {
      if (!(error instanceof DOMException && error.name === 'AbortError'))
        this.fail(error);
    });
  }

  pause(): void {
    this.element.pause();
  }

  async seek(milliseconds: number): Promise<void> {
    this.host.emit(
      new MediaPlayerEvent.Seeking(this.item, this.position().position),
    );
    try {
      // Without metadata the element only records a start position and never seeks.
      if (this.element.readyState === HTMLMediaElement.HAVE_NOTHING)
        await this.next(['loadedmetadata']);
      const duration = this.element.duration;
      const seeked = this.next(['seeked']);
      this.element.currentTime = Number.isFinite(duration)
        ? Math.min(milliseconds / 1000, duration)
        : milliseconds / 1000;
      await seeked;
    } catch (error) {
      throw mediaError(error);
    }
  }

  close(): void {
    this.finish(new Error('Media player item closed'));
    if (MediaItemImpl.instanceOf(this.item)) this.item.uniffiDestroy();
  }

  private observe(): void {
    this.listeners.listen(this.element, 'canplay', this.onCanPlay);
    this.listeners.listen(this.element, 'seeked', this.onSeeked);
    this.listeners.listen(this.element, 'playing', this.onPlaying);
    this.listeners.listen(this.element, 'pause', this.onPause);
    this.listeners.listen(this.element, 'waiting', this.onStall);
    this.listeners.listen(this.player, 'stalldetected', this.onStall);
    this.listeners.listen(
      this.element,
      'durationchange',
      this.onDurationChange,
    );
    this.listeners.listen(this.element, 'ended', this.onEnded);
    this.listeners.listen(this.player, 'error', this.onError);
  }

  private readonly onCanPlay = (): void =>
    this.host.emit(new MediaPlayerEvent.Prepared(this.item));

  private readonly onSeeked = (): void =>
    this.host.emit(new MediaPlayerEvent.Seeked(this.item, this.position()));

  private readonly onPlaying = (): void => {
    const position = this.position();
    this.host.emit(
      this.started
        ? new MediaPlayerEvent.Resumed(this.item, position)
        : new MediaPlayerEvent.Started(this.item, position.position),
    );
    this.started = true;
    this.host.startedRendering();
    this.publishPosition();
    this.positionTimer ??= setInterval(() => this.publishPosition(), 100);
  };

  private readonly onPause = (): void => {
    // The element pauses just before it ends; that is not a pause.
    if (!this.element.ended)
      this.host.emit(new MediaPlayerEvent.Paused(this.item, this.position()));
  };

  private readonly onStall = (): void => {
    this.host.emit(new MediaPlayerEvent.Stalled(this.item, this.position()));
  };

  private readonly onDurationChange = (): void => this.publishDuration();

  private readonly onEnded = (): void => {
    const position = this.position().position;
    this.finish(new Error('Media player item ended'));
    this.host.ended();
    this.host.emit(new MediaPlayerEvent.Ended(this.item, position));
    this.host.emit(new MediaPlayerEvent.Completed());
  };

  private readonly onError = (event: Event): void => {
    const error = criticalError(event);
    if (error) this.fail(error);
  };

  private fail(error: unknown): void {
    if (this.lifetime.signal.aborted) return;
    if (!this.loaded) return this.finish(error);
    const position = this.position().position;
    this.finish(error);
    this.host.emit(
      new MediaPlayerEvent.Failed(this.item, position, mediaError(error)),
    );
  }

  private finish(reason: unknown): void {
    clearInterval(this.positionTimer);
    this.listeners.removeAll();
    this.lifetime.abort(reason);
  }

  private next(types: string[]): Promise<void> {
    const fired = new Promise<void>(resolve =>
      this.listeners.listenOnceMulti(this.element, types, () => resolve()),
    );
    return abortable(fired, this.lifetime.signal);
  }

  private publishPosition(): void {
    const { currentTime, paused, seeking } = this.element;
    if (paused || seeking || currentTime === this.reportedPosition) return;
    this.reportedPosition = currentTime;
    this.host.emit(new MediaPlayerEvent.Position(this.item, this.position()));
  }

  private publishDuration(): void {
    const duration = this.element.duration;
    if (Number.isFinite(duration))
      this.host.emit(new MediaPlayerEvent.Duration(this.item, duration * 1000));
  }

  private position(): PlaybackPosition {
    const seconds = this.element.currentTime;
    const duration = this.element.duration;
    return {
      position: seconds * 1000,
      remaining: Number.isFinite(duration)
        ? Math.max(0, duration - seconds) * 1000
        : 0,
    };
  }
}

// Non-critical Shaka errors are still being retried.
function criticalError(event: Event): shaka.util.Error | undefined {
  const error = (event as unknown as { detail: shaka.util.Error }).detail;
  return error.severity === shaka.util.Error.Severity.CRITICAL
    ? error
    : undefined;
}
