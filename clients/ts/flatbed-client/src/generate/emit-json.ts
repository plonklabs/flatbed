import type { FbsField, FbsSchema, FbsTable, FbsType } from "./model.js";
import { HEADER } from "./util.js";

// The JSON wire shape is the server's serde output: enums are variant-name
// strings and 64-bit ints are JSON numbers, so values above 2^53 lose precision
// on this path — a limit of the JSON number type.

const is64 = (t: FbsType): boolean =>
  t.kind === "scalar" && (t.scalar === "int64" || t.scalar === "uint64");

/** A TS value expression → its JSON-wire form. */
const toWire = (t: FbsType, expr: string): string => {
  if (t.kind === "enum") return `${t.name}[${expr}]`;
  if (t.kind === "table") return `toWire${t.name}(${expr})`;
  if (t.kind === "vector") {
    const elem = toWire(t.element, "x");
    return elem === "x" ? expr : `${expr}.map((x) => ${elem})`;
  }
  return is64(t) ? `Number(${expr})` : expr;
};

const passthroughTs = (t: FbsType): string =>
  t.kind === "string" ? "string" : t.kind === "scalar" && t.scalar === "bool" ? "boolean" : "number";

const fromWire = (t: FbsType, expr: string): string => {
  if (t.kind === "enum") return `${t.name}[${expr} as keyof typeof ${t.name}]`;
  if (t.kind === "table") return `fromWire${t.name}(${expr} as Record<string, unknown>)`;
  if (t.kind === "vector") return `(${expr} as unknown[]).map((x) => ${fromWire(t.element, "x")})`;
  return is64(t) ? `BigInt(${expr} as number)` : `${expr} as ${passthroughTs(t)}`;
};

// String/table/vector fields are optional; guard so an absent one stays `undefined` (which `JSON.stringify` drops).
const optional = (t: FbsType): boolean =>
  t.kind === "string" || t.kind === "table" || t.kind === "vector";

// A scalar/enum absent from the JSON decodes to its declared default — the same
// value the FlatBuffer path yields for an omitted field, and it keeps
// `BigInt(undefined)` from throwing on a missing 64-bit field.
const defaultLiteral = (f: FbsField): string => {
  const { type: t, default: d } = f;
  if (t.kind === "scalar" && t.scalar === "bool") return d.kind === "int" && d.value !== 0n ? "true" : "false";
  if (d.kind === "int") return is64(t) ? `${d.value}n` : `${d.value}`;
  if (d.kind === "real") return `${d.value}`;
  return "undefined";
};

const toWireField = (f: FbsField): string => {
  const v = `value.${f.name}`;
  const conv = toWire(f.type, v);
  return optional(f.type) && conv !== v ? `${v} != null ? ${conv} : undefined` : conv;
};

const fromWireField = (f: FbsField): string => {
  const v = `obj.${f.name}`;
  const conv = fromWire(f.type, v);
  if (optional(f.type)) return conv !== v ? `${v} != null ? ${conv} : undefined` : conv;
  return `${v} != null ? ${conv} : ${defaultLiteral(f)}`;
};

// Emitted as hoisted `function` declarations, not arrows: a table's toWire/
// fromWire may call a nested table's before it's defined, since schema order
// isn't dependency-ordered and table references can be mutual.
const toWireFn = (t: FbsTable): string =>
  `function toWire${t.name}(value: ${t.name}): unknown {\n  return {\n` +
  t.fields.map((f) => `    ${f.name}: ${toWireField(f)},\n`).join("") +
  "  };\n}\n\n";

const fromWireFn = (t: FbsTable): string =>
  `function fromWire${t.name}(obj: Record<string, unknown>): ${t.name} {\n  return {\n` +
  t.fields.map((f) => `    ${f.name}: ${fromWireField(f)},\n`).join("") +
  "  };\n}\n\n";

const rootFns = (t: FbsTable): string =>
  `export function encode${t.name}Json(value: ${t.name}): Uint8Array {\n` +
  `  return new TextEncoder().encode(JSON.stringify(toWire${t.name}(value)));\n}\n\n` +
  `export function decode${t.name}Json(bytes: Uint8Array): ${t.name} {\n` +
  `  return fromWire${t.name}(JSON.parse(new TextDecoder().decode(bytes)) as Record<string, unknown>);\n}\n\n`;

// Enums need a value import: used as `Name[...]` at runtime.
const referencedEnums = (schema: FbsSchema): ReadonlySet<string> => {
  const names = new Set<string>();
  const visit = (t: FbsType): void => {
    if (t.kind === "enum") names.add(t.name);
    if (t.kind === "vector") visit(t.element);
  };
  schema.tables.forEach((table) => table.fields.forEach((f) => visit(f.type)));
  return names;
};

/** Emit per-table JSON encode/decode that matches the server's serde wire shape. */
export const emitJson = (schema: FbsSchema): string => {
  const enums = schema.enums.filter((e) => referencedEnums(schema).has(e.name)).map((e) => e.name);
  const typeImports = schema.tables.map((t) => t.name);
  return (
    HEADER +
    (enums.length > 0 ? `import { ${enums.join(", ")} } from "./types.js";\n` : "") +
    (typeImports.length > 0 ? `import type { ${typeImports.join(", ")} } from "./types.js";\n` : "") +
    "\n" +
    schema.tables.map((t) => toWireFn(t) + fromWireFn(t) + rootFns(t)).join("")
  );
};
