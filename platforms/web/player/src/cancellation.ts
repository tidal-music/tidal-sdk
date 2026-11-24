export async function abortable<T>(
  promise: Promise<T>,
  signal?: AbortSignal,
): Promise<T> {
  if (!signal) return promise;
  signal.throwIfAborted();
  let abort: () => void = () => {};
  const cancelled = new Promise<never>((_, reject) => {
    abort = () => reject(signal.reason);
    signal.addEventListener('abort', abort, { once: true });
  });
  try {
    const value = await Promise.race([promise, cancelled]);
    signal.throwIfAborted();
    return value;
  } finally {
    signal.removeEventListener('abort', abort);
  }
}
