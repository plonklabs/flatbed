/** A single HTTP round-trip the client hands a transport to perform. */
export interface FlatbedRequest {
  method: string;
  url: string;
  headers: Record<string, string>;
  /** Absent for GET/HEAD; the encoded request payload otherwise. */
  body?: Uint8Array;
}

/** A transport's reply: status, headers, and the raw response bytes. */
export interface FlatbedResponse {
  status: number;
  /** `true` iff the server returned a 2xx status; the client throws {@link FlatbedError} when this is `false`. */
  ok: boolean;
  /**
   * Response headers keyed by lowercase name. The client reads the error
   * identity headers from them; a transport that omits them yields a
   * {@link FlatbedError} without that identity.
   */
  headers?: Readonly<Record<string, string>>;
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

// `Headers.forEach` yields lowercased names.
const headerRecord = (headers: Headers): Record<string, string> => {
  const record: Record<string, string> = {};
  headers.forEach((value, name) => {
    record[name] = value;
  });
  return record;
};

/** The default transport, backed by `fetch`. */
export function fetchTransport(
  fetchImpl: typeof globalThis.fetch = globalThis.fetch,
): Transport {
  return {
    send: ({ method, url, headers, body }) =>
      fetchImpl(url, body === undefined ? { method, headers } : { method, headers, body: body as BodyInit }).then(
        (res) =>
          res.arrayBuffer().then((bytes) => ({
            status: res.status,
            ok: res.ok,
            headers: headerRecord(res.headers),
            body: new Uint8Array(bytes),
          })),
      ),
  };
}
