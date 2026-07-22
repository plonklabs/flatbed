import { FlatbedError } from "./error.js";
import { fetchTransport, type Transport } from "./transport.js";

const CONTENT_TYPE = "application/x-flatbuffers";

/** How a generated client talks to a flatbed service. */
export interface ClientConfig {
  /** Base URL of the flatbed service; a request path is appended to it. */
  readonly baseUrl: string;
  /** Override the HTTP layer (axios, auth, retries, …). Defaults to `fetch`. */
  readonly transport?: Transport;
  /** A custom `fetch` for the default transport; ignored when `transport` is set. */
  readonly fetch?: typeof globalThis.fetch;
  /**
   * Extra headers merged into every request (auth tokens, tracing, …). The
   * framework sets the lowercase `accept` and `content-type` keys after this
   * merge, so a same-cased entry here is replaced.
   */
  readonly headers?: Readonly<Record<string, string>>;
}

/**
 * Perform one operation: encode is already done (`body`), and `decode` turns the
 * response bytes back into the typed value. Owns the flatbed wire rules — the
 * `application/x-flatbuffers` content type, the GET/HEAD no-body constraint,
 * baseUrl joining, and `FlatbedError` mapping — so a generated method is just a
 * one-line call.
 */
export const request = <T>(
  config: ClientConfig,
  method: string,
  path: string,
  body: Uint8Array,
  decode: (bytes: Uint8Array) => T,
): Promise<T> => {
  const transport = config.transport ?? fetchTransport(config.fetch);
  const headers: Record<string, string> = { ...config.headers, accept: CONTENT_TYPE };
  // GET/HEAD carry no body (browser `fetch` rejects it). Every other method sends
  // a Content-Type even with an empty body — the server answers 415 to a
  // POST/PUT/PATCH that arrives without one.
  const sendBody = method === "GET" || method === "HEAD" ? undefined : body;
  if (sendBody !== undefined) {
    headers["content-type"] = CONTENT_TYPE;
  }
  return transport
    .send({ method, url: config.baseUrl.replace(/\/+$/, "") + path, headers, body: sendBody })
    .then((res) => {
      if (!res.ok) throw new FlatbedError(res.status, `${method} ${path} → ${res.status}`);
      return decode(res.body);
    });
};
