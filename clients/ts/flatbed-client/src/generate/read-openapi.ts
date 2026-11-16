import type { Operation } from "./model.js";

const HTTP_METHODS = new Set(["get", "put", "post", "delete", "patch", "head", "options"]);

const REF_PREFIX = "#/components/schemas/";

const asObject = (v: unknown): Record<string, unknown> | undefined =>
  typeof v === "object" && v !== null && !Array.isArray(v) ? (v as Record<string, unknown>) : undefined;

/** The bare component name a `$ref` schema points at, else undefined. */
const refName = (schema: unknown): string | undefined => {
  const ref = asObject(schema)?.["$ref"];
  return typeof ref === "string" && ref.startsWith(REF_PREFIX) ? ref.slice(REF_PREFIX.length) : undefined;
};

// The type name comes from the JSON content's `$ref`; the x-flatbuffers entry
// carries an inline binary schema, so it has no ref to read.
const jsonRef = (content: unknown): string | undefined =>
  refName(asObject(asObject(content)?.["application/json"])?.["schema"]);

const advertisesFlatbuffers = (content: unknown): boolean =>
  asObject(content)?.["application/x-flatbuffers"] !== undefined;

const advertisesJson = (content: unknown): boolean =>
  asObject(content)?.["application/json"] !== undefined;

/** The `content` of the operation's lowest success (2xx) response code. */
const successContent = (op: Record<string, unknown>): unknown => {
  const responses = asObject(op["responses"]) ?? {};
  // Lowest 2xx, so the choice doesn't depend on the spec author's key order.
  const [code] = Object.keys(responses)
    .filter((c) => c.startsWith("2"))
    .sort();
  return code !== undefined ? asObject(responses[code])?.["content"] : undefined;
};

const WIRE_MEDIA_TYPES = new Set(["application/json", "application/x-flatbuffers"]);

/** The media types the operation's success response sends besides JSON and FlatBuffers. */
const foreignMediaTypes = (op: Record<string, unknown>): readonly string[] =>
  Object.keys(asObject(successContent(op)) ?? {}).filter((type) => !WIRE_MEDIA_TYPES.has(type));

const operationFrom = (method: string, path: string, op: Record<string, unknown>): Operation | undefined => {
  const request = asObject(op["requestBody"])?.["content"];
  const response = successContent(op);
  if (foreignMediaTypes(op).length > 0) return undefined;
  if (!advertisesFlatbuffers(request) && !advertisesFlatbuffers(response)) return undefined;
  const operationId = op["operationId"];
  const supportsJson =
    (request === undefined || advertisesJson(request)) &&
    (response === undefined || advertisesJson(response));
  return {
    method: method.toUpperCase(),
    path,
    operationId: typeof operationId === "string" ? operationId : undefined,
    // Bind a codec type only for a direction that speaks flatbuffers, so a
    // JSON-only request/response never gets a flatbuffer encoder/decoder.
    requestType: advertisesFlatbuffers(request) ? jsonRef(request) : undefined,
    responseType: advertisesFlatbuffers(response) ? jsonRef(response) : undefined,
    supportsJson,
  };
};

/** Every operation of a parsed OpenAPI spec, as `[method, path, operation]`. */
const allOperations = (spec: unknown): readonly (readonly [string, string, Record<string, unknown>])[] =>
  Object.entries(asObject(asObject(spec)?.["paths"]) ?? {}).flatMap(([path, item]) =>
    Object.entries(asObject(item) ?? {})
      .filter(([method]) => HTTP_METHODS.has(method))
      .flatMap(([method, op]) => {
        const parsed = asObject(op);
        return parsed !== undefined ? [[method, path, parsed] as const] : [];
      }),
  );

/** The `application/x-flatbuffers` operations advertised by a parsed OpenAPI spec. */
export const readOperations = (spec: unknown): readonly Operation[] =>
  allOperations(spec).flatMap(([method, path, op]) => {
    const found = operationFrom(method, path, op);
    return found !== undefined ? [found] : [];
  });

/**
 * The operations left out because their success response sends a media type
 * that is neither JSON nor FlatBuffers (a stream, say), named with what they send.
 */
export const readSkipped = (spec: unknown): readonly string[] =>
  allOperations(spec).flatMap(([method, path, op]) => {
    const foreign = foreignMediaTypes(op);
    return foreign.length > 0 ? [`${method.toUpperCase()} ${path} (sends ${foreign.join(", ")})`] : [];
  });
