import { FlatbedError } from "./error.js";
import { fetchTransport, type Transport } from "./transport.js";

const CONTENT_TYPE = "application/x-flatbuffers";

/** Options for constructing a {@link FlatbedClient}. */
export interface ClientOptions {
  /** Base URL of the flatbed service; a request path is appended to it. */
  baseUrl: string;
  /** Override the HTTP layer (axios, auth, retries, …). Defaults to `fetch`. */
  transport?: Transport;
  /** A custom `fetch` for the default transport; ignored when `transport` is set. */
  fetch?: typeof globalThis.fetch;
}

/**
 * The base a generated `client.ts` extends. It owns the flatbed wire rules — the
 * `application/x-flatbuffers` content type, the GET/HEAD no-body constraint,
 * baseUrl joining, and error mapping — so a generated client is only per-route
 * method bindings.
 */
export class FlatbedClient {
  private readonly baseUrl: string;
  private readonly transport: Transport;

  constructor(options: ClientOptions) {
    this.baseUrl = options.baseUrl.replace(/\/+$/, "");
    this.transport = options.transport ?? fetchTransport(options.fetch);
  }

  protected async request<T>(
    method: string,
    path: string,
    body: Uint8Array,
    decode: (bytes: Uint8Array) => T,
  ): Promise<T> {
    const headers: Record<string, string> = { accept: CONTENT_TYPE };
    // GET/HEAD carry no body (browser `fetch` rejects it). Every other method
    // sends a Content-Type even with an empty body — the server answers 415 to a
    // POST/PUT/PATCH that arrives without one.
    let sendBody: Uint8Array | undefined;
    if (method !== "GET" && method !== "HEAD") {
      headers["content-type"] = CONTENT_TYPE;
      sendBody = body;
    }
    const res = await this.transport.send({
      method,
      url: this.baseUrl + path,
      headers,
      body: sendBody,
    });
    if (!res.ok) {
      throw new FlatbedError(res.status, `${method} ${path} → ${res.status}`);
    }
    return decode(res.body);
  }
}
