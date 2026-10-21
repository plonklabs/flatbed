import { defaultLiteral, flagsDefault } from "./emit-json.js";
import type { FbsEnum, FbsField, FbsSchema, FbsTable, Operation } from "./model.js";
import { camelCase, isValidTsIdentifier, paramIdent, pascalCase, pathParams } from "./identifiers.js";
import { HEADER } from "./util.js";

/** GET/HEAD never carry a body, so their request type is dropped. */
export const effectiveRequestType = (op: Operation): string | undefined =>
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

/** The table a paginated response carries its cursors in. */
const PAGE_TABLE = "Page";

/**
 * How a list operation maps onto a `Page`: its request table, and the response
 * fields holding the items, the cursors, and (optionally) the applied request.
 */
interface ListShape {
  readonly request: FbsTable;
  readonly items: { readonly field: string; readonly type: string };
  readonly page: string;
  readonly applied?: { readonly field: string; readonly type: string };
}

const tableOf = (f: FbsField): string | undefined => (f.type.kind === "table" ? f.type.name : undefined);

const itemsOf = (f: FbsField): string | undefined =>
  f.type.kind === "vector" && f.type.element.kind === "table" ? f.type.element.name : undefined;

const hasPageField = (t: FbsTable): boolean => t.fields.some((f) => tableOf(f) === PAGE_TABLE);

/**
 * The list shape of an operation whose request and response are both
 * `(paginated)` tables — a paginated table with a `Page` field is a response,
 * any other paginated table a request — or `undefined` for any other operation.
 */
const listShape = (schema: FbsSchema, op: Operation): ListShape | undefined => {
  const tables = new Map(schema.tables.map((t) => [t.name, t]));
  const isRequest = (t: FbsTable | undefined): t is FbsTable => t?.paginated === true && !hasPageField(t);
  const request = tables.get(effectiveRequestType(op) ?? "");
  const response = tables.get(op.responseType ?? "");
  if (!isRequest(request) || response?.paginated !== true) return undefined;
  const items = response.fields.find((f) => itemsOf(f) !== undefined);
  const page = response.fields.find((f) => tableOf(f) === PAGE_TABLE);
  const applied = response.fields.find((f) => isRequest(tables.get(tableOf(f) ?? "")));
  if (items === undefined || page === undefined) return undefined;
  return {
    request,
    items: { field: items.name, type: itemsOf(items)! },
    page: page.name,
    ...(applied !== undefined ? { applied: { field: applied.name, type: tableOf(applied)! } } : {}),
  };
};

