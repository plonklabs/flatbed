import type { CodecRoots, FbsEnum, FbsField, FbsSchema, FbsTable, FbsType, FbsUnion } from "./model.js";
import { reachableTables } from "./reachable.js";
import { HEADER } from "./util.js";

// The JSON wire shape is the server's serde output: enums are variant-name
// strings, a `bit_flags` value is the list of its flag names, a union is a
// `<field>_type` variant-name key beside the `<field>` value, and 64-bit ints
// are JSON numbers, so values above 2^53 lose precision on this path — a limit
// of the JSON number type.

const is64 = (t: FbsType): boolean =>
  t.kind === "scalar" && (t.scalar === "int64" || t.scalar === "uint64");

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
  if (t.kind === "flags") return `${expr} as ${t.name}[]`;
  return is64(t) ? `BigInt(${expr} as number)` : `${expr} as ${passthroughTs(t)}`;
};

// String/table/vector fields are optional; guard so an absent one stays `undefined` (which `JSON.stringify` drops).
const optional = (t: FbsType): boolean =>
  t.kind === "string" || t.kind === "table" || t.kind === "vector";

/** The flag names set in a field's default mask, as a TS array literal. */
const flagsDefault = (enums: ReadonlyMap<string, FbsEnum>, f: FbsField): string => {
  const mask = f.default.kind === "int" ? f.default.value : 0n;
  const members = f.type.kind === "flags" ? (enums.get(f.type.name)?.members ?? []) : [];
  return JSON.stringify(members.filter((m) => (mask & m.value) !== 0n).map((m) => m.name));
};

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

/** The `key: expr` entries one field contributes to its table's wire object. */
const toWireField = (f: FbsField): string => {
  const v = `value.${f.name}`;
  if (f.type.kind === "union") {
    return (
      `    ${f.name}_type: ${v}?.type,\n` +
      `    ${f.name}: ${v} != null ? toWire${f.type.name}(${v}) : undefined,\n`
    );
  }
  const conv = toWire(f.type, v);
  return `    ${f.name}: ${optional(f.type) && conv !== v ? `${v} != null ? ${conv} : undefined` : conv},\n`;
};

// flatc spells an absent union as the `NONE` variant.
const fromWireField = (enums: ReadonlyMap<string, FbsEnum>, f: FbsField): string => {
  const v = `obj.${f.name}`;
  if (f.type.kind === "union") {
    const kind = `obj.${f.name}_type`;
    return `${kind} != null && ${kind} !== "NONE" ? fromWire${f.type.name}(${kind}, ${v}) : undefined`;
  }
  const conv = fromWire(f.type, v);
  if (optional(f.type)) return `${v} != null ? ${conv} : undefined`;
  const absent = f.type.kind === "flags" ? flagsDefault(enums, f) : defaultLiteral(f);
  return `${v} != null ? ${conv} : ${absent}`;
};

// Emitted as hoisted `function` declarations, not arrows: a table's toWire/
// fromWire may call a nested table's before it's defined, since schema order
// isn't dependency-ordered and table references can be mutual.
const toWireFn = (t: FbsTable): string =>
  `function toWire${t.name}(value: ${t.name}): unknown {\n  return {\n` +
  t.fields.map(toWireField).join("") +
  "  };\n}\n\n";

const fromWireFn = (enums: ReadonlyMap<string, FbsEnum>, t: FbsTable): string =>
  `function fromWire${t.name}(obj: Record<string, unknown>): ${t.name} {\n  return {\n` +
  t.fields.map((f) => `    ${f.name}: ${fromWireField(enums, f)},\n`).join("") +
  "  };\n}\n\n";

const toWireUnionFn = (u: FbsUnion): string =>
  `function toWire${u.name}(value: ${u.name}): unknown {\n  switch (value.type) {\n` +
  u.members
    .map((m) => `    case ${JSON.stringify(m.name)}:\n      return toWire${m.table}(value.value);\n`)
    .join("") +
  "  }\n}\n\n";

