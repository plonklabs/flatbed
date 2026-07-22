import type { FbsField, FbsSchema, FbsTable, FbsType } from "./model.js";
import { HEADER } from "./util.js";

// The JSON wire shape mirrors the server's serde output, which differs from the
// TS type surface in two places: an enum is a variant-name string (not its
// number), and a 64-bit int is a JSON number (not a `bigint`). `toWire`/
// `fromWire` translate at exactly those fields; everything else passes through.
//
// 64-bit values above 2^53 lose precision on the JSON path — the server encodes
// them as JSON numbers too, so this is a property of the wire format, not the
// client. Use the FlatBuffer path when full 64-bit range matters.

const is64 = (t: FbsType): boolean =>
  t.kind === "scalar" && (t.scalar === "int64" || t.scalar === "uint64");

/** A TS value expression → its JSON-wire form. */
const toWire = (t: FbsType, expr: string): string => {
  if (t.kind === "enum") return `${t.name}[${expr}]`;
  if (t.kind === "table") return `toWire${t.name}(${expr})`;
  if (t.kind === "vector") return `${expr}.map((x) => ${toWire(t.element, "x")})`;
  return is64(t) ? `Number(${expr})` : expr;
};

/** A JSON-wire value expression → its TS form. */
const fromWire = (t: FbsType, expr: string): string => {
  if (t.kind === "enum") return `${t.name}[${expr} as keyof typeof ${t.name}]`;
  if (t.kind === "table") return `fromWire${t.name}(${expr})`;
  if (t.kind === "vector") return `${expr}.map((x: any) => ${fromWire(t.element, "x")})`;
  return is64(t) ? `BigInt(${expr})` : expr;
};

// A string/table/vector field is optional on the wire, so guard the transform on
// its presence; `JSON.stringify` then drops an `undefined` result. Scalars and
// enums are always present.
const optional = (t: FbsType): boolean =>
  t.kind === "string" || t.kind === "table" || t.kind === "vector";

const toWireField = (f: FbsField): string => {
  const v = `value.${f.name}`;
  const conv = toWire(f.type, v);
  return optional(f.type) && conv !== v ? `${v} != null ? ${conv} : undefined` : conv;
};

const fromWireField = (f: FbsField): string => {
  const v = `obj.${f.name}`;
  const conv = fromWire(f.type, v);
  return optional(f.type) && conv !== v ? `${v} != null ? ${conv} : undefined` : conv;
};

const toWireFn = (t: FbsTable): string =>
  `function toWire${t.name}(value: ${t.name}): unknown {\n  return {\n` +
  t.fields.map((f) => `    ${f.name}: ${toWireField(f)},\n`).join("") +
  "  };\n}\n\n";

const fromWireFn = (t: FbsTable): string =>
  `function fromWire${t.name}(obj: any): ${t.name} {\n  return {\n` +
  t.fields.map((f) => `    ${f.name}: ${fromWireField(f)},\n`).join("") +
  "  };\n}\n\n";

const rootFns = (t: FbsTable): string =>
  `export function encode${t.name}Json(value: ${t.name}): Uint8Array {\n` +
  `  return new TextEncoder().encode(JSON.stringify(toWire${t.name}(value)));\n}\n\n` +
  `export function decode${t.name}Json(bytes: Uint8Array): ${t.name} {\n` +
  `  return fromWire${t.name}(JSON.parse(new TextDecoder().decode(bytes)));\n}\n\n`;

// Enums appear as runtime index expressions (`Name[...]`), so they're imported
// as values, not types — but only the ones a field actually references.
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
