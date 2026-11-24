import {
  HttpMethod,
  TransportError,
  type HttpRequest,
  type HttpResponse,
  type HttpTransport,
} from '../generated/player';

// Fetch never times out on its own, and the core retries timeouts.
const TIMEOUT_MS = 5_000;

/** The core owns bearer authentication and retry policy. */
export class FetchHttpTransport implements HttpTransport {
  async execute(
    request: HttpRequest,
    options?: { signal: AbortSignal },
  ): Promise<HttpResponse> {
    const timeout = AbortSignal.timeout(TIMEOUT_MS);
    try {
      const response = await fetch(request.url, {
        method: HttpMethod[request.method]?.toUpperCase(),
        headers: Object.fromEntries(request.headers),
        body: request.body,
        signal: options ? AbortSignal.any([options.signal, timeout]) : timeout,
      });
      return {
        statusCode: response.status,
        body: await response.arrayBuffer(),
      };
    } catch (error) {
      const description =
        error instanceof Error ? error.message : String(error);
      if (timeout.aborted && !options?.signal.aborted)
        throw new TransportError.TimeoutError({ description });
      throw new TransportError.NetworkError({ description });
    }
  }
}
