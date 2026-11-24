import { serve } from './server.mjs';

const cdp = process.argv[2];
if (!cdp)
  throw new Error(
    'Pass the DevTools HTTP URL of a running Chromium, e.g. http://127.0.0.1:9222.',
  );
const pages = await (await fetch(`${cdp}/json/list`)).json();
const target = pages.find(page => page.type === 'page');
if (!target) throw new Error('The browser has no page target');
const socket = new WebSocket(target.webSocketDebuggerUrl);
await new Promise(resolve =>
  socket.addEventListener('open', resolve, { once: true }),
);
let sequence = 0;
const pending = new Map();
const events = new Map();
socket.addEventListener('message', event => {
  const message = JSON.parse(event.data);
  if (message.id) {
    pending.get(message.id)?.(message);
    pending.delete(message.id);
  } else {
    events.get(message.method)?.(message.params);
  }
});
const call = (method, params = {}) =>
  new Promise((resolve, reject) => {
    const id = ++sequence;
    pending.set(id, message =>
      message.error
        ? reject(new Error(JSON.stringify(message.error)))
        : resolve(message.result),
    );
    socket.send(JSON.stringify({ id, method, params }));
  });
const server = await serve();
try {
  await call('Page.enable');
  await call('Runtime.enable');
  events.set('Runtime.consoleAPICalled', event =>
    console.log(
      event.type,
      event.args.map(arg => arg.value ?? arg.description).join(' '),
    ),
  );
  const loaded = new Promise(resolve =>
    events.set('Page.loadEventFired', resolve),
  );
  await call('Page.navigate', { url: server.origin });
  await loaded;
  const result = await call('Runtime.evaluate', {
    expression: `(async () => { const tests = await import('/tests/browser.mjs'); return await Promise.race([tests.run(), new Promise((_, reject) => setTimeout(() => reject(new Error('Browser verification exceeded 240 seconds')), 240000))]); })()`,
    awaitPromise: true,
    returnByValue: true,
    userGesture: true,
  });
  if (result.exceptionDetails)
    throw new Error(JSON.stringify(result.exceptionDetails));
  console.log(JSON.stringify(result.result.value, null, 2));
} finally {
  socket.close();
  await server.close();
}
