/** A single HTTP round-trip the client hands a transport to perform. */
export interface FlatbedRequest {
  method: string;
  url: string;
  headers: Record<string, string>;
  /** Absent for GET/HEAD; the encoded request payload otherwise. */
  body?: Uint8Array;
}

/** A transport's reply: status plus the raw response bytes. */
export interface FlatbedResponse {
  status: number;
  /** `true` iff the server returned a 2xx status; the client throws {@link FlatbedError} when this is `false`. */
  ok: boolean;
  body: Uint8Array;
}

/**
 * The seam the client depends on for HTTP. Replace the default fetch transport
 * with one built on axios, or one that adds auth headers, retries, or
 * interceptors — without editing any generated code.
 */
export interface Transport {
  send(req: FlatbedRequest): Promise<FlatbedResponse>;
}

/** The default transport, backed by `fetch`. */
export function fetchTransport(
  fetchImpl: typeof globalThis.fetch = globalThis.fetch,
): Transport {
  return {
    async send({ method, url, headers, body }) {
      const init: RequestInit = { method, headers };
      if (body !== undefined) {
        init.body = body as BodyInit;
      }
      const res = await fetchImpl(url, init);
      return {
        status: res.status,
        ok: res.ok,
        body: new Uint8Array(await res.arrayBuffer()),
      };
    },
  };
}