/** The `args` object type — only the keys the operation actually has. */
const argType = (op: Operation, params: readonly string[], list: boolean): string | undefined => {
  const req = effectiveRequestType(op);
  const parts = [
    ...(req !== undefined ? [list ? `body: Omit<${req}, "page">` : `body: ${req}`] : []),
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

const call = (op: Operation, fmt: "fb" | "json", body = "args.body"): string => {
  const req = effectiveRequestType(op);
  const encode =
    req !== undefined
      ? fmt === "json"
        ? `json.encode${req}Json(${body})`
        : `codec.encode${req}Root(${body})`
      : "new Uint8Array()";
  const decode =
    op.responseType !== undefined
      ? fmt === "json"
        ? `json.decode${op.responseType}Json`
        : `codec.decode${op.responseType}Root`
      : "(bytes: Uint8Array) => bytes";
  const contentType = fmt === "json" ? "JSON_CONTENT_TYPE" : "FLATBUFFERS_CONTENT_TYPE";
  return `request(config, "${op.method}", ${pathExpr(op)}, ${encode}, ${decode}, ${contentType})`;
};

const OPTS = 'opts?: { as?: "json" | "flatbuffer" }';

// Optional kinds (strings, tables, vectors, unions) have no default to send.
const fieldDefault = (enums: ReadonlyMap<string, FbsEnum>, f: FbsField): string | undefined => {
  if (f.type.kind === "enum") return `${defaultLiteral(f)} as ${f.type.name}`;
  if (f.type.kind === "flags") return flagsDefault(enums, f);
  return f.type.kind === "scalar" ? defaultLiteral(f) : undefined;
};

const filterFields = (shape: ListShape): readonly FbsField[] =>
  shape.request.fields.filter((f) => f.name !== "page" && f.name !== "page_size");

/** The enum types a list call's cursor body names in a cast. */
const cursorEnums = (shape: ListShape): readonly string[] =>
  filterFields(shape).flatMap((f) => (f.type.kind === "enum" ? [f.type.name] : []));

// The body a cursor is sent in: `page` and the caller's `page_size`, every
// other field at its schema default. The server rejects a cursor sent with any
// filter set, and JSON requires the numeric fields to be present.
const cursorBody = (schema: FbsSchema, shape: ListShape): string => {
  const enums = new Map(schema.enums.map((e) => [e.name, e]));
  const rest = filterFields(shape).flatMap((f) => {
    const value = fieldDefault(enums, f);
    return value !== undefined ? [`${f.name}: ${value}`] : [];
  });
  return `{ ${["page", "page_size: args.body.page_size", ...rest].join(", ")} }`;
};

const listMethod = (schema: FbsSchema, op: Operation, shape: ListShape): string => {
  const sig = [`args: ${argType(op, pathParams(op.path), true)!}`, ...(op.supportsJson ? [OPTS] : [])].join(", ");
  const generics = `${shape.items.type}, ${shape.applied?.type ?? "never"}`;
  const send = op.supportsJson
    ? `(opts?.as === "json"\n` +
      `          ? ${call(op, "json", "body")}\n` +
      `          : ${call(op, "fb", "body")})`
    : call(op, "fb", "body");
  const parts = [
    `items: res.${shape.items.field}`,
    `page: res.${shape.page}`,
    ...(shape.applied !== undefined ? [`applied: res.${shape.applied.field}`] : []),
  ].join(", ");
  return (
    `  ${methodName(op)}: (${sig}): Promise<FlatbedPage<${generics}>> =>\n` +
    `    paginate<${shape.request.name}, ${generics}>(\n` +
    `      (body) =>\n` +
    `        ${send}.then((res) => ({ ${parts} })),\n` +
    `      args.body,\n` +
    `      (page) => (${cursorBody(schema, shape)}),\n` +
    `    ),\n`
  );
};

const method =
  (schema: FbsSchema) =>
  (op: Operation): string => {
    const shape = listShape(schema, op);
    if (shape !== undefined) return listMethod(schema, op, shape);
    const arg = argType(op, pathParams(op.path), false);
    const returns = op.responseType ?? "Uint8Array";
    const name = methodName(op);
    if (!op.supportsJson) {
      return `  ${name}: (${arg !== undefined ? `args: ${arg}` : ""}): Promise<${returns}> =>\n    ${call(op, "fb")},\n`;
    }
    const sig = [arg !== undefined ? `args: ${arg}` : "", OPTS].filter((s) => s !== "").join(", ");
    return (
      `  ${name}: (${sig}): Promise<${returns}> =>\n` +
      `    opts?.as === "json"\n` +
      `      ? ${call(op, "json")}\n` +
      `      : ${call(op, "fb")},\n`
    );
  };

/**
 * Emit a `createFlatbedClient(config)` factory whose object has one method per
 * operation; an operation whose request and response are `(paginated)` tables
 * becomes a list call resolving to a `Page`. Method names are emitted verbatim,
 * so the caller must have already validated them (unique, valid identifiers).
 */
export const emitClient = (ops: readonly Operation[], schema: FbsSchema): string => {
  const shapes = ops.flatMap((op) => listShape(schema, op) ?? []);
  // A list call names its request, item, applied, and cursor-enum types, but
  // never its response type.
  const namedTypes = (op: Operation): readonly string[] => {
    const shape = listShape(schema, op);
    if (shape === undefined) return [effectiveRequestType(op), op.responseType].filter((t) => t !== undefined);
    return [shape.request.name, shape.items.type, ...(shape.applied !== undefined ? [shape.applied.type] : []), ...cursorEnums(shape)];
  };
  const importTypes = [...new Set(ops.flatMap(namedTypes))].sort();
  const typed = (op: Operation): boolean =>
    effectiveRequestType(op) !== undefined || op.responseType !== undefined;
  const usesCodec = ops.some(typed);
  const anyJson = ops.some((op) => op.supportsJson);
  const usesJsonCodec = ops.some((op) => op.supportsJson && typed(op));
  // With no operations only `ClientConfig` is used, so importing `request`/the
  // content-type constants would trip `noUnusedLocals` in the generated file.
  const hasOps = ops.length > 0;
  const runtimeImports = [
    ...(hasOps ? ["request"] : []),
    "type ClientConfig",
    ...(hasOps ? ["FLATBUFFERS_CONTENT_TYPE"] : []),
    ...(anyJson ? ["JSON_CONTENT_TYPE"] : []),
    // Aliased: the schema declares its own `Page` table.
    ...(shapes.length > 0 ? ["paginate", "type Page as FlatbedPage"] : []),
  ];
  return (
    HEADER +
    `import { ${runtimeImports.join(", ")} } from "@plonklabs/flatbed-client";\n` +
    (usesCodec ? 'import * as codec from "./codec.js";\n' : "") +
    (usesJsonCodec ? 'import * as json from "./json-codec.js";\n' : "") +
    (importTypes.length > 0 ? `import type { ${importTypes.join(", ")} } from "./types.js";\n` : "") +
    "\nexport const createFlatbedClient = (config: ClientConfig) => ({\n" +
    ops.map(method(schema)).join("") +
    "});\n"
  );
};

/** Emit the barrel module re-exporting the generated folder. */
export const emitIndex = (): string =>
  HEADER +
  'export * from "./types.js";\n' +
  'export * from "./client.js";\n' +
  'export * as codec from "./codec.js";\n' +
  'export * as json from "./json-codec.js";\n';
