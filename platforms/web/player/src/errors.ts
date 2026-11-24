import shaka from 'shaka-player';
import { MediaError } from '../generated/player';

export function mediaError(error: unknown): MediaError {
  const description = error instanceof Error ? error.message : String(error);
  if (error instanceof shaka.util.Error) {
    const code = String(error.code);
    switch (error.category) {
      case shaka.util.Error.Category.NETWORK:
        return new MediaError.Network({ code, description });
      case shaka.util.Error.Category.DRM:
        return new MediaError.Other({ code, description });
      case shaka.util.Error.Category.MANIFEST:
      case shaka.util.Error.Category.MEDIA:
        return new MediaError.Content({ code, description });
    }
  }
  return new MediaError.Other({ code: 'BROWSER', description });
}
