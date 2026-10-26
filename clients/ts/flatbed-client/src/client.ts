import { FlatbedError } from "./error.js";
import { fetchTransport, type FlatbedResponse, type Transport } from "./transport.js";

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
      if (!res.ok) throw toFlatbedError(res, method, path, contentType);
      return decode(res.body);
    });
};

const asString = (value: unknown): string | undefined => (typeof value === "string" ? value : undefined);

const parseJsonObject = (bytes: Uint8Array): Readonly<Record<string, unknown>> => {
  try {
    const value: unknown = JSON.parse(new TextDecoder().decode(bytes));
    return typeof value === "object" && value !== null ? (value as Record<string, unknown>) : {};
  } catch {
    return {};
  }
};

/**
 * A FlatBuffer route error carries its code and message in `x-error-code` /
 * `x-error-message`; a JSON error carries them as the body's `code` / `message`.
 * Framework-level failures (an undecodable request, say) answer in JSON whatever
 * the request's format, so the response's own `content-type` decides, falling
 * back to the request's when the transport reports no headers.
 */
const toFlatbedError = (res: FlatbedResponse, method: string, path: string, contentType: string): FlatbedError => {
  const headers = res.headers ?? {};
  const isJson = (headers["content-type"] ?? contentType).startsWith(JSON_CONTENT_TYPE);
  const body = isJson ? parseJsonObject(res.body) : {};
  return new FlatbedError(res.status, `${method} ${path} → ${res.status}`, {
    code: headers["x-error-code"] ?? asString(body.code),
    serverMessage: headers["x-error-message"] ?? asString(body.message),
    requestId: headers["x-request-id"],
  });
};
