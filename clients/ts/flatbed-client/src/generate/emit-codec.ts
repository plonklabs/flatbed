import type { CodecRoots, FbsEnum, FbsField, FbsSchema, FbsTable, FbsType, FbsUnion } from "./model.js";
import { reachableTables, referenced } from "./reachable.js";
import { HEADER } from "./util.js";
import { SCALAR_OPS, type ScalarOps } from "./wire.js";

/** Lookups the emitters share, gathered once from the schema. */
interface Ctx {
  readonly enums: ReadonlyMap<string, FbsEnum>;
  readonly unions: ReadonlyMap<string, FbsUnion>;
}

const ctxOf = (schema: FbsSchema): Ctx => ({
  enums: new Map(schema.enums.map((e) => [e.name, e])),
  unions: new Map((schema.unions ?? []).map((u) => [u.name, u])),
});

const enumOf = (ctx: Ctx, name: string): FbsEnum => {
  const en = ctx.enums.get(name);
  if (en === undefined) throw new Error(`unknown enum ${name}`);
  return en;
};

/** The scalar ops backing a scalar/enum/flags wire type (an enum → its underlying). */
const opsFor = (ctx: Ctx, t: FbsType): ScalarOps => {
  if (t.kind === "scalar") return SCALAR_OPS[t.scalar];
  if (t.kind !== "enum" && t.kind !== "flags") throw new Error(`no scalar ops for wire kind ${t.kind}`);
  return SCALAR_OPS[enumOf(ctx, t.name).underlying];
};

const is64 = (ops: ScalarOps): boolean => ops.zero.endsWith("n");

// The flag helpers work in `bigint` so one shape covers every width; a field
// narrower than 64 bits converts at the call site.
const encodeFlags = (ctx: Ctx, name: string, expr: string): string =>
  is64(opsFor(ctx, { kind: "flags", name })) ? `encode${name}(${expr})` : `Number(encode${name}(${expr}))`;

const decodeFlags = (ctx: Ctx, name: string, read: string): string =>
  is64(opsFor(ctx, { kind: "flags", name })) ? `decode${name}(${read})` : `decode${name}(BigInt(${read}))`;

/** The flag names set in `mask`, in declaration order, as a TS array literal. */
const flagNames = (ctx: Ctx, name: string, mask: bigint): string =>
  JSON.stringify(
    enumOf(ctx, name)
      .members.filter((m) => (mask & m.value) !== 0n)
      .map((m) => m.name),
  );

/** The `n` suffix a scalar/enum literal needs when it is 64-bit backed. */
const suffix = (ctx: Ctx, t: FbsType): string => (is64(opsFor(ctx, t)) ? "n" : "");

/**
 * The wire default a field is compared against (encode's omit-if-equal) and the
 * value it decodes to when absent, as TS literals. The reader already resolved
 * an enum's declared default to its integer value.
 */
const defaults = (ctx: Ctx, f: FbsField): { readonly encode: string; readonly decode: string } => {
  const { type: t, default: d } = f;
  if (t.kind === "scalar" && t.scalar === "bool") {
    const on = d.kind === "int" && d.value !== 0n;
    return { encode: on ? "1" : "0", decode: on ? "true" : "false" };
  }
  if (t.kind === "enum") {
    const lit = `${d.kind === "int" ? d.value : 0n}${suffix(ctx, t)}`;
    return { encode: lit, decode: lit };
  }
  if (t.kind === "flags") {
    const mask = d.kind === "int" ? d.value : 0n;
    return { encode: `${mask}${suffix(ctx, t)}`, decode: flagNames(ctx, t.name, mask) };
  }
  if (d.kind === "none") {
    throw new Error(`unreachable: scalar/enum field '${f.name}' has no wire default`);
  }
  const lit = d.kind === "int" ? `${d.value}${suffix(ctx, t)}` : `${d.value}`;
  return { encode: lit, decode: lit };
};

const vectorIife = (stmts: readonly string[]): string =>
  "(() => {\n" +
  stmts.map((s) => `        ${s}\n`).join("") +
  "        return builder.endVector();\n" +
  "      })()";

