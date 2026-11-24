import shaka from 'shaka-player';
import type {
  MediaPlayer,
  MediaPlayerEvents,
  MediaPlayerFactory,
} from '../generated/player';
import { registerDrmScheme } from './drm';
import { ShakaMediaPlayer } from './media-player';

/** Creates Shaka media players; the core owns ordering, advancement and crossfade. */
export class ShakaMediaPlayerFactory implements MediaPlayerFactory {
  private videoElement?: HTMLVideoElement;

  /**
   * @param onVideoElement Receives the video element that most recently started
   * rendering, for the host's video surface, or nothing when no video renders.
   */
  constructor(
    private readonly onVideoElement: (videoElement?: HTMLVideoElement) => void,
  ) {
    shaka.polyfill.installAll();
    if (!shaka.Player.isBrowserSupported())
      throw new Error('Shaka is not supported by this browser');
    registerDrmScheme();
  }

  create(events: MediaPlayerEvents): MediaPlayer {
    const player = new ShakaMediaPlayer(
      events,
      videoElement => this.setVideoElement(videoElement),
      () => {
        if (this.videoElement === player.element)
          this.setVideoElement(undefined);
      },
    );
    return player;
  }

  private setVideoElement(videoElement?: HTMLVideoElement): void {
    if (videoElement === this.videoElement) return;
    this.videoElement = videoElement;
    this.onVideoElement(videoElement);
  }
}
