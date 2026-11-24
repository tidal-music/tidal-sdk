import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { resolve, extname } from 'node:path';
import { fileURLToPath } from 'node:url';

export async function serve() {
  const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
  const events = [];
  const listeners = new Set();
  const held = new Map();
  // Held segment requests per id; a seek can leave several pending.
  const heldSegments = new Map();
  const failing = id => id === 'segment-fail' || id === 'segment-seek-fail';
  const record = (kind, id) => {
    const event = { kind, id };
    events.push(event);
    for (const listener of [...listeners]) listener(event);
  };
  const matches = (event, kind, id) => event.kind === kind && event.id === id;
  let origin;
  const released = new Set();
  const manifest = id => ({
    data: {
      id,
      attributes: {
        uri: `${origin}/${id.startsWith('media-hold') ? `media/hold.mpd?id=${id}` : id.startsWith('segment-') ? `media/${id}/tone.mpd` : { video: 'tests/fixtures/color.mp4', missing: 'tests/fixtures/missing.mpd' }[id] ?? 'tests/fixtures/tone.mpd'}`,
        formats: ['AACLC'],
        ...(id === 'video'
          ? { link: { href: `${origin}/tests/fixtures/color.mp4` } }
          : {}),
        ...(id === 'drm-clear'
          ? {
              drmData: {
                drmSystem: 'WIDEVINE',
                licenseUrl: `${origin}/drm/license`,
                certificateUrl: `${origin}/drm/certificate`,
                initData: [],
              },
            }
          : {}),
      },
    },
  });
  const server = createServer(async (request, response) => {
    response.setHeader('Access-Control-Allow-Origin', '*');
    response.setHeader('Access-Control-Allow-Headers', '*');
    if (request.method === 'OPTIONS') {
      response.writeHead(204);
      response.end();
      return;
    }
    const url = new URL(request.url, origin);
    const json = value => {
      response.setHeader('Content-Type', 'application/json');
      response.end(JSON.stringify(value));
    };
    if (url.pathname === '/control/wait') {
      const kind = url.searchParams.get('kind');
      const id = url.searchParams.get('id');
      const existing = events.find(event => matches(event, kind, id));
      if (existing) {
        json(existing);
        return;
      }
      const listener = event => {
        if (matches(event, kind, id)) {
          listeners.delete(listener);
          json(event);
        }
      };
      listeners.add(listener);
      response.on('close', () => listeners.delete(listener));
      return;
    }
    if (url.pathname.startsWith('/control/release/')) {
      const id = url.pathname.split('/').at(-1);
      const pending = held.get(id);
      held.delete(id);
      if (id.startsWith('segment-')) {
        released.add(id);
        for (const pending of heldSegments.get(id) ?? []) {
          if (pending.destroyed) continue;
          if (failing(id)) pending.writeHead(404);
          pending.end(
            failing(id)
              ? undefined
              : await readFile(resolve(root, 'tests/fixtures', pending.segment)),
          );
        }
        heldSegments.delete(id);
      } else if (pending && !pending.destroyed) {
        pending.setHeader(
          'Content-Type',
          id.startsWith('media-hold')
            ? 'application/dash+xml'
            : 'application/json',
        );
        if (id.startsWith('media-hold'))
          pending.end(await readFile(resolve(root, 'tests/fixtures/tone.mpd')));
        else pending.end(JSON.stringify(manifest(id)));
      }
      record('release', id);
      json({ released: true });
      return;
    }
    // Serves the tone manifest and init segment for /media/segment-*/, holding media
    // segments until released; segment-metadata-hold holds its init segment too, and
    // segment-seek-fail serves its first media segment at once. The failing ids then
    // answer 404.
    if (url.pathname.startsWith('/media/segment-')) {
      const [, , segmentId, name] = url.pathname.split('/');
      if (
        name.startsWith('chunk-') ||
        (segmentId === 'segment-metadata-hold' && name.startsWith('init-'))
      ) {
        const free =
          segmentId === 'segment-seek-fail' && name.endsWith('-00001.m4s');
        if (free || released.has(segmentId)) {
          const fail = failing(segmentId) && !free;
          if (fail) response.writeHead(404);
          response.end(
            fail
              ? undefined
              : await readFile(resolve(root, 'tests/fixtures', name)),
          );
        } else {
          response.segment = name;
          heldSegments.set(segmentId, [
            ...(heldSegments.get(segmentId) ?? []),
            response,
          ]);
          record('request', segmentId);
        }
        return;
      }
      response.end(await readFile(resolve(root, 'tests/fixtures', name)));
      return;
    }
    let id;
    if (
      url.pathname.startsWith('/api/trackManifests/') ||
      url.pathname.startsWith('/api/videoManifests/')
    )
      id = url.pathname.split('/').at(-1);
    // Held requests wait for /control/release; ids keep scenarios apart.
    if (url.pathname === '/media/hold.mpd') id = url.searchParams.get('id');
    if (id === 'fetch-stall-body') {
      // Sends the headers and part of the body, then stalls.
      response.writeHead(200, { 'Content-Type': 'application/json' });
      response.write('{');
      return;
    }
    if (id) {
      if (id.startsWith('fetch-hold') || url.pathname === '/media/hold.mpd') {
        held.set(id, response);
        record('request', id);
        response.on('close', () => {
          if (!response.writableEnded) record('abort', id);
        });
        return;
      }
      json(manifest(id));
      return;
    }
    if (url.pathname === '/') {
      response.setHeader('Content-Type', 'text/html');
      response.end(
        '<title>Web player adapter verification</title><pre id="results">Running real generated bindings and Shaka…\n</pre>',
      );
      return;
    }
    const path = resolve(root, '.' + url.pathname);
    if (!path.startsWith(root + '/')) {
      response.writeHead(403);
      response.end();
      return;
    }
    try {
      const data = await readFile(path);
      response.setHeader(
        'Content-Type',
        {
          '.js': 'application/javascript',
          '.mjs': 'application/javascript',
          '.wasm': 'application/wasm',
          '.mpd': 'application/dash+xml',
          '.m4s': 'video/mp4',
          '.mp4': 'video/mp4',
        }[extname(path)] ?? 'application/octet-stream',
      );
      response.setHeader('Accept-Ranges', 'bytes');
      const range = request.headers.range?.match(/^bytes=(\d+)-(\d*)$/);
      if (range) {
        const start = Number(range[1]);
        const end = range[2]
          ? Math.min(Number(range[2]), data.length - 1)
          : data.length - 1;
        if (start > end) {
          response.writeHead(416, {
            'Content-Range': `bytes */${data.length}`,
          });
          response.end();
        } else {
          response.writeHead(206, {
            'Content-Range': `bytes ${start}-${end}/${data.length}`,
            'Content-Length': end - start + 1,
          });
          response.end(data.subarray(start, end + 1));
        }
      } else {
        response.setHeader('Content-Length', data.length);
        response.end(data);
      }
    } catch {
      response.writeHead(404);
      response.end();
    }
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  origin = `http://127.0.0.1:${server.address().port}`;
  return {
    origin,
    close: () =>
      new Promise(resolve => {
        server.closeAllConnections();
        server.close(resolve);
      }),
  };
}