const buildVector = (ctx: Ctx, expr: string, element: FbsType): string => {
  if (element.kind === "table") {
    return vectorIife([
      `const o = ${expr}.map((x) => encode${element.name}(builder, x));`,
      "builder.startVector(4, o.length, 4);",
      "[...o].reverse().forEach((off) => builder.addOffset(off));",
    ]);
  }
  if (element.kind === "string") {
    return vectorIife([
      `const o = ${expr}.map((x) => builder.createString(x));`,
      "builder.startVector(4, o.length, 4);",
      "[...o].reverse().forEach((off) => builder.addOffset(off));",
    ]);
  }
  const ops = opsFor(ctx, element);
  const elem =
    element.kind === "scalar" && element.scalar === "bool"
      ? "x ? 1 : 0"
      : element.kind === "flags"
        ? encodeFlags(ctx, element.name, "x")
        : "x";
  return vectorIife([
    `const a = ${expr};`,
    `builder.startVector(${ops.size}, a.length, ${ops.size});`,
    `[...a].reverse().forEach((x) => builder.${ops.vecAdd}(${elem}));`,
  ]);
};

/** `[prep lines, addField line]` for one field's encode. */
const encodeField = (ctx: Ctx, f: FbsField): readonly [string, string] => {
  const val = `value.${f.name}`;
  const off = `${f.name}Offset`;
  const offsetAdd = `  if (${off}) builder.addFieldOffset(${f.id}, ${off}, 0);\n`;
  const t = f.type;
  if (t.kind === "vector") {
    return [
      `  const ${off} = ${val} != null\n    ? ${buildVector(ctx, val, t.element)}\n    : 0;\n`,
      offsetAdd,
    ];
  }
  if (t.kind === "table") {
    return [`  const ${off} = ${val} != null ? encode${t.name}(builder, ${val}) : 0;\n`, offsetAdd];
  }
  if (t.kind === "string") {
    return [`  const ${off} = ${val} != null ? builder.createString(${val}) : 0;\n`, offsetAdd];
  }
  if (t.kind === "union") {
    // The discriminator lives in the slot just before the value's own id.
    const u = `${f.name}Union`;
    return [
      `  const ${u} = ${val} != null ? encode${t.name}(builder, ${val}) : undefined;\n`,
      `  if (${u}) {\n` +
        `    builder.addFieldInt8(${f.id - 1}, ${u}[0], 0);\n` +
        `    builder.addFieldOffset(${f.id}, ${u}[1], 0);\n` +
        "  }\n",
    ];
  }
  const ops = opsFor(ctx, t);
  const arg =
    t.kind === "scalar" && t.scalar === "bool"
      ? `${val} ? 1 : 0`
      : t.kind === "flags"
        ? encodeFlags(ctx, t.name, val)
        : val;
  return ["", `  builder.${ops.addField}(${f.id}, ${arg}, ${defaults(ctx, f).encode});\n`];
};

// The vtable needs one slot per field id (not per field): gapped explicit ids
// allocate up to `max id + 1` slots.
const slotCount = (t: FbsTable): number =>
  t.fields.reduce((max, f) => Math.max(max, f.id + 1), 0);

const encodeFn = (ctx: Ctx, t: FbsTable): string => {
  const [preps, adds] = t.fields
    .map((f) => encodeField(ctx, f))
    .reduce<[string, string]>(([p, a], [fp, fa]) => [p + fp, a + fa], ["", ""]);
  return (
    `export function encode${t.name}(builder: flatbuffers.Builder, value: ${t.name}): flatbuffers.Offset {\n` +
    preps +
    `  builder.startObject(${slotCount(t)});\n` +
    adds +
    "  return builder.endObject();\n}\n\n"
  );
};

// Only a request body is encoded from the top; nested tables are reached through
// their parent's `encode…`, so they never need a Root entry point.
const encodeRootFn = (t: FbsTable): string =>
  `export function encode${t.name}Root(value: ${t.name}): Uint8Array {\n` +
  "  const builder = new flatbuffers.Builder();\n" +
  `  builder.finish(encode${t.name}(builder, value));\n` +
  "  return builder.asUint8Array();\n}\n\n";

/** The TS expression reading the i-th vector element from `base`. */
const decodeElement = (ctx: Ctx, element: FbsType): string => {
  if (element.kind === "table") return `decode${element.name}(bb, bb.__indirect(base + i * 4))`;
  if (element.kind === "string") return "bb.__string(base + i * 4) as string";
  if (element.kind === "scalar" && element.scalar === "bool") return "bb.readInt8(base + i) !== 0";
  const ops = opsFor(ctx, element);
  const read = `bb.${ops.read}(base + i * ${ops.size})`;
  if (element.kind === "flags") return decodeFlags(ctx, element.name, read);
  return element.kind === "enum" ? `${read} as ${element.name}` : read;
};

