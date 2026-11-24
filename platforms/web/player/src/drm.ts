import shaka from 'shaka-player';
import {
  CertificateRequest,
  type DrmProvider,
  type LicenseSession,
  type MediaItem,
} from '../generated/player';

const WIDEVINE = 'com.widevine.alpha';
const FAIRPLAY = 'com.apple.fps';
const SCHEME = 'tidal-drm';
const loads = new Map<string, { item: MediaItem; signal: AbortSignal }>();
let nextLoad = 0;
// The Widevine service certificate request.
const CERTIFICATE_REQUEST = new Uint8Array([0x08, 0x04]);

/** Web media players fetch licences through their item when Shaka asks. */
export class ShakaDrmProvider implements DrmProvider {
  preloadsLicenses(): boolean {
    return false;
  }

  createSession(): LicenseSession {
    throw new Error('The web DRM provider does not preload licences');
  }
}

export function configureDrm(
  player: shaka.Player,
  item: MediaItem,
  signal: AbortSignal,
): void {
  const id = String(++nextLoad);
  loads.set(id, { item, signal });
  signal.addEventListener('abort', () => loads.delete(id), { once: true });
  const uri = (system: string, kind: string) =>
    `${SCHEME}:${id}/${system}/${kind}`;
  player.configure({
    drm: {
      servers: {
        [WIDEVINE]: uri(WIDEVINE, 'license'),
        [FAIRPLAY]: uri(FAIRPLAY, 'license'),
      },
      advanced: {
        [WIDEVINE]: { serverCertificateUri: uri(WIDEVINE, 'certificate') },
        [FAIRPLAY]: { serverCertificateUri: uri(FAIRPLAY, 'certificate') },
      },
    },
  });
}

// Registering again replaces the plugin.
export function registerDrmScheme(): void {
  shaka.net.NetworkingEngine.registerScheme(SCHEME, fetchDrm);
}

const fetchDrm: shaka.extern.SchemePlugin = (uri, request) => {
  const [id, system, kind] = uri.slice(SCHEME.length + 1).split('/');
  const load = loads.get(id ?? '');
  const options = load && { signal: load.signal };
  const data = !load
    ? Promise.reject(new Error('DRM request after the load ended'))
    : kind !== 'certificate'
      ? load.item.fetchLicense(toArrayBuffer(request.body), options)
      : system === FAIRPLAY
        ? load.item.fetchCertificate(new CertificateRequest.Get(), options)
        : load.item
            .fetchCertificate(
              new CertificateRequest.Post(CERTIFICATE_REQUEST.slice().buffer),
              options,
            )
            .then(serviceCertificate);
  return shaka.util.AbortableOperation.notAbortable(
    data.then(
      (data): shaka.extern.Response => ({
        uri,
        originalUri: uri,
        data,
        headers: {},
        originalRequest: request,
      }),
      (error: unknown) => {
        throw new shaka.util.Error(
          shaka.util.Error.Severity.CRITICAL,
          shaka.util.Error.Category.DRM,
          shaka.util.Error.Code.LICENSE_REQUEST_FAILED,
          error,
        );
      },
    ),
  );
};

// The service answers with a Widevine SignedMessage; MediaKeys.setServerCertificate
// accepts only its `msg` field (2), the signed certificate.
function serviceCertificate(response: ArrayBuffer): ArrayBuffer {
  const bytes = new Uint8Array(response);
  let offset = 0;
  const varint = () => {
    let value = 0;
    for (let shift = 0; offset < bytes.length; shift += 7) {
      const byte = bytes[offset++]!;
      value += (byte & 0x7f) * 2 ** shift;
      if (byte < 0x80) return value;
    }
    throw new Error('Truncated Widevine service certificate');
  };
  while (offset < bytes.length) {
    const key = varint();
    const wireType = key & 7;
    if (wireType === 0) {
      varint();
    } else if (wireType === 2) {
      const length = varint();
      if (key >>> 3 === 2) return response.slice(offset, offset + length);
      offset += length;
    } else if (wireType === 1) {
      offset += 8;
    } else if (wireType === 5) {
      offset += 4;
    } else {
      break;
    }
  }
  throw new Error('Widevine service certificate response has no certificate');
}

function toArrayBuffer(
  body: ArrayBuffer | ArrayBufferView | null,
): ArrayBuffer {
  if (!body) return new ArrayBuffer(0);
  if (body instanceof ArrayBuffer) return body;
  return body.buffer.slice(
    body.byteOffset,
    body.byteOffset + body.byteLength,
  ) as ArrayBuffer;
}
