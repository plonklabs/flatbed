/** The server's identity for a failed request, read from its error response. */
export interface FlatbedErrorIdentity {
  /** The server's error code (`x-error-code`, or a JSON body's `code`). */
  readonly code?: string;
  /** The server's error message (`x-error-message`, or a JSON body's `message`). */
  readonly serverMessage?: string;
  /** The request id the server logged and reported the failure under (`x-request-id`). */
  readonly requestId?: string;
}

/** Thrown when a flatbed service answers with a non-2xx status. */
export class FlatbedError extends Error {
  readonly code: string | undefined;
  readonly serverMessage: string | undefined;
  readonly requestId: string | undefined;

  constructor(
    readonly status: number,
    message: string,
    identity: FlatbedErrorIdentity = {},
  ) {
    super(message);
    this.name = "FlatbedError";
    this.code = identity.code;
    this.serverMessage = identity.serverMessage;
    this.requestId = identity.requestId;
  }
}