const fromWireUnionFn = (u: FbsUnion): string =>
  `function fromWire${u.name}(type: unknown, value: unknown): ${u.name} {\n  switch (type) {\n` +
  u.members
    .map(
      (m) =>
        `    case ${JSON.stringify(m.name)}:\n` +
        `      return { type: ${JSON.stringify(m.name)}, value: fromWire${m.table}(value as Record<string, unknown>) };\n`,
    )
    .join("") +
  `    default:\n      throw new Error(\`unknown ${u.name} variant: \${String(type)}\`);\n  }\n}\n\n`;

const encodeJsonFn = (t: FbsTable): string =>
  `export function encode${t.name}Json(value: ${t.name}): Uint8Array {\n` +
  `  return new TextEncoder().encode(JSON.stringify(toWire${t.name}(value)));\n}\n\n`;

const decodeJsonFn = (t: FbsTable): string =>
  `export function decode${t.name}Json(bytes: Uint8Array): ${t.name} {\n` +
  `  return fromWire${t.name}(JSON.parse(new TextDecoder().decode(bytes)) as Record<string, unknown>);\n}\n\n`;

// Both toWire and fromWire name the enum at runtime (`Name[...]`), so a table on
// either side needs its enums as value imports; import only those referenced.
const referenced = (tables: readonly FbsTable[], kind: "enum" | "flags" | "union"): ReadonlySet<string> => {
  const names = new Set<string>();
  const visit = (t: FbsType): void => {
    if (t.kind === kind) names.add(t.name);
    if (t.kind === "vector") visit(t.element);
  };
  tables.forEach((table) => table.fields.forEach((f) => visit(f.type)));
  return names;
};

/**
 * Emit the JSON encode/decode that matches the server's serde wire shape. A
 * table gets a `toWire…`/`encode…Json` when it's reachable from a request body
 * and a `fromWire…`/`decode…Json` when reachable from a response body; the
 * exported `…Json` entry points are limited to the body types themselves.
 */
export const emitJson = (schema: FbsSchema, roots: CodecRoots): string => {
  const enumsByName = new Map(schema.enums.map((e) => [e.name, e]));
  const encodeSet = reachableTables(schema, roots.encodeRoots);
  const decodeSet = reachableTables(schema, roots.decodeRoots);
  const body = schema.tables
    .map((t) =>
      (encodeSet.has(t.name) ? toWireFn(t) : "") +
      (roots.encodeRoots.has(t.name) ? encodeJsonFn(t) : "") +
      (decodeSet.has(t.name) ? fromWireFn(enumsByName, t) : "") +
      (roots.decodeRoots.has(t.name) ? decodeJsonFn(t) : ""),
    )
    .join("");
  const emitted = schema.tables.filter((t) => encodeSet.has(t.name) || decodeSet.has(t.name));
  const encodedUnions = referenced(schema.tables.filter((t) => encodeSet.has(t.name)), "union");
  const decodedUnions = referenced(schema.tables.filter((t) => decodeSet.has(t.name)), "union");
  const unions = (schema.unions ?? []).filter((u) => encodedUnions.has(u.name) || decodedUnions.has(u.name));
  const unionFns = unions
    .map(
      (u) =>
        (encodedUnions.has(u.name) ? toWireUnionFn(u) : "") + (decodedUnions.has(u.name) ? fromWireUnionFn(u) : ""),
    )
    .join("");
  const enumSet = referenced(emitted, "enum");
  const enums = schema.enums.filter((e) => enumSet.has(e.name)).map((e) => e.name);
  // Flags are only named in a decode-side `as` cast; unions in their helpers.
  const decodedFlags = referenced(schema.tables.filter((t) => decodeSet.has(t.name)), "flags");
  const typeImports = [
    ...emitted.map((t) => t.name),
    ...schema.enums.filter((e) => decodedFlags.has(e.name)).map((e) => e.name),
    ...unions.map((u) => u.name),
  ];
  return (
    HEADER +
    (enums.length > 0 ? `import { ${enums.join(", ")} } from "./types.js";\n` : "") +
    (typeImports.length > 0 ? `import type { ${typeImports.join(", ")} } from "./types.js";\n` : "") +
    "\n" +
    unionFns +
    body
  );
};
