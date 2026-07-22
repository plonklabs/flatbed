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

/** The `content` of the operation's lowest success (2xx) response code. */
const successContent = (op: Record<string, unknown>): unknown => {
  const responses = asObject(op["responses"]) ?? {};
  // Lowest 2xx, so the choice doesn't depend on the spec author's key order.
  const [code] = Object.keys(responses)
    .filter((c) => c.startsWith("2"))
    .sort();
  return code !== undefined ? asObject(responses[code])?.["content"] : undefined;
};

const operationFrom = (method: string, path: string, op: Record<string, unknown>): Operation | undefined => {
  const request = asObject(op["requestBody"])?.["content"];
  const response = successContent(op);
  if (!advertisesFlatbuffers(request) && !advertisesFlatbuffers(response)) return undefined;
  const operationId = op["operationId"];
  return {
    method: method.toUpperCase(),
    path,
    operationId: typeof operationId === "string" ? operationId : undefined,
    // Bind a codec type only for a direction that speaks flatbuffers, so a
    // JSON-only request/response never gets a flatbuffer encoder/decoder.
    requestType: advertisesFlatbuffers(request) ? jsonRef(request) : undefined,
    responseType: advertisesFlatbuffers(response) ? jsonRef(response) : undefined,
  };
};

/** The `application/x-flatbuffers` operations advertised by a parsed OpenAPI spec. */
export const readOperations = (spec: unknown): readonly Operation[] =>
  Object.entries(asObject(asObject(spec)?.["paths"]) ?? {}).flatMap(([path, item]) =>
    Object.entries(asObject(item) ?? {})
      .filter(([method]) => HTTP_METHODS.has(method))
      .flatMap(([method, op]) => {
        const parsed = asObject(op);
        const found = parsed !== undefined ? operationFrom(method, path, parsed) : undefined;
        return found !== undefined ? [found] : [];
      }),
  );