/** The TS expression reading one field, given `{name}_o` (its vtable offset). */
const decodeField = (ctx: Ctx, f: FbsField): string => {
  const o = `${f.name}_o`;
  const t = f.type;
  if (t.kind === "vector") {
    const tsElem = decodeElementType(t.element);
    return (
      `${o}\n` +
      `      ? (() => {\n` +
      `          const len = bb.__vector_len(pos + ${o});\n` +
      `          const base = bb.__vector(pos + ${o});\n` +
      `          return Array.from({ length: len }, (_, i): ${tsElem} => ${decodeElement(ctx, t.element)});\n` +
      `        })()\n` +
      `      : undefined`
    );
  }
  if (t.kind === "table") return `${o} ? decode${t.name}(bb, bb.__indirect(pos + ${o})) : undefined`;
  if (t.kind === "string") return `${o} ? (bb.__string(pos + ${o}) as string) : undefined`;
  if (t.kind === "union") {
    const to = `${f.name}_type_o`;
    return `${o} && ${to} ? decode${t.name}(bb, bb.readUint8(pos + ${to}), bb.__indirect(pos + ${o})) : undefined`;
  }
  const dec = defaults(ctx, f).decode;
  if (t.kind === "scalar" && t.scalar === "bool") return `${o} ? bb.readInt8(pos + ${o}) !== 0 : ${dec}`;
  const ops = opsFor(ctx, t);
  if (t.kind === "enum") return `(${o} ? bb.${ops.read}(pos + ${o}) : ${dec}) as ${t.name}`;
  if (t.kind === "flags") return `${o} ? ${decodeFlags(ctx, t.name, `bb.${ops.read}(pos + ${o})`)} : ${dec}`;
  return `${o} ? bb.${ops.read}(pos + ${o}) : ${dec}`;
};

/** The TS type of a vector element, for the decoded array's annotation. */
const decodeElementType = (t: FbsType): string => {
  if (t.kind === "table" || t.kind === "enum") return t.name;
  if (t.kind === "flags") return `${t.name}[]`;
  if (t.kind === "string") return "string";
  if (t.kind === "scalar") return SCALAR_OPS[t.scalar].ts;
  return "unknown";
};

const decodeFn = (ctx: Ctx, t: FbsTable): string => {
  const offset = (name: string, id: number): string => `  const ${name}_o = bb.__offset(pos, ${4 + id * 2});\n`;
  const offsets = t.fields
    .map((f) => (f.type.kind === "union" ? offset(`${f.name}_type`, f.id - 1) : "") + offset(f.name, f.id))
    .join("");
  const fields = t.fields.map((f) => `    ${f.name}: ${decodeField(ctx, f)},\n`).join("");
  return (
    `export function decode${t.name}(bb: flatbuffers.ByteBuffer, pos: number): ${t.name} {\n` +
    offsets +
    "  return {\n" +
    fields +
    "  };\n}\n\n"
  );
};

const decodeRootFn = (t: FbsTable): string =>
  `export function decode${t.name}Root(bytes: Uint8Array): ${t.name} {\n` +
  "  const bb = new flatbuffers.ByteBuffer(bytes);\n" +
  `  return decode${t.name}(bb, bb.__indirect(bb.position()));\n}\n\n`;

const flagsTableFn = (e: FbsEnum): string =>
  `const ${e.name}_FLAGS: Readonly<Record<${e.name}, bigint>> = {\n` +
  e.members.map((m) => `  ${m.name}: ${m.value}n,\n`).join("") +
  "};\n\n";

const encodeFlagsFn = (e: FbsEnum): string =>
  `function encode${e.name}(names: readonly ${e.name}[]): bigint {\n` +
  `  return names.reduce((bits, n) => bits | ${e.name}_FLAGS[n], 0n);\n}\n\n`;

// Bits the schema doesn't name have no flag name to decode to, so they are an
// error rather than silently dropped.
const decodeFlagsFn = (e: FbsEnum): string =>
  `function decode${e.name}(bits: bigint): ${e.name}[] {\n` +
  `  const names = (Object.keys(${e.name}_FLAGS) as ${e.name}[]).filter((n) => (bits & ${e.name}_FLAGS[n]) !== 0n);\n` +
  `  const unknown = names.reduce((rest, n) => rest & ~${e.name}_FLAGS[n], bits);\n` +
  `  if (unknown !== 0n) throw new Error(\`unknown ${e.name} bits: 0x\${unknown.toString(16)}\`);\n` +
  "  return names;\n}\n\n";

