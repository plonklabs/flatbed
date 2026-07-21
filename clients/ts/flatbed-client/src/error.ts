/** Thrown when a flatbed service answers with a non-2xx status. */
export class FlatbedError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
    this.name = "FlatbedError";
  }
}
