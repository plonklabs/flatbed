import type { FbsSchema, Operation } from "./model.js";
import { camelCase, isValidTsIdentifier, paramIdent, pascalCase, pathParams } from "./identifiers.js";
import { HEADER } from "./util.js";

/** GET/HEAD never carry a body, so their request type is dropped. */
const effectiveRequestType = (op: Operation): string | undefined =>
  op.method === "GET" || op.method === "HEAD" ? undefined : op.requestType;

/** The camelCased operationId when present, else `<method><PascalPathSegments>`. */
export const methodName = (op: Operation): string => {
  if (op.operationId !== undefined) return camelCase(op.operationId);
  const segments = op.path.split("/").filter((s) => s.length > 0);
  const word = (seg: string): string =>
    seg.startsWith("{") && seg.endsWith("}") ? `By${pascalCase(seg.slice(1, -1))}` : pascalCase(seg);
  return op.method.toLowerCase() + segments.map(word).join("");
};

/** Throw if any operation derives a method or path-param name that won't compile. */
export const checkNames = (ops: readonly Operation[]): void => {
  const seen = new Map<string, string>();
  ops.forEach((op) => {
    const name = methodName(op);
    const sig = `${op.method} ${op.path}`;
    if (!isValidTsIdentifier(name)) {
      throw new Error(
        `operation \`${sig}\` derives the client method name \`${name}\`, which is not a valid TypeScript identifier — set an explicit operationId`,
      );
    }
    const prev = seen.get(name);
    if (prev !== undefined) {
      throw new Error(
        `two operations derive the same client method \`${name}\`: \`${prev}\` and \`${sig}\` — give one an explicit operationId to disambiguate`,
      );
    }
    seen.set(name, sig);

    const keys = new Set<string>();
    pathParams(op.path).forEach((param) => {
      const key = paramIdent(param);
      if (keys.has(key)) {
        throw new Error(
          `operation \`${sig}\` has two path parameters that map to the key \`${key}\` — rename one in the spec`,
        );
      }
      keys.add(key);
    });
  });
};

/** Throw if an operation references a request/response type absent from the schema. */
export const checkTypes = (schema: FbsSchema, ops: readonly Operation[]): void => {
  const tables = new Set(schema.tables.map((t) => t.name));
  ops.forEach((op) => {
    [effectiveRequestType(op), op.responseType].forEach((name) => {
      if (name !== undefined && !tables.has(name)) {
        throw new Error(
          `operation ${op.method} ${op.path} references type '${name}', which is not a table in the FlatBuffer schema`,
        );
      }
    });
  });
};

/** The `args` object type — only the keys the operation actually has. */
const argType = (op: Operation, params: readonly string[]): string | undefined => {
  const req = effectiveRequestType(op);
  const parts = [
    ...(req !== undefined ? [`body: ${req}`] : []),
    ...(params.length > 0
      ? [`pathParams: { ${params.map((p) => `${paramIdent(p)}: string`).join("; ")} }`]
      : []),
  ];
  return parts.length > 0 ? `{ ${parts.join("; ")} }` : undefined;
};

// Build the request path as concatenated pieces: each static chunk is
// JSON.stringify'd (so a stray quote/backslash/backtick in an untrusted spec
// path can't break the emitted literal) and each param is URL-encoded — a `/`,
// `?`, `#`, `&`, or space in a value would otherwise misroute the URL. Only a
// whole `/`-delimited `{param}` segment is a parameter; a brace inside a larger
// segment stays literal, so the emitted path and the argument object always
// agree on which segments are parameters.
const pathExpr = (op: Operation): string => {
  const params = new Set(pathParams(op.path));
  const expr = op.path
    .split(/\{([^}]+)\}/u)
    .map((part, i) => {
      if (i % 2 === 0) return JSON.stringify(part);
      return params.has(part)
        ? `encodeURIComponent(args.pathParams.${paramIdent(part)})`
        : JSON.stringify(`{${part}}`);
    })
    .filter((piece) => piece !== '""')
    .join(" + ");
  return expr === "" ? '""' : expr;
};

const method = (op: Operation): string => {
  const params = pathParams(op.path);
  const req = effectiveRequestType(op);
  const arg = argType(op, params);
  const returns = op.responseType ?? "Uint8Array";
  const body = req !== undefined ? `codec.encode${req}Root(args.body)` : "new Uint8Array()";
  const decode =
    op.responseType !== undefined ? `codec.decode${op.responseType}Root` : "(bytes: Uint8Array) => bytes";
  return (
    `  ${methodName(op)}: (${arg !== undefined ? `args: ${arg}` : ""}): Promise<${returns}> =>\n` +
    `    request(config, "${op.method}", ${pathExpr(op)}, ${body}, ${decode}),\n`
  );
};

/**
 * Emit a `createFlatbedClient(config)` factory whose object has one method per
 * operation. Method names are emitted verbatim, so the caller must have already
 * validated them (unique, valid identifiers).
 */
export const emitClient = (ops: readonly Operation[]): string => {
  const importTypes = [
    ...new Set(
      ops.flatMap((op) => [effectiveRequestType(op), op.responseType].filter((t) => t !== undefined)),
    ),
  ].sort();
  const usesCodec = ops.some((op) => effectiveRequestType(op) !== undefined || op.responseType !== undefined);
  return (
    HEADER +
    'import { request, type ClientConfig } from "@plonklabs/flatbed-client";\n' +
    (usesCodec ? 'import * as codec from "./codec.js";\n' : "") +
    (importTypes.length > 0 ? `import type { ${importTypes.join(", ")} } from "./types.js";\n` : "") +
    "\nexport const createFlatbedClient = (config: ClientConfig) => ({\n" +
    ops.map(method).join("") +
    "});\n"
  );
};

/** Emit the barrel module re-exporting the generated folder. */
export const emitIndex = (): string =>
  HEADER +
  'export * from "./types.js";\n' +
  'export * from "./client.js";\n' +
  'export * as codec from "./codec.js";\n';