const encodeUnionFn = (u: FbsUnion): string =>
  `function encode${u.name}(builder: flatbuffers.Builder, value: ${u.name}): readonly [number, flatbuffers.Offset] {\n` +
  "  switch (value.type) {\n" +
  u.members
    .map((m) => `    case ${JSON.stringify(m.name)}:\n      return [${m.value}, encode${m.table}(builder, value.value)];\n`)
    .join("") +
  "  }\n}\n\n";

// An unknown discriminator (a variant newer than this schema) decodes as absent.
const decodeUnionFn = (u: FbsUnion): string =>
  `function decode${u.name}(bb: flatbuffers.ByteBuffer, type: number, pos: number): ${u.name} | undefined {\n` +
  "  switch (type) {\n" +
  u.members
    .map(
      (m) =>
        `    case ${m.value}:\n      return { type: ${JSON.stringify(m.name)}, value: decode${m.table}(bb, pos) };\n`,
    )
    .join("") +
  "    default:\n      return undefined;\n  }\n}\n\n";

/**
 * The flag and union helpers the emitted tables call: an encode helper for a
 * type an encoded table references, a decode helper for a decoded one, so none
 * is left unused.
 */
const helpers = (ctx: Ctx, encoded: readonly FbsTable[], decoded: readonly FbsTable[]): string => {
  const [encFlags, decFlags] = [referenced(encoded, "flags"), referenced(decoded, "flags")];
  const [encUnions, decUnions] = [referenced(encoded, "union"), referenced(decoded, "union")];
  const flags = [...ctx.enums.values()]
    .filter((e) => encFlags.has(e.name) || decFlags.has(e.name))
    .map(
      (e) =>
        flagsTableFn(e) +
        (encFlags.has(e.name) ? encodeFlagsFn(e) : "") +
        (decFlags.has(e.name) ? decodeFlagsFn(e) : ""),
    );
  const unions = [...ctx.unions.values()].map(
    (u) => (encUnions.has(u.name) ? encodeUnionFn(u) : "") + (decUnions.has(u.name) ? decodeUnionFn(u) : ""),
  );
  return [...flags, ...unions].join("");
};

/**
 * Emit the encode/decode functions over the `flatbuffers` runtime. A table gets
 * an `encode…` when it's reachable from a request body and a `decode…` when
 * reachable from a response body; the `…Root` entry points are limited to the
 * body types themselves, so the dead direction of every type is never emitted.
 */
export const emitCodec = (schema: FbsSchema, roots: CodecRoots): string => {
  const ctx = ctxOf(schema);
  const encodeSet = reachableTables(schema, roots.encodeRoots);
  const decodeSet = reachableTables(schema, roots.decodeRoots);
  const body = schema.tables
    .map((t) =>
      (encodeSet.has(t.name) ? encodeFn(ctx, t) : "") +
      (roots.encodeRoots.has(t.name) ? encodeRootFn(t) : "") +
      (decodeSet.has(t.name) ? decodeFn(ctx, t) : "") +
      (roots.decodeRoots.has(t.name) ? decodeRootFn(t) : ""),
    )
    .join("");
  const emitted = schema.tables.filter((t) => encodeSet.has(t.name) || decodeSet.has(t.name));
  const encoded = schema.tables.filter((t) => encodeSet.has(t.name));
  const decoded = schema.tables.filter((t) => decodeSet.has(t.name));
  // A plain enum only appears in the codec as an `as Name` cast on the decode
  // side (encode writes the underlying integer), so only decoded tables import
  // it; flag and union types are named by their helpers in either direction.
  const usedEnums = referenced(decoded, "enum");
  const usedFlags = referenced(emitted, "flags");
  const usedUnions = referenced(emitted, "union");
  const typeList = [
    ...emitted.map((t) => t.name),
    ...schema.enums.filter((e) => usedEnums.has(e.name) || usedFlags.has(e.name)).map((e) => e.name),
    ...(schema.unions ?? []).filter((u) => usedUnions.has(u.name)).map((u) => u.name),
  ].join(", ");
  return (
    HEADER +
    // Only table encode/decode uses the runtime; with nothing emitted the import
    // would be unused and `noUnusedLocals` would reject the generated file.
    (emitted.length > 0 ? 'import * as flatbuffers from "flatbuffers";\n' : "") +
    (typeList.length > 0 ? `import type { ${typeList} } from "./types.js";\n` : "") +
    "\n" +
    helpers(ctx, encoded, decoded) +
    body
  );
};
