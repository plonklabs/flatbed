import { FlatbedError } from "./error.js";
import { fetchTransport, type Transport } from "./transport.js";

/** The `Content-Type`/`Accept` for the FlatBuffer wire format (the default). */
export const FLATBUFFERS_CONTENT_TYPE = "application/x-flatbuffers";
/** The `Content-Type`/`Accept` for the JSON wire format. */
export const JSON_CONTENT_TYPE = "application/json";

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
 * response bytes back into the typed value. `contentType` selects the wire codec
 * for both directions. Owns the flatbed wire rules — the GET/HEAD no-body
 * constraint, baseUrl joining, and `FlatbedError` mapping — so a generated
 * method is just a one-line call.
 */
export const request = <T>(
  config: ClientConfig,
  method: string,
  path: string,
  body: Uint8Array,
  decode: (bytes: Uint8Array) => T,
  contentType: string,
): Promise<T> => {
  const transport = config.transport ?? fetchTransport(config.fetch);
  // The server selects its response codec from the request `Content-Type`, not
  // `Accept`, so it's sent on every request, including bodyless GET/HEAD.
  const headers: Record<string, string> = {
    ...config.headers,
    accept: contentType,
    "content-type": contentType,
  };
  // GET/HEAD carry no body (browser `fetch` rejects it).
  const sendBody = method === "GET" || method === "HEAD" ? undefined : body;
  return transport
    .send({ method, url: config.baseUrl.replace(/\/+$/, "") + path, headers, body: sendBody })
    .then((res) => {
      if (!res.ok) throw new FlatbedError(res.status, `${method} ${path} → ${res.status}`);
      return decode(res.body);
    });
};
